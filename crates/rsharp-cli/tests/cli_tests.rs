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
fn standalone_inference_reports_a_missing_compiler() {
    let file = temp_file("missing_rustc.rs");
    for src in [
        "fn f() { let n = std::hint::black_box(42); }",
        "fn f() { let n = std::hint::black_box(42); let m = std::hint::black_box(n); }",
    ] {
        std::fs::write(&file, src).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_rsharp"))
            .arg("from-rust")
            .arg(&file)
            .env("PATH", file.parent().unwrap())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("Failed to invoke rustc"));
    }
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
        (
            "examples/inference_and_value_blocks.rsharp",
            "triple=18 lo=105 hi=70 adjusted=-11 total=25",
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
    std::fs::write(&bad_rsh, "void main() { let x = 1; }").unwrap();
    let bad_str = bad_rsh.to_string_lossy().to_string();
    assert_eq!(run_cli(&args(&["check", &bad_str])).exit_code, 1);
    assert_eq!(run_cli(&args(&["run", &bad_str])).exit_code, 1);
    assert_eq!(run_cli(&args(&["build", &bad_str])).exit_code, 1);
    assert_eq!(run_cli(&args(&["to-rust", &bad_str])).exit_code, 1);
    assert_eq!(run_cli(&args(&["from-rust", &bad_str])).exit_code, 1);

    // Unwritable output paths for to-rust and from-rust
    let unwritable = temp_file("missing_subdir_xyz")
        .join("nested")
        .join("out.txt");
    let unwritable_str = unwritable.to_string_lossy().to_string();
    assert_eq!(
        run_cli(&args(&["to-rust", &rsh_str, "-o", &unwritable_str])).exit_code,
        1
    );
    let ok_rs = temp_file("valid_for_write_err.rs");
    std::fs::write(&ok_rs, "fn main() {}").unwrap();
    let ok_rs_str = ok_rs.to_string_lossy().to_string();
    assert_eq!(
        run_cli(&args(&["from-rust", &ok_rs_str, "-o", &unwritable_str])).exit_code,
        1
    );
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

    // Unknown option to `lsp` subcommand
    assert_eq!(run_cli(&args(&["lsp", "--bad"])).exit_code, 1);

    // End-to-end stdio LSP handshake (`initialize` -> `initialized` -> `shutdown` -> `exit`)
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(bin)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let frame = |body: &str| format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    {
        let stdin = child.stdin.as_mut().unwrap();
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
        let inited = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
        let shutdown = r#"{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}"#;
        let exit = r#"{"jsonrpc":"2.0","method":"exit","params":null}"#;
        stdin.write_all(frame(init).as_bytes()).unwrap();
        stdin.write_all(frame(inited).as_bytes()).unwrap();
        stdin.write_all(frame(shutdown).as_bytes()).unwrap();
        stdin.write_all(frame(exit).as_bytes()).unwrap();
    }
    let lsp_out = child.wait_with_output().unwrap();
    assert!(
        lsp_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&lsp_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&lsp_out.stdout);
    assert!(stdout.contains("semanticTokensProvider"));
    assert!(stdout.contains("documentSymbolProvider"));

    // Immediate EOF on `rsharp lsp` stdin exercises LSP initialize error propagation
    let eof_child = Command::new(bin)
        .arg("lsp")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!eof_child.status.success());
    assert!(String::from_utf8_lossy(&eof_child.stderr).contains("error:"));
}

#[test]
fn test_cli_from_rust_project_and_workspace_mode() {
    let base = std::env::temp_dir().join(format!("rsharp_cli_proj_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let proj = base.join("mini_crate");
    std::fs::create_dir_all(proj.join("src").join("bin")).unwrap();
    std::fs::create_dir_all(proj.join("src").join("vendor")).unwrap();
    std::fs::create_dir_all(proj.join("vendor")).unwrap();
    std::fs::create_dir_all(proj.join("target")).unwrap();
    std::fs::create_dir_all(proj.join(".hidden")).unwrap();
    std::fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"mini_crate\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("lib.rs"),
        "pub fn double(x: i32) -> i32 { let y = x + x; y }\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("vendor").join("vendored.rs"),
        "pub fn helper() -> i32 { 1 }\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("bin").join("tool.rs"),
        "fn main() { let v = mini_crate::double(21); println!(\"{}\", v); }\n",
    )
    .unwrap();
    // Ignored directories must be skipped (including top-level `vendor/`, unlike `src/vendor/`)
    std::fs::write(proj.join("vendor").join("ignored.rs"), "invalid {{{").unwrap();
    std::fs::write(proj.join("target").join("ignored.rs"), "invalid {{{").unwrap();
    std::fs::write(proj.join(".hidden").join("ignored.rs"), "invalid {{{").unwrap();

    let proj_str = proj.to_string_lossy().to_string();
    let lib_rs_str = proj
        .join("src")
        .join("lib.rs")
        .to_string_lossy()
        .to_string();
    let out_dir = base.join("rsharp_out");
    let out_dir_str = out_dir.to_string_lossy().to_string();

    // Error paths for --project and --workspace
    assert_eq!(run_cli(&args(&["from-rust", "--project"])).exit_code, 1);
    assert_eq!(
        run_cli(&args(&[
            "from-rust",
            "--project",
            &base.join("missing_dir").to_string_lossy()
        ]))
        .exit_code,
        1
    );
    assert_eq!(
        run_cli(&args(&["from-rust", "--project", &proj_str, "-o"])).exit_code,
        1
    );
    assert_eq!(
        run_cli(&args(&["from-rust", "--project", &proj_str, "--unknown"])).exit_code,
        1
    );
    assert_eq!(
        run_cli(&args(&["from-rust", &lib_rs_str, "--workspace"])).exit_code,
        1
    );

    let empty_dir = base.join("empty_dir");
    std::fs::create_dir_all(&empty_dir).unwrap();
    let empty_out = run_cli(&args(&[
        "from-rust",
        "--project",
        &empty_dir.to_string_lossy(),
    ]));
    assert_eq!(empty_out.exit_code, 1);
    assert!(empty_out.stderr.contains("No .rs files"));

    // Successful project conversion with --verify and -o <out_dir>
    let ok = run_cli(&args(&[
        "from-rust",
        "--project",
        &proj_str,
        "-o",
        &out_dir_str,
        "--verify",
    ]));
    assert_eq!(ok.exit_code, 0, "stderr: {}", ok.stderr);
    assert!(ok.stdout.contains("Converted 3/3 files cleanly"));
    let converted_lib = std::fs::read_to_string(out_dir.join("src").join("lib.rsharp")).unwrap();
    assert!(converted_lib.starts_with("public int double(int x)\n{\n"));
    assert!(
        out_dir
            .join("src")
            .join("vendor")
            .join("vendored.rsharp")
            .is_file()
    );
    assert!(!out_dir.join("vendor").exists());
    assert!(
        out_dir
            .join("src")
            .join("bin")
            .join("tool.rsharp")
            .is_file()
    );

    // Project conversion with `--no-ra --all-rs-files --no-raw-rust --verify`
    let no_ra_ok = run_cli(&args(&[
        "from-rust",
        "--project",
        &proj_str,
        "--no-ra",
        "--all-rs-files",
        "--no-raw-rust",
        "--verify",
    ]));
    assert_eq!(no_ra_ok.exit_code, 0, "stderr: {}", no_ra_ok.stderr);
    assert!(
        no_ra_ok.stdout.contains("0 rust{} blocks"),
        "stdout: {}",
        no_ra_ok.stdout
    );

    // `--no-raw-rust` fails when a file legitimately requires verbatim `rust { ... }`
    let raw_proj = base.join("raw_crate");
    std::fs::create_dir_all(raw_proj.join("src")).unwrap();
    std::fs::write(
        raw_proj.join("Cargo.toml"),
        "[package]\nname = \"raw_crate\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(
        raw_proj.join("src").join("lib.rs"),
        "type VerbatimOnly<T>;\n",
    )
    .unwrap();
    let raw_fail = run_cli(&args(&[
        "from-rust",
        "--project",
        &raw_proj.to_string_lossy(),
        "--no-ra",
        "--no-raw-rust",
    ]));
    assert_eq!(raw_fail.exit_code, 1);
    assert!(
        raw_fail.stderr.contains("rust{} fallback block(s)"),
        "stderr: {}",
        raw_fail.stderr
    );
    // Without `--no-raw-rust`, it succeeds and reports the block count
    let raw_ok = run_cli(&args(&[
        "from-rust",
        "--project",
        &raw_proj.to_string_lossy(),
        "--no-ra",
    ]));
    assert_eq!(raw_ok.exit_code, 0, "stderr: {}", raw_ok.stderr);
    assert!(
        raw_ok.stdout.contains("1 rust{} blocks"),
        "stdout: {}",
        raw_ok.stdout
    );

    // Single-file --workspace + --strict
    let ws_ok = run_cli(&args(&[
        "from-rust",
        &lib_rs_str,
        "--workspace",
        &proj_str,
        "--strict",
    ]));
    assert_eq!(ws_ok.exit_code, 0, "stderr: {}", ws_ok.stderr);
    assert!(ws_ok.stdout.contains("int y ="));

    // Project error collection: invalid Rust file + --strict on unnameable closure binding
    let bad_proj = base.join("bad_crate");
    std::fs::create_dir_all(bad_proj.join("src")).unwrap();
    std::fs::write(
        bad_proj.join("Cargo.toml"),
        "[package]\nname = \"bad_crate\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(bad_proj.join("src").join("bad.rs"), "fn broken( {").unwrap();
    let bad_out = run_cli(&args(&[
        "from-rust",
        "--project",
        &bad_proj.to_string_lossy(),
        "--strict",
    ]));
    assert_eq!(bad_out.exit_code, 1);
    assert!(bad_out.stderr.contains("with 1 error(s)"));

    // Output write failure when -o points to an existing regular file
    let blocking_file = base.join("blocking_file");
    std::fs::write(&blocking_file, "file").unwrap();
    let write_err = run_cli(&args(&[
        "from-rust",
        "--project",
        &proj_str,
        "-o",
        &blocking_file.to_string_lossy(),
    ]));
    assert_eq!(write_err.exit_code, 1);
    assert!(write_err.stderr.contains("with 3 error(s)"));

    // Oracle open failure via isolated child process env override
    let bin = env!("CARGO_BIN_EXE_rsharp");
    let bad_ra_ws = Command::new(bin)
        .env("RUST_ANALYZER", "nonexistent_ra_cli_test_xyz")
        .args(["from-rust", &lib_rs_str, "--workspace", &proj_str])
        .output()
        .unwrap();
    assert!(!bad_ra_ws.status.success());
    let bad_ra_proj = Command::new(bin)
        .env("RUST_ANALYZER", "nonexistent_ra_cli_test_xyz")
        .args(["from-rust", "--project", &proj_str])
        .output()
        .unwrap();
    assert!(!bad_ra_proj.status.success());

    let _ = std::fs::remove_dir_all(&base);
}
