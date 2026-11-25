use rsharp_core::{
    PreludeMode, RSharpError, TranspileOptions, check_rsharp, run_rsharp, transpile_to_rust,
};

#[test]
fn semantic_enforces_immutable_by_default() {
    let src = r#"
        void main()
        {
            int x = 10;
            x = 20;
        }
    "#;
    let err = check_rsharp(src).expect_err("mutating immutable variable must fail rustc");
    assert!(matches!(err, RSharpError::Compile { .. }));
}

#[test]
fn semantic_enforces_borrow_checker_and_move_semantics() {
    // Use after explicit `take` move must fail borrow checker
    let moved = r#"
        void Consume(string s)
        {
            _ = s;
        }
        void main()
        {
            string name = "Alice".ToString();
            Consume(take name);
            PrintLine(name);
        }
    "#;
    let err = check_rsharp(moved).expect_err("use after move must fail rustc");
    assert!(matches!(err, RSharpError::Compile { .. }));

    // Simultaneous read + write borrow must fail borrow checker
    let aliasing = r#"
        void main()
        {
            mut int x = 5;
            read int r = read x;
            write int w = write x;
            deref w = 10;
            PrintLine(deref r);
        }
    "#;
    let err = check_rsharp(aliasing).expect_err("aliasing read and write borrows must fail rustc");
    assert!(matches!(err, RSharpError::Compile { .. }));
}

#[test]
fn semantic_enforces_explicit_pattern_types() {
    // `foreach` with wrong explicit type must fail rustc
    let wrong_foreach = r#"
        void main()
        {
            List<int> nums = [1, 2, 3];
            foreach (string bad in nums)
            {
                PrintLine(bad);
            }
        }
    "#;
    let err = check_rsharp(wrong_foreach).expect_err("mismatched foreach binding type must fail");
    assert!(matches!(err, RSharpError::Compile { .. }));

    // `if is` with wrong explicit type must fail rustc
    let wrong_if_is = r#"
        void main()
        {
            int? opt = Some(42);
            if (opt is Some(string bad))
            {
                PrintLine(bad);
            }
        }
    "#;
    let err = check_rsharp(wrong_if_is).expect_err("mismatched if-is binding type must fail");
    assert!(matches!(err, RSharpError::Compile { .. }));
}

#[test]
fn semantic_enforces_unsafe_boundary() {
    let outside_unsafe = r#"
        void main()
        {
            int x = 10;
            read ptr<int> p = (read x) as read ptr<int>;
            int v = deref p;
            PrintLine(v);
        }
    "#;
    let err = check_rsharp(outside_unsafe).expect_err("raw pointer deref outside unsafe must fail");
    assert!(matches!(err, RSharpError::Compile { .. }));

    let inside_unsafe = r#"
        void main()
        {
            int x = 10;
            read ptr<int> p = (read x) as read ptr<int>;
            mut int v = 0;
            unsafe
            {
                v = deref p;
            }
            PrintLine(v);
        }
    "#;
    let out = run_rsharp(inside_unsafe).expect("raw pointer deref inside unsafe must pass");
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "10");
}

#[test]
fn semantic_async_await_executes_natively() {
    let src = r#"
        public async Task<int> ComputeAsync(int a, int b)
        {
            return a * b + 1;
        }

        public async Task<int> PipelineAsync(int x)
        {
            int first = await ComputeAsync(x, 3);
            int second = await ComputeAsync(first, 2);
            return second;
        }

        void main()
        {
            rust {
                use std::future::Future;
                use std::pin::pin;
                use std::task::{Context, Poll, Waker};
                let waker = Waker::noop();
                let mut cx = Context::from_waker(waker);
                let mut fut = pin!(PipelineAsync(4));
                match fut.as_mut().poll(&mut cx) {
                    Poll::Ready(val) => println!("async={}", val),
                    Poll::Pending => panic!("unexpected pending"),
                }
            }
        }
    "#;
    let out = run_rsharp(src).expect("async/await program should compile and run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "async=27");
}

#[test]
fn semantic_lifetimes_and_trait_associated_types() {
    let src = r#"
        public trait Container
        {
            type Elem;
            read Self.Elem First(read this);
        }

        public struct Holder<life a, T>
        {
            public read<a> slice<T> Items;
        }

        impl<life a, T> Container for Holder<life a, T>
        {
            type Elem = T;
            read T First(read this)
            {
                return read this.Items[0];
            }
        }

        void main()
        {
            int[3] arr = [9, 8, 7];
            Holder<int> h = new Holder { Items = read slice arr[..] };
            PrintLine(deref h.First());
        }
    "#;
    let out = run_rsharp(src).expect("lifetimes + associated type trait should compile and run");
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "9");
}

#[test]
fn semantic_for_loop_continue_executes_step() {
    // Regression check: `continue` in C-style `for` must NOT skip `i++`!
    let src = r#"
        void main()
        {
            mut int sum = 0;
            for (mut int i = 0; i < 6; i++)
            {
                if (i % 2 == 0)
                {
                    continue;
                }
                sum += i;
            }
            PrintLine(sum);
        }
    "#;
    let out = run_rsharp(src).expect("for loop with continue should terminate and sum odds");
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "9");
}

#[test]
fn semantic_check_library_without_main() {
    let lib_src = r#"
        public int Square(int x)
        {
            return x * x;
        }
    "#;
    check_rsharp(lib_src).expect("library without main should pass check_rsharp");

    let rs = transpile_to_rust(
        lib_src,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .unwrap();
    assert!(rs.contains("pub fn Square(x: i32) -> i32"));
}
