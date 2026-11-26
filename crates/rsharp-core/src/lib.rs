pub mod ast;
pub mod codegen_rust;
pub mod error;
pub mod from_rust;
pub mod lexer;
pub mod parallel;
pub mod parser;
pub mod ra_oracle;
pub mod rustc_runner;
pub mod token;

use std::path::Path;

pub use codegen_rust::{PreludeMode, TranspileOptions};
pub use error::{RSharpError, Result, Span};
pub use from_rust::{FromRustOptions, convert_from_rust_with_hints};
pub use parallel::{
    ProjectConvertOptions, ProjectFile, ProjectFileOutcome, convert_project_files,
    convert_project_files_async,
};
pub use ra_oracle::{
    RustAnalyzerOracle, TypeHintMap, discover_cargo_rs_files, find_cargo_workspace_root,
    parse_cargo_metadata_rs_files,
};
pub use rustc_runner::{RunOutput, RustcJsonOutput};

fn has_candidate_rust_block(source: &str) -> bool {
    let bytes = source.as_bytes();
    let mut offset = 0;
    while let Some(rel) = source[offset..].find("rust") {
        let start = offset + rel;
        let end = start + 4;
        let left_ok =
            start == 0 || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        if left_ok && source[end..].bytes().find(|b| !b.is_ascii_whitespace()) == Some(b'{') {
            return true;
        }
        offset = end;
    }
    false
}

/// Counts the number of verbatim `rust { ... }` blocks in R# source code.
pub fn count_raw_rust_blocks(source: &str) -> usize {
    if !has_candidate_rust_block(source) {
        return 0;
    }
    lexer::tokenize(source)
        .map(|toks| {
            toks.iter()
                .filter(|t| matches!(t.kind, token::TokenKind::RustBlock(_)))
                .count()
        })
        .unwrap_or(0)
}

/// Transpiles R# (`.rsharp`) source code directly to Rust source code.
pub fn transpile_to_rust(source: &str, options: TranspileOptions) -> Result<String> {
    let ast = parser::parse_file(source)?;
    codegen_rust::generate_rust(&ast, options)
}

/// Converts Rust (`.rs`) source code into idiomatic R# (`.rsharp`) source code.
pub fn convert_from_rust(rust_source: &str, options: FromRustOptions) -> Result<String> {
    from_rust::convert_from_rust(rust_source, options)
}

/// Transpiles and compiles an R# source string to a native binary using `rustc`.
pub fn build_rsharp(source: &str, out_bin: &Path) -> Result<()> {
    let rust_src = transpile_to_rust(
        source,
        TranspileOptions {
            prelude_mode: PreludeMode::Embedded,
        },
    )?;
    rustc_runner::compile_rust_to_bin(&rust_src, out_bin)
}

/// Transpiles, compiles, and executes an R# source string via `rustc`.
pub fn run_rsharp(source: &str) -> Result<RunOutput> {
    let rust_src = transpile_to_rust(
        source,
        TranspileOptions {
            prelude_mode: PreludeMode::Embedded,
        },
    )?;
    rustc_runner::compile_and_run_rust(&rust_src)
}

fn is_library_rust_source(rust_src: &str) -> bool {
    !rust_src.contains("fn main(") && !rust_src.contains("fn main (")
}

/// Transpiles and semantically checks an R# source string via `rustc --emit=metadata`.
pub fn check_rsharp(source: &str) -> Result<()> {
    let rust_src = transpile_to_rust(
        source,
        TranspileOptions {
            prelude_mode: PreludeMode::Embedded,
        },
    )?;
    rustc_runner::check_rust_source(&rust_src, is_library_rust_source(&rust_src))
}

/// Transpiles and semantically checks an R# source string via `rustc --emit=metadata --error-format=json`,
/// returning both the generated Rust source (for diagnostic mapping) and raw JSON diagnostic lines.
pub fn check_rsharp_json(source: &str) -> Result<(String, RustcJsonOutput)> {
    let rust_src = transpile_to_rust(
        source,
        TranspileOptions {
            prelude_mode: PreludeMode::Embedded,
        },
    )?;
    let is_lib = is_library_rust_source(&rust_src);
    rustc_runner::check_rust_source_json(&rust_src, is_lib).map(|out| (rust_src, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_rsharp_json_covers_ok_and_syntax_error() {
        let (rust_src, ok) = check_rsharp_json("void main() { int x = 1; _ = x; }").unwrap();
        assert!(ok.success);
        assert!(rust_src.contains("fn main()"));

        let (_, lib_err) = check_rsharp_json("public int Bad() { return \"nope\"; }").unwrap();
        assert!(!lib_err.success);
        assert!(lib_err.messages.iter().any(|m| m.contains("E0308")));

        assert!(check_rsharp_json("void main() { let x = 1; }").is_err());
    }

    #[test]
    fn count_raw_rust_blocks_fast_path_and_all_bin_ops_covered() {
        assert_eq!(count_raw_rust_blocks("void main() { int x = 1; }"), 0);
        assert_eq!(
            count_raw_rust_blocks("void main() { int rust_val = 1; /* rust */ }"),
            0
        );
        assert_eq!(count_raw_rust_blocks("rust { type X; }"), 1);

        let all_ops = "\
fn ops(mut a: i32, b: i32, c: bool) -> i32 {
    let _ = a + b - a * b / 1 % 2;
    let _ = (c && !c) || (a == b) || (a != b) || (a < b) || (a <= b) || (a > b) || (a >= b);
    let _ = (a ^ b) & (a | b) | (a << 1) | (a >> 1);
    a += 1; a -= 1; a *= 2; a /= 2; a %= 3;
    a ^= 1; a &= 7; a |= 2; a <<= 1; a >>= 1;
    a
}
";
        let rsh = convert_from_rust(
            all_ops,
            FromRustOptions {
                use_rustc_inference: false,
            },
        )
        .unwrap();
        assert!(rsh.contains(">>="));
        assert!(rsh.contains("<<="));
    }
}
