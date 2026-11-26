use rsharp_core::rustc_runner::compile_and_run_rust;
use rsharp_core::{
    FromRustOptions, PreludeMode, RSharpError, TranspileOptions, convert_from_rust, run_rsharp,
    transpile_to_rust,
};

#[test]
fn conversion_uses_rider_csharp_block_layout() {
    let rust = r#"
struct Number { value: i32 }
impl Number {
    fn get(&self) -> i32 {
        if self.value > 1 {
            self.value
        } else if self.value < 0 {
            0
        } else {
            1
        }
    }
}
fn main() {
    let number = Number { value: 42 };
    println!("{{{}}}", number.get());
    if false {}
}
"#;
    let rsharp = convert_from_rust(rust, FromRustOptions::default()).unwrap();
    assert_eq!(
        rsharp,
        r#"struct Number
{
    int value;
}
impl Number
{
    int get(read this)
    {
        if ((this.value > 1))
        {
            return this.value;
        }
        else if ((this.value < 0))
        {
            return 0;
        }
        else
        {
            return 1;
        }
    }
}
void main()
{
    Number number = new Number { value = 42 };
    println!("{{{}}}" , number . get ());
    if (false)
    {
    }
}
"#
    );
    assert_eq!(run_rsharp(&rsharp).unwrap().stdout.trim(), "{42}");
}

#[test]
fn roundtrip_rust_to_rsharp_to_rust_preserves_behavior() {
    let rust_src = r#"
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};

/// A 2D point struct.
#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

pub struct Wrapper(pub i32);
pub struct Empty;

pub trait Measurable {
    fn magnitude_sq(&self) -> i32;
    fn label(&self) -> &'static str {
        "pt"
    }
}

impl Measurable for Point {
    fn magnitude_sq(&self) -> i32 {
        self.x * self.x + self.y * self.y
    }
}

impl Point {
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub fn translate(&mut self, dx: i32, dy: i32) {
        self.x += dx;
        self.y += dy;
    }

    pub fn into_sum(self) -> i32 {
        self.x + self.y
    }
}

pub enum Action {
    Quit,
    Add(i32),
    Move { dx: i32, dy: i32 },
}

pub type Score = i64;
pub const BASE: i32 = 10;
static FACTOR: i32 = 2;

pub fn longest<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.len() >= b.len() {
        a
    } else {
        b
    }
}

pub fn run_action(act: Action) -> i32 {
    match act {
        Action::Quit => 0,
        Action::Add(n) if n > 0 => n * 2,
        Action::Add(n) => n,
        Action::Move { dx, dy } => dx + dy,
    }
}

pub fn unwrap_or_neg(opt: Option<i32>) -> i32 {
    let Some(v) = opt else {
        return -1;
    };
    v
}

pub fn parse_plus_one(s: &str) -> Result<i32, std::num::ParseIntError> {
    let n: i32 = s.parse::<i32>()?;
    Ok(n + 1)
}

fn main() {
    let mut p = Point::new(3, 4);
    p.translate(1, 2);
    let p2 = Point { x: 10, ..p };
    let mag = p2.magnitude_sq();
    let label = p2.label();

    let (a, b): (i32, i32) = (5, 6);
    let mut sum = a + b + BASE * FACTOR;
    let sum = sum + 1; // shadowing!

    let mut q: VecDeque<i32> = VecDeque::new();
    q.push_back(7);
    let mut map: HashMap<i32, i32> = HashMap::new();
    map.insert(1, 9);

    let nums = vec![1, 2, 3];
    let mut total: i32 = 0;
    for x in &nums {
        total += *x;
    }

    let mut countdown = 3;
    while countdown > 0 {
        countdown -= 1;
    }

    let opt = Some(42);
    let mut extracted = 0;
    if let Some(v) = opt {
        extracted = v;
    }

    let read = 11; // Rust variable using R# keyword name!
    let r_act = run_action(Action::Move { dx: 3, dy: 4 });
    let r_unw = unwrap_or_neg(Some(9));
    let r_parse = parse_plus_one("41").unwrap();
    let best = longest("hi", "world");
    let flag = if sum > 0 { 1 } else { 0 };
    let _ = Empty;
    let w = Wrapper(8);

    println!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{}",
        p.into_sum(),
        mag,
        label,
        sum,
        total,
        extracted,
        read,
        r_act,
        r_unw,
        r_parse,
        best,
        flag,
        w.0 + q.len() as i32 + map.len() as i32
    );
}
"#;

    let original_out = compile_and_run_rust(rust_src).expect("original Rust must run");

    let rsharp = convert_from_rust(rust_src, FromRustOptions::default())
        .expect("Rust -> R# conversion must succeed");
    assert!(rsharp.contains("public trait Measurable"));
    assert!(rsharp.contains("read this"));
    assert!(rsharp.contains("write this"));
    assert!(rsharp.contains("take this"));
    assert!(rsharp.contains("read<a> str"));
    assert!(rsharp.contains("shadow int sum"));
    assert!(rsharp.contains("try "));
    assert!(rsharp.contains("require ("));
    assert!(rsharp.contains("@read"));

    let roundtrip_out = run_rsharp(&rsharp).expect("converted R# must compile and run");
    assert_eq!(roundtrip_out.exit_code, 0);
    assert_eq!(roundtrip_out.stdout, original_out.stdout);
}

#[test]
fn roundtrip_rsharp_to_rust_to_rsharp_to_rust() {
    let rsharp_1 = r#"
        [Derive(Debug, Clone)]
        public struct Counter
        {
            public int Value;

            public static Self New(int start)
            {
                return new Self { Value = start };
            }

            public void Inc(write this, int by)
            {
                this.Value += by;
            }

            public int Current(read this)
            {
                return this.Value;
            }
        }

        public int SumMatrix(int[2, 2] m)
        {
            return m[0, 0] + m[0, 1] + m[1, 0] + m[1, 1];
        }

        void main()
        {
            mut Counter c = Counter.New(5);
            c.Inc(7);
            int[2, 2] mat = [[1, 2], [3, 4]];
            int s = SumMatrix(mat);
            println!("{},{}", c.Current(), s);
        }
    "#;

    let rust_1 = transpile_to_rust(
        rsharp_1,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("R# -> Rust #1");
    let out_1 = compile_and_run_rust(&rust_1).expect("Rust #1 runs");

    let rsharp_2 =
        convert_from_rust(&rust_1, FromRustOptions::default()).expect("Rust #1 -> R# #2");
    assert!(rsharp_2.contains("int[2, 2]"));

    let rust_2 = transpile_to_rust(
        &rsharp_2,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("R# #2 -> Rust #2");
    let out_2 = compile_and_run_rust(&rust_2).expect("Rust #2 runs");

    assert_eq!(out_1.stdout, out_2.stdout);
    assert_eq!(out_1.stdout.trim(), "12,10");
}

#[test]
fn from_rust_uses_rustc_type_probe_for_complex_inferred_types() {
    // Iterator `.collect()` / chained expression where local inference returns None,
    // triggering `rustc` type probing!
    let rust_src = r#"
        fn main() {
            let v: Vec<i32> = vec![1, 2, 3, 4];
            let doubled = v.iter().map(|x| x * 2).sum::<i32>();
            println!("{}", doubled);
        }
    "#;
    let rsharp = convert_from_rust(
        rust_src,
        FromRustOptions {
            use_rustc_inference: true,
        },
    )
    .expect("rustc type probe should resolve `doubled` as `int`");
    assert!(rsharp.contains("int doubled ="), "got:\n{}", rsharp);
}

#[test]
fn from_rust_reports_unresolved_type_when_unresolvable() {
    let rust_src = r#"
        fn demo() {
            let x = external_unknown_fn();
            let _ = x;
        }
    "#;
    let err_no_probe = convert_from_rust(
        rust_src,
        FromRustOptions {
            use_rustc_inference: false,
        },
    )
    .expect_err("must fail without type info");
    assert!(matches!(err_no_probe, RSharpError::UnresolvedType { .. }));

    let err_with_probe = convert_from_rust(
        rust_src,
        FromRustOptions {
            use_rustc_inference: true,
        },
    )
    .expect_err("must fail when external fn cannot be resolved by rustc");
    assert!(matches!(err_with_probe, RSharpError::UnresolvedType { .. }));
}

#[test]
fn from_rust_covers_additional_rust_constructs() {
    let rust_src = r#"
        use std::io as io;
        use std::collections::*;

        pub mod inner {
            pub union Bits {
                pub i: i32,
                pub f: f32,
            }
            pub const fn sq(x: i32) -> i32 {
                x * x
            }
        }

        pub unsafe trait Raw {
            type Out: Clone = i32;
            const TAG: usize = 1;
            fn get(&self) -> Self::Out;
        }

        unsafe impl Raw for i32 {
            type Out = i32;
            const TAG: usize = 2;
            fn get(&self) -> i32 {
                *self
            }
        }

        pub async fn fetch(x: i32) -> i32 {
            let fut = async move { x + 1 };
            fut.await
        }

        pub async fn ping() {}

        pub fn misc(cb: fn(i32) -> i32, p: *const i32, mp: *mut i32, d: Box<dyn std::any::Any>) -> ! {
            let _ = (cb, p, mp, d);
            let f = |v: i32| -> i32 { v + 1 };
            let _ = f(1);
            unsafe {
                let _ = 1;
            }
            'outer: loop {
                continue 'outer;
            }
        }

        extern "C" {
            fn abs(x: i32) -> i32;
        }

        macro_rules! id {
            ($e:expr) => { $e };
        }
    "#;

    let rsharp = convert_from_rust(
        rust_src,
        FromRustOptions {
            use_rustc_inference: false,
        },
    )
    .expect("should convert all advanced Rust constructs");
    assert!(rsharp.contains("using io = std.io;"));
    assert!(rsharp.contains("using std.collections.*;"));
    assert!(rsharp.contains("public union Bits"));
    assert!(rsharp.contains("public unsafe trait Raw"));
    assert!(rsharp.contains("public async Task<int> fetch"));
    assert!(rsharp.contains("public async Task ping"));
    assert!(rsharp.contains("read ptr<int>"));
    assert!(rsharp.contains("write ptr<int>"));
    assert!(rsharp.contains("dynamic"));
    assert!(rsharp.contains("never misc"));
    assert!(rsharp.contains("macro id"));
    assert!(rsharp.contains("extern \"C\""));
}

#[test]
fn from_rust_comprehensive_syntax_and_inference_coverage() {
    let rust_src = r#"
#[allow(dead_code)]
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

#[allow(dead_code)]
mod external_sub;

#[test]
#[inline(always)]
#[inline(never)]
#[inline(custom_hint)]
#[warn(unused)]
#[deny(missing_docs)]
#[repr(C)]
#[no_mangle]
#[must_use]
#[deprecated]
#[should_panic]
#[ignore]
#[non_exhaustive]
#[cold]
#[track_caller]
#[cfg(test)]
#[cfg_attr(test, allow(dead_code))]
#[proc_macro]
#[proc_macro_derive(MyDerive)]
#[proc_macro_attribute]
#[custom_flag]
#[custom_kv = "hello"]
pub(crate) fn attr_showcase() {}

pub(super) struct GenericContainer<'a, T: Clone + 'static, const N: usize>
where
    T: Copy + 'a,
    'a: 'static,
{
    #[allow(dead_code)]
    pub data: &'a [T],
    pub fixed: [T; N],
    pub opt_ref: Option<&'a T>,
    pub cb: Box<dyn Fn(i32) -> i32>,
    pub raw_slice: Box<[u8]>,
}

#[allow(dead_code)]
pub enum DetailedEnum {
    #[allow(dead_code)]
    First = 1,
    Second(i8, u8, i16, u16, u32, u64, i128, u128, isize, usize, f32, f64),
}

#[allow(dead_code)]
pub union DetailedUnion {
    pub a: i32,
}

#[allow(dead_code)]
pub trait SuperTrait: Clone + Copy {
    type Bare;
    const BARE_CONST: i32;
    #[inline]
    fn with_self_box(self: Box<Self>) -> Self::Bare;
    fn with_mut_life<'a>(&'a mut self);
    fn returns_impl(&self) -> impl Iterator<Item = i32>;
}

#[allow(dead_code)]
impl DetailedUnion {
    #[inline]
    pub unsafe extern "C" fn raw_op(x: i32) -> i32 {
        return x;
    }
}

#[allow(dead_code)]
pub type AliasT<T> = BTreeMap<T, BTreeSet<BinaryHeap<T>>>;

#[allow(dead_code)]
pub const CVAL: i32 = 1;

#[allow(dead_code)]
pub static mut SVAL: i32 = 2;

extern "C" {
    static EXT_SYM: i32;
}

extern crate core;

#[allow(dead_code)]
pub fn expressions_and_inference(slice_arg: &[i32], mut_slice: &mut [i32], flag: bool) -> i32 {
    const LOCAL_K: i32 = 3;
    let f = 3.14f32;
    let d = 2.71;
    let ch = 'z';
    let b = true;
    let paren = (1 + 2);
    let casted = paren as i64;
    let r = &paren;
    let mut mv = 10;
    let mr = &mut mv;
    let derefed = *r;
    let neg = -derefed;
    let notb = !b;
    let cmp = paren == 3 && (paren != 0 || paren < 5 || paren <= 5 || paren > 1 || paren >= 1);
    let arith = ((10 - 2) / 2 % 3) ^ (1 & 3 | 4) << 1 >> 1;
    let mut acc = arith;
    acc -= 1;
    acc *= 2;
    acc /= 1;
    acc %= 7;
    acc ^= 1;
    acc &= 3;
    acc |= 2;
    acc <<= 1;
    acc >>= 1;
    let pair = (1, 2);
    let (pa, pb) = (10, 20);
    let arr = [1, 2, 3];
    let rep = [0; 4];
    let single = (99,);
    let s1 = String::from("a");
    let s2 = String::new();
    let s3 = format!("v={}", acc);
    let s4 = "hi".to_string();
    let s5 = s4.clone();
    let ln = s4.len();
    let emp = s4.is_empty();
    let bx = Box::new(5);
    let uninit: i32;
    uninit = 1;

    let rng1 = 0..5;
    let rng2 = 0..=5;
    let _ = (f, d, ch, casted, mr, neg, notb, cmp, pair, pa, pb, rep, single, s1, s2, s3, s5, ln, emp, bx, uninit, rng1, rng2, LOCAL_K);

    for i in 0..3 {
        if i == 1 {
            continue;
        }
        if i == 2 {
            break;
        }
    }
    let mut mnums = vec![1, 2];
    for x in &mut mnums {
        *x += 1;
    }
    for x in slice_arg {
        let _ = x;
    }
    for x in mut_slice {
        *x += 1;
    }
    for x in arr {
        let _ = x;
    }

    let mut wopt = Some(1);
    while let Some(v) = wopt {
        let _ = v;
        wopt = None;
    }

    let m_expr = match acc {
        0 | 1 => 10,
        2..=5 => 20,
        _ => 30,
    };

    match (acc, m_expr) {
        (0, _) => {
            let _ = 1;
        }
        _ => (),
    }

    let lbl_val: i32 = 'blk: {
        break 'blk 5;
    };
    let blk_single: i32 = { 7 };
    let blk_multi: i32 = {
        let z = 1;
        z + 2
    };
    let unsf_expr: i32 = unsafe { 4 };
    let if_multi: i32 = if flag {
        let k = 1;
        k + 1
    } else {
        2
    };
    let _ = (lbl_val, blk_single, blk_multi, unsf_expr, if_multi);

    if flag {
        return 1;
    } else if acc > 0 {
        return 2;
    } else {
        return 3;
    }
}

pub fn tail_match_and_block(x: i32) -> i32 {
    match x {
        0 => {
            let a = 1;
            a
        }
        _ => x,
    }
}
"#;

    let rsharp = convert_from_rust(
        rust_src,
        FromRustOptions {
            use_rustc_inference: false,
        },
    )
    .expect("comprehensive Rust->R# conversion should succeed");
    assert!(rsharp.contains("internal void attr_showcase"));
    assert!(rsharp.contains("public(super) struct GenericContainer"));
    assert!(rsharp.contains("public trait SuperTrait : Clone, Copy"));
    assert!(rsharp.contains("unsafe extern \"C\" static int raw_op"));
    assert!(rsharp.contains("SortedDictionary"));
    assert!(rsharp.contains("SortedSet"));
    assert!(rsharp.contains("PriorityQueue"));

    // Separate valid compilable snippet to exercise `rustc` block-body & nested-paren closure probes
    let probe_src = r#"
        pub fn closure_block_probe() -> i32 {
            let v: Vec<i32> = vec![1, 2, 3];
            let s = v.iter().map(|n| { *n + 1 }).sum::<i32>();
            let t = v.iter().map(|k| (*k) + 2).sum::<i32>();
            let u = v.iter().fold(0i32, |acc: i32, x: &i32| acc + *x);
            let w: i32 = [1, 2].iter().map(|m| *m).sum();
            let q = [1, 2].iter().filter(|p| **p > 0).count();
            s + t + u + w + (q as i32)
        }
    "#;
    let probed = convert_from_rust(probe_src, FromRustOptions::default())
        .expect("block-body and paren closure probe should succeed");
    assert!(probed.contains("read int n"));
    assert!(probed.contains("read int k"));
    assert!(probed.contains("nuint q"));

    // Additional from_rust syntax branches: void return, labeled break/while/for, macro items inside trait/impl,
    // struct patterns with field: pat and rest, match guards in expression position, etc.
    let extra_rust = r#"
        pub fn extra_stmts(opt: Option<i32>, val: i32) {
            'w: while val < 0 {
                break 'w;
            }
            'f: for i in 0..2 {
                if i == 0 { continue 'f; }
            }
            'l: loop {
                break 'l;
            }
            if let Some(_) = opt {
                return;
            }
            let _ = match val {
                n if n > 0 => n,
                _ => 0,
            };
        }

        pub struct Pair { pub x: i32, pub y: i32 }
        pub fn match_struct(p: Pair) -> i32 {
            let Pair { x: a, .. } = p;
            let _ = a;
            let _: i32 = match p {
                Pair { x: 1, .. } => 1,
                Pair { x, y } => x + y,
            };
            if p.x > 0 {
                return p.x;
            } else {
                p.y
            }
        }

        pub trait MacroTrait {
            my_mac!();
        }

        impl MacroTrait for Pair {
            my_mac!();
        }

        my_top_macro!(1, 2);
    "#;
    let extra_rsh = convert_from_rust(
        extra_rust,
        FromRustOptions {
            use_rustc_inference: false,
        },
    )
    .expect("extra Rust constructs should convert cleanly");
    assert!(extra_rsh.contains("w: while"));
    assert!(extra_rsh.contains("f: foreach"));
    assert!(extra_rsh.contains("l: loop"));
    assert!(extra_rsh.contains("when"));
    assert!(extra_rsh.contains("my_top_macro!"));

    // Error paths in from_rust & probe helpers
    assert!(matches!(
        convert_from_rust("fn broken( {", FromRustOptions::default()),
        Err(RSharpError::FromRust { .. })
    ));
    assert!(matches!(
        convert_from_rust("fn f() { let x; }", FromRustOptions::default()),
        Err(RSharpError::UnresolvedType { .. })
    ));
    assert!(matches!(
        convert_from_rust(
            "fn f() { let (a, b) = get_pair(); }",
            FromRustOptions {
                use_rustc_inference: false
            }
        ),
        Err(RSharpError::UnresolvedType { .. })
    ));
    assert!(matches!(
        convert_from_rust(
            "fn f() { let (a, b) = (unknown(), 1); }",
            FromRustOptions {
                use_rustc_inference: false
            }
        ),
        Err(RSharpError::UnresolvedType { .. })
    ));
    assert!(matches!(
        convert_from_rust("fn f() { let (a, b); }", FromRustOptions::default()),
        Err(RSharpError::UnresolvedType { .. })
    ));
    assert!(matches!(
        convert_from_rust(
            "fn f() { let v: Vec<i32> = vec![1]; let _: Vec<i32> = v.iter().map(|x| *x).collect(); }",
            FromRustOptions {
                use_rustc_inference: false
            }
        ),
        Err(RSharpError::UnresolvedType { .. })
    ));
    assert!(rsharp_core::rustc_runner::probe_local_type("fn f() {}", "missing", 1).is_err());
    assert!(
        rsharp_core::rustc_runner::probe_closure_param_type("fn f() {}", "missing", 1).is_err()
    );
}
