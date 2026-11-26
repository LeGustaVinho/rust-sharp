use std::cell::RefCell;
use std::io::{self, BufRead, Cursor, Read, Write};
use std::path::Path;
use std::rc::Rc;

struct SharedWriter(Rc<RefCell<Vec<u8>>>);
impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

use rsharp_core::{
    FromRustOptions, PreludeMode, RustAnalyzerOracle, TranspileOptions, TypeHintMap,
    convert_from_rust, convert_from_rust_with_hints, find_cargo_workspace_root, lexer, parser,
    ra_oracle::{parse_inlay_hint_response, path_to_file_uri},
    run_rsharp,
    token::TokenKind,
    transpile_to_rust,
};
use serde_json::json;

fn frame_lsp(msg: &serde_json::Value) -> Vec<u8> {
    let body = msg.to_string();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

struct FailingWriter;
impl Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "write boom"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FailingReader;
impl Read for FailingReader {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "read boom"))
    }
}
impl BufRead for FailingReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "read boom"))
    }
    fn consume(&mut self, _amt: usize) {}
}

struct CountdownWriter {
    remaining_ok: usize,
}
impl Write for CountdownWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.remaining_ok == 0 {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "countdown boom"));
        }
        self.remaining_ok -= 1;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn roundtrip_and_check(rust_src: &str, hints: &TypeHintMap, strict: bool) -> (String, String) {
    let rsh = convert_from_rust_with_hints(
        rust_src,
        FromRustOptions {
            use_rustc_inference: false,
        },
        hints,
        strict,
    )
    .expect("from_rust failed");
    let rust = transpile_to_rust(
        &rsh,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("to_rust failed");
    (rsh, rust)
}

#[test]
fn ra_oracle_mock_protocol_and_helpers_full_coverage() {
    // TypeHintMap basic operations
    let mut map = TypeHintMap::default();
    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
    map.insert(1, 5, "i32".to_string());
    map.insert_with_verified(2, 10, "MyType".to_string(), false);
    assert!(!map.is_empty());
    assert_eq!(map.len(), 2);
    assert_eq!(map.get(1, 5), Some("i32"));
    assert_eq!(map.get(9, 9), None);
    assert_eq!(map.get_entry(1, 5), Some(("i32", true)));
    assert_eq!(map.get_entry(2, 10), Some(("MyType", false)));

    // path_to_file_uri on Unix-style leading slash, Windows-style, and spaces
    let unix_uri = path_to_file_uri(Path::new("/tmp/my folder/a b.rs"));
    assert!(unix_uri.starts_with("file:///") || unix_uri.starts_with("file://"));
    assert!(unix_uri.contains("%20"));
    let rel_uri = path_to_file_uri(Path::new("relative_nonexistent/file name.rs"));
    assert!(rel_uri.starts_with("file:///"));
    assert!(rel_uri.contains("%20"));

    // find_cargo_workspace_root: outer workspace > member crate > single crate > none
    let tmp = std::env::temp_dir().join(format!("rsharp_oracle_ws_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let ws = tmp.join("ws");
    let member_src = ws.join("crates").join("m1").join("src");
    std::fs::create_dir_all(&member_src).unwrap();
    std::fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/m1\"]\n",
    )
    .unwrap();
    std::fs::write(
        ws.join("crates").join("m1").join("Cargo.toml"),
        "[package]\nname = \"m1\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let lib_file = member_src.join("lib.rs");
    std::fs::write(&lib_file, "pub fn f() {}\n").unwrap();
    assert_eq!(
        find_cargo_workspace_root(&lib_file).as_deref(),
        Some(ws.as_path())
    );
    assert_eq!(
        find_cargo_workspace_root(&ws.join("crates").join("m1")).as_deref(),
        Some(ws.as_path())
    );
    let single = tmp.join("single");
    std::fs::create_dir_all(&single).unwrap();
    std::fs::write(single.join("Cargo.toml"), "[package]\nname = \"s\"\n").unwrap();
    assert_eq!(
        find_cargo_workspace_root(&single).as_deref(),
        Some(single.as_path())
    );
    let isolated = tmp.join("no_cargo");
    std::fs::create_dir_all(&isolated).unwrap();
    assert!(find_cargo_workspace_root(&isolated).is_none());

    // parse_inlay_hint_response edge cases
    assert!(parse_inlay_hint_response(&json!({"not": "array"})).is_empty());
    let parsed = parse_inlay_hint_response(&json!([
        { "kind": 2, "position": { "line": 0, "character": 1 }, "label": "param:" },
        { "kind": 1, "label": ": MissingPos" },
        { "kind": 1, "position": { "character": 1 }, "label": ": MissingLine" },
        { "kind": 1, "position": { "line": 0 }, "label": ": MissingChar" },
        { "kind": 1, "position": { "line": 0, "character": 1 }, "label": "= 42" },
        { "kind": 1, "position": { "line": 0, "character": 2 }, "label": "   " },
        {
            "kind": 1,
            "position": { "line": 1, "character": 4 },
            "label": [{ "value": ": " }, { "value": "Foo" }],
            "textEdits": [{ "newText": ": crate::Foo" }]
        },
        {
            "kind": 1,
            "position": { "line": 2, "character": 4 },
            "label": ": Bar",
            "textEdits": [
                { "newText": "use crate::Bar;\n" },
                { "newText": ": Bar" }
            ],
            "_rsharp_no_edit": true
        },
        {
            "kind": 1,
            "position": { "line": 3, "character": 4 },
            "label": ": Baz",
            "textEdits": [{ "newText": "" }, { "newText": ": crate::Baz" }]
        },
        {
            "kind": 1,
            "position": { "line": 4, "character": 4 },
            "label": 123
        },
        {
            "kind": 1,
            "position": { "line": 5, "character": 4 },
            "label": [{ "value": ": " }, { "value": "ArrLabel" }],
            "textEdits": [{ "range": {} }]
        }
    ]));
    assert_eq!(parsed.get_entry(2, 4), Some(("crate::Foo", true)));
    assert_eq!(parsed.get_entry(3, 4), Some(("Bar", false)));
    assert_eq!(parsed.get_entry(4, 4), Some(("crate::Baz", true)));
    assert_eq!(parsed.get_entry(5, 4), None);
    assert_eq!(parsed.get_entry(6, 4), Some(("ArrLabel", true)));

    // Deterministic RustAnalyzerOracle::from_io full session:
    // initialize + server request + non-quiescent + quiescent + type_hints_for_file
    // (parameter hint, non-colon label, primitive, no-data without/with textEdits,
    //  data resolved with/without textEdits) + shutdown/close.
    let mut incoming = Vec::new();
    // Server-to-client request during initialize read_response
    incoming.extend(frame_lsp(&json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "client/registerCapability",
        "params": {}
    })));
    // Notification ignored by read_response
    incoming.extend(frame_lsp(&json!({
        "jsonrpc": "2.0",
        "method": "window/logMessage",
        "params": {}
    })));
    // Initialize response (id 1)
    incoming.extend(frame_lsp(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "capabilities": {} }
    })));
    // Server request during wait_until_quiescent
    incoming.extend(frame_lsp(&json!({
        "jsonrpc": "2.0",
        "id": 100,
        "method": "window/workDoneProgress/create",
        "params": {}
    })));
    // Non-quiescent status followed by quiescent=true
    incoming.extend(frame_lsp(&json!({
        "jsonrpc": "2.0",
        "method": "experimental/serverStatus",
        "params": { "quiescent": false }
    })));
    incoming.extend(frame_lsp(&json!({
        "jsonrpc": "2.0",
        "method": "experimental/serverStatus",
        "params": { "quiescent": true }
    })));
    // textDocument/inlayHint response (id 2)
    incoming.extend(frame_lsp(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "result": [
            { "kind": 2, "position": { "line": 0, "character": 1 }, "label": "arg:" },
            { "kind": 1, "position": { "line": 0, "character": 2 }, "label": "-> i32" },
            { "kind": 1, "position": { "line": 0, "character": 3 }, "label": ": i32" },
            { "kind": 1, "position": { "line": 0, "character": 4 }, "label": ": NoDataNoEdit" },
            {
                "kind": 1,
                "position": { "line": 0, "character": 5 },
                "label": ": NoDataWithEdit",
                "textEdits": [{ "newText": ": crate::NoDataWithEdit" }]
            },
            {
                "kind": 1,
                "position": { "line": 0, "character": 6 },
                "label": ": Resolvable",
                "data": { "token": 1 }
            },
            {
                "kind": 1,
                "position": { "line": 0, "character": 7 },
                "label": ": Unresolvable",
                "data": { "token": 2 }
            },
            {
                "kind": 1,
                "position": { "line": 0, "character": 8 },
                "label": ": Eager",
                "textEdits": [{ "newText": ": crate::Eager" }],
                "data": { "token": 3 }
            }
        ]
    })));
    // inlayHint/resolve for id 3 (returns textEdits)
    incoming.extend(frame_lsp(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "result": {
            "kind": 1,
            "position": { "line": 0, "character": 6 },
            "label": ": Resolvable",
            "textEdits": [{ "newText": ": crate::Resolvable" }]
        }
    })));
    // inlayHint/resolve for id 4 (returns empty textEdits -> _rsharp_no_edit)
    incoming.extend(frame_lsp(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "result": {
            "kind": 1,
            "position": { "line": 0, "character": 7 },
            "label": ": Unresolvable",
            "textEdits": []
        }
    })));
    // shutdown response (id 5) using lowercase content-length header
    let shutdown_body = json!({"jsonrpc": "2.0", "id": 5, "result": null}).to_string();
    incoming.extend(
        format!(
            "content-length: {}\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{}",
            shutdown_body.len(),
            shutdown_body
        )
        .as_bytes(),
    );

    let outgoing = Rc::new(RefCell::new(Vec::new()));
    let mut oracle = RustAnalyzerOracle::from_io(
        Cursor::new(incoming),
        SharedWriter(Rc::clone(&outgoing)),
        &ws,
    )
    .expect("from_io init failed");
    let hints = oracle
        .type_hints_for_file(&lib_file, "fn main() {\r\n    let x = 1;\r\n}\r\n")
        .expect("type_hints_for_file failed");
    assert_eq!(hints.get_entry(1, 3), Some(("i32", true)));
    assert_eq!(hints.get_entry(1, 4), Some(("NoDataNoEdit", false)));
    assert_eq!(hints.get_entry(1, 5), Some(("crate::NoDataWithEdit", true)));
    assert_eq!(hints.get_entry(1, 6), Some(("crate::Resolvable", true)));
    assert_eq!(hints.get_entry(1, 7), Some(("Unresolvable", false)));
    assert_eq!(hints.get_entry(1, 8), Some(("crate::Eager", true)));
    oracle.close();
    let sent = String::from_utf8(outgoing.borrow().clone()).unwrap();
    assert!(sent.contains("\"method\":\"initialize\""));
    assert!(!sent.contains("\"resolveSupport\""));
    assert_eq!(sent.matches("\"method\":\"inlayHint/resolve\"").count(), 2);
    assert!(sent.contains("\"method\":\"shutdown\""));
    assert!(sent.contains("\"method\":\"exit\""));

    // Error paths: LSP error response, EOF, missing Content-Length, truncated body,
    // invalid JSON, writer error, reader error, missing binary, exiting binary,
    // and failing writes at every LSP step via CountdownWriter.
    let err_resp = frame_lsp(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": { "code": -32603, "message": "boom" }
    }));
    assert!(RustAnalyzerOracle::from_io(Cursor::new(err_resp), Vec::new(), &ws).is_err());
    assert!(RustAnalyzerOracle::from_io(Cursor::new(Vec::<u8>::new()), Vec::new(), &ws).is_err());
    assert!(RustAnalyzerOracle::from_io(Cursor::new(b"\r\n".to_vec()), Vec::new(), &ws).is_err());
    assert!(
        RustAnalyzerOracle::from_io(
            Cursor::new(b"Content-Length: 50\r\n\r\nshort".to_vec()),
            Vec::new(),
            &ws
        )
        .is_err()
    );
    assert!(
        RustAnalyzerOracle::from_io(
            Cursor::new(b"Content-Length: 4\r\n\r\n{bad".to_vec()),
            Vec::new(),
            &ws
        )
        .is_err()
    );
    assert!(
        RustAnalyzerOracle::from_io(Cursor::new(Vec::<u8>::new()), FailingWriter, &ws).is_err()
    );
    assert!(RustAnalyzerOracle::from_io(FailingReader, Vec::new(), &ws).is_err());
    assert!(RustAnalyzerOracle::open_with_bin("nonexistent_ra_binary_xyz_12345", &ws).is_err());
    // Binary that exits immediately without LSP output triggers `initialize()?` Err in `open_with_bin`
    let immediate_bin = if cfg!(windows) { "hostname" } else { "true" };
    assert!(RustAnalyzerOracle::open_with_bin(immediate_bin, &ws).is_err());

    // Build reusable full-session incoming transcript with server requests and hints
    let make_session = |resolve_err: bool, null_hints: bool| {
        let mut buf = Vec::new();
        buf.extend(frame_lsp(
            &json!({"jsonrpc":"2.0","id":80,"method":"ping","params":{}}),
        ));
        buf.extend(frame_lsp(
            &json!({"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}),
        ));
        buf.extend(frame_lsp(
            &json!({"jsonrpc":"2.0","id":81,"method":"ping2","params":{}}),
        ));
        buf.extend(frame_lsp(&json!({"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"quiescent":true}})));
        if null_hints {
            buf.extend(frame_lsp(&json!({"jsonrpc":"2.0","id":2,"result":null})));
        } else {
            buf.extend(frame_lsp(&json!({
                "jsonrpc":"2.0",
                "id":2,
                "result":[
                    {
                        "kind": 1,
                        "position": {"line":0,"character":2},
                        "label": [{"value": ": "}, {"value": "Res"}],
                        "data": {"k": 1}
                    }
                ]
            })));
            if resolve_err {
                buf.extend(frame_lsp(
                    &json!({"jsonrpc":"2.0","id":3,"error":{"code":-1,"message":"nope"}}),
                ));
            } else {
                buf.extend(frame_lsp(&json!({"jsonrpc":"2.0","id":3,"result":{"kind":1,"position":{"line":0,"character":2},"label":": Res"}})));
            }
        }
        buf
    };

    // Cover every write failure point during initialize + wait_until_quiescent + type_hints_for_file + close
    for limit in 0..=45 {
        let stream = make_session(false, false);
        let cw = CountdownWriter {
            remaining_ok: limit,
        };
        if let Ok(mut o) = RustAnalyzerOracle::from_io(Cursor::new(stream), cw, &ws) {
            let _ = o.type_hints_for_file(&lib_file, "fn main() {}");
            o.close();
        }
    }
    // Cover non-object resolved inlayHint (`result: null`) in `mark_no_edit_if_empty`
    let mut non_obj_resolve = Vec::new();
    non_obj_resolve.extend(frame_lsp(&json!({"jsonrpc":"2.0","id":1,"result":{}})));
    non_obj_resolve.extend(frame_lsp(
        &json!({"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"quiescent":true}}),
    ));
    non_obj_resolve.extend(frame_lsp(&json!({"jsonrpc":"2.0","id":2,"result":[{"kind":1,"position":{"line":0,"character":2},"label":": Res","data":{"k":1}}]})));
    non_obj_resolve.extend(frame_lsp(&json!({"jsonrpc":"2.0","id":3,"result":null})));
    let mut o_non_obj =
        RustAnalyzerOracle::from_io(Cursor::new(non_obj_resolve), Vec::new(), &ws).unwrap();
    let _ = o_non_obj
        .type_hints_for_file(&lib_file, "fn main() {}")
        .unwrap();
    o_non_obj.close();
    // Cover EOF during wait_until_quiescent
    let mut eof_quiescent = Vec::new();
    eof_quiescent.extend(frame_lsp(&json!({"jsonrpc":"2.0","id":1,"result":{}})));
    assert!(RustAnalyzerOracle::from_io(Cursor::new(eof_quiescent), Vec::new(), &ws).is_err());

    // Cover non-array inlayHint response (`result: null`) & error on `inlayHint/resolve` & error on `inlayHint`
    let mut o_null =
        RustAnalyzerOracle::from_io(Cursor::new(make_session(false, true)), Vec::new(), &ws)
            .unwrap();
    assert!(
        o_null
            .type_hints_for_file(&lib_file, "fn main() {}")
            .unwrap()
            .is_empty()
    );
    o_null.close();

    let mut o_res_err =
        RustAnalyzerOracle::from_io(Cursor::new(make_session(true, false)), Vec::new(), &ws)
            .unwrap();
    let _ = o_res_err
        .type_hints_for_file(&lib_file, "fn main() {}")
        .unwrap();
    o_res_err.close();

    let mut eof_inlay = Vec::new();
    eof_inlay.extend(frame_lsp(&json!({"jsonrpc":"2.0","id":1,"result":{}})));
    eof_inlay.extend(frame_lsp(
        &json!({"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"quiescent":true}}),
    ));
    let mut o_eof_inlay =
        RustAnalyzerOracle::from_io(Cursor::new(eof_inlay), Vec::new(), &ws).unwrap();
    assert!(
        o_eof_inlay
            .type_hints_for_file(&lib_file, "fn main() {}")
            .is_err()
    );
    o_eof_inlay.close();

    // Real rust-analyzer integration + Drop cleanup path (drop without close())
    let oracle_drop = RustAnalyzerOracle::open(&ws).expect("real rust-analyzer open failed");
    drop(oracle_drop);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn real_world_syntax_parser_lexer_and_codegen_regressions() {
    // Lexer: standalone `$` token + raw macro bodies + trailing dot before `@` + byte escapes
    let dollar_toks = lexer::tokenize("$").unwrap();
    assert_eq!(dollar_toks[0].kind, TokenKind::Dollar);

    // Semantic test compiling and running real-world R# constructs end-to-end
    let rsharp_prog = r#"
        using std.collections.HashMap;

        struct Queue { public int count; }

        enum CollectionKind { List, Queue, Dictionary }

        struct Holder {
            public int x;
            public int y;
            public int value;
        }

        impl Holder {
            public void swap_fields(write this, int a, int b) {
                (this.x, this.y) = (a, b);
            }
        }

        const read<static> str GLOBAL_MSG = "ok";

        int string(int n) {
            return n + 1;
        }

        const int use_const_generic<const nuint N>() {
            return N as int;
        }

        int apply_fn((int) -> int f, int v) {
            return f(v);
        }

        void main() {
            // T?? double optional
            int?? nested = Some(Some(7));
            int nval = match (nested) {
                Some(Some(int v)) => v,
                _ => 0,
            };

            // Explicit type on destructuring stmt
            (read str, read slice<read str>) pair_src = ("hi", read ["a", "b"]);
            (lbl, items) : (read str, read slice<read str>) = pair_src;

            // Destructuring assign on local struct fields, slice, tuple struct, struct, paren
            mut Holder h = new Holder { x = 1, y = 2, value = 3 };
            (h.x, h.y) = (10, 20);
            h.swap_fields(30, 40);
            mut int a1 = 0;
            mut int a2 = 0;
            [a1, .., a2] = [1, 2, 3];
            mut int p1 = 0;
            (p1,) = (5,);
            (p1) = 6;

            // Local `struct Queue` must NOT be remapped to VecDeque; `fn string` not remapped to String
            Queue q = new Queue { count = string(4) };

            // Enum wildcard & alias shadowing std collection aliases
            int kind_tag = kind_blk: {
                using CollectionKind.*;
                break kind_blk match (List) {
                    List => 1,
                    Queue => 2,
                    Dictionary => 3,
                };
            };

            // Const generic multi-statement block + explicit `::` path chain
            int cg = use_const_generic<{ nuint k = 2; return k + 3; }>();

            // Paren group preservation `(f)(x)`, `(|| {})()`, `(&mut v).into_iter()`
            int fn_res = (apply_fn)((int x) => x * 2, 9);
            int imm_closure = (() => 11)();
            mut List<int> vec_vals = [1, 2, 3];
            mut int sum = 0;
            foreach (v in (write vec_vals).into_iter()) {
                sum += deref v;
            }

            // Wildcard lambda, match-ergonomics lambda, impl Trait lambda param, attr on pattern param
            int wsum = [10, 20].iter().enumerate().map(([Allow(unused)] (nuint idx, _)) => idx as int).sum<int>();
            int impl_lam = ((impl Copy x) => { return 1; })(42);

            // Statement-position match with expression arms returning non-unit values
            mut HashMap<read str, int> map = HashMap.new();
            match (true) {
                true => map.insert("k", 99),
                false => map.remove("k"),
            }

            // Inline rust local shadowing + `ALL_CAPS_CONST.len()`
            rust { let (reading, _, _) = (vec![1, 2], 3, 4); }
            nuint rlen = reading.iter().count();
            nuint glen = GLOBAL_MSG.len();

            // Untyped `mut` in pattern + `break lbl (-10)`
            mut int neg = blk: {
                break blk (-10);
            };
            neg += 10;

            PrintLine($"{nval},{lbl},{items.len()},{h.x},{h.y},{a1},{a2},{p1},{q.count},{kind_tag},{cg},{fn_res},{imm_closure},{sum},{wsum},{impl_lam},{rlen},{glen},{neg}");
        }
    "#;
    let out = run_rsharp(rsharp_prog).expect("semantic real-world R# program failed");
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(
        out.stdout.trim(),
        "7,hi,2,30,40,1,3,6,5,1,5,18,11,6,1,1,2,2,0"
    );

    // Parser edge cases:
    // - explicit `::` 3-segment path chain `a::b::c`
    // - `this.m!()` macro path via `expr_to_path(Expr::This)`
    // - failed `:` type after destructuring pattern falls back cleanly
    // - destructuring assign with `Struct { x: a, y, .. }` and `TupleStruct(a)` and `[a, .. rest]`
    // - `const fn` with `impl Trait` param (skips `__rsharp_impl_check` inside `const fn`)
    // - `using` alias shadowing `List`, `using mymod.List;`, `using std.collections.List;`
    let extra_rsharp = r#"
        using mymod.List;
        using mymod.{List};
        using List = other.MyList;
        using std.collections.List;
        using core.collections.List;
        using alloc.vec.List;
        using rsharp_prelude.List;
        using std.collections.{Queue};
        using core.collections.{Queue};
        using alloc.collections.{Queue};
        using rsharp_prelude.{Queue};

        struct Pair(int);
        struct Boxed { int a; int b; }
        struct MY_TYPE;

        const void const_impl(impl Copy x) {
            impl Copy y = x;
            int _ = 0;
        }

        void parser_and_codegen_edges() {
            int x = a::b::c;
            int c1 = foo::BAR.len();
            int c2 = foo::baz.qux();
            int c3 = std.MY_CONST.len();
            int c4 = a.MY_TYPE.new();
            string s_new = string.new();
            this.my_mac!();
            require (match (true) { true => Some(1), false => None } is Some(int req_v)) else { return; }
            (int u, mut v) = (1, 2);
            [mut int s0, .. mut rest] = [1, 2, 3];
            Boxed { mut int a, b: mut b2 } = new Boxed { a = 1, b = 2 };
            Pair(mut pv) = Pair(1);
            (mut int pv2) = 1;
            mut int t1 = 0;
            mut int t2 = 0;
            Pair(t1) = Pair(2);
            Boxed { a: t1, b: t2, .. } = new Boxed { a = 3, b = 4 };
            Boxed { a: t1, b } = new Boxed { a = 5, b = 6 };
            ((t1), t2) = (1, 2);
            [t1, .. rest_a] = [1, 2];
            int k = ((int a) => a)(1);
            int m1 = (([Allow(unused)] impl Copy a, mut impl Copy b, impl life static c) => 1)(1, 2, 3);
            int m2 = (([Allow(unused)] (p,) : (int,)) => p)((1,));
            int m3 = (((impl Copy a,), (impl life static b,)) => 1)((1,), (2,));
            int m4 = ((deref _) => 1)(read 1);
            int m5 = t1.Foo<int>;
            int m6 = t1.sub.Foo<int>;
            int m7 = (1 + 2).Foo<int>;
            int m8 = unknown_mod.Foo<int>.Bar();
            match (k) {
                Foo<int>.Bar => 1,
                _ => 0,
            }
            rust { let x_a @ (y_b, [z_c], Pair(p_d), Boxed { a: b_e, .. }) = val; }
            rust { let (or_a | or_a) = v; }
            rust { let &r_f: &i32 = &1; }
            rust { let (p_g) = 1; }
            rust { let _ = 1; }
            rust { foo(); }
        }
    "#;
    let rust_out = transpile_to_rust(
        extra_rsharp,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("extra_rsharp codegen failed");
    assert!(rust_out.contains("a::b::c"));
    assert!(rust_out.contains("self::my_mac!"));
    assert!(rust_out.contains("Pair(t1) = Pair(2);"));
    assert!(rust_out.contains("Boxed { a: t1, b: t2, .. } ="));
    assert!(rust_out.contains("[t1, rest_a @ ..] = [1, 2];"));
    assert!(rust_out.contains("use mymod::List;"));
    assert!(rust_out.contains("use std::collections::Vec;"));
    assert!(rust_out.contains("use std::collections::{VecDeque};"));
    assert!(rust_out.contains("String::new()"));

    // Parser fallback when destructuring pattern followed by `:` and non-type token
    assert!(parser::parse_file("void f() { (a, b) : ; }").is_err());
    // Parser `expr_to_path` returning None on non-path macro callee & bad macro delimiters
    assert!(parser::parse_file("void f() { (1 + 2).bar!(); }").is_err());
    assert!(parser::parse_file("void f() { foo! ; }").is_err());
    assert!(parser::parse_file("type T = foo! ;").is_err());
    assert!(parser::parse_file("void f() { match (x) { foo! => 1 } }").is_err());
    // Parser `Expr::Path` with turbofish in `expr_to_path`
    let _ = parser::parse_file("void f() { a::b<int>!(); }");
    // Parser `pattern_has_binding_modifier` on nested Or, DerefPat, Paren, SliceRest
    let _ = parser::parse_file(
        "void f() { ((Ok(mut z) | Err(mut z)), a) = v; (deref mut x, a) = v; ((mut y), a) = v; [.. mut m] = arr; }",
    );
    // `is_all_caps_const` on single-char uppercase vs multi-char const
    let caps_rust = transpile_to_rust(
        "const int K = 1; void f() { int a = K.abs(); int b = MY_CONST.len(); }",
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .unwrap();
    assert!(caps_rust.contains("K.abs()"));
    assert!(caps_rust.contains("MY_CONST.len()"));
}

#[test]
fn from_rust_real_world_hints_filters_and_edge_cases() {
    // 1. TypeHintMap filtering: unnameable markers, Atomic<T> normalization,
    //    nested impl Trait, nested BareFn, unqualified QSelf trait, impl Trait with elided ref,
    //    unimported dyn Trait, unverified non-prelude paths, impl Trait containing dyn.
    let src = r#"
use std::sync::Arc;
use crate::ImportedTrait;
trait LocalTrait {}
fn test_hints(flag: bool) {
    let a0 = unknown_fn();
    let a1 = unknown_fn();
    let a2 = unknown_fn();
    let a3 = unknown_fn();
    let a4 = unknown_fn();
    let a5 = unknown_fn();
    let a6 = unknown_fn();
    let a7 = unknown_fn();
    let a8 = unknown_fn();
    let a9 = unknown_fn();
    let a10 = unknown_fn();
    let a11 = unknown_fn();
    let a12 = unknown_fn();
    let a13 = unknown_fn();
    let a14 = unknown_fn();
    let a15 = unknown_fn();
    let a16 = unknown_fn();
    let a17 = unknown_fn();
    let a18 = unknown_fn();
    let a19 = unknown_fn();
    let a20 = unknown_fn();
    let a21 = unknown_fn();
    let a22 = unknown_fn();
    let a23 = unknown_fn();
    let a24 = unknown_fn();
    let a25 = unknown_fn();
    let a26 = unknown_fn();
    let a27 = unknown_fn();
    let a28 = unknown_fn();
    let a29 = unknown_fn();
}
"#;
    let hint_specs: &[(&str, bool)] = &[
        ("{unknown}", true),
        ("{closure@a.rs:1:1}", true),
        ("std::sync::atomic::Atomic<bool>", true),
        ("std::sync::atomic::Atomic<i8>", true),
        ("std::sync::atomic::Atomic<i16>", true),
        ("std::sync::atomic::Atomic<i32>", true),
        ("std::sync::atomic::Atomic<i64>", true),
        ("std::sync::atomic::Atomic<isize>", true),
        ("std::sync::atomic::Atomic<u8>", true),
        ("std::sync::atomic::Atomic<u16>", true),
        ("std::sync::atomic::Atomic<u32>", true),
        ("std::sync::atomic::Atomic<u64>", true),
        ("std::sync::atomic::Atomic<usize>", true),
        ("std::sync::atomic::Atomic<*mut u8>", true),
        (
            "(&std::sync::atomic::Atomic<bool>, *const i32, [u8; 2], &[u8], (i32,))",
            true,
        ),
        ("impl Fn() -> impl Future<Output = i32>", true),
        ("Option<impl Clone>", true),
        ("Wrapper<fn(i32) -> i32>", true),
        ("<S as Serializer>::SerializeMap", true),
        ("impl Fn(&str) -> i32", true),
        ("impl Fn(i32) -> &str", true),
        ("impl Iterator<Item = &str>", true),
        ("impl Iterator<Item = &'a str>", true),
        ("Box<dyn UnimportedTrait + Send>", true),
        ("Box<dyn LocalTrait + Send>", true),
        ("Box<dyn ImportedTrait + Send>", true),
        ("UnimportedType", false),
        ("impl Fn(&dyn Send)", false),
        (
            "(std::sync::Arc<dyn UnimportedTrait>, [Box<dyn UnimportedTrait>; 1])",
            true,
        ),
        ("invalid {{{ type", true),
    ];
    let mut hints = TypeHintMap::default();
    for (idx, (ty, verified)) in hint_specs.iter().enumerate() {
        let line_no = (6 + idx) as u32;
        let name_len = format!("a{}", idx).len() as u32;
        hints.insert_with_verified(line_no, 8 + name_len, (*ty).to_string(), *verified);
    }
    let (rsh, _rust) = roundtrip_and_check(src, &hints, false);
    assert!(rsh.contains("AtomicBool a2"));
    assert!(rsh.contains("AtomicPtr<byte> a13"));
    assert!(rsh.contains("Box<dyn LocalTrait + Send> a24"));
    assert!(rsh.contains("Box<dyn ImportedTrait + Send> a25"));
    assert!(rsh.contains("var a15 ="), "got:\n{rsh}");
    assert!(rsh.contains("var a23 ="), "got:\n{rsh}");
    assert!(!rsh.contains("rust {"), "got:\n{rsh}");

    // 2. Pattern destructuring with hints: Tuple, Slice, TupleStruct, Struct, Reference, Paren, Wild, Rest
    let pat_src = r#"
struct Pt { x: i32, y: i32 }
struct Wrap(i32, i32);
fn pat_hints() {
    let (t0, _, ..) = make_tuple();
    let (solo,) = make_solo();
    let [s0, _, ..] = make_slice();
    let Wrap(w0, _) = make_wrap();
    let Pt { x, y: y2, .. } = make_pt();
    let &r0 = make_ref();
    let &mut r1 = make_mut_ref();
    let (p0) = make_paren();
    let _ = make_wild();
    let (_,) = make_only_wild();
    let [_, ..] = make_only_wild_slice();
    let Wrap(_) = make_only_wild_ts();
    let mut deferred;
    if cond { deferred = 42i32; } else { deferred = 7i32; }
    let deferred_blk;
    { deferred_blk = "hi"; }
    let deferred_match;
    match 1 { _ => deferred_match = true }
    let (da, db);
    da = 1;
    db = 2;
}
"#;
    let mut pat_hints = TypeHintMap::default();
    // Provide line/col hints for t0, solo, s0, w0, x, y2, r0, r1, p0, da, db
    for (line_idx, line) in pat_src.lines().enumerate() {
        let l1 = (line_idx + 1) as u32;
        for (ident, ty) in [
            ("t0", "i32"),
            ("solo", "i32"),
            ("s0", "i32"),
            ("w0", "i32"),
            ("x", "i32"),
            ("y2", "i32"),
            ("r0", "i32"),
            ("r1", "i32"),
            ("p0", "i32"),
            ("da", "i32"),
            ("db", "i32"),
        ] {
            if let Some(pos) = line.find(ident) {
                // Only match binding lines (`let ...`)
                if line.trim_start().starts_with("let ") {
                    pat_hints.insert(l1, (pos + ident.len()) as u32, ty.to_string());
                }
            }
        }
    }
    let (pat_rsh, _pat_rust) = roundtrip_and_check(pat_src, &pat_hints, false);
    assert!(pat_rsh.contains("(int t0, _, ..) = make_tuple();"));
    assert!(pat_rsh.contains("(int solo,) = make_solo();"));
    assert!(pat_rsh.contains("[int s0, _, ..] = make_slice();"));
    assert!(pat_rsh.contains("Wrap(int w0, _) = make_wrap();"));
    assert!(pat_rsh.contains("Pt { int x, y: int y2, .. } = make_pt();"));
    assert!(pat_rsh.contains("deref int r0 = make_ref();"));
    assert!(pat_rsh.contains("deref write int r1 = make_mut_ref();"));
    assert!(pat_rsh.contains("int da;"));
    assert!(pat_rsh.contains("int db;"));

    // 3. Closures, for loops, cfg-tail blocks/macros, unsafe tail, unlabeled loop jump,
    //    break values, local-var shadowing module path, member receivers, `Option<&'static str>`, `[&str; 2]`.
    let complex_src = r#"
mod api {
    pub const BUDGET: usize = 10;
    pub fn trace_ws() {}
}

#[cfg(test)]
mod tests {
    use super::api;
    pub fn check() -> usize { api::BUDGET }
}

fn cfg_tails() -> i32 {
    #[cfg(unix)]
    { 1 }
    #[cfg(not(unix))]
    { 2 }
}

fn macro_tail() -> i32 {
    my_macro! { 42 }
}

fn unsafe_tail(p: *const i32) -> i32 {
    unsafe { *p }
}

fn break_and_loop_jumps(items: &[i32]) -> i32 {
    let mut total: i32 = 0;
    for &x in items {
        let v: i32 = if x == 0 {
            continue;
        } else {
            x
        };
        let w: i32 = match x {
            1 => {
                let _y: i32 = { break; };
                0
            }
            q @ (b'"' | b'\'') => q as i32,
            c @ ('a'..='z') => c as i32,
            n @ 1..=9 => n,
            s @ "hi" => s.len() as i32,
            b @ true => b as i32,
            f @ 1.5 => f as i32,
            _ => {
                total += 1;
                -10
            }
        };
        total += v + w;
    }
    let res: i32 = loop {
        let out: i32 = total;
        break out;
    };
    let api: i32 = res;
    let _b: usize = api::BUDGET;
    let _o: Option<&'static str> = Some("a");
    let _arr: [&str; 2] = ["a", "b"];
    let _dyn_send: Box<dyn std::any::Any + Send> = Box::new(1i32);
    let _global: i32 = 1;
    let _try_recv: Option<usize> = Some("abc").as_deref().map(|s: &str| s.len());
    res + api + _global
}
"#;
    let (cx_rsh, cx_rust) = roundtrip_and_check(complex_src, &TypeHintMap::default(), false);
    assert!(cx_rsh.contains("return my_macro!{ 42 };"));
    assert!(cx_rsh.contains("api::BUDGET"));
    assert!(cx_rsh.contains("Option<read<static> str>"));
    assert!(cx_rsh.contains("(read str)[2]"));
    assert!(cx_rsh.contains("break (out);"));
    assert!(cx_rsh.contains("return -10;"), "got:\n{cx_rsh}");
    assert!(!cx_rsh.contains("__blk_val"), "got:\n{cx_rsh}");
    assert!(cx_rsh.contains("Box<dyn std.any.Any + Send>"));
    assert!(cx_rust.contains("api::BUDGET"));
    assert!(cx_rust.contains("return my_macro!{ 42 };"));

    // 4. Closure parameter hints & strict vs non-strict fallback
    let clo_src = "fn f() { let _ = [1].iter().copied().map(|x| x + 1); }";
    let mut clo_hints = TypeHintMap::default();
    clo_hints.insert(1, 43, "i32".to_string());
    let (clo_rsh, _) = roundtrip_and_check(clo_src, &clo_hints, false);
    assert!(clo_rsh.contains("(int x) =>"), "got:\n{clo_rsh}");

    // Elided-ref closure param hint (`&str`) must stay inferred when strict=false so Rust
    // return-lifetime unification (`let id = |x| x;`) keeps compiling, while `read_count`
    // (identifier containing `read`) is not misdetected as a `read ` keyword.
    let clo_ref_src = "fn f() -> &'static str { let id = |x| x; id(\"ok\") }";
    let mut clo_ref_hints = TypeHintMap::default();
    clo_ref_hints.insert(1, 36, "&str".to_string());
    let (clo_ref_rsh, _) = roundtrip_and_check(clo_ref_src, &clo_ref_hints, false);
    assert!(clo_ref_rsh.contains("(x) =>"), "got:\n{clo_ref_rsh}");
    let clo_ref_strict_src = "fn f() { let _ = [\"a\"].iter().map(|x| x); }";
    let mut clo_ref_strict_hints = TypeHintMap::default();
    clo_ref_strict_hints.insert(1, 36, "&str".to_string());
    let (clo_ref_strict, _) = roundtrip_and_check(clo_ref_strict_src, &clo_ref_strict_hints, true);
    assert!(
        clo_ref_strict.contains("(read str x) =>"),
        "got:\n{clo_ref_strict}"
    );
    let mut clo_opt_ref_hints = TypeHintMap::default();
    clo_opt_ref_hints.insert(1, 36, "Option<&str>".to_string());
    let (clo_opt_ref_rsh, _) = roundtrip_and_check(clo_ref_src, &clo_opt_ref_hints, false);
    assert!(
        clo_opt_ref_rsh.contains("(x) =>"),
        "got:\n{clo_opt_ref_rsh}"
    );
    let mut non_ref_read_ident_hints = TypeHintMap::default();
    non_ref_read_ident_hints.insert(1, 43, "unread".to_string());
    let (unread_rsh, _) = roundtrip_and_check(clo_src, &non_ref_read_ident_hints, false);
    assert!(unread_rsh.contains("(unread x) =>"), "got:\n{unread_rsh}");

    // Bare fn pointer hint with elided ref input (`fn(&str) -> i32`) falls back to `var`
    // so generic fn items do not fail E0308 HRTB coercion.
    let fn_ptr_src = "fn g<T: AsRef<str>>(_: T) -> i32 { 1 }\nfn f(c: bool) -> i32 { let r = if c { g } else { g }; r(\"a\") }";
    let mut fn_ptr_hints = TypeHintMap::default();
    fn_ptr_hints.insert(2, 28, "fn(&str) -> i32".to_string());
    let (fn_ptr_rsh, _) = roundtrip_and_check(fn_ptr_src, &fn_ptr_hints, false);
    assert!(fn_ptr_rsh.contains("var r ="), "got:\n{fn_ptr_rsh}");

    // Unnameable closure param: strict=false -> native inferred `(x) =>`, strict=true -> Err
    let mut bad_clo_hints = TypeHintMap::default();
    bad_clo_hints.insert(1, 43, "{unknown}".to_string());
    let (fb_rsh, _) = roundtrip_and_check(clo_src, &bad_clo_hints, false);
    assert!(fb_rsh.contains("(x) =>"), "got:\n{fb_rsh}");
    assert!(!fb_rsh.contains("rust {"), "got:\n{fb_rsh}");
    assert!(
        convert_from_rust_with_hints(
            clo_src,
            FromRustOptions {
                use_rustc_inference: false
            },
            &bad_clo_hints,
            true
        )
        .is_err()
    );
    // Non-strict closure param with no hints uses native inferred `(x) =>`
    let (no_hint_clo, _) = roundtrip_and_check(clo_src, &TypeHintMap::default(), false);
    assert!(no_hint_clo.contains("(x) =>"), "got:\n{no_hint_clo}");
    assert!(!no_hint_clo.contains("rust {"), "got:\n{no_hint_clo}");

    // 5. `for` loop with verified vs unverified hints
    let for_src = "fn f(xs: &[i32]) { for x in xs { let _ = x; } }";
    let mut for_hints = TypeHintMap::default();
    for_hints.insert_with_verified(1, 24, "&i32".to_string(), true);
    let (for_rsh, _) = roundtrip_and_check(for_src, &for_hints, false);
    assert!(for_rsh.contains("foreach (read int x in xs)"));
    let mut for_unver = TypeHintMap::default();
    for_unver.insert_with_verified(1, 24, "Unimported".to_string(), false);
    let (for_rsh2, _) = roundtrip_and_check(for_src, &for_unver, false);
    assert!(for_rsh2.contains("foreach (x in xs)"));

    // 6. Fast-fail rustc probe (second unresolvable binding doesn't re-invoke rustc)
    let multi_fail = r#"
fn multi() {
    let a = nonexistent_crate::foo();
    let b = nonexistent_crate::bar();
    let _ = [1].iter().map(|p| nonexistent_crate::baz(p));
}
"#;
    assert!(
        convert_from_rust(
            multi_fail,
            FromRustOptions {
                use_rustc_inference: true
            }
        )
        .is_err()
    );

    // 7. Nested type-filter helper branches (Ptr, Slice, Array, Paren, Tuple, QSelf, AssocType, Parenthesized)
    let deep_src = r#"
fn deep() {
    let d0 = u();
    let d1 = u();
    let d2 = u();
    let d3 = u();
    let d4 = u();
    let d5 = u();
    let d6 = u();
    let d7 = u();
    let d8 = u();
    let d9 = u();
    let d10 = u();
    let d11 = u();
}
"#;
    let deep_specs: &[(&str, bool)] = &[
        ("*const impl Clone", true),
        ("&[impl Clone]", true),
        ("[impl Clone; 2]", true),
        ("(impl Clone,)", true),
        ("((impl Clone))", true),
        ("<impl Clone as Trait>::Out", true),
        ("Foo<Item = impl Clone>", true),
        ("Foo(impl Clone) -> i32", true),
        ("Foo(i32) -> impl Clone", true),
        ("Foo<Bar = fn()>", true),
        ("(*const fn(), &[fn()], [fn(); 1], (fn(),), ((fn())))", true),
        ("<fn() as Tr>::Out", true),
    ];
    let mut deep_hints = TypeHintMap::default();
    for (idx, (ty, v)) in deep_specs.iter().enumerate() {
        let line_no = (3 + idx) as u32;
        let col = 8 + format!("d{}", idx).len() as u32;
        deep_hints.insert_with_verified(line_no, col, (*ty).to_string(), *v);
    }
    let (deep_rsh, _) = roundtrip_and_check(deep_src, &deep_hints, false);
    assert!(deep_rsh.contains("var d0 ="), "got:\n{deep_rsh}");
    assert!(deep_rsh.contains("var d11 ="), "got:\n{deep_rsh}");
    assert!(!deep_rsh.contains("rust {"), "got:\n{deep_rsh}");

    // 8. Additional from_rust edge branches for 100% line/region coverage:
    //    unverified hints in `try_fmt_pat_with_hints` (all_unverified_paths_in_scope & impl_trait_contains_dyn),
    //    Pat::Wild/Paren/Struct/Lit in try_fmt_pat_with_hints, Stmt::Macro in Break/None/cfg tail,
    //    outer attrs on if/match in tail & stmt pos, emit_local fallbacks (struct/tuplestruct/ref/tuple/deferred/strict),
    //    fmt_condition Expr::Let with if RHS, Expr::Unsafe ending in semi, expr_has_unlabeled_loop_jump variants,
    //    normalize_unstable_stdlib_type edge cases, collect_use_root_ident variants, is_boxed_any non-Any.
    let extra_rust_src = r#"
use *;
use foo;
use bar as baz;
use std::string::String;
use qux::*;
struct Known { a: i32, b: i32 }
fn extra_edges(opt: Option<i32>, r: &mut i32, end_n: usize) -> i32 {
    #[allow(unused)]
    if true {
        let _ = 1;
    }
    let _b1: i32 = {
        my_blk_mac! { 1 }
    };
    let _b2: () = unsafe {
        foo();
    };
    let _o = Some(1);
    let _d1;
    foo();
    if true { other = 1; } else { _d1 = 2i32; }
    let _d2;
    let _u1 = unknown_call();
    let UnknownStruct { x, y } = u();
    let UnknownStruct { x: renamed } = u();
    let UnknownTuple(a) = u();
    let &ref_u = u();
    let &mut ref_m = r;
    let (da, db);
    let s1 = 1i32;
    let (mut s1, s2);
    let (tu1, tu2) = (u(), 1i32);
    let (tu3, tu4) = u();
    let (tu5, tu6) = u();
    let (tu7, tu8) = (1i32, 2i32);
    let Known { a: _, .. } = u();
    let Known { a: ka, b: kb } = u();
    let Known { .. } = u();
    let _k_def: i32 = Kind::A.def();
    let _try_m: String = opt?.to_string();
    let _any_other: Box<dyn Clone> = u();
    let (nested_a, (nested_b, nested_c)): (i32, (i32, i32)) = (1, (2, 3));
    let _ = (nested_a, nested_b, nested_c, ka, kb, tu7, tu8);
    for i in 0..end_n {
        let _ = i;
    }
    for j in 1u32..5u32 {
        let _ = j;
    }
    if let Some(v) = if true { opt } else { None } {
        let _ = v;
    }
    for _ in [1] {
        let _j1: i32 = {
            let uninit;
            let Some(v) = opt else { break; };
            uninit = v;
            uninit
        };
        let _j2: i32 = {
            fn inner() {}
            loop { break; }
            let _ = unsafe { continue; };
            0
        };
        let _j3: i32 = {
            let _ = 0;
            match opt {
                Some(x) if { break; } => x,
                _ => 0,
            }
        };
        let _j4: i32 = {
            let _ = 0;
            match opt {
                Some(x) => x,
                None => (break),
            }
        };
        let _j5: i32 = {
            let _ = &(break);
            0
        };
        let _j6: i32 = {
            let _ = (break)?;
            0
        };
        let _j7: i32 = {
            let _ = (break).await;
            0
        };
        let _j8: i32 = {
            return break;
        };
        let _j9: i32 = {
            let _ = [break];
            0
        };
        let _j10: i32 = {
            let _ = (break,);
            0
        };
    }
    match 1 {
        q_hint @ 1 => q_hint,
        c @ 'a' => c as i32,
        n @ 1u32 => n as i32,
        bs @ b"hi" => bs.len() as i32,
        rb @ (b'a'..=b'z') => rb as i32,
        rf @ (1.0..=2.0) => rf as i32,
        pair @ (1, 2) => pair.0,
        _ => 0,
    };
    let (q_ty @ 1,): (i32,) = (1,);
    let _ = q_ty;
    [1].iter().map(|_| 1);
    [1].iter().map(|_| 1);
    [1].iter().map(|_| 1);
    [1].iter().map(|(p)| p);
    [1].iter().map(|(p)| p);
    [1].iter().map(|1| 1);
    #[allow(unused)]
    match opt {
        Some(v) => v,
        None => {
            #[allow(unused)]
            if true { 1 } else { 2 }
        }
    }
}
fn cfg_macro_tails() -> i32 {
    #[cfg(unix)]
    mac_a! { 1 }
    #[cfg(not(unix))]
    mac_b! { 2 }
}
fn void_macro_tail() {
    mac_c! { 1 }
}
"#;
    let mut eh = TypeHintMap::default();
    let mut wild_idx = 0usize;
    let mut paren_idx = 0usize;
    for (idx, line) in extra_rust_src.lines().enumerate() {
        let l1 = (idx + 1) as u32;
        if line.contains("let (mut s1, s2);") {
            let p1 = line.find("s1").unwrap();
            let p2 = line.find("s2").unwrap();
            eh.insert(l1, (p1 + 2) as u32, "i32".to_string());
            eh.insert(l1, (p2 + 2) as u32, "i32".to_string());
        } else if line.contains("let (tu3, tu4) = u();") {
            let p1 = line.find("tu3").unwrap();
            let p2 = line.find("tu4").unwrap();
            eh.insert_with_verified(l1, (p1 + 3) as u32, "{unknown}".to_string(), false);
            eh.insert_with_verified(l1, (p2 + 3) as u32, "i32".to_string(), true);
        } else if line.contains("let (tu5, tu6) = u();") {
            let p1 = line.find("tu5").unwrap();
            let p2 = line.find("tu6").unwrap();
            eh.insert_with_verified(
                l1,
                (p1 + 3) as u32,
                "impl Fn(Box<dyn Send>)".to_string(),
                false,
            );
            eh.insert_with_verified(l1, (p2 + 3) as u32, "UnimportedX".to_string(), false);
        } else if line.contains("let Known { a: ka, b: kb } = u();") {
            let p1 = line.find("ka").unwrap();
            let p2 = line.find("kb").unwrap();
            eh.insert(l1, (p1 + 2) as u32, "i32".to_string());
            eh.insert(l1, (p2 + 2) as u32, "i32".to_string());
        } else if line.contains("q_hint @ 1") {
            let p = line.find("q_hint").unwrap();
            eh.insert(l1, (p + 6) as u32, "i32".to_string());
        } else if line.contains(".map(|_| 1);") {
            let p = line.find("|_|").unwrap() + 2;
            if wild_idx == 0 {
                eh.insert(l1, p as u32, "i32".to_string());
            } else if wild_idx == 1 {
                eh.insert(l1, p as u32, "{unknown}".to_string());
            }
            wild_idx += 1;
        } else if line.contains(".map(|(p)| p);") {
            let p = line.find("|(p)|").unwrap() + 3;
            if paren_idx == 0 {
                eh.insert(l1, p as u32, "i32".to_string());
            } else {
                eh.insert(l1, p as u32, "{unknown}".to_string());
            }
            paren_idx += 1;
        }
    }
    let (extra_rsh, _) = roundtrip_and_check(extra_rust_src, &eh, false);
    assert!(extra_rsh.contains("return my_blk_mac!{ 1 };"));
    assert!(extra_rsh.contains("return mac_a!{ 1 };"));
    assert!(extra_rsh.contains("return mac_b!{ 2 };"));
    assert!(extra_rsh.contains("shadow mut int s1;"));
    assert!(extra_rsh.contains("deref write int ref_m = r;"));

    // No-hints local inference: `Some("hi")` -> `Option<read str>`, `for j in 1u32..5u32`,
    // nested tuple & `@` subpattern `ty_hint` propagation.
    let (no_hint_rsh, _) = roundtrip_and_check(
        r#"
fn no_hints() {
    let o_ref = Some("hi");
    for j in 1u32..5u32 { let _ = j; }
    let (q_ty @ 1, (pa, pb)) = (1i32, (2i32, 3i32));
    let _ = (o_ref, q_ty, pa, pb);
}
"#,
        &TypeHintMap::default(),
        false,
    );
    assert!(no_hint_rsh.contains("Option<read str> o_ref = Some(\"hi\");"));
    assert!(no_hint_rsh.contains("foreach (uint j in 1u32..5u32)"));
    assert!(no_hint_rsh.contains("(1 as int q_ty, (int pa, int pb))"));

    // Strict mode error on unnameable binding + non-ident binding + unverified paths in scope checks
    let strict_src = "fn f() {\n    let x = u();\n    let 1 = 1;\n}\n";
    let mut sh = TypeHintMap::default();
    sh.insert(2, 9, "{unknown}".to_string());
    assert!(
        convert_from_rust_with_hints(
            strict_src,
            FromRustOptions {
                use_rustc_inference: false
            },
            &sh,
            true
        )
        .is_err()
    );
    let _ = convert_from_rust_with_hints(
        "fn f() { let 1 = 1; }",
        FromRustOptions {
            use_rustc_inference: false,
        },
        &TypeHintMap::default(),
        false,
    );

    // Additional hint types hitting all remaining branches of `any_child_type`, `all_unverified_paths_in_scope`,
    // `normalize_unstable_stdlib_type`, `impl_trait_returns_elided_ref`
    let filter_src = r#"
trait LocalTrait {}
fn flt() {
    let (f0,) = u();
    let (f1,) = u();
    let (f2,) = u();
    let (f3,) = u();
    let (f4,) = u();
    let (f5,) = u();
    let (f6,) = u();
    let (f7,) = u();
    let (f8,) = u();
    let (f9,) = u();
    let (f10,) = u();
    let (f11,) = u();
    let (f12,) = u();
    let (f13,) = u();
    let (f14,) = u();
    let (f15,) = u();
}
"#;
    let filter_specs: &[(&str, bool)] = &[
        ("impl Iterator<Item = i32> + 'static", false),
        ("impl Copy + 'a", false),
        ("impl FnOnce() -> &'static str", false),
        ("impl TraitWithUnimported<Item = i32>", false),
        ("Box<dyn UnimportedDyn>", false),
        ("Box<dyn Send + 'static>", false),
        ("fn(i32) -> !", false),
        ("std::option::Option<_>", false),
        ("std::sync::atomic::Atomic<Custom>", true),
        ("std::sync::atomic::Atomic<*const u8>", true),
        (
            "Iterator<Item = std::sync::atomic::Atomic<bool>, 'static>",
            true,
        ),
        (
            "std::result::Result<<std::vec::Vec<i32> as std::iter::Iterator>::Item, i32>",
            false,
        ),
        ("fn() -> UnimportedRet", false),
        ("impl Borrow<'a>", true),
        ("impl FnOnce() -> Foo<'a>", true),
        ("Box<dyn 'static + LocalTrait>", false),
    ];
    let mut fh = TypeHintMap::default();
    for (idx, (ty, v)) in filter_specs.iter().enumerate() {
        let l1 = (4 + idx) as u32;
        let name = format!("f{}", idx);
        let col = 9 + name.len() as u32;
        fh.insert_with_verified(l1, col, (*ty).to_string(), *v);
    }
    let _ = roundtrip_and_check(filter_src, &fh, false);
}

#[test]
fn github_10_repos_regressions_all_9_bugs() {
    // Bug 1: Multi-line inner `/*! ... */` and outer `/** ... */` block doc comments
    let doc_src = r#"/*!
First line with `backticks` and (parens).

Second paragraph.
*/
/**
Outer multi-line doc with `code`.

More text.
*/
pub fn documented() -> i32 {
    1
}
"#;
    let (_doc_rsh, doc_rust) = roundtrip_and_check(doc_src, &TypeHintMap::default(), false);
    assert!(doc_rust.contains("//! First line with `backticks`"));
    assert!(doc_rust.contains("//! Second paragraph."));
    assert!(doc_rust.contains("/// Outer multi-line doc with `code`."));
    assert!(doc_rust.contains("/// More text."));

    // Bug 2: `unsafe { ... }` expression used as method-call / postfix receiver
    let unsafe_recv_src = r#"
struct Rec;
impl Rec {
    fn record(&mut self, v: usize) -> usize { v }
}
fn call_unsafe_recv(mut r: Rec) -> usize {
    let p: *mut Rec = &mut r;
    unsafe { &mut *p }.record(42)
}
fn stmt_unsafe_recv(mut r: Rec) {
    let p: *mut Rec = &mut r;
    unsafe { &mut *p }.record(7);
}
"#;
    let (_ur_rsh, ur_rust) = roundtrip_and_check(unsafe_recv_src, &TypeHintMap::default(), false);
    assert!(ur_rust.contains(".record(42)"));
    assert!(ur_rust.contains(".record(7);"));

    // Bug 3: Rust lifetimes and raw tokens inside custom attribute arguments
    let attr_src = r#"
#[my_attr("{}", { struct Msg<'a>(&'a String, &'a Option<usize>); Msg(&s, &n) })]
pub struct AttributedWithLifetimes;
"#;
    let (_attr_rsh, attr_rust) = roundtrip_and_check(attr_src, &TypeHintMap::default(), false);
    assert!(
        attr_rust.contains("struct Msg") && attr_rust.contains("'a"),
        "got:\n{attr_rust}"
    );

    // Bug 5: `alloc::string` module paths, `hashbrown::HashMap` imports, and `string` pattern bindings
    let alias_src = r#"
use alloc::string::{String, ToString};
use alloc::string::String as AllocString;
use alloc::string;
use hashbrown::HashMap;

enum Token {
    Text(String),
}

fn use_aliases(tok: Token, m: HashMap<i32, i32>) -> alloc::string::String {
    let _ = m;
    let mut out: alloc::string::String = alloc::string::ToString::to_string(&42);
    match tok {
        Token::Text(string) => out.push_str(&string),
    }
    out
}
"#;
    let (_al_rsh, al_rust) = roundtrip_and_check(alias_src, &TypeHintMap::default(), false);
    assert!(
        al_rust.contains("use alloc::string::{String, ToString};"),
        "got:\n{al_rust}"
    );
    assert!(
        al_rust.contains("use alloc::string::String as AllocString;"),
        "got:\n{al_rust}"
    );
    assert!(al_rust.contains("use alloc::string;"), "got:\n{al_rust}");
    assert!(
        al_rust.contains("use hashbrown::HashMap;"),
        "got:\n{al_rust}"
    );
    assert!(
        al_rust.contains("alloc::string::ToString::to_string"),
        "got:\n{al_rust}"
    );
    assert!(
        al_rust.contains("-> alloc::string::String"),
        "got:\n{al_rust}"
    );
    assert!(
        al_rust.contains("Token::Text(string) =>"),
        "got:\n{al_rust}"
    );

    // Bug 6: `#[cfg(...)]` on tail return/break expression hoisted outside `return` / `break`
    let cfg_tail_src = r#"
fn cfg_tail_expr(x: i32) -> bool {
    #[cfg(not(feature = "alt"))]
    match x {
        0 => true,
        _ => false,
    }
    #[cfg(feature = "alt")]
    x.is_positive()
}
fn cfg_tail_unsafe(b: &[u8]) -> &[u8] {
    #[cfg(debug_assertions)]
    { &b[0..1] }
    #[cfg(not(debug_assertions))]
    unsafe { b.get_unchecked(0..1) }
}
"#;
    let (_cfg_rsh, cfg_rust) = roundtrip_and_check(cfg_tail_src, &TypeHintMap::default(), false);
    assert!(
        !cfg_rust.contains("return { #[cfg("),
        "cfg must be hoisted outside return:\n{cfg_rust}"
    );
    assert!(
        cfg_rust.contains("#[cfg(feature = \"alt\")]\n    return x.is_positive();"),
        "got:\n{cfg_rust}"
    );

    // Bug 7: Shorthand `mut` binding in struct destructuring pattern
    let mut_pat_src = r#"
enum State {
    Linked { next: Option<i32> },
}
fn drain(state: &mut State) -> i32 {
    match state {
        State::Linked { mut next } => {
            let mut count: i32 = 0;
            while let Some(v) = next {
                next = if v > 0 { Some(v - 1) } else { None };
                count += 1;
            }
            count
        }
    }
}
"#;
    let (_mp_rsh, mp_rust) = roundtrip_and_check(mut_pat_src, &TypeHintMap::default(), false);
    assert!(
        mp_rust.contains("State::Linked { mut next } =>"),
        "got:\n{mp_rust}"
    );

    // Bug 8: `#![deny(warnings)]` and `break 'lbl ('__blk: { ... })` parenthesization
    let deny_src = r#"#![deny(warnings)]
pub fn check_deny(x: i32) -> i32 {
    'outer: loop {
        if x > 0 {
            break 'outer {
                let y: i32 = x + 1;
                y
            };
        }
        return 0;
    }
}
"#;
    let (_dn_rsh, dn_rust) = roundtrip_and_check(deny_src, &TypeHintMap::default(), false);
    assert!(
        dn_rust.contains("#![allow(unused_parens, unused_braces, unreachable_code)]"),
        "got:\n{dn_rust}"
    );
    assert!(dn_rust.contains("break 'outer {\n"), "got:\n{dn_rust}");
    assert!(!dn_rust.contains("__blk_val"), "got:\n{dn_rust}");

    // Bug 9: `ref` pattern binding with rust-analyzer hint `&[u8; 15]` must not double-wrap `&`
    let ref_pat_src = r#"struct InlineExt([u8; 15], u8);
impl InlineExt {
    fn as_bytes(&self) -> &[u8] {
        let InlineExt(ref data, len) = self;
        &data[..*len as usize]
    }
}
"#;
    let mut ref_hints = TypeHintMap::default();
    let line4 = ref_pat_src.lines().nth(3).unwrap();
    let data_end = (line4.find("data").unwrap() + 4) as u32;
    let len_end = (line4.find("len").unwrap() + 3) as u32;
    ref_hints.insert(4, data_end, "&[u8; 15]".to_string());
    ref_hints.insert(4, len_end, "&u8".to_string());
    let (_rp_rsh, rp_rust) = roundtrip_and_check(ref_pat_src, &ref_hints, false);
    assert!(
        rp_rust.contains("let _: &&[u8; 15] = &data;"),
        "got:\n{rp_rust}"
    );
    assert!(!rp_rust.contains("&&&[u8; 15]"), "got:\n{rp_rust}");

    // Stage C extra regressions (ripgrep, thiserror, minijinja, rayon, bytes, anyhow):
    // 1. Trait AssocType and AssocConst doc comments preserved (#![deny(missing_docs)])
    // 2. Closure & unsafe block macro tail (`quote!{ ... }`) treated as return value
    // 3. `#[cfg(...)]` on array/tuple/call elements emitted directly without `{ ... }`
    // 4. `void` fn tail call emits `return ...;` to preserve `()` inference (e.g. `.collect()`)
    // 5. Range operand inside binary expression parenthesized (`r == (start..=end)`)
    // 6. `(&mut a[..]).chain_mut(...)` parsed as expression, not UFCS array type `<&mut [a; ..]>`
    // 7. `let size = layout.size() + 1;` does not mis-infer `i32` from unsuffixed literal
    // 8. `impl Fn() -> !` skips unstable `Fn() -> !` bound in `__rsharp_impl_check`
    // 9. Renamed `use std::error::Error as StdError;` rejects shadowed `Box<dyn Error>` hint
    let stage_c_src = r#"
use std::error::Error as StdError;
use anyhow::Error;

pub trait Matcher {
    /// Captures type doc.
    type Captures;
    /// Default limit doc.
    const LIMIT: usize = 1;
}

fn macro_closure_and_unsafe(opt: Option<i32>) -> Option<i32> {
    let a: Option<i32> = opt.map(|x: i32| {
        id_mac! { x }
    });
    let b: i32 = unsafe { id_mac! { 1 } };
    let _ = b;
    a
}

static CFG_ITEMS: &[(&str, &str)] = &[
    #[cfg(feature = "a")]
    ("a", "A"),
    ("b", "B"),
];

fn cfg_tuple_and_call() {
    let t: (i32,) = (#[cfg(not(test))] 1,);
    let v: Vec<i32> = vec![#[cfg(not(test))] 1];
    let _ = (t, v);
    take_arg(#[cfg(not(test))] 1);
}

fn unit_tail_collect(items: std::vec::IntoIter<()>) {
    items.collect()
}

fn range_cmp(r: std::ops::RangeInclusive<i32>, s: i32, e: i32) -> bool {
    r == (s..=e) && (s..=e) == r
}

fn mut_slice_method(mut a: [u8; 4]) -> usize {
    (&mut a[..]).len()
}

fn unsuffixed_binary(layout_size: usize) {
    let s1 = unknown_fn() + 1;
    let s2 = 1 + unknown_fn();
    let s3 = (1) + 2usize;
    let s4 = 1.0 + unknown_fn();
    let s5 = layout_size + 1;
    let _ = (s1, s2, s3, s4, s5);
}

fn shadowed_dyn_and_never_fn() {
    let boxed = Box::<dyn StdError>::from("err");
    let f = || -> ! { panic!() };
    let _ = (boxed, f);
}
"#;
    let mut sc_hints = TypeHintMap::default();
    for (idx, line) in stage_c_src.lines().enumerate() {
        let l1 = (idx + 1) as u32;
        if let Some(pos) = line.find("let boxed =") {
            sc_hints.insert(l1, (pos + 9) as u32, "Box<dyn Error>".to_string());
        } else if let Some(pos) = line.find("let f =") {
            sc_hints.insert(l1, (pos + 5) as u32, "impl Fn() -> !".to_string());
        }
    }
    let (sc_rsh, sc_rust) = roundtrip_and_check(stage_c_src, &sc_hints, false);
    assert!(
        sc_rust.contains("/// Captures type doc.\n    type Captures;"),
        "got:\n{sc_rust}"
    );
    assert!(
        sc_rust.contains("/// Default limit doc.\n    const LIMIT: usize = 1;"),
        "got:\n{sc_rust}"
    );
    assert!(sc_rust.contains("id_mac!{ x }\n"), "got:\n{sc_rust}");
    assert!(!sc_rust.contains("id_mac!{ x };"), "got:\n{sc_rust}");
    assert!(
        sc_rust.contains("#[cfg(feature = \"a\")] (\"a\", \"A\")"),
        "got:\n{sc_rust}"
    );
    assert!(
        !sc_rust.contains("{ #[cfg(feature = \"a\")]"),
        "got:\n{sc_rust}"
    );
    assert!(
        sc_rust.contains("return items.collect();"),
        "got:\n{sc_rust}"
    );
    assert!(
        sc_rust.contains("(r == (s..=e))") && sc_rust.contains("((s..=e) == r)"),
        "got:\n{sc_rust}"
    );
    assert!(sc_rust.contains("(&mut a[..]).len()"), "got:\n{sc_rust}");
    assert!(
        sc_rsh.contains("var s1 = (unknown_fn() + 1);"),
        "got:\n{sc_rsh}"
    );
    assert!(
        sc_rsh.contains("var s2 = (1 + unknown_fn());"),
        "got:\n{sc_rsh}"
    );
    assert!(
        sc_rsh.contains("nuint s3 = ((1) + 2usize);"),
        "got:\n{sc_rsh}"
    );
    assert!(
        sc_rsh.contains("var s4 = (1.0 + unknown_fn());"),
        "got:\n{sc_rsh}"
    );
    assert!(
        sc_rsh.contains("nuint s5 = (layout_size + 1);"),
        "got:\n{sc_rsh}"
    );
    assert!(sc_rsh.contains("var boxed ="), "got:\n{sc_rsh}");
    assert!(!sc_rsh.contains("rust {"), "got:\n{sc_rsh}");
    assert!(!sc_rust.contains("Fn() -> !"), "got:\n{sc_rust}");

    // Final Stage C wave regressions (thiserror r#field, ripgrep self:: + static dyn ref,
    // ratatui/minijinja List shadowing + None::<T>.method() + (a.powf(b)).into()):
    let wave2_src = r#"
pub struct NotSource {
    r#source: char,
    destination: char,
}

pub struct List<'a> {
    pub items: Vec<&'a str>,
}

struct Extractor;
struct Literal;
impl Literal {
    fn exact(v: Vec<u8>) -> Self { let _ = v; Literal }
}
impl Extractor {
    fn extract(&self) -> Literal {
        self::Literal::exact(vec![])
    }
}

fn turbofish_none_and_paren_call(a: f64, b: f64) -> f64 {
    let _ = None::<&i32>.is_none();
    (a.powf(b)).abs()
}

fn static_dyn_ref_hint(read_from: &mut dyn std::io::Read) {
    let dec = wrap(read_from);
    let _ = dec;
}
"#;
    let mut w2_hints = TypeHintMap::default();
    for (idx, line) in wave2_src.lines().enumerate() {
        if let Some(pos) = line.find("let dec =") {
            w2_hints.insert(
                (idx + 1) as u32,
                (pos + 7) as u32,
                "Wrapper<&mut (dyn std::io::Read + 'static)>".to_string(),
            );
        }
    }
    let (w2_rsh, w2_rust) = roundtrip_and_check(wave2_src, &w2_hints, false);
    assert!(w2_rust.contains("r#source: char,"), "got:\n{w2_rust}");
    assert!(
        w2_rust.contains("pub items: Vec<&'a str>,"),
        "got:\n{w2_rust}"
    );
    assert!(w2_rust.contains("self::Literal::exact("), "got:\n{w2_rust}");
    assert!(
        w2_rust.contains("(None::<&i32>).is_none()"),
        "got:\n{w2_rust}"
    );
    assert!(w2_rust.contains("a.powf(b).abs()"), "got:\n{w2_rust}");
    assert!(
        w2_rsh.contains("var dec = wrap(read_from);"),
        "got:\n{w2_rsh}"
    );
    assert!(!w2_rsh.contains("rust {"), "got:\n{w2_rsh}");

    // Imported `List` shadowing `Vec` alias in another module (`ratatui-widgets/src/list/rendering.rs`)
    let imported_list_src = r#"
use crate::list::{List, ListState};
impl List<'_> {
    fn count(&self) -> usize {
        let items: Vec<i32> = Vec::new();
        items.len()
    }
}
"#;
    let (_il_rsh, il_rust) = roundtrip_and_check(imported_list_src, &TypeHintMap::default(), false);
    assert!(il_rust.contains("impl List<'_>"), "got:\n{il_rust}");
    assert!(
        il_rust.contains("let items: Vec<i32> = Vec::new();"),
        "got:\n{il_rust}"
    );

    // Final minijinja + ratatui regressions:
    // 1. `pub unsafe fn borrow_mut() -> (&Owner, &mut Dep)` (unsafe fn returning tuple, NOT unsafe fn ptr)
    // 2. `List::new()` in `mod tests` when `struct List` is in scope
    // 3. Packed struct aligned field copy `&{ val.0 }`
    // 4. Local macro expanding to `continue;` inside value-position `if` / `{ ... }`
    // 5. Inactive `#[cfg]` unsuffixed numeric literal falls back when RA hints are present
    let wave3_src = r#"
#[repr(packed)]
struct Packed(u128);

pub struct List<'a>(&'a str);
impl<'a> List<'a> {
    pub fn new(s: &'a str) -> Self { List(s) }
}
mod tests {
    use super::*;
    fn make() -> List<'static> { List::new("a") }
}

pub unsafe fn borrow_mut<'a>(a: &'a i32, b: &'a mut i32) -> (&'a i32, &'a mut i32) {
    (a, b)
}

fn packed_copy(p: Packed) -> u128 {
    let r: &u128 = &{ p.0 };
    *r
}

fn loop_jump_macro_test(mut n: i32) -> i32 {
    macro_rules! step {
        () => {{
            n -= 1;
            continue;
        }};
    }
    loop {
        let v: i32 = if n > 0 {
            let x: i32 = n;
            let _ = x;
            step!()
        } else {
            break n
        };
        let _ = match v {
            0 => {
                let y: i32 = 1;
                let _ = y;
                step!();
            }
            _ => {}
        };
    }
}

fn cfg_inactive_int(active: i32) -> usize {
    let a = active;
    let _ = a;
    #[cfg(not(feature = "fast"))]
    {
        let mut start = 0;
        start += 1;
        return start;
    }
    0
}
"#;
    let mut w3_hints = TypeHintMap::default();
    for (idx, line) in wave3_src.lines().enumerate() {
        if let Some(pos) = line.find("let a = active;") {
            w3_hints.insert((idx + 1) as u32, (pos + 5) as u32, "i32".to_string());
        }
    }
    let (w3_rsh, w3_rust) = roundtrip_and_check(wave3_src, &w3_hints, false);
    assert!(
        w3_rust.contains(
            "pub unsafe fn borrow_mut<'a>(a: &'a i32, b: &'a mut i32) -> (&'a i32, &'a mut i32)"
        ),
        "got:\n{w3_rust}"
    );
    assert!(w3_rust.contains("List::new(\"a\")"), "got:\n{w3_rust}");
    assert!(w3_rsh.contains("read { return p.0; }"), "got:\n{w3_rsh}");
    assert!(w3_rust.contains("p.0"), "got:\n{w3_rust}");
    assert!(!w3_rust.contains("'__if_val:"), "got:\n{w3_rust}");
    assert!(!w3_rsh.contains("rust {"), "got:\n{w3_rsh}");
    assert!(
        w3_rust.contains("let y: i32 = 1;") && w3_rust.contains("step!();"),
        "got:\n{w3_rust}"
    );
    assert!(w3_rsh.contains("mut var start = 0;"), "got:\n{w3_rsh}");

    // Non-std `Error` import (e.g. `use minijinja::Error;`) must not treat `dyn Error` as in-scope trait
    let non_std_err_src = r#"
use minijinja::Error;
fn inspect(err: &Error) {
    let source = std::error::Error::source(err).unwrap();
    let _ = source;
}
"#;
    let mut nse_hints = TypeHintMap::default();
    let l4 = non_std_err_src.lines().nth(3).unwrap();
    nse_hints.insert(
        4,
        (l4.find("source").unwrap() + 6) as u32,
        "&dyn Error".to_string(),
    );
    let (nse_rsh, _) = roundtrip_and_check(non_std_err_src, &nse_hints, false);
    assert!(nse_rsh.contains("var source ="), "got:\n{nse_rsh}");
    assert!(!nse_rsh.contains("rust {"), "got:\n{nse_rsh}");

    // Mixed `Foo<'_, 'static>` RA hint relaxes `'static` to `'_` (`minijinja-py/src/environment.rs`)
    let mixed_lt_src = "fn make(src: &str) {\n    let tmpl = parse(src);\n    let _ = tmpl;\n}\n";
    let mut mlt_hints = TypeHintMap::default();
    mlt_hints.insert(2, 12, "minijinja::Template<'_, 'static>".to_string());
    let (_mlt_rsh, mlt_rust) = roundtrip_and_check(mixed_lt_src, &mlt_hints, false);
    assert!(
        mlt_rust.contains("let tmpl: minijinja::Template<'_, '_> ="),
        "got:\n{mlt_rust}"
    );
}

#[test]
fn ra_resolves_eight_hints_in_flight_and_retries_only_the_stale_hint() {
    let hints: Vec<_> = (0..10)
        .map(|i| {
            json!({
                "kind": 1, "position": {"line": 0, "character": i}, "label": ": Local", "data": i
            })
        })
        .collect();
    let mut stream = Vec::new();
    for msg in [
        json!({"id":1,"result":{}}),
        json!({"method":"experimental/serverStatus","params":{"quiescent":true}}),
        json!({"id":2,"result":hints}),
        json!({"id":100,"method":"ping"}),
        json!({"id":10,"error":{"code":-32801}}),
        json!({"id":9,"error":{"code":-1}}),
    ] {
        stream.extend(frame_lsp(&msg));
    }
    // The retry gets ID 11. Freeing ID 9 admits the ninth original hint.
    for (id, i) in [
        (11, 7),
        (13, 9),
        (12, 8),
        (8, 5),
        (7, 4),
        (6, 3),
        (5, 2),
        (4, 1),
        (3, 0),
    ] {
        let mut resolved = hints[i].clone();
        resolved["textEdits"] = json!([{"newText": format!(": crate::T{i}")}]);
        stream.extend(frame_lsp(&json!({"id":id,"result":resolved})));
    }
    let written = Rc::new(RefCell::new(Vec::new()));
    let mut oracle = RustAnalyzerOracle::from_io(
        Cursor::new(stream),
        SharedWriter(written.clone()),
        Path::new("missing"),
    )
    .unwrap();
    let result = oracle
        .type_hints_for_file(Path::new("sample.rs"), "")
        .unwrap();
    for i in 0..10 {
        if i == 6 {
            assert_eq!(result.get_entry(1, i), Some(("Local", false)));
        } else {
            assert_eq!(
                result.get_entry(1, i),
                Some((format!("crate::T{i}").as_str(), true))
            );
        }
    }
    let sent = String::from_utf8(written.borrow().clone()).unwrap();
    let (before_ping, after_ping) = sent.split_once("\"id\":100").unwrap();
    assert_eq!(before_ping.matches("inlayHint/resolve").count(), 8);
    assert_eq!(after_ping.matches("inlayHint/resolve").count(), 3);
    assert_eq!(after_ping.matches("\"data\":7").count(), 1);
    assert!(after_ping.rfind("didClose") > after_ping.rfind("inlayHint/resolve"));
}

#[test]
fn failed_resolves_keep_the_original_unverified_label() {
    fn stream(responses: Vec<serde_json::Value>) -> Cursor<Vec<u8>> {
        let hint = json!({"kind":1,"position":{"line":0,"character":3},"label":": T","data":42});
        let mut frames: Vec<_> = [
            json!({"id":77,"result":null}),
            json!({"id":1,"result":{}}),
            json!({"method":"experimental/serverStatus","params":{"quiescent":true}}),
            json!({"id":2,"result":[hint]}),
            json!({"id":88,"result":null}),
        ]
        .iter()
        .flat_map(frame_lsp)
        .collect();
        frames.extend(responses.iter().flat_map(frame_lsp));
        Cursor::new(frames)
    }
    let stale = |id| json!({"id":id,"error":{"code":-32801}});
    for replies in [Vec::new(), (3..=8).map(stale).collect()] {
        let mut oracle =
            RustAnalyzerOracle::from_io(stream(replies), Vec::new(), Path::new("missing")).unwrap();
        let result = oracle
            .type_hints_for_file(Path::new("sample.rs"), "")
            .unwrap();
        assert_eq!(result.get_entry(1, 3), Some(("T", false)));
    }

    struct RejectRetry;
    impl Write for RejectRetry {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if String::from_utf8_lossy(buf).contains("\"id\":4") {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut oracle =
        RustAnalyzerOracle::from_io(stream(vec![stale(3)]), RejectRetry, Path::new("missing"))
            .unwrap();
    let result = oracle
        .type_hints_for_file(Path::new("sample.rs"), "")
        .unwrap();
    assert_eq!(result.get_entry(1, 3), Some(("T", false)));

    let mut error_stream: Vec<_> = [
        json!({"id":1,"result":{}}),
        json!({"method":"experimental/serverStatus","params":{"quiescent":true}}),
    ]
    .iter()
    .flat_map(frame_lsp)
    .collect();
    error_stream.extend(frame_lsp(&json!({"id":2,"error":{"code":-32603}})));
    let mut oracle =
        RustAnalyzerOracle::from_io(Cursor::new(error_stream), Vec::new(), Path::new("missing"))
            .unwrap();
    assert!(
        oracle
            .type_hints_for_file(Path::new("sample.rs"), "")
            .unwrap_err()
            .to_string()
            .contains("-32603")
    );
}

#[test]
fn ra_oracle_retries_on_content_modified_error() {
    let mut stream = Vec::new();
    stream.extend_from_slice(&frame_lsp(
        &json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
    ));
    stream.extend_from_slice(&frame_lsp(&json!({
        "jsonrpc": "2.0",
        "method": "experimental/serverStatus",
        "params": { "quiescent": true }
    })));
    // First inlayHint attempt returns LSP -32801 ContentModified
    stream.extend_from_slice(&frame_lsp(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "error": {"code": -32801, "message": "content modified"}
    })));
    // Retry attempt (id 3) succeeds
    stream.extend_from_slice(&frame_lsp(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "result": [
            {"kind": 1, "position": {"line": 0, "character": 5}, "label": ": i32"}
        ]
    })));
    stream.extend_from_slice(&frame_lsp(
        &json!({"jsonrpc": "2.0", "id": 4, "result": null}),
    ));

    let mut oracle = RustAnalyzerOracle::from_io(
        Box::new(Cursor::new(stream)),
        Box::new(Vec::<u8>::new()),
        Path::new("."),
    )
    .expect("from_io init");
    let hints = oracle
        .type_hints_for_file(Path::new("retry.rs"), "let x = 1;")
        .expect("should retry -32801 and succeed");
    assert_eq!(hints.get(1, 5), Some("i32"));
    oracle.close();
}

#[test]
fn github_10_repos_full_branch_coverage() {
    // 1. Parser: RawMacroBody '[' and '{' attribute args, nested parens in starts_fn_ptr_type,
    //    and non-range Type::Array in is_unambiguous_inherent_qself_type.
    let parser_rsh = r#"
[my_attr['a]]
[my_attr{'a}]
void attr_delims() {}

unsafe ((int, int)) -> int make_nested_fn_ptr(unsafe ((int, int)) -> int f) {
    return f;
}

void qself_arrays(read int[2] a) {
    _ = (int[2]).default();
    _ = (a[0]).abs();
}
"#;
    let parser_rust = transpile_to_rust(
        parser_rsh,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("parser branch coverage rsharp should transpile");
    assert!(
        parser_rust.contains("#[my_attr['a]]"),
        "got:\n{parser_rust}"
    );
    assert!(
        parser_rust.contains("#[my_attr{'a}]"),
        "got:\n{parser_rust}"
    );
    assert!(
        parser_rust.contains("unsafe fn((i32, i32)) -> i32"),
        "got:\n{parser_rust}"
    );
    assert!(
        parser_rust.contains("<[i32; 2]>::default()"),
        "got:\n{parser_rust}"
    );
    assert!(parser_rust.contains("a[0].abs()"), "got:\n{parser_rust}");

    // 2. Codegen: non-std-alias enum variant in `using E.*`, attributed trailing return in
    //    value block, `bound_contains_never` (lifetime, fn params, angle Type/AssocBinding/other),
    //    and `is_string_module_expr` (Member true/false, Path true/false, Ident false).
    let codegen_rsh = r#"
enum MixedKind { Other, List }
using MixedKind.*;

void codegen_branches(int x) {
    _ = (int n) => {
        [allow(unused)]
        return n;
    };
    impl Clone + maybe Sized + life static a = x;
    impl Fn(int) -> never b = make_fn();
    impl From<int> c = make_from();
    impl Iterator<Item = int> d = make_iter();
    impl HasLife<life static> e = make_life();
    _ = std.string.ToString.to_string(read x);
    _ = std.string.string.new();
    _ = std.fmt.string;
    _ = std::string.string;
    _ = std::fmt.string;
    _ = (a, b, c, d, e, Other, List);
}
"#;
    let codegen_rust = transpile_to_rust(
        codegen_rsh,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("codegen branch coverage rsharp should transpile");
    assert!(
        codegen_rust.contains("#[allow(unused)]\n        n"),
        "got:\n{codegen_rust}"
    );
    assert!(
        !codegen_rust.contains("Fn(i32) -> !"),
        "got:\n{codegen_rust}"
    );
    assert!(
        codegen_rust.contains("std::string::ToString::to_string"),
        "got:\n{codegen_rust}"
    );
    assert!(
        codegen_rust.contains("std::string::String::new"),
        "got:\n{codegen_rust}"
    );
    assert!(
        codegen_rust.contains("std::fmt::string"),
        "got:\n{codegen_rust}"
    );

    // 3. from_rust: BlockTail::Break with outer attr, Pat::Struct untyped `mut` fallback,
    //    Pat::Ident `@` subpat with `ref`/`ref mut` RA hints, strip_leading_borrow_prefix
    //    (`read `, `write `, `read<static> `, `read<int>` no space, non-matching prefix),
    //    binary `(Some(lt), None)` with non-unsuffixed LHS, and loop-jump call/method args.
    let from_rust_src = r#"
struct reader;

fn labeled_break_attr(c: bool) -> i32 {
    let v: i32 = if c {
        let tmp: i32 = 1;
        let _ = tmp;
        #[allow(unused)]
        42
    } else {
        0
    };
    v
}

fn unknown_struct_mut_local(u: UnknownStruct) {
    let UnknownStruct { mut a, b } = u;
    let _ = (a, b);
}

fn by_ref_at_subpats(v: i32, s: &'static str, r: reader, m: MyCustom) {
    match v {
        ref x @ 1..=5 => { let _ = x; }
        _ => {}
    }
    match v {
        ref mut y @ 1..=5 => { let _ = y; }
        _ => {}
    }
    match s {
        ref z @ _ => { let _ = z; }
    }
    match r {
        ref w @ _ => { let _ = w; }
    }
    match m {
        ref q @ _ => { let _ = q; }
    }
}

fn binary_known_lhs_unknown_rhs(x: i32) {
    let r = x + unknown_rhs();
    let _ = r;
}

fn loop_jump_call_and_method_args(mut n: i32) {
    macro_rules! bail {
        () => { continue; };
    }
    loop {
        let _: i32 = if n > 0 {
            let t: i32 = 1;
            let _ = t;
            foo(1, bail!())
        } else {
            0
        };
        let _: i32 = if n > 0 {
            let t: i32 = 1;
            let _ = t;
            foo(1, 2)
        } else {
            let t: i32 = 2;
            let _ = t;
            n.bar(1, bail!())
        };
    }
}
"#;
    let mut fr_hints = TypeHintMap::default();
    for (idx, line) in from_rust_src.lines().enumerate() {
        let l1 = (idx + 1) as u32;
        if let Some(pos) = line.find("ref x @") {
            fr_hints.insert(l1, (pos + 5) as u32, "&i32".to_string());
        } else if let Some(pos) = line.find("ref mut y @") {
            fr_hints.insert(l1, (pos + 9) as u32, "&mut i32".to_string());
        } else if let Some(pos) = line.find("ref z @") {
            fr_hints.insert(l1, (pos + 5) as u32, "&'static str".to_string());
        } else if let Some(pos) = line.find("ref w @") {
            fr_hints.insert(l1, (pos + 5) as u32, "reader".to_string());
        } else if let Some(pos) = line.find("ref q @") {
            fr_hints.insert(l1, (pos + 5) as u32, "MyCustom".to_string());
        }
    }
    let (fr_rsh, fr_rust) = roundtrip_and_check(from_rust_src, &fr_hints, false);
    assert!(fr_rust.contains("#[allow(unused)]\n"), "got:\n{fr_rust}");
    assert!(fr_rsh.contains("borrow read int x"), "got:\n{fr_rsh}");
    assert!(fr_rsh.contains("borrow write int y"), "got:\n{fr_rsh}");
    assert!(fr_rsh.contains("borrow read str z"), "got:\n{fr_rsh}");
    assert!(fr_rsh.contains("borrow read reader w"), "got:\n{fr_rsh}");
    assert!(fr_rsh.contains("borrow read MyCustom q"), "got:\n{fr_rsh}");
    assert!(
        fr_rsh.contains("int r = (x + unknown_rhs());"),
        "got:\n{fr_rsh}"
    );
    let loop_fn_rust = fr_rust
        .split("fn loop_jump_call_and_method_args")
        .nth(1)
        .expect("loop fn present");
    assert!(!loop_fn_rust.contains("'__if_val:"), "got:\n{loop_fn_rust}");

    // Also test strict mode on `let UnknownStruct { mut a, b } = u;` to hit untyped `mut` output.
    let strict_struct = "struct S { a: i32 }\nfn f(u: UnknownStruct) { let UnknownStruct { mut a, b } = u; let _ = (a, b); }\n";
    let strict_rsh = convert_from_rust_with_hints(
        strict_struct,
        FromRustOptions {
            use_rustc_inference: false,
        },
        &TypeHintMap::default(),
        true,
    )
    .expect("strict unknown struct local should convert");
    assert!(strict_rsh.contains("mut a, b"), "got:\n{strict_rsh}");

    // 4. ra_oracle: workspace scan for optional deps, cfg-gated mods, `#![no_std]`, and hint
    //    filtering (`hint_uses_cfg_gated_item`, `hint_uses_unqualified_alloc_type`, `extract_mod_decl_name`).
    let ws = std::env::temp_dir().join(format!("rsharp_ra_cfg_scan_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::create_dir_all(ws.join(".hidden")).unwrap();
    std::fs::create_dir_all(ws.join("target")).unwrap();
    std::fs::create_dir_all(ws.join("node_modules")).unwrap();
    std::fs::write(ws.join("README.md"), "ignore me\n").unwrap();
    std::fs::write(
        ws.join("Cargo.toml"),
        concat!(
            "[package]\nname = \"nostd_demo\"\nversion = \"0.1.0\"\n",
            "# optional comment without true\n",
            "# optional true without equals\n",
            "bad.dep = { optional = true }\n",
            "serde-json = { version = \"1\", optional = true }\n",
        ),
    )
    .unwrap();
    std::fs::write(
        ws.join("src").join("lib.rs"),
        concat!(
            "#![no_std]\n",
            "// comment line\n\n",
            "#[cfg(feature = \"fuel\")]\n",
            "#[allow(dead_code)]\n",
            "pub(crate) mod fuel;\n",
            "#[cfg(unix)]\n",
            "mod unix_mod;\n",
            "#[cfg(feature = \"bad\")]\n",
            "pub const NOT_A_MOD: i32 = 1;\n",
            "#[cfg(feature = \"bad\")]\n",
            "mod bad-mod;\n",
            "#[cfg(feature = \"bad\")]\n",
            "fn no_semi() {}\n",
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src").join("other.rs"), "pub fn ok() {}\n").unwrap();

    let mut stream = Vec::new();
    stream.extend_from_slice(&frame_lsp(
        &json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
    ));
    stream.extend_from_slice(&frame_lsp(&json!({
        "jsonrpc": "2.0",
        "method": "experimental/serverStatus",
        "params": { "quiescent": true }
    })));
    // Return hints exercising every filter path in `query_inlay_hints`:
    // - pos 0: bare `serde_json::Value` (optional dep at pos 0 -> masked)
    // - pos 1: `Option<serde_json::Value>` (optional dep after `<` -> masked)
    // - pos 2: `my_serde_json::Value` (substring with `_` prefix -> NOT masked)
    // - pos 3: `crate::fuel::Fuel` (cfg-gated mod -> masked)
    // - pos 4: unverified `String` in no_std (no data -> `_rsharp_no_edit: true` -> masked)
    // - pos 5: unverified `Option<Vec<u8>>` in no_std (masked)
    // - pos 6: unverified `Box<u8>` in no_std (masked)
    // - pos 7: unverified `alloc::string::String` (qualified `::` -> kept)
    // - pos 8: unverified `MyString` + `StringBuf` (alnum boundary -> kept)
    // - pos 9: verified `String` via textEdits (verified == true -> kept)
    // - pos 10: unverified `i32` (trivial primitive -> kept)
    stream.extend_from_slice(&frame_lsp(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "result": [
            {"kind": 1, "position": {"line": 0, "character": 1}, "label": ": serde_json::Value"},
            {"kind": 1, "position": {"line": 0, "character": 2}, "label": ": Option<serde_json::Value>"},
            {"kind": 1, "position": {"line": 0, "character": 3}, "label": ": my_serde_json::Value"},
            {"kind": 1, "position": {"line": 0, "character": 4}, "label": ": crate::fuel::Fuel"},
            {"kind": 1, "position": {"line": 0, "character": 5}, "label": ": String"},
            {"kind": 1, "position": {"line": 0, "character": 6}, "label": ": Option<Vec<u8>>"},
            {"kind": 1, "position": {"line": 0, "character": 7}, "label": ": Box<u8>"},
            {"kind": 1, "position": {"line": 0, "character": 8}, "label": ": alloc::string::String"},
            {"kind": 1, "position": {"line": 0, "character": 9}, "label": ": (MyString, StringBuf)"},
            {"kind": 1, "position": {"line": 0, "character": 10}, "label": ": String", "textEdits": [{"newText": ": String"}]},
            {"kind": 1, "position": {"line": 0, "character": 11}, "label": ": i32"}
        ]
    })));
    stream.extend_from_slice(&frame_lsp(
        &json!({"jsonrpc": "2.0", "id": 3, "result": null}),
    ));

    let mut oracle = RustAnalyzerOracle::from_io(
        Box::new(Cursor::new(stream)),
        Box::new(Vec::<u8>::new()),
        &ws,
    )
    .expect("from_io with Cargo.toml workspace");
    let hints = oracle
        .type_hints_for_file(&ws.join("src").join("lib.rs"), "fn f() {}")
        .expect("hints should filter cfg-gated and no_std alloc hints");
    assert_eq!(hints.get(1, 1), Some("{unknown}"));
    assert_eq!(hints.get(1, 2), Some("{unknown}"));
    assert_eq!(hints.get(1, 3), Some("my_serde_json::Value"));
    assert_eq!(hints.get(1, 4), Some("{unknown}"));
    assert_eq!(hints.get(1, 5), Some("{unknown}"));
    assert_eq!(hints.get(1, 6), Some("{unknown}"));
    assert_eq!(hints.get(1, 7), Some("{unknown}"));
    assert_eq!(hints.get(1, 8), Some("alloc::string::String"));
    assert_eq!(hints.get(1, 9), Some("(MyString, StringBuf)"));
    assert_eq!(hints.get(1, 10), Some("String"));
    assert_eq!(hints.get(1, 11), Some("i32"));
    oracle.close();
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn github_20_repos_regressions_and_full_branch_coverage() {
    // 1. Lexer: `.` / `::` followed by keyword macro (`std.write!`, `std::write!`)
    //    and `.` followed by non-keyword before `!` (`a.0!()`).
    let lex_ok = lexer::tokenize("std.write!(buf, \"hi\"); std::write!(buf, \"ok\");")
        .expect("keyword macro after dot/coloncolon should lex");
    assert!(!lex_ok.is_empty());
    let lex_non_kw = lexer::tokenize("a.0!()").expect("non-keyword after dot before bang");
    assert!(!lex_non_kw.is_empty());

    // 2. Empty file under `PreludeMode::None` omits crate-level `#![allow(...)]`.
    let empty_rust = transpile_to_rust(
        "",
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("empty file transpile");
    assert!(empty_rust.is_empty(), "got: {empty_rust:?}");

    // 3. Parser + Codegen regressions from the 20-repo campaign:
    //    - `EventStream![] events()` non-brace macro in return-type position
    //    - `impl Foo { rust { type Bar; } }` (`ImplItem::RawRust`)
    //    - `public unsafe impl EntityCommand insert_by_id<T>()`
    //    - `(Self.drop_ptr<T> as _)` generic turbofish before `as`
    //    - `(C.QueryData as QueryData).Item<life _, life _> param` typed qself pattern
    //    - `require (opt is Some(v) : Option<int>) else { return; }`
    //    - `match (...) { ... }.unwrap()`, `[0]`, `(1)`, `?`, and next-line tuple destructuring
    //    - `unsafe { ... }` postfix `.`, `(1)`, `[0]` on same line vs next-line `[Allow(...)]` / `(a, b)`
    //    - brace macro call followed by next-line `[Allow(...)]` / `(a, b)` and in `match` arms
    //    - `read (0..10)` parenthesizing range under unary borrow (`&(0..10)`)
    //    - attributed `impl Trait` VarDecl and attributed destructuring pattern checks
    //    - `type_contains_impl_trait` across Read/Write/Slice/ReadPtr/WritePtr/Optional/Grouped/Array/Tuple
    //    - generic `impl<T>` / `fn<T>` skipping nested `__rsharp_impl_check` (`E0401`)
    //    - closure parameter with `@` subpattern (`Pattern::BindAt`)
    let rsh_20 = r#"
struct Foo;
impl Foo {
    rust { type VerbatimItem; }
    EventStream![] events() { return make_stream(); }
    public unsafe impl EntityCommand insert_by_id<T>(T x) { return x; }
}

EventStream![] top_events() { return make_stream(); }

impl<T> GenericBox<T> {
    void generic_impl_check(T x) {
        impl Copy a = x;
        _ = a;
    }
}

void test_parser_and_codegen_20(Option<int> opt, bool flag, int x, (int, int) pair) {
    require (opt is Some(v) : Option<int>) else { return; }
    _ = v;

    match (flag) { true => Some(1), false => None }.unwrap();
    match (flag) { true => [1, 2], false => [3, 4] }[0];
    match (flag) { true => f, false => g }(1);
    match (flag) { true => true, false => false } ? 1 : 2;
    match (flag) { true => {}, false => {} }
    (int ma, int mb) = (1, 2);

    unsafe { return arr; }[0];
    unsafe { return f; }(1);
    unsafe { foo(); }
    [Allow(unused)]
    int after_unsafe = 1;

    my_mac! { 1 }
    [Allow(unused)]
    int after_mac_attr = 2;

    my_mac! { 2 }
    (int da, int db) = (3, 4);

    _ = match (x) {
        1 => unsafe { return 1; }
        [Allow(unused)]
        2 => match (flag) { true => 1, false => 2 }
        [Allow(unused)]
        3 => my_mac! { 3 }
        [Allow(unused)]
        4 => 4,
        _ => 0,
    };

    _ = match (pair) {
        (0, 0) => unsafe { return 1; }
        (1, 1) => match (flag) { true => 1, false => 2 }
        (2, 2) => my_mac! { 2 }
        (3, 3) => 3,
        _ => 0,
    };

    _ = (Self.drop_ptr<int> as _);
    _ = read (0..10);
    _ = write (0..10);
    _ = ((1 as int all) => all);
    _ = (((C.QueryData as QueryData).Item<life _, life _> qc) => qc);

    [Allow(unused)]
    impl Copy attr_impl = 1;
    [Allow(unused)]
    (int pa, int pb) = (1, 2);

    (impl Copy t0, int k0) = (1, 0);
    (read impl Copy t1, write impl Copy t2) = (read 1, write 2);
    (read slice<impl Copy> t3, read ptr<impl Copy> t4) = (s, p1);
    (write ptr<impl Copy> t5, read (impl Copy)? t6) = (p2, o1);
    (read (impl Copy) t7, read (impl Copy)[1] t8) = (g1, a1);
    (read (impl Copy, int) t9, read (int, int) t10) = (tp1, read (1, 2));
    _ = (ma, mb, after_unsafe, after_mac_attr, da, db, attr_impl, pa, pb, t0, k0, t1, t2, t3, t4, t5, t6, t7, t8, t9, t10);
}
"#;
    let rust_20 = transpile_to_rust(
        rsh_20,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("20-repo R# regressions should parse and transpile");
    assert!(
        rust_20.contains("#![allow(unused_parens, unused_braces, unreachable_code)]"),
        "got:\n{rust_20}"
    );
    assert!(rust_20.contains("type VerbatimItem;"), "got:\n{rust_20}");
    assert!(
        rust_20.contains("fn events() -> EventStream![]"),
        "got:\n{rust_20}"
    );
    assert!(
        rust_20.contains("pub unsafe fn insert_by_id<T>(x: T) -> impl EntityCommand"),
        "got:\n{rust_20}"
    );
    assert!(
        rust_20.contains("let Some(v): Option<i32> = opt else {"),
        "got:\n{rust_20}"
    );
    assert!(rust_20.contains("&(0..10)"), "got:\n{rust_20}");
    assert!(rust_20.contains("&mut (0..10)"), "got:\n{rust_20}");
    assert!(
        !rust_20.contains("let _: &&impl Copy"),
        "should skip impl-trait pattern checks:\n{rust_20}"
    );
    assert!(
        !rust_20
            .split("fn generic_impl_check")
            .nth(1)
            .unwrap()
            .split("fn test_parser_and_codegen_20")
            .next()
            .unwrap()
            .contains("__rsharp_impl_check"),
        "generic impl should skip __rsharp_impl_check:\n{rust_20}"
    );
    assert!(
        parser::parse_file("void f() { require (opt is Some(v) : ) else { return; } }").is_err()
    );

    // 4. from_rust regressions:
    //    - `Item::Verbatim` via `type Unassigned;` (typst)
    //    - `let Some(v): Option<i32> = opt else { return; };` (`Pat::Type` in diverge)
    //    - `Expr::Unsafe` with trailing `if`/`match`/`block` in value position (rtk)
    //    - `UnOp::Deref` on `*const bool` and `*mut i32` (deno)
    //    - `Expr::Repeat` const-expr length (`[0u8; 1 + 2]`)
    //    - `String(s)` single-segment pattern path NOT mapped to `string(s)` (rtk)
    //    - `Pat::Or` in match arm & `bind_scope_pat_idents` shadowing a module path (starship/claw-code)
    //    - RA hint filters: bare `unsafe fn()`, `unsafe extern "C" fn()`, `fn(&str) -> &str`,
    //      `for<'a> fn(&'a str) -> &'a str`, `fn(i32)`, `fn(i32) -> i32`,
    //      `impl Future<Output = i32>` unimported vs imported, and `impl Clone + 'static`.
    let rust_src_20 = r#"
type VerbatimTop;
enum Val { String(i32), Other(i32) }

fn from_rust_20_cases(opt: Option<i32>, flag: bool, p_const: *const bool, p_mut: *mut i32, v: Val) {
    let Some(req_val): Option<i32> = opt else { return; };
    let u_if: i32 = unsafe {
        let tmp = 1i32;
        if flag { tmp } else { 0 }
    };
    unsafe {
        let b_val = *p_const;
        let m_val = *p_mut;
        let _ = (b_val, m_val);
    }
    let bx: Box<i32> = Box::new(1);
    let d_bx = *bx;
    let _ = d_bx;
    let arr = [0u8; 1 + 2];
    match v {
        Val::String(s) | Val::Other(s) => {
            let _ = s;
        }
    }
    use Val::String;
    if let String(inner) = v {
        let _ = inner;
    }
    let h0 = unknown_fn();
    let h1 = unknown_fn();
    let h2 = unknown_fn();
    let h3 = unknown_fn();
    let h4 = unknown_fn();
    let h5 = unknown_fn();
    let h6 = unknown_fn();
    let h7 = unknown_fn();
    let _ = (req_val, u_if, arr, h0, h1, h2, h3, h4, h5, h6, h7);
}
"#;
    let hint_specs_20: &[(&str, &str)] = &[
        ("h0", "unsafe fn()"),
        ("h1", "unsafe extern \"C\" fn()"),
        ("h2", "fn(&str) -> &str"),
        ("h3", "for<'a> fn(&'a str) -> &'a str"),
        ("h4", "fn(i32)"),
        ("h5", "fn(i32) -> i32"),
        ("h6", "impl Future<Output = i32>"),
        ("h7", "impl Clone + 'static"),
    ];
    let mut hints_20 = TypeHintMap::default();
    for (idx, line) in rust_src_20.lines().enumerate() {
        let l1 = (idx + 1) as u32;
        for (var, ty) in hint_specs_20 {
            let pat = format!("let {var} =");
            if let Some(pos) = line.find(&pat) {
                hints_20.insert_with_verified(
                    l1,
                    (pos + 4 + var.len()) as u32,
                    (*ty).to_string(),
                    true,
                );
            }
        }
    }
    let (rsh_fr20, rust_fr20) = roundtrip_and_check(rust_src_20, &hints_20, false);
    assert!(rsh_fr20.contains("VerbatimTop"), "got:\n{rsh_fr20}");
    assert!(
        rsh_fr20.contains("require (opt is Some(req_val) : int?) else\n    {"),
        "got:\n{rsh_fr20}"
    );
    assert!(rsh_fr20.contains("bool b_val ="), "got:\n{rsh_fr20}");
    assert!(rsh_fr20.contains("int m_val ="), "got:\n{rsh_fr20}");
    assert!(rsh_fr20.contains("String(inner)"), "got:\n{rsh_fr20}");
    assert!(rsh_fr20.contains("var h0 ="), "got:\n{rsh_fr20}");
    assert!(rsh_fr20.contains("var h2 ="), "got:\n{rsh_fr20}");
    assert!(rsh_fr20.contains("var h6 ="), "got:\n{rsh_fr20}");
    assert!(rust_fr20.contains("if flag"), "got:\n{rust_fr20}");

    // Also test `impl Future<Output = i32>` when `Future` IS explicitly imported.
    let imported_future_src = r#"
use std::future::Future;
fn with_future() {
    let f = unknown_fn();
    let _ = f;
}
"#;
    let mut fut_hints = TypeHintMap::default();
    fut_hints.insert_with_verified(4, 9, "impl Future<Output = i32>".to_string(), true);
    let (fut_rsh, _) = roundtrip_and_check(imported_future_src, &fut_hints, false);
    assert!(
        fut_rsh.contains("impl Future<Output = int> f ="),
        "got:\n{fut_rsh}"
    );

    // 5. ra_oracle: `#[cfg(test)] pub use ...::<Name>;` re-export filtering.
    let ws = std::env::temp_dir().join(format!("rsharp_ra_cfg_test_reexp_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("Cargo.toml"),
        "[package]\nname = \"reexp_demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        ws.join("src").join("lib.rs"),
        concat!(
            "#[cfg(test)]\n",
            "pub use crate::sub::TestBinding;\n",
            "#[cfg(test)]\n",
            "pub use BareBinding;\n",
            "#[cfg(test)]\n",
            "pub use crate::sub::*;\n",
            "#[cfg(test)]\n",
            "const NOT_A_PUB_USE: i32 = 1;\n",
        ),
    )
    .unwrap();

    let mut stream = Vec::new();
    stream.extend_from_slice(&frame_lsp(
        &json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
    ));
    stream.extend_from_slice(&frame_lsp(&json!({
        "jsonrpc": "2.0",
        "method": "experimental/serverStatus",
        "params": { "quiescent": true }
    })));
    stream.extend_from_slice(&frame_lsp(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "result": [
            {"kind": 1, "position": {"line": 0, "character": 1}, "label": ": super::TestBinding"},
            {"kind": 1, "position": {"line": 0, "character": 2}, "label": ": Option<super::TestBinding>"},
            {"kind": 1, "position": {"line": 0, "character": 3}, "label": ": super::TestBindingExtra"},
            {"kind": 1, "position": {"line": 0, "character": 4}, "label": ": i32"}
        ]
    })));
    stream.extend_from_slice(&frame_lsp(
        &json!({"jsonrpc": "2.0", "id": 3, "result": null}),
    ));

    let mut oracle = RustAnalyzerOracle::from_io(
        Box::new(Cursor::new(stream)),
        Box::new(Vec::<u8>::new()),
        &ws,
    )
    .expect("from_io with cfg(test) pub use workspace");
    let reexp_hints = oracle
        .type_hints_for_file(&ws.join("src").join("lib.rs"), "fn f() {}")
        .expect("hints should filter cfg(test) pub use re-exports");
    assert_eq!(reexp_hints.get(1, 1), Some("{unknown}"));
    assert_eq!(reexp_hints.get(1, 2), Some("{unknown}"));
    assert_eq!(reexp_hints.get(1, 3), Some("super::TestBindingExtra"));
    assert_eq!(reexp_hints.get(1, 4), Some("i32"));
    oracle.close();
    let _ = std::fs::remove_dir_all(&ws);
}
