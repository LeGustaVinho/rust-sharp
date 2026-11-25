pub mod ast;
pub mod codegen_rust;
pub mod error;
pub mod from_rust;
pub mod lexer;
pub mod parser;
pub mod rustc_runner;
pub mod token;

use std::path::Path;

pub use codegen_rust::{PreludeMode, TranspileOptions};
pub use error::{RSharpError, Result, Span};
pub use from_rust::FromRustOptions;
pub use rustc_runner::RunOutput;

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

/// Transpiles and semantically checks an R# source string via `rustc --emit=metadata`.
pub fn check_rsharp(source: &str) -> Result<()> {
    let rust_src = transpile_to_rust(
        source,
        TranspileOptions {
            prelude_mode: PreludeMode::Embedded,
        },
    )?;
    let is_lib = !rust_src.contains("fn main(") && !rust_src.contains("fn main (");
    rustc_runner::check_rust_source(&rust_src, is_lib)
}
