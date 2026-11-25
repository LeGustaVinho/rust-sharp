use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{RSharpError, Result};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Result of executing a compiled R# program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

fn unique_temp_dir(prefix: &str) -> Result<PathBuf> {
    let id = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), id));
    std::fs::create_dir_all(&dir)
        .map_err(|e| RSharpError::io(format!("Failed to create temp dir: {}", e)))?;
    Ok(dir)
}

/// Compiles generated Rust source code into a native executable at `out_bin`.
pub fn compile_rust_to_bin(rust_source: &str, out_bin: &Path) -> Result<()> {
    let dir = unique_temp_dir("rsharp_build")?;
    let rs_path = dir.join("main.rs");
    std::fs::write(&rs_path, rust_source)
        .map_err(|e| RSharpError::io(format!("Failed to write temp Rust file: {}", e)))?;

    let output = Command::new("rustc")
        .arg("--edition=2024")
        .arg(&rs_path)
        .arg("-o")
        .arg(out_bin)
        .output()
        .map_err(|e| RSharpError::compile(format!("Failed to invoke rustc: {}", e)))?;

    let _ = std::fs::remove_dir_all(&dir);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(RSharpError::compile(stderr));
    }
    Ok(())
}

/// Type-checks generated Rust source via `rustc --emit=metadata` without producing an executable.
pub fn check_rust_source(rust_source: &str, is_lib: bool) -> Result<()> {
    let dir = unique_temp_dir("rsharp_check")?;
    let rs_path = dir.join("check.rs");
    let meta_path = dir.join("check.rmeta");
    std::fs::write(&rs_path, rust_source)
        .map_err(|e| RSharpError::io(format!("Failed to write temp Rust file: {}", e)))?;

    let mut cmd = Command::new("rustc");
    cmd.arg("--edition=2024").arg("--emit=metadata");
    if is_lib {
        cmd.arg("--crate-type=lib");
    }
    let output = cmd
        .arg(&rs_path)
        .arg("-o")
        .arg(&meta_path)
        .output()
        .map_err(|e| RSharpError::compile(format!("Failed to invoke rustc: {}", e)))?;

    let _ = std::fs::remove_dir_all(&dir);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(RSharpError::compile(stderr));
    }
    Ok(())
}

/// Compiles and executes generated Rust source, returning stdout, stderr, and exit code.
pub fn compile_and_run_rust(rust_source: &str) -> Result<RunOutput> {
    let dir = unique_temp_dir("rsharp_run")?;
    let exe_name = if cfg!(windows) { "prog.exe" } else { "prog" };
    let bin_path = dir.join(exe_name);
    let res = compile_rust_to_bin(rust_source, &bin_path);
    if let Err(e) = res {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }

    let run = Command::new(&bin_path)
        .output()
        .map_err(|e| RSharpError::io(format!("Failed to execute compiled binary: {}", e)))?;

    let _ = std::fs::remove_dir_all(&dir);

    Ok(RunOutput {
        exit_code: run.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&run.stdout).to_string(),
        stderr: String::from_utf8_lossy(&run.stderr).to_string(),
    })
}

/// Probes `rustc` to recover the inferred concrete type of an unannotated local `let <var_name> = ...;`.
pub fn probe_local_type(rust_source: &str, var_name: &str, seq: usize) -> Result<String> {
    let marker = format!("__RSHARP_PROBE_{}__", seq);
    let injected = inject_type_probe(rust_source, var_name, &marker).ok_or_else(|| {
        RSharpError::unresolved_type(var_name, "Could not locate binding site for type probe")
    })?;

    let dir = unique_temp_dir("rsharp_probe")?;
    let rs_path = dir.join("probe.rs");
    let meta_path = dir.join("probe.rmeta");
    std::fs::write(&rs_path, &injected)
        .map_err(|e| RSharpError::io(format!("Failed to write probe file: {}", e)))?;

    let is_lib = !rust_source.contains("fn main(") && !rust_source.contains("fn main (");
    let mut cmd = Command::new("rustc");
    cmd.arg("--edition=2024").arg("--emit=metadata");
    if is_lib {
        cmd.arg("--crate-type=lib");
    }
    let output = cmd
        .arg(&rs_path)
        .arg("-o")
        .arg(&meta_path)
        .output()
        .map_err(|e| RSharpError::compile(format!("Failed to invoke rustc: {}", e)))?;

    let _ = std::fs::remove_dir_all(&dir);

    let stderr = String::from_utf8_lossy(&output.stderr);
    extract_found_type(&stderr, &marker).ok_or_else(|| {
        RSharpError::unresolved_type(
            var_name,
            "rustc could not infer concrete type (external crate or compile error)",
        )
    })
}

/// Injects `struct <marker>; let _: <marker> = <var_name>;` immediately after `let (mut )?<var_name> = ...;`.
fn inject_type_probe(source: &str, var_name: &str, marker: &str) -> Option<String> {
    let patterns = [
        format!("let mut {} =", var_name),
        format!("let {} =", var_name),
    ];
    for pat in &patterns {
        if let Some(start) = source.find(pat)
            && let Some(semi_rel) = source[start..].find(';') {
                let insert_pos = start + semi_rel + 1;
                let mut out = String::with_capacity(source.len() + 64);
                out.push_str(&source[..insert_pos]);
                out.push_str(&format!(" struct {}; let _: {} = {};", marker, marker, var_name));
                out.push_str(&source[insert_pos..]);
                return Some(out);
            }
    }
    None
}

/// Extracts `<Type>` from rustc's `expected \`<marker>\`, found \`<Type>\`` diagnostic.
fn extract_found_type(stderr: &str, marker: &str) -> Option<String> {
    let needle = format!("expected `{}`, found `", marker);
    let idx = stderr.find(&needle)?;
    let rest = &stderr[idx + needle.len()..];
    let end = rest.find('`')?;
    let ty = rest[..end].trim();
    if ty.is_empty() {
        None
    } else {
        Some(ty.to_string())
    }
}
