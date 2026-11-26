/// Classification of built-in R# language entries for hover and completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    Keyword,
    PrimitiveType,
    StdType,
    Function,
}

/// Documentation and signature entry for an R# keyword, primitive, or prelude item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinEntry {
    pub name: &'static str,
    pub kind: BuiltinKind,
    pub detail: &'static str,
    pub doc: &'static str,
}

/// Built-in member documentation for prelude and core standard types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberBuiltin {
    pub owner: &'static str,
    pub name: &'static str,
    pub detail: &'static str,
    pub doc: &'static str,
}

pub const BUILTINS: &[BuiltinEntry] = &[
    // Ownership & words-over-sigils keywords
    BuiltinEntry {
        name: "read",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: &T / &x)",
        doc: "Immutable borrow / shared reference (`read T` -> `&T`, `read<a> T` -> `&'a T`, `read this` -> `&self`).",
    },
    BuiltinEntry {
        name: "write",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: &mut T / &mut x)",
        doc: "Mutable borrow / exclusive reference (`write T` -> `&mut T`, `write<a> T` -> `&'a mut T`, `write this` -> `&mut self`).",
    },
    BuiltinEntry {
        name: "take",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: move / self)",
        doc: "Explicit ownership transfer (`take this` -> `self`, `take (...) => ...` -> `move |...| ...`).",
    },
    BuiltinEntry {
        name: "borrow",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: ref / ref mut)",
        doc: "Pattern borrow binding (`borrow read T x` -> `ref x`, `borrow write T x` -> `ref mut x`).",
    },
    BuiltinEntry {
        name: "slice",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: [T])",
        doc: "Dynamically sized slice type (`read slice<T>` -> `&[T]`, `write slice<T>` -> `&mut [T]`).",
    },
    BuiltinEntry {
        name: "ptr",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: *const T / *mut T)",
        doc: "Raw pointer type or operator (`read ptr<T>` -> `*const T`, `write ptr<T>` -> `*mut T`).",
    },
    BuiltinEntry {
        name: "deref",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: *expr)",
        doc: "Dereference operator (`deref p` -> `*p`).",
    },
    BuiltinEntry {
        name: "life",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: 'a)",
        doc: "Lifetime parameter declaration (`<life a>` -> `<'a>`).",
    },
    BuiltinEntry {
        name: "forall",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: for<'a>)",
        doc: "Higher-ranked trait bound lifetime quantifier (`forall<life a> Fn(...)`).",
    },
    BuiltinEntry {
        name: "try",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: expr? / try { ... })",
        doc: "Error/Option propagation operator (`try expr` -> `(expr)?`) or `try { ... }` block expression.",
    },
    BuiltinEntry {
        name: "shadow",
        kind: BuiltinKind::Keyword,
        detail: "keyword",
        doc: "Explicitly shadows an existing binding in the current scope.",
    },
    BuiltinEntry {
        name: "require",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: let ... else)",
        doc: "Refutable pattern guard with diverging `else` block (`require (x is Some(int v)) else { return; }`).",
    },
    BuiltinEntry {
        name: "rust",
        kind: BuiltinKind::Keyword,
        detail: "keyword",
        doc: "Inline raw Rust block (`rust { ... }`) embedded directly at item, statement, or expression level.",
    },
    BuiltinEntry {
        name: "macro",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: macro_rules!)",
        doc: "Declarative macro definition (`macro name { ... }`).",
    },
    BuiltinEntry {
        name: "dyn",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: dyn Trait)",
        doc: "Dynamic trait object dispatch (`read dyn Trait`).",
    },
    BuiltinEntry {
        name: "impl",
        kind: BuiltinKind::Keyword,
        detail: "keyword",
        doc: "Trait or inherent implementation block (`impl Trait for Type`, `default impl`, `impl !Trait for Type`), or opaque return type (`impl Trait`).",
    },
    BuiltinEntry {
        name: "alias",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: type Alias = T)",
        doc: "Type alias declaration (`alias UserId = ulong;`).",
    },
    BuiltinEntry {
        name: "type",
        kind: BuiltinKind::Keyword,
        detail: "keyword",
        doc: "Associated type declaration inside a `trait`/`impl` block, or abstract/extern opaque type (`type Opaque;`).",
    },
    BuiltinEntry {
        name: "struct",
        kind: BuiltinKind::Keyword,
        detail: "keyword",
        doc: "Struct type declaration supporting fields and C#-style inline methods.",
    },
    BuiltinEntry {
        name: "enum",
        kind: BuiltinKind::Keyword,
        detail: "keyword",
        doc: "Algebraic enum type declaration supporting unit, tuple, and struct variants plus inline methods.",
    },
    BuiltinEntry {
        name: "union",
        kind: BuiltinKind::Keyword,
        detail: "keyword",
        doc: "Untagged C-compatible union declaration.",
    },
    BuiltinEntry {
        name: "trait",
        kind: BuiltinKind::Keyword,
        detail: "keyword",
        doc: "Trait (interface) declaration (`trait`, `auto trait`) or trait alias (`trait CopyClone = Clone + Copy;`).",
    },
    BuiltinEntry {
        name: "var",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: let)",
        doc: "Inferred local variable or destructuring binding (`var x = expr;`, `mut var x = expr;`, `var (a, b) = pair;`). Prefer explicit types when obvious.",
    },
    BuiltinEntry {
        name: "Self",
        kind: BuiltinKind::Keyword,
        detail: "keyword",
        doc: "Implementing type within a `struct`, `enum`, `union`, `trait`, or `impl` block.",
    },
    BuiltinEntry {
        name: "this",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: self)",
        doc: "Current instance receiver (`read this`, `write this`, `take this`) or current module path.",
    },
    BuiltinEntry {
        name: "public",
        kind: BuiltinKind::Keyword,
        detail: "visibility modifier (Rust: pub)",
        doc: "Public visibility (`public`, `public(in path)`, `public(this)`).",
    },
    BuiltinEntry {
        name: "internal",
        kind: BuiltinKind::Keyword,
        detail: "visibility modifier (Rust: pub(crate))",
        doc: "Crate-internal visibility (`pub(crate)`).",
    },
    BuiltinEntry {
        name: "private",
        kind: BuiltinKind::Keyword,
        detail: "visibility modifier",
        doc: "Private module visibility (default).",
    },
    BuiltinEntry {
        name: "mut",
        kind: BuiltinKind::Keyword,
        detail: "modifier",
        doc: "Marks a variable binding, parameter, or static as mutable (R# is immutable by default).",
    },
    BuiltinEntry {
        name: "const",
        kind: BuiltinKind::Keyword,
        detail: "modifier",
        doc: "Compile-time constant item, const function, const generic parameter, or const block.",
    },
    BuiltinEntry {
        name: "static",
        kind: BuiltinKind::Keyword,
        detail: "modifier",
        doc: "Static item, associated function without `this`, or `'static` lifetime.",
    },
    BuiltinEntry {
        name: "async",
        kind: BuiltinKind::Keyword,
        detail: "modifier",
        doc: "Asynchronous function, block, or closure (`async Task<T>`).",
    },
    BuiltinEntry {
        name: "unsafe",
        kind: BuiltinKind::Keyword,
        detail: "modifier",
        doc: "Unsafe function, trait, impl, extern block, attribute, or block.",
    },
    BuiltinEntry {
        name: "extern",
        kind: BuiltinKind::Keyword,
        detail: "modifier",
        doc: "Foreign function interface (FFI) ABI specifier or `extern crate`.",
    },
    BuiltinEntry {
        name: "super",
        kind: BuiltinKind::Keyword,
        detail: "path keyword",
        doc: "Parent module path segment.",
    },
    BuiltinEntry {
        name: "crate",
        kind: BuiltinKind::Keyword,
        detail: "path keyword",
        doc: "Current crate root path segment.",
    },
    BuiltinEntry {
        name: "if",
        kind: BuiltinKind::Keyword,
        detail: "control flow",
        doc: "Conditional statement or expression (`if (cond) { ... }` or `if (x is Some(int v))`).",
    },
    BuiltinEntry {
        name: "else",
        kind: BuiltinKind::Keyword,
        detail: "control flow",
        doc: "Alternative branch for `if` or diverging branch for `require`.",
    },
    BuiltinEntry {
        name: "while",
        kind: BuiltinKind::Keyword,
        detail: "control flow",
        doc: "Conditional loop (`while (cond) { ... }` or `while (x is Some(int v))`).",
    },
    BuiltinEntry {
        name: "for",
        kind: BuiltinKind::Keyword,
        detail: "control flow",
        doc: "C-style counted loop (`for (mut int i = 0; i < n; i++) { ... }`).",
    },
    BuiltinEntry {
        name: "foreach",
        kind: BuiltinKind::Keyword,
        detail: "control flow (Rust: for x in iter)",
        doc: "Iterator loop with explicit element typing (`foreach (read T item in items) { ... }`).",
    },
    BuiltinEntry {
        name: "in",
        kind: BuiltinKind::Keyword,
        detail: "keyword",
        doc: "Iterator separator in `foreach` or visibility scope in `public(in path)`.",
    },
    BuiltinEntry {
        name: "loop",
        kind: BuiltinKind::Keyword,
        detail: "control flow",
        doc: "Infinite loop exited via `break` (optionally with a value or label).",
    },
    BuiltinEntry {
        name: "match",
        kind: BuiltinKind::Keyword,
        detail: "control flow",
        doc: "Exhaustive pattern matching statement or expression.",
    },
    BuiltinEntry {
        name: "when",
        kind: BuiltinKind::Keyword,
        detail: "keyword (Rust: match guard `if`)",
        doc: "Match arm guard condition (`Pat when cond => ...`).",
    },
    BuiltinEntry {
        name: "is",
        kind: BuiltinKind::Keyword,
        detail: "pattern operator",
        doc: "Pattern match test (`expr is Some(int v)` or `expr is not None`).",
    },
    BuiltinEntry {
        name: "not",
        kind: BuiltinKind::Keyword,
        detail: "pattern operator",
        doc: "Negated pattern match (`expr is not None`).",
    },
    BuiltinEntry {
        name: "return",
        kind: BuiltinKind::Keyword,
        detail: "control flow",
        doc: "Explicit return statement (always required in R# functions and methods).",
    },
    BuiltinEntry {
        name: "break",
        kind: BuiltinKind::Keyword,
        detail: "control flow",
        doc: "Exits a loop, optionally with a label and/or expression value.",
    },
    BuiltinEntry {
        name: "continue",
        kind: BuiltinKind::Keyword,
        detail: "control flow",
        doc: "Skips to the next iteration of a loop, optionally targeting a label.",
    },
    BuiltinEntry {
        name: "await",
        kind: BuiltinKind::Keyword,
        detail: "operator (Rust: expr.await)",
        doc: "Awaits completion of an asynchronous future (`await expr`).",
    },
    BuiltinEntry {
        name: "with",
        kind: BuiltinKind::Keyword,
        detail: "operator (Rust: ..base)",
        doc: "Struct update expression (`alice with { Owner = \"Bob\".ToString() }`).",
    },
    BuiltinEntry {
        name: "new",
        kind: BuiltinKind::Keyword,
        detail: "operator",
        doc: "Struct or enum struct-variant literal (`new Point { X = 1, Y = 2 }`).",
    },
    BuiltinEntry {
        name: "as",
        kind: BuiltinKind::Keyword,
        detail: "operator",
        doc: "Primitive/pointer cast (`expr as T`), qualified path `(T as Trait).Item`, or `@` pattern binding.",
    },
    BuiltinEntry {
        name: "where",
        kind: BuiltinKind::Keyword,
        detail: "clause",
        doc: "Generic trait and lifetime constraints (`where T : Display, Clone`).",
    },
    BuiltinEntry {
        name: "namespace",
        kind: BuiltinKind::Keyword,
        detail: "declaration (Rust: mod)",
        doc: "Module declaration (`namespace App.Models;` or `namespace App { ... }`).",
    },
    BuiltinEntry {
        name: "using",
        kind: BuiltinKind::Keyword,
        detail: "directive (Rust: use)",
        doc: "Brings items or namespaces into scope (`using std.io.{this, Read};`).",
    },
    BuiltinEntry {
        name: "true",
        kind: BuiltinKind::Keyword,
        detail: "bool",
        doc: "Boolean literal `true`.",
    },
    BuiltinEntry {
        name: "false",
        kind: BuiltinKind::Keyword,
        detail: "bool",
        doc: "Boolean literal `false`.",
    },
    BuiltinEntry {
        name: "maybe",
        kind: BuiltinKind::Keyword,
        detail: "contextual keyword (Rust: ?Trait)",
        doc: "Relaxes an implicit trait bound (`where T : maybe Sized`).",
    },
    BuiltinEntry {
        name: "capture",
        kind: BuiltinKind::Keyword,
        detail: "contextual keyword (Rust: use<...>)",
        doc: "Precise capture bound on `impl Trait` (`impl Display + capture<life a, T>`).",
    },
    BuiltinEntry {
        name: "yield",
        kind: BuiltinKind::Keyword,
        detail: "contextual keyword (Rust: yield)",
        doc: "Yields a value from a coroutine/generator expression (`yield expr` or `yield`).",
    },
    // Primitive types (R# aliases + native Rust primitives)
    BuiltinEntry {
        name: "sbyte",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: i8)",
        doc: "8-bit signed integer (`i8`).",
    },
    BuiltinEntry {
        name: "byte",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: u8)",
        doc: "8-bit unsigned integer (`u8`).",
    },
    BuiltinEntry {
        name: "short",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: i16)",
        doc: "16-bit signed integer (`i16`).",
    },
    BuiltinEntry {
        name: "ushort",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: u16)",
        doc: "16-bit unsigned integer (`u16`).",
    },
    BuiltinEntry {
        name: "int",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: i32)",
        doc: "32-bit signed integer (`i32`).",
    },
    BuiltinEntry {
        name: "uint",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: u32)",
        doc: "32-bit unsigned integer (`u32`).",
    },
    BuiltinEntry {
        name: "long",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: i64)",
        doc: "64-bit signed integer (`i64`).",
    },
    BuiltinEntry {
        name: "ulong",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: u64)",
        doc: "64-bit unsigned integer (`u64`).",
    },
    BuiltinEntry {
        name: "int128",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: i128)",
        doc: "128-bit signed integer (`i128`).",
    },
    BuiltinEntry {
        name: "uint128",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: u128)",
        doc: "128-bit unsigned integer (`u128`).",
    },
    BuiltinEntry {
        name: "nint",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: isize)",
        doc: "Pointer-sized signed integer (`isize`).",
    },
    BuiltinEntry {
        name: "nuint",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: usize)",
        doc: "Pointer-sized unsigned integer (`usize`).",
    },
    BuiltinEntry {
        name: "float",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: f32)",
        doc: "32-bit IEEE 754 floating-point number (`f32`).",
    },
    BuiltinEntry {
        name: "double",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive (Rust: f64)",
        doc: "64-bit IEEE 754 floating-point number (`f64`).",
    },
    BuiltinEntry {
        name: "bool",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "Boolean type (`true` or `false`).",
    },
    BuiltinEntry {
        name: "char",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "Unicode scalar value (4 bytes).",
    },
    BuiltinEntry {
        name: "string",
        kind: BuiltinKind::PrimitiveType,
        detail: "owned string (Rust: String)",
        doc: "UTF-8 heap-allocated growable string (`std::string::String`).",
    },
    BuiltinEntry {
        name: "str",
        kind: BuiltinKind::PrimitiveType,
        detail: "string slice (Rust: str)",
        doc: "UTF-8 string slice (typically used as `read str` -> `&str`).",
    },
    BuiltinEntry {
        name: "void",
        kind: BuiltinKind::PrimitiveType,
        detail: "unit type (Rust: ())",
        doc: "Unit return type with no value (`()`).",
    },
    BuiltinEntry {
        name: "never",
        kind: BuiltinKind::PrimitiveType,
        detail: "never type (Rust: !)",
        doc: "Diverging type for expressions/functions that never return (`!`).",
    },
    BuiltinEntry {
        name: "dynamic",
        kind: BuiltinKind::PrimitiveType,
        detail: "type alias (Rust: Box<dyn Any>)",
        doc: "Runtime dynamically typed boxed value (`ToDynamic` / `AsDynamic<T>`).",
    },
    BuiltinEntry {
        name: "i8",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "8-bit signed integer (R# alias: `sbyte`).",
    },
    BuiltinEntry {
        name: "u8",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "8-bit unsigned integer (R# alias: `byte`).",
    },
    BuiltinEntry {
        name: "i16",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "16-bit signed integer (R# alias: `short`).",
    },
    BuiltinEntry {
        name: "u16",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "16-bit unsigned integer (R# alias: `ushort`).",
    },
    BuiltinEntry {
        name: "i32",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "32-bit signed integer (R# alias: `int`).",
    },
    BuiltinEntry {
        name: "u32",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "32-bit unsigned integer (R# alias: `uint`).",
    },
    BuiltinEntry {
        name: "i64",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "64-bit signed integer (R# alias: `long`).",
    },
    BuiltinEntry {
        name: "u64",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "64-bit unsigned integer (R# alias: `ulong`).",
    },
    BuiltinEntry {
        name: "i128",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "128-bit signed integer (R# alias: `int128`).",
    },
    BuiltinEntry {
        name: "u128",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "128-bit unsigned integer (R# alias: `uint128`).",
    },
    BuiltinEntry {
        name: "isize",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "Pointer-sized signed integer (R# alias: `nint`).",
    },
    BuiltinEntry {
        name: "usize",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "Pointer-sized unsigned integer (R# alias: `nuint`).",
    },
    BuiltinEntry {
        name: "f32",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "32-bit floating-point number (R# alias: `float`).",
    },
    BuiltinEntry {
        name: "f64",
        kind: BuiltinKind::PrimitiveType,
        detail: "primitive",
        doc: "64-bit floating-point number (R# alias: `double`).",
    },
    // Prelude & stdlib types
    BuiltinEntry {
        name: "List",
        kind: BuiltinKind::StdType,
        detail: "pub type List<T> = Vec<T>",
        doc: "Contiguous growable array type (prelude alias for `Vec<T>`).",
    },
    BuiltinEntry {
        name: "Queue",
        kind: BuiltinKind::StdType,
        detail: "pub type Queue<T> = VecDeque<T>",
        doc: "Double-ended queue (prelude alias for `VecDeque<T>`).",
    },
    BuiltinEntry {
        name: "Dictionary",
        kind: BuiltinKind::StdType,
        detail: "pub type Dictionary<K, V> = HashMap<K, V>",
        doc: "Hash map collection (prelude alias for `HashMap<K, V>`).",
    },
    BuiltinEntry {
        name: "SortedDictionary",
        kind: BuiltinKind::StdType,
        detail: "pub type SortedDictionary<K, V> = BTreeMap<K, V>",
        doc: "Ordered map collection (prelude alias for `BTreeMap<K, V>`).",
    },
    BuiltinEntry {
        name: "SortedSet",
        kind: BuiltinKind::StdType,
        detail: "pub type SortedSet<T> = BTreeSet<T>",
        doc: "Ordered set collection (prelude alias for `BTreeSet<T>`).",
    },
    BuiltinEntry {
        name: "PriorityQueue",
        kind: BuiltinKind::StdType,
        detail: "pub type PriorityQueue<T> = BinaryHeap<T>",
        doc: "Binary max-heap priority queue (prelude alias for `BinaryHeap<T>`).",
    },
    BuiltinEntry {
        name: "HashSet",
        kind: BuiltinKind::StdType,
        detail: "std::collections::HashSet<T>",
        doc: "Hash set collection re-exported in the R# prelude.",
    },
    BuiltinEntry {
        name: "LinkedList",
        kind: BuiltinKind::StdType,
        detail: "std::collections::LinkedList<T>",
        doc: "Doubly-linked list re-exported in the R# prelude.",
    },
    BuiltinEntry {
        name: "Option",
        kind: BuiltinKind::StdType,
        detail: "enum Option<T>",
        doc: "Optional value (`Some(T)` or `None`). Shorthand syntax: `T?`.",
    },
    BuiltinEntry {
        name: "Result",
        kind: BuiltinKind::StdType,
        detail: "enum Result<T, E>",
        doc: "Fallible result (`Ok(T)` or `Err(E)`). Propagate with `try expr`.",
    },
    BuiltinEntry {
        name: "Task",
        kind: BuiltinKind::StdType,
        detail: "async Task<T>",
        doc: "C#-style async function return type wrapper (desugars to Rust `async fn -> T`).",
    },
    BuiltinEntry {
        name: "Box",
        kind: BuiltinKind::StdType,
        detail: "std::boxed::Box<T>",
        doc: "Heap-allocated smart pointer.",
    },
    BuiltinEntry {
        name: "Rc",
        kind: BuiltinKind::StdType,
        detail: "std::rc::Rc<T>",
        doc: "Single-threaded reference-counted pointer.",
    },
    BuiltinEntry {
        name: "Arc",
        kind: BuiltinKind::StdType,
        detail: "std::sync::Arc<T>",
        doc: "Thread-safe atomically reference-counted pointer.",
    },
    BuiltinEntry {
        name: "Vec",
        kind: BuiltinKind::StdType,
        detail: "std::vec::Vec<T>",
        doc: "Native Rust growable vector (R# alias: `List<T>`).",
    },
    BuiltinEntry {
        name: "HashMap",
        kind: BuiltinKind::StdType,
        detail: "std::collections::HashMap<K, V>",
        doc: "Native Rust hash map (R# alias: `Dictionary<K, V>`).",
    },
    BuiltinEntry {
        name: "String",
        kind: BuiltinKind::StdType,
        detail: "std::string::String",
        doc: "Native Rust owned string (R# alias: `string`).",
    },
    BuiltinEntry {
        name: "Some",
        kind: BuiltinKind::StdType,
        detail: "Option::Some(T)",
        doc: "Present value variant of `Option<T>`.",
    },
    BuiltinEntry {
        name: "None",
        kind: BuiltinKind::StdType,
        detail: "Option::None",
        doc: "Absent value variant of `Option<T>`.",
    },
    BuiltinEntry {
        name: "Ok",
        kind: BuiltinKind::StdType,
        detail: "Result::Ok(T)",
        doc: "Success variant of `Result<T, E>`.",
    },
    BuiltinEntry {
        name: "Err",
        kind: BuiltinKind::StdType,
        detail: "Result::Err(E)",
        doc: "Error variant of `Result<T, E>`.",
    },
    // Prelude functions
    BuiltinEntry {
        name: "PrintLine",
        kind: BuiltinKind::Function,
        detail: "void PrintLine<T : Display>(T value)",
        doc: "Prints a displayable value followed by a newline.",
    },
    BuiltinEntry {
        name: "Print",
        kind: BuiltinKind::Function,
        detail: "void Print<T : Display>(T value)",
        doc: "Prints a displayable value without a trailing newline.",
    },
    BuiltinEntry {
        name: "ToDynamic",
        kind: BuiltinKind::Function,
        detail: "dynamic ToDynamic<T : Any + static>(T value)",
        doc: "Boxes any `'static` value into an R# `dynamic` (`Box<dyn Any>`).",
    },
    BuiltinEntry {
        name: "AsDynamic",
        kind: BuiltinKind::Function,
        detail: "read T? AsDynamic<T : Any + static>(read dynamic value)",
        doc: "Attempts to downcast and borrow a `dynamic` value as `Option<&T>`.",
    },
];

pub const BUILTIN_MEMBERS: &[MemberBuiltin] = &[
    // List / Vec
    MemberBuiltin {
        owner: "List",
        name: "Add",
        detail: "void Add(write this, T item)",
        doc: "Appends an item to the end of the list (`Vec::push`).",
    },
    MemberBuiltin {
        owner: "List",
        name: "Count",
        detail: "nuint Count(read this)",
        doc: "Returns the number of elements in the list (`Vec::len`).",
    },
    MemberBuiltin {
        owner: "List",
        name: "IsEmpty",
        detail: "bool IsEmpty(read this)",
        doc: "Returns `true` if the list contains no elements.",
    },
    MemberBuiltin {
        owner: "List",
        name: "push",
        detail: "void push(write this, T item)",
        doc: "Appends an element to the back of the collection.",
    },
    MemberBuiltin {
        owner: "List",
        name: "pop",
        detail: "T? pop(write this)",
        doc: "Removes and returns the last element, or `None` if empty.",
    },
    MemberBuiltin {
        owner: "List",
        name: "len",
        detail: "nuint len(read this)",
        doc: "Returns the number of elements in the vector.",
    },
    // Queue / VecDeque
    MemberBuiltin {
        owner: "Queue",
        name: "Enqueue",
        detail: "void Enqueue(write this, T item)",
        doc: "Pushes an item to the back of the queue.",
    },
    MemberBuiltin {
        owner: "Queue",
        name: "Dequeue",
        detail: "T? Dequeue(write this)",
        doc: "Removes and returns the front item of the queue.",
    },
    MemberBuiltin {
        owner: "Queue",
        name: "Peek",
        detail: "read T? Peek(read this)",
        doc: "Returns a shared reference to the front item without removing it.",
    },
    MemberBuiltin {
        owner: "Queue",
        name: "Count",
        detail: "nuint Count(read this)",
        doc: "Returns the number of elements in the queue.",
    },
    // Dictionary / HashMap
    MemberBuiltin {
        owner: "Dictionary",
        name: "Add",
        detail: "V? Add(write this, K key, V value)",
        doc: "Inserts a key-value pair, returning any previous value.",
    },
    MemberBuiltin {
        owner: "Dictionary",
        name: "ContainsKey",
        detail: "bool ContainsKey(read this, read K key)",
        doc: "Returns `true` if the dictionary contains the specified key.",
    },
    MemberBuiltin {
        owner: "Dictionary",
        name: "Count",
        detail: "nuint Count(read this)",
        doc: "Returns the number of entries in the dictionary.",
    },
    // string / String / str
    MemberBuiltin {
        owner: "string",
        name: "Length",
        detail: "nuint Length(read this)",
        doc: "Returns the byte length of the string.",
    },
    MemberBuiltin {
        owner: "string",
        name: "ToString",
        detail: "string ToString(read this)",
        doc: "Clones or converts to an owned `string` (`String`).",
    },
    MemberBuiltin {
        owner: "string",
        name: "len",
        detail: "nuint len(read this)",
        doc: "Returns the byte length of the string.",
    },
    MemberBuiltin {
        owner: "string",
        name: "is_empty",
        detail: "bool is_empty(read this)",
        doc: "Returns `true` if the string has a length of zero.",
    },
    MemberBuiltin {
        owner: "string",
        name: "parse",
        detail: "Result<T, E> parse<T>(read this)",
        doc: "Parses this string slice into another type implementing `FromStr`.",
    },
    // Option & Result
    MemberBuiltin {
        owner: "Option",
        name: "unwrap",
        detail: "T unwrap(take this)",
        doc: "Returns the contained `Some` value, panicking if `None`.",
    },
    MemberBuiltin {
        owner: "Option",
        name: "unwrap_or",
        detail: "T unwrap_or(take this, T default)",
        doc: "Returns the contained `Some` value or a provided default.",
    },
    MemberBuiltin {
        owner: "Option",
        name: "is_some",
        detail: "bool is_some(read this)",
        doc: "Returns `true` if the option is a `Some` value.",
    },
    MemberBuiltin {
        owner: "Option",
        name: "is_none",
        detail: "bool is_none(read this)",
        doc: "Returns `true` if the option is a `None` value.",
    },
    MemberBuiltin {
        owner: "Result",
        name: "unwrap",
        detail: "T unwrap(take this)",
        doc: "Returns the contained `Ok` value, panicking on `Err`.",
    },
    MemberBuiltin {
        owner: "Result",
        name: "is_ok",
        detail: "bool is_ok(read this)",
        doc: "Returns `true` if the result is `Ok`.",
    },
    MemberBuiltin {
        owner: "Result",
        name: "is_err",
        detail: "bool is_err(read this)",
        doc: "Returns `true` if the result is `Err`.",
    },
];

pub fn find_builtin(name: &str) -> Option<&'static BuiltinEntry> {
    BUILTINS.iter().find(|entry| entry.name == name)
}

pub fn normalize_builtin_owner(ty_name: &str) -> &str {
    match ty_name {
        "Vec" => "List",
        "VecDeque" => "Queue",
        "HashMap" => "Dictionary",
        "String" | "str" => "string",
        other => other,
    }
}

pub fn builtin_members_for(ty_name: &str) -> Vec<&'static MemberBuiltin> {
    let owner = normalize_builtin_owner(ty_name);
    BUILTIN_MEMBERS
        .iter()
        .filter(|member| member.owner == owner)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_builtins_and_members() {
        assert_eq!(find_builtin("read").unwrap().kind, BuiltinKind::Keyword);
        assert_eq!(
            find_builtin("int").unwrap().kind,
            BuiltinKind::PrimitiveType
        );
        assert_eq!(find_builtin("List").unwrap().kind, BuiltinKind::StdType);
        assert_eq!(
            find_builtin("PrintLine").unwrap().kind,
            BuiltinKind::Function
        );
        assert!(find_builtin("NotABuiltinXYZ").is_none());

        assert!(builtin_members_for("Vec").iter().any(|m| m.name == "Add"));
        assert!(
            builtin_members_for("VecDeque")
                .iter()
                .any(|m| m.name == "Enqueue")
        );
        assert!(
            builtin_members_for("HashMap")
                .iter()
                .any(|m| m.name == "ContainsKey")
        );
        assert!(
            builtin_members_for("str")
                .iter()
                .any(|m| m.name == "ToString")
        );
        assert!(builtin_members_for("UnknownType").is_empty());
    }
}
