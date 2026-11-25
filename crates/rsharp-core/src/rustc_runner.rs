use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{RSharpError, Result};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
const PROG_EXE: &str = "prog.exe";
#[cfg(not(windows))]
const PROG_EXE: &str = "prog";

/// Result of executing a compiled R# program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

fn prepare_temp_rust_file(
    base: &Path,
    prefix: &str,
    file_name: &str,
    content: &str,
) -> Result<(PathBuf, PathBuf)> {
    let dir = create_temp_dir_in(base, prefix)?;
    let rs_path = dir.join(file_name);
    write_temp_file(&rs_path, content)?;
    Ok((dir, rs_path))
}

fn create_temp_dir_in(base: &Path, prefix: &str) -> Result<PathBuf> {
    let id = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = base.join(format!("{}_{}_{}", prefix, std::process::id(), id));
    std::fs::create_dir_all(&dir)
        .map_err(|e| RSharpError::io(format!("Failed to create temp dir: {}", e)))?;
    Ok(dir)
}

fn write_temp_file(path: &Path, content: &str) -> Result<()> {
    std::fs::write(path, content)
        .map_err(|e| RSharpError::io(format!("Failed to write temp Rust file: {}", e)))
}

fn run_rustc_cmd(rustc_bin: &OsStr, args: &[&OsStr]) -> Result<Output> {
    Command::new(rustc_bin)
        .args(args)
        .output()
        .map_err(|e| RSharpError::compile(format!("Failed to invoke rustc: {}", e)))
}

fn run_binary_cmd(bin_path: &Path) -> Result<Output> {
    Command::new(bin_path)
        .output()
        .map_err(|e| RSharpError::io(format!("Failed to execute compiled binary: {}", e)))
}

/// Compiles generated Rust source code into a native executable at `out_bin`.
pub fn compile_rust_to_bin(rust_source: &str, out_bin: &Path) -> Result<()> {
    compile_rust_to_bin_in(
        &std::env::temp_dir(),
        OsStr::new("rustc"),
        rust_source,
        out_bin,
    )
}

fn compile_rust_to_bin_in(
    temp_base: &Path,
    rustc_bin: &OsStr,
    rust_source: &str,
    out_bin: &Path,
) -> Result<()> {
    let (dir, rs_path) = prepare_temp_rust_file(temp_base, "rsharp_build", "main.rs", rust_source)?;

    let output = run_rustc_cmd(
        rustc_bin,
        &[
            OsStr::new("--edition=2024"),
            rs_path.as_os_str(),
            OsStr::new("-o"),
            out_bin.as_os_str(),
        ],
    )?;

    let _ = std::fs::remove_dir_all(&dir);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(RSharpError::compile(stderr));
    }
    Ok(())
}

/// Type-checks generated Rust source via `rustc --emit=metadata` without producing an executable.
pub fn check_rust_source(rust_source: &str, is_lib: bool) -> Result<()> {
    check_rust_source_in(
        &std::env::temp_dir(),
        OsStr::new("rustc"),
        rust_source,
        is_lib,
    )
}

fn check_rust_source_in(
    temp_base: &Path,
    rustc_bin: &OsStr,
    rust_source: &str,
    is_lib: bool,
) -> Result<()> {
    let (dir, rs_path) =
        prepare_temp_rust_file(temp_base, "rsharp_check", "check.rs", rust_source)?;
    let meta_path = dir.join("check.rmeta");

    let mut args = vec![OsStr::new("--edition=2024"), OsStr::new("--emit=metadata")];
    if is_lib {
        args.push(OsStr::new("--crate-type=lib"));
    }
    args.push(rs_path.as_os_str());
    args.push(OsStr::new("-o"));
    args.push(meta_path.as_os_str());
    let output = run_rustc_cmd(rustc_bin, &args)?;

    let _ = std::fs::remove_dir_all(&dir);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(RSharpError::compile(stderr));
    }
    Ok(())
}

/// Compiles and executes generated Rust source, returning stdout, stderr, and exit code.
pub fn compile_and_run_rust(rust_source: &str) -> Result<RunOutput> {
    compile_and_run_rust_in(&std::env::temp_dir(), OsStr::new("rustc"), rust_source)
}

fn compile_and_run_rust_in(
    temp_base: &Path,
    rustc_bin: &OsStr,
    rust_source: &str,
) -> Result<RunOutput> {
    let dir = create_temp_dir_in(temp_base, "rsharp_run")?;
    let bin_path = dir.join(PROG_EXE);
    let res = compile_rust_to_bin_in(temp_base, rustc_bin, rust_source, &bin_path);
    if let Err(e) = res {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }

    let run = run_binary_cmd(&bin_path)?;

    let _ = std::fs::remove_dir_all(&dir);

    Ok(RunOutput {
        exit_code: run.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&run.stdout).to_string(),
        stderr: String::from_utf8_lossy(&run.stderr).to_string(),
    })
}

/// Probes `rustc` to recover the inferred concrete type of an unannotated closure parameter `|<param_name>| ...`.
pub fn probe_closure_param_type(rust_source: &str, param_name: &str, seq: usize) -> Result<String> {
    let marker = format!("__RSHARP_CPROBE_{}__", seq);
    let injected =
        inject_closure_param_probe(rust_source, param_name, &marker).ok_or_else(|| {
            RSharpError::unresolved_type(
                param_name,
                "Could not locate closure parameter for type probe",
            )
        })?;

    run_type_probe_in(
        &std::env::temp_dir(),
        OsStr::new("rustc"),
        &injected,
        rust_source,
        param_name,
        &marker,
    )
}

/// Probes `rustc` to recover the inferred concrete type of an unannotated local `let <var_name> = ...;`.
pub fn probe_local_type(rust_source: &str, var_name: &str, seq: usize) -> Result<String> {
    let marker = format!("__RSHARP_PROBE_{}__", seq);
    let injected = inject_type_probe(rust_source, var_name, &marker).ok_or_else(|| {
        RSharpError::unresolved_type(var_name, "Could not locate binding site for type probe")
    })?;

    run_type_probe_in(
        &std::env::temp_dir(),
        OsStr::new("rustc"),
        &injected,
        rust_source,
        var_name,
        &marker,
    )
}

fn run_type_probe_in(
    temp_base: &Path,
    rustc_bin: &OsStr,
    injected: &str,
    orig_source: &str,
    name: &str,
    marker: &str,
) -> Result<String> {
    let (dir, rs_path) = prepare_temp_rust_file(temp_base, "rsharp_probe", "probe.rs", injected)?;
    let meta_path = dir.join("probe.rmeta");

    let is_lib = !orig_source.contains("fn main(") && !orig_source.contains("fn main (");
    let mut args = vec![OsStr::new("--edition=2024"), OsStr::new("--emit=metadata")];
    if is_lib {
        args.push(OsStr::new("--crate-type=lib"));
    }
    args.push(rs_path.as_os_str());
    args.push(OsStr::new("-o"));
    args.push(meta_path.as_os_str());
    let output = run_rustc_cmd(rustc_bin, &args)?;

    let _ = std::fs::remove_dir_all(&dir);

    let stderr = String::from_utf8_lossy(&output.stderr);
    extract_found_type(&stderr, marker).ok_or_else(|| {
        RSharpError::unresolved_type(
            name,
            "rustc could not infer concrete type (external crate or compile error)",
        )
    })
}

fn inject_closure_param_probe(source: &str, param_name: &str, marker: &str) -> Option<String> {
    let patterns = [format!("|mut {}|", param_name), format!("|{}|", param_name)];
    let (start, needle_len) = patterns
        .iter()
        .find_map(|pat| source.find(pat).map(|pos| (pos, pat.len())))?;
    let body_start = start + needle_len;
    let rest = &source[body_start..];
    let trimmed_offset = rest.len() - rest.trim_start().len();
    let abs = body_start + trimmed_offset;
    let mut out = String::with_capacity(source.len() + 64);
    if source[abs..].starts_with('{') {
        out.push_str(&source[..abs + 1]);
        out.push_str(&format!(
            " struct {}; let _: {} = {};",
            marker, marker, param_name
        ));
        out.push_str(&source[abs + 1..]);
    } else {
        // Wrap single-expression closure body up to closing ')' or ',' or ';'
        let end_rel = find_expr_end(&source[abs..]);
        out.push_str(&source[..abs]);
        out.push_str(&format!(
            "{{ struct {}; let _: {} = {}; {} }}",
            marker,
            marker,
            param_name,
            &source[abs..abs + end_rel]
        ));
        out.push_str(&source[abs + end_rel..]);
    }
    Some(out)
}

fn find_expr_end(s: &str) -> usize {
    let mut depth = 0usize;
    for (idx, ch) in s.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    return idx;
                }
                depth -= 1;
            }
            ',' | ';' if depth == 0 => return idx,
            _ => {}
        }
    }
    s.len()
}

/// Injects `struct <marker>; let _: <marker> = <var_name>;` immediately after `let (mut )?<var_name> = ...;`.
fn inject_type_probe(source: &str, var_name: &str, marker: &str) -> Option<String> {
    let patterns = [
        format!("let mut {} =", var_name),
        format!("let {} =", var_name),
    ];
    for pat in &patterns {
        if let Some(start) = source.find(pat)
            && let Some(semi_rel) = source[start..].find(';')
        {
            let insert_pos = start + semi_rel + 1;
            let mut out = String::with_capacity(source.len() + 64);
            out.push_str(&source[..insert_pos]);
            out.push_str(&format!(
                " struct {}; let _: {} = {};",
                marker, marker, var_name
            ));
            out.push_str(&source[insert_pos..]);
            return Some(out);
        }
    }
    None
}

/// Extracts `<Type>` from rustc's `expected \`<marker>\`, found ...` diagnostic.
fn extract_found_type(stderr: &str, marker: &str) -> Option<String> {
    let needle = format!("expected `{}`, found ", marker);
    if let Some(idx) = stderr.find(&needle)
        && let Some(line) = stderr[idx + needle.len()..].lines().next()
    {
        if let Some(ty) = extract_backticked_type(line) {
            return Some(ty);
        }
        match line.trim() {
            "integer" => return Some("i32".to_string()),
            "floating-point number" => return Some("f64".to_string()),
            _ => {}
        }
    }
    // Multi-line rustc note form:
    //   = note: expected struct `<marker>`
    //           found associated type `<T as Trait>::Item<'static>`
    let note_needle = format!("expected struct `{}`", marker);
    if let Some(idx) = stderr.find(&note_needle) {
        let mut lines = stderr[idx..].lines();
        lines.next();
        if let Some(found_line) = lines.next()
            && found_line.contains("found ")
            && let Some(ty) = extract_backticked_type(found_line)
        {
            return Some(ty);
        }
    }
    None
}

fn extract_backticked_type(line: &str) -> Option<String> {
    let open = line.find('`')?;
    let after_open = &line[open + 1..];
    let end = after_open.find('`')?;
    let ty = after_open[..end].trim();
    if ty.is_empty() {
        None
    } else {
        Some(ty.replace("{integer}", "i32").replace("{float}", "f64"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rustc_runner_covers_all_error_and_parsing_branches() {
        let dir = create_temp_dir_in(&std::env::temp_dir(), "rsharp_unit_runner").unwrap();
        let blocking_file = dir.join("not_a_dir");
        std::fs::write(&blocking_file, "x").unwrap();
        let bad_rustc = OsStr::new("__rsharp_nonexistent_rustc__");
        let dummy_bin = dir.join("dummy_out");

        // create_temp_dir_in failure & propagation through every *_in entry point
        assert!(create_temp_dir_in(&blocking_file, "p").is_err());
        assert!(compile_rust_to_bin_in(&blocking_file, bad_rustc, "", &dummy_bin).is_err());
        assert!(check_rust_source_in(&blocking_file, bad_rustc, "", true).is_err());
        assert!(compile_and_run_rust_in(&blocking_file, bad_rustc, "").is_err());
        assert!(run_type_probe_in(&blocking_file, bad_rustc, "", "", "x", "M").is_err());

        // write_temp_file failure & run_rustc_cmd / run_binary_cmd spawn failures
        assert!(prepare_temp_rust_file(&dir, "p", "missing_sub/a.rs", "").is_err());
        assert!(compile_rust_to_bin_in(&dir, bad_rustc, "fn main() {}", &dummy_bin).is_err());
        assert!(check_rust_source_in(&dir, bad_rustc, "fn f() {}", true).is_err());
        assert!(run_type_probe_in(&dir, bad_rustc, "fn f() {}", "fn f() {}", "x", "M").is_err());
        assert!(run_binary_cmd(&dir.join("missing_binary_xyz")).is_err());
        assert!(compile_and_run_rust_in(&dir, OsStr::new("rustc"), "fn main( {").is_err());

        // Trigger run_binary_cmd failure inside compile_and_run_rust_in by using a valid dummy
        // executable as rustc that exits 0 without producing the target binary.
        let noop_exe = dir.join(PROG_EXE);
        compile_rust_to_bin("fn main() {}", &noop_exe).unwrap();
        assert!(compile_and_run_rust_in(&dir, noop_exe.as_os_str(), "fn main() {}").is_err());

        // Exercise success and remaining error paths on *_in helpers inside this crate instantiation
        assert!(check_rust_source_in(&dir, OsStr::new("rustc"), "fn main() {}", false).is_ok());
        assert!(check_rust_source_in(&dir, OsStr::new("rustc"), "fn bad( {", true).is_err());
        assert_eq!(
            compile_and_run_rust_in(&dir, OsStr::new("rustc"), "fn main() {}")
                .unwrap()
                .exit_code,
            0
        );
        assert_eq!(
            run_type_probe_in(
                &dir,
                OsStr::new("rustc"),
                "fn main() { let x = 1_i32; struct M; let _: M = x; }",
                "fn main() {}",
                "x",
                "M",
            )
            .unwrap(),
            "i32"
        );
        assert!(
            run_type_probe_in(
                &dir,
                OsStr::new("rustc"),
                "fn f() {}",
                "fn f() {}",
                "x",
                "M"
            )
            .is_err()
        );

        // find_expr_end: nested delimiters, closing delim at depth 0, comma/semicolon, and EOF
        assert_eq!(find_expr_end("foo(a, [b, {c}]), rest"), 16);
        assert_eq!(find_expr_end("x + 1)"), 5);
        assert_eq!(find_expr_end("x + 1; next"), 5);
        assert_eq!(find_expr_end("x + 1"), 5);

        // extract_found_type & extract_backticked_type edge cases
        assert_eq!(
            extract_found_type("expected `M`, found `i64`\n", "M"),
            Some("i64".to_string())
        );
        assert_eq!(
            extract_found_type("expected `M`, found integer\n", "M"),
            Some("i32".to_string())
        );
        assert_eq!(
            extract_found_type("expected `M`, found floating-point number\n", "M"),
            Some("f64".to_string())
        );
        assert_eq!(
            extract_found_type("expected `M`, found unknown\n", "M"),
            None
        );
        assert_eq!(
            extract_found_type("expected struct `M`\n    found type `Vec<i32>`\n", "M"),
            Some("Vec<i32>".to_string())
        );
        assert_eq!(
            extract_found_type("expected struct `M`\n    found type ``\n", "M"),
            None
        );
        assert_eq!(extract_backticked_type("`unclosed"), None);
        assert_eq!(extract_backticked_type("``"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
