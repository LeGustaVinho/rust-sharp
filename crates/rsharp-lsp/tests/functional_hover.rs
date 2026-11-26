use lsp_types::{
    DidOpenTextDocumentParams, HoverContents, HoverParams, TextDocumentIdentifier,
    TextDocumentItem, TextDocumentPositionParams, Uri, WorkDoneProgressParams,
};
use rsharp_lsp::{handlers::ServerState, position::LineIndex};
use std::str::FromStr;

#[test]
fn functional_method_hovers_follow_the_receiver() {
    let source = r#"
public struct Custom {
    /// My application-specific mapping.
    public int map(read this) { return 1; }
    public Option<int> iter(read this) { return Some(1); }
}
Option<int> make_option() { return Some(1); }
void demo(Option<int> option, Result<int, string> result, List<int> items, Custom custom,
          int? nullable, int[3] array, read slice<int> values, AtomicUsize atom,
          SortedDictionary<int, int> dict, PriorityQueue<int> heap, Queue<int> queue,
          Dictionary<int, int> map, SortedSet<int> set, string text) {
    option.map(x => x + 1);
    result.map(x => x + 1);
    items.iter().map(x => x + 1).filter(x => x > 2).collect<List<int>>();
    items.retain(x => x > 0);
    items.into_iter().peekable().next_if(x => x > 0);
    items.extend(other);
    Vec.from_iter(other);
    nullable.and_then(x => Some(x));
    array.map(x => x);
    values.chunk_by((a, b) => a == b);
    atom.try_update(x => Some(x + 1));
    dict.extract_if(.., (k, v) => k > 0);
    heap.retain(x => x > 0);
    queue.retain(x => x > 0);
    map.retain((k, v) => k > 0);
    set.retain(x => x > 0);
    text.trim_end_matches(c => c == ' ');
    $"example {1}".retain(c => c == 'a');
    false.then(() => 1);
    Some(1).map(x => x);
    Ok(1).map(x => x);
    custom.iter().map(x => x);
    make_option().map(x => x);
    make_unknown().flat_map(f);
    "école".trim_matches(c => c == 'é');
    @std.cell.Ref.map(custom, x => x);
    custom.map();
    custom.filter();
    items.retain;
    map(x => x).filter(free);
    items.iter().map(x => f(x)).filter(nested);
    var unknown = make_unknown();
    unknown.map(x => x);
    filter(x => x);
    // option.map(comment)
    string sample = "option.map(text)";
}
).map(malformed);
"#;
    let uri = Uri::from_str("file:///functional.rsharp").unwrap();
    let mut server = ServerState::default();
    server.did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem::new(uri.clone(), "rsharp".into(), 1, source.into()),
    });
    let lines = LineIndex::new(source);
    let hover = |needle: &str, method: &str| {
        let offset = source.find(needle).unwrap() + needle.find(method).unwrap();
        server
            .hover(HoverParams {
                text_document_position_params: TextDocumentPositionParams::new(
                    TextDocumentIdentifier::new(uri.clone()),
                    lines.offset_to_position(source, offset),
                ),
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .map(|hover| {
                let HoverContents::Markup(markup) = hover.contents else {
                    panic!("expected Markdown hover");
                };
                assert_eq!(
                    hover.range.unwrap().end,
                    lines.offset_to_position(source, offset + method.len())
                );
                markup.value
            })
    };

    for (needle, method, expected) in [
        ("option.map", "map", "Option::map"),
        ("result.map", "map", "Result::map"),
        ("iter().map", "map", "Iterator::map"),
        ("filter(x => x > 2)", "filter", "Iterator::filter"),
        ("collect<List", "collect", "Iterator::collect"),
        ("items.retain", "retain", "Vec::retain"),
        ("items.into_iter", "into_iter", "IntoIterator::into_iter"),
        ("next_if(x", "next_if", "Peekable::next_if"),
        ("items.extend", "extend", "Extend::extend"),
        ("Vec.from_iter", "from_iter", "FromIterator::from_iter"),
        ("nullable.and_then", "and_then", "Option::and_then"),
        ("array.map", "map", "[T; N]::map"),
        ("values.chunk_by", "chunk_by", "[T]::chunk_by"),
        ("atom.try_update", "try_update", "AtomicUsize::try_update"),
        ("dict.extract_if", "extract_if", "BTreeMap::extract_if"),
        ("heap.retain", "retain", "BinaryHeap::retain"),
        ("queue.retain", "retain", "VecDeque::retain"),
        ("map.retain", "retain", "HashMap::retain"),
        ("set.retain", "retain", "BTreeSet::retain"),
        (
            "text.trim_end_matches",
            "trim_end_matches",
            "str::trim_end_matches",
        ),
        ("{1}\".retain", "retain", "String::retain"),
        (
            "filter(free)",
            "filter",
            "Possible Rust standard-library methods",
        ),
        ("filter(nested)", "filter", "Iterator::filter"),
        (
            "map(malformed)",
            "map",
            "Possible Rust standard-library methods",
        ),
        ("false.then", "then", "bool::then"),
        ("Some(1).map", "map", "Option::map"),
        ("Ok(1).map", "map", "Result::map"),
        ("custom.iter().map", "map", "Option::map"),
        ("make_option().map", "map", "Option::map"),
        ("make_unknown().flat_map", "flat_map", "Iterator::flat_map"),
        ("trim_matches(c", "trim_matches", "str::trim_matches"),
        ("Ref.map(custom", "map", "Ref::map"),
        ("custom.map", "map", "My application-specific mapping."),
        (
            "unknown.map",
            "map",
            "Possible Rust standard-library methods",
        ),
    ] {
        let text = hover(needle, method).unwrap_or_else(|| panic!("no hover for {needle}"));
        assert!(text.contains(expected), "{needle}: {text}");
    }

    let option_map = hover("option.map", "map").unwrap();
    assert!(option_map.contains("Transforms the value in `Some` and propagates `None`."));
    assert!(!option_map.contains("Result::map"));
    assert!(!hover("iter().map", "map").unwrap().contains("Option::map"));
    assert!(hover("custom.filter", "filter").is_none());
    assert!(hover("items.retain;", "retain").is_none());
    assert!(hover("    filter(x", "filter").is_none());
    assert!(hover("option.map(comment)", "map").is_none());
    assert!(hover("option.map(text)", "map").is_none());
}

#[test]
fn functional_hover_ignores_stale_tokens_while_editing() {
    use lsp_types::{
        DidChangeTextDocumentParams, Position, TextDocumentContentChangeEvent,
        VersionedTextDocumentIdentifier,
    };

    let uri = Uri::from_str("file:///editing.rsharp").unwrap();
    let params = HoverParams {
        text_document_position_params: TextDocumentPositionParams::new(
            TextDocumentIdentifier::new(uri.clone()),
            Position::new(0, 9),
        ),
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let mut server = ServerState::default();
    server.did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem::new(
            uri.clone(),
            "rsharp".into(),
            1,
            "Some(1).map(f)".into(),
        ),
    });
    assert!(server.hover(params.clone()).is_some());
    server.did_change(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier::new(uri, 2),
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "Some(1).sum(f); \"".into(),
        }],
    });
    assert!(server.hover(params).is_none());
}
