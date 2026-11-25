# R# (R Sharp) Language Specification

File extension: `.rsharp`

## 1. Design Rules

1. **Semantic Superset of Rust**: Every Rust construct maps cleanly to R#; inline `rust { ... }` blocks provide direct raw Rust execution at item, statement, or expression level.
2. **Words Over Sigils**:
   - `&T` / `&x` -> `read T` / `read x`
   - `&mut T` / `&mut x` -> `write T` / `write x`
   - ownership move / `move ||` -> `take x` / `take (...) => ...`
   - `&[T]` / `&mut [T]` -> `read slice<T>` / `write slice<T>`
   - `*x` -> `deref x`
   - `*const T` / `*mut T` -> `read ptr<T>` / `write ptr<T>`
   - `'a` -> `life a` in declarations, `read<a> T` / `write<a> T` in usage, `read<static> T` for `'static`
   - `expr?` -> `try expr`
   - `::` -> `.`
   - `#[attr]` / `#![attr]` -> `[Attr]` / `[module: Attr]`
3. **Immutable by Default**: `int x = 10;` is immutable; `mut int x = 10;` is mutable.
4. **Explicit Typing Only**: `var` and untyped `let` are rejected. Trait objects (`dyn Trait`) and runtime dynamic values (`dynamic`, backed by `Box<dyn std::any::Any>`) are supported.
5. **Explicit Returns Only**: Functions and methods always require `return expr;` (or `return;` for `void`).
6. **Traits Stay `trait`**: Trait declarations use `trait` and `impl Trait for Type`.
7. **Method Receivers**: Explicit `read this` (`&self`), `write this` (`&mut self`), `take this` (`self`), or `static` for associated functions.
8. **Minimal v1 Identifier Interop**: Identifiers preserve their exact spelling during transpilation; `rsharp-prelude` provides C#-friendly type aliases and minimal ergonomic extension helpers.

## 2. Type & Standard Library Aliases

| Rust | R# Alias (native Rust name also valid) | C# Equivalent |
| :--- | :--- | :--- |
| `i8` / `u8` | `sbyte` / `byte` | `sbyte` / `byte` |
| `i16` / `u16` | `short` / `ushort` | `short` / `ushort` |
| `i32` / `u32` | `int` / `uint` | `int` / `uint` |
| `i64` / `u64` | `long` / `ulong` | `long` / `ulong` |
| `i128` / `u128` | `int128` / `uint128` | `Int128` / `UInt128` |
| `isize` / `usize` | `nint` / `nuint` | `nint` / `nuint` |
| `f32` / `f64` | `float` / `double` | `float` / `double` |
| `bool` / `char` | `bool` / `char` | `bool` / `char` |
| `()` / `!` | `void` / `never` | `void` / `[DoesNotReturn]` |
| `String` / `&str` | `string` / `read str` | `string` / `ReadOnlySpan<char>` |
| `Vec<T>` | `List<T>` | `List<T>` |
| `VecDeque<T>` | `Queue<T>` | `Queue<T>` |
| `HashMap<K, V>` | `Dictionary<K, V>` | `Dictionary<K, V>` |
| `HashSet<T>` | `HashSet<T>` | `HashSet<T>` |
| `BTreeMap<K, V>` | `SortedDictionary<K, V>` | `SortedDictionary<K, V>` |
| `BTreeSet<T>` | `SortedSet<T>` | `SortedSet<T>` |
| `LinkedList<T>` | `LinkedList<T>` | `LinkedList<T>` |
| `BinaryHeap<T>` | `PriorityQueue<T>` | `PriorityQueue<T, P>` |
| `Option<T>` | `T?` or `Option<T>` | `T?` |
| `Result<T, E>` | `Result<T, E>` | Result / Exceptions |
| `Box<dyn Any>` | `dynamic` | `dynamic` / `object` |

## 3. Core Grammar Overview (EBNF Summary)

```ebnf
File          := InnerAttr* Item*
Item          := OuterAttr* Visibility? ( NamespaceDecl | UsingDecl | StructDecl
               | EnumDecl | UnionDecl | TraitDecl | ImplDecl | AliasDecl
               | ConstDecl | StaticDecl | FnDecl | ExternBlock | MacroDecl
               | InlineRust | MacroCall ';' )

Type          := ('read' | 'write') LifetimeArg? ( 'slice' '<' Type '>' | 'ptr' '<' Type '>' | 'dyn' TraitBounds | Type )
               | 'slice' '<' Type '>'
               | 'impl' TraitBounds
               | 'dyn' TraitBounds
               | 'void' | 'never' | 'dynamic'
               | '(' TupleTypeElems ')'
               | BasePath TypeArgs? ArrayDims? '?'?

ArrayDims     := '[' Expr (',' Expr)* ']'       (* int[4] -> [i32; 4], int[2, 3] -> [[i32; 3]; 2] *)

FnDecl        := 'const'? 'async'? 'unsafe'? ('extern' StringLit?)? 'static'?
                 ReturnType Ident GenericParams? '(' ParamList? ')' WhereClause? (Block | ';')

Stmt          := LocalDecl | ConstDecl | StaticDecl | Item
               | IfStmt | RequireStmt | WhileStmt | ForStmt | ForeachStmt
               | LoopStmt | MatchStmt | ReturnStmt | BreakStmt | ContinueStmt
               | UnsafeBlock | InlineRust | ExprStmt

LocalDecl     := 'shadow'? 'mut'? Type Ident ('=' Expr)? ';'
               | '(' TypedBinding (',' TypedBinding)* ')' '=' Expr ';'
```

## 4. Bidirectional Conversion Rules

- **R# -> Rust**:
  - `for (init; cond; step) { body }` desugars to a loop preserving exact `continue` / `break` / label semantics.
  - `T[R, C]` desugars to `[[T; C]; R]` and `m[r, c]` desugars to `m[r][c]`.
  - Named tuples `(T0 A, T1 B)` desugar to `(T0, T1)` with static field-to-index resolution (`p.A` -> `p.0`).
  - `opt ?? fallback` desugars to `(opt).unwrap_or_else(|| fallback)`.
  - `$"...{expr}..."` desugars to `format!("...{}...", expr)`.
  - `try expr` desugars to `(expr)?` and `await expr` desugars to `(expr).await`.
  - `p with { X = 1 }` desugars to `Type { X: 1, ..p }`.
  - Pattern bindings (`foreach`, `if is`, `while is`, `require is`, `match`) emit zero-cost compile-time type checks (`let _: &T = &x;`) enforced by `rustc`.
- **Rust -> R#**:
  - Parses Rust via `syn` (full syntax).
  - Resolves untyped `let` bindings using local type inference plus optional `rustc` compiler type probing, emitting an explicit `UnresolvedType` error if a standalone snippet cannot be resolved.
  - Converts implicit tail returns into explicit `return expr;` statements.
  - Converts `&`, `&mut`, `&'a`, `&[T]`, `*const`, `*mut`, `::`, `#[...]`, `?`, `.await`, `for`, `if let`, `while let`, `let ... else`, `match`, `..base`, closures, and stdlib types into idiomatic R#.
  - Falls back to `rust { ... }` for rare macro-heavy constructs that have no direct C#-style representation so no Rust semantics are ever lost.
