use rsharp_core::run_rsharp;
use rsharp_core::rustc_runner::compile_and_run_rust;

fn assert_differential(rsharp_src: &str, rust_ref: &str) {
    let rs_out = run_rsharp(rsharp_src).expect("R# program should compile and run");
    let ref_out = compile_and_run_rust(rust_ref).expect("Reference Rust should compile and run");
    assert_eq!(rs_out.exit_code, ref_out.exit_code, "exit code mismatch");
    assert_eq!(rs_out.stdout, ref_out.stdout, "stdout mismatch");
    assert_eq!(rs_out.stderr, ref_out.stderr, "stderr mismatch");
}

#[test]
fn differential_matrix_multiplication_and_labeled_breaks() {
    let rsharp = r#"
        void main()
        {
            int[2, 2] a = [[1, 2], [3, 4]];
            int[2, 2] b = [[5, 6], [7, 8]];
            mut int[2, 2] c = [[0, 0], [0, 0]];
            for (mut nuint i = 0; i < 2; i++)
            {
                for (mut nuint j = 0; j < 2; j++)
                {
                    for (mut nuint k = 0; k < 2; k++)
                    {
                        c[i, j] += a[i, k] * b[k, j];
                    }
                }
            }
            PrintLine($"{c[0, 0]},{c[0, 1]},{c[1, 0]},{c[1, 1]}");

            mut int found = -1;
            outer: for (mut int r = 0; r < 5; r++)
            {
                for (mut int col = 0; col < 5; col++)
                {
                    if (r * 5 + col == 13)
                    {
                        found = r * 10 + col;
                        break outer;
                    }
                }
            }
            PrintLine(found);
        }
    "#;

    let rust_ref = r#"
        fn main() {
            let a = [[1i32, 2], [3, 4]];
            let b = [[5i32, 6], [7, 8]];
            let mut c = [[0i32, 0], [0, 0]];
            for i in 0..2 {
                for j in 0..2 {
                    for k in 0..2 {
                        c[i][j] += a[i][k] * b[k][j];
                    }
                }
            }
            println!("{},{},{},{}", c[0][0], c[0][1], c[1][0], c[1][1]);
            let mut found = -1;
            'outer: for r in 0..5 {
                for col in 0..5 {
                    if r * 5 + col == 13 {
                        found = r * 10 + col;
                        break 'outer;
                    }
                }
            }
            println!("{}", found);
        }
    "#;

    assert_differential(rsharp, rust_ref);
}

#[test]
fn differential_collections_aliases_and_helpers() {
    let rsharp = r#"
        void main()
        {
            mut List<int> list = [10, 20];
            list.Add(30);

            mut Queue<int> q = Queue.new();
            q.Enqueue(1);
            q.Enqueue(2);
            int first = q.Dequeue() ?? 0;

            mut Dictionary<string, int> dict = Dictionary.new();
            dict.Add("k".ToString(), 7);

            mut SortedDictionary<int, int> sdict = SortedDictionary.new();
            sdict.insert(2, 20);
            sdict.insert(1, 10);

            mut SortedSet<int> sset = SortedSet.new();
            sset.insert(5);
            sset.insert(3);

            mut PriorityQueue<int> pq = PriorityQueue.new();
            pq.push(4);
            pq.push(9);
            int top = pq.pop() ?? 0;

            PrintLine($"list={list.Count()} q={first} dict={dict.ContainsKey(read "k".ToString())} sdict={sdict.len()} sset={sset.len()} pq={top}");
        }
    "#;

    let rust_ref = r#"
        use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, VecDeque};
        fn main() {
            let mut list = vec![10, 20];
            list.push(30);
            let mut q = VecDeque::new();
            q.push_back(1);
            q.push_back(2);
            let first = q.pop_front().unwrap_or(0);
            let mut dict = HashMap::new();
            dict.insert("k".to_string(), 7);
            let mut sdict = BTreeMap::new();
            sdict.insert(2, 20);
            sdict.insert(1, 10);
            let mut sset = BTreeSet::new();
            sset.insert(5);
            sset.insert(3);
            let mut pq = BinaryHeap::new();
            pq.push(4);
            pq.push(9);
            let top = pq.pop().unwrap_or(0);
            println!("list={} q={} dict={} sdict={} sset={} pq={}", list.len(), first, dict.contains_key("k"), sdict.len(), sset.len(), top);
        }
    "#;

    assert_differential(rsharp, rust_ref);
}

#[test]
fn differential_coalesce_lazy_short_circuit_and_patterns() {
    let rsharp = r#"
        public int SideEffect(write int counter)
        {
            deref counter += 1;
            return 99;
        }

        public int Classify(int n)
        {
            return match (n)
            {
                -1 | 0 => 0,
                1..=5 => 1,
                int x when x % 2 == 0 => x / 2,
                _ => -1,
            };
        }

        public int UnwrapOrZero(int? opt)
        {
            require (opt is Some(int v)) else { return 0; }
            return v * 10;
        }

        void main()
        {
            mut int calls = 0;
            int? present = Some(5);
            int? absent = None;
            int a = present ?? SideEffect(write calls);
            int b = absent ?? SideEffect(write calls);

            int val = blk: {
                int t = 3;
                break blk t * 4;
            };

            (int X, int Y) pt = (a, b);
            int u1 = UnwrapOrZero(Some(3));
            int u2 = UnwrapOrZero(None);

            PrintLine($"{pt.X},{pt.Y},{calls},{val},{Classify(0)},{Classify(4)},{Classify(10)},{Classify(9)},{u1},{u2}");
        }
    "#;

    let rust_ref = r#"
        fn main() {
            println!("5,99,1,12,0,1,5,-1,30,0");
        }
    "#;

    assert_differential(rsharp, rust_ref);
}

#[test]
fn differential_lambdas_const_fn_and_dynamic() {
    let rsharp = r#"
        public const int Cube(int x)
        {
            return x * x * x;
        }

        public int Apply(int val, (int) -> int f)
        {
            return f(val);
        }

        void main()
        {
            int offset = 7;
            List<int> nums = [1, 2, 3, 4];
            List<int> mapped = nums.into_iter().map(take (int x) : int => x * 2 + offset).collect<List<int>>();
            int c = Cube(3);
            int ap = Apply(5, (int n) : int => n + 10);

            dynamic d1 = ToDynamic(42);
            dynamic d2 = ToDynamic("hi".ToString());
            int v1 = match (AsDynamic<int>(read d1))
            {
                Some(read int x) => deref x,
                None => 0,
            };
            bool isInt = AsDynamic<int>(read d2) is Some(_);
            PrintLine($"{mapped[0]},{mapped[3]},{c},{ap},{v1},{isInt}");
        }
    "#;

    let rust_ref = r#"
        fn main() {
            println!("9,15,27,15,42,false");
        }
    "#;

    assert_differential(rsharp, rust_ref);
}
