use rsharp_core::rustc_runner::compile_and_run_rust;
use rsharp_core::{
    FromRustOptions, PreludeMode, RSharpError, TranspileOptions, convert_from_rust, run_rsharp,
    transpile_to_rust,
};

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
