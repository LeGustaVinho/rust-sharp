use std::path::{Path, PathBuf};

use rsharp_core::{
    FromRustOptions, PreludeMode, ProjectConvertOptions, ProjectFile, RustAnalyzerOracle,
    ToRustProjectOptions, TranspileOptions, build_rsharp, check_rsharp, convert_from_rust,
    convert_from_rust_with_hints, convert_project_files, discover_cargo_rs_files,
    find_cargo_workspace_root, run_rsharp, transpile_project_files, transpile_to_rust,
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
  rsharp to-rust --project <dir> [-o <out_dir>] [--prelude none|crate|embedded]
  rsharp from-rust <file.rs> [-o <out.rsharp>] [--workspace <dir>] [--no-rustc-infer] [--strict]
  rsharp from-rust --project <dir> [-o <out_dir>] [--verify] [--strict] [--no-ra] [--all-rs-files] [--no-raw-rust]
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

fn parse_prelude_mode(val: &str) -> Result<PreludeMode, String> {
    match val {
        "embedded" => Ok(PreludeMode::Embedded),
        "crate" => Ok(PreludeMode::CrateImport),
        "none" => Ok(PreludeMode::None),
        other => Err(format!(
            "Invalid prelude mode '{}'; expected embedded, crate, or none",
            other
        )),
    }
}

fn cmd_to_rust(args: &[String]) -> Result<CliOutput, String> {
    if args.first().map(String::as_str) == Some("--project") {
        return cmd_to_rust_project(&args[1..]);
    }
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
                prelude_mode = parse_prelude_mode(val)?;
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

fn cmd_to_rust_project(args: &[String]) -> Result<CliOutput, String> {
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
    let mut prelude_mode = PreludeMode::None;

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
            "--prelude" => {
                let val = args
                    .get(i + 1)
                    .ok_or_else(|| "Missing value after '--prelude'".to_string())?;
                prelude_mode = parse_prelude_mode(val)?;
                i += 2;
            }
            flag => return Err(format!("Unknown option '{}'", flag)),
        }
    }

    let mut rsh_files = Vec::new();
    collect_project_files(&project_dir, "rsharp", &mut rsh_files);
    rsh_files.sort_unstable();
    if rsh_files.is_empty() {
        return Err(format!(
            "No .rsharp files found under '{}'",
            project_dir.display()
        ));
    }

    let files: Vec<ProjectFile> = rsh_files
        .iter()
        .map(|path| ProjectFile {
            path: path.clone(),
            rel: path
                .strip_prefix(&project_dir)
                .unwrap_or(path)
                .to_path_buf(),
        })
        .collect();
    let outcomes = transpile_project_files(
        &files,
        &ToRustProjectOptions {
            out_dir,
            prelude_mode,
        },
    );

    let mut transpiled = 0usize;
    let mut errors = Vec::new();
    for outcome in outcomes {
        match outcome.result {
            Ok(()) => transpiled += 1,
            Err(e) => errors.push(format!("{}: {}", outcome.rel.display(), e)),
        }
    }

    if !errors.is_empty() {
        return Err(format!(
            "Transpiled {}/{} files with {} error(s):\n{}",
            transpiled,
            rsh_files.len(),
            errors.len(),
            errors.join("\n")
        ));
    }

    Ok(CliOutput {
        exit_code: 0,
        stdout: format!(
            "Transpiled {}/{} files cleanly\n",
            transpiled,
            rsh_files.len()
        ),
        stderr: String::new(),
    })
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
    let mut no_ra = false;
    let mut all_rs_files = false;
    let mut no_raw_rust = false;

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
            "--no-ra" => {
                no_ra = true;
                i += 1;
            }
            "--all-rs-files" => {
                all_rs_files = true;
                i += 1;
            }
            "--no-raw-rust" => {
                no_raw_rust = true;
                i += 1;
            }
            flag => return Err(format!("Unknown option '{}'", flag)),
        }
    }

    let ws_root = find_cargo_workspace_root(&project_dir).unwrap_or_else(|| project_dir.clone());
    let proj_canon = std::fs::canonicalize(&project_dir).ok();
    let rs_files = if !all_rs_files && let Some(discovered) = discover_cargo_rs_files(&project_dir)
    {
        discovered
    } else {
        let mut files = Vec::new();
        collect_project_files(&project_dir, "rs", &mut files);
        files.sort_unstable();
        files
    };
    if rs_files.is_empty() {
        return Err(format!(
            "No .rs files found under '{}'",
            project_dir.display()
        ));
    }

    let mut oracle = if no_ra {
        None
    } else {
        Some(RustAnalyzerOracle::open(&ws_root).map_err(|e| e.to_string())?)
    };
    let files: Vec<ProjectFile> = rs_files
        .iter()
        .map(|path| {
            let rel = path
                .strip_prefix(&project_dir)
                .ok()
                .or_else(|| {
                    proj_canon
                        .as_deref()
                        .and_then(|c| path.strip_prefix(c).ok())
                })
                .unwrap_or(path)
                .to_path_buf();
            ProjectFile {
                path: path.clone(),
                rel,
            }
        })
        .collect();
    let outcomes = convert_project_files(
        &files,
        oracle.as_mut(),
        &ProjectConvertOptions {
            out_dir,
            verify,
            strict_unnameable,
        },
    );
    if let Some(o) = oracle {
        o.close();
    }

    let mut converted = 0usize;
    let mut total_raw_rust = 0usize;
    let mut raw_rust_files = Vec::new();
    let mut errors = Vec::new();
    for outcome in outcomes {
        match outcome.result {
            Ok(raw_count) => {
                converted += 1;
                if raw_count > 0 {
                    total_raw_rust += raw_count;
                    raw_rust_files.push(format!(
                        "{}: {} rust{{}} block(s)",
                        outcome.rel.display(),
                        raw_count
                    ));
                }
            }
            Err(e) => errors.push(format!("{}: {}", outcome.rel.display(), e)),
        }
    }

    if !errors.is_empty() {
        return Err(format!(
            "Converted {}/{} files with {} error(s):\n{}",
            converted,
            rs_files.len(),
            errors.len(),
            errors.join("\n")
        ));
    }
    if no_raw_rust && total_raw_rust > 0 {
        return Err(format!(
            "Found {} rust{{}} fallback block(s) across {} file(s) with --no-raw-rust:\n{}",
            total_raw_rust,
            raw_rust_files.len(),
            raw_rust_files.join("\n")
        ));
    }

    Ok(CliOutput {
        exit_code: 0,
        stdout: format!(
            "Converted {}/{} files cleanly ({} rust{{}} blocks)\n",
            converted,
            rs_files.len(),
            total_raw_rust
        ),
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
    let res = convert_with_oracle(&mut oracle, in_path, src, strict_unnameable)
        .map_err(|e| e.to_string());
    oracle.close();
    res
}

fn convert_with_oracle(
    oracle: &mut RustAnalyzerOracle,
    in_path: &Path,
    src: &str,
    strict_unnameable: bool,
) -> rsharp_core::Result<String> {
    let hints = oracle.type_hints_for_file(in_path, src)?;
    convert_from_rust_with_hints(
        src,
        FromRustOptions {
            use_rustc_inference: false,
        },
        &hints,
        strict_unnameable,
    )
}

fn collect_project_files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
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
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        let is_dir = match entry.file_type() {
            Ok(ty) if !ty.is_symlink() => ty.is_dir(),
            _ => p.is_dir(),
        };
        if is_dir {
            if let Some(name) = p.file_name().and_then(|n| n.to_str())
                && !name.starts_with('.')
                && !SKIP.contains(&name)
                && !(name == "vendor" && !in_src)
            {
                collect_project_files(&p, ext, out);
            }
        } else if p.extension().and_then(|e| e.to_str()) == Some(ext) {
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

    #[test]
    fn convert_with_oracle_covers_ok_and_hint_eof_error() {
        let frame = |body: &str| format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut buf = Vec::new();
        buf.extend(frame(r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#).as_bytes());
        buf.extend(
            frame(r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"quiescent":true}}"#)
                .as_bytes(),
        );
        buf.extend(frame(r#"{"jsonrpc":"2.0","id":2,"result":[]}"#).as_bytes());
        let mut oracle =
            RustAnalyzerOracle::from_io(Cursor::new(buf), Vec::new(), &std::env::temp_dir())
                .unwrap();
        assert!(convert_with_oracle(&mut oracle, Path::new("a.rs"), "fn main() {}", false).is_ok());
        assert!(
            convert_with_oracle(&mut oracle, Path::new("a.rs"), "fn main() {}", false).is_err()
        );
    }
}
