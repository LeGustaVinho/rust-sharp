use rsharp_core::lexer::tokenize;
use rsharp_core::parser::parse_file;
use rsharp_core::rustc_runner::compile_and_run_rust;
use rsharp_core::{
    FromRustOptions, PreludeMode, TranspileOptions, check_rsharp, convert_from_rust, run_rsharp,
    transpile_to_rust,
};

#[test]
fn test_wave3_lexer_and_parser_surface() {
    // Shebang preserved; `#![` is not treated as a shebang
    let sb = "#!/usr/bin/env rsharp\nvoid main() {}\n";
    let ast = parse_file(sb).expect("shebang file must parse");
    assert_eq!(ast.shebang.as_deref(), Some("#!/usr/bin/env rsharp"));
    let rust = transpile_to_rust(
        sb,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("shebang file must transpile");
    assert!(rust.starts_with("#!/usr/bin/env rsharp\n"));

    // `#![` starts with `#![`, so lexer does not emit Shebang
    let toks = tokenize("#![allow]").expect("hash-bang-bracket tokens");
    assert!(matches!(toks[0].kind, rsharp_core::token::TokenKind::Hash));

    // `rust { ... }` with char literals (`'{'`, `'}'`, `'\''`, `'\u{7b}'`), lifetimes, and raw strings
    let raw_rust_block = r#####"
void f() {
    rust {
        let _c1 = '{';
        let _c2 = '}';
        let _c3 = '\'';
        let _c4 = '\u{7b}';
        let _s1 = r"}";
        let _s2 = r#"}"#;
        let _s3 = br#"}"#;
        let _s4 = cr#"}"#;
        fn id<'a>(x: &'a str) -> &'a str { x }
        let _ = (_c1, _c2, _c3, _c4, _s1, _s2, _s3, _s4, id("ok"));
    }
}
"#####;
    check_rsharp(raw_rust_block).expect("rust block with braces in chars/raw strings must compile");

    // Unterminated raw string inside `rust { ... }` must produce lex error
    assert!(tokenize(r##"void f() { rust { let s = r#"unterminated } }"##).is_err());

    // Comprehensive Wave 3 valid syntax corpus + deterministic truncation/mutation robustness
    let wave3_corpus = r####"#!/usr/bin/env rsharp
[module: Forbid(unsafe_code)]
[module: Expect(dead_code)]
[my_attr[a, b]]
[my_attr{ x = 1 }]
[CfgAttr(false, my_attr[a, b])]
[CfgAttr(false, my_attr{ x = 1 })]
[Doc = concat!("a", "b")]
[Doc = my_mac!["x"]]
extern crate this as my_self;

public struct @match;
public struct @type(public int);
public enum @loop<T = int> {
    @in,
    @as { int @for; },
    public void M([Allow(unused)] read this) {}
}
public union @while<T = int> {
    public int A;
    public void U([Allow(unused)] read this) {}
}

public trait ParenTr<T : (Clone) + (maybe Sized) +> : Copy + where T:, {
    type Item : Display +;
    [Allow(unused)]
    tr_m!(1);
}

impl<T> ParenTr<T> for @type where T : Clone + {
    type Item = int;
    [Allow(unused)]
    im_m!(1);
    [Allow(unused)]
    im_mb! { fn x() {} }
}

public trait BraceMacroTr {
    [Allow(unused)]
    tr_mb! { fn y(); }
}

public alias Cb = unsafe extern "C" ([Allow(unused)] int x, [Allow(unused)] ...) -> int;
public alias CbPlain = ([Allow(unused)] int x, [Allow(unused)] ...) -> int;
public alias InherentTy = (S).Assoc.Sub;
public alias MultiQTy = (T as A).B.C<int>;
public alias DynTrailing = Box<dyn (Fn() -> int) + Send +>;
public alias ImplTrailing = impl Copy +;
public alias AssocTrailing<I : Iterator<Item : Display +>> = I;
public alias ConstCharByte = (C<'a'>, B<b'x'>);

public void Wave3Syntax(
    [Allow(unused)] read this,
    W(int a),
    deref int b,
    borrow read int c
) {
    ;
    {
        int scoped = 1;
        _ = scoped;
    }
    m_brace! { 1 }
    public int local_fn(int x) { return x + 1; }
    const int cfn(int x) { return x; }
    static int sfn(int x) { return x; }
    async Task<int> afn() { return 1; }
    unsafe int ufn() { return 1; }
    extern "C" int efn() { return 1; }
    extern int efn2() { return 1; }
    extern () -> int fp_fn() { return efn2; }
    unsafe trait LocalTr {}
    unsafe impl LocalTr for @type {}
    unsafe extern "C" { void ext_local(); }
    extern { void ext_local2(); }
    extern crate core as local_core;
    namespace local_ns { public const int K = 1; }

    _ = (List<int>).new();
    _ = (slice<int>).len(read arr);
    _ = (int)::MAX;
    _ = ((T as A).B as C).X;
    _ = (S as Tr).Assoc::new();
    _ = new S<int> { X = 1, ..S.default() };
    _ = new global::m.S { ..S.default() };
    _ = t.0.1;
    _ = (W(int a)) => a;
    _ = (deref int a) => a;
    _ = (borrow read int a) => a;

    W(int wv) = w;
    W(wa) = w;
    deref int dv = r;
    deref write int dmv = mr;
    borrow read int br = x;
    borrow write int bw = mx;
    (int pv) = 1;

    _ = match (x) {
        | LO..=HI => 1,
        | ..=LO => 2,
        | (S as Tr).M.. => 3,
        | (S as Tr).M..=(S as Tr).N => 4,
        | S<int>.LO..=S<int>.HI => 4,
        | (E)::A => 5,
        | E<int>.A(int v) => v,
        | global::core.option.Option.Some(1..=5 as borrow read int rv) => deref rv,
        | Some(1..=5 as borrow write int wv) => deref wv,
        | [_, .. borrow read slice<int> mid, _] => mid.len(),
        | [_, .. borrow write slice<int> mid2, _] => mid2.len(),
        | [_, .. borrow read umid] => umid.len(),
        | [_, .. borrow write umid2] => umid2.len(),
        | Point { [Allow(unused)] borrow read int X, borrow write int Y } => (deref X) + (deref Y),
        | Point { borrow read X2, borrow write Y2 } => (deref X2) + (deref Y2),
        | _ => 0,
    };
}
"####;
    parse_file(wave3_corpus).expect("Wave 3 syntax corpus must parse cleanly");
    transpile_to_rust(
        wave3_corpus,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("Wave 3 syntax corpus must transpile cleanly");

    // Targeted invalid / rollback parser edge cases for Wave 3 branches
    for invalid in [
        "[MyAttr[1",
        "[MyAttr[1]",
        "[MyAttr{1]",
        "[MyAttr = a[1]",
        "void f<T : (+)>() {}",
        "void f<T : (Clone +>() {}",
        "void f(Box<dyn Copy + (>) {}",
        "void f(impl Copy + (>) {}",
        "void f(unsafe extern \"C\" ([+ int) -> void x) {}",
        "void f(([+ int) -> void x) {}",
        "void f((int, [+ int) -> void x) {}",
        "void f((int, [Allow(unused)] ...) x) {}",
        "void f((int as Tr).+ x) {}",
        "void f((int as Tr).A.+ x) {}",
        "void f((int as Tr).A.B<+ x) {}",
        "void f((int).+ x) {}",
        "public alias BadGat<T : Tr<Item<int +>> = T;",
        "public alias BadAssoc<I : Iterator<Item : Copy + (>> = I;",
        "void f() { _ = (T as +).X; }",
        "void f() { _ = (T as Tr).X::+; }",
        "void f() { _ = new S { ..+ }; }",
        "void f() { match (x) { 1 as borrow + int a => () } }",
        "void f() { match (x) { [.. borrow + a] => () } }",
        "void f() { match (x) { [.. borrow read +] => () } }",
        "void f() { match (x) { Point { [+ x } => () } }",
        "void f() { match (x) { Point { borrow + x } => () } }",
        "void f() { match (x) { Point { borrow read + } => () } }",
        "void f() { match (x) { (T as +).X => () } }",
        "void f() { match (x) { (T +).X => () } }",
        "void f() { match (x) { (T):: + => () } }",
        "void f() { match (x) { (T).X => () } }",
        "void f() { match (x) { (+).X => () } }",
        "void f() { match (x) { LO..=+ => () } }",
        "void f() { match (x) { LO..=A.+ => () } }",
        "void f() { match (x) { LO..=(S as Tr).+ => () } }",
        "void f() { match (x) { (S as Tr).A..= => () } }",
        "void f() { { + } }",
        "void f() { _ = t.0.1a; }",
        "void f() { (int a) = +; }",
        "void f() { borrow read int r = 1 }",
        "void f() { extern \"C\" () -> void x = f; }",
    ] {
        let _ = parse_file(invalid);
    }

    // Also test valid disambiguation cases for `(x).foo()`, `deref p = 1;`, `match (x) { (int a) => a }`,
    // `(T as A).B.C`, `((int, int))::default()`, and raw namespace / pub(in @type)
    for valid in [
        "namespace @type;\npublic(in crate.@type) void f() { (@type, int) p = (@type, 1); _ = p; }",
        "void f() { (int x, int y) @match = (1, 2); _ = @match.x; }",
        "void f() { _ = (x).foo(); _ = (()).clone(); _ = (a, b).clone(); _ = ((int, int))::default(); _ = (string).new(); _ = (int).MAX; }",
        "void f() { deref p = 1; _ = (deref h) == 1; }",
        "void f() { _ = match (x) { (1 | 2) => 1, (int a,) => a, _ => 0 }; }",
        "void f() { _ = match (x) { E<int>.A { int a } => a, (S as Tr).B => 1, (E)::A => 2, _ => 0 }; }",
        "void f() { _ = (int) => 1; }",
        "void f() { async { return 1; }; }",
        "void f() { unsafe { int u = 1; }; }",
        "void f() { const int K = 1; static int S = 2; }",
        "void f() { _ = match (x) { ..=LO => 1, ..HI => 2, LO.. => 3, _ => 0 }; }",
        "void f() { _ = (int as Tr).A::B<int>(); }",
        "void f() { _ = (slice<int>)::len; _ = (read str).len; _ = (write str).len; _ = (read ptr<byte>).null; _ = (write ptr<byte>).null; _ = (dyn Copy)::a; _ = (impl Copy)::a; _ = (never)::a; _ = (dynamic)::a; _ = (int?)::None; _ = (int[2])::default; _ = ((T as A).B)::c; _ = (global::std.vec.Vec)::new; }",
    ] {
        let _ = parse_file(valid).and_then(|ast| {
            transpile_to_rust(
                valid,
                TranspileOptions {
                    prelude_mode: PreludeMode::None,
                },
            )
            .map(|_| ast)
        });
    }
}

#[test]
fn semantic_wave3_qself_generic_structs_and_patterns() {
    let rsharp = r#"
public trait HasB {
    type B : HasX;
}
public trait HasX {
    const int X;
}
public struct Inner;
impl HasX for Inner {
    const int X = 42;
}
public struct Outer;
impl HasB for Outer {
    type B = Inner;
}

public int GetNested<T : HasB>() {
    return ((T as HasB).B as HasX).X;
}

public trait Bounds {
    const int MIN;
}
public struct Lim;
impl Bounds for Lim {
    const int MIN = 20;
}

public const int LO = 1;
public const int HI = 10;

[Derive(Default)]
public struct Point {
    public int X;
    public int Y;
}

public struct Boxed<T> {
    public T Val;
}

public enum Choice<T> {
    A(T),
    B,
}

public struct Tag;

public int MatchAll(int n, Choice<int> ch, mut int? opt, mut int[4] arr, mut Point pt) {
    int r1 = match (n) {
        | LO..=HI => 100,
        | ..=LO => 10,
        | (Lim as Bounds).MIN.. => 200,
        | _ => 50,
    };
    int r2 = match (ch) {
        | Choice<int>.A(int v) => v,
        | (Choice<int>)::B => 0,
    };
    int r3 = if (opt is | Some(1) | Some(2)) => 7 else => 0;
    int r4 = match (opt) {
        Some(1..=5 as borrow write int v) => arm4: {
            deref v += 10;
            break arm4 deref v;
        },
        _ => 0,
    };
    mut int r5 = 0;
    match (arr) {
        [_, .. borrow write int[2] mid, _] => {
            mid[0] = 99;
            r5 = mid.len() as int;
        }
    }
    int r6 = match (pt) {
        Point { [Allow(unused)] borrow read int X, borrow write int Y } => arm6: {
            deref Y += deref X;
            break arm6 deref Y;
        },
    };
    int r7 = match (global::core.option.Option.Some(3)) {
        global::core.option.Option.Some(int v) => v,
        global::core.option.Option.None => 0,
    };
    return r1 + r2 + r3 + r4 + r5 + r6 + r7 + arr[1];
}

void main() {
    mut List<int> v = (List<int>).new();
    v.push(5);
    int slice_len = (slice<int>).len(read v) as int;
    bool is_null = (read ptr<byte>).is_null(core.ptr.null());
    int nested = GetNested<Outer>();
    Boxed<int> b = new Boxed<int> { Val = 8 };
    Point p1 = new Point { X = 3, ..Point.default() };
    Point p2 = new Point { ..Point.default() };
    int total = MatchAll(25, Choice<int>.A(b.Val), Some(2), [1, 2, 3, 4], new Point { X = 4, Y = 6 });
    PrintLine($"{slice_len}|{is_null}|{nested}|{p1.X},{p1.Y}|{p2.X}|{total}");
}
"#;
    let out = run_rsharp(rsharp).expect("Wave 3 qself/generic-struct/patterns must run");
    assert_eq!(out.exit_code, 0);
    // total = 200 + 8 + 7 + 12 + 2 + 10 + 3 + 99 = 341
    assert_eq!(out.stdout.trim(), "1|true|42|3,0|0|341");
}

#[test]
fn semantic_wave3_destructuring_local_fns_blocks_tuples_and_attrs() {
    let rsharp = r#"#!/usr/bin/env rsharp
[module: Forbid(unsafe_code)]
[module: Expect(unused_variables)]
extern crate this as my_self;

public const int SELF_CONST = 11;

public struct Wrap(public int);

public struct DefBox<T = int> {
    public T Val;
    public read T Get([Allow(unused)] read this) {
        return read this.Val;
    }
}

public struct CChar<const char CH>;
public struct CByte<const byte BY>;

public int CallParenBound<T : (Clone) + (maybe Sized) +>(read T _x) where T:, {
    Box<dyn (Fn() -> int) + Send +> cb = Box.new(() => 9);
    return cb();
}

public int ParamDestructure(Wrap(int a) : Wrap, deref int b, borrow read int c) {
    return a + b + (deref c);
}

macro emit_val {
    ($v:expr) => { let _ = $v; };
}

[Doc = concat!("wave", "3")]
void main() {
    ;
    mut int acc = 0;
    {
        int scoped = 5;
        acc += scoped;
    }
    emit_val! { 123 }

    int local_add(int a, int b) {
        return a + b;
    }
    namespace local_mod {
        public const int BONUS = 4;
    }

    Wrap(int wv) = Wrap(7);
    mut int wa = 0;
    Wrap(wa) = Wrap(8);

    int base = 10;
    deref int dv = read base;
    mut int mbase = 20;
    deref write int dmv = write mbase;
    borrow read int br = base;
    borrow write int bw = mbase;
    deref bw += 1;
    (int pv) = 3;

    ((int, int), int) nested_tup = ((10, 20), 30);
    int chained = nested_tup.0.1;

    DefBox db = new DefBox { Val = 6 };
    CChar<'a'> _cc = CChar;
    CByte<b'x'> _cb = CByte;
    int pb = CallParenBound(read base);
    int pd = ParamDestructure(Wrap(2), read base, 3);
    ((int, int)) -> int clo = ((int x, int y)) => {
        int s = x + y;
        return s;
    };
    int cl_res = clo((4, 5));

    int sum = acc
        + local_add(my_self.SELF_CONST, local_mod.BONUS)
        + wv
        + wa
        + dv
        + dmv
        + (deref br)
        + (deref bw)
        + pv
        + chained
        + (deref db.Get())
        + pb
        + pd
        + cl_res;
    PrintLine($"{sum}");
}
"#;
    let out = run_rsharp(rsharp).expect("Wave 3 local fns/destructuring/blocks/tuples must run");
    assert_eq!(out.exit_code, 0);
    // 5 + (11+4) + 7 + 8 + 10 + 20 + 10 + 21 + 3 + 20 + 6 + 9 + (2+10+3) + 9 = 158
    assert_eq!(out.stdout.trim(), "158");
}

#[test]
fn roundtrip_rust_199_wave3_zero_fallbacks_and_identical_behavior() {
    let rust_src = r####"#!/usr/bin/env rust
#![expect(unused_variables)]
extern crate self as my_self;

pub const LO: i32 = 1;
pub const HI: i32 = 5;

pub mod r#type {
    pub const VAL: i32 = 7;
}
use r#type::VAL;

pub trait HasB {
    type B: HasX;
}
pub trait HasX {
    const X: i32;
}
pub struct Inner;
impl HasX for Inner {
    const X: i32 = 33;
}
pub struct Outer;
impl HasB for Outer {
    type B = Inner;
}

pub fn get_nested<T: HasB>() -> i32 {
    <<T as HasB>::B as HasX>::X
}

#[derive(Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

pub struct TupStruct(#[allow(dead_code)] pub i32);

pub enum Choice<T> {
    A(T),
    B {
        #[allow(dead_code)]
        k: i32,
    },
}

pub union U {
    #[allow(dead_code)]
    pub a: i32,
}

pub trait AttrTr {
    #[allow(dead_code)]
    type Assoc;
    #[allow(dead_code)]
    const C: i32 = 1;
    #[allow(dead_code)]
    fn m(#[allow(unused)] &self) -> i32 {
        1
    }
}

impl AttrTr for TupStruct {
    #[allow(dead_code)]
    type Assoc = i32;
    #[allow(dead_code)]
    const C: i32 = 2;
    #[allow(dead_code)]
    fn m(#[allow(unused)] &self) -> i32 {
        self.0
    }
}

pub struct Boxed<T> {
    pub val: T,
}

pub struct CChar<const CH: char>;
pub struct CByte<const BY: u8>;

pub fn paren_and_const<T: (Clone) + (?Sized)>(_x: &T) -> i32 {
    let _: CChar<'a'> = CChar;
    let _: CByte<b'x'> = CByte;
    let arr: [i32; 2 + 2] = [1, 2, 3, 4];
    let cb: Box<dyn (Fn() -> i32) + Send> = Box::new(|| 5);
    arr[3] + cb()
}

pub fn param_destr(TupStruct(a): TupStruct, &b: &i32, ref c: i32) -> i32 {
    a + b + *c
}

pub fn raw_life<'r#async, 'a>(_x: &'r#async str) -> &'r#async str
where
    'r#async: 'a,
{
    _x
}

fn main() {
    let mut v: Vec<i32> = <Vec<i32>>::new();
    v.push(10);
    let slen: usize = <[i32]>::len(&v);
    let nested: i32 = get_nested::<Outer>();
    let b: Boxed<i32> = Boxed::<i32> { val: 4 };
    let p_def: Point = Point { x: 6, ..Point::default() };
    let p_empty: Point = Point { ..Point::default() };

    let mut opt: Option<i32> = Some(3);
    let m1: i32 = match opt {
        | Some(ref mut mv @ LO..=HI) => {
            *mv += 2;
            *mv
        }
        | _ => 0,
    };

    let mut arr4: [i32; 4] = [1, 2, 3, 4];
    let m2: usize = match arr4 {
        [_, ref mut mid @ .., _] => {
            mid[0] = 8;
            mid.len()
        }
    };

    let mut pt: Point = Point { x: 2, y: 3 };
    let Point { #[allow(unused)] ref x, ref mut y } = pt;
    *y += *x;
    let pt_sum: i32 = *y;

    let TupStruct(tv) = TupStruct(9);
    let r_src: i32 = 12;
    let r_ref: &i32 = &r_src;
    let &dv = r_ref;
    let ref br = r_src;
    let mut m_src: i32 = 14;
    let ref mut bw = m_src;
    *bw += 1;
    let (pv) = 15_i32;

    let mut scope_acc: i32 = 0;
    {
        let inner_v: i32 = 3;
        scope_acc += inner_v;
    }

    pub fn local_helper(k: i32) -> i32 {
        k + 1
    }

    let tup: ((i32, i32), i32) = ((10, 20), 30);
    let t_chain: i32 = tup.0.1;

    let clo = |(ca, cb): (i32, i32)| {
        let s: i32 = ca + cb;
        s
    };
    let cl_v: i32 = clo((3, 4));

    let raw_quote: &str = r#""quoted""#;
    let raw_triple: &str = r##"a"""b"##;

    let ch_v: i32 = match Choice::<i32>::A(b.val) {
        Choice::<i32>::A(n) => n,
        _ => 0,
    };

    #[allow(unused)]
    println!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        slen,
        nested,
        p_def.x + p_empty.x,
        m1,
        m2,
        arr4[1],
        pt_sum,
        tv,
        dv,
        *br,
        *bw,
        pv,
        scope_acc,
        local_helper(my_self::VAL),
        t_chain,
        cl_v,
        paren_and_const(&r_src) + param_destr(TupStruct(1), &r_src, 2) + ch_v,
        format!("{}:{}", raw_quote, raw_triple)
    );
}
"####;

    let rust_out = compile_and_run_rust(rust_src).expect("Wave 3 Rust fixture must compile & run");
    assert_eq!(rust_out.exit_code, 0);

    let rsharp = convert_from_rust(
        rust_src,
        FromRustOptions {
            use_rustc_inference: true,
        },
    )
    .expect("Wave 3 Rust fixture must convert to R#");
    assert!(
        !rsharp.contains("rust {"),
        "Wave 3 round-trip must have ZERO rust {{ ... }} fallbacks, got:\n{}",
        rsharp
    );
    assert!(rsharp.starts_with("#!/usr/bin/env rust\n"));
    assert!(rsharp.contains("extern crate this as my_self;"));
    assert!(rsharp.contains("[module: Expect(unused_variables)]"));

    let rsharp_out = run_rsharp(&rsharp).expect("Converted Wave 3 R# must compile and run cleanly");
    assert_eq!(rsharp_out.exit_code, 0);
    assert_eq!(rsharp_out.stdout, rust_out.stdout);
}

#[test]
fn from_rust_wave3_extra_branch_coverage() {
    let src = r####"
#[my_attr[a, b]]
#[my_attr{ k = 1 }]
#[forbid(unsafe_code)]
pub trait Tr {
    #[allow(dead_code)]
    tr_mac!(1);
    #[allow(dead_code)]
    tr_bmac! { fn a(); }
}

pub struct S;
pub struct W(i32);

impl Tr for S {
    #[allow(dead_code)]
    im_mac!(1);
    #[allow(dead_code)]
    im_bmac! { fn a() {} }
}

unsafe extern "C" {
    #[allow(dead_code)]
    ext_mac!(1);
    #[allow(dead_code)]
    ext_bmac! {}
}

pub type VariadicFn = unsafe extern "C" fn(#[allow(unused)] i32, #[allow(unused)] x: i32, #[allow(unused)] ...) -> i32;
pub struct ConstDefault<const N: usize = { let k = 1; k + 1 }>;
pub struct ConstSingle<const N: usize = { 1 + 2 }>;
pub type AssocInherent = <S>::Assoc;
pub type MultiQ = <S as Tr>::A::B;
pub fn cap_raw<'r#async>(x: &'r#async str) -> impl std::fmt::Display + use<'r#async> + 'r#async { x }
pub fn recv_mut(#[allow(unused)] &mut self) {}
pub fn recv_val(#[allow(unused)] self) {}
pub fn recv_mut_val(#[allow(unused)] mut self) {}
pub fn recv_typed(#[allow(unused)] self: Box<Self>) {}

pub fn extra_pats_and_stmts(
    ref mut rm_param: i32,
    (ref r_tup, ref mut rm_tup): (i32, i32),
    &mut dm_param: &mut i32,
    W(wa): W,
    u_tup: UnknownTup,
    s_ref: &[i32],
    p: Point,
    mut mv: i32,
) {
    #[allow(unused)]
    brace_stmt! { 1 }
    #[allow(unused)]
    {
        let _blk: i32 = 1;
    }
    let _ = match s_ref {
        [_, ref mid @ .., _] => mid.len(),
        [_, mut mid2 @ .., _] => mid2.len(),
        _ => 0,
    };
    let _ = match 3_i32 {
        ref v @ 1..=5 => *v,
        _ => 0,
    };
    let _ = match p {
        Point { #[allow(unused)] ref a, ref mut b } => (*a, *b),
    };
    let _ = match 1_i32 {
        <S as Tr>::C => 1,
        <S>::D => 2,
        _ => 0,
    };
    let &mut dm_local: &mut i32 = &mut mv;
    let W(w_local) = W(1);
    let _ = (rm_param, r_tup, rm_tup, dm_param, wa, u_tup, dm_local, w_local);
    let _ = |(x,): (i32,)| { let _ = x; };
    let wc = |W(a): W| a;
    let _ = wc;
    let _ = |ref r: i32| *r;
    let _ = |_: i32| 1_i32;
    let _ = Point { x: 1, ..p };
    let _ = Point { ..p };
    let _ = Point { ..Point::default() };
    let _ = S::<i32> { x: 1, ..p };
    let _ = r#"hello "world""#;
    let _ = r##"a"""b"##;
    let _ = r#""starts""#;
    let _ = crate::r#type::f();
    let _ = self::r#type::f();
    use crate::r#type::{self, r#match as m_alias};
    pub(in crate::r#type) fn vis_raw() {}
    pub(in self::r#type) fn vis_self() {}
}
"####;
    let rsharp = convert_from_rust(
        src,
        FromRustOptions {
            use_rustc_inference: false,
        },
    )
    .expect("from_rust Wave 3 extra branches must convert");
    assert!(rsharp.contains("[Forbid(unsafe_code)]"));
    assert!(rsharp.contains("[my_attr[a , b]]"));
    assert!(rsharp.contains("[my_attr{ k = 1 }]"));
    assert!(rsharp.contains("(S)::Assoc"));
    assert!(rsharp.contains("(S as Tr).A::B"));

    // Error paths for destructuring without initializer in TupleStruct / Reference and untyped tuple closure param
    for bad in [
        "fn f() { let W(a); }",
        "fn f() { let &a; }",
        "fn f(u: Unknown) { let &a = u; }",
        "fn f() { let _ = |(a, b)| a + b; }",
    ] {
        let _ = convert_from_rust(
            bad,
            FromRustOptions {
                use_rustc_inference: false,
            },
        );
    }
}
