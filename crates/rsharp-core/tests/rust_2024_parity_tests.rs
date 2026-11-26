use rsharp_core::lexer::tokenize;
use rsharp_core::rustc_runner::compile_and_run_rust;
use rsharp_core::token::Keyword;
use rsharp_core::{
    FromRustOptions, PreludeMode, TranspileOptions, check_rsharp, convert_from_rust, run_rsharp,
    transpile_to_rust,
};

#[test]
fn test_new_keywords_and_literals_lexer_and_runtime() {
    for kw in ["borrow", "forall"] {
        let parsed = Keyword::parse(kw).expect("new keyword must parse");
        assert_eq!(parsed.as_str(), kw);
    }

    // Lexer errors for unterminated raw / verbatim strings
    assert!(tokenize(r#"@"unterminated"#).is_err());
    assert!(tokenize(r#""""unterminated"#).is_err());

    // Compile and run all new literal forms on rustc 1.99.1 --edition=2024
    let rsharp = r#"
        void main()
        {
            read str raw1 = @"C:\path\to\file";
            read str raw2 = @"quoted ""word""";
            read str raw3 = """contains "quotes" and #hashes#""";
            read str raw4 = """"four ""quotes"""""";
            byte bch = b'A';
            read byte[5] bs1 = "hello"u8;
            read byte[3] bs2 = @"a\b"u8;
            read byte[5] bs3 = """a"b"c"""u8;
            read core.ffi.CStr cs1 = c"hello";
            read core.ffi.CStr cs2 = c@"raw\c";
            read core.ffi.CStr cs3 = c"""raw "c"""";

            PrintLine($"{raw1}|{raw2}|{raw3}|{raw4}|{bch}|{bs1.len()}|{bs2.len()}|{bs3.len()}|{cs1.to_bytes().len()}|{cs2.to_bytes().len()}|{cs3.to_bytes().len()}");
        }
    "#;
    let out = run_rsharp(rsharp).expect("all Rust 2024 literal forms must compile and run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.stdout.trim(),
        r#"C:\path\to\file|quoted "word"|contains "quotes" and #hashes#|four ""quotes""|65|5|3|5|5|5|7"#
    );
}

#[test]
fn semantic_let_chains_and_while_let_chains() {
    let rsharp = r#"
        public int CheckBoth(int? a, int? b)
        {
            if (a is Some(int x) && b is Some(int y) && x > y)
            {
                return x - y;
            }
            else if (a is Some(int x) && x > 0)
            {
                return x;
            }
            return 0;
        }

        void main()
        {
            mut List<int> items = [10, 8, 5, 0, 3];
            mut int sum = 0;
            mut nuint idx = 0;
            while (items.get(idx) is Some(read int v) && (deref v) > 0)
            {
                sum += deref v;
                idx += 1;
            }
            PrintLine($"{CheckBoth(Some(9), Some(4))},{CheckBoth(Some(2), Some(8))},{CheckBoth(None, Some(1))},{sum}");
        }
    "#;
    let out = run_rsharp(rsharp).expect("let chains and while-let chains must compile and run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "5,2,0,23");
}

#[test]
fn semantic_raw_ref_exprs_const_blocks_and_async_closures() {
    let rsharp = r#"
        [Repr(packed)]
        public struct Packed
        {
            public byte Tag;
            public int Value;
        }

        public impl std.ops.AsyncFn(int) -> int MakeAdder(int base)
        {
            return async take (int x) : Task<int> => x + base;
        }

        void main()
        {
            mut Packed pk = new Packed { Tag = 1, Value = 77 };
            read ptr<int> rp = read ptr pk.Value;
            write ptr<int> wp = write ptr pk.Value;
            mut int readBack = 0;
            unsafe
            {
                readBack = rp.read_unaligned();
                wp.write_unaligned(99);
            }
            int updated = unsafe { return rp.read_unaligned(); };

            Option<string>[3] emptyArr = [const(None); 3];
            int cval = const {
                int a = 6;
                return a * 7;
            };

            mut int asyncRes = 0;
            rust {
                use std::future::Future;
                use std::pin::pin;
                use std::task::{Context, Poll, Waker};
                let afn = MakeAdder(10);
                let mut cx = Context::from_waker(Waker::noop());
                let mut fut = pin!(afn(5));
                if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                    asyncRes = v;
                }
            }

            PrintLine($"{readBack},{updated},{emptyArr.len()},{cval},{asyncRes}");
        }
    "#;
    let out = run_rsharp(rsharp).expect("raw ptr exprs, const blocks, and async closures must run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "77,99,3,42,15");
}

#[test]
fn semantic_ufcs_shorthand_labeled_loop_expr_and_destructuring() {
    let rsharp = r#"
        public trait Speaker
        {
            read str Speak(read this);
        }

        public trait LoudSpeaker
        {
            read str Speak(read this);
        }

        public struct Dog
        {
            public int Age;
        }

        impl Speaker for Dog
        {
            read str Speak(read this) { return "woof"; }
        }

        impl LoudSpeaker for Dog
        {
            read str Speak(read this) { return "WOOF!"; }
        }

        public int SumDestructured((int a, int b), Dog { int Age; }, [int x, int y])
        {
            return a + b + Age + x + y;
        }

        public (I as Iterator).Item? FirstItem<I>(mut I it) where I : Iterator
        {
            return it.next();
        }

        void main()
        {
            int Age = 4;
            Dog dog = new Dog { Age };
            read str s1 = (Dog as Speaker).Speak(read dog);
            read str s2 = (Dog as LoudSpeaker).Speak(read dog);

            int looped = outer: loop
            {
                break outer 123;
            };

            Dog { int Age } = dog;
            [int first, int second] = [10, 20];
            ((int p1, int p2), int p3) = ((1, 2), 3);
            int total = SumDestructured((p1, p2), new Dog { Age }, [first, second]);

            List<int> nums = [7, 8, 9];
            List<int> collected = nums.iter().copied().collect<List<_>>();
            int firstVal = FirstItem(collected.into_iter()) ?? 0;

            PrintLine($"{s1}|{s2}|{looped}|{total + p3}|{firstVal}");
        }
    "#;
    let out =
        run_rsharp(rsharp).expect("UFCS, shorthand, labeled loop expr, destructuring must run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "woof|WOOF!|123|40|7");
}

#[test]
fn semantic_patterns_slice_bind_at_borrow_deref_ranges_const() {
    let rsharp = r#"
        const int TARGET = 50;

        public int AnalyzeSlice(read slice<int> s)
        {
            return match (s)
            {
                [] => 0,
                [read int only] => deref only,
                [read int first, .. read slice<int> mid, read int last] => (deref first) + (deref last) + (mid.len() as int),
            };
        }

        public int DerefMatch(read int r)
        {
            return match (r)
            {
                deref int val => val * 2,
            };
        }

        public read<static> str Grade(int score)
        {
            return match (score)
            {
                const(TARGET - 1) => "almost",
                ..=59 => "F",
                60..=89 as int mid => {
                    _ = mid;
                    return "B";
                }
                90.. => "A",
            };
        }

        void main()
        {
            int[0] e = [];
            int[1] o = [9];
            int[4] m = [2, 10, 20, 5];

            mut Option<string> owned = Some("hello".ToString());
            if (owned is Some(borrow read string borrowed))
            {
                PrintLine($"borrowed={borrowed.len()}");
            }
            if (owned is Some(borrow write string mref))
            {
                mref.push('!');
            }
            int finalLen = owned.unwrap().len() as int;

            (int, int, int, int) quad = (1, 2, 3, 4);
            int ends = match (quad)
            {
                (int a, .., int z) => a + z,
            };

            [Allow(unused)]
            int annotatedStmt = 1;
            int armed = match (annotatedStmt)
            {
                [Allow(unused)]
                1 => 10,
                _ => 0,
            };

            int ten = 5;
            int doubled = DerefMatch(read ten);
            PrintLine($"{AnalyzeSlice(read slice e[..])},{AnalyzeSlice(read slice o[..])},{AnalyzeSlice(read slice m[..])},{Grade(49)},{Grade(50)},{Grade(75)},{Grade(95)},{finalLen},{ends},{armed},{doubled}");
        }
    "#;
    let out = run_rsharp(rsharp).expect("all new pattern forms must compile and run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.stdout.replace("\r\n", "\n").trim(),
        "borrowed=5\n0,9,9,almost,F,B,A,6,5,10,10"
    );
}

#[test]
fn semantic_gats_hrtbs_maybe_sized_precise_capture_defaults_and_fn_ptrs() {
    let rsharp = r#"
        using std.fmt.{this, Display};

        public trait LendingIter
        {
            type Item<life a> where Self : life a;
            Option<Self.Item<life a>> Next<life a>(write<a> this);
        }

        public struct SliceLender
        {
            public List<int> Data;
            public nuint Pos;
        }

        impl LendingIter for SliceLender
        {
            type Item<life a> = read<a> int where Self : life a;
            Option<read<a> int> Next<life a>(write<a> this)
            {
                if (this.Pos >= this.Data.len())
                {
                    return None;
                }
                nuint i = this.Pos;
                this.Pos += 1;
                return Some(read this.Data[i]);
            }
        }

        public struct Wrapper<T : maybe Sized = int>(public T);

        public struct FixedBuf<T = byte, const nuint N = 4>
        {
            public T[N] Buf;
        }

        public nuint UnsizedLen<T>(read T val) where T : maybe Sized, std.fmt.Debug
        {
            _ = val;
            return 1;
        }

        public nuint ApplyHrtb<F>(F f, read str s)
            where F : forall<life a> Fn(read<a> str) -> read<a> str
        {
            return f(s).len();
        }

        public impl Display + capture<life a> FirstOnly<life a, life b>(read<a> str x, read<b> str y)
        {
            _ = y;
            return x;
        }

        public read<_> str ElidedLife(read<_> str s)
        {
            return s;
        }

        public alias BoundedPair<T> where T : Clone = (T, T);

        public unsafe extern "C" int CAdd(int a, int b)
        {
            return a + b;
        }

        void main()
        {
            List<int> initData = [11, 22];
            mut SliceLender lender = new SliceLender { Data = initData, Pos = 0 };
            int g1 = match (lender.Next()) { Some(deref int v) => v, None => 0 };
            int g2 = match (lender.Next()) { Some(deref int v) => v, None => 0 };

            FixedBuf fb = new FixedBuf { Buf = [1, 2, 3, 4] };
            Wrapper w = Wrapper(5);
            nuint ulen = UnsizedLen(read "abc");
            nuint hlen = ApplyHrtb((read str s) : read str => s, "rust2024");

            read str first = "ok";
            string captured = $"{FirstOnly(first, "ignored")}";
            read str elided = ElidedLife("hi");
            BoundedPair<int> bp = (1, 2);

            read (dyn Display + Send) dynObj = read w.0;
            unsafe extern "C" (int, int) -> int cfn = CAdd;
            int csum = unsafe { return cfn(20, 22); };
            forall<life a> (read<a> str) -> read<a> str hfn = ElidedLife;
            read str hres = hfn("zz");

            PrintLine($"{g1},{g2},{fb.Buf.len()},{w.0},{ulen},{hlen},{captured},{elided},{bp.0 + bp.1},{dynObj},{csum},{hres}");
        }
    "#;
    let out = run_rsharp(rsharp).expect("GATs, HRTBs, maybe Sized, capture<>, defaults must run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "11,22,4,5,1,8,ok,hi,3,5,42,zz");
}

#[test]
fn semantic_rust_2024_unsafe_attrs_extern_crate_visibility_and_unsafe_extern() {
    let lib_rsharp = r#"
        extern crate core as core_alias;

        namespace net
        {
            public(in crate.net) int InternalNetHelper()
            {
                return 7;
            }

            public(this) int SelfOnly()
            {
                return 3;
            }

            public int Expose()
            {
                return InternalNetHelper() + SelfOnly();
            }
        }

        [unsafe NoMangle]
        public extern "C" int exported_add([Allow(unused)] int a, int b)
        {
            return a + b;
        }

        [unsafe ExportName = "custom_mul"]
        public extern "C" int my_mul(int a, int b)
        {
            return a * b;
        }

        unsafe extern "C"
        {
            public safe int external_safe(int x);
            public unsafe int external_variadic(read ptr<sbyte> fmt, ...);
            public static int EXTERNAL_ERRNO;
        }

        public struct ConstExprGrid<T, const nuint N>
        {
            public T[N] Data;
        }

        public ConstExprGrid<int, { 2 + 2 }> MakeGrid()
        {
            return new ConstExprGrid { Data = [1, 2, 3, 4] };
        }
    "#;
    check_rsharp(lib_rsharp).expect("Rust 2024 unsafe attrs, unsafe extern, extern crate, pub(in) must pass rustc --edition=2024");

    let rs = transpile_to_rust(
        lib_rsharp,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .unwrap();
    assert!(rs.contains("#[unsafe(no_mangle)]"));
    assert!(rs.contains("#[unsafe(export_name = \"custom_mul\")]"));
    assert!(rs.contains("pub safe fn external_safe"));
    assert!(rs.contains("..."));
    assert!(rs.contains("pub static EXTERNAL_ERRNO: i32;"));
    assert!(rs.contains("extern crate core as core_alias;"));
    assert!(rs.contains("pub(in crate::net) fn InternalNetHelper"));
    assert!(rs.contains("pub(self) fn SelfOnly"));
}

#[test]
fn roundtrip_rust_2024_full_parity_with_zero_rust_fallbacks() {
    let rust_src = r##"
extern crate core as c_core;

use std::fmt::{self, Display as FmtDisplay};

#[unsafe(no_mangle)]
pub extern "C" fn exported_fn(#[allow(unused)] a: i32, b: i32) -> i32 {
    a + b
}

pub mod sub {
    pub(in crate::sub) fn helper() -> i32 {
        4
    }
    pub(self) fn local_only() -> i32 {
        6
    }
    pub fn total() -> i32 {
        helper() + local_only()
    }
}

pub trait Lender {
    type Item<'a> where Self: 'a;
    fn peek<'a>(&'a self) -> Self::Item<'a>;
}

pub struct BoxedVal<T = i32, const N: usize = 2> {
    pub items: [T; N],
}

impl<T, const N: usize> Lender for BoxedVal<T, N> {
    type Item<'a> = &'a [T] where Self: 'a;
    fn peek<'a>(&'a self) -> &'a [T] {
        &self.items
    }
}

pub fn apply_hrtb<F>(f: F, text: &str) -> usize
where
    F: for<'a> Fn(&'a str) -> &'a str,
{
    f(text).len()
}

pub fn keep_first<'a, 'b>(x: &'a str, _y: &'b str) -> impl FmtDisplay + use<'a> {
    x
}

pub fn slice_check(s: &[i32]) -> i32 {
    match s {
        [] => 0,
        [only] => *only,
        [first, mid @ .., last] => *first + *last + (mid.len() as i32),
    }
}

pub fn let_chain_demo(a: Option<i32>, b: Option<i32>) -> i32 {
    if let Some(x) = a && let Some(y) = b && x > y {
        x - y
    } else {
        0
    }
}

pub fn multi_stmt_if_expr(flag: bool) -> i32 {
    let res: i32 = if flag {
        let k: i32 = 10;
        k + 5
    } else {
        2
    };
    res
}

fn main() {
    let raw: &str = r#"hello "raw""#;
    let bch: u8 = b'Z';
    let bstr: &[u8; 4] = b"byte";
    let cstr: &'static core::ffi::CStr = c"cstr";
    let arr: [Option<String>; 2] = [const { None }; 2];

    let mut target: i32 = 12;
    let rp: *const i32 = &raw const target;
    let wp: *mut i32 = &raw mut target;
    unsafe {
        *wp = *rp + 8;
    }

    let bv: BoxedVal<i32, 3> = BoxedVal { items: [1, 2, 3] };
    let sc: i32 = slice_check(<BoxedVal<i32, 3> as Lender>::peek(&bv));
    let lc: i32 = let_chain_demo(Some(10), Some(3));
    let ms: i32 = multi_stmt_if_expr(true);
    let hl: usize = apply_hrtb(|s: &str| -> &str { s }, "hello");
    let kf = keep_first("ok", "no");
    let _ = fmt::Error;

    println!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        raw,
        bch,
        bstr.len(),
        cstr.to_bytes().len(),
        arr.len(),
        target,
        sc,
        lc,
        ms,
        hl,
        kf,
        sub::total() + exported_fn(1, 2)
    );
}
"##;

    let orig_out = compile_and_run_rust(rust_src).expect("original Rust 2024 must run");

    let rsharp = convert_from_rust(
        rust_src,
        FromRustOptions {
            use_rustc_inference: true,
        },
    )
    .expect("Rust 2024 -> R# must succeed");

    // Verify ZERO `rust {` escape-hatch fallbacks were needed!
    assert!(
        !rsharp.contains("rust {"),
        "expected zero rust {{}} fallbacks, got:\n{}",
        rsharp
    );
    assert!(rsharp.contains("[unsafe NoMangle]"));
    assert!(rsharp.contains("extern crate core as c_core;"));
    assert!(rsharp.contains("using std.fmt.{this, FmtDisplay = Display};"));
    assert!(rsharp.contains("forall<life a> Fn(read<a> str) -> read<a> str"));
    assert!(rsharp.contains("capture<life a>"));
    assert!(rsharp.contains("read ptr target"));
    assert!(rsharp.contains("write ptr target"));
    assert!(rsharp.contains("const(None)"));

    let rt_out = run_rsharp(&rsharp).expect("round-tripped R# must compile and run");
    assert_eq!(rt_out.exit_code, 0);
    assert_eq!(rt_out.stdout, orig_out.stdout);
}

#[test]
fn from_rust_covers_all_rust_2024_syntax_branches() {
    let rust_src = r##"
extern crate alloc;

#[unsafe(export_name = "my_export")]
#[unsafe(link_section = ".custom")]
pub extern "C" fn attr_fns(#[allow(unused)] (a, b): (i32, i32)) -> i32 {
    a + b
}

unsafe extern "C" {
    pub safe fn safe_ext(x: f64) -> f64;
    pub unsafe fn variadic_ext(fmt: *const i8, ...) -> i32;
    pub static mut EXT_MUT: i32;
    pub safe static SAFE_STAT: i32;
    pub unsafe static mut UNSAFE_STAT: i32;
}

pub type CheckedPair<T> where T: Clone = (T, T);

pub fn advanced_types<T: ?Sized, I: Iterator, F>(
    _r: &(dyn std::fmt::Display + Send),
    _cb: for<'a> unsafe extern "C" fn(&'a str) -> &'a str,
    _it: <I as Iterator>::Item,
    _f: F,
) where
    for<'a> F: Fn(&'a str) -> &'a str,
{
    let _: Vec<_> = vec![1, 2];
}

pub fn patterns_and_exprs(flag: bool, n: i32, opt: Option<String>, r: &i32) -> i32 {
    #[allow(unused)]
    let raw_simple = r"plain_raw";
    let raw_bs = br#"raw "bytes""#;
    let raw_cs = cr#"raw "cstr""#;
    let raw_c_plain = cr"plain_c";
    let norm_b = b"abc";
    let _ = (raw_simple, raw_bs, raw_cs, raw_c_plain, norm_b);

    let c_blk: i32 = const {
        let x: i32 = 2;
        x + 3
    };
    let lp: i32 = 'l: loop {
        break 'l c_blk;
    };
    let _ac = async move |x: i32| -> i32 { x + 1 };

    let branched: i32 = if flag {
        let t: i32 = 1;
        t + 1
    } else if n > 0 {
        let u: i32 = 2;
        u + 1
    } else {
        0
    };

    let matched_ref: i32 = match r {
        &v => v,
    };

    if let Some(ref s) = opt {
        let _ = s.len();
    }
    let mut opt_m = opt;
    if let Some(ref mut ms) = opt_m {
        ms.push('x');
    }

    let bound: i32 = match n {
        #[allow(unused)]
        v @ 1..=5 => v,
        ..=0 => -1,
        10.. => 10,
        _ => 0,
    };

    let (first, .., last) = (1, 2, 3, 4);
    let [a, b] = [5, 6];
    let _ = (lp, branched, matched_ref, bound, first, last, a, b);
    0
}
"##;

    let rsharp = convert_from_rust(
        rust_src,
        FromRustOptions {
            use_rustc_inference: false,
        },
    )
    .expect("all Rust 2024 from_rust branches should convert cleanly");

    assert!(rsharp.contains("[unsafe ExportName = \"my_export\"]"));
    assert!(rsharp.contains("[unsafe LinkSection = \".custom\"]"));
    assert!(rsharp.contains("public safe double safe_ext"));
    assert!(rsharp.contains("..."));
    assert!(rsharp.contains("public static mut int EXT_MUT;"));
    assert!(rsharp.contains("public safe static int SAFE_STAT;"));
    assert!(rsharp.contains("public unsafe static mut int UNSAFE_STAT;"));
    assert!(rsharp.contains("maybe Sized"));
    assert!(rsharp.contains("forall<life a> unsafe extern \"C\""));
    assert!(rsharp.contains("(I as Iterator).Item"));
    assert!(rsharp.contains("async take (int x) : Task<int>"));
    assert!(rsharp.contains("borrow read"));
    assert!(rsharp.contains("borrow write"));
    assert!(rsharp.contains("deref v"));
    assert!(rsharp.contains("1..=5 as int v"));
    assert!(rsharp.contains("if (flag) {\n"), "got:\n{rsharp}");
    assert!(!rsharp.contains("__if_val:"), "got:\n{rsharp}");
    assert!(rsharp.contains("extern crate alloc;"));
}
