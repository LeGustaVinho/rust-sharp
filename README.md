# R# (R Sharp)

[![Rust 2024](docs/badges/rust.svg)](Cargo.toml)
[![Workspace version 0.1.0](docs/badges/version.svg)](Cargo.toml)
[![MIT license](docs/badges/license.svg)](#license)
[![VS Code extension](docs/badges/vscode.svg)](editors/vscode/README.md)

R# is a programming language with C#-inspired syntax and Rust ownership semantics. It translates `.rsharp` files to Rust 2024 and uses `rustc` to type-check, borrow-check, and compile them to native binaries.

Use C#-style declarations, named tuples, and typed patterns with Rust's ownership model, traits, and standard library. The same CLI translates Rust source back to R# and supports inline Rust in `.rsharp` files.

Author Note: I'm a senior C# engineer, and when I discovered Rust, I loved it. It's the most powerful language I've ever seen, and its only drawback is that it's difficult to learn and read. In the age of AI, where we read very little code, I didn't want to learn a whole new and enormous Rust syntax, so that's why I thought about creating R#.

The main goal of R# is to be as powerful as Rust, but easy to read/understand and productive.

[Quick start](#quick-start) · [Language features](#language-features) · [R# and Rust side by side](#r-and-rust-side-by-side) · [CLI reference](#cli-reference) · [VS Code](#vs-code)

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

## Language features

- **Ownership and lifetimes.** Use `read` for shared borrows, `write` for exclusive mutable borrows, `take` to mark a move, and `deref` for dereferencing. Write lifetime parameters as `life a` and borrow slices with `read slice<T>` or `write slice<T>`. Rust checks lifetimes, aliasing, and uses after a move.
- **Declarations and data.** Put types before binding names and mark mutable bindings with `mut`. R# supports structs with inline methods, tuple structs, data-carrying enums, unions, and struct updates with `with`. Fixed-size arrays, multidimensional indexing, and named tuple fields lower to native Rust arrays and tuples.
- **Control flow and patterns.** Use C-style `for`, iterator-based `foreach`, and labeled loops and blocks. `if` and `while` accept `is` patterns and pattern chains. `match` supports guards with `when`, along with range, slice, and destructuring patterns. `require ... else` binds a successful pattern for the rest of the scope.
- **Optional values and errors.** `T?` maps to `Option<T>`, and `T??` is a nested option. `value ?? fallback` evaluates the fallback only for `None`. Use `try` for Rust-style early propagation from `Result` or `Option`.
- **Traits, generics, and callables.** Define `trait` and `impl` blocks, associated types, generic associated types, const generics, and `where` clauses. Use `dyn Trait` for trait objects, `impl Trait` for opaque returns or parameter bounds, `=>` lambdas for closures, and signatures like `(int) -> int` for function pointers.
- **Async and const evaluation.** `async Task<T>` and `await` lower to Rust async functions and `.await`, so the resulting futures use Rust's executor model. Const functions and inline const blocks lower to Rust compile-time constructs.
- **Strings and the standard library.** Interpolated strings lower to `format!`. The prelude provides `PrintLine`, collection helpers such as `Add` and `Enqueue`, and checked downcasts with `ToDynamic` and `AsDynamic<T>`. Native Rust names and methods such as `Vec<T>`, `push`, and `iter` remain available.
- **Rust interoperability.** Use `namespace` and `using` for Rust modules and imports, attributes such as `[Derive(Debug)]`, Rust-style macro calls, raw pointers in `unsafe` code, and `extern "C"` declarations. Embed Rust at item, statement, or expression level with `rust { ... }`.

### Syntax at a glance

| Concept | R# | Rust |
| --- | --- | --- |
| Local binding | `int count = 3;` | `let count: i32 = 3;` |
| Mutable binding | `mut int count = 3;` | `let mut count: i32 = 3;` |
| Shared and mutable borrows | `read value`, `write value` | `&value`, `&mut value` |
| Move a value into a call | `Consume(take value)` | `Consume(value)` |
| Borrowed slice with a lifetime | `read<a> slice<int>` | `&'a [i32]` |
| Method receivers | `read this`, `write this`, `take this` | `&self`, `&mut self`, `self` |
| Fixed-size array | `int[4]` | `[i32; 4]` |
| Iterate over a slice | `foreach (read int n in numbers)` | `for n in numbers` |
| Match one variant | `if (value is Some(int n))` | `if let Some(n) = value` |
| Bind or return early | `require (value is Some(int n)) else { return 0; }` | `let Some(n) = value else { return 0; };` |
| Match guard | `int n when n > 0 => n` | `n if n > 0 => n` |
| Capture by move | `take (int n) => n + offset` | `move \|n: i32\| n + offset` |
| Call generic methods | `values.collect<List<int>>()` | `values.collect::<Vec<i32>>()` |
| Import and rename | `using std.io.{Read, W = Write};` | `use std::io::{Read, Write as W};` |
| Derive implementations | `[Derive(Debug, Clone)]` | `#[derive(Debug, Clone)]` |
| Raw string literal | `@"C:\logs"` | `r"C:\logs"` |
| Interpolate an owned string | `$"Total {total}"` | `format!("Total {total}")` |
| Wait for an async result | `await FetchAsync()` | `fetch_async().await` |
| Inline const expression | `const(2 + 3)` | `const { 2 + 3 }` |
| Raw mutable address | `write ptr value` | `&raw mut value` |
| Precise capture in an opaque return type | `impl Display + capture<life a, T>` | `impl Display + use<'a, T>` |

### Standard library aliases

| R# | Rust type | Common operations |
| --- | --- | --- |
| `string`, `read str` | `String`, `&str` | `ToString()`, `Length()` |
| `List<T>` | `Vec<T>` | `Add(item)`, `Count()`, `IsEmpty()` |
| `Queue<T>` | `VecDeque<T>` | `Enqueue(item)`, `Dequeue()`, `Peek()` |
| `Dictionary<K, V>` | `HashMap<K, V>` | `Add(key, value)`, `ContainsKey(read key)` |
| `SortedDictionary<K, V>`, `SortedSet<T>` | `BTreeMap<K, V>`, `BTreeSet<T>` | `insert()`, `iter()` |
| `PriorityQueue<T>` | `BinaryHeap<T>` | `push(item)`, `pop()`, `peek()` |
| `dynamic` | `Box<dyn Any>` | `ToDynamic(value)`, `AsDynamic<T>(read value)` |

The helpers preserve Rust semantics. `PriorityQueue<T>` is a max-heap, `Dictionary.Add` replaces an existing entry and returns the previous value as an option, and `Length()` counts UTF-8 bytes.

## R# and Rust side by side

### Borrowing a slice and returning a reference

The lifetime on the return type ties the reference to the input slice. Both versions let the caller mutate a separate total through an exclusive borrow.

**R#**

```csharp
read<a> int First<life a>(read<a> slice<int> values)
{
    return read values[0];
}

void AddTo(write int total, int amount)
{
    deref total += amount;
}
```

**Rust**

```rust
fn first<'a>(values: &'a [i32]) -> &'a i32 {
    &values[0]
}

fn add_to(total: &mut i32, amount: i32) {
    *total += amount;
}
```

### Optional values and error propagation

`try` propagates a parse error to the caller. The default port is used when the option is empty.

**R#**

```csharp
Result<int, std.num.ParseIntError> ParsePort(read str input)
{
    int port = try input.parse<int>();
    return Ok(port);
}

int PortOrDefault(int? port)
{
    return port ?? 8080;
}
```

**Rust**

```rust
fn parse_port(input: &str) -> Result<i32, std::num::ParseIntError> {
    let port = input.parse::<i32>()?;
    Ok(port)
}

fn port_or_default(port: Option<i32>) -> i32 {
    port.unwrap_or_else(|| 8080)
}
```

### Matrices and named tuple fields

The matrix is a nested fixed-size Rust array, and the tuple field names correspond to tuple indices. Both functions return `6`.

**R#**

```csharp
int MatrixValue()
{
    int[2, 3] grid = [[1, 2, 3], [4, 5, 6]];
    (nuint Row, nuint Column) cell = (1, 2);
    return grid[cell.Row, cell.Column];
}
```

**Rust**

```rust
fn matrix_value() -> i32 {
    let grid: [[i32; 3]; 2] = [[1, 2, 3], [4, 5, 6]];
    let cell: (usize, usize) = (1, 2);
    grid[cell.0][cell.1]
}
```

### Structs and method receivers

Methods defined inside an R# struct become methods in a Rust `impl` block. `write this` gives the method exclusive access to update the struct.

**R#**

```csharp
[Derive(Debug)]
struct Counter
{
    int Value;

    void Increment(write this)
    {
        this.Value += 1;
    }
}
```

**Rust**

```rust
#[derive(Debug)]
struct Counter {
    value: i32,
}

impl Counter {
    fn increment(&mut self) {
        self.value += 1;
    }
}
```

## CLI reference

Install the CLI from the repository root to run `rsharp` directly.

```sh
cargo install --path crates/rsharp-cli --locked
```

| Command | What it does |
| --- | --- |
| `rsharp to-rust <file.rsharp> [-o <out.rs>]` | Generate Rust source. `--prelude embedded` (the default) includes the prelude helpers, `crate` imports them from `rsharp_prelude`, and `none` omits the prelude. |
| `rsharp from-rust <file.rs> [-o <out.rsharp>]` | Convert Rust source to R#. For standalone files, use `--no-rustc-infer` to disable the compiler-assisted type inference pass. |
| `rsharp check <file.rsharp>` | Check syntax, types, and borrows without producing an executable. |
| `rsharp build <file.rsharp> [-o <binary>]` | Compile a native executable. By default, it is written next to the source file, with an `.exe` extension on Windows. |
| `rsharp run <file.rsharp>` | Compile and run an R# program. |
| `rsharp lsp` | Start the R# language server over standard input and output. |

The single-file conversion commands write to standard output unless you pass `-o`. Rust-to-R# conversion depends on the constructs and type information available in the input. The converter can preserve some Rust constructs in inline `rust { ... }` blocks.

### Convert Rust with Cargo workspace context

Development builds that list `--workspace` and `--project` in `rsharp --help` can use `rust-analyzer` to resolve types using the Cargo workspace. Install `rust-analyzer` and `rust-src` for your Rust toolchain.

```sh
rustup component add rust-analyzer rust-src
rsharp from-rust path/to/project/src/lib.rs --workspace path/to/project -o lib.rsharp
rsharp from-rust --project path/to/project -o converted --verify
```

`--workspace` converts one file with workspace context. `--project` walks Rust source files and, with `-o`, writes the converted files under the output directory with their relative paths. `--verify` transpiles each result back to Rust as a conversion check.

Workspace-backed conversion can preserve bindings with unnameable types in inline Rust. Use `--strict` in either mode to report those bindings as errors.

## Examples and language reference

| Example | What it shows |
| --- | --- |
| [Bank accounts](examples/accounts.rsharp) | Structs, methods, borrowing, slices, and `Result` |
| [Matrices and control flow](examples/matrices_and_control.rsharp) | Matrices, named tuples, pattern matching, and error propagation |
| [Rust interoperability](examples/rust_interop.rsharp) | Traits, lifetimes, dynamic dispatch, and inline Rust |

The [language specification](docs/syntax-spec.md) covers grammar, standard library aliases, and how constructs translate between R# and Rust.

## VS Code

The extension uses the native `rsharp lsp` server.

- Syntax and semantic highlighting, including Rust highlighting inside inline Rust blocks.
- Live syntax diagnostics and optional Rust type and borrow checks on save.
- Completions, hover information, go-to-definition, document outline, and snippets.
- Commands to run and check R# files, preview the generated Rust, and convert Rust files back to R#.
- An optional prompt to convert a Rust-only Cargo project into a new folder of R# sources.
- Optional synchronization of saved R# edits to the original Rust modules for Cargo builds (`rsharp.syncRustOnSave`).

See the [extension guide](editors/vscode/README.md) for local installation and configuration.

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
