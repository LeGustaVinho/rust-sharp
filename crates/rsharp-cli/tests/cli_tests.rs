use std::path::PathBuf;
use std::process::Command;

use rsharp_cli::run_cli;

fn args(slice: &[&str]) -> Vec<String> {
    slice.iter().map(|s| s.to_string()).collect()
}

fn temp_file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rsharp_cli_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn test_cli_help_and_unknown_commands() {
    for flag in [vec![], args(&["--help"]), args(&["-h"]), args(&["help"])] {
        let out = run_cli(&flag);
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("Usage:"));
    }

    let bad = run_cli(&args(&["bogus"]));
    assert_eq!(bad.exit_code, 1);
    assert!(bad.stderr.contains("Unknown command"));
}

#[test]
fn test_cli_to_rust_and_from_rust_and_check_and_run_and_build() {
    let rsh_file = temp_file("sample.rsharp");
    std::fs::write(
        &rsh_file,
        r#"
            public int Add(int a, int b)
            {
                return a + b;
            }
            void main()
            {
                PrintLine(Add(19, 23));
            }
        "#,
    )
    .unwrap();
    let rsh_str = rsh_file.to_string_lossy().to_string();

    // to-rust to stdout with prelude modes
    for mode in ["embedded", "crate", "none"] {
        let out = run_cli(&args(&["to-rust", &rsh_str, "--prelude", mode]));
        assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
        assert!(out.stdout.contains("pub fn Add"));
    }

    // to-rust to output file
    let rs_out = temp_file("sample_out.rs");
    let rs_out_str = rs_out.to_string_lossy().to_string();
    let out = run_cli(&args(&[
        "to-rust",
        &rsh_str,
        "-o",
        &rs_out_str,
        "--prelude",
        "none",
    ]));
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("Wrote"));

    // from-rust stdout & file & --no-rustc-infer
    let out = run_cli(&args(&["from-rust", &rs_out_str, "--no-rustc-infer"]));
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("public int Add"));

    let rsh_round = temp_file("sample_round.rsharp");
    let rsh_round_str = rsh_round.to_string_lossy().to_string();
    let out = run_cli(&args(&["from-rust", &rs_out_str, "-o", &rsh_round_str]));
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("Wrote"));

    // check
    let out = run_cli(&args(&["check", &rsh_str]));
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("OK"));

    // run
    let out = run_cli(&args(&["run", &rsh_str]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "42");

    // build explicit -o & execute built binary
    let bin_path = temp_file(if cfg!(windows) {
        "sample_bin.exe"
    } else {
        "sample_bin"
    });
    let bin_str = bin_path.to_string_lossy().to_string();
    let out = run_cli(&args(&["build", &rsh_str, "-o", &bin_str]));
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("Built"));
    let exec = Command::new(&bin_path).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&exec.stdout).trim(), "42");

    // build default path
    let out = run_cli(&args(&["build", &rsh_str]));
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
}

#[test]
fn test_cli_examples_end_to_end() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();

    let cases = [
        ("examples/accounts.rsharp", "Alice: 150\nTotal: 300"),
        (
            "examples/matrices_and_control.rsharp",
            "matrix=19 fallback=35 area=16 doubled=24",
        ),
        (
            "examples/rust_interop.rsharp",
            "Item(Sword)\nlongest_word\ndynamic=123\nunsafe=42\ninline_rust=42",
        ),
    ];

    for (rel, expected) in cases {
        let p = workspace_root.join(rel);
        let p_str = p.to_string_lossy().to_string();
        let out = run_cli(&args(&["run", &p_str]));
        assert_eq!(out.exit_code, 0, "example {} failed: {}", rel, out.stderr);
        let normalized = out.stdout.replace("\r\n", "\n");
        assert_eq!(
            normalized.trim(),
            expected,
            "example {} stdout mismatch",
            rel
        );
    }
}

#[test]
fn test_cli_error_paths() {
    let bad_cases = [
        args(&["to-rust"]),
        args(&["to-rust", "nonexistent_file_12345.rsharp"]),
        args(&["from-rust"]),
        args(&["build"]),
        args(&["run"]),
        args(&["check"]),
    ];
    for c in bad_cases {
        let out = run_cli(&c);
        assert_eq!(out.exit_code, 1, "expected error for {:?}", c);
        assert!(out.stderr.contains("error:"));
    }

    let rsh_file = temp_file("err_flags.rsharp");
    std::fs::write(&rsh_file, "void main() {}").unwrap();
    let rsh_str = rsh_file.to_string_lossy().to_string();

    let flag_errors = [
        args(&["to-rust", &rsh_str, "-o"]),
        args(&["to-rust", &rsh_str, "--prelude"]),
        args(&["to-rust", &rsh_str, "--prelude", "invalid"]),
        args(&["to-rust", &rsh_str, "--unknown"]),
        args(&["from-rust", &rsh_str, "-o"]),
        args(&["from-rust", &rsh_str, "--unknown"]),
        args(&["build", &rsh_str, "-o"]),
        args(&["build", &rsh_str, "--unknown"]),
    ];
    for c in flag_errors {
        let out = run_cli(&c);
        assert_eq!(out.exit_code, 1, "expected error for {:?}", c);
    }

    // Invalid syntax through check/run/build/from-rust
    let bad_rsh = temp_file("bad_syntax.rsharp");
    std::fs::write(&bad_rsh, "void main() { var x = 1; }").unwrap();
    let bad_str = bad_rsh.to_string_lossy().to_string();
    assert_eq!(run_cli(&args(&["check", &bad_str])).exit_code, 1);
    assert_eq!(run_cli(&args(&["run", &bad_str])).exit_code, 1);
    assert_eq!(run_cli(&args(&["build", &bad_str])).exit_code, 1);
    assert_eq!(run_cli(&args(&["to-rust", &bad_str])).exit_code, 1);
    assert_eq!(run_cli(&args(&["from-rust", &bad_str])).exit_code, 1);
}

#[test]
fn test_cli_binary_entrypoint() {
    let bin = env!("CARGO_BIN_EXE_rsharp");

    let ok = Command::new(bin).arg("--help").output().unwrap();
    assert!(ok.status.success());
    assert!(String::from_utf8_lossy(&ok.stdout).contains("Usage:"));

    let fail = Command::new(bin).arg("not-a-command").output().unwrap();
    assert!(!fail.status.success());
    assert!(String::from_utf8_lossy(&fail.stderr).contains("error:"));
}
