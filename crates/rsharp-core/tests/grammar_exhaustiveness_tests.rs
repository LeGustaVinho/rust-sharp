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

    // Nightly/experimental Expr::Verbatim & Item::Verbatim still fall back safely to `rust { ... }`.
    let (rsh_verb, _) = convert_no_ra(
        "type VerbatimGeneric<T>;\ntype VerbatimWhere where i32: Copy;\nmacro decl_mac() {}\nfn tail_call(x: i32) -> i32 { become tail_call(x) }",
    );
    assert_eq!(count_raw_rust_blocks(&rsh_verb), 4, "got:\n{rsh_verb}");
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
        "trait BadAlias = ;",
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
