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
4. **Explicit Typing Preferred, `var` Supported**: Prefer explicit types whenever obvious or easily resolvable; C#-style `var` (`var x = expr;`, `mut var x = expr;`, `var (a, b) = expr;`) and inferred lambda parameters (`(x) => ...`) are supported for opaque, unnameable, or verbose types, while Rust's `let` keyword remains rejected. Trait objects (`dyn Trait`) and runtime dynamic values (`dynamic`, backed by `Box<dyn std::any::Any>`) are supported.
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
               | EnumDecl | UnionDecl | TraitDecl | TraitAliasDecl | ImplDecl
               | AliasDecl | AbstractTypeDecl | ConstDecl | StaticDecl | FnDecl
               | ExternBlock | MacroDecl | InlineRust | MacroCall ';' )

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
               | UnsafeBlock | TryBlock | Block | InlineRust | ExprStmt

LocalDecl     := 'shadow'? 'mut'? (Type | 'var') Ident ('=' Expr)? ';'
               | 'var' Pattern (':' Type)? ('=' Expr)? ';'
               | Pattern (':' Type)? '=' Expr ';'
```

## 4. Rust 2024 Parity Extensions

| Rust 2024 Construct | R# Syntax |
| :--- | :--- |
| `r"..."` / `r#"..."#` | `@"..."` / `"""..."""` |
| `b'a'`, `b"hi"`, `br#"hi"#` | `b'a'`, `"hi"u8`, `@"hi"u8` / `"""hi"""u8` |
| `c"hi"`, `cr#"hi"#` | `c"hi"`, `c@"hi"` / `c"""hi"""` |
| `if let Some(a) = x && let Some(b) = y && a > b` | `if (x is Some(int a) && y is Some(int b) && a > b)` |
| `&raw const x` / `&raw mut x` | `read ptr x` / `write ptr x` |
| `const { expr }` / `const { ... }` | `const(expr)` / `const { ... return expr; }` |
| `async \|x: i32\| -> i32 { ... }` | `async (int x) : Task<int> => ...` |
| `<Dog as Animal>::speak(&d)` / `<I as Iterator>::Item` | `(Dog as Animal).speak(read d)` / `(I as Iterator).Item` |
| `Point { x, y }` | `new Point { x, y }` |
| `'a: loop { break 'a 42; }` | `a: loop { break a 42; }` |
| `[]`, `[first, mid @ .., last]` | `[]`, `[int first, .. read slice<int> mid, int last]` |
| `n @ 1..=5` | `1..=5 as int n` |
| `Some(ref s)` / `Some(ref mut s)` / `&val` | `Some(borrow read string s)` / `Some(borrow write string s)` / `deref int val` |
| `..=59`, `..60`, `90..`, `(a, .., z)` | `..=59`, `..60`, `90..`, `(int a, .., int z)` |
| `const { MAX - 1 }` (pattern) | `const(MAX - 1)` |
| `let Point { x, y } = p;`, `fn f((a, b): (i32, i32))` | `Point { int x; int y; } = p;`, `void F((int a, int b))` |
| `T: ?Sized` | `T : maybe Sized` |
| `for<'a> Fn(&'a str) -> &'a str` | `forall<life a> Fn(read<a> str) -> read<a> str` |
| `struct Buf<T = u8, const N: usize = 64>` | `struct Buf<T = byte, const nuint N = 64>` |
| `type Item<'a> where Self: 'a;` (GAT) | `type Item<life a> where Self : life a;` |
| `impl Display + use<'a, T>` | `impl Display + capture<life a, T>` |
| `unsafe extern "C" fn(i32) -> i32` | `unsafe extern "C" (int) -> int` |
| `Formatter<'_>`, `&'_ str`, `&(dyn A + B)` | `Formatter<life _>`, `read<_> str`, `read (dyn A + B)` |
| `collect::<Vec<_>>()` | `collect<List<_>>()` |
| `use std::io::{self, Read, Write as W};` | `using std.io.{this, Read, W = Write};` |
| `pub(in crate::net)`, `pub(self)` | `public(in crate.net)`, `public(this)` |
| `unsafe extern "C" { pub safe fn f(); static E: i32; fn p(s: *const i8, ...); }` | `unsafe extern "C" { public safe void f(); static int E; void p(read ptr<sbyte> s, ...); }` |
| `#[unsafe(no_mangle)]`, `#[unsafe(export_name = "f")]` | `[unsafe NoMangle]`, `[unsafe ExportName = "f"]` |
| `extern crate alloc;` | `extern crate alloc;` |
| `Ok(a) if let Some(b) = opt && b > 0 => ...` *(Rust 1.95)* | `Ok(int a) when opt is Some(int b) && b > 0 => ...` |
| `pub unsafe extern "C" fn f(n: i32, mut args: ...) -> i32` *(Rust 1.99)* | `public unsafe extern "C" int f(int n, mut ... args)` |
| `[false; _]` / `make::<_>()` *(Rust 1.89)* | `[false; _]` / `make<_>()` |
| `#[unsafe(naked)]` *(Rust 1.88)* | `[unsafe Naked]` |
| `return expr` / `break lbl expr` / `continue lbl` *(as expr)* | `return expr` / `break lbl expr` / `continue lbl` |
| `if let Some(v) = o { v } else { 0 }` *(expr)* | `if (o is Some(int v)) => v else => 0` |
| `(a, b) = (b, a);` *(destructuring assign)* | `(a, b) = (b, a);` *(no types = assign; typed = decl)* |
| `Iterator<Item: Display>`, `Lending<Item<'a> = &'a str>` | `Iterator<Item : Display>`, `Lending<Item<life a> = read<a> str>` |
| `struct S<'a, 'b: 'a + 'static>` | `struct S<life a, life b : a + static>` |
| `unsafe extern "C" fn(*const i8, ...) -> i32`, `fn(x: i32) -> bool` | `unsafe extern "C" (read ptr<sbyte>, ...) -> int`, `(int x) -> bool` |
| `fn f(mut self)`, `fn g(mut self: Box<Self>)` | `void f(mut take this)`, `void g(mut Box<Self> this)` |
| `my_ty!()`, `my_pat!()`, `trait/impl/extern { m!(); }` | `my_ty!()`, `my_pat!()`, `trait/impl/extern { m!(); }` |
| `::std::fmt::Display`, `use {a, b};`, `self::foo()` | `global::std.fmt.Display`, `using { a, b };`, `this.foo()` |
| `#![allow(unused)]` *(inside fn/trait/impl/extern/block)* | `[inner: Allow(unused)]` *(and `[module: ...]` in file/namespace)* |
| `//!`, `/** ... */`, `/*! ... */` | `//!`, `/** ... */`, `/*! ... */` |
| `S { ..base }`, `Pair { 0: 1, ..p }`, `Pair { 0: a, .. }` | `base with {}`, `p with { 0 = 1 }`, `Pair { 0: int a, .. }` |
| `&mut v`, `&(1 \| 2)`, `(x,)` | `deref write int v`, `deref (1 \| 2)`, `(int x,)` |
| `r#type`, `'r#async`, `'r#loop`, `1.`, `b"\xff\x00"` | `@type`, `<life @async>`, `@loop:`, `1.`, `"\xff\x00"u8` |
| `macro_rules! m ( ... );` / `[ ... ];` | `macro m ( ... );` / `[ ... ];` |

### Wave 3 Parity Extensions (Stable Rust 1.99.1 Completeness)

| Rust 2024 Construct | R# Syntax |
| :--- | :--- |
| `<Vec<i32>>::new()`, `<[i32]>::len(&v)` | `(List<int>).new()`, `(slice<int>).len(read v)` (or `(<Type>)::member` for any type) |
| `<<T as A>::B as C>::X`, `<S as Tr>::A::B` | `((T as A).B as C).X`, `(S as Tr).A::B` |
| `S::<i32> { x: 1 }`, `::m::S { x: 1 }` | `new S<int> { x = 1 }`, `new global::m.S { x = 1 }` |
| `Point { x: 1, ..Point::default() }` | `new Point { X = 1, ..Point.default() }` (alongside `p with { X = 1 }` for locals / `this`) |
| `<S as Tr>::C`, `<E>::A`, `E::<i32>::A(v)`, `::core::option::Option::Some(v)` *(patterns)* | `(S as Tr).C`, `(E)::A`, `E<int>.A(int v)`, `global::core.option.Option.Some(int v)` |
| `LO..=HI`, `..=LO`, `<S as Tr>::MIN..` *(range patterns)* | `LO..=HI`, `..=LO`, `(S as Tr).MIN..` |
| `\| A \| B` *(leading or-pattern bar)* | `\| A \| B` |
| `Some(ref mut v @ 1..=5)`, `[_, ref mut mid @ .., _]` | `Some(1..=5 as borrow write int v)`, `[_, .. borrow write int[2] mid, _]` |
| `Point { #[allow(unused)] ref x, ref mut y }` | `Point { [Allow(unused)] borrow read int X, borrow write int Y }` |
| `let W(v) = w;`, `W(a) = w;`, `let &v = r;`, `let ref r = x;`, `let (v) = 1;` | `W(int v) = w;`, `W(a) = w;`, `deref int v = r;`, `borrow read int r = x;`, `(int v) = 1;` |
| `fn f(W(a): W, &b: &i32, ref c: i32)`, `\|(a, b): (i32, i32)\| { let s = a + b; s }` | `void f(W(int a) : W, deref int b, borrow read int c)`, `((int a, int b)) => { int s = a + b; return s; }` |
| `T: (Clone) + (?Sized)`, `Box<dyn (Fn() -> i32) + Send>` | `T : (Clone) + (maybe Sized)`, `Box<dyn (Fn() -> int) + Send>` |
| `T: Clone +`, `where T:,` *(trailing `+` and empty predicate bounds)* | `T : Clone +`, `where T:,` |
| `C<'a'>`, `B<b'x'>`, `[i32; 2 + 2]` *(const generic char/byte & exprs)* | `C<'a'>`, `B<b'x'>`, `int[2 + 2]` |
| `fn helper(x: i32) -> i32 { ... }`, `mod inner { ... }` *(local items in blocks)* | `int helper(int x) { ... }`, `namespace inner { ... }` |
| `{ let x = 1; }`, `;`, `m! { ... }` *(block stmt, empty stmt, brace macro stmt)* | `{ int x = 1; }`, `;`, `m! { ... }` |
| `t.0.1` *(chained tuple indexing)* | `t.0.1` |
| `#!/usr/bin/env ...`, `extern crate self as alias;` | `#!/usr/bin/env ...`, `extern crate this as alias;` |
| `#[forbid(...)]`, `#[expect(...)]`, `#[a[x]]`, `#[a{ x }]`, `#[doc = concat!(...)]` | `[Forbid(...)]`, `[Expect(...)]`, `[a[x]]`, `[a{ x }]`, `[Doc = concat!(...)]` |
| Attributes on `self` receivers, tuple-struct/union/enum-named fields, assoc items/macros, fn-ptr params & `...` | `[Allow(...)] read this`, `[Allow(...)] public int`, `[Allow(...)] ...` |
| `Option<Option<T>>`, `let (a, b): (T1, T2) = expr;` | `T??`, `(a, b) : (T1, T2) = expr;` |
| `\|_\|\|`, `\|(i, _)\|`, `let (a, mut b) = pair;` | `(_) => ...`, `((nuint i, _)) => ...`, `(int a, mut b) = pair;` |
| `Foo<{ let k = 1; k + 1 }>` *(multi-statement const generic block)* | `Foo<{ int k = 1; return k + 1; }>` |

### Zero-Fallback & Nightly AST Parity Extensions

| Rust Construct | R# Syntax |
| :--- | :--- |
| `let x = opaque();`, `let mut x = expr;` | `var x = opaque();`, `mut var x = expr;` *(prefer explicit type when obvious)* |
| `let (a, mut b) = pair;`, `let (a, b);`, `let ref r = x;` | `var (a, mut b) = pair;`, `var (a, b);`, `var borrow read r = x;` |
| `\|x\| x + 1`, `\|mut a, b\| a + b` *(inferred closure params)* | `(x) => x + 1`, `(mut a, b) => a + b` |
| `let v = if c { let t = 1; t } else { 2 };` *(multi-stmt value `if`)* | `var v = if (c) { int t = 1; return t; } else { return 2; };` |
| `let v = { let t = 1; t + 1 };`, `&{ p.0 }` *(value / move block)* | `var v = { int t = 1; return t + 1; };`, `read { return p.0; }` |
| `pub trait CopyClone<T> = Clone + Copy where T: Send;` | `public trait CopyClone<T> = Clone + Copy where T : Send;` |
| `auto trait MyAuto {}`, `unsafe auto trait U {}` | `auto trait MyAuto {}`, `unsafe auto trait U {}` |
| `default impl Tr for T {}`, `default unsafe impl Send for T {}` | `default impl Tr for T {}`, `default unsafe impl Send for T {}` |
| `impl !Sync for T {}` | `impl !Sync for T {}` |
| `extern "C" { pub type Opaque; }`, `impl T { type Elem; }`, `type bool;` | `extern "C" { public type Opaque; }`, `impl T { type Elem; }`, `type bool;` |
| `try { let x = f()?; Ok(x) }` | `try { var x = try f(); return Ok(x); }` |
| `yield expr`, `yield` | `yield expr`, `yield` |
| `!(let Some(_) = opt)` *(boolean `Expr::Let`)* | `!((opt is Some(_)))` |

> **Intentionally Deferred (Experimental Nightly-Only)**: `gen` blocks & functions, `const trait` / `~const`, associated const equality (`Tr<C = 1>`), `for<'a> \|...\|` closure binders, `become` tail calls, `box` patterns, `default fn` specialization, `const { ... }` patterns, `more_qualified_paths` in struct literals, `const \|\|` / `static \|\|`, postfix `.use` / `.match`, and generic/verbatim top-level `type T<X>;` declarations (safely preserved in `rust { ... }` during conversion).

## 5. Bidirectional Conversion Rules

- **R# -> Rust**:
  - `for (init; cond; step) { body }` desugars to a loop preserving exact `continue` / `break` / label semantics.
  - `T[R, C]` desugars to `[[T; C]; R]` and `m[r, c]` desugars to `m[r][c]`.
  - Named tuples `(T0 A, T1 B)` desugar to `(T0, T1)` with static field-to-index resolution (`p.A` -> `p.0`).
  - `opt ?? fallback` desugars to `(opt).unwrap_or_else(|| fallback)`.
  - `$"...{expr}..."` desugars to `format!("...{}...", expr)`.
  - `try expr` desugars to `(expr)?` and `await expr` desugars to `(expr).await`.
  - `p with { X = 1 }` desugars to `Type { X: 1, ..p }`.
  - Value blocks (`{ ... return expr; }`, `if (...) { ... return expr; }`, `try { ... return expr; }`) lift trailing `return expr;` into Rust tail expressions (wrapping `#[cfg]`-gated returns in `{ expr }`), while preserving early function returns via `return (return expr);`.
  - `var` bindings lower directly to `let [mut] pat [= init];`, and typed pattern bindings (`foreach`, `if is`, `while is`, `require is`, `match`) emit zero-cost compile-time type checks (`let _: &T = &x;`) enforced by `rustc`.
- **Rust -> R#**:
  - Parses Rust via `syn` (full syntax).
  - Discovers compiled Cargo target files via `cargo metadata --no-deps --format-version 1` in `--project` mode (automatically excluding non-member fixtures and templates; pass `--all-rs-files` to walk all `.rs` files).
  - Prefers explicit types whenever obvious or safely resolved via local type inference, standalone `rustc` type probing, or batch Cargo-workspace hints from `rust-analyzer` (`rsharp from-rust --project <dir> [-o <out_dir>] [--verify] [--strict] [--no-ra] [--all-rs-files] [--no-raw-rust]`).
  - Emits native `var` bindings and inferred lambda parameters `(x) => ...` (instead of `rust { ... }`) whenever a type is unnameable, opaque, `#[cfg]`-inactive, unresolvable without `rust-analyzer`, or unsafe to annotate explicitly (such as elided-reference closure parameters or higher-ranked `fn(&T) -> ...` pointers), unless `--strict` is enabled.
  - Converts implicit tail returns into explicit `return expr;` statements, and converts `&`, `&mut`, `&'a`, `&[T]`, `*const`, `*mut`, `::`, `#[...]`, `?`, `.await`, `for`, `if let`, `while let`, `let ... else`, `match`, `..base`, closures, value blocks, and stdlib types into idiomatic R#.
  - Reserves inline `rust { ... }` strictly for unsupported experimental nightly verbatim constructs; pass `--no-raw-rust` to enforce zero `rust { ... }` blocks in CI/project conversions.
