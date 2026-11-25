use rsharp_core::{
    FromRustOptions, PreludeMode, TranspileOptions, check_rsharp, convert_from_rust,
    lexer::tokenize, parser::parse_file, run_rsharp, rustc_runner, token::TokenKind,
    transpile_to_rust,
};

#[test]
fn test_wave2_lexer_and_parser_surface() {
    let src = r#"
//! Module inner doc
/*! Block inner doc
 * second line
 */
/** Outer block doc */
/* nested /* regular */ comment */
public const double TRAILING = 1.;
public const read byte[2] RAW_BYTES = "\xff\x00"u8;

macro mp ( ($x:expr) => { $x + 1 }; );
macro mb [ ($x:expr) => { $x * 2 }; ];

public int @type() {
    int @async = 1;
    double d = 1. + 2.;
    int m = 1.abs();
    int sum = 0;
    foreach (int i in 1..4) {
        _ = i;
    }
    return @async + (d as int) + m + sum + mp!(2) + mb!(3);
}
"#;
    let tokens = tokenize(src).expect("wave 2 tokens should lex");
    assert!(
        tokens
            .iter()
            .any(|t| matches!(&t.kind, TokenKind::InnerDocComment(s) if s == "Module inner doc"))
    );
    assert!(
        tokens
            .iter()
            .any(|t| matches!(&t.kind, TokenKind::InnerDocComment(s) if s == "Block inner doc"))
    );
    assert!(
        tokens
            .iter()
            .any(|t| matches!(&t.kind, TokenKind::DocComment(s) if s == "Outer block doc"))
    );
    assert!(
        tokens
            .iter()
            .any(|t| matches!(&t.kind, TokenKind::FloatLit(s) if s == "1."))
    );

    let ast = parse_file(src).expect("wave 2 file should parse");
    assert_eq!(ast.inner_docs.len(), 3);

    // Empty block doc comment and unterminated block doc error
    let empty_doc_tokens = tokenize("/*!   */").expect("empty inner doc should lex");
    assert!(matches!(
        empty_doc_tokens[0].kind,
        TokenKind::InnerDocComment(_)
    ));
    assert!(tokenize("/** unterminated").is_err());

    // Variadic `...` in tuple type without `->` must be rejected
    assert!(parse_file("alias Bad = (int, ...);").is_err());
}

#[test]
fn semantic_rust_199_if_let_guards_variadic_const_infer_and_naked() {
    let rsharp = r#"
public bool[N] AllFalse<const nuint N>() {
    return [false; _];
}

public int MatchIfLetGuard(Result<int, int> res, int? bonus) {
    match (res) {
        Ok(int a) when bonus is Some(int b) && b > 0 => { return a + b; }
        Ok(int a) when bonus is None => { return a; }
        Err(int e) when bonus is Some(int b) => { return e - b; }
        _ => { return 0; }
    }
}

public int MatchConstWithIfLetGuard(int n, int? opt) {
    match (n) {
        const(3) when opt is Some(int k) => { return k * 10; }
        _ => { return -1; }
    }
}

public unsafe extern "C" int SumVariadicStub(int n, mut ... args) {
    _ = write args;
    return n;
}

public void main() {
    bool[3] arr = AllFalse<_>();
    int a = MatchIfLetGuard(Ok(10), Some(5));
    int b = MatchIfLetGuard(Ok(10), None);
    int c = MatchIfLetGuard(Err(20), Some(4));
    int d = MatchConstWithIfLetGuard(3, Some(7));
    int v = unsafe { return SumVariadicStub(42); };
    PrintLine($"arr={arr.len()},{arr[0]} a={a} b={b} c={c} d={d} v={v}");
}
"#;
    let out = run_rsharp(rsharp).expect("Rust 1.89/1.95/1.99 constructs must compile and run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "arr=3,false a=15 b=10 c=16 d=70 v=42");

    // Verify `[unsafe Naked]` compiles via rustc library check!
    let naked_lib = r#"
[unsafe Naked]
public unsafe extern "C" void NakedRet() {
    core.arch.naked_asm!("ret");
}
"#;
    check_rsharp(naked_lib).expect("[unsafe Naked] must pass rustc --edition=2024 check");
}

#[test]
fn semantic_control_flow_exprs_if_expr_and_destructuring_assign() {
    let rsharp = r#"
public struct Point {
    public int X;
    public int Y;
}

public int EarlyExit(int? opt) {
    int v = match (opt) {
        Some(int x) => x,
        None => return -99,
    };
    return v + 1;
}

public int LoopControlExpr(int? a, int? b) {
    mut int step = 0;
    int res = loop {
        step++;
        int? cur = step == 1 ? a : b;
        int val = match (cur) {
            Some(int x) => x,
            None => continue,
        };
        break val * 2;
    };
    return res;
}

public int IfExprChain(int? opt, bool flag) {
    int picked = if (opt is Some(int v) && v > 0) => v + 100
        else if (flag) => 50
        else => 0;
    return picked;
}

public void main() {
    // Destructuring declaration vs assignment
    (mut int a, mut int b) = (10, 20);
    (a, b) = (b, a);

    [mut int p, mut int q] = [1, 2];
    [p, q] = [q + 10, p + 20];

    Point src = new Point { X = 77, Y = 88 };
    Point { X: a, Y: b } = src;

    // while & foreach in expression position
    mut int acc = 0;
    match (true) {
        true => while (acc < 3) {
            acc++;
        },
        false => (),
    }
    match (true) {
        true => inner_for: foreach (int k in 1..=3) {
            acc += k;
            if (k == 3) {
                break inner_for;
            }
        },
        false => (),
    }

    int e1 = EarlyExit(Some(41));
    int e2 = EarlyExit(None);
    int lp = LoopControlExpr(None, Some(21));
    int i1 = IfExprChain(Some(5), false);
    int i2 = IfExprChain(None, true);
    int i3 = IfExprChain(None, false);
    PrintLine($"ab={a},{b} pq={p},{q} acc={acc} e={e1},{e2} lp={lp} if={i1},{i2},{i3}");
}
"#;
    let out = run_rsharp(rsharp).expect("control-flow exprs and destructuring assign must run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.stdout.trim(),
        "ab=77,88 pq=12,21 acc=9 e=42,-99 lp=42 if=105,50,0"
    );
}

#[test]
fn semantic_assoc_bounds_gats_lifetimes_receivers_fn_ptrs_and_global_paths() {
    let rsharp = r#"
using global::std.fmt.Display;
using { std.cmp.Ordering, std.fmt.Debug };
using _ = std.fmt.Write;
extern crate core as _;

public trait Lending {
    type Item<life a>;
    Self.Item<static> Produce(read this);
}

public struct NumSource {
    public int Val;
}

impl Lending for NumSource {
    type Item<life a> = int;
    int Produce(read this) {
        return this.Val;
    }
}

public string FirstDisplayed<I : Iterator<Item : Display>>(mut I it) {
    match (it.next()) {
        Some(I.Item v) => { return $"{v}"; }
        None => { return String.new(); }
    }
}

public int UseGatEq<T : Lending<Item<static> = int>>(read T src) {
    return src.Produce();
}

public int UseGatBound<T : Lending<Item<static> : Copy + Display>>(read T src) {
    T.Item<static> x = src.Produce();
    T.Item<static> y = x;
    return $"{y}".len() as int;
}

public struct RefPair<life a, life b : a + static> {
    public read<a> int First;
    public read<b> int Second;
}

public struct Counter {
    public int Value;

    public int ConsumeAndAdd(mut take this, int delta) {
        this.Value += delta;
        return this.Value;
    }

    public int BoxedAdd(mut Box<Self> this, int delta) {
        this.Value += delta;
        return this.Value;
    }
}

alias NamedCb = (int x, int y) -> int;
alias VariadicPtr = unsafe extern "C" (read ptr<sbyte>, ...) -> int;

public int ApplyCb(NamedCb cb, int a, int b) {
    return cb(a, b);
}

public int AddTwo(int x, int y) {
    return x + y;
}

public int ModuleHelper() {
    return 123;
}

public void main() {
    List<int> nums = [7, 8];
    string s = FirstDisplayed(nums.into_iter());
    NumSource src = new NumSource { Val = 55 };
    int g1 = UseGatEq(read src);
    int g2 = UseGatBound(read src);

    const int STATIC_NUM = 9;
    int local = 4;
    RefPair rp = new RefPair { First = read local, Second = read STATIC_NUM };

    Counter c1 = new Counter { Value = 10 };
    int r1 = c1.ConsumeAndAdd(5);
    Box<Counter> c2 = Box.new(new Counter { Value = 20 });
    int r2 = c2.BoxedAdd(7);

    int cb_res = ApplyCb(AddTwo, 3, 4);
    nuint sz = global::core.mem.size_of<VariadicPtr>();
    int h = this.ModuleHelper();
    Ordering ord = 1.cmp(read 2);
    PrintLine($"s={s} g={g1},{g2} rp={deref rp.First},{deref rp.Second} c={r1},{r2} cb={cb_res} sz={sz > 0} h={h} ord={ord:?}");
}
"#;
    let out = run_rsharp(rsharp).expect("assoc bounds, GATs, receivers, fn ptrs must run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.stdout.trim(),
        "s=7 g=55,2 rp=4,9 c=15,27 cb=7 sz=true h=123 ord=Less"
    );
}

#[test]
fn semantic_macros_everywhere_inner_attrs_union_methods_patterns_and_raw_idents() {
    let rsharp = r#"
//! Crate-level inner doc
[module: Allow(unused)]

macro ty_i32 ( () => { i32 }; );
macro pat_five [ () => { 5 }; ];
macro trait_sig { () => { fn magic(&self) -> i32; }; }
macro impl_sig { () => { fn magic(&self) -> i32 { 77 } }; }
macro ext_empty { () => {}; }

namespace inner_mod {
    //! Inner namespace doc
    [module: Allow(dead_code)]
    public int Helper() {
        return 3;
    }
}

public trait HasMagic {
    [inner: Allow(dead_code)]
    trait_sig!();
}

public struct MagicBox {
    public int A;
    public int B;
}

impl HasMagic for MagicBox {
    [inner: Allow(dead_code)]
    impl_sig!();
}

unsafe extern "C" {
    [inner: Allow(dead_code)]
    ext_empty!();
}

public union Bits {
    public int I;
    public float F;

    public static Self FromInt(int v) {
        return new Bits { I = v };
    }

    public unsafe int AsInt(read this) {
        unsafe {
            return this.I;
        }
    }
}

public struct Pair(public int, public int);

public enum Choice {
    A([Allow(dead_code)] int),
    B(int),
}

public int UnwrapChoice((Choice.A(int v) | Choice.B(int v)) : Choice) {
    return v;
}

public read<@async> str RawLifetime<life @async>(read<@async> str s) {
    return s;
}

public T GenericWithAttr<[Allow(unused)] T>(T x) {
    return [Allow(unused)] (x);
}

public void main() {
    [inner: Allow(unused)]
    ty_i32!() x = 5;
    int matched = match (x) {
        pat_five!() => 100,
        _ => 0,
    };

    MagicBox mb = new MagicBox { [Allow(unused)] A = 1, B = 2 };
    MagicBox mb_copy = mb with {};
    int mg = mb_copy.magic();

    Bits b = Bits.FromInt(64);
    int bi = unsafe { return b.AsInt(); };

    Pair p1 = new Pair { 0 = 10, 1 = 20 };
    Pair p2 = p1 with { 0 = 30 };
    Pair { 0: int p0, .. } = p2;

    mut int mval = 8;
    write int mr = write mval;
    int got_mut = match (mr) {
        deref write int v => v + 1,
    };
    int got_paren = match (read (2, 9)) {
        deref ((1 | 2, int y)) => y,
        _ => 0,
    };
    (int only,) = (44,);
    int ch = UnwrapChoice(Choice.B(19));

    int @type = 7;
    read str rl = RawLifetime("raw_lt");
    int lbl_v = @loop: loop {
        break @loop 55;
    };
    double flt = 1. + 2.;
    read byte[2] raw_b = "\xff\x00"u8;
    int ga = GenericWithAttr(11);

    PrintLine($"m={matched} mg={mg} bi={bi} p0={p0} mut={got_mut} par={got_paren} only={only} ch={ch} kw={@type} rl={rl} lbl={lbl_v} f={flt} rb={raw_b[0]},{raw_b[1]} ga={ga} ns={inner_mod.Helper()}");
}
"#;
    let out = run_rsharp(rsharp).expect("all Wave 2 syntax features must compile and run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.stdout.trim(),
        "m=100 mg=77 bi=64 p0=30 mut=9 par=9 only=44 ch=19 kw=7 rl=raw_lt lbl=55 f=3 rb=255,0 ga=11 ns=3"
    );
}

#[test]
fn roundtrip_rust_199_wave2_zero_fallbacks_and_identical_behavior() {
    let rust_src = r##"
//! File inner doc
#![allow(unused)]

use ::std::fmt::Display;
use {std::cmp::Ordering, std::fmt::Write as _};
extern crate core as _;

macro_rules! ty_num ( () => { i32 }; );
macro_rules! pat_ten [ () => { 10 }; ];
macro_rules! tr_m { () => { fn ping(&self) -> i32; }; }
macro_rules! im_m { () => { fn ping(&self) -> i32 { 9 } }; }

mod sub_mod {
    //! Submodule inner doc
    #![allow(dead_code)]
    pub fn hi() -> i32 { 4 }
}

pub trait Ping {
    #![allow(dead_code)]
    tr_m!();
}

pub struct Boxed {
    pub a: i32,
    pub b: i32,
}

impl Ping for Boxed {
    #![allow(dead_code)]
    im_m!();
}

pub trait Lend {
    type Item<'a>;
    fn get(&self) -> Self::Item<'static>;
}

impl Lend for Boxed {
    type Item<'a> = i32;
    fn get(&self) -> i32 { self.a }
}

pub struct RefHolder<'a, 'b: 'a + 'static> {
    pub x: &'a i32,
    pub y: &'b i32,
}

pub struct Tup(pub i32, pub i32);

pub enum Pick {
    Left(#[allow(dead_code)] i32),
    Right(i32),
}

impl Boxed {
    pub fn consume(mut self, d: i32) -> i32 {
        self.a += d;
        self.a
    }
    pub fn boxed_add(mut self: Box<Self>, d: i32) -> i32 {
        self.b += d;
        self.b
    }
}

pub fn all_false<const N: usize>() -> [bool; N] {
    [false; _]
}

pub fn show_first<I: Iterator<Item: Display>>(mut it: I) -> String {
    match it.next() {
        Some(v) => format!("{}", v),
        None => String::new(),
    }
}

pub fn use_lend<T: Lend<Item<'static> = i32>>(t: &T) -> i32 {
    t.get()
}

pub fn use_lend_bound<T: Lend<Item<'static>: Copy>>(t: &T) -> i32 {
    let v = t.get();
    let _ = v;
    1
}

pub fn unwrap_pick((Pick::Left(v) | Pick::Right(v)): Pick) -> i32 {
    v
}

pub fn raw_lt<'r#async>(s: &'r#async str) -> &'r#async str {
    s
}

pub fn early(opt: Option<i32>) -> i32 {
    let v: i32 = match opt {
        Some(n) => n,
        None => return -1,
    };
    v + 1
}

pub fn id_gen<#[allow(unused)] T>(x: T) -> T {
    #[allow(unused)] (x)
}

pub fn main() {
    #![allow(unused)]
    let arr: [bool; 3] = all_false::<_>();
    let guard_v: i32 = match Ok::<i32, ()>(10) {
        Ok(a) if let Some(b) = Some(5) && b > 0 => a + b,
        _ => 0,
    };
    let if_v: i32 = if let Some(k) = Some(7) { k + 1 } else { 0 };
    let if_chain: i32 = if let Some(k) = None::<i32> { k } else if true { 8 } else { 9 };

    let (mut a, mut b): (i32, i32) = (1, 2);
    (a, b) = (b, a);
    let (only,): (i32,) = (33,);
    let [s0, s1] = [4, 5];

    let bx = Boxed { #[allow(unused)] a: 10, b: 20 };
    let bx2 = Boxed { ..bx };
    let Boxed { a: ba, b: bb } = bx2;
    let p = bx2.ping();
    let l1 = use_lend(&bx2);
    let l2 = use_lend_bound(&bx2);
    let c1 = bx2.consume(3);
    let c2 = Box::new(Boxed { a: 1, b: 2 }).boxed_add(4);

    let tp = Tup { 0: 7, 1: 8 };
    let tp2 = Tup { 0: 9, ..tp };
    let Tup { 0: t0, .. } = tp2;

    let mut mv: i32 = 6;
    let mr: &mut i32 = &mut mv;
    let dm: i32 = match mr {
        &mut v => v + 1,
    };
    let dp: i32 = match &(2, 11) {
        &((1 | 2, y)) => y,
        _ => 0,
    };

    let n: ty_num!() = 10;
    let pm: i32 = match n {
        pat_ten!() => 1,
        _ => 0,
    };

    let r#type: i32 = 42;
    let rlt: &str = raw_lt("ok");
    let lbl: i32 = 'r#loop: loop {
        break 'r#loop 99;
    };
    let flt: f64 = 1.;
    let rawb: &[u8; 2] = b"\xff\x00";
    let cb: fn(x: i32, y: i32) -> i32 = |#[allow(unused)] x: i32, y: i32| -> i32 { x + y };
    let cbv: i32 = cb(3, 4);
    let _vptr: Option<unsafe extern "C" fn(*const i8, ...) -> i32> = None;
    let sf = show_first(vec![3].into_iter());
    let up = unwrap_pick(Pick::Right(15));
    let ea = early(Some(10));
    let ig = id_gen(2);
    let ord = 1i32.cmp(&2i32);
    const CVAL: i32 = 5;
    let local_v: i32 = 3;
    let rh = RefHolder { x: &local_v, y: &CVAL };
    println!(
        "arr={} g={} if={},{} ab={},{} o={} s={},{} b={},{} p={} l={},{} c={},{} t0={} d={},{} pm={} kw={} rlt={} lbl={} f={} rb={},{} cb={} sf={} up={} ea={} ig={} ord={:?} rh={},{} sub={}",
        arr.len(), guard_v, if_v, if_chain, a, b, only, s0, s1, ba, bb, p, l1, l2, c1, c2, t0, dm, dp, pm, r#type, rlt, lbl, flt, rawb[0], rawb[1], cbv, sf, up, ea, ig, ord, *rh.x, *rh.y, sub_mod::hi()
    );
}
"##;

    let orig_out = rustc_runner::compile_and_run_rust(rust_src)
        .expect("original Rust 1.99.1 program must run");
    assert_eq!(orig_out.exit_code, 0);

    let rsharp = convert_from_rust(
        rust_src,
        FromRustOptions {
            use_rustc_inference: true,
        },
    )
    .expect("Rust 1.99.1 -> R# must succeed");

    assert!(
        !rsharp.contains("rust {"),
        "expected ZERO rust {{}} fallbacks, got:\n{}",
        rsharp
    );

    let rt_out = run_rsharp(&rsharp).expect("round-tripped R# must compile and run");
    assert_eq!(rt_out.exit_code, 0);
    assert_eq!(rt_out.stdout, orig_out.stdout);
}

#[test]
fn from_rust_covers_wave2_extra_branches() {
    let rust_src = r##"
#[unsafe(naked)]
pub unsafe extern "C" fn naked_fn() {
    core::arch::naked_asm!("ret");
}

pub unsafe extern "C" fn c_var(n: i32, #[allow(unused)] mut args: ...) -> i32 {
    let _ = &mut args;
    n
}

unsafe extern "C" {
    #![allow(dead_code)]
    ext_mac!();
}

pub fn extra_exprs(flag: bool, tup: (i32,)) -> i32 {
    let (single,) = tup;
    let (first, .., last) = (1, 2, 3, 4);
    let mut k: i32 = 0;
    let _ = match flag {
        true => while k < 2 {
            k += 1;
        },
        false => for _i in 0..1 {
            k += 1;
        },
    };
    let _c: i32 = loop {
        if k < 10 {
            k += 1;
            continue;
        }
        break k;
    };
    let _b: i32 = if flag {
        1
    } else if k > 0 {
        2
    } else if k < 0 {
        3
    } else {
        4
    };
    single + first + last
}
"##;

    let rsharp = convert_from_rust(
        rust_src,
        FromRustOptions {
            use_rustc_inference: false,
        },
    )
    .expect("extra Wave 2 from_rust branches should convert cleanly");

    assert!(!rsharp.contains("rust {"), "got:\n{}", rsharp);
    assert!(rsharp.contains("[unsafe Naked]"));
    assert!(rsharp.contains("mut ... args"));
    assert!(rsharp.contains("[inner: Allow(dead_code)]"));
    assert!(rsharp.contains("ext_mac!();"));
    assert!(rsharp.contains("(int first, .., int last)"));

    // Also verify the converted R# parses and transpiles cleanly back to Rust!
    let rust_back = transpile_to_rust(
        &rsharp,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("converted Wave 2 R# must transpile cleanly back to Rust");
    assert!(rust_back.contains("#[unsafe(naked)]"));
    assert!(rust_back.contains("mut args: ..."));
}

#[test]
fn wave2_edge_branches_and_lexer_nested_block_doc() {
    // Lexer: nested block comment inside a block doc comment + `//!` empty line
    let doc_src = "/** outer /* nested */ doc */\n//!\npublic const int K = 1;\n";
    let toks = tokenize(doc_src).expect("nested block doc comment must lex");
    assert!(
        toks.iter()
            .any(|t| matches!(t.kind, TokenKind::DocComment(_)))
    );
    assert!(
        toks.iter()
            .any(|t| matches!(t.kind, TokenKind::InnerDocComment(_)))
    );

    // Parser + codegen: labeled while expr, bare return/break/continue exprs, `is not` guard,
    // multiple expr attrs, and complex pattern parameter inference
    let rsharp = r#"
public void ControlExprs(int code) {
    mut int k = 0;
    outer_l: loop {
        match (code) {
            0 => return,
            1 => break outer_l,
            2 => continue outer_l,
            3 => inner_w: while (k < 2) {
                k++;
                break inner_w;
            },
            int x when x is not 99 => {
                _ = [Allow(unused)] [Allow(dead_code)] (x);
                break outer_l;
            }
            _ => break,
        }
        break outer_l;
    }
}

public int BreakValExpr(bool flag) {
    int v = lbl: loop {
        int r = match (flag) {
            true => break lbl 42,
            false => break 7,
        };
        _ = r;
    };
    return v;
}

public int PatParamDeref(deref int x) {
    return x;
}

public int PatParamDerefMut(deref write int x) {
    return x;
}

public int PatParamBindAt(_ as int n) {
    return n;
}

public int PatParamParen((int x)) {
    return x;
}

public int PatParamBorrow(borrow read int x) {
    return deref x;
}

public void main() {
    ControlExprs(3);
    int a = BreakValExpr(true);
    int b = BreakValExpr(false);
    mut int mv = 5;
    int d1 = PatParamDeref(read 4);
    int d2 = PatParamDerefMut(write mv);
    int p1 = PatParamParen(9);
    int ba = PatParamBindAt(8);
    int b1 = PatParamBorrow(6);
    PrintLine($"a={a} b={b} d={d1},{d2} p={p1} ba={ba} br={b1}");
}
"#;
    let out = run_rsharp(rsharp).expect("wave2 edge branches must compile and run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "a=42 b=7 d=4,5 p=9 ba=8 br=6");

    // from_rust: control-flow exprs with labels, non-UTF-8 byte escapes, brace macros in trait/impl/extern
    let rust_src = r##"
pub const BYTES: &[u8; 7] = b"a\\\"\n\r\t\xff";

pub trait BraceTr {
    tr_b! { fn a(&self); }
}

pub struct S;

impl BraceTr for S {
    im_b! { fn a(&self) {} }
}

unsafe extern "C" {
    ext_b! {}
}

pub fn flow_exprs(code: i32) -> i32 {
    let mut k: i32 = 0;
    let r: i32 = 'out: loop {
        let _ = match code {
            0 => return 0,
            1 => break 'out 11,
            2 => continue 'out,
            3 => 'wl: while k < 1 { k += 1; break 'wl; },
            4 => 'fl: for _i in 0..1 { break 'fl; },
            _ => return,
        };
    };
    let branch_v: i32 = if code == 1 {
        1
    } else if code == 2 {
        let t: i32 = 2;
        t + 1
    } else {
        0
    };
    r + branch_v
}
"##;
    let converted = convert_from_rust(
        rust_src,
        FromRustOptions {
            use_rustc_inference: false,
        },
    )
    .expect("wave2 extra from_rust branches must convert");
    assert!(!converted.contains("rust {"), "got:\n{}", converted);
    assert!(converted.contains(r#""a\\\"\n\r\t\xff"u8"#));
    assert!(converted.contains("tr_b!{"));
    assert!(converted.contains("im_b!{"));
    assert!(converted.contains("ext_b!{"));
    assert!(converted.contains("break out 11"));
    assert!(converted.contains("continue out"));
}
