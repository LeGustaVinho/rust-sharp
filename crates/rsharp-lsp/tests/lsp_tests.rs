use std::path::PathBuf;
use std::str::FromStr;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    CompletionContext, CompletionParams, CompletionResponse, CompletionTriggerKind,
    DiagnosticSeverity, DidChangeConfigurationParams, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    HoverContents, HoverParams, NumberOrString, PartialResultParams, Position,
    SemanticTokensParams, SemanticTokensResult, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
    VersionedTextDocumentIdentifier, WorkDoneProgressParams,
};
use serde_json::{Value, json};

use rsharp_lsp::analysis::DocumentIndex;
use rsharp_lsp::diagnostics::{parse_rustc_json_diagnostics, semantic_diagnostics, syntax_diagnostics};
use rsharp_lsp::position::LineIndex;
use rsharp_lsp::run_connection;

fn test_uri(name: &str) -> Uri {
    Uri::from_str(&format!("file:///workspace/{}", name)).unwrap()
}

struct LspHarness {
    client: Connection,
    server_thread: Option<std::thread::JoinHandle<Result<(), String>>>,
    next_id: i32,
}

impl LspHarness {
    fn start(init_opts: Option<Value>) -> Self {
        let (server, client) = Connection::memory();
        let server_thread = std::thread::spawn(move || run_connection(&server));
        let mut h = Self {
            client,
            server_thread: Some(server_thread),
            next_id: 1,
        };
        let mut params = json!({ "capabilities": {} });
        if let Some(opts) = init_opts {
            params["initializationOptions"] = opts;
        }
        let resp = h.request("initialize", params);
        assert!(resp.error.is_none());
        assert!(resp.result.unwrap().get("capabilities").is_some());
        h.notify("initialized", json!({}));
        h
    }

    fn request(&mut self, method: &str, params: Value) -> Response {
        let id = RequestId::from(self.next_id);
        self.next_id += 1;
        self.client
            .sender
            .send(Message::Request(Request::new(
                id.clone(),
                method.to_string(),
                params,
            )))
            .unwrap();
        loop {
            match self.client.receiver.recv().unwrap() {
                Message::Response(resp) if resp.id == id => return resp,
                Message::Notification(_) => {}
                other => panic!("unexpected message: {:?}", other),
            }
        }
    }

    fn notify(&self, method: &str, params: Value) {
        self.client
            .sender
            .send(Message::Notification(Notification::new(
                method.to_string(),
                params,
            )))
            .unwrap();
    }

    fn recv_diagnostics(&self) -> lsp_types::PublishDiagnosticsParams {
        match self.client.receiver.recv().unwrap() {
            Message::Notification(notif) if notif.method == "textDocument/publishDiagnostics" => {
                serde_json::from_value(notif.params).unwrap()
            }
            other => panic!("expected publishDiagnostics, got {:?}", other),
        }
    }

    fn shutdown(mut self) {
        let resp = self.request("shutdown", Value::Null);
        assert!(resp.error.is_none());
        self.notify("exit", Value::Null);
        let res = self.server_thread.take().unwrap().join().unwrap();
        assert!(res.is_ok(), "server returned error: {:?}", res);
    }
}

fn find_pos(source: &str, needle: &str) -> Position {
    let idx = source.find(needle).expect("needle not found in source");
    LineIndex::new(source).offset_to_position(source, idx)
}

#[test]
fn lsp_full_workflow_on_examples_and_ide_features() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let accounts_src =
        std::fs::read_to_string(workspace_root.join("examples/accounts.rsharp")).unwrap();
    let matrices_src =
        std::fs::read_to_string(workspace_root.join("examples/matrices_and_control.rsharp"))
            .unwrap();
    let interop_src =
        std::fs::read_to_string(workspace_root.join("examples/rust_interop.rsharp")).unwrap();

    let mut h = LspHarness::start(Some(json!({ "checkOnSave": true })));

    // Open all 3 example files and assert clean syntax diagnostics.
    let uri_acc = test_uri("accounts.rsharp");
    let uri_mat = test_uri("matrices.rsharp");
    let uri_int = test_uri("interop.rsharp");

    for (uri, text) in [
        (&uri_acc, &accounts_src),
        (&uri_mat, &matrices_src),
        (&uri_int, &interop_src),
    ] {
        h.notify(
            "textDocument/didOpen",
            serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "rsharp".to_string(),
                    version: 1,
                    text: text.clone(),
                },
            })
            .unwrap(),
        );
        let diags = h.recv_diagnostics();
        assert_eq!(diags.uri, *uri);
        assert!(diags.diagnostics.is_empty(), "unexpected diagnostics: {:?}", diags.diagnostics);
    }

    // 1. Document Outline (`documentSymbol`) on accounts.rsharp
    let sym_resp = h.request(
        "textDocument/documentSymbol",
        serde_json::to_value(DocumentSymbolParams {
            text_document: TextDocumentIdentifier::new(uri_acc.clone()),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap(),
    );
    let DocumentSymbolResponse::Nested(syms) =
        serde_json::from_value(sym_resp.result.unwrap()).unwrap()
    else {
        panic!("expected nested symbols");
    };
    let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Account"));
    assert!(names.contains(&"TotalBalance"));
    assert!(names.contains(&"main"));
    let account_sym = syms.iter().find(|s| s.name == "Account").unwrap();
    let child_names: Vec<&str> = account_sym
        .children
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert!(child_names.contains(&"Owner"));
    assert!(child_names.contains(&"Balance"));
    assert!(child_names.contains(&"New"));
    assert!(child_names.contains(&"Deposit"));
    assert!(child_names.contains(&"Summary"));

    // Document symbols on matrices & interop examples too (enum, trait, impl, generics)
    for uri in [&uri_mat, &uri_int] {
        let resp = h.request(
            "textDocument/documentSymbol",
            serde_json::to_value(DocumentSymbolParams {
                text_document: TextDocumentIdentifier::new(uri.clone()),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .unwrap(),
        );
        let DocumentSymbolResponse::Nested(s) =
            serde_json::from_value(resp.result.unwrap()).unwrap()
        else {
            panic!("expected nested");
        };
        assert!(!s.is_empty());
    }

    // 2. Hover on user struct with doc comment, member call, keyword, primitive, prelude function
    let hover_cases = [
        (&uri_acc, &accounts_src, "Account alice", "struct Account"),
        (&uri_acc, &accounts_src, "Deposit(50)", "Deposit"),
        (&uri_acc, &accounts_src, "read slice<Account>", "Immutable borrow"),
        (&uri_acc, &accounts_src, "PrintLine(accounts", "PrintLine"),
        (&uri_acc, &accounts_src, "long total", "64-bit signed integer"),
        (&uri_mat, &matrices_src, "ToString()", "ToString"),
        (&uri_int, &interop_src, "DescribeDyn(read", "DescribeDyn"),
    ];
    for (uri, src, needle, expected) in hover_cases {
        let pos = find_pos(src, needle);
        let resp = h.request(
            "textDocument/hover",
            serde_json::to_value(HoverParams {
                text_document_position_params: TextDocumentPositionParams::new(
                    TextDocumentIdentifier::new(uri.clone()),
                    pos,
                ),
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .unwrap(),
        );
        let hover: lsp_types::Hover = serde_json::from_value(resp.result.unwrap()).unwrap();
        let HoverContents::Markup(m) = hover.contents else {
            panic!("expected markdown hover");
        };
        assert!(
            m.value.contains(expected),
            "hover for '{}' did not contain '{}': {}",
            needle,
            expected,
            m.value
        );
    }

    // 3. Go to Definition on method call (`alice.Deposit`), struct field (`this.Balance`),
    //    local variable (`sum`), enum variant (`Shape.Circle`), and cross-document symbol!
    let def_pos = find_pos(&accounts_src, "Deposit(50)");
    let def_resp = h.request(
        "textDocument/definition",
        serde_json::to_value(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri_acc.clone()),
                def_pos,
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap(),
    );
    let GotoDefinitionResponse::Scalar(loc) =
        serde_json::from_value(def_resp.result.unwrap()).unwrap()
    else {
        panic!("expected scalar location");
    };
    assert_eq!(loc.uri, uri_acc);
    assert_eq!(loc.range.start.line, 13); // `public Result<long, string> Deposit(...)` (0-based line 13)

    // Field `this.Balance`
    let bal_pos = find_pos(&accounts_src, "Balance += amount");
    let bal_resp = h.request(
        "textDocument/definition",
        serde_json::to_value(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri_acc.clone()),
                bal_pos,
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap(),
    );
    let GotoDefinitionResponse::Scalar(bal_loc) =
        serde_json::from_value(bal_resp.result.unwrap()).unwrap()
    else {
        panic!("expected scalar");
    };
    assert_eq!(bal_loc.range.start.line, 6);

    // 4. Semantic Tokens on all 3 examples
    for uri in [&uri_acc, &uri_mat, &uri_int] {
        let tok_resp = h.request(
            "textDocument/semanticTokens/full",
            serde_json::to_value(SemanticTokensParams {
                text_document: TextDocumentIdentifier::new(uri.clone()),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .unwrap(),
        );
        let SemanticTokensResult::Tokens(tokens) =
            serde_json::from_value(tok_resp.result.unwrap()).unwrap()
        else {
            panic!("expected full tokens");
        };
        assert!(!tokens.data.is_empty());
    }

    // 5. Save valid file -> `rustc` check succeeds with zero errors (surfaces real `unused_mut` warning, filters summary noise)
    h.notify(
        "textDocument/didSave",
        serde_json::to_value(DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier::new(uri_acc.clone()),
            text: None,
        })
        .unwrap(),
    );
    let save_diags = h.recv_diagnostics();
    assert!(
        save_diags
            .diagnostics
            .iter()
            .all(|d| d.severity != Some(DiagnosticSeverity::ERROR)),
        "expected no error diagnostics on valid example, got: {:?}",
        save_diags.diagnostics
    );
    assert!(
        save_diags
            .diagnostics
            .iter()
            .all(|d| !d.message.ends_with("emitted"))
    );

    // 6. Dot-completion on `alice.` and `accounts.` + general completion
    let edited = accounts_src.replace(
        "PrintLine($\"Total: {total}\");",
        "alice.;\n    accounts.;\n    PrintLine($\"Total: {total}\");",
    );
    h.notify(
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier::new(uri_acc.clone(), 2),
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: edited.clone(),
            }],
        })
        .unwrap(),
    );
    let syntax_err_diags = h.recv_diagnostics();
    assert!(!syntax_err_diags.diagnostics.is_empty());

    // Dot-completion after `alice.` should suggest Account's fields & methods
    let alice_dot = find_pos(&edited, "alice.");
    let comp_pos = Position::new(alice_dot.line, alice_dot.character + 6);
    let comp_resp = h.request(
        "textDocument/completion",
        serde_json::to_value(CompletionParams {
            text_document_position: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri_acc.clone()),
                comp_pos,
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: Some(CompletionContext {
                trigger_kind: CompletionTriggerKind::TRIGGER_CHARACTER,
                trigger_character: Some(".".to_string()),
            }),
        })
        .unwrap(),
    );
    let CompletionResponse::Array(items) =
        serde_json::from_value(comp_resp.result.unwrap()).unwrap()
    else {
        panic!("expected array");
    };
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"Owner"));
    assert!(labels.contains(&"Deposit"));
    assert!(labels.contains(&"Summary"));

    // Dot-completion after `accounts.` (`List<Account>`) should suggest prelude `Add`, `Count`, `IsEmpty`
    let accs_dot = find_pos(&edited, "accounts.;");
    let accs_pos = Position::new(accs_dot.line, accs_dot.character + 9);
    let accs_comp = h.request(
        "textDocument/completion",
        serde_json::to_value(CompletionParams {
            text_document_position: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri_acc.clone()),
                accs_pos,
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .unwrap(),
    );
    let CompletionResponse::Array(list_items) =
        serde_json::from_value(accs_comp.result.unwrap()).unwrap()
    else {
        panic!("expected array");
    };
    let list_labels: Vec<&str> = list_items.iter().map(|i| i.label.as_str()).collect();
    assert!(list_labels.contains(&"Add"));
    assert!(list_labels.contains(&"Count"));
    assert!(list_labels.contains(&"IsEmpty"));

    // General completion inside `main` should suggest visible locals, functions, and keywords
    let gen_comp = h.request(
        "textDocument/completion",
        serde_json::to_value(CompletionParams {
            text_document_position: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri_acc.clone()),
                find_pos(&edited, "PrintLine($\"Total"),
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .unwrap(),
    );
    let CompletionResponse::Array(gen_items) =
        serde_json::from_value(gen_comp.result.unwrap()).unwrap()
    else {
        panic!("expected array");
    };
    let gen_labels: Vec<&str> = gen_items.iter().map(|i| i.label.as_str()).collect();
    assert!(gen_labels.contains(&"alice"));
    assert!(gen_labels.contains(&"TotalBalance"));
    assert!(gen_labels.contains(&"foreach"));
    assert!(gen_labels.contains(&"PrintLine"));

    h.shutdown();
}

#[test]
fn lsp_diagnostics_live_syntax_and_save_rustc_and_config_toggle() {
    let mut h = LspHarness::start(None);
    let uri = test_uri("check.rsharp");

    // 1. Open with lex error (unterminated string)
    h.notify(
        "textDocument/didOpen",
        serde_json::to_value(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "rsharp".to_string(),
                version: 1,
                text: "void main() { string s = \"unterminated; }".to_string(),
            },
        })
        .unwrap(),
    );
    let lex_diags = h.recv_diagnostics();
    assert_eq!(lex_diags.diagnostics.len(), 1);
    assert_eq!(
        lex_diags.diagnostics[0].code,
        Some(NumberOrString::String("lex".to_string()))
    );

    // 2. Change to valid R# syntax with a Rust type error (`int x = "oops";`)
    let type_err_src = "void main()\n{\n    int x = \"oops\";\n    _ = x;\n}\n";
    h.notify(
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier::new(uri.clone(), 2),
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: type_err_src.to_string(),
            }],
        })
        .unwrap(),
    );
    let live_diags = h.recv_diagnostics();
    assert!(live_diags.diagnostics.is_empty());

    // 3. Save triggers rustc semantic check -> E0308 mapped to line 2 (`"oops"`)
    h.notify(
        "textDocument/didSave",
        serde_json::to_value(DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier::new(uri.clone()),
            text: None,
        })
        .unwrap(),
    );
    let save_diags = h.recv_diagnostics();
    assert!(!save_diags.diagnostics.is_empty());
    let first = &save_diags.diagnostics[0];
    assert_eq!(first.severity, Some(DiagnosticSeverity::ERROR));
    assert_eq!(first.source.as_deref(), Some("rsharp (rustc)"));
    assert_eq!(first.code, Some(NumberOrString::String("E0308".to_string())));
    assert_eq!(first.range.start.line, 2);

    // 4. Disable `checkOnSave` via configuration -> save skips rustc
    h.notify(
        "workspace/didChangeConfiguration",
        serde_json::to_value(DidChangeConfigurationParams {
            settings: json!({ "rsharp": { "checkOnSave": false } }),
        })
        .unwrap(),
    );
    h.notify(
        "textDocument/didSave",
        serde_json::to_value(DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier::new(uri.clone()),
            text: None,
        })
        .unwrap(),
    );
    let skipped = h.recv_diagnostics();
    assert!(skipped.diagnostics.is_empty());

    // 5. Re-introduce lex error after valid state -> keeps previous valid index for hover
    h.notify(
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier::new(uri.clone(), 3),
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "void main()\n{\n    int x = \"unterminated\n}\n".to_string(),
            }],
        })
        .unwrap(),
    );
    let lex_again = h.recv_diagnostics();
    assert_eq!(lex_again.diagnostics.len(), 1);

    // 6. Close document clears diagnostics
    h.notify(
        "textDocument/didClose",
        serde_json::to_value(DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier::new(uri.clone()),
        })
        .unwrap(),
    );
    let closed = h.recv_diagnostics();
    assert!(closed.diagnostics.is_empty());

    // Unknown method returns MethodNotFound; invalid params returns InvalidParams
    let bad_method = h.request("custom/unknown", json!({}));
    assert!(bad_method.error.is_some());
    let bad_params = h.request("textDocument/hover", json!({ "bad": true }));
    assert!(bad_params.error.is_some());

    // Unknown notification & malformed notification are safely ignored
    h.notify("custom/ignored", json!({}));
    h.notify("textDocument/didChange", json!({ "bad": true }));
    h.notify("workspace/didChangeConfiguration", json!({ "bad": true }));

    // Empty change list on didChange does not publish diagnostics
    h.notify(
        "textDocument/didChange",
        serde_json::to_value(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier::new(uri.clone(), 4),
            content_changes: vec![],
        })
        .unwrap(),
    );

    // Requests for unknown URI return null result cleanly
    let missing_sym = h.request(
        "textDocument/documentSymbol",
        serde_json::to_value(DocumentSymbolParams {
            text_document: TextDocumentIdentifier::new(uri.clone()),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap(),
    );
    assert_eq!(missing_sym.result, Some(Value::Null));

    h.shutdown();
}

#[test]
fn lsp_cross_doc_and_rich_syntax_constructs_coverage() {
    let mut h = LspHarness::start(Some(json!({ "checkOnSave": false })));
    let uri_a = test_uri("defs.rsharp");
    let uri_b = test_uri("uses.rsharp");

    let doc_a = r#"#!/usr/bin/env rsharp
//! Module inner doc
[module: Allow(dead_code)]
using std.fmt;
extern crate std;

namespace App.Core
{
    /// Type alias doc
    public alias UserId = ulong;

    public const int MAX_ITEMS = 100;
    internal static mut int COUNTER = 0;

    /// A generic container
    [Derive(Debug)]
    public struct Boxed<life a, T, const nuint N = 4>
    {
        public T Value;
        public static Self Make(T v) { return new Self { Value = v }; }
    }

    public struct TuplePair(int, int);

    public union RawBits
    {
        public int AsInt;
        public float AsFloat;
    }

    public enum Status<T>
    {
        OkVal(T),
        Code = 1,
        public bool IsOk(read this) { return true; }
    }

    public trait Service<T>
    {
        type Output;
        const int VER = 1;
        async Task<int> Run(read this, [Allow(unused)] mut T item);
    }

    impl<T> Service<T> for Boxed<life a, T, 4> where T : Clone
    {
        type Output = T;
        async Task<int> Run(read this, mut T item)
        {
            shadow mut int x = 1;
            int uninit;
            _ = uninit;
            if (x == 1) { int scoped = 2; _ = scoped; }
            for (mut int i = 0; i < 3; i++) { _ = i; }
            return x;
        }
    }

    unsafe extern "C"
    {
        public safe void ffi_fn(int a);
    }

    macro my_mac { () => {}; }
    macro my_mac_p () => {};
    macro my_mac_b [] => {};
    macro my_mac_other;
    println!("hi");
    my_block_mac! { a }
    rust { let _ = 1; }
    ;
}

namespace FileScoped.Ns;

/// Exported helper function
public int SharedHelper(int v)
{
    return v + 1;
}
"#;

    let doc_b = r#"
void Caller()
{
    int r = SharedHelper(10);
    _ = r;
    int missing = UnknownSymbolXYZ;
    _ = missing;
    int k = 0;
    k.
    unknown_call<int>(1);
    unknown_obj.field_x;
}
"#;

    for (uri, text) in [(&uri_a, doc_a), (&uri_b, doc_b)] {
        h.notify(
            "textDocument/didOpen",
            serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "rsharp".to_string(),
                    version: 1,
                    text: text.to_string(),
                },
            })
            .unwrap(),
        );
        let _ = h.recv_diagnostics();
    }

    // Cross-document hover & goto-definition for `SharedHelper` from doc_b -> doc_a
    let shared_pos = find_pos(doc_b, "SharedHelper");
    let hover_cross = h.request(
        "textDocument/hover",
        serde_json::to_value(HoverParams {
            text_document_position_params: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri_b.clone()),
                shared_pos,
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .unwrap(),
    );
    assert!(hover_cross.result.unwrap().to_string().contains("Exported helper"));

    let def_cross = h.request(
        "textDocument/definition",
        serde_json::to_value(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri_b.clone()),
                shared_pos,
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap(),
    );
    let GotoDefinitionResponse::Scalar(cross_loc) =
        serde_json::from_value(def_cross.result.unwrap()).unwrap()
    else {
        panic!("expected scalar");
    };
    assert_eq!(cross_loc.uri, uri_a);

    // Hover & definition on unresolved symbol & trailing dot (`k.`) return null
    let unk_pos = find_pos(doc_b, "UnknownSymbolXYZ");
    for method in ["textDocument/hover", "textDocument/definition"] {
        let resp = h.request(
            method,
            serde_json::to_value(HoverParams {
                text_document_position_params: TextDocumentPositionParams::new(
                    TextDocumentIdentifier::new(uri_b.clone()),
                    unk_pos,
                ),
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .unwrap(),
        );
        assert_eq!(resp.result, Some(Value::Null));
    }

    let dot_pos = find_pos(doc_b, "k.");
    let after_dot = Position::new(dot_pos.line, dot_pos.character + 2);
    for method in ["textDocument/hover", "textDocument/definition"] {
        let resp = h.request(
            method,
            serde_json::to_value(HoverParams {
                text_document_position_params: TextDocumentPositionParams::new(
                    TextDocumentIdentifier::new(uri_b.clone()),
                    after_dot,
                ),
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .unwrap(),
        );
        assert_eq!(resp.result, Some(Value::Null));
    }

    // Semantic tokens & outline on doc_a and doc_b
    for uri in [&uri_a, &uri_b] {
        let sym = h.request(
            "textDocument/documentSymbol",
            serde_json::to_value(DocumentSymbolParams {
                text_document: TextDocumentIdentifier::new(uri.clone()),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .unwrap(),
        );
        assert!(sym.error.is_none());
        let sem = h.request(
            "textDocument/semanticTokens/full",
            serde_json::to_value(SemanticTokensParams {
                text_document: TextDocumentIdentifier::new(uri.clone()),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .unwrap(),
        );
        assert!(sem.error.is_none());
    }

    h.shutdown();
}

#[test]
fn diagnostics_unit_edge_cases() {
    // Codegen error path (`p with { ... }` on unknown struct type)
    let codegen_bad = "void main() { int x = 1; int y = x with { A = 2 }; _ = y; }";
    let lines = LineIndex::new(codegen_bad);
    let diags = syntax_diagnostics(codegen_bad, &lines);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].code,
        Some(NumberOrString::String("codegen".to_string()))
    );

    // Rustc JSON parser edge cases: invalid JSON, warning with children, aborting summary,
    // failure-note, fallback span without text or enclosing item.
    let rsharp_src = "void Foo()\n{\n    int x = 1;\n}\n";
    let rust_src = "fn Foo() {\n    let x: i32 = 1;\n}\n";
    let idx = LineIndex::new(rsharp_src);
    let tokens = rsharp_core::lexer::tokenize(rsharp_src).unwrap();
    let doc_idx = DocumentIndex::build(rsharp_src, &tokens);

    let json_lines = vec![
        "not json at all".to_string(),
        r#"{"level":"failure-note","message":"try --explain","spans":[]}"#.to_string(),
        r#"{"level":"error","message":"aborting due to 1 previous error","spans":[]}"#.to_string(),
        r#"{"level":"warning","message":"unused variable: `x`","code":{"code":"unused_variables"},"spans":[{"line_start":2,"is_primary":true,"label":"unused","text":[{"text":"    let x: i32 = 1;","highlight_start":9,"highlight_end":10}]}],"children":[{"level":"help","message":"prefix with `_`"},{"level":"note","message":""}]}"#.to_string(),
        r#"{"level":"error","message":"global error","spans":[{"line_start":0,"is_primary":true,"label":null,"text":[{"text":"   ","highlight_start":1,"highlight_end":3}]}]}"#.to_string(),
        r#"{"level":"error","message":"spanless error","spans":[]}"#.to_string(),
        r#"{"level":"error","message":"empty highlight","spans":[{"line_start":1,"is_primary":true,"label":null,"text":[{"text":"abc","highlight_start":2,"highlight_end":2}]}]}"#.to_string(),
    ];
    let parsed = parse_rustc_json_diagnostics(rust_src, &json_lines, rsharp_src, &idx, &doc_idx);
    assert_eq!(parsed.len(), 4);
    assert_eq!(parsed[0].severity, Some(DiagnosticSeverity::WARNING));
    assert!(parsed[0].message.contains("help: prefix with `_`"));
    assert_eq!(parsed[0].range.start.line, 2);
    assert_eq!(parsed[1].range.start.line, 0);
    assert_eq!(parsed[2].range.start.line, 0);

    // No enclosing Rust item and non-ident after `fn `
    let no_item = parse_rustc_json_diagnostics(
        "// only comment\nfn () {}\n",
        &[r#"{"level":"error","message":"err","spans":[{"line_start":2,"is_primary":true,"text":[]}]}"#.to_string()],
        rsharp_src,
        &idx,
        &doc_idx,
    );
    assert_eq!(no_item.len(), 1);
    assert_eq!(no_item[0].range.start.line, 0);

    // `semantic_diagnostics` returns empty when transpilation itself fails
    assert!(semantic_diagnostics("var bad = 1;", &LineIndex::new("var bad = 1;"), &doc_idx).is_empty());
}

#[test]
fn lsp_exhaustive_edge_cases_and_malformed_recovery() {
    // 1. Early channel drop before initialize returns Err; channel drop after init returns Ok.
    let (srv_fail, cli_fail) = Connection::memory();
    drop(cli_fail);
    assert!(run_connection(&srv_fail).is_err());

    let (srv_eof, cli_eof) = Connection::memory();
    let t = std::thread::spawn(move || run_connection(&srv_eof));
    cli_eof
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(1),
            "initialize".to_string(),
            json!({ "capabilities": {}, "initializationOptions": { "unrelated": 1 } }),
        )))
        .unwrap();
    let _ = cli_eof.receiver.recv().unwrap();
    cli_eof
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".to_string(),
            json!({}),
        )))
        .unwrap();
    // Send an unexpected Response message and malformed notifications, then close without shutdown.
    cli_eof
        .sender
        .send(Message::Response(Response::new_ok(RequestId::from(99), Value::Null)))
        .unwrap();
    for m in [
        "textDocument/didOpen",
        "textDocument/didSave",
        "textDocument/didClose",
    ] {
        cli_eof
            .sender
            .send(Message::Notification(Notification::new(
                m.to_string(),
                json!({ "malformed": true }),
            )))
            .unwrap();
    }
    drop(cli_eof);
    assert!(t.join().unwrap().is_ok());

    // 2. Full edge-case R# corpus hitting every scanner, semantic token, and handler branch
    let mut h = LspHarness::start(Some(json!({ "checkOnSave": false })));
    let uri_rich = test_uri("rich.rsharp");
    let rich_src = r#"[Derive(Debug)]
[unsafe NoMangle]
public(in crate.net) struct Container<T : Into<List<int>>, const nuint N = 2, _> where T : Clone
{
    public std.io.Error ErrField;
    public static Self Make() { return Self.Make(); }
    public async Task<int> Fetch(read this, [Allow(unused)], (int a, int b)) where T : Clone
    {
        List<List<int>> nested = [];
        shadow mut int val = 1;
        shadow int val = 2;
        if (val < 3) { _ = val; }
        _ = (val).to_string();
        _ = foo<List<int>>();
        _ = Fetch();
        return val;
    }
}

public enum Color { Red, Blue }
public trait Runnable { void Go(read this); }
public alias Id = int;
public const int K = 1;
macro my_m { () => {}; }
namespace Ns1 { public int Fn1() { return 1; } }

public (int, int) MakeTuple() { return (1, 2); }
public read (dyn Runnable) MakeDyn() {}
public int[4] MakeArr() { return [1, 2, 3, 4]; }
public void NestedGen<T : A<B>>() {}
"#;

    h.notify(
        "textDocument/didOpen",
        serde_json::to_value(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri_rich.clone(),
                language_id: "rsharp".to_string(),
                version: 1,
                text: rich_src.to_string(),
            },
        })
        .unwrap(),
    );
    let _ = h.recv_diagnostics();

    // Semantic tokens on rich_src (attr at idx 0, unsafe attr, async call, Self.Make, (val).to_string, < comparison & nested generic call)
    let sem_resp = h.request(
        "textDocument/semanticTokens/full",
        serde_json::to_value(SemanticTokensParams {
            text_document: TextDocumentIdentifier::new(uri_rich.clone()),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap(),
    );
    assert!(sem_resp.error.is_none());

    // General completion inside Fetch -> hits Namespace, Struct, Enum, Trait, Constant, TypeAlias, TypeParameter, Macro, and shadowed duplicate dedup
    let comp_inside = h.request(
        "textDocument/completion",
        serde_json::to_value(CompletionParams {
            text_document_position: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri_rich.clone()),
                find_pos(rich_src, "return val"),
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .unwrap(),
    );
    let CompletionResponse::Array(arr) =
        serde_json::from_value(comp_inside.result.unwrap()).unwrap()
    else {
        panic!("expected array");
    };
    let labels: Vec<&str> = arr.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"Ns1"));
    assert!(labels.contains(&"Color"));
    assert!(labels.contains(&"Runnable"));
    assert!(labels.contains(&"Id"));
    assert!(labels.contains(&"K"));
    assert!(labels.contains(&"my_m"));
    assert!(labels.contains(&"T"));

    // Dot-completion on `Color.` (EnumMember) and `unknown_recv.` (unresolved receiver)
    let uri_dot = test_uri("dot.rsharp");
    let dot_src = r#"public enum Color { Red, Blue }
public struct List { public void Add(read this) {} }
impl Color { public void Dup(read this) {} }
impl Color { public void Dup(read this) {} }
void f() { Color.; unknown_recv.; List x = new List {}; x.; Color.Missing; }
"#;
    h.notify(
        "textDocument/didOpen",
        serde_json::to_value(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri_dot.clone(),
                language_id: "rsharp".to_string(),
                version: 1,
                text: dot_src.to_string(),
            },
        })
        .unwrap(),
    );
    let _ = h.recv_diagnostics();

    let color_dot = find_pos(dot_src, "Color.;");
    let color_comp = h.request(
        "textDocument/completion",
        serde_json::to_value(CompletionParams {
            text_document_position: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri_dot.clone()),
                Position::new(color_dot.line, color_dot.character + 6),
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .unwrap(),
    );
    let CompletionResponse::Array(color_items) =
        serde_json::from_value(color_comp.result.unwrap()).unwrap()
    else {
        panic!("expected array");
    };
    assert!(color_items.iter().any(|i| i.label == "Red"));

    let unk_dot = find_pos(dot_src, "unknown_recv.;");
    let unk_comp = h.request(
        "textDocument/completion",
        serde_json::to_value(CompletionParams {
            text_document_position: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri_dot.clone()),
                Position::new(unk_dot.line, unk_dot.character + 13),
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .unwrap(),
    );
    let CompletionResponse::Array(unk_items) =
        serde_json::from_value(unk_comp.result.unwrap()).unwrap()
    else {
        panic!("expected array");
    };
    assert!(unk_items.is_empty());

    // Dot-completion on `x.` (user `List` overlapping builtin `List.Add`) & `Color.` (duplicate `Dup`)
    let x_dot = find_pos(dot_src, "x.;");
    let _ = h.request(
        "textDocument/completion",
        serde_json::to_value(CompletionParams {
            text_document_position: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(uri_dot.clone()),
                Position::new(x_dot.line, x_dot.character + 2),
            ),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .unwrap(),
    );

    // Hover & goto_definition on whitespace (`ident_at_position -> None`) and unresolved member (`Color.Missing`)
    let ws_pos = Position::new(4, 9); // space before `{`
    let missing_mem = find_pos(dot_src, "Missing;");
    for pos in [ws_pos, missing_mem] {
        for method in ["textDocument/hover", "textDocument/definition"] {
            let r = h.request(
                method,
                serde_json::to_value(HoverParams {
                    text_document_position_params: TextDocumentPositionParams::new(
                        TextDocumentIdentifier::new(uri_dot.clone()),
                        pos,
                    ),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                })
                .unwrap(),
            );
            assert_eq!(r.result, Some(Value::Null));
        }
    }

    // Requests & notifications on an unknown/unopened URI
    let missing_uri = test_uri("does_not_exist.rsharp");
    h.notify(
        "textDocument/didSave",
        serde_json::to_value(DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier::new(missing_uri.clone()),
            text: None,
        })
        .unwrap(),
    );
    for method in [
        "textDocument/hover",
        "textDocument/definition",
        "textDocument/completion",
    ] {
        let r = h.request(
            method,
            serde_json::to_value(HoverParams {
                text_document_position_params: TextDocumentPositionParams::new(
                    TextDocumentIdentifier::new(missing_uri.clone()),
                    Position::new(0, 0),
                ),
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .unwrap(),
        );
        assert_eq!(r.result, Some(Value::Null));
    }
    let sem_missing = h.request(
        "textDocument/semanticTokens/full",
        serde_json::to_value(SemanticTokensParams {
            text_document: TextDocumentIdentifier::new(missing_uri),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap(),
    );
    assert_eq!(sem_missing.result, Some(Value::Null));

    // Malformed / incomplete snippets exercising scanner recovery branches cleanly
    let malformed_snippets = [
        "namespace ;",
        "struct ;",
        "union ;",
        "enum ;",
        "trait ;",
        "alias ;",
        "macro ;",
        "macro bare_eof",
        "extern \"C\" void ext_fn();",
        "struct SpacedGen<T : Into<List<int> >> {}",
        "struct BrokenAngle<T;",
        "struct BadGen<123> {}",
        "struct BadBody { <int>(); }",
        "struct BadField { 123; }",
        "struct Unfin { int x }",
        "enum BadEnum { public <int>(); }",
        "enum ENoComma { A public void M() {} }",
        "enum E { ; /// trailing\n}",
        "struct S { public }",
        "struct S2 { /// trailing\n}",
        "struct Unclosed {",
        "struct UnclosedGen<T",
        "struct SemiGen<T>;",
        "123;",
        "void f() { foreach x; for y; foreach () {} for () {} foreach (int v in xs); int a = 1; int + b = 2; int _ = 3; read = 4; (1) = 5; x = 6; if (x is Some(n)) {} int unfin }",
        "public ",
        "/// trailing doc",
        "<int>();",
        "void>> ();",
        "void g<T : A<B>>(",
        "const read x = 1;",
        "void m() { if (a < b; c) {} }",
        ".orphan",
    ];
    for snippet in malformed_snippets {
        let toks = rsharp_core::lexer::tokenize(snippet).unwrap();
        let idx = DocumentIndex::build(snippet, &toks);
        let lines = LineIndex::new(snippet);
        let _ = idx.to_document_symbols(snippet, &lines);
        let _ = rsharp_lsp::semantic_tokens::compute_semantic_tokens(snippet, &toks, &lines, &idx);
    }

    h.shutdown();
}
