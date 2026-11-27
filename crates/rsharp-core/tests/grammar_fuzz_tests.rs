use std::time::Instant;

use rsharp_core::{
    FromRustOptions, PreludeMode, TranspileOptions, TypeHintMap, convert_from_rust_with_hints,
    count_raw_rust_blocks, parser, rustc_runner, transpile_to_rust,
};

struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn choose<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let idx = (self.next_u64() as usize) % items.len();
        &items[idx]
    }

    fn range(&mut self, upper: usize) -> usize {
        (self.next_u64() as usize) % upper
    }

    fn coin(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }
}

fn gen_type(rng: &mut SplitMix64, depth: usize) -> String {
    if depth == 0 {
        return (*rng.choose(&[
            "i32",
            "u64",
            "f64",
            "bool",
            "char",
            "usize",
            "String",
            "Vec<i32>",
            "Option<i32>",
            "Result<i32, String>",
            "&'static str",
            "&'a [u8]",
            "*const u8",
            "*mut i32",
            "()",
        ]))
        .to_string();
    }
    match rng.range(12) {
        0 => format!("&{}", gen_type(rng, depth - 1)),
        1 => format!("&mut {}", gen_type(rng, depth - 1)),
        2 => format!("&'a &'static {}", gen_type(rng, depth - 1)),
        3 => format!("Option<Option<{}>>", gen_type(rng, depth - 1)),
        4 => format!("[{}; {{ 1 + 2 }}]", gen_type(rng, depth - 1)),
        5 => format!("({}, {})", gen_type(rng, depth - 1), gen_type(rng, 0)),
        6 => format!("fn(&{}) -> {}", gen_type(rng, depth - 1), gen_type(rng, 0)),
        7 => format!("Box<dyn (Fn({}) -> i32) + Send>", gen_type(rng, depth - 1)),
        8 => format!("Box<dyn Iterator<Item = {}>>", gen_type(rng, depth - 1)),
        9 => format!("<{} as Trait>::Assoc", gen_type(rng, depth - 1)),
        10 => format!("Vec<{}>", gen_type(rng, depth - 1)),
        _ => "mac_ty! { i32 }".to_string(),
    }
}

fn gen_pat(rng: &mut SplitMix64, depth: usize, id: &mut usize, allow_or: bool) -> String {
    let fresh = |id: &mut usize| -> String {
        let n = *id;
        *id += 1;
        format!("v{n}")
    };
    if depth == 0 {
        return match rng.range(8) {
            0 => "_".to_string(),
            1 => fresh(id),
            2 => format!("mut {}", fresh(id)),
            3 => format!("ref {}", fresh(id)),
            4 => format!("ref mut {}", fresh(id)),
            5 => "0..=10".to_string(),
            6 => "'a'..='z'".to_string(),
            _ => "42".to_string(),
        };
    }
    match rng.range(if allow_or { 11 } else { 10 }) {
        0 => format!("&{}", gen_pat(rng, depth - 1, id, false)),
        1 => format!("&mut {}", gen_pat(rng, depth - 1, id, false)),
        2 => format!("&&{}", gen_pat(rng, depth - 1, id, false)),
        3 => format!("box {}", gen_pat(rng, depth - 1, id, false)),
        4 => format!(
            "({}, {})",
            gen_pat(rng, depth - 1, id, true),
            gen_pat(rng, 0, id, false)
        ),
        5 => format!(
            "[{}, .., {}]",
            gen_pat(rng, depth - 1, id, false),
            gen_pat(rng, 0, id, false)
        ),
        6 => format!(
            "S {{ a: {}, ref b, mut c, .. }}",
            gen_pat(rng, depth - 1, id, true)
        ),
        7 => {
            let b = fresh(id);
            let sub = gen_pat(rng, depth - 1, id, false);
            format!("{b} @ ({sub})")
        }
        8 => format!("Some({})", gen_pat(rng, depth - 1, id, true)),
        9 => format!("({})", gen_pat(rng, depth - 1, id, true)),
        _ => "0..=5 | 10..=20".to_string(),
    }
}

fn gen_expr(rng: &mut SplitMix64, depth: usize) -> String {
    if depth == 0 {
        return (*rng.choose(&[
            "x",
            "0",
            "1.0",
            "0.",
            "true",
            "false",
            "\"hi\"",
            "r#\"a//b\"#",
            "(x, y)",
            "[1, 2, 3]",
            "[0; { 2 * 2 }]",
            "S { a: x, ..base }",
            "foo::<Box<i32>>(x)",
            "bar!()",
            "baz![1, 2]",
            "qux! { x }",
        ]))
        .to_string();
    }
    match rng.range(18) {
        0 => format!(
            "if ({}) {{ {} }} else {{ {} }}",
            gen_expr(rng, 0),
            gen_expr(rng, depth - 1),
            gen_expr(rng, depth - 1)
        ),
        1 => format!(
            "if let Some(a) = ({}) {{ {} }} else {{ {} }}",
            gen_expr(rng, 0),
            gen_expr(rng, depth - 1),
            gen_expr(rng, 0)
        ),
        2 => format!(
            "match ({}) {{ Some(a) if a > 0 => {{ {} }}.into(), None => {}, _ => {} }}",
            gen_expr(rng, 0),
            gen_expr(rng, depth - 1),
            gen_expr(rng, 0),
            gen_expr(rng, depth - 1)
        ),
        3 => format!(
            "(move |v| {{ {} }}).call_it({})",
            gen_expr(rng, depth - 1),
            gen_expr(rng, 0)
        ),
        4 => format!("move |v| {{ {} }}.unwrap_or(0)", gen_expr(rng, depth - 1)),
        5 => format!("(|a, b| {} + {})(1, 2)", gen_expr(rng, 0), gen_expr(rng, 0)),
        6 => format!("({} as i64).abs()", gen_expr(rng, depth - 1)),
        7 => format!("&({} ..={})", gen_expr(rng, 0), gen_expr(rng, 0)),
        8 => "&(..)".to_string(),
        9 => "(0. ..=1.).contains(&0.5)".to_string(),
        10 => format!("{{ let t = {}; t }}.into()", gen_expr(rng, depth - 1)),
        11 => format!("'lbl: loop {{ break 'lbl {}; }}", gen_expr(rng, depth - 1)),
        12 => format!(
            "if ({}) {{ x }} else {{ 'inner: loop {{ break 'inner {}; }} }}",
            gen_expr(rng, 0),
            gen_expr(rng, depth - 1)
        ),
        13 => format!(
            "({}).method({})?",
            gen_expr(rng, depth - 1),
            gen_expr(rng, 0)
        ),
        14 => format!(
            "({} + {} * {})",
            gen_expr(rng, depth - 1),
            gen_expr(rng, 0),
            gen_expr(rng, 0)
        ),
        15 => format!("unsafe {{ {} }}.ok()", gen_expr(rng, depth - 1)),
        16 => format!("async move {{ {} }}", gen_expr(rng, depth - 1)),
        _ => format!(
            "({} == {} && !{})",
            gen_expr(rng, 0),
            gen_expr(rng, 0),
            gen_expr(rng, 0)
        ),
    }
}

fn gen_attr(rng: &mut SplitMix64) -> &'static str {
    rng.choose(&[
        "#[allow(unused)]",
        "#[cfg(feature = \"std\")]",
        "#[doc = r#\"line // comment\"#]",
        "#[custom_attr(r#\"hello // world\"#, b'x')]",
        "#[enum_attr(#[derive(Clone)] pub enum E<'a> { A(&'a str) })]",
        "#[single_lt('a)]",
        "#[pair_lt('a, 'b)]",
        "#[meta_var($x)]",
    ])
}

fn gen_module(rng: &mut SplitMix64, idx: usize) -> String {
    let mut id = 0usize;
    let attr = gen_attr(rng);
    let d_ret = rng.range(3);
    let ret_ty = gen_type(rng, d_ret);
    let d_pp = rng.range(3);
    let param_pat = gen_pat(rng, d_pp, &mut id, false);
    let d_pt = rng.range(2);
    let param_ty = gen_type(rng, d_pt);
    let d_lp = rng.range(3);
    let let_pat = gen_pat(rng, d_lp, &mut id, false);
    let let_ty = if rng.coin() {
        let d_lt = rng.range(2);
        format!(": {}", gen_type(rng, d_lt))
    } else {
        String::new()
    };
    let d_ap = rng.range(3);
    let arm_pat = gen_pat(rng, d_ap, &mut id, true);
    let d_ip = rng.range(2);
    let if_let_pat = gen_pat(rng, d_ip, &mut id, true);
    let d_cp = rng.range(2);
    let closure_pat = gen_pat(rng, d_cp, &mut id, false);
    let d_e1 = 2 + rng.range(2);
    let e1 = gen_expr(rng, d_e1);
    let d_e2 = 1 + rng.range(2);
    let e2 = gen_expr(rng, d_e2);
    let d_e3 = 1 + rng.range(2);
    let e3 = gen_expr(rng, d_e3);
    let extra_item = match rng.range(5) {
        0 => "pub trait Tr { pub unsafe fn com_call(&self, x: i32) -> i32; }\n",
        1 => "use { ::core::option::Option, self::helper };\n",
        2 => "pub trait Alias<T> = Clone + Send where T: Sync;\n",
        3 => "pub trait EmptyAlias =;\n",
        _ => "impl S { const fn bodyless(&self); }\n",
    };
    format!(
        "{extra_item}\
         {attr}\n\
         pub fn f_{idx}<'a>(({param_pat}): {param_ty}, x: i32, y: i32, base: S) -> {ret_ty} {{\n\
             let {let_pat}{let_ty} = {e1};\n\
             let cl = move |{closure_pat}| {{ {e2} }}.unwrap_or(0);\n\
             if let {if_let_pat} = x {{\n\
                 match y {{\n\
                     {arm_pat} if let Some(k) = z && k > 0 => {{ {e3} }}.into(),\n\
                     _ => {e2},\n\
                 }};\n\
             }}\n\
             {e1}\n\
         }}\n"
    )
}

#[test]
fn deterministic_combinatorial_grammar_fuzzer_2000_modules() {
    let mut rng = SplitMix64::new(0x5253_4841_5250_2026);
    let opts = FromRustOptions {
        use_rustc_inference: false,
    };
    let hints = TypeHintMap::default();
    let transpile_opts = TranspileOptions {
        prelude_mode: PreludeMode::None,
    };

    for i in 0..2000 {
        let rust_src = gen_module(&mut rng, i);
        // Sanity check: generator must emit syntactically valid Rust for `syn`.
        assert!(
            syn::parse_file(&rust_src).is_ok(),
            "generator produced invalid Rust at case {i}:\n{rust_src}"
        );

        let rsharp1 = convert_from_rust_with_hints(&rust_src, opts, &hints, false)
            .unwrap_or_else(|e| panic!("case {i} from_rust failed: {e}\n--- RUST ---\n{rust_src}"));
        let raw_count1 = count_raw_rust_blocks(&rsharp1);
        assert_eq!(
            raw_count1, 0,
            "case {i} emitted {raw_count1} rust{{}} block(s):\n--- RUST ---\n{rust_src}\n--- R# ---\n{rsharp1}"
        );

        let rust2 = transpile_to_rust(&rsharp1, transpile_opts).unwrap_or_else(|e| {
            panic!("case {i} transpile_to_rust failed: {e}\n--- RUST ---\n{rust_src}\n--- R# ---\n{rsharp1}")
        });
        syn::parse_file(&rust2).unwrap_or_else(|e| {
            panic!("case {i} regenerated Rust failed syn::parse_file: {e}\n--- RUST1 ---\n{rust_src}\n--- R#1 ---\n{rsharp1}\n--- RUST2 ---\n{rust2}")
        });

        // Double round-trip idempotency check (Rust1 -> R#1 -> Rust2 -> R#2 -> parse).
        let rsharp2 =
            convert_from_rust_with_hints(&rust2, opts, &hints, false).unwrap_or_else(|e| {
                panic!("case {i} second from_rust failed: {e}\n--- RUST2 ---\n{rust2}")
            });
        assert_eq!(
            count_raw_rust_blocks(&rsharp2),
            0,
            "case {i} second pass emitted rust{{}}:\n{rsharp2}"
        );
        parser::parse_file(&rsharp2).unwrap_or_else(|e| {
            panic!("case {i} second R# parse failed: {e}\n--- R#2 ---\n{rsharp2}")
        });
    }
}

#[test]
fn deterministic_semantic_batch_compiles_via_rustc() {
    let mut rng = SplitMix64::new(0xC0DE_5EED_0001);
    let mut rust_src = String::from(
        "#![allow(unused, dead_code, clippy::all)]\n\
         #[derive(Clone, Copy)]\n\
         pub struct Pair { pub a: i32, pub b: i32 }\n",
    );
    for i in 0..100 {
        let op = *rng.choose(&["+", "-", "*", "^", "|", "&"]);
        let use_match = rng.coin();
        let use_closure = rng.coin();
        let use_range = rng.coin();
        rust_src.push_str(&format!(
            "pub fn sem_{i}(p: Pair, opt: Option<i32>, arr: &[i32; 3]) -> i32 {{\n\
                 let Pair {{ a, b }} = p;\n\
                 let r: i32 = if a > b {{ a {op} b }} else {{ b {op} a }};\n"
        ));
        if use_match {
            rust_src.push_str(
                "    let m: i32 = match opt {\n\
                         Some(v) if v > 0 => { let t: i32 = v + 1; t },\n\
                         Some(v) => v,\n\
                         None => 0,\n\
                     };\n",
            );
        } else {
            rust_src.push_str("    let m: i32 = opt.unwrap_or(0);\n");
        }
        if use_closure {
            rust_src.push_str("    let c: i32 = (move |k: i32| { let u: i32 = k + 1; u })(r);\n");
        } else {
            rust_src.push_str("    let c: i32 = r;\n");
        }
        if use_range {
            rust_src.push_str(
                "    let f: i32 = if (0. ..=1.).contains(&0.5) { arr[0] } else { arr[1] };\n",
            );
        } else {
            rust_src.push_str("    let f: i32 = arr[2];\n");
        }
        rust_src.push_str("    c.wrapping_add(m).wrapping_add(f)\n}\n");
    }

    let rsharp = convert_from_rust_with_hints(
        &rust_src,
        FromRustOptions {
            use_rustc_inference: false,
        },
        &TypeHintMap::default(),
        false,
    )
    .expect("semantic batch from_rust failed");
    assert_eq!(count_raw_rust_blocks(&rsharp), 0);
    let regenerated = transpile_to_rust(
        &rsharp,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("semantic batch transpile_to_rust failed");
    rustc_runner::check_rust_source(&regenerated, true)
        .expect("regenerated semantic batch must compile cleanly with rustc");
}

#[test]
fn parser_linear_time_stress_matrix_no_exponential_backtracking() {
    let handle = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let opts = FromRustOptions {
                use_rustc_inference: false,
            };
            let hints = TypeHintMap::default();
            let transpile_opts = TranspileOptions {
                prelude_mode: PreludeMode::None,
            };

            // 1. 50-deep nested generics
            let deep_box = format!(
                "pub fn f(_: {}{}{}) {{}}",
                "Box<".repeat(50),
                "i32",
                ">".repeat(50)
            );
            // 2. 50-deep nested value blocks with postfix `.into()`
            let deep_blocks = format!(
                "pub fn f(x: i32) {{ {}{}{}; }}",
                "{ ".repeat(40),
                "x",
                " }.into()".repeat(40)
            );
            // 3. 40-deep nested `if` statements with postfix `.into()`
            let deep_ifs = format!(
                "pub fn f(c: bool, x: i32) {{ {}{}{}; }}",
                "if c { ".repeat(35),
                "x",
                " } else { x }.into()".repeat(35)
            );
            // 4. 35-deep nested `match` arms with block postfix `.into()`
            let deep_matches = format!(
                "pub fn f(x: i32) {{ {}{}{}; }}",
                "match x { _ => { ".repeat(30),
                "x",
                " }.into() }.into()".repeat(30)
            );
            // 5. 40-deep nested lambdas with block postfix `.unwrap_or(0)`
            let deep_lambdas = format!(
                "pub fn f(x: i32) {{ let _ = {}{}{}; }}",
                "move |_| { ".repeat(35),
                "x",
                " }.unwrap_or(0)".repeat(35)
            );
            // 6. 50-deep nested parenthesized expressions & tuple patterns
            let deep_parens = format!(
                "pub fn f(x: i32) -> i32 {{ let {}a{} = {}x{}; a }}",
                "(".repeat(50),
                ")".repeat(50),
                "(".repeat(50),
                ")".repeat(50)
            );
            // 7. 50-deep nested `deref` patterns & 150-deep chained `.method()?`
            let deep_deref_chain = format!(
                "pub fn f({}x: {}i32) -> Result<i32, ()> {{ Ok(x{}) }}",
                "&".repeat(35),
                "&".repeat(35),
                ".m()?".repeat(150)
            );

            for (name, src) in [
                ("deep_box", deep_box),
                ("deep_blocks", deep_blocks),
                ("deep_ifs", deep_ifs),
                ("deep_matches", deep_matches),
                ("deep_lambdas", deep_lambdas),
                ("deep_parens", deep_parens),
                ("deep_deref_chain", deep_deref_chain),
            ] {
                let start = Instant::now();
                let rsh = convert_from_rust_with_hints(&src, opts, &hints, false)
                    .unwrap_or_else(|e| panic!("{name} from_rust failed: {e}"));
                assert_eq!(count_raw_rust_blocks(&rsh), 0, "{name} emitted rust{{}}");
                let rust = transpile_to_rust(&rsh, transpile_opts)
                    .unwrap_or_else(|e| panic!("{name} transpile failed: {e}\n{rsh}"));
                syn::parse_file(&rust).unwrap_or_else(|e| panic!("{name} syn parse failed: {e}"));
                let elapsed = start.elapsed();
                assert!(
                    elapsed.as_secs() < 2,
                    "{name} took {elapsed:?} — possible exponential backtracking!"
                );
            }

            // Cover loop/while/foreach postfix promotion, error propagation, struct-lit guards, and exclusive range pattern.
            let extra_rsharp = "\
void f(bool c, int x, List<int> xs)
{
    while (c) { break; }.into();
    foreach (int v in xs) { _ = v; break; }.into();
    if (write new S { a = 1 }) {}
    if (deref new S { a = 1 }) {}
    match (x) { ..60 => {}, _ => {} }
}
";
            let rust = transpile_to_rust(extra_rsharp, transpile_opts).unwrap();
            assert!(syn::parse_file(&rust).is_ok(), "{rust}");
            assert!(parser::parse_file("void bad() { { return 1; }.; }").is_err());
        })
        .unwrap();
    handle.join().unwrap();
}
