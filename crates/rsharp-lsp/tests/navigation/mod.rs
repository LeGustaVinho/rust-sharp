use super::*;
use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rsharp editor #{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        Self(root)
    }
    fn write(&self, name: &str, source: &str) -> String {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        rsharp_core::ra_oracle::path_to_file_uri(&path)
    }
    fn folder(&self) -> Value {
        json!({"name":"fixture","uri":rsharp_core::ra_oracle::path_to_file_uri(&self.0)})
    }
    fn start(&self) -> LspHarness {
        LspHarness::initialize(json!({"capabilities":{},"workspaceFolders":[self.folder()]}))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn open(h: &LspHarness, uri: &str, text: &str) {
    h.notify(
        "textDocument/didOpen",
        json!({"textDocument":{"uri":uri,"languageId":"rsharp","version":1,"text":text}}),
    );
}
fn cursor(uri: &str, text: &str, marked: &str) -> Value {
    let (left, right) = marked.split_once('|').unwrap();
    let offset = text.find(&format!("{left}{right}")).unwrap() + left.len();
    json!({"textDocument":{"uri":uri},"position":LineIndex::new(text).offset_to_position(text,offset)})
}
fn resolve(h: &mut LspHarness, method: &str, uri: &str, text: &str, marked: &str) -> Value {
    h.request(method, cursor(uri, text, marked)).result.unwrap()
}
fn action(h: &mut LspHarness, uri: &str, text: &str, marked: &str) -> Value {
    let pos = cursor(uri, text, marked)["position"].clone();
    h.request("textDocument/codeAction",json!({"textDocument":{"uri":uri},"range":{"start":pos,"end":pos},"context":{"diagnostics":[]}})).result.unwrap()
}
fn apply(text: &str, changes: &Value) -> String {
    let index = LineIndex::new(text);
    let mut edits: Vec<lsp_types::TextEdit> = serde_json::from_value(changes.clone()).unwrap();
    edits.sort_unstable_by_key(|e| std::cmp::Reverse(e.range.start));
    let mut result = text.to_owned();
    for e in edits {
        let start = index.position_to_offset(text, e.range.start);
        let end = index.position_to_offset(text, e.range.end);
        result.replace_range(start..end, &e.new_text);
    }
    result
}

#[test]
fn navigate_and_edit_a_converted_project_with_closed_modules() {
    let fixture = Fixture::new();
    let model = "public enum Status { Ready, Failed(string), Details { int Code; }, }\n";
    let other = "public enum Status { Ready, Other, }\n";
    let main = r#"namespace model;
namespace other;
using State = @crate.model.Status;
using @crate.other.{Status};
State CurrentStatus() { return State.Ready; }
int Run() {
    var status = CurrentStatus();
    Status different = Status.Ready;
    return match (status) {
        State.Ready => 1,
    };
}
"#;
    let main_uri = fixture.write("src/main.rsharp", main);
    let model_uri = fixture.write("src/model.rsharp", model);
    let other_uri = fixture.write("src/other.rsharp", other);
    let mut h = fixture.start();
    open(&h, &main_uri, main);
    let at = |text: &str, name: &str, skip: usize| {
        let offset = text.find(name).unwrap() + skip;
        LineIndex::new(text).offset_to_position(text, offset)
    };
    let cursor = |name: &str, skip: usize| json!({"textDocument": {"uri": main_uri}, "position": at(main, name, skip)});
    let mut request = |method: &str, params| h.request(method, params).result.unwrap();
    assert_eq!(
        request("textDocument/definition", cursor("State.Ready", 7))["uri"],
        model_uri
    );
    assert_eq!(
        request("textDocument/definition", cursor("Status.Ready", 8))["uri"],
        other_uri
    );
    assert_eq!(
        request("textDocument/definition", cursor("State.Ready", 1))["range"]["start"],
        json!(find_pos(model, "Status"))
    );
    assert_eq!(
        request("textDocument/typeDefinition", cursor("match (status)", 8))["uri"],
        model_uri
    );
    let choices = request("textDocument/completion", cursor("State.Ready", 6));
    assert_eq!(
        choices
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Ready", "Failed", "Details"]
    );
    let mut refs = cursor("State.Ready", 7);
    refs["context"] = json!({"includeDeclaration": true});
    assert_eq!(
        request("textDocument/references", refs)
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(request("textDocument/prepareRename", cursor("State.Ready", 7)).is_object());
    let mut rename = cursor("State.Ready", 7);
    rename["newName"] = json!("Available");
    let edits = request("textDocument/rename", rename);
    let documents = edits["documentChanges"].as_array().unwrap();
    assert_eq!(documents.len(), 2);
    for d in documents {
        let id = &d["textDocument"]["uri"];
        assert!(id == &main_uri || id == &model_uri);
        for e in d["edits"].as_array().unwrap() {
            assert_eq!(e["newText"], "Available");
        }
    }
    let pos = at(main, "match (status)", 1);
    let actions = request(
        "textDocument/codeAction",
        json!({"textDocument":{"uri":main_uri},
        "range":{"start":pos,"end":pos},"context":{"diagnostics":[]}}),
    );
    let insertion = &actions[0]["edit"]["documentChanges"][0]["edits"][0]["newText"]
        .as_str()
        .unwrap();
    assert!(insertion.contains("State.Failed(..)"));
    assert!(insertion.contains("State.Details { .. }"));
    assert!(!insertion.contains("Ready"));
    let input = main.replace(
        "        State.Ready => 1,",
        &format!("        State.Ready => 1,\n{insertion}"),
    );
    rsharp_core::parser::parse_file(&input).unwrap();
    open(&h, &model_uri, "public enum Status { Unsaved, }");
    assert!(
        h.request("textDocument/definition", cursor("State.Ready", 7))
            .result
            .unwrap()
            .is_null()
    );
    h.notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":model_uri}}),
    );
    assert_eq!(
        h.request("textDocument/definition", cursor("State.Ready", 7))
            .result
            .unwrap()["uri"],
        model_uri
    );
    std::fs::remove_file(fixture.0.join("src/model.rsharp")).unwrap();
    // VS Code percent-encodes the drive separator and lowercases drive letters on Windows.
    #[cfg(windows)]
    let model_uri = format!(
        "file:///{}%3A{}",
        model_uri[8..9].to_ascii_lowercase(),
        &model_uri[10..]
    );
    h.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":model_uri,"type":3}]}),
    );
    assert!(
        h.request("textDocument/definition", cursor("State.Ready", 7))
            .result
            .unwrap()
            .is_null()
    );
    h.shutdown();
}

#[test]
fn navigation_follows_nested_imports_reexports_and_generic_members() {
    let f = Fixture::new();
    let model = f.write(
        "src/model/mod.rsharp",
        "public enum Status<T> { Ready, Failed(T), }\n",
    );
    let other = f.write("src/other.rsharp", "public enum Status { Ready, Other, }");
    f.write(
        "src/facade.rsharp",
        "public using Renamed = @crate.model.Status;\npublic using @crate.other.*;",
    );
    f.write("src/loopback.rsharp", "public using @crate.loopback.*;");
    let text = r#"namespace model;
namespace other;
namespace facade;
using { @crate.facade.{Renamed, Status}, Group = @crate.model };
alias Number = Renamed<int>;
Number Get() { return Renamed<int>.Ready; }
void Run() {
    var val = Get();
    Renamed<int> a = Group.Status<int>.Ready;
    Status b = Status.Other;
    @crate.loopback.Missing();
    {
        using Renamed = @crate.other.Status;
        Renamed local = Renamed.Other;
    }
}
"#;
    let uri = f.write("src/main.rsharp", text);
    let mut h = f.start();
    open(&h, &uri, text);
    for marker in [
        "Renamed<int>.Re|ady",
        "Group.Status<int>.Re|ady",
        "facade.{Ren|amed",
        "Number Ge|t()",
    ] {
        let method = if marker.starts_with("Number") {
            "textDocument/typeDefinition"
        } else {
            "textDocument/definition"
        };
        assert_eq!(
            resolve(&mut h, method, &uri, text, marker)["uri"],
            model,
            "{marker}"
        );
    }
    for marker in [
        "Status.Ot|her",
        "Renamed.Ot|her",
        "facade.{Renamed, St|atus",
    ] {
        assert_eq!(
            resolve(&mut h, "textDocument/definition", &uri, text, marker)["uri"],
            other,
            "{marker}"
        );
    }
    assert_eq!(
        resolve(
            &mut h,
            "textDocument/typeDefinition",
            &uri,
            text,
            "var v|al"
        )["uri"],
        model
    );
    assert!(
        resolve(
            &mut h,
            "textDocument/definition",
            &uri,
            text,
            "loopback.Mi|ssing"
        )
        .is_null()
    );
    h.shutdown();
}

#[test]
fn rename_edits_only_identifiers_including_interpolation_and_refuses_conflicts() {
    let f = Fixture::new();
    let model = "public enum Status { Ready, Failed, }\n";
    let def = f.write("src/model.rsharp", model);
    let text = r#"using S = @crate.model.Status;
using Available = @crate.model.Status.Ready;
void main() {
    var a = S.Ready;
    var b = Available;
    string s = $"literal S.Ready {{Ready}} {  S.Ready  :?} / {S.Ready}";
    string t = "S.Ready";
    // S.Ready remains in the comment
    rust { let _ = "S.Ready"; }
}
"#;
    let uri = f.write("src/main.rsharp", text);
    let mut h = f.start();
    open(&h, &uri, text);
    assert_eq!(
        resolve(
            &mut h,
            "textDocument/definition",
            &uri,
            text,
            "  S.Re|ady  :?"
        )["uri"],
        def
    );
    assert!(
        resolve(
            &mut h,
            "textDocument/definition",
            &uri,
            text,
            "literal S.Re|ady"
        )
        .is_null()
    );
    let at = cursor(&uri, text, "a = S.Re|ady");
    let mut refs = at.clone();
    refs["context"] = json!({"includeDeclaration":false});
    let locations = h.request("textDocument/references", refs).result.unwrap();
    assert_eq!(locations.as_array().unwrap().len(), 6, "{locations}");
    let rename = |h: &mut LspHarness, name: &str| {
        let mut p = at.clone();
        p["newName"] = json!(name);
        h.request("textDocument/rename", p)
    };
    for invalid in ["_", "enum", "a b", "   ", "\"", "Failed"] {
        assert!(rename(&mut h, invalid).error.is_some(), "{invalid}");
    }
    let edit = rename(&mut h, "@type").result.unwrap();
    let changes = edit["documentChanges"].as_array().unwrap();
    let consumer = changes
        .iter()
        .find(|d| d["textDocument"]["uri"] == uri)
        .unwrap();
    assert_eq!(consumer["textDocument"]["version"], 1);
    let output = apply(text, &consumer["edits"]);
    assert!(output.contains("Status.@type;"));
    assert!(output.contains("var b = Available;"));
    assert!(output.contains("literal S.Ready {{Ready}} {  S.@type  :?} / {S.@type}"));
    assert!(output.contains("string t = \"S.Ready\""));
    assert!(output.contains("// S.Ready"));
    assert!(output.contains("rust { let _ = \"S.Ready\""));
    assert!(
        resolve(
            &mut h,
            "textDocument/prepareRename",
            &uri,
            text,
            "b = Avai|lable"
        )
        .is_null()
    );
    let stale = f.write(
        "src/model.rsharp",
        "public enum Status { Empty, Ready, Failed, }",
    );
    assert!(
        rename(&mut h, "Done")
            .error
            .unwrap()
            .message
            .contains("changed on disk")
    );
    h.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":stale,"type":2}]}),
    );
    assert!(rename(&mut h, "Done").error.is_none());
    let invalid = f.write("src/invalid.rsharp", "enum E {}");
    open(&h, &invalid, "\"unterminated Ready");
    assert!(
        rename(&mut h, "Done")
            .error
            .unwrap()
            .message
            .contains("lexical error")
    );
    h.shutdown();
}

#[test]
fn match_action_preserves_patterns_comments_and_line_endings() {
    let declarations = "enum E { A, B(string), C { int X; }, @enum, }\n";
    let uri = "untitled:match";
    let mut h = LspHarness::start(None);
    for (arms, missing) in [
        ("", vec!["A", "B(..)", "C { .. }", "@enum"]),
        ("E.A | E.@enum => 7,", vec!["B(..)", "C { .. }"]),
        (
            "E.B(string s) => 0, E.C { X: int x } => 2,",
            vec!["A", "@enum"],
        ),
        ("E.B(value) => 1, E.C { .. } => 2,", vec!["A", "@enum"]),
        (
            "E.A when (false) => 0, E.B(\"x\") => 2",
            vec!["A", "B(..)", "C { .. }", "@enum"],
        ),
        (
            "E.A => 1 // keep this\r\n\t",
            vec!["B(..)", "C { .. }", "@enum"],
        ),
        ("_ => 3", vec![]),
        ("everything => 3", vec![]),
        (
            "(E.A | E.@enum) => 1, E.B(..) => 2, E.C { .. } => 3,",
            vec![],
        ),
    ] {
        let text = format!(
            "{}int F(E v) {{\r\n\treturn match (v) {{{arms}}};\r\n}}",
            declarations.replace('\n', "\r\n")
        );
        open(&h, uri, &text);
        let actions = action(&mut h, uri, &text, "ma|tch (v)");
        if missing.is_empty() {
            assert!(actions.is_null(), "{arms}: {actions}");
            continue;
        }
        let applied = apply(&text, &actions[0]["edit"]["documentChanges"][0]["edits"]);
        assert_eq!(
            applied.matches("todo!()").count(),
            missing.len(),
            "{arms}: {applied}"
        );
        for pattern in missing {
            assert!(
                applied.contains(&format!("E.{pattern} => todo!(),")),
                "{applied}"
            );
        }
        assert!(!applied.replace("\r\n", "").contains('\n'));
        rsharp_core::parser::parse_file(&applied).unwrap();
        open(&h, uri, &applied);
        assert!(
            action(&mut h, uri, &applied, "ma|tch (v)").is_null(),
            "{applied}"
        );
    }
    for invalid in [
        "void F() {}",
        "int F(int a) {return match (a) { };}",
        "int F(E a) {return match (a) { E.A => };}",
        "int F(E a) {return match (a) {",
    ] {
        let text = format!("{declarations}{invalid}");
        open(&h, uri, &text);
        let marker = if invalid.contains("match") {
            "ma|tch"
        } else {
            "void F|()"
        };
        assert!(action(&mut h, uri, &text, marker).is_null());
    }
    let response = h
        .request("rsharp/rustForSync", json!({"text":"void main() {}"}))
        .result
        .unwrap();
    let generated = rsharp_core::transpile_to_rust(
        "void main() {}",
        rsharp_core::TranspileOptions {
            prelude_mode: rsharp_core::PreludeMode::None,
        },
    )
    .unwrap();
    assert_eq!(response, generated);
    assert!(
        h.request("rsharp/rustForSync", json!({"text":"void ("}))
            .error
            .is_some()
    );
    h.shutdown();
}

#[test]
fn workspace_changes_isolate_conversion_copies_and_follow_disk_updates() {
    let f = Fixture::new();
    let consumer = "using State = @crate.model.Status;\nState F() { return State.Ready; }";
    let uri = f.write("rsharp-sample/main.rsharp", consumer);
    let model = f.write(
        "rsharp-sample/model.rsharp",
        "public enum Status { Ready, }",
    );
    f.write(
        "rsharp-sampl2/model.rsharp",
        "public enum Status { Ready, Other, }",
    );
    f.write("target/model.rsharp", "public enum Status { Ready, }");
    let mut h = LspHarness::start(None);
    open(&h, &uri, consumer);
    let get =
        |h: &mut LspHarness| resolve(h, "textDocument/definition", &uri, consumer, "State.Re|ady");
    assert!(get(&mut h).is_null());
    h.notify(
        "workspace/didChangeWorkspaceFolders",
        json!({"event":{"added":[f.folder()],"removed":[]}}),
    );
    assert_eq!(get(&mut h)["uri"], model);
    open(&h, &model, "public enum Status { Changed, }");
    h.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":model,"type":2}]}),
    );
    assert!(get(&mut h).is_null());
    h.notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":model}}),
    );
    assert_eq!(get(&mut h)["uri"], model);
    h.notify(
        "workspace/didChangeWorkspaceFolders",
        json!({"event":{"added":[],"removed":[f.folder()]}}),
    );
    assert!(get(&mut h).is_null());
    h.shutdown();
}

#[test]
fn type_navigation_handles_constructor_calls_methods_and_ambiguous_names() {
    let fixture = Fixture::new();
    let a = fixture.write("src/a.rsharp", "public enum E<T> { A, B(T), }");
    fixture.write("src/b.rsharp", "public enum E<T> { A, C(T), }");
    let root = fixture.write("src/root.rsharp", "public enum Unique { One, }");
    let text = r#"namespace a; namespace b;
using Typed = @crate.a.E;
struct Holder {
    public Typed<int> item;
    public Typed<int> ReadItem(read this) { var me = this; return this.item; }
}
Holder Make() { return new Holder { item = Typed<int>.A }; }
alias Recursive = Recursive;
void F() {
   var h = new Holder { item = Typed<int>.A };
   var a = h.item;
   var b = Make().item;
   var c = Make().ReadItem();
   var unknown = 7 + 1;
   var nested = Typed<Holder<int>>.A;
   Unique single = Unique.One;
   E<int> ambiguous = E<int>.A;
   var r = Recursive;
}
"#;
    let uri = fixture.write("src/main.rsharp", text);
    let mut h = fixture.start();
    open(&h, &uri, text);
    for marker in [
        "var a| =",
        "var b| =",
        "var c| =",
        "Make().ite|m;",
        "Typed<Holder<int>>.A|;",
        "var nes|ted",
    ] {
        assert_eq!(
            resolve(&mut h, "textDocument/typeDefinition", &uri, text, marker)["uri"],
            a,
            "{marker}"
        );
    }
    assert_eq!(
        resolve(&mut h, "textDocument/typeDefinition", &uri, text, "var m|e")["uri"],
        uri
    );
    assert_eq!(
        resolve(&mut h, "textDocument/definition", &uri, text, "Unique.O|ne")["uri"],
        root
    );
    assert!(
        resolve(
            &mut h,
            "textDocument/definition",
            &uri,
            text,
            "ambiguous = E<int>.A|"
        )
        .is_null()
    );
    assert!(
        resolve(
            &mut h,
            "textDocument/typeDefinition",
            &uri,
            text,
            "var unk|nown"
        )
        .is_null()
    );
    assert!(
        resolve(
            &mut h,
            "textDocument/typeDefinition",
            &uri,
            text,
            "var r| ="
        )
        .is_null()
    );
    h.shutdown();
}

#[test]
fn conflicting_imports_and_nested_matches_are_handled_in_scope() {
    let fixture = Fixture::new();
    let ea = fixture.write("src/a.rsharp", "public enum E { A, B, }");
    fixture.write("src/b.rsharp", "public enum E { A, C, }");
    let facade = fixture.write(
        "src/facade.rsharp",
        "public using @crate.a.*; public using @crate.b.*;",
    );
    let uri = fixture.write("src/main.rsharp", "");
    let mut h = fixture.start();
    for (header, expected) in [
        ("using @crate.a.*;", Some(&ea)),
        ("using @crate.a.*; using @crate.b.*;", None),
        ("using @crate.a.E; using @crate.b.E;", None),
        ("using @crate.a.{this, E}; using @crate.a.E;", Some(&ea)),
        ("using @crate.facade.E;", None),
    ] {
        let text = format!("{header} void M() {{ E data = E.A; }}");
        open(&h, &uri, &text);
        let hit = resolve(
            &mut h,
            "textDocument/definition",
            &uri,
            &text,
            "data = E|.A",
        );
        assert_eq!(
            hit["uri"].as_str(),
            expected.map(String::as_str),
            "{header}"
        );
    }
    open(&h, &facade, "public using @crate.a.*;");
    let parent = "using @crate.a.E; enum Unrelated { Outside, } int Test(E e, Unrelated u) {return match (u) { Unrelated.Outside => match (e) { E.A => 7 }, };}";
    open(&h, &uri, parent);
    let edit = action(&mut h, &uri, parent, "ma|tch (e)");
    let updated = apply(parent, &edit[0]["edit"]["documentChanges"][0]["edits"]);
    assert!(updated.contains("E.B => todo!()"));
    assert_eq!(updated.matches("todo!()").count(), 1);
    let at = cursor(&uri, parent, "ma|tch (e)")["position"].clone();
    assert!(
        h.request(
            "textDocument/codeAction",
            json!({"textDocument":{"uri":uri},"range":{"start":at,"end":at},
        "context":{"diagnostics":[],"only":["source.organizeImports"]}})
        )
        .result
        .unwrap()
        .is_null()
    );
    h.shutdown();
}

#[test]
fn rename_prevents_binding_capture_and_ignores_opaque_or_incomplete_text() {
    let uri = "untitled:scope";
    let mut h = LspHarness::start(None);
    let text = "int Sum(int a) { int later = 2; return a + later; }";
    open(&h, uri, text);
    let mut p = cursor(uri, text, "int a|)");
    p["newName"] = json!("later");
    assert!(
        h.request("textDocument/rename", p)
            .error
            .unwrap()
            .message
            .contains("conflict")
    );
    for src in [
        "using bad + ;",
        "using { , };",
        "using unfinished",
        r#"void F(){string s = $"nested { $"inner {printable}" }";}"#,
        r#"void F(){string s = $"invalid { ? @ }";}"#,
    ] {
        open(&h, uri, src);
        assert!(
            h.request(
                "textDocument/documentSymbol",
                json!({"textDocument":{"uri":uri}})
            )
            .error
            .is_none()
        );
    }
    open(&h, uri, "enum E { A, } void M() { E e = E.A; }");
    for bad in [
        (json!(null), "textDocument/rename"),
        (json!({"unexpected":true}), "rsharp/rustForSync"),
    ] {
        assert!(h.request(bad.1, bad.0).error.is_some());
    }
    open(&h, uri, "\"unterminated");
    assert!(action(&mut h, uri, "\"unterminated", "un|terminated").is_null());
    h.notify("workspace/didChangeWatchedFiles",json!({"changes":[{"uri":"untitled:other","type":2},{"uri":"file:///outside.txt","type":3}]}));
    h.shutdown();
}

#[test]
fn qualified_match_patterns_use_the_scrutinee_enum_and_correct_module() {
    let f = Fixture::new();
    let e = f.write(
        "src/models.rsharp",
        "public enum State { Pair((int,int)), Done, public int Method() {return 1;} }",
    );
    let text = r#"namespace models;
struct State { public int No; }
@crate.models.State Get() { return @crate.models.State.Done; }
int Test() { return match (Get()) { @crate.models.State.Pair((int x, int y)) => x, }; }
int Other(State s) { return match (s) { }; }
"#;
    let uri = f.write("src/main.rsharp", text);
    let mut h = f.start();
    open(&h, &uri, text);
    let changes = action(&mut h, &uri, text, "mat|ch (Get");
    let new = apply(text, &changes[0]["edit"]["documentChanges"][0]["edits"]);
    assert!(new.contains("@crate.models.State.Done => todo!(),"));
    assert_eq!(new.matches("todo!()").count(), 1);
    assert!(action(&mut h, &uri, text, "mat|ch (s)").is_null());
    let full = "namespace nested { public using Alias = super.models.State; void F(){ Alias value = Alias.Done; } }";
    open(&h, &uri, full);
    assert_eq!(
        resolve(&mut h, "textDocument/definition", &uri, full, "Alias.Do|ne")["uri"],
        e
    );
    let module = "namespace inner { public enum State { A, } State F() { return this.State.A; } }";
    open(&h, &uri, module);
    assert_eq!(
        resolve(
            &mut h,
            "textDocument/definition",
            &uri,
            module,
            "this.State.A|;"
        )["range"]["start"],
        json!(find_pos(module, "A,"))
    );
    let malformed = "void F(){ match value }";
    open(&h, &uri, malformed);
    assert!(action(&mut h, &uri, malformed, "mat|ch").is_null());
    h.shutdown();
}
