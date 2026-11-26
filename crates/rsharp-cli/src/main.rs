use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Run on a 16 MB stack thread so deeply nested Rust expressions never overflow Windows' 1 MB default stack.
    let out = std::thread::Builder::new()
        .name("rsharp-main".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || rsharp_cli::run_cli(&args))
        .unwrap()
        .join()
        .unwrap();
    if !out.stdout.is_empty() {
        print!("{}", out.stdout);
    }
    if !out.stderr.is_empty() {
        eprint!("{}", out.stderr);
    }
    ExitCode::from(out.exit_code as u8)
}
