use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::ops::Range;

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{ExprClosure, Local, Pat, Type};

use super::{
    check_rust_source_json, extract_found_type, probe_closure_param_type, probe_local_type,
};
use crate::error::{RSharpError, Result};

struct Site {
    key: usize,
    name: String,
    insertion: Range<usize>,
    individual: OnceCell<Result<String>>,
}

#[derive(Default)]
struct Sites(Vec<Site>);

impl Sites {
    fn insert(&mut self, pat: &Pat, insertion: Range<usize>) {
        if let Pat::Ident(binding) = pat {
            let key = binding.ident.span().byte_range().start;
            self.0.push(Site {
                key,
                name: binding.ident.to_string(),
                insertion,
                individual: OnceCell::new(),
            });
        }
    }
}

impl<'ast> Visit<'ast> for Sites {
    fn visit_local(&mut self, local: &'ast Local) {
        if local.init.is_some() {
            let pat = match &local.pat {
                Pat::Type(t) if matches!(*t.ty, Type::Infer(_)) => &*t.pat,
                other => other,
            };
            let pat = match pat {
                Pat::Paren(p) => &*p.pat,
                other => other,
            };
            let end = local.semi_token.span.byte_range().end;
            self.insert(pat, end..end);
        }
        visit::visit_local(self, local);
    }

    fn visit_expr_closure(&mut self, closure: &'ast ExprClosure) {
        let body = closure.body.span().byte_range();
        for pat in &closure.inputs {
            self.insert(pat, body.clone());
        }
        visit::visit_expr_closure(self, closure);
    }
}

pub(crate) struct TypeProbes<'a> {
    source: &'a str,
    file: &'a syn::File,
    sites: OnceCell<Sites>,
    types: OnceCell<Result<BTreeMap<usize, String>>>,
}

impl<'a> TypeProbes<'a> {
    pub(crate) fn new(source: &'a str, file: &'a syn::File) -> Self {
        // syn parses the bytes after the BOM and shebang, so its spans start there.
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        let source = &source[file.shebang.as_ref().map_or(0, String::len)..];
        Self {
            source,
            file,
            sites: OnceCell::new(),
            types: OnceCell::new(),
        }
    }

    pub(crate) fn infer(&self, pat: &Pat, name: &str, closure: bool) -> Result<String> {
        let sites = self.sites.get_or_init(|| {
            let mut sites = Sites::default();
            sites.visit_file(self.file);
            sites.0.sort_unstable_by_key(|s| s.key);
            sites
        });
        let site = match pat {
            Pat::Ident(p) => sites
                .0
                .binary_search_by_key(&p.ident.span().byte_range().start, |s| s.key)
                .ok()
                .map(|i| &sites.0[i]),
            _ => None,
        };
        let Some(site) = site else {
            return if closure {
                probe_closure_param_type(self.source, name, 1)
            } else {
                probe_local_type(self.source, name, 1)
            };
        };
        if sites.0.len() > 1 {
            let types = self.types.get_or_init(|| {
                // shortcut: probe files mentioning "column" individually to preserve column!(), narrow the match if profiling warrants it.
                if self.source.contains("column") {
                    Ok(BTreeMap::new())
                } else {
                    self.compile(&sites.0)
                }
            });
            if let Some(ty) = types.as_ref().map_err(Clone::clone)?.get(&site.key) {
                return Ok(ty.clone());
            }
        }
        site.individual
            .get_or_init(|| {
                self.compile(std::slice::from_ref(site))?
                    .remove(&site.key)
                    .ok_or_else(|| {
                        RSharpError::unresolved_type(
                            name,
                            "rustc could not infer concrete type (external crate or compile error)",
                        )
                    })
            })
            .clone()
    }

    fn compile(&self, sites: &[Site]) -> Result<BTreeMap<usize, String>> {
        let batch = sites.len() > 1;
        let mut edits = Vec::new();
        for site in sites {
            edits.push((site.insertion.start, Some(site)));
            if !site.insertion.is_empty() {
                edits.push((site.insertion.end, None));
            }
        }
        edits.sort_by_key(|(pos, site)| (*pos, site.is_some()));
        let mut source = String::with_capacity(self.source.len());
        let mut anchors = BTreeMap::new();
        let mut cursor = 0;
        for (pos, site) in edits {
            source.push_str(&self.source[cursor..pos]);
            cursor = pos;
            if let Some(site) = site {
                if !site.insertion.is_empty() {
                    source.push('{');
                }
                let marker = format!("__RSharpProbe{}", site.key);
                source.push_str(&format!(" {{ struct {marker} {{}} let _: {marker} = "));
                anchors.insert(source.len(), site.key);
                // Borrowing makes the mismatch independent of other bindings' inferred types.
                if batch {
                    source.push('&');
                }
                source.push_str(&site.name);
                source.push_str("; } ");
            } else {
                source.push('}');
            }
        }
        source.push_str(&self.source[cursor..]);
        let is_lib = !self.source.contains("fn main(") && !self.source.contains("fn main (");
        let output = check_rust_source_json(&source, is_lib)?;
        Ok(output
            .messages
            .iter()
            .filter_map(|message| diagnostic_type(message, &anchors, batch))
            .collect())
    }
}

fn diagnostic_type(
    message: &str,
    anchors: &BTreeMap<usize, usize>,
    batch: bool,
) -> Option<(usize, String)> {
    let diagnostic: serde_json::Value = serde_json::from_str(message).ok()?;
    if diagnostic["code"]["code"] != "E0308" {
        return None;
    }
    let primary = diagnostic["spans"]
        .as_array()?
        .iter()
        .find(|span| span["is_primary"] == true)?;
    let start = primary["byte_start"].as_u64()? as usize;
    let &key = anchors.get(&start)?;
    let marker = format!("__RSharpProbe{key}");
    let notes = diagnostic["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|child| child["message"].as_str());
    let ty = primary["label"]
        .as_str()
        .into_iter()
        .chain(notes)
        .find_map(|text| extract_found_type(text, &marker))?;
    if !batch {
        return Some((key, ty));
    }
    let inner = ty.strip_prefix('&')?;
    if inner
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|t| t == "_")
    {
        return None;
    }
    syn::parse_str::<Type>(inner).ok()?;
    Some((key, inner.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rustc_runner::COMPILATIONS;
    use crate::{FromRustOptions, convert_from_rust};

    #[test]
    fn conversion_only_launches_rustc_when_needed() {
        let direct = "fn f(x: i32) -> i32 { let y = x + 1; y }";
        let convert = |src| convert_from_rust(src, FromRustOptions::default());
        COMPILATIONS.set(0);
        assert!(convert(direct).unwrap().contains("int y"));
        assert_eq!(COMPILATIONS.get(), 0);

        let single = "fn main() { let value = std::hint::black_box(7u64); }";
        assert!(convert(single).unwrap().contains("ulong value"));
        assert_eq!(COMPILATIONS.get(), 1);
        COMPILATIONS.set(0);

        let src = "\u{feff}#!/usr/bin/env rust-script\r\n// 🦀\r\n\
            fn f(a: &[i32], b: &[u64]) {\r\n\
                let x = a.iter().map(|v| *v).sum::<i32>();\r\n\
                let x = b.iter().map(|v| a.iter().map(|v| *v).sum::<i32>() as u64 + *v).sum::<u64>();\r\n\
                let _ = x;\r\n\
            }";
        let out = convert(src).unwrap();
        for snippet in ["int x =", "ulong x =", "read int v", "read ulong v"] {
            assert!(out.contains(snippet), "{snippet} missing from {out}");
        }
        assert_eq!(COMPILATIONS.get(), 1);
        assert!(
            convert_from_rust(
                src,
                FromRustOptions {
                    use_rustc_inference: false
                }
            )
            .is_err()
        );
        assert_eq!(COMPILATIONS.get(), 1);
    }

    #[test]
    fn inferred_array_lengths_keep_the_original_source_location() {
        for location in ["line", "column"] {
            let src = format!(
                "fn f() {{ let first = 1; let a = std::hint::black_box([0u8; {location}!() as usize]); }}"
            );
            let expected = if location == "column" {
                src.find("column!").unwrap() + 1
            } else {
                1
            };
            let out = convert_from_rust(&src, FromRustOptions::default()).unwrap();
            assert!(out.contains(&format!("byte[{expected}] a")), "{out}");
        }
    }

    #[test]
    fn missing_batch_diagnostic_retries_only_the_unresolved_binding() {
        let source = "fn main () { let first: u8; first = 1; let (n): _ = std::hint::black_box(first); let n = missing(); }";
        COMPILATIONS.set(0);
        let err = convert_from_rust(source, FromRustOptions::default()).unwrap_err();
        assert!(
            err.to_string()
                .contains("rustc could not infer concrete type")
        );
        assert_eq!(COMPILATIONS.get(), 2);
    }

    #[test]
    fn ambiguous_or_unrelated_diagnostics_do_not_supply_a_type() {
        use serde_json::json;
        let anchors = BTreeMap::from([(12, 4)]);
        assert_eq!(
            diagnostic_type("unexpected compiler output", &anchors, true),
            None
        );
        for spans in [json!(null), json!([]), json!([{"is_primary":true}])] {
            let message = json!({"code":{"code":"E0308"},"spans":spans});
            assert_eq!(diagnostic_type(&message.to_string(), &anchors, true), None);
        }
        let mut msg =
            json!({"code":{"code":"E0308"}, "spans":[{"is_primary":true,"byte_start":12}]});
        for label in [
            "expected `Other`, found `&i32`",
            "expected `__RSharpProbe4`, found `i32`",
            "expected `__RSharpProbe4`, found `&Vec<_>`",
            "expected `__RSharpProbe4`, found `&{closure}`",
        ] {
            msg["spans"][0]["label"] = json!(label);
            assert_eq!(diagnostic_type(&msg.to_string(), &anchors, true), None);
        }
        msg["children"] =
            json!([{"message":"expected struct `__RSharpProbe4`\nfound reference `&i64`"}]);
        msg["spans"][0]["label"] = json!("mismatched types");
        assert_eq!(
            diagnostic_type(&msg.to_string(), &anchors, true),
            Some((4, "i64".into()))
        );
        msg["spans"][0]["byte_start"] = json!(99);
        assert_eq!(diagnostic_type(&msg.to_string(), &anchors, true), None);
    }

    #[test]
    fn non_identifier_patterns_preserve_the_probe_error() {
        let source = "fn f() {}";
        let file = syn::parse_file(source).unwrap();
        let probes = TypeProbes::new(source, &file);
        let wildcard: Pat = syn::parse_quote!(_);
        for closure in [false, true] {
            assert!(probes.infer(&wildcard, "_", closure).is_err());
        }
    }
}
