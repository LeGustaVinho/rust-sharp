use std::path::{Path, PathBuf};

use rsharp_core::{
    FromRustOptions, PreludeMode, TranspileOptions, build_rsharp, check_rsharp, convert_from_rust,
    run_rsharp, transpile_to_rust,
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
  rsharp from-rust <file.rs> [-o <out.rsharp>] [--no-rustc-infer]
  rsharp build <file.rsharp> [-o <out_bin>]
  rsharp run <file.rsharp>
  rsharp check <file.rsharp>
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
    let (_, src) = read_input_file(args, "rsharp from-rust <file.rs> [-o <out.rsharp>]")?;
    let mut out_path: Option<PathBuf> = None;
    let mut use_rustc_inference = true;

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
            "--no-rustc-infer" => {
                use_rustc_inference = false;
                i += 1;
            }
            flag => return Err(format!("Unknown option '{}'", flag)),
        }
    }

    let rsharp = convert_from_rust(
        &src,
        FromRustOptions {
            use_rustc_inference,
        },
    )
    .map_err(|e| e.to_string())?;

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
