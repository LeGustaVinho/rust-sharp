use std::path::{Path, PathBuf};

use rsharp_core::{
    FromRustOptions, PreludeMode, RustAnalyzerOracle, TranspileOptions, build_rsharp, check_rsharp,
    convert_from_rust, convert_from_rust_with_hints, find_cargo_workspace_root, run_rsharp,
    transpile_to_rust,
};

/// Result of executing a CLI invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

const USAGE: &str = "\
R# (R Sharp) - Rust Performance. C# Productivity.

Usage:
  rsharp to-rust <file.rsharp> [-o <out.rs>] [--prelude embedded|crate|none]
  rsharp from-rust <file.rs> [-o <out.rsharp>] [--workspace <dir>] [--no-rustc-infer] [--strict]
  rsharp from-rust --project <dir> [-o <out_dir>] [--verify] [--strict]
  rsharp build <file.rsharp> [-o <out_bin>]
  rsharp run <file.rsharp>
  rsharp check <file.rsharp>
  rsharp lsp
  rsharp --help
";

/// Executes CLI arguments and returns captured stdout/stderr/exit_code.
pub fn run_cli(args: &[String]) -> CliOutput {
    if args.is_empty() || matches!(args[0].as_str(), "--help" | "-h" | "help") {
        return CliOutput {
            exit_code: 0,
            stdout: USAGE.to_string(),
            stderr: String::new(),
        };
    }

    match dispatch(args) {
        Ok(out) => out,
        Err(err) => CliOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("error: {}\n", err),
        },
    }
}

fn dispatch(args: &[String]) -> Result<CliOutput, String> {
    match args[0].as_str() {
        "to-rust" => cmd_to_rust(&args[1..]),
        "from-rust" => cmd_from_rust(&args[1..]),
        "build" => cmd_build(&args[1..]),
        "run" => cmd_run(&args[1..]),
        "check" => cmd_check(&args[1..]),
        "lsp" => cmd_lsp(&args[1..]),
        other => Err(format!("Unknown command '{}'. Run 'rsharp --help'.", other)),
    }
}

fn read_input_file(args: &[String], usage_hint: &str) -> Result<(PathBuf, String), String> {
    let path = args
        .first()
        .ok_or_else(|| format!("Missing input file. Usage: {}", usage_hint))?;
    let p = PathBuf::from(path);
    let content = std::fs::read_to_string(&p)
        .map_err(|e| format!("Failed to read '{}': {}", p.display(), e))?;
    Ok((p, content))
}

fn cmd_to_rust(args: &[String]) -> Result<CliOutput, String> {
    let (_, src) = read_input_file(args, "rsharp to-rust <file.rsharp> [-o <out.rs>]")?;
    let mut out_path: Option<PathBuf> = None;
    let mut prelude_mode = PreludeMode::Embedded;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                let val = args
                    .get(i + 1)
                    .ok_or_else(|| "Missing path after '-o'".to_string())?;
                out_path = Some(PathBuf::from(val));
                i += 2;
            }
            "--prelude" => {
                let val = args
                    .get(i + 1)
                    .ok_or_else(|| "Missing value after '--prelude'".to_string())?;
                prelude_mode = match val.as_str() {
                    "embedded" => PreludeMode::Embedded,
                    "crate" => PreludeMode::CrateImport,
                    "none" => PreludeMode::None,
                    other => {
                        return Err(format!(
                            "Invalid prelude mode '{}'; expected embedded, crate, or none",
                            other
                        ));
                    }
                };
                i += 2;
            }
            flag => return Err(format!("Unknown option '{}'", flag)),
        }
    }

    let rust =
        transpile_to_rust(&src, TranspileOptions { prelude_mode }).map_err(|e| e.to_string())?;

    if let Some(p) = out_path {
        std::fs::write(&p, &rust)
            .map_err(|e| format!("Failed to write '{}': {}", p.display(), e))?;
        Ok(CliOutput {
            exit_code: 0,
            stdout: format!("Wrote {}\n", p.display()),
            stderr: String::new(),
        })
    } else {
        Ok(CliOutput {
            exit_code: 0,
            stdout: rust,
            stderr: String::new(),
        })
    }
}

fn cmd_from_rust(args: &[String]) -> Result<CliOutput, String> {
    if args.first().map(String::as_str) == Some("--project") {
        return cmd_from_rust_project(&args[1..]);
    }
    let (in_path, src) = read_input_file(args, "rsharp from-rust <file.rs> [-o <out.rsharp>]")?;
    let mut out_path: Option<PathBuf> = None;
    let mut workspace_dir: Option<PathBuf> = None;
    let mut use_rustc_inference = true;
    let mut strict_unnameable = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                let val = args
                    .get(i + 1)
                    .ok_or_else(|| "Missing path after '-o'".to_string())?;
                out_path = Some(PathBuf::from(val));
                i += 2;
            }
            "--workspace" => {
                let val = args
                    .get(i + 1)
                    .ok_or_else(|| "Missing directory after '--workspace'".to_string())?;
                workspace_dir = Some(PathBuf::from(val));
                i += 2;
            }
            "--no-rustc-infer" => {
                use_rustc_inference = false;
                i += 1;
            }
            "--strict" => {
                strict_unnameable = true;
                i += 1;
            }
            flag => return Err(format!("Unknown option '{}'", flag)),
        }
    }

    let rsharp = if let Some(ws) = workspace_dir {
        convert_file_with_workspace(&in_path, &src, &ws, strict_unnameable)?
    } else {
        convert_from_rust(
            &src,
            FromRustOptions {
                use_rustc_inference,
            },
        )
        .map_err(|e| e.to_string())?
    };

    if let Some(p) = out_path {
        std::fs::write(&p, &rsharp)
            .map_err(|e| format!("Failed to write '{}': {}", p.display(), e))?;
        Ok(CliOutput {
            exit_code: 0,
            stdout: format!("Wrote {}\n", p.display()),
            stderr: String::new(),
        })
    } else {
        Ok(CliOutput {
            exit_code: 0,
            stdout: rsharp,
            stderr: String::new(),
        })
    }
}

fn cmd_from_rust_project(args: &[String]) -> Result<CliOutput, String> {
    let dir_arg = args
        .first()
        .ok_or_else(|| "Missing directory after '--project'".to_string())?;
    let project_dir = PathBuf::from(dir_arg);
    if !project_dir.is_dir() {
        return Err(format!(
            "Project directory '{}' does not exist",
            project_dir.display()
        ));
    }

    let mut out_dir: Option<PathBuf> = None;
    let mut verify = false;
    let mut strict_unnameable = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                let val = args
                    .get(i + 1)
                    .ok_or_else(|| "Missing path after '-o'".to_string())?;
                out_dir = Some(PathBuf::from(val));
                i += 2;
            }
            "--verify" => {
                verify = true;
                i += 1;
            }
            "--strict" => {
                strict_unnameable = true;
                i += 1;
            }
            flag => return Err(format!("Unknown option '{}'", flag)),
        }
    }

    let ws_root = find_cargo_workspace_root(&project_dir).unwrap_or_else(|| project_dir.clone());
    let mut rs_files = Vec::new();
    collect_project_rs_files(&project_dir, &mut rs_files);
    if rs_files.is_empty() {
        return Err(format!(
            "No .rs files found under '{}'",
            project_dir.display()
        ));
    }

    let mut oracle = RustAnalyzerOracle::open(&ws_root).map_err(|e| e.to_string())?;
    let mut converted = 0usize;
    let mut errors = Vec::new();
    for path in &rs_files {
        let rel = path.strip_prefix(&project_dir).unwrap_or(path);
        match convert_project_file(
            &mut oracle,
            path,
            rel,
            out_dir.as_deref(),
            verify,
            strict_unnameable,
        ) {
            Ok(()) => converted += 1,
            Err(e) => errors.push(format!("{}: {}", rel.display(), e)),
        }
    }
    oracle.close();

    if !errors.is_empty() {
        return Err(format!(
            "Converted {}/{} files with {} error(s):\n{}",
            converted,
            rs_files.len(),
            errors.len(),
            errors.join("\n")
        ));
    }

    Ok(CliOutput {
        exit_code: 0,
        stdout: format!("Converted {}/{} files cleanly\n", converted, rs_files.len()),
        stderr: String::new(),
    })
}

fn convert_file_with_workspace(
    in_path: &Path,
    src: &str,
    ws: &Path,
    strict_unnameable: bool,
) -> Result<String, String> {
    let mut oracle = RustAnalyzerOracle::open(ws).map_err(|e| e.to_string())?;
    let res = convert_with_oracle(&mut oracle, in_path, src, strict_unnameable);
    oracle.close();
    res
}

fn convert_with_oracle(
    oracle: &mut RustAnalyzerOracle,
    in_path: &Path,
    src: &str,
    strict_unnameable: bool,
) -> Result<String, String> {
    let hints = oracle
        .type_hints_for_file(in_path, src)
        .map_err(|e| e.to_string())?;
    convert_from_rust_with_hints(
        src,
        FromRustOptions {
            use_rustc_inference: false,
        },
        &hints,
        strict_unnameable,
    )
    .map_err(|e| e.to_string())
}

fn convert_project_file(
    oracle: &mut RustAnalyzerOracle,
    path: &Path,
    rel: &Path,
    out_dir: Option<&Path>,
    verify: bool,
    strict_unnameable: bool,
) -> Result<(), String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let rsharp = convert_with_oracle(oracle, path, &src, strict_unnameable)?;
    if verify {
        transpile_to_rust(
            &rsharp,
            TranspileOptions {
                prelude_mode: PreludeMode::None,
            },
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(base_out) = out_dir {
        let mut dest = base_out.join(rel);
        dest.set_extension("rsharp");
        std::fs::create_dir_all(dest.parent().unwrap_or(base_out)).map_err(|e| e.to_string())?;
        std::fs::write(&dest, &rsharp).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn collect_project_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    const SKIP: &[&str] = &[
        "target",
        "node_modules",
        "dist",
        "build",
        "pkg",
        "out",
        "obj",
        "worktrees",
        "third_party",
    ];
    let in_src = dir.file_name().and_then(|n| n.to_str()) == Some("src");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str())
                && !name.starts_with('.')
                && !SKIP.contains(&name)
                && !(name == "vendor" && !in_src)
            {
                collect_project_rs_files(&p, out);
            }
        } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

fn cmd_build(args: &[String]) -> Result<CliOutput, String> {
    let (in_path, src) = read_input_file(args, "rsharp build <file.rsharp> [-o <out_bin>]")?;
    let mut out_path = default_binary_path(&in_path);

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                let val = args
                    .get(i + 1)
                    .ok_or_else(|| "Missing path after '-o'".to_string())?;
                out_path = PathBuf::from(val);
                i += 2;
            }
            flag => return Err(format!("Unknown option '{}'", flag)),
        }
    }

    build_rsharp(&src, &out_path).map_err(|e| e.to_string())?;
    Ok(CliOutput {
        exit_code: 0,
        stdout: format!("Built {}\n", out_path.display()),
        stderr: String::new(),
    })
}

fn cmd_run(args: &[String]) -> Result<CliOutput, String> {
    let (_, src) = read_input_file(args, "rsharp run <file.rsharp>")?;
    let run = run_rsharp(&src).map_err(|e| e.to_string())?;
    Ok(CliOutput {
        exit_code: run.exit_code,
        stdout: run.stdout,
        stderr: run.stderr,
    })
}

fn cmd_check(args: &[String]) -> Result<CliOutput, String> {
    let (path, src) = read_input_file(args, "rsharp check <file.rsharp>")?;
    check_rsharp(&src).map_err(|e| e.to_string())?;
    Ok(CliOutput {
        exit_code: 0,
        stdout: format!("OK {}\n", path.display()),
        stderr: String::new(),
    })
}

fn cmd_lsp(args: &[String]) -> Result<CliOutput, String> {
    if let Some(flag) = args.first() {
        return Err(format!("Unknown option '{}'", flag));
    }
    rsharp_lsp::run_stdio()?;
    Ok(CliOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    })
}

fn default_binary_path(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("rsharp_out");
    let mut p = input.with_file_name(stem);
    #[cfg(windows)]
    {
        p.set_extension("exe");
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn mock_oracle(inlay_responses: usize) -> RustAnalyzerOracle {
        let frame = |body: &str| format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut buf = Vec::new();
        buf.extend(frame(r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#).as_bytes());
        buf.extend(
            frame(r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"quiescent":true}}"#)
                .as_bytes(),
        );
        for k in 0..inlay_responses {
            let id = 2 + k as i64;
            let body = format!(r#"{{"jsonrpc":"2.0","id":{},"result":[]}}"#, id);
            buf.extend(frame(&body).as_bytes());
        }
        let ws = std::env::temp_dir();
        RustAnalyzerOracle::from_io(Cursor::new(buf), Vec::new(), &ws).unwrap()
    }

    #[test]
    fn convert_project_file_covers_all_error_branches() {
        let base = std::env::temp_dir().join(format!("rsharp_cli_unit_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        // 1. Read failure (missing file)
        let mut oracle = mock_oracle(0);
        let err = convert_project_file(
            &mut oracle,
            &base.join("missing.rs"),
            Path::new("missing.rs"),
            None,
            false,
            false,
        );
        assert!(err.is_err());

        // 2. Hint failure (oracle reader at EOF)
        let good_file = base.join("good.rs");
        std::fs::write(&good_file, "fn main() {}").unwrap();
        let err = convert_project_file(
            &mut oracle,
            &good_file,
            Path::new("good.rs"),
            None,
            false,
            false,
        );
        assert!(err.is_err());

        // 3. Verify failure (nightly multi-stmt `const { let ... }` pattern fails R# `--verify`)
        let bad_verify = base.join("bad_verify.rs");
        std::fs::write(
            &bad_verify,
            "fn f(x: i32) { match x { const { let a = 1; a } => {} _ => {} } }\n",
        )
        .unwrap();
        // 4. Success with `out_dir = None`, mkdir failure, and write failure when destination is a directory
        let mut oracle2 = mock_oracle(4);
        let err = convert_project_file(
            &mut oracle2,
            &bad_verify,
            Path::new("bad_verify.rs"),
            None,
            true,
            false,
        );
        assert!(err.is_err());

        assert!(
            convert_project_file(
                &mut oracle2,
                &good_file,
                Path::new("good.rs"),
                None,
                false,
                false,
            )
            .is_ok()
        );

        let blocking = base.join("blocking");
        std::fs::write(&blocking, "file").unwrap();
        let err = convert_project_file(
            &mut oracle2,
            &good_file,
            Path::new("sub/good.rs"),
            Some(&blocking),
            false,
            false,
        );
        assert!(err.is_err());

        let out_dir = base.join("out");
        std::fs::create_dir_all(out_dir.join("good.rsharp")).unwrap();
        let err = convert_project_file(
            &mut oracle2,
            &good_file,
            Path::new("good.rs"),
            Some(&out_dir),
            false,
            false,
        );
        assert!(err.is_err());

        let _ = std::fs::remove_dir_all(&base);
    }
}
