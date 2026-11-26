# R# (R Sharp)

[![Rust 2024](https://img.shields.io/badge/Rust-2024-CE412B?logo=rust&logoColor=white)](Cargo.toml)
[![Workspace version 0.1.0](https://img.shields.io/badge/version-0.1.0-2F6FEB)](Cargo.toml)
[![MIT license](https://img.shields.io/badge/license-MIT-22863A)](#license)
[![VS Code extension](https://img.shields.io/badge/VS%20Code-extension-007ACC)](editors/vscode/README.md)

R# is a programming language with C#-inspired syntax and Rust ownership semantics. It translates `.rsharp` files to Rust 2024 and uses `rustc` to type-check, borrow-check, and compile them to native binaries.

Write explicit types and method signatures, use `read` and `write` for shared and mutable borrows, and propagate errors with `try`. You can also convert Rust source to R# or embed Rust directly in a `rust { ... }` block.

## Quick start

Install the stable Rust toolchain and make sure `cargo` and `rustc` are on your PATH. Clone the repository if you do not already have a local copy.

```sh
git clone https://github.com/LeGustaVinho/rust-sharp.git
cd rust-sharp
```

Save this program as `hello.rsharp` in the repository root.

```csharp
int Double(read int value)
{
    return (deref value) * 2;
}

void main()
{
    int number = 21;
    int result = Double(read number);
    PrintLine($"The answer is {result}");
}
```

```sh
cargo run -p rsharp-cli -- run hello.rsharp
```

The program prints `The answer is 42`. To see how R# translates the borrow and dereference, emit the Rust source without the embedded prelude.

```sh
cargo run -p rsharp-cli -- to-rust hello.rsharp --prelude none
```

## CLI reference

Install the CLI from the repository root to run `rsharp` directly.

```sh
cargo install --path crates/rsharp-cli --locked
```

| Command | What it does |
| --- | --- |
| `rsharp to-rust <file.rsharp> [-o <out.rs>]` | Generate Rust source. The `--prelude` option accepts `embedded` (default), `crate`, or `none`. |
| `rsharp from-rust <file.rs> [-o <out.rsharp>]` | Convert Rust source to R#. For standalone files, use `--no-rustc-infer` to disable the compiler-assisted type inference pass. |
| `rsharp check <file.rsharp>` | Check syntax, types, and borrows without producing an executable. |
| `rsharp build <file.rsharp> [-o <binary>]` | Compile a native executable. By default, it is written next to the source file, with an `.exe` extension on Windows. |
| `rsharp run <file.rsharp>` | Compile and run an R# program. |
| `rsharp lsp` | Start the R# language server over standard input and output. |

The single-file conversion commands print their output to standard output unless you pass `-o`. Rust-to-R# conversion depends on the constructs and type information available in the input. Inline Rust blocks can preserve constructs that have no direct R# equivalent.

## Examples and language reference

| Example | What it shows |
| --- | --- |
| [Bank accounts](examples/accounts.rsharp) | Structs, methods, borrowing, slices, and `Result` |
| [Matrices and control flow](examples/matrices_and_control.rsharp) | Matrices, named tuples, pattern matching, and error propagation |
| [Rust interoperability](examples/rust_interop.rsharp) | Traits, lifetimes, dynamic dispatch, and inline Rust |

The [language specification](docs/syntax-spec.md) covers grammar, standard library aliases, and how constructs translate between R# and Rust.

## VS Code

The extension provides syntax and semantic highlighting, live diagnostics, completions, go-to-definition, and a generated Rust preview. It uses the native `rsharp lsp` server and can run Rust type and borrow checks on save. See the [extension guide](editors/vscode/README.md) for local installation and configuration.

## Development

Run the Rust workspace tests from the repository root.

```sh
cargo test --workspace
```

- [`rsharp-core`](crates/rsharp-core) implements parsing, code generation, and conversion from Rust.
- [`rsharp-cli`](crates/rsharp-cli) exposes the `rsharp` command.
- [`rsharp-lsp`](crates/rsharp-lsp) implements the language server.
- [`rsharp-prelude`](crates/rsharp-prelude) provides type aliases and helper functions for R# programs.

## License

The [workspace manifest](Cargo.toml) declares the MIT license.
