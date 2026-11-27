use std::path::Path;

use rsharp_core::{
    FromRustOptions, PreludeMode, TranspileOptions, TypeHintMap, convert_from_rust_with_hints,
    count_raw_rust_blocks, discover_cargo_rs_files, parse_cargo_metadata_rs_files, parser,
    run_rsharp, transpile_to_rust,
};

fn convert_no_ra(rust_src: &str) -> (String, String) {
    let rsh = convert_from_rust_with_hints(
        rust_src,
        FromRustOptions {
            use_rustc_inference: false,
        },
        &TypeHintMap::default(),
        false,
    )
    .expect("from_rust conversion should succeed");
    let rust_out = transpile_to_rust(
        &rsh,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .unwrap_or_else(|e| panic!("round-trip R# parse/codegen failed: {e}\n--- R# ---\n{rsh}"));
    (rsh, rust_out)
}

#[test]
fn zero_fallback_exhaustive_syn_and_var_matrix() {
    // 1. Standard Rust 2024 constructs with unnameable types, closures, value `if`/blocks,
    //    early returns, loop jumps, packed field move blocks, and explicit-when-obvious types:
    //    must produce ZERO `rust { ... }` blocks and execute identically!
    let runnable_rust = r#"
#[repr(packed)]
struct Packed(u128);

struct Point { x: i32, y: i32 }
struct Wrap(i32);

mod private_api {
    struct Secret(i32);
    impl Secret {
        pub fn val(&self) -> i32 { self.0 }
    }
    pub fn make_secret(v: i32) -> impl Fn() -> i32 {
        let s = Secret(v);
        move || s.val()
    }
}

fn early_return_in_value_if(c: bool) -> i32 {
    let v = if c {
        let tmp = 99i32;
        #[allow(unused)]
        return tmp;
    } else {
        let base = 10i32;
        base + 1
    };
    v + 100
}

fn early_return_unit_in_value_if(c: bool) {
    let _ = if c {
        return
    } else {
        let x = 1i32;
        let _ = x;
    };
}

fn loop_with_value_if_and_jumps(mut n: i32) -> i32 {
    let mut acc = 0i32;
    loop {
        let step = if n <= 0 {
            break acc
        } else if n == 2 {
            n -= 1;
            continue;
        } else {
            let cur = n;
            n -= 1;
            cur
        };
        acc += step;
    }
}

fn main() {
    let explicit_int = 42i32;
    let explicit_infer: _ = 7i32;
    let clo = |x| x + 1i32;
    let secret_fn = private_api::make_secret(5);
    let (a, mut b) = (unknown_pair_a(), unknown_pair_b());
    b += a;
    let all @ (p1, p2): (i32, i32) = (3, 4);
    let _ = all;
    let p = Packed(123u128);
    let aligned_ref: &u128 = &{ p.0 };
    let r1 = early_return_in_value_if(true);
    let r2 = early_return_in_value_if(false);
    early_return_unit_in_value_if(true);
    let r3 = loop_with_value_if_and_jumps(4);
    let matched_val = match p1 {
        3 => {
            let k = 10i32;
            k + p2
        }
        _ => 0,
    };
    let sum = explicit_int + explicit_infer + clo(2) + secret_fn() + b + (*aligned_ref as i32) + r1 + r2 + r3 + matched_val;
    println!("{}", sum);
}

fn unknown_pair_a() -> i32 { 1 }
fn unknown_pair_b() -> i32 { 2 }
"#;

    let (rsh, _rust_out) = convert_no_ra(runnable_rust);
    assert_eq!(
        count_raw_rust_blocks(&rsh),
        0,
        "expected ZERO rust{{}} fallbacks, got:\n{rsh}"
    );
    assert!(rsh.contains("int explicit_int = 42i32;"), "got:\n{rsh}");
    assert!(rsh.contains("int explicit_infer = 7i32;"), "got:\n{rsh}");
    assert!(rsh.contains("var clo = (x) =>"), "got:\n{rsh}");
    assert!(rsh.contains("var secret_fn ="), "got:\n{rsh}");
    assert!(rsh.contains("read { return p.0; }"), "got:\n{rsh}");
    assert!(rsh.contains("return (return tmp);"), "got:\n{rsh}");

    let run = run_rsharp(&rsh).expect("converted R# program should compile and run cleanly");
    assert_eq!(run.exit_code, 0, "stderr: {}", run.stderr);
    // 42 + 7 + 3 + 5 + 3 + 123 + 99 + 111 + (4+3+1=8) + 14 = 415
    assert_eq!(run.stdout.trim(), "415");

    // 2. All remaining `from_rust` zero-fallback syntax & nightly parity cases:
    //    - `auto trait`, `unsafe auto trait`, `trait Alias = ...`, `default impl`,
    //      `default unsafe impl`, `impl !Send`, Typst-style abstract `#[elem] type Elem;` in `impl`,
    //      `extern "C" { pub type Opaque; }`, `try { ... }`, `yield expr`, `yield`,
    //      `Expr::Let` in boolean expr, `let pat_mac!() = v;`, `let ref r = u();`,
    //      `let ref mut rm = u();`, `let (p) = u();`, match-arm macro tail & early return tail,
    //      and inferred closure params across every pattern kind (with outer attrs).
    let syntax_parity_rust = r#"
auto trait MyAuto {}
unsafe auto trait MyUnsafeAuto {}
#[allow(dead_code)]
pub trait CopyClone<T> = Clone + Copy where T: Send;
pub trait SingleBound = Clone;
struct Target;
trait Spec {}
default impl Spec for Target {}
default unsafe impl Send for Target {}
impl !Sync for Target {}
impl Target {
    #[cfg(all())]
    type AbstractElem<T> where T: Copy;
}
extern "C" {
    #[allow(dead_code)]
    pub type OpaqueC;
}
#[allow(dead_code)]
type bool;
#[allow(dead_code)]
pub type TopOpaque;

fn parity_constructs(opt: Option<i32>, flag: bool) {
    let all_pat @ (a, b) = unknown();
    let pat_mac!() = unknown();
    let ref r_var = unknown();
    let ref mut rm_var = unknown();
    let (paren_var) = unknown();
    let (u1, u2);
    u1 = 1;
    u2 = 2;

    let _ = [1].iter().map(|#[allow(unused)] x| x);
    let _ = [1].iter().map(|_| 1);
    let _ = [(1, 2)].iter().map(|(x, y)| x + y);
    let _ = [Point { x: 1, y: 2 }].iter().map(|Point { x, .. }| x);
    let _ = [Wrap(1)].iter().map(|Wrap(v)| v);
    let _ = [[1]].iter().map(|[x]| x);
    let _ = [&1].iter().map(|&x| x);
    let _ = [1].iter().map(|1..=2| 0);

    let m_mac = match opt {
        Some(v) => {
            let t = v;
            my_mac!(t)
        }
        None => {
            let z = 0i32;
            let _ = z;
            return
        }
    };
    let m_brace_mac = match opt {
        Some(v) => {
            let t = v;
            my_mac! { t }
        }
        None => 0i32,
    };
    let m_postfix_blk = match opt {
        Some(v) => {
            let t = v + 1;
            Some(t)
        }
        .map(Ok::<i32, ()>),
        None => None,
    };
    let m_semi = match opt {
        Some(v) => {
            let t = v;
            my_mac!(t);
        }
        None => {
            let z = 0i32;
            let _ = z;
        }
    };
    let t_blk = try {
        let x = 1i32;
        x
    };
    let y1 = yield 42;
    let y2 = yield;
    let b_let = !(let Some(_) = opt);
    let _ = (all_pat, a, b, r_var, rm_var, paren_var, u1, u2, m_mac, m_brace_mac, m_postfix_blk, m_semi, t_blk, y1, y2, b_let, flag);
}
"#;
    let (rsh2, rust2) = convert_no_ra(syntax_parity_rust);
    assert_eq!(
        count_raw_rust_blocks(&rsh2),
        0,
        "expected ZERO rust{{}} blocks in full syntax parity test, got:\n{rsh2}"
    );
    assert!(rust2.contains("auto trait MyAuto"), "got:\n{rust2}");
    assert!(
        rust2.contains("unsafe auto trait MyUnsafeAuto"),
        "got:\n{rust2}"
    );
    assert!(
        rust2.contains("pub trait CopyClone<T> = Clone + Copy"),
        "got:\n{rust2}"
    );
    assert!(
        rust2.contains("default impl Spec for Target"),
        "got:\n{rust2}"
    );
    assert!(
        rust2.contains("default unsafe impl Send for Target"),
        "got:\n{rust2}"
    );
    assert!(rust2.contains("impl !Sync for Target"), "got:\n{rust2}");
    assert!(rust2.contains("type AbstractElem<T>"), "got:\n{rust2}");
    assert!(rust2.contains("pub type OpaqueC;"), "got:\n{rust2}");
    assert!(rust2.contains("type bool;"), "got:\n{rust2}");
    assert!(rust2.contains("pub type TopOpaque;"), "got:\n{rust2}");
    assert!(rust2.contains(".map(Ok::<i32, ()>)"), "got:\n{rust2}");
    assert!(rust2.contains("try {"), "got:\n{rust2}");
    assert!(rust2.contains("yield 42"), "got:\n{rust2}");
    assert!(rust2.contains("yield ()"), "got:\n{rust2}");
    assert!(rust2.contains("let ref r_var ="), "got:\n{rust2}");
    assert!(rust2.contains("let ref mut rm_var ="), "got:\n{rust2}");
    assert!(rust2.contains("let (paren_var) ="), "got:\n{rust2}");
    assert!(rust2.contains("let (u1, u2);"), "got:\n{rust2}");
    assert!(rust2.contains("let all_pat @ (a, b) ="), "got:\n{rust2}");

    // Phase 4: generic/where `type Foo<T>;` and `become` now convert natively with 0 `rust {}`!
    let (rsh_native, rust_native) = convert_no_ra(
        "type VerbatimGeneric<T>;\n\
         type VerbatimWhere where i32: Copy;\n\
         fn tail_call(x: i32) -> i32 { become tail_call(x) }",
    );
    assert_eq!(count_raw_rust_blocks(&rsh_native), 0, "got:\n{rsh_native}");
    assert!(rust_native.contains("type VerbatimGeneric<T>;"));
    assert!(rust_native.contains("type VerbatimWhere where i32: Copy;"));
    assert!(rust_native.contains("become tail_call(x)"));

    // Residual exotic `syn` Verbatim items (`macro decl_mac() {}` and `builtin # ...`) still fall back safely.
    let (rsh_verb, _) = convert_no_ra(
        "macro decl_mac() {}\nfn builtin_expr() { let _ = builtin # offset_of(S, f); }",
    );
    assert_eq!(count_raw_rust_blocks(&rsh_verb), 2, "got:\n{rsh_verb}");

    // Phase 4: `default` specialization items in `impl` (fn, type, const) + `default` as type name + `gen` as type name.
    let (rsh_spec, rust_spec) = convert_no_ra(
        "trait Spec { type Out; const N: i32; fn run(&self) -> i32; }\n\
         impl Spec for () {\n\
             #[allow(unused)]\n\
             default type Out = i32;\n\
             #[allow(unused)]\n\
             default const N: i32 = 1;\n\
             #[allow(unused)]\n\
             pub default fn run(&self) -> i32 { 1 }\n\
             default fn helper() -> i32 { 2 }\n\
         }\n\
         type default = i32;\n\
         struct Boxed;\n\
         impl Boxed {\n\
             fn returns_default() -> default { 0 }\n\
         }\n",
    );
    assert_eq!(count_raw_rust_blocks(&rsh_spec), 0, "got:\n{rsh_spec}");
    assert!(rust_spec.contains("default type Out = i32;"));
    assert!(rust_spec.contains("default const N: i32 = 1;"));
    assert!(rust_spec.contains("default fn run(&self) -> i32"));
    assert!(rust_spec.contains("default fn helper() -> i32"));
    // Direct R# contextual `gen` as type name (non-modifier fallback in `parse_fn_decl`):
    assert!(parser::parse_file("alias gen = int; gen returns_gen() { return 0; }").is_ok());

    // Phase 4: `ra_ap_syntax` lossless nightly fallback (0 `rust {}` for `gen fn`, `async gen fn`,
    // `gen {}`, `gen move {}`, `async gen {}`, `async gen move {}`, `const trait`, `impl const`).
    let (rsh_nightly, rust_nightly) = convert_no_ra(
        "pub const trait ConstTr { fn eval(&self) -> i32; }\n\
         impl const ConstTr for i32 { fn eval(&self) -> i32 { *self } }\n\
         pub gen fn count_up(n: i32) -> i32 {\n\
             let g1 = gen { yield 1; };\n\
             let g2 = gen move { yield n; };\n\
             let g3 = async gen { yield 2; };\n\
             let g4 = async gen move { yield n; };\n\
             let _ = (g1, g2, g3, g4);\n\
             yield n;\n\
         }\n\
         pub async gen fn async_stream(x: i32) -> i32 {\n\
             yield x;\n\
         }\n",
    );
    assert_eq!(
        count_raw_rust_blocks(&rsh_nightly),
        0,
        "nightly constructs must convert with 0 rust{{}}:\n{rsh_nightly}"
    );
    assert!(
        rust_nightly.contains("pub const trait ConstTr"),
        "{rust_nightly}"
    );
    assert!(
        rust_nightly.contains("impl const ConstTr for i32"),
        "{rust_nightly}"
    );
    assert!(
        rust_nightly.contains("pub gen fn count_up"),
        "{rust_nightly}"
    );
    assert!(
        rust_nightly.contains("pub async gen fn async_stream"),
        "{rust_nightly}"
    );
    assert!(rust_nightly.contains("gen  {"), "{rust_nightly}");
    assert!(rust_nightly.contains("gen move  {"), "{rust_nightly}");
    assert!(rust_nightly.contains("async gen  {"), "{rust_nightly}");
    assert!(rust_nightly.contains("async gen move  {"), "{rust_nightly}");

    // Invalid Rust syntax must STILL return `Err` even with `ra_ap_syntax` fallback enabled.
    assert!(
        convert_from_rust_with_hints(
            "fn broken( {",
            FromRustOptions {
                use_rustc_inference: false,
            },
            &TypeHintMap::default(),
            false,
        )
        .is_err()
    );

    // Per-item lossless fallback in `from_rust_nightly`: valid nightly item that `syn` still rejects
    // (`const trait` + `impl<const N: usize> const ...` or `fn f() { #[allow(unused)] yield; }` or `extern { ... }`
    // — specifically Rust 2024 `safe` / nightly `use` / `trait` with `#[attr]` or `builtin#`):
    // `syn` rejects `async trait AsyncTr {}` while `ra_ap_syntax` parses it with 0 errors!
    let (rsh_future, _) = convert_no_ra(
        "fn normal_ok() -> i32 { 1 }\n\
         async trait FutureNightlyTrait {}\n",
    );
    assert!(rsh_future.contains("int normal_ok()"), "{rsh_future}");
    assert_eq!(count_raw_rust_blocks(&rsh_future), 1, "{rsh_future}");

    // Strict mode error propagation inside per-item nightly fallback:
    assert!(
        convert_from_rust_with_hints(
            "fn strict_fail() { let x = unknown_call(); }\nasync trait FutureNightlyTrait {}\n",
            FromRustOptions {
                use_rustc_inference: false,
            },
            &TypeHintMap::default(),
            true,
        )
        .is_err()
    );

    // Contextual `gen` as identifier + negative parser paths for Phase 4 constructs:
    assert!(parser::parse_file("void f(int gen) { int x = gen; }").is_ok());
    for bad in [
        "type Bad<;",
        "type Bad where 123;",
        "const trait ;",
        "impl S { default a b; }",
        "void f() { become 1 + ; }",
        "void f() { gen take 1; }",
        "void f() { gen { + }; }",
        "void f() { async gen { + }; }",
    ] {
        assert!(
            parser::parse_file(bad).is_err(),
            "expected parse error for `{bad}`"
        );
    }
}

#[test]
fn adjacent_statement_boundary_ambiguity_and_parser_coverage() {
    // 1. Nested `auto trait` / `unsafe auto trait` / `default impl` / `default unsafe impl`,
    //    value `if` with `is` / `is not` / `let`-chain / `else if` / no-else,
    //    `#[cfg]` lifted returns in value blocks, postfix `.` after `{ ... }` and `if { ... }`,
    //    match guards with `(deref a) == (deref b) =>`, lambda inside guard call & parens & block,
    //    `var` with explicit type, `borrow read _ x`, and next-line boundary disambiguation.
    let rsh = r#"
trait Dummy {}
struct DummyS;
alias auto = int;
alias default = int;

auto top_auto() { return 1; }
unsafe auto top_unsafe_auto() { return 1; }
default top_default() { return 1; }

void test_matrix(Option<int> opt, bool c1, bool c2, read int a, read int b, int n) {
    auto trait NestedAuto {}
    unsafe auto trait NestedUnsafeAuto {}
    default impl Dummy for DummyS {}
    default unsafe impl Send for DummyS {}
    unsafe auto nested_unsafe_auto() { return 1; }
    mut int auto = 0;
    auto = 1;
    mut int default = 0;
    default = 2;

    int brk_val = outer: loop {
        break outer inner: { break inner 1; };
    };

    { return 1; }.abs();
    if (c1) { return 1; } else { return 2; }.abs();

    var v_is = if (opt is Some(int x)) { return x; } else { return 0; };
    var v_not = if (opt is not None) { return 1; } else { return 0; };
    var v_chain = if (opt is Some(int x) && x > 0) { return x; } else { return 0; };
    var v_elif = if (c1) { return 1; } else if (c2) { return 2; } else { return 3; };
    var v_no_else = if (c1) { _ = n; };

    var cfg_val = {
        int base = 1;
        [Cfg(all())]
        return base;
        [Cfg(not(all()))]
        return 2;
    };

    var (ta, tb) : (int, int) = (1, 2);
    var borrow read _ br_infer = a;

    // Statement boundaries: block-ending stmt followed on next line by `[Attr]` or `(a, b) =`
    if (c1) { _ = 1; }
    [Allow(unused)]
    int after_if = 1;

    { _ = 1; }
    [Allow(unused)]
    int after_blk = 2;

    try { return Ok(1); }
    [Allow(unused)]
    int after_try = 3;

    match (n) {
        x when (deref a) == (deref b) => x,
        x when items.any((it) => it > 0) => x,
        x when ((y) => y > 0)(x) => x,
        x when { var f = (z) => z > 0; return f(x); } => x,
        _ => 0,
    }
    (int pa, int pb) = (1, 2);

    _ = (v_is, v_not, v_chain, v_elif, v_no_else, cfg_val, ta, tb, br_infer, after_if, after_blk, after_try, pa, pb, auto, default, brk_val);
}
"#;
    let rust = transpile_to_rust(
        rsh,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("ambiguity matrix R# should transpile cleanly");
    assert!(rust.contains("auto trait NestedAuto"), "got:\n{rust}");
    assert!(
        rust.contains("unsafe auto trait NestedUnsafeAuto"),
        "got:\n{rust}"
    );
    assert!(
        rust.contains("default impl Dummy for DummyS"),
        "got:\n{rust}"
    );
    assert!(
        rust.contains("default unsafe impl Send for DummyS"),
        "got:\n{rust}"
    );
    assert!(rust.contains("(if c1 {"), "got:\n{rust}");
    assert!(
        rust.contains("let (ta, tb): (i32, i32) = (1, 2);"),
        "got:\n{rust}"
    );
    assert!(!rust.contains("let _: &&_ = &br_infer"), "got:\n{rust}");
    assert!(
        rust.contains("#[cfg(all())]\n        { base }"),
        "got:\n{rust}"
    );

    assert!(parser::parse_file("trait TrailingPlus = Clone + ;").is_ok());
    assert!(parser::parse_file("void f() { var x = yield; }").is_ok());
    assert!(parser::parse_file("void f() { var x = try 1; }").is_ok());

    // Parser error paths for new constructs
    for invalid in [
        "extern \"C\" { type 123; }",
        "extern \"C\" { type Opaque }",
        "trait BadAlias = 123;",
        "trait BadAlias = Clone",
        "trait BadAlias = Clone + 123;",
        "trait BadAlias = Clone where 123;",
        "void f() { var (1 +); }",
        "void f() { var (a,) : ; }",
        "void f() { var (a,) = ; }",
        "void f() { var (a,) 1; }",
        "void f() { var x = if (c) { 1 }; }",
        "void f() { var x = if (c) { return 1; } else if (c2) { 1 }; }",
        "void f() { var x = if (c) { return 1; } else { 1 }; }",
        "void f() { var x = try { 1 }; }",
        "void f() { var x = yield 1 +; }",
        "auto trait 123 {}",
        "unsafe auto trait 123 {}",
        "default impl 123 {}",
        "default impl Foo { 123 }",
        "default unsafe impl 123 {}",
        "default unsafe impl Foo { 123 }",
        "default unsafe bad_item;",
        "void f() { default unsafe x; }",
        "void f() { var borrow read (int, int) 123 = x; }",
        "void f() { match (x) { _ => { return 1; }. ; } }",
    ] {
        assert!(
            parser::parse_file(invalid).is_err(),
            "expected parse error for: {invalid}"
        );
    }
}

#[test]
fn cargo_metadata_discovery_and_raw_rust_counter_full_coverage() {
    // 1. `count_raw_rust_blocks` ignores comments/strings and returns 0 on lex error.
    assert_eq!(count_raw_rust_blocks("void main() { int x = 1; }"), 0);
    assert_eq!(
        count_raw_rust_blocks("// rust { ignored }\nvoid main() { read str s = \"rust { x }\"; }"),
        0
    );
    assert_eq!(
        count_raw_rust_blocks("rust { type A; }\nvoid f() { rust { let x = 1; } }"),
        2
    );
    assert_eq!(count_raw_rust_blocks("void f() { \"unterminated"), 0);

    // 2. `discover_cargo_rs_files` + `parse_cargo_metadata_rs_files` full branch coverage.
    let base = std::env::temp_dir().join(format!("rsharp_cargo_disc_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let proj = base.join("demo_ws");
    std::fs::create_dir_all(proj.join("src").join("sub")).unwrap();
    std::fs::create_dir_all(proj.join("src").join(".hidden")).unwrap();
    std::fs::create_dir_all(proj.join("src").join("target")).unwrap();
    std::fs::create_dir_all(proj.join("src").join("node_modules")).unwrap();
    std::fs::create_dir_all(proj.join("templates")).unwrap();
    std::fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"demo_ws\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(proj.join("src").join("lib.rs"), "pub mod sub;\n").unwrap();
    std::fs::write(
        proj.join("src").join("sub").join("mod.rs"),
        "pub fn ok() {}\n",
    )
    .unwrap();
    std::fs::write(proj.join("src").join("README.md"), "not rust\n").unwrap();
    std::fs::write(proj.join("src").join(".hidden").join("h.rs"), "invalid").unwrap();
    std::fs::write(proj.join("src").join("target").join("t.rs"), "invalid").unwrap();
    std::fs::write(
        proj.join("src").join("node_modules").join("n.rs"),
        "invalid",
    )
    .unwrap();
    std::fs::write(proj.join("build.rs"), "fn main() {}\n").unwrap();
    // Template outside Cargo targets must NOT be discovered!
    std::fs::write(proj.join("templates").join("tpl.rs"), "fn {{name}}() {}\n").unwrap();

    // Direct Cargo root discovery includes src/lib.rs, src/sub/mod.rs, build.rs and excludes templates/tpl.rs
    let files = discover_cargo_rs_files(&proj).expect("should discover Cargo targets");
    assert_eq!(files.len(), 3, "got: {files:?}");
    assert!(files.iter().any(|p| p.ends_with("lib.rs")));
    assert!(files.iter().any(|p| p.ends_with("mod.rs")));
    assert!(files.iter().any(|p| p.ends_with("build.rs")));
    assert!(!files.iter().any(|p| p.ends_with("tpl.rs")));

    // Subdirectory inside a Cargo crate resolves workspace root via `find_cargo_workspace_root`
    let sub_files = discover_cargo_rs_files(&proj.join("src"));
    assert!(sub_files.is_none()); // package manifest is parent of `src`, outside `proj/src`

    // Non-Cargo directory returns None
    let non_cargo = base.join("plain_dir");
    std::fs::create_dir_all(&non_cargo).unwrap();
    assert!(discover_cargo_rs_files(&non_cargo).is_none());

    // Broken Cargo.toml causes `cargo metadata` failure -> None
    let broken_cargo = base.join("broken_cargo");
    std::fs::create_dir_all(&broken_cargo).unwrap();
    std::fs::write(broken_cargo.join("Cargo.toml"), "not valid toml [[[").unwrap();
    assert!(discover_cargo_rs_files(&broken_cargo).is_none());
    assert!(
        rsharp_core::ra_oracle::discover_cargo_rs_files_with_bin(
            &proj,
            "__nonexistent_rsharp_cargo__"
        )
        .is_none()
    );

    // Direct `parse_cargo_metadata_rs_files` edge cases:
    assert!(parse_cargo_metadata_rs_files("not json", &proj).is_none());
    assert!(parse_cargo_metadata_rs_files("{}", &proj).is_none());
    assert!(parse_cargo_metadata_rs_files(r#"{"packages": "bad"}"#, &proj).is_none());

    let lib_str = proj
        .join("src")
        .join("lib.rs")
        .to_string_lossy()
        .replace('\\', "\\\\");
    let manifest_str = proj
        .join("Cargo.toml")
        .to_string_lossy()
        .replace('\\', "\\\\");
    let outside_manifest = base
        .join("outside")
        .join("Cargo.toml")
        .to_string_lossy()
        .replace('\\', "\\\\");
    let missing_rs = proj
        .join("src")
        .join("missing.rs")
        .to_string_lossy()
        .replace('\\', "\\\\");
    let dir_path = proj.join("src").to_string_lossy().replace('\\', "\\\\");
    let synthetic = format!(
        r#"{{
            "packages": [
                {{}},
                {{"manifest_path": ""}},
                {{"manifest_path": "{outside_manifest}", "targets": []}},
                {{"manifest_path": "{manifest_str}"}},
                {{
                    "manifest_path": "{manifest_str}",
                    "targets": [
                        {{}},
                        {{"src_path": "{missing_rs}"}},
                        {{"src_path": "{dir_path}"}},
                        {{"src_path": "{lib_str}"}}
                    ]
                }}
            ]
        }}"#
    );
    let parsed =
        parse_cargo_metadata_rs_files(&synthetic, &proj).expect("valid synthetic metadata");
    assert_eq!(parsed.len(), 2, "got: {parsed:?}");

    // Empty targets -> None
    let empty_targets =
        format!(r#"{{"packages": [{{"manifest_path": "{manifest_str}", "targets": []}}]}}"#);
    assert!(parse_cargo_metadata_rs_files(&empty_targets, &proj).is_none());

    // Nonexistent project dir exercises canonicalize fallback
    assert!(
        parse_cargo_metadata_rs_files(&empty_targets, Path::new("Z:\\nonexistent_rsharp_xyz"))
            .is_none()
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn github_40_repos_all_15_root_causes_zero_fallback_and_stress() {
    // 1. Global aliased `using` (hyperswitch, tokio, iced, wezterm x2) + root glob/group
    let use_rust = r#"
use ::image as img;
use ::window::glium::backend::Context as GliumContext;
use ::std::fmt::Display;
use ::foo::*;
use ::{alpha, beta};
use ::std::{fmt, io};
"#;
    let (use_rsh, use_rs) = convert_no_ra(use_rust);
    assert_eq!(count_raw_rust_blocks(&use_rsh), 0);
    assert!(
        use_rsh.contains("using img = global::image;"),
        "got:\n{use_rsh}"
    );
    assert!(
        use_rsh.contains("using GliumContext = global::window.glium.backend.Context;"),
        "got:\n{use_rsh}"
    );
    assert!(use_rs.contains("use ::image as img;"), "got:\n{use_rs}");
    assert!(
        use_rs.contains("use ::window::glium::backend::Context as GliumContext;"),
        "got:\n{use_rs}"
    );

    // 2. All remaining 14 root causes from the 40-repo GitHub benchmark (ZERO rust {} fallbacks)
    let compat_rust = r##"
mod err {
    pub struct Error;
}
struct InstallOptions { a: i32 }
struct Signals { a: i32, b: i32 }
struct State<P> { value: P }
enum RuleState { A(i32), B(i32) }
enum Target { Rust }
struct Wrapper(i32);

struct Runner;
impl Runner {
    #[ref_cast_custom]
    pub(super) const fn new_unchecked(p: &str) -> &Self;
    fn collect_run<const B: bool>() {}
    fn parent<T>() -> i32 { 1 }
    fn handle(&self, e: &self::err::Error, w: &mut self::err::Error) {
        let _ = (e, w);
    }
}

#[linkme::distributed_slice]
pub static TESTS: [i32];

fn local_ref_cast_and_empty_semi() {
    #[ref_cast_custom]
    fn ref_cast<T>(p: &str) -> &T;
    fn inner() {};
    let _ = ref_cast::<i32>;
}

fn receiver_this_path(e: &self::err::Error, w: &mut self::err::Error) {
    let _ = (e, w);
}

#[case(br#"$"('" "')""#)]
#[case::postgres_password("pw", r#""postgres://user:pass@localhost/db""#)]
#[case(r"plain_raw")]
#[case(b"byte_str")]
#[case(c"c_str")]
#[case(cr#"c_raw"#)]
#[case(b'x')]
#[doc("bar")]
fn all_40_repo_patterns(
    state: RuleState,
    target: Target,
    opts: &InstallOptions,
    c1: bool,
    c2: bool,
    c3: bool,
) {
    // Non-ident patterns in `let` (biome, SpacetimeDB, mise, surrealdb)
    let (RuleState::A(d1) | RuleState::B(d1)) = state;
    let Target::Rust = target;
    let 1 = unknown_v();
    let 1..=2 = unknown_v();
    let const { 1 } = unknown_v();
    let ((pa, pb)) = unknown_v();
    let (bound_a @ _) = unknown_v();

    // Uninitialized destructuring with and without explicit types (hyperswitch, egui)
    let [left, right, top, bottom]: [bool; 4];
    let (mut ta, mut tb, tc): (_, _, i32);
    let [u_slice];
    let InstallOptions { a: u_field };
    let Wrapper(u_wrap);
    let &u_ref;
    left = true; right = false; top = true; bottom = false;
    ta = 1; tb = 2; tc = 3;
    u_slice = 0; u_field = 0; u_wrap = 0; u_ref = 0;

    // `deref Struct { ... } = opts;` (mise, rustfs, qdrant, jj)
    let &InstallOptions { a: opt_a } = opts;

    // Chained `@` subpatterns (hyperswitch x4)
    let created_at @ modified_at @ last_synced = unknown_v();

    // `_` element types in destructured closure params (zed, hyperswitch, qdrant)
    let _ = |(window, workspace): (_, i32)| { let _ = (window, workspace); };
    let _ = |(mut window, workspace): (_, i32)| { window = workspace; };

    // `[stmt] : Statement[1] = ...` & `[hunk] : _[1] = ...` (polars, zed)
    let [stmt]: [i32; 1] = [1];
    let [hunk]: [_; 1] = [2];
    let arr_lit = [foo(1)];

    // Generic struct pattern, ternary generic fn, empty `<>()` turbofish (dioxus, iced, polars, biome)
    let State::<i32> { value: sv } = unknown_v();
    let _fn_ptr = if c1 { Runner::collect_run::<true> } else { Runner::collect_run::<false> };
    let _p = Runner::parent::<>();
    let _m = unknown_v().parent::<>();

    // `break format!(...)` vs `break outer !(cond)` (rustfs)
    let _b1 = loop {
        break format!("ok");
    };
    let _b2 = 'outer: loop {
        break 'outer !(c1);
    };

    // Dangling-else inside `if` expression (surrealdb/each.rs)
    let _e1 = if c1 {
        if c2 {
            1
        }
    } else {
        2
    };
    let _e2 = if c1 {
        if c2 {
            1
        } else if c3 {
            2
        }
    } else {
        3
    };
    let _e3 = if c1 {
        if c2 { 1 } else { 2 }
    } else {
        3
    };

    // `&..` / `&mut ..` (SpacetimeDB)
    let _r1 = &..;
    let _r2 = &mut ..;

    // Stale `var_types` when `var` shadows a previously typed struct variable (surrealdb/morsel/set.rs)
    let signals = Signals { a: 1, b: 2 };
    let (_answer, signals) = unknown_pair();
    let _s2 = Signals { a: 3, ..signals };

    // Additional chain & receiver paren coverage (`x?.field`, `f?()`, `let _;`)
    let _;
    let _field_after_try = get_opt()?.a;
    let _call_after_try = get_cb()?();

    let _ = (
        d1, pa, pb, bound_a, left, right, top, bottom, ta, tb, tc,
        u_slice, u_field, u_wrap, u_ref, opt_a, created_at, modified_at, last_synced,
        stmt, hunk, arr_lit, sv, _fn_ptr, _p, _m, _b1, _b2, _e1, _e2, _e3, _r1, _r2, _s2,
        _field_after_try, _call_after_try,
    );
}

async fn await_then_method<F: std::future::Future<Output = Runner>>(f: F) {
    let _ = f.await.parent::<()>();
}
"##;
    let (compat_rsh, compat_rs) = convert_no_ra(compat_rust);
    assert_eq!(
        count_raw_rust_blocks(&compat_rsh),
        0,
        "expected ZERO rust{{}} blocks, got:\n{compat_rsh}"
    );
    assert!(
        compat_rsh.contains("var [left, right, top, bottom] : bool[4];"),
        "got:\n{compat_rsh}"
    );
    assert!(
        compat_rsh.contains("var deref InstallOptions { a: opt_a } = opts;"),
        "got:\n{compat_rsh}"
    );
    assert!(
        compat_rsh.contains("new Signals { a = 3, ..signals }"),
        "got:\n{compat_rsh}"
    );
    assert!(
        compat_rs.contains("let created_at @ modified_at @ last_synced ="),
        "got:\n{compat_rs}"
    );
    assert!(
        compat_rs.contains("e: &self::err::Error"),
        "got:\n{compat_rs}"
    );

    // 3. Direct R# parser & codegen checks: `_ ident`, `mut _ ident`, `read ..`, `this.Mod.T`,
    //    `(a, b) : (_, int)` param, empty `<>` reject when not followed by `(`, and unary/binary precedence.
    let direct_rsh = r#"
void direct_checks(
    read this.err.Error e,
    write this.err.Error w,
    this.Mod.T plain
) {
    var f1 = ((_ window, int n)) => n;
    var f2 = ((mut _ window, int n)) => { window = n; return window; };
    var f3 = ((a, b) : (_, int)) => b;
    var A(or_x) | B(or_x) = state;
    var r = read ..;
    var x = foo<>();
    var cmp = 1 < 2 && 3 <= 4 || 5 > 4 && 6 >= 6;
    var bits = (1 | 2 ^ 3 & 4) == 5 && 1 != 2;
    var math = ((1 << 2) >> 1) + 3 - 4 * 5 / 2 % 3;
    var is_p = opt is Some(int v) && opt is not None;
    var un = -!~1;
    var f4 = take () => 1;
    var p_try = try panic("boom");
    (int x, int y) pt = (1, 2);
    var px = pt.x.abs();
    var ti = (deref pair_ref).0;
    _ = (e, w, plain, f1, f2, f3, f4, or_x, r, x, cmp, bits, math, is_p, un, p_try, px, ti);
}
"#;
    let direct_rs = transpile_to_rust(
        direct_rsh,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("direct R# constructs should transpile cleanly");
    assert!(
        !direct_rs.contains("let _: &_ = &window;"),
        "got:\n{direct_rs}"
    );
    assert!(direct_rs.contains("&(..)"), "got:\n{direct_rs}");
    assert!(
        direct_rs.contains("e: &self::err::Error"),
        "got:\n{direct_rs}"
    );

    // `a <> b` is NOT empty turbofish (not followed by `(`); invalid `take this.X` is not a receiver;
    // error paths on `(..` and `().` postfix.
    for invalid in [
        "void f() { var x = a <> b; }",
        "void f(take this.X x) {}",
        "void f(take Box<Self> this.X x) {}",
        "void f(Box<Self> this.X x) {}",
        "void f() { var x = (..1 +); }",
        "void f() { var x = ().; }",
    ] {
        assert!(
            parser::parse_file(invalid).is_err(),
            "should fail: {invalid}"
        );
    }

    // Exercise `rustc` probe with both explicit and inferred `Pat::Type`, `Pat::Paren`, uninitialized `let`, and `column!()` via lib crate.
    let probed_infer = rsharp_core::convert_from_rust(
        "fn main() { let first: u8; first = 1; let explicit: u8 = first; let (n): _ = std::hint::black_box(explicit); let a = std::hint::black_box([0u8; column!() as usize]); let _ = (n, a); }",
        FromRustOptions::default(),
    )
    .expect("rustc probe on parenthesized inferred local should succeed");
    assert!(probed_infer.contains("(byte n)"), "got:\n{probed_infer}");

    // 4. Deeply nested chained `try (...).set_default(i)` stress test (150 levels — 2x atuin's 79!)
    //    Runs directly on the default debug stack (proving O(1) stack across from_rust, parser, and codegen!).
    let make_deep = |n: usize| {
        let mut s =
            String::from("fn deep_builder(mut b: Builder) -> Result<Builder, ()> {\n    Ok(b");
        for i in 0..n {
            s.push_str(&format!(".set_default(\"k{i}\", {i})?"));
        }
        s.push_str(")\n}\n");
        s
    };
    let (deep150_rsh, deep150_rs) = convert_no_ra(&make_deep(150));
    assert_eq!(count_raw_rust_blocks(&deep150_rsh), 0);
    assert!(deep150_rs.contains(".set_default(\"k149\", 149))?"));
}

#[test]
fn github_60_repos_all_8_root_causes_zero_fallback_and_discovery() {
    // 1-8. All 8 real-Rust root causes from the 60-repo GitHub benchmark with 0 `rust {}` fallbacks.
    let rust_all_8 = r#"
#![feature(box_patterns, trait_alias)]

pub trait MaybeSerde =;
pub trait MaybeWhere<T> = where T: Clone;

#[cfg(feature = "ssz")]
use {
    crate::chain_spec::{BYTES_PER_LOGS_BLOOM, MAX_EXTRA_DATA_BYTES},
    ::ssz::types::{List, Vector},
};

pub fn brace_macro_followed_by_call_stmt(waited: i32) {
    tokio::select! {
        _ = async {} => {}
    }
    drop(waited);
    tokio::select! {
        _ = async {} => {}
    }
    drop(std::hint::black_box(waited));
}

pub fn float_dot_ranges(x: f64, y: f32) -> i32 {
    let a = !(0. ..=1.).contains(&x);
    let b = (0. ..1.).contains(&x);
    let c = (0. ..).contains(&y);
    let d = match x {
        0. ..=1. => 1,
        _ => 0,
    };
    if a && b && c { d } else { 0 }
}

pub fn ternary_block_branch_guard(cond: bool, fresh_connection: i32) -> i32 {
    let v1 = if cond {
        fresh_connection
    } else {
        loop {
            break fresh_connection;
        }
    };
    let v2 = if cond {
        loop {
            break 1;
        }
    } else {
        2
    };
    v1 + v2
}

pub enum Inner {
    Disabled { code: i32 },
    Pair(i32, i32),
}

pub enum Wrap {
    Api(Box<Inner>, i32),
    None,
}

pub fn nightly_box_patterns(w: Wrap) -> i32 {
    match w {
        Wrap::Api(box Inner::Disabled { code, .. }, _) => code,
        Wrap::Api(box Inner::Pair(a, b), _) => a + b,
        Wrap::None => 0,
    }
}

pub fn inferred_optional_closure_param<T, R>(f: impl Fn((T, Option<i32>)) -> R) {
    let _ = f;
}

pub fn call_inferred_optional_closure() {
    inferred_optional_closure_param(|(lower, upper): (i32, Option<_>)| -> Option<_> {
        upper.map(|u| lower + u)
    });
}

pub(super) fn simd_cpu() -> Option<cfg_select! { all() => { () } }> {
    None
}

pub fn single_tail_block_receiver(tmp_dir: i32, pair: (i32, i32)) -> i32 {
    let f: Box<dyn FnOnce()> = Box::new({ move || { drop(tmp_dir); } }.into());
    f();
    let x = { pair.0 }.abs();
    let y = { 1 + 2 }.abs();
    x + y
}

pub struct SearchRank {
    pub score: i32,
    pub exact: bool,
}

pub fn for_loop_scope_no_leak<J: IntoIterator<Item = (i32, Vec<String>, SearchRank)>>(
    candidates: Vec<(i32, SearchRank)>,
    phrase_jobs: J,
) -> Vec<SearchRank> {
    let mut out = Vec::new();
    for (mut _candidate, priority) in candidates {
        let _ = priority;
    }
    for (_id, _paths, priority) in phrase_jobs {
        out.push(SearchRank { score: 1, ..priority });
    }
    let _u = for _x in 0..1 {};
    out
}
"#;
    let (rsh, rs) = convert_no_ra(rust_all_8);
    assert_eq!(count_raw_rust_blocks(&rsh), 0, "got:\n{rsh}");
    assert!(rsh.contains("0. ..=1."), "got:\n{rsh}");
    assert!(rs.contains("0. ..=1."), "got:\n{rs}");
    assert!(rsh.contains("0. ..1."), "got:\n{rsh}");
    assert!(rs.contains("0. ..1."), "got:\n{rs}");
    assert!(rsh.contains("box Inner.Disabled"), "got:\n{rsh}");
    assert!(rs.contains("box Inner::Disabled"), "got:\n{rs}");
    assert!(
        rsh.contains("new SearchRank { score = 1, ..priority }"),
        "for-loop binding leaked into struct update:\n{rsh}"
    );
    assert!(
        rs.contains("pub trait MaybeSerde = ;") || rs.contains("pub trait MaybeSerde =;"),
        "got:\n{rs}"
    );

    // Direct R# checks for parser/codegen branches of all new grammar features:
    // - ternary `cond ? ident : loop { ... }` and nested labeled loop inside block
    // - `box` patterns in `let` destructuring (typed + mut), `match`, and contextual `box` ident
    // - `_?`, `_??`, `_[4]` in lambda tuple params
    // - brace macro return types with `?`, `??`, `[N]`, generic `<T>()`, plus non-return-type
    //   brace macros followed by `[Attr]`, `[Attr(a, b)]`, `Vec<read int>`, `Vec<T> f()`
    let direct_rsh = r#"
my_item_macro! { foo }

[Derive(Debug)]
struct AfterAttr {}

my_item_macro! { bar }

[Allow(dead_code, unused)]
int after_multi_arg_attr() { return 0; }

my_item_macro! { baz }

Vec<read int> after_ref_generic_fn() { return Vec.new(); }

my_item_macro! { qux }

Vec<T> after_type_param_fn<T>() { return Vec.new(); }

cfg_select! { a => b }? macro_opt_ret() { return None; }
cfg_select! { a => b }?? macro_res_ret() { return Ok(()); }
cfg_select! { a => b }[4] macro_arr_ret() { return [0; 4]; }
cfg_select! { a => b } macro_gen_ret<T>() {}
cfg_select! { a => b } macro_where_ret<T>() where T : Clone {}

trait MacroRetTrait {
    cfg_select! { a => b } macro_trait_ret();
}

void direct_60_checks(bool cond, int fresh_connection) {
    var t1 = cond ? fresh_connection : loop { break fresh_connection; };
    var t2 = cond ? { a: loop { break a 1; } } : 2;
    box int bx = boxed_a;
    (box mut bm, bn) = boxed_pair;
    var bm_match = match (x) {
        Some(box) => box,
        None => 0,
    };
    var lam = ((Input lower, _? upper, _?? res, _[4] arr)) => (lower, upper, res, arr);
    _ = (t1, t2, bx, bm, bn, bm_match, lam);
}
"#;
    let direct_rs = transpile_to_rust(
        direct_rsh,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("direct R# 60-repo constructs should transpile cleanly");
    assert!(
        direct_rs.contains("let box bx = boxed_a;"),
        "got:\n{direct_rs}"
    );
    assert!(
        direct_rs.contains("let (box mut bm, bn) = boxed_pair;"),
        "got:\n{direct_rs}"
    );
    assert!(direct_rs.contains("Option<_>"), "got:\n{direct_rs}");
    assert!(direct_rs.contains("Option<Option<_>>"), "got:\n{direct_rs}");
    assert!(direct_rs.contains("[_; 4]"), "got:\n{direct_rs}");

    // Exercise parser error/backtrack branches for `box`, ternary, and `[` after item macro.
    for invalid in [
        "void f() { match (x) { box (1 +) => 0 } }",
        "void f() { var x = c ? (1 +) : 2; }",
        "my_item_macro! { u } [1 +]",
        "my_item_macro! { m } [foo : bar]",
    ] {
        assert!(
            parser::parse_file(invalid).is_err(),
            "should fail: {invalid}"
        );
    }

    // Exercise `for_loop_scope_no_leak` with non-empty `TypeHintMap` as well.
    let mut hints = rsharp_core::ra_oracle::TypeHintMap::default();
    hints.insert(1, 1, "i32".to_string());
    let hinted_rsh = rsharp_core::convert_from_rust_with_hints(
        "fn f(xs: Vec<i32>) { for x in xs { let _ = x; } }",
        FromRustOptions {
            use_rustc_inference: false,
        },
        &hints,
        false,
    )
    .expect("hinted for-loop should convert cleanly");
    assert!(
        hinted_rsh.contains("foreach (x in xs)"),
        "got:\n{hinted_rsh}"
    );

    // Non-module discovery exclusion: `include!("...")` fragments, nested `Cargo.toml`/`book.toml`,
    // and nested `tests/.../src/` fixture directories.
    let base = std::env::temp_dir().join(format!("rsharp_disc60_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let proj = base.join("ws");
    std::fs::create_dir_all(proj.join("src").join("nested_pkg")).unwrap();
    std::fs::create_dir_all(proj.join("src").join("nested_book")).unwrap();
    std::fs::create_dir_all(
        proj.join("tests")
            .join("testsuite")
            .join("includes")
            .join("all_includes")
            .join("src"),
    )
    .unwrap();
    std::fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"ws\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("lib.rs"),
        "pub const PALETTE: [u8; 3] = include!(\"palette_literals.rs\");\n\
         pub const TEXT: &str = include!(\"notes.txt\");\n\
         pub const DYN: () = include!(concat!(\"a\", \".rs\"));\n\
         pub mod ok_mod;\n",
    )
    .unwrap();
    std::fs::write(proj.join("src").join("ok_mod.rs"), "pub fn ok() {}\n").unwrap();
    std::fs::write(proj.join("src").join("palette_literals.rs"), "[1, 2, 3]\n").unwrap();
    std::fs::write(
        proj.join("src").join("nested_pkg").join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("nested_pkg").join("fixture.rs"),
        "not valid rust {{",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("nested_book").join("book.toml"),
        "[book]\ntitle = \"fixture\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("nested_book").join("book_ex.rs"),
        "not valid rust {{",
    )
    .unwrap();
    std::fs::write(
        proj.join("tests").join("testsuite").join("main.rs"),
        "fn main() {}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("tests")
            .join("testsuite")
            .join("includes")
            .join("all_includes")
            .join("src")
            .join("example.rs"),
        "fn ANCHOR: broken {{\n",
    )
    .unwrap();

    let discovered = discover_cargo_rs_files(&proj).expect("should discover valid crate files");
    assert_eq!(discovered.len(), 3, "got: {discovered:?}");
    assert!(discovered.iter().any(|p| p.ends_with("lib.rs")));
    assert!(discovered.iter().any(|p| p.ends_with("ok_mod.rs")));
    assert!(discovered.iter().any(|p| p.ends_with("main.rs")));
    assert!(
        !discovered
            .iter()
            .any(|p| p.ends_with("palette_literals.rs"))
    );
    assert!(!discovered.iter().any(|p| p.ends_with("fixture.rs")));
    assert!(!discovered.iter().any(|p| p.ends_with("book_ex.rs")));
    assert!(!discovered.iter().any(|p| p.ends_with("example.rs")));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn github_60_repos_wave2_all_6_root_causes_hang_and_discovery() {
    // 1. 60-deep nested generic type (wgpu send_sync_recursion.rs O(2^N) hang regression)
    //    Runs on a 16 MB stack matching CLI/parallel workers (must finish in linear O(N) time, 0 rust{}).
    let mut deep_box = String::from("T");
    for _ in 0..60 {
        deep_box = format!("Box<{deep_box}>");
    }
    let deep_tower = format!("struct Tower<T: ?Sized>({deep_box});\n");
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let (tower_rsh, tower_rs) = convert_no_ra(&deep_tower);
            assert_eq!(count_raw_rust_blocks(&tower_rsh), 0);
            assert!(tower_rs.contains("struct Tower<T: ?Sized>(Box<Box<Box<"));
        })
        .unwrap()
        .join()
        .unwrap();

    // 2-6. Remaining 5 Wave-2 real-Rust root causes + 25-deep nested generics directly on default stack.
    let mut nested25 = String::from("T");
    for _ in 0..25 {
        nested25 = format!("Box<{nested25}>");
    }
    let rust_wave2 = format!(
        r#"
struct Tower25<T: ?Sized>({nested25});

#[windows_interface::interface("372E1D3B-38D3-42E4-A15B-8AB2B178F513")]
pub unsafe trait IApplicationView: IUnknown {{
    pub unsafe fn get_iids(&self, count: *mut u32) -> i32;
    pub unsafe fn set_focus(&self) -> i32;
}}

#[enum_trait_object(
    #[expect(clippy::enum_variant_names)]
    #[derive(Clone, Debug)]
    pub enum Object<'gc> {{
        A(A<'gc>),
        B(B<'gc>),
    }}
)]
pub trait TObject<'gc> {{
    fn id(&self) -> i32;
}}

#[my_lifetime_attr(Pair<'a, 'b>)]
#[my_char_attr('a', '\n')]
pub fn double_ref_and_const_array_and_closure_postfix(
    layers: &[&'static str],
    mut_refs: &[&mut i32],
    items: Vec<Option<i32>>,
) -> Vec<*const u8> {{
    const USIZE_SIZE: usize = 8;
    let mut data = [0i32; {{ 2 * USIZE_SIZE }}];
    let _mat: [[u8; {{ 2 * USIZE_SIZE }}]; {{ 1 + 1 }}] = [[0; {{ 2 * USIZE_SIZE }}]; {{ 1 + 1 }}];
    let _arr_lit = [data.len()];
    data[0] = 1;
    let _kept: Vec<_> = items
        .into_iter()
        .filter(move |v| {{
            let x = v;
            x.map(|d| d > 0)
        }}.unwrap_or(false))
        .collect();
    let _mut_sum: i32 = mut_refs.iter().map(|&s: &&mut i32| *s).sum();
    layers.iter().map(|&s: &&'static _| {{ s.as_ptr() }}).collect()
}}
"#
    );
    let (rsh, rs) = convert_no_ra(&rust_wave2);
    assert_eq!(count_raw_rust_blocks(&rsh), 0, "got:\n{rsh}");
    assert!(
        rs.contains("pub unsafe fn get_iids(&self, count: *mut u32) -> i32;"),
        "got:\n{rs}"
    );
    assert!(rs.contains("enum_trait_object("), "got:\n{rs}");
    assert!(rs.contains("my_lifetime_attr("), "got:\n{rs}");
    assert!(rs.contains("&&'static _"), "got:\n{rs}");
    assert!(rs.contains("&&mut i32"), "got:\n{rs}");
    assert!(rs.contains(".unwrap_or(false)"), "got:\n{rs}");

    // Exercise TraitItem::Verbatim non-fn fallback (`pub const` inside trait) so `from_rust` keeps 100% coverage.
    let trait_const_rsh = rsharp_core::convert_from_rust_with_hints(
        "trait BadConst { pub const X: i32; }",
        FromRustOptions {
            use_rustc_inference: false,
        },
        &rsharp_core::ra_oracle::TypeHintMap::default(),
        false,
    )
    .expect("non-fn TraitItem::Verbatim should emit rust{} fallback");
    assert!(trait_const_rsh.contains("rust {"));

    // Direct R# checks: `deref read<static> _ s`, `deref read ptr<int> p`, `deref read int v`,
    // and single-pass assoc type / assoc bound generics with GAT args + parenthesized trait args.
    let direct_rsh = r#"
void direct_wave2<I>(I it) where I : Iterator<Item = int> + Lending<Item<static> : Copy> {
    var f1 = (deref read<static> _ s) => s;
    var f2 = (deref read ptr<int> p) => p;
    var f3 = (deref read int v) => v;
    _ = (it, f1, f2, f3);
}
"#;
    let direct_rs = transpile_to_rust(
        direct_rsh,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("direct Wave-2 R# constructs should transpile cleanly");
    assert!(
        direct_rs.contains("let _: &&'static _ = &s;"),
        "got:\n{direct_rs}"
    );
    assert!(
        direct_rs.contains("let _: &*const i32 = &p;"),
        "got:\n{direct_rs}"
    );
    assert!(direct_rs.contains("let _: &i32 = &v;"), "got:\n{direct_rs}");
    // Cover error path when a lambda block has a broken postfix continuation.
    assert!(parser::parse_file("void f() { var c = (v) => { return 1; }. ; }").is_err());

    // Discovery hygiene with ZERO false positives on real `tests/*/in/` or `tests/*/out/` modules:
    // - `tests/testsuite/in/mod.rs` and `tests/testsuite/out.rs` + `tests/testsuite/out/helper.rs` MUST be discovered!
    // - `tests/testsuite/cargo_add/script_bare/in/cargo-test-fixture.rs` (no `in/mod.rs`, no `in.rs`) MUST be skipped!
    // - `src/language_type.tera.rs` (template) MUST be skipped!
    // - `src/proxy_link.rs` (1-line Windows text symlink stub pointing to `ok_mod.rs`) MUST be skipped!
    // - `src/minicore.rs` (`include_str!("minicore.rs")`) and `src/process_table.rs` (`include!(concat!(...))`) MUST be skipped!
    let base = std::env::temp_dir().join(format!("rsharp_disc60w2_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let proj = base.join("ws");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::create_dir_all(proj.join("tests").join("testsuite").join("in")).unwrap();
    std::fs::create_dir_all(proj.join("tests").join("testsuite").join("out")).unwrap();
    std::fs::create_dir_all(
        proj.join("tests")
            .join("testsuite")
            .join("cargo_add")
            .join("script_bare")
            .join("in"),
    )
    .unwrap();
    std::fs::create_dir_all(
        proj.join("tests")
            .join("testsuite")
            .join("script")
            .join("rustc_fixtures"),
    )
    .unwrap();
    std::fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"ws\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("lib.rs"),
        "pub mod ok_mod;\n\
         pub mod empty_mod;\n\
         pub const CORE: &str = include_str!(\"minicore.rs\");\n\
         pub fn tbl() { include!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/src/process_table.rs\")); }\n\
         pub fn out_lib() { include!(concat!(env!(\"OUT_DIR\"), \"/lib.rs\")); }\n\
         pub fn out_win() { include!(concat!(env!(\"OUT_DIR\"), \"\\\\sub\\\\gen_win.rs\")); }\n\
         pub fn missing_inc() { include!(\"missing_nonexistent.rs\"); }\n\
         // edge cases for balanced paren / unclosed quote scanner:\n\
         // include!(\"unterminated\n\
         // include!(concat!(\"x.rs\"\n",
    )
    .unwrap();
    // >512 bytes so `is_text_symlink_stub` exercises its `meta.len() <= 512` false branch too.
    std::fs::write(
        proj.join("src").join("ok_mod.rs"),
        format!("// {}\n\npub fn ok() {{}}\n", "x".repeat(600)),
    )
    .unwrap();
    // Comment-only valid `.rs` file exercises the fallthrough `false` return in `is_expr_fragment_file`.
    std::fs::write(proj.join("src").join("empty_mod.rs"), "// empty module\n").unwrap();
    std::fs::write(
        proj.join("src").join("minicore.rs"),
        "//- minicore: broken {{\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("process_table.rs"),
        "pub fn gen_included_via_concat() {}\n",
    )
    .unwrap();
    // Orphan match-expression fragment (like ZLUDA's `process_table_windows.rs`).
    std::fs::write(
        proj.join("src").join("orphan_match_fragment.rs"),
        "// generated fragment\n\nmatch (name, flag) { _ => 0 }\n",
    )
    .unwrap();
    // Orphan array-literal fragment.
    std::fs::write(
        proj.join("src").join("orphan_array_fragment.rs"),
        "[1, 2, 3]\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("language_type.tera.rs"),
        "{% for k in langs %}{{k}}{% endfor %}\n",
    )
    .unwrap();
    // 1-line Windows git symlink text stub pointing to an existing sibling `.rs` file:
    std::fs::write(proj.join("src").join("proxy_link.rs"), "ok_mod.rs").unwrap();

    // Real test modules under `tests/testsuite/in/` (via `mod.rs`) and `tests/testsuite/out/` (via `out.rs`):
    std::fs::write(
        proj.join("tests").join("testsuite").join("main.rs"),
        "mod r#in;\nmod out;\nmod cargo_add;\nfn main() {}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("tests")
            .join("testsuite")
            .join("in")
            .join("mod.rs"),
        "pub fn real_in_test() {}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("tests").join("testsuite").join("out.rs"),
        "pub mod helper;\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("tests")
            .join("testsuite")
            .join("out")
            .join("helper.rs"),
        "pub fn real_out_helper() {}\n",
    )
    .unwrap();
    // Cargo-style fixture dirs under `tests/testsuite/cargo_add/`: `cargo_add/mod.rs` IS a real module,
    // `script_bare/mod.rs` IS a real module, while `script_bare/in/` has neither `mod.rs` nor `in.rs`!
    std::fs::write(
        proj.join("tests")
            .join("testsuite")
            .join("cargo_add")
            .join("mod.rs"),
        "mod script_bare;\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("tests")
            .join("testsuite")
            .join("cargo_add")
            .join("script_bare")
            .join("mod.rs"),
        "pub fn real_test() {}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("tests")
            .join("testsuite")
            .join("cargo_add")
            .join("script_bare")
            .join("in")
            .join("cargo-test-fixture.rs"),
        "---\n[package]\n---\nfn main() {}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("tests")
            .join("testsuite")
            .join("script")
            .join("rustc_fixtures")
            .join("multifrontmatter.rs"),
        "---\n---\n---\n",
    )
    .unwrap();

    let discovered = discover_cargo_rs_files(&proj).expect("should discover valid crate files");
    assert_eq!(discovered.len(), 9, "got: {discovered:?}");
    assert!(discovered.iter().any(|p| p.ends_with("lib.rs")));
    assert!(discovered.iter().any(|p| p.ends_with("ok_mod.rs")));
    assert!(discovered.iter().any(|p| p.ends_with("empty_mod.rs")));
    assert!(discovered.iter().any(|p| p.ends_with("main.rs")));
    assert!(
        discovered
            .iter()
            .any(|p| p.ends_with("in\\mod.rs") || p.ends_with("in/mod.rs"))
    );
    assert!(discovered.iter().any(|p| p.ends_with("out.rs")));
    assert!(discovered.iter().any(|p| p.ends_with("helper.rs")));
    assert!(
        discovered
            .iter()
            .any(|p| p.ends_with("cargo_add\\mod.rs") || p.ends_with("cargo_add/mod.rs"))
    );
    assert!(
        discovered
            .iter()
            .any(|p| p.ends_with("script_bare\\mod.rs") || p.ends_with("script_bare/mod.rs"))
    );
    assert!(!discovered.iter().any(|p| p.ends_with("minicore.rs")));
    assert!(!discovered.iter().any(|p| p.ends_with("process_table.rs")));
    assert!(
        !discovered
            .iter()
            .any(|p| p.ends_with("orphan_match_fragment.rs"))
    );
    assert!(
        !discovered
            .iter()
            .any(|p| p.ends_with("orphan_array_fragment.rs"))
    );
    assert!(
        !discovered
            .iter()
            .any(|p| p.ends_with("language_type.tera.rs"))
    );
    assert!(!discovered.iter().any(|p| p.ends_with("proxy_link.rs")));
    assert!(
        !discovered
            .iter()
            .any(|p| p.ends_with("cargo-test-fixture.rs"))
    );
    assert!(
        !discovered
            .iter()
            .any(|p| p.ends_with("multifrontmatter.rs"))
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn module_graph_discovery_all_rules_and_edge_cases() {
    let base = std::env::temp_dir().join(format!("rsharp_modgraph_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let proj = base.join("ws");
    std::fs::create_dir_all(proj.join("src").join("inline_plain")).unwrap();
    std::fs::create_dir_all(proj.join("src").join("custom_inline")).unwrap();
    std::fs::create_dir_all(proj.join("src").join("custom_os")).unwrap();
    std::fs::create_dir_all(proj.join("src").join("nightly_fallback").join("fb_dir")).unwrap();
    std::fs::create_dir_all(proj.join("src").join("dir_not_file")).unwrap();
    std::fs::create_dir_all(proj.join("tests").join("fixtures")).unwrap();

    std::fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"ws\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("lib.rs"),
        "macro_rules! decl_mods { ($($m:ident),*) => { $(mod $m;)* }; }\n\
         macro_rules! static_mod { () => { mod from_static_macro; }; }\n\
         macro_rules! plain_mac { () => { fn nop() {} }; }\n\
         decl_mods!(macro_a, (macro_b));\n\
         static_mod!();\n\
         plain_mac!();\n\
         #[path = \"custom_os/unix.rs\"]\n\
         #[cfg_attr(windows, path = \"custom_os/win.rs\")]\n\
         #[cfg_attr(unix, 123)]\n\
         #[path = 123]\n\
         mod os_imp;\n\
         #[path = \"missing_mod.rs\"]\n\
         mod missing_mod;\n\
         #[path = \"dir_not_file\"]\n\
         mod dir_not_file_mod;\n\
         #[path = \"lib.rs\"]\n\
         mod cycle_self;\n\
         mod inline_plain {\n\
             pub mod leaf;\n\
         }\n\
         #[path = \"custom_inline\"]\n\
         mod inline_path {\n\
             pub mod leaf;\n\
         }\n\
         mod r#match;\n\
         cfg_if::cfg_if! {\n\
             if #[cfg(unix)] {\n\
                 pub(crate) mod cfg_if_mod;\n\
             } else {\n\
                 pub mod cfg_if_else;\n\
             }\n\
         }\n\
         include!(\"included_mods.rs\");\n\
         include!(\"included_mods.rs\");\n\
         include!(\"missing_inc.rs\");\n\
         include!(\"dir_not_file\");\n\
         mod nightly_fallback;\n",
    )
    .unwrap();
    // Unreferenced pseudocode file inside `src/` (like linera-protocol's `template.rs`) MUST be skipped!
    std::fs::write(
        proj.join("src").join("template.rs"),
        "fn {{ invalid_pseudocode }}()\n",
    )
    .unwrap();
    std::fs::write(proj.join("src").join("macro_a.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(proj.join("src").join("macro_b.rs"), "pub fn b() {}\n").unwrap();
    std::fs::write(
        proj.join("src").join("from_static_macro.rs"),
        "pub fn s() {}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("custom_os").join("unix.rs"),
        "pub fn u() {}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("custom_os").join("win.rs"),
        "pub fn w() {}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("inline_plain").join("leaf.rs"),
        "pub fn il() {}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src").join("custom_inline").join("leaf.rs"),
        "pub fn cl() {}\n",
    )
    .unwrap();
    std::fs::write(proj.join("src").join("match.rs"), "pub fn m() {}\n").unwrap();
    std::fs::write(proj.join("src").join("cfg_if_mod.rs"), "pub fn c1() {}\n").unwrap();
    std::fs::write(proj.join("src").join("cfg_if_else.rs"), "pub fn c2() {}\n").unwrap();
    // Included fragment itself MUST NOT appear in discovered, but `mod from_inc;` inside it MUST!
    std::fs::write(proj.join("src").join("included_mods.rs"), "mod from_inc;\n").unwrap();
    std::fs::write(proj.join("src").join("from_inc.rs"), "pub fn inc() {}\n").unwrap();
    // Valid Rust token stream that fails `syn::parse_file`, exercising the token-stream fallback!
    std::fs::write(
        proj.join("src").join("nightly_fallback.rs"),
        "#![allow(dead_code)]\n\
         #[1 + 2]\n\
         gen fn nightly_only() {}\n\
         #[path = \"fb_target.rs\"]\n\
         pub(crate) mod fb_t;\n\
         #[path = \"fb_dir\"]\n\
         mod fb_in { pub mod leaf; }\n\
         include!(\"fb_inc.rs\");\n\
         include!(concat!(\"x\", \".rs\"));\n\
         let include = 1;\n\
         { mod at_zero_missing; }\n\
         $mod dollar_prefixed;\n\
         mod $m;\n\
         mod bad = 1;\n\
         #not_bracket\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src")
            .join("nightly_fallback")
            .join("fb_target.rs"),
        "pub fn ft() {}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src")
            .join("nightly_fallback")
            .join("fb_dir")
            .join("leaf.rs"),
        "pub fn fl() {}\n",
    )
    .unwrap();
    std::fs::write(proj.join("src").join("fb_inc.rs"), "mod fb_from_inc;\n").unwrap();
    std::fs::write(
        proj.join("src")
            .join("nightly_fallback")
            .join("fb_from_inc.rs"),
        "pub fn ffi() {}\n",
    )
    .unwrap();
    // Integration test target using `#[path = "fixtures/helper.rs"]` (outside `src/` without `mod.rs`)!
    std::fs::write(
        proj.join("tests").join("integ.rs"),
        "#[path = \"fixtures/helper.rs\"]\nmod helper;\n#[test]\nfn t() { helper::h(); }\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("tests").join("fixtures").join("helper.rs"),
        "pub fn h() {}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("tests").join("fixtures").join("unreferenced.rs"),
        "broken {{{\n",
    )
    .unwrap();

    let discovered = discover_cargo_rs_files(&proj).expect("should discover module graph");
    for expected in [
        "lib.rs",
        "macro_a.rs",
        "macro_b.rs",
        "from_static_macro.rs",
        "unix.rs",
        "win.rs",
        "match.rs",
        "cfg_if_mod.rs",
        "cfg_if_else.rs",
        "from_inc.rs",
        "nightly_fallback.rs",
        "fb_target.rs",
        "fb_from_inc.rs",
        "integ.rs",
        "helper.rs",
    ] {
        assert!(
            discovered.iter().any(|p| p.ends_with(expected)),
            "missing {expected} in {discovered:?}"
        );
    }
    assert!(
        discovered
            .iter()
            .any(|p| p.ends_with("inline_plain\\leaf.rs") || p.ends_with("inline_plain/leaf.rs"))
    );
    assert!(
        discovered
            .iter()
            .any(|p| p.ends_with("custom_inline\\leaf.rs") || p.ends_with("custom_inline/leaf.rs"))
    );
    assert!(
        discovered
            .iter()
            .any(|p| p.ends_with("fb_dir\\leaf.rs") || p.ends_with("fb_dir/leaf.rs"))
    );
    for excluded in [
        "template.rs",
        "included_mods.rs",
        "fb_inc.rs",
        "unreferenced.rs",
    ] {
        assert!(
            !discovered.iter().any(|p| p.ends_with(excluded)),
            "unexpectedly discovered {excluded} in {discovered:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn phase5_stage_c_lifetime_validator_brace_macro_tail_and_cargo_features() {
    use rsharp_core::{CargoFeaturesMode, RustAnalyzerOracle};
    use std::cell::RefCell;
    use std::io::{Cursor, Write};
    use std::rc::Rc;

    // 1. Tail-expression brace-macro lowering (`return cfg_select! { ... };` -> `cfg_select! { ... }`)
    //    (fixes `emilk/egui` `context.rs` Stage C semantic bug).
    let src_macro_tail = "\
macro_rules! my_select {
    ($x:expr) => { $x };
}
pub fn pick(v: i32) -> i32 {
    let base = v + 1;
    #[allow(unused)]
    my_select! {
        base * 2
    }
}
";
    let (rsh_mt, rust_mt) = convert_no_ra(src_macro_tail);
    assert_eq!(count_raw_rust_blocks(&rsh_mt), 0, "{rsh_mt}");
    assert!(
        !rust_mt.contains("return my_select!"),
        "brace macro tail must not emit `return my_select!`:\n{rust_mt}"
    );
    assert!(
        rust_mt.contains("#[allow(unused)]\n    my_select!"),
        "{rust_mt}"
    );
    rsharp_core::check_rsharp(&rsh_mt).expect("tail brace macro must pass rustc");

    // 2. In-scope lifetime validator for `rust-analyzer` hints (fixes `emilk/egui` `Atoms<'a>` E0261/E0495):
    let src_lt = "\
struct Atoms<'a>(&'a str);
trait Tr<'a> {
    fn def_fn(&self) {
        let a = Atoms(\"x\");
        let _ = a;
    }
}
trait DynTr {}
trait IterTr { type Item; }
struct Holder<'ok>(&'ok str);
impl<'ok> Holder<'ok> {
    fn method(&self) {
        let ok_dyn = make_dyn();
        let bad_dyn = make_bad_dyn();
        let bad_vec = make_bad_vec();
        let bad_assoc = make_bad_assoc();
        let bad_qself = make_bad_qself();
        let bad_fn_in = make_bad_fn_in();
        let bad_fn_ret = make_bad_fn_ret();
        let good_hrtb_fn = make_hrtb_fn();
        let good_hrtb_dyn = make_hrtb_dyn();
        let good_plain_fn = make_plain_fn();
        let good_const_arg = make_const_arg();
        let good_use_dyn = make_use_dyn();
        let atoms_local = Atoms(self.0);
        let f = |p| p;
        let _ = (
            ok_dyn, bad_dyn, bad_vec, bad_assoc, bad_qself, bad_fn_in, bad_fn_ret,
            good_hrtb_fn, good_hrtb_dyn, good_plain_fn, good_const_arg, good_use_dyn,
            atoms_local, f,
        );
    }
}
";
    let mut hints = TypeHintMap::default();
    let find_end = |needle: &str| -> (u32, u32) {
        for (idx, line) in src_lt.lines().enumerate() {
            if let Some(pos) = line.find(needle) {
                let col = pos + needle.len();
                return ((idx + 1) as u32, col as u32);
            }
        }
        panic!("needle not found: {needle}");
    };
    let mut add = |needle: &str, ty: &str| {
        let (l, c) = find_end(needle);
        hints.insert(l, c, ty.to_string());
    };
    add("let a", "Atoms<'a>");
    add("let ok_dyn", "Box<dyn DynTr + 'ok>");
    add("let bad_dyn", "Box<dyn DynTr + 'undeclared>");
    add("let bad_vec", "Vec<Box<dyn DynTr + 'undeclared>>");
    add(
        "let bad_assoc",
        "Box<dyn IterTr<Item = Box<dyn DynTr + 'undeclared>>>",
    );
    add(
        "let bad_qself",
        "<Box<dyn DynTr + 'undeclared> as IterTr>::Item",
    );
    add("let bad_fn_in", "fn(Box<dyn DynTr + 'undeclared>)");
    add("let bad_fn_ret", "fn() -> Box<dyn DynTr + 'undeclared>");
    add("let good_hrtb_fn", "for<'h> fn(Atoms<'h>) -> Atoms<'h>");
    add("let good_hrtb_dyn", "Box<dyn for<'h> Tr<'h> + 'static>");
    add("let good_plain_fn", "fn(i32)");
    add("let good_const_arg", "Arr<4>");
    add("let good_use_dyn", "impl DynTr + use<>");
    add("let atoms_local", "Atoms<'ok>");
    let (cl_line, cl_col) = find_end("|p");
    hints.insert(cl_line, cl_col, "Atoms<'a>".to_string());

    let rsh_lt = convert_from_rust_with_hints(
        src_lt,
        FromRustOptions {
            use_rustc_inference: false,
        },
        &hints,
        false,
    )
    .expect("lifetime-sanitized conversion must succeed");
    assert_eq!(count_raw_rust_blocks(&rsh_lt), 0, "{rsh_lt}");
    assert!(rsh_lt.contains("Atoms<life _> a ="), "{rsh_lt}");
    assert!(rsh_lt.contains("Atoms<life _> atoms_local ="), "{rsh_lt}");
    assert!(
        rsh_lt.contains("Box<dyn DynTr + life ok> ok_dyn ="),
        "{rsh_lt}"
    );
    assert!(rsh_lt.contains("var bad_dyn ="), "{rsh_lt}");
    assert!(rsh_lt.contains("var bad_vec ="), "{rsh_lt}");
    assert!(rsh_lt.contains("var bad_assoc ="), "{rsh_lt}");
    assert!(rsh_lt.contains("var bad_qself ="), "{rsh_lt}");
    assert!(rsh_lt.contains("var bad_fn_in ="), "{rsh_lt}");
    assert!(rsh_lt.contains("var bad_fn_ret ="), "{rsh_lt}");
    assert!(
        rsh_lt.contains("forall<life h> (Atoms<life h>) -> Atoms<life h> good_hrtb_fn ="),
        "{rsh_lt}"
    );
    assert!(
        rsh_lt.contains("Box<dyn forall<life h> Tr<life h> + static> good_hrtb_dyn ="),
        "{rsh_lt}"
    );
    assert!(rsh_lt.contains("(int) -> void good_plain_fn ="), "{rsh_lt}");
    assert!(rsh_lt.contains("(p) => p"), "{rsh_lt}");
    let rust_lt = transpile_to_rust(
        &rsh_lt,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("lifetime-sanitized R# must transpile");
    assert!(rust_lt.contains("let a: Atoms<'_> ="), "{rust_lt}");
    assert!(
        rust_lt.contains("let atoms_local: Atoms<'_> ="),
        "{rust_lt}"
    );

    // 3. CargoFeaturesMode parsing, cargo_args, and LSP initializationOptions payload:
    assert_eq!(
        CargoFeaturesMode::parse_cli("default").unwrap(),
        CargoFeaturesMode::Default
    );
    assert_eq!(
        CargoFeaturesMode::parse_cli("").unwrap(),
        CargoFeaturesMode::Default
    );
    assert_eq!(
        CargoFeaturesMode::parse_cli("all").unwrap(),
        CargoFeaturesMode::All
    );
    assert_eq!(
        CargoFeaturesMode::parse_cli("rt, time").unwrap(),
        CargoFeaturesMode::List(vec!["rt".to_string(), "time".to_string()])
    );
    assert!(CargoFeaturesMode::parse_cli(",,,").is_err());
    assert!(CargoFeaturesMode::Default.cargo_args().is_empty());
    assert_eq!(
        CargoFeaturesMode::All.cargo_args(),
        vec!["--all-features".to_string()]
    );
    assert_eq!(
        CargoFeaturesMode::List(vec!["a".to_string(), "b".to_string()]).cargo_args(),
        vec!["--features".to_string(), "a,b".to_string()]
    );

    struct SharedWriter(Rc<RefCell<Vec<u8>>>);
    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let frame = |body: &str| format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    let make_init_stream = || {
        let mut buf = Vec::new();
        buf.extend(frame(r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#).as_bytes());
        buf.extend(
            frame(
                r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"quiescent":true}}"#,
            )
            .as_bytes(),
        );
        buf
    };

    let sent_all = Rc::new(RefCell::new(Vec::new()));
    let _o_all = RustAnalyzerOracle::from_io_with_features(
        Cursor::new(make_init_stream()),
        SharedWriter(Rc::clone(&sent_all)),
        &std::env::temp_dir(),
        &CargoFeaturesMode::All,
    )
    .expect("from_io_with_features All");
    let text_all = String::from_utf8(sent_all.borrow().clone()).unwrap();
    assert!(text_all.contains(r#""features":"all""#), "{text_all}");

    let sent_list = Rc::new(RefCell::new(Vec::new()));
    let _o_list = RustAnalyzerOracle::from_io_with_features(
        Cursor::new(make_init_stream()),
        SharedWriter(Rc::clone(&sent_list)),
        &std::env::temp_dir(),
        &CargoFeaturesMode::List(vec!["rt".to_string(), "time".to_string()]),
    )
    .expect("from_io_with_features List");
    let text_list = String::from_utf8(sent_list.borrow().clone()).unwrap();
    assert!(
        text_list.contains(r#""features":["rt","time"]"#),
        "{text_list}"
    );

    // 4. Live Stage C discoveries:
    //    a) Struct destructuring of lifetime-parameterized fields (`Atoms<'a>` -> `Atoms<'_>`)
    //       compiles inside `impl Window<'_>` (fixes `emilk/egui` `window.rs` & `checkbox.rs`).
    //    b) Capturing closures (`|| self.err(...)` / `|(a, _): (i32, i32)| a + outer`) stay `var`
    //       instead of emitting an invalid `fn()` pointer (fixes `zoxide --no-ra`).
    //    c) Files swapping `#[cfg(test)]` vs `#[cfg(not(test))]` skip RA hints cleanly
    //       (fixes `tokio/src/fs/file.rs` dual `--lib` / `--tests` compilation).
    let src_stage_c = "\
struct Atoms<'a>(&'a str);
struct StaticAtoms(&'static str);
struct Multi<'a, 'b>(&'a str, &'b str);
struct Win<'a> { title: Atoms<'a>, s: StaticAtoms, m: Multi<'a, 'static> }
impl Win<'_> {
    fn show(self) -> usize {
        let Win { title, s, m } = self;
        let outer = 1i32;
        let cap_self = || self_len(s.0);
        let cap_outer = |(a, _): (i32, i32)| a + outer;
        let non_cap = |(a, _): (i32, i32)| a + 1;
        let _ = (cap_self, cap_outer, non_cap, m.0);
        title.0.len()
    }
}
fn self_len(s: &str) -> usize { s.len() }
";
    let (rsh_sc, rust_sc) = convert_no_ra(src_stage_c);
    assert_eq!(count_raw_rust_blocks(&rsh_sc), 0, "{rsh_sc}");
    assert!(rsh_sc.contains("Atoms<life _> title"), "{rsh_sc}");
    assert!(rsh_sc.contains("Multi<life _, life static> m"), "{rsh_sc}");
    assert!(rsh_sc.contains("var cap_outer ="), "{rsh_sc}");
    assert!(rsh_sc.contains("((int, int)) -> int non_cap ="), "{rsh_sc}");
    assert!(rust_sc.contains("Atoms<'_>"), "{rust_sc}");
    rsharp_core::check_rsharp(&rsh_sc)
        .expect("struct field lifetime elision & capturing closure must pass rustc");

    // Capturing `self` directly inside a method closure:
    let src_self_cap = "\
struct S(i32);
impl S {
    fn get(&self) -> i32 { self.0 }
    fn run(&self) -> i32 {
        let f = || self.get();
        f()
    }
}
";
    let (rsh_self, _) = convert_no_ra(src_self_cap);
    assert!(rsh_self.contains("var f = () => this.get();"), "{rsh_self}");
    rsharp_core::check_rsharp(&rsh_self).expect("self-capturing closure must pass rustc");

    // Dual `#[cfg(test)]` / `#[cfg(not(test))]` file skips RA hints without querying LSP:
    let mut o_dual = RustAnalyzerOracle::from_io_with_features(
        Cursor::new(make_init_stream()),
        Vec::new(),
        &std::env::temp_dir(),
        &CargoFeaturesMode::Default,
    )
    .unwrap();
    let dual_hints = o_dual
        .type_hints_for_file(
            std::path::Path::new("file.rs"),
            "#[cfg(test)]\nuse mocks::J;\n#[cfg(not(test))]\nuse real::J;\n",
        )
        .unwrap();
    assert!(dual_hints.is_empty());

    // Sound method-call inference (fixes `tokio` `file.rs` + `open_options.rs` Stage C):
    // - `path.to_owned()` on `&Path` stays `var`, while `"hi".to_owned()` infers `string`
    // - `opts.clone()` on `&Opts` stays `var`, while owned `v.clone()` infers `Opts`
    // - `times.into_std()` does NOT steal `MyFile::into_std() -> i32`, while `f.into_std()` does!
    let src_methods = "\
use std::path::Path;
#[derive(Clone)]
struct Opts(i32);
struct MyFile;
struct MyTimes;
impl MyFile { fn into_std(&self) -> i32 { 1 } }
impl MyTimes { fn into_std(&self) -> u64 { 2 } }
pub fn check_methods(p: &Path, o: &Opts, owned_o: Opts, f: &MyFile, t: MyTimes) {
    let pb = p.to_owned();
    let s = \"hi\".to_owned();
    let oc = o.clone();
    let owned_c = owned_o.clone();
    let fi = f.into_std();
    let ti = t.into_std();
    let _ = (pb, s, oc, owned_c, fi, ti);
}
";
    let (rsh_m, _) = convert_no_ra(src_methods);
    assert!(rsh_m.contains("var pb = p.to_owned();"), "{rsh_m}");
    assert!(rsh_m.contains("string s = \"hi\".to_owned();"), "{rsh_m}");
    assert!(rsh_m.contains("var oc = o.clone();"), "{rsh_m}");
    assert!(rsh_m.contains("Opts owned_c = owned_o.clone();"), "{rsh_m}");
    assert!(rsh_m.contains("int fi = f.into_std();"), "{rsh_m}");
    assert!(rsh_m.contains("ulong ti = t.into_std();"), "{rsh_m}");
    rsharp_core::check_rsharp(&rsh_m).expect("sound method-call inference must pass rustc");

    // Local `use` shadowing an outer struct with a trait (fixes `tokio` `mpsc/bounded.rs` E0782):
    let src_local_shadow = "\
mod chan {
    pub trait Semaphore { fn is_idle(&self) -> bool; }
}
pub struct Semaphore;
impl chan::Semaphore for Semaphore { fn is_idle(&self) -> bool { true } }
pub fn check_shadow(s: &Semaphore) -> bool {
    use chan::Semaphore;
    let sem = s;
    let ok = sem.is_idle();
    ok
}
";
    let mut sh_hints = TypeHintMap::default();
    let find_sh = |needle: &str| -> (u32, u32) {
        for (idx, line) in src_local_shadow.lines().enumerate() {
            if let Some(pos) = line.find(needle) {
                return ((idx + 1) as u32, (pos + needle.len()) as u32);
            }
        }
        panic!("needle not found: {needle}");
    };
    let (l1, c1) = find_sh("let sem");
    sh_hints.insert(l1, c1, "&Semaphore".to_string());
    let (l2, c2) = find_sh("let ok");
    sh_hints.insert(l2, c2, "bool".to_string());
    let rsh_sh = convert_from_rust_with_hints(
        src_local_shadow,
        FromRustOptions {
            use_rustc_inference: false,
        },
        &sh_hints,
        false,
    )
    .expect("local use shadow conversion must succeed");
    assert!(rsh_sh.contains("var sem = s;"), "{rsh_sh}");
    assert!(rsh_sh.contains("bool ok = sem.is_idle();"), "{rsh_sh}");
    rsharp_core::check_rsharp(&rsh_sh).expect("local use shadow must pass rustc");

    // Non-capturing closure returning `async { ... }` must stay `var`, NOT `fn() -> impl Future` (E0562, `tokio`):
    let src_async_clo = "pub fn f() { let quick = || async {}; let _ = quick(); }";
    let (rsh_ac, _) = convert_no_ra(src_async_clo);
    assert!(rsh_ac.contains("var quick = () => async"), "{rsh_ac}");
    rsharp_core::check_rsharp(&rsh_ac).expect("closure returning async block must pass rustc");

    // Unsuffixed numeric array/repeat/unary inlines stay `var` when RA hints are active (fixes `tokio` `tests/udp.rs`):
    let src_unsuf = "\
pub fn takes_u8(_: &mut [u8], _: i8) {}
pub fn run() {
    let other = 1usize;
    let mut buf = [0; 4];
    let mut arr = [1, 2];
    let neg = -1;
    takes_u8(&mut buf, neg);
    takes_u8(&mut arr, neg);
    let _ = other;
}
";
    let mut u_hints = TypeHintMap::default();
    let (ol, oc) = src_unsuf
        .lines()
        .enumerate()
        .find_map(|(i, l)| {
            l.find("let other")
                .map(|p| ((i + 1) as u32, (p + 9) as u32))
        })
        .unwrap();
    u_hints.insert(ol, oc, "usize".to_string());
    let rsh_u = convert_from_rust_with_hints(
        src_unsuf,
        FromRustOptions {
            use_rustc_inference: false,
        },
        &u_hints,
        false,
    )
    .expect("unsuffixed array/repeat/unary conversion must succeed");
    assert!(rsh_u.contains("mut var buf = [0; 4];"), "{rsh_u}");
    assert!(rsh_u.contains("mut var arr = [1, 2];"), "{rsh_u}");
    assert!(rsh_u.contains("var neg = -1;"), "{rsh_u}");
    rsharp_core::check_rsharp(&rsh_u).expect("unsuffixed array/repeat/unary must pass rustc");

    // Parenthesized array-of-reference inference (`&["a", "b"]` -> `read (read str)[2]`, fixes `fd --no-ra`)
    // and heterogeneous binary ops (`Instant + Duration`, fixes `tokio` `tests/time_sleep.rs`):
    let src_arr_bin = "\
use std::time::{Duration, Instant};
pub fn check_arr_and_time(now: Instant) -> Instant {
    let dirs = &[\"a\", \"b\"];
    let rep = [\"x\"; 2];
    let deadline = (now + Duration::from_millis(1)) + Duration::new(0, 1);
    let _ = (dirs, rep);
    deadline
}
";
    let (rsh_ab, rust_ab) = convert_no_ra(src_arr_bin);
    assert!(rsh_ab.contains("read (read str)[2] dirs ="), "{rsh_ab}");
    assert!(rsh_ab.contains("(read str)[2] rep ="), "{rsh_ab}");
    assert!(rust_ab.contains("let dirs: &[(&str); 2] ="), "{rust_ab}");
    assert!(rsh_ab.contains("var deadline ="), "{rsh_ab}");
    rsharp_core::check_rsharp(&rsh_ab)
        .expect("ref-array parens and Instant + Duration must pass rustc");
}
