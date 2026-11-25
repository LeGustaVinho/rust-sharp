# R# (R Sharp)

**Rust Performance. C# Productivity.**

R# (`.rsharp`) is a semantic superset of Rust with a C#-inspired syntax. Everything expressible in Rust is expressible in R#, complex Rust idioms are simplified with readable keywords (`read`, `write`, `take`, `slice`, `deref`, `life`, `try`), and bidirectional conversion lets you seamlessly convert **R# -> Rust**, convert **Rust -> R#**, and execute inline `rust { ... }` directly.

## Core Principles

- **Words over sigils**: `read T` (`&T`), `write T` (`&mut T`), `take x` (move), `read slice<T>` (`&[T]`), `deref p` (`*p`), `life a` (`'a`), `try expr` (`expr?`).
- **C#-style syntax**: explicit variable declarations (`int x = 10;`, `mut int x = 10;`), C# method signatures, `if`/`else`/`while`/`for`/`foreach`/`loop`/`match`, fixed arrays & matrices (`int[2, 3]`), named tuples (`(int X, int Y)`), generics (`where T : A, B`), `async Task<T>` / `await`, attributes (`[Derive(Debug)]`), and comments (`//`, `/* */`, `///`).
- **Strict explicitness**: No `var` / implicit variable typing (dynamic dispatch `dyn Trait` and runtime `dynamic` are supported); no implicit returns (`return` is always required in functions/methods); `trait` keeps its name.
- **Native Rust ecosystem**: Native Rust stdlib names remain valid alongside C#-friendly aliases (`List<T>`, `Queue<T>`, `Dictionary<K, V>`, `SortedDictionary<K, V>`, `SortedSet<T>`, `PriorityQueue<T>`, `string`, `T?`, etc.).

## CLI Usage

```bash
# Transpile R# to Rust
cargo run -p rsharp-cli -- to-rust examples/accounts.rsharp

# Compile and run an R# file directly via rustc
cargo run -p rsharp-cli -- run examples/accounts.rsharp

# Compile an R# file to a native binary
cargo run -p rsharp-cli -- build examples/accounts.rsharp -o accounts.exe

# Convert Rust source to R#
cargo run -p rsharp-cli -- from-rust path/to/file.rs

# Syntax & semantic check via rustc
cargo run -p rsharp-cli -- check examples/accounts.rsharp
```

## Documentation

See [docs/syntax-spec.md](docs/syntax-spec.md) for the complete Rust vs. R# vs. C# syntax comparison and formal grammar rules.
