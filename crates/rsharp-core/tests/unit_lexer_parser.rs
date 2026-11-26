use rsharp_core::error::{RSharpError, Span};
use rsharp_core::lexer::tokenize;
use rsharp_core::parser::{parse_expr_str, parse_file};
use rsharp_core::token::{Keyword, TokenKind};
use rsharp_core::{PreludeMode, TranspileOptions, transpile_to_rust};

#[test]
fn test_all_keywords_roundtrip() {
    let keywords = [
        "read",
        "write",
        "take",
        "slice",
        "deref",
        "ptr",
        "life",
        "try",
        "shadow",
        "require",
        "rust",
        "macro",
        "void",
        "never",
        "dynamic",
        "dyn",
        "impl",
        "alias",
        "type",
        "struct",
        "enum",
        "union",
        "trait",
        "Self",
        "this",
        "public",
        "internal",
        "private",
        "mut",
        "const",
        "static",
        "async",
        "unsafe",
        "extern",
        "super",
        "crate",
        "if",
        "else",
        "while",
        "for",
        "foreach",
        "in",
        "loop",
        "match",
        "when",
        "is",
        "not",
        "return",
        "break",
        "continue",
        "await",
        "with",
        "new",
        "as",
        "where",
        "namespace",
        "using",
        "true",
        "false",
        "var",
        "let",
    ];
    for kw in keywords {
        let parsed = Keyword::parse(kw).expect("keyword must parse");
        assert_eq!(parsed.as_str(), kw);
    }
    assert_eq!(Keyword::parse("not_a_keyword"), None);
}

#[test]
fn test_lexer_literals_operators_comments_and_verbatim_idents() {
    let src = r#"
        // line comment
        //// quadruple slash ignored as normal comment
        /// doc line
        /* nested /* block */ comment */
        @read @write @new
        0xFF_u32 0b1010_u8 0o77 42 1_000i64 3.14 2.5e-3 1E+4 5f32
        "hello \"world\"\n" 'a' '\n'
        $"escaped {{brace}} \n {x} {y:.2} {(a + b)} {"a\"b"} {rust { 1 }}"
        + ++ += - -- -= -> * *= / /= % %= = == => ! != < <= << <<= > >= >> >>=
        & && &= | || |= ^ ^= ~ : :: . .. ..= ? ?? , ; ( ) { } [ ]
        rust { let s = "}"; /* } */ // }
            let x = { 1 + 2 };
        }
        foo.rust
    "#;
    let toks = tokenize(src).expect("should lex cleanly");
    assert!(
        toks.iter()
            .any(|t| matches!(t.kind, TokenKind::DocComment(ref s) if s == "doc line"))
    );
    assert!(toks.iter().any(
        |t| matches!(t.kind, TokenKind::Ident { ref name, verbatim: true } if name == "read")
    ));
    assert!(toks.iter().any(
        |t| matches!(t.kind, TokenKind::RustBlock(ref s) if s.contains("let x = { 1 + 2 };"))
    ));
    assert!(
        toks.iter()
            .any(|t| matches!(t.kind, TokenKind::InterpString(_)))
    );
    assert!(
        toks.iter()
            .any(|t| matches!(t.kind, TokenKind::Kw(Keyword::Rust)))
    );
    assert_eq!(toks.last().unwrap().kind, TokenKind::Eof);
}

#[test]
fn test_lexer_error_cases() {
    let bad_inputs = [
        ("/* unterminated", "Unterminated block comment"),
        ("@123", "Expected identifier or string after '@'"),
        ("\"unterminated", "Unterminated string literal"),
        ("\"bad\\", "Unterminated escape sequence in string literal"),
        ("''", "Empty char literal"),
        ("'a", "Unterminated char literal"),
        ("'\\", "Unterminated escape sequence in char literal"),
        (
            "$\"unterminated",
            "Unterminated interpolated string literal",
        ),
        ("$\"bad\\", "Unterminated escape in interpolated string"),
        ("$\"{}\"", "Empty interpolation expression"),
        ("$\"{x", "Unterminated interpolation expression"),
        ("rust { let x = 1;", "Unterminated 'rust { ... }' block"),
        ("`", "Unexpected character"),
    ];
    for (src, expected_msg) in bad_inputs {
        let err = tokenize(src).expect_err(&format!("expected lex error for: {}", src));
        assert!(
            err.to_string().contains(expected_msg),
            "for `{}` got `{}` expected `{}`",
            src,
            err,
            expected_msg
        );
    }
}

#[test]
fn test_parser_strict_rejections() {
    // Accepts C#-style inferred `var`
    assert!(parse_file("void main() { var x = 10; }").is_ok());

    // Rejects `let`
    let err = parse_file("void main() { let x = 10; }").unwrap_err();
    assert!(err.to_string().contains("let"));

    // Rejects implicit return
    let err = parse_file("public int Add(int a, int b) { a + b }").unwrap_err();
    assert!(err.to_string().contains("Implicit return"));

    // Rejects unsized array `int[]`
    let err = parse_file("void main() { int[] x = [1]; }").unwrap_err();
    assert!(
        err.to_string()
            .contains("Array type requires explicit length")
    );

    // Rejects `require` with non-pattern condition
    let err = parse_file("void main() { require (true) else { return; } }").unwrap_err();
    assert!(err.to_string().contains("require"));

    // Rejects trailing expression tokens
    let err = parse_expr_str("1 + 2 3").unwrap_err();
    assert!(err.to_string().contains("Unexpected trailing"));

    // Rejects non-path trait in `impl ... for`
    let err = parse_file("impl (int, int) for Point {}").unwrap_err();
    assert!(err.to_string().contains("Expected trait path"));

    // Rejects forced local missing `=` or `;`
    let err = parse_file("void main() { mut int x 123 }").unwrap_err();
    assert!(err.to_string().contains("Expected '=' or ';'"));

    // Rejects `with` on arbitrary expression in codegen
    let err = transpile_to_rust(
        "void main() { int x = (1 + 2) with { A = 1 }; }",
        TranspileOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(err, RSharpError::Codegen { .. }));
}

#[test]
fn test_parser_full_syntax_surface() {
    let src = r#"
[module: Allow(dead_code)]
namespace App.Core;

using std.collections.Dictionary;
using std.io.*;
using Io = std.io;

/// Documentation on unit struct
public struct Marker;

public struct Meters(public double);

[Derive(Debug, Clone)]
internal struct Grid<life a, T : Clone + Copy, const nuint N>
    where T : Clone, Copy, life a : static
{
    public read<a> slice<T> Data;
    private T[N] Inline;

    public const int Rank(read this)
    {
        return 1;
    }

    public void Touch(write<a> this, mut int delta)
    {
        delta += 1;
        return;
    }

    public void Consume(take this)
    {
        return;
    }

    public void BoxedRecv(take Box<Self> this)
    {
        return;
    }

    public void PinRecv(std.pin.Pin<write Self> this)
    {
        return;
    }
}

public enum Packet
{
    Ping = 1,
    Data(byte Code, List<byte> Payload),
    Header { ushort Len; byte Flags; },

    public bool IsPing(read this)
    {
        return this is Packet.Ping;
    }
}

public union RawBits
{
    public int AsInt;
    public float AsFloat;
}

public unsafe trait RawStore : Send, Sync
{
    type Item : Clone = int;
    const nuint CAP = 16;
    read Self.Item Get(read this);
    void DefaultNoop(read this) { return; }
}

unsafe impl RawStore for Marker
{
    type Item = int;
    const nuint CAP = 32;
    read int Get(read this)
    {
        static int ZERO = 0;
        return read ZERO;
    }
}

public(super) alias Callback = (int, int) -> int;
public alias SingleTuple = (int,);
public const nuint LIMIT = 100;
static mut int GLOBAL_CTR = 0;

extern "C"
{
    int puts(read ptr<sbyte> s);
}

public extern "C" void OnSignal(int sig)
{
    _ = sig;
}

macro MyEcho { ($x:expr) => { $x }; }

println!("hello");

public never FailNow()
{
    panic("fatal");
}

public async Task AsyncVoid()
{
    return;
}

public async Task<int> AsyncVal(int x)
{
    impl std.future.Future<Output = int> fut = async take {
        return x + 1;
    };
    return await fut;
}

public impl Iterator<Item = int> MakeIter()
{
    List<int> v = [1, 2, 3];
    return v.into_iter();
}

namespace Nested
{
    public void SubFn()
    {
        return;
    }
}

[Inline(Always)]
[Cfg(test)]
[CustomAttr = "v"]
public void ExerciseAll()
{
    const int LOCAL_C = 7;
    mut int a = 1;
    shadow mut int a = a + LOCAL_C;
    a++;
    a--;
    ++a;
    --a;
    a += 2;
    a -= 1;
    a *= 3;
    a /= 2;
    a %= 5;
    a &= 7;
    a |= 2;
    a ^= 1;
    a <<= 1;
    a >>= 1;

    bool logic = !false && (true || false) && (a == 1 || a != 2) && (a < 10 && a <= 10 && a > 0 && a >= 0);
    int bits = (~a | (a & 3) ^ (a << 1) >> 1) + (a * 2 / 1 % 3) - (-a);
    double castVal = (a as double);
    _ = (logic, bits, castVal);

    int[4] zeros = [0; 4];
    List<int> dynZeros = [0; 3];
    List<int> emptyList = [];
    int[2] arr = [1, 2];
    (int,) single = (42,);
    () unitVal = ();
    (int first, mut int second) = (1, 2);
    second += single.0 + arr[0] + zeros[0] + dynZeros.len() as int + emptyList.len() as int;
    _ = (first, second, unitVal);

    int tern = a > 0 ? 10 : 20;
    int? opt = Some(tern);
    int coal = opt ?? 0;
    if (opt is not None)
    {
        _ = coal;
    }
    else if (opt is Some(int v))
    {
        _ = v;
    }
    else
    {
        return;
    }

    require (opt is Some(int extracted)) else { return; }
    _ = extracted;

    mut int? wopt = Some(1);
    while (wopt is Some(int wv))
    {
        _ = wv;
        wopt = None;
    }
    while (wopt is not None)
    {
        break;
    }

    outer: for (mut int i = 0; i < 5; i++)
    {
        if (i == 1) { continue outer; }
        if (i == 3) { break outer; }
    }
    for (;;)
    {
        break;
    }

    mut int idx = 0;
    wlbl: while (idx < 3)
    {
        idx++;
        continue wlbl;
    }

    llbl: loop
    {
        break llbl;
    }

    foreach (int item in 0..=3)
    {
        _ = item;
    }
    _ = (..5, ..=5, 1.., ..);

    int valBlk = blk: {
        int tmp = 10;
        break blk tmp + 5;
    };

    int mres = match (valBlk)
    {
        -1 | 0 => 0,
        1..=10 => 1,
        int n when n > 10 => { return; }
        _ => 2,
    };
    _ = mres;

    (int x) : int => x + 1;
    take (mut int x) => { x += 1; return x; };

    int unsafeExpr = unsafe { return 1; };
    _ = unsafeExpr;

    int rawExpr = rust { 1 + 1 };
    _ = rawExpr;

    read slice<int> sl = read slice arr[0..2];
    mut int[2] marr = [3, 4];
    write slice<int> wsl = write slice marr[..];
    _ = (sl, wsl);
}
"#;

    let ast = parse_file(src).expect("full syntax should parse");
    assert!(!ast.inner_attrs.is_empty());
    assert!(ast.items.len() >= 15);

    let rust_embedded = transpile_to_rust(
        src,
        TranspileOptions {
            prelude_mode: PreludeMode::Embedded,
        },
    )
    .expect("embedded transpile should succeed");
    assert!(rust_embedded.contains("pub mod App"));
    assert!(
        rust_embedded.contains("pub trait RawStore")
            || rust_embedded.contains("pub unsafe trait RawStore")
    );
    assert!(rust_embedded.contains("macro_rules! MyEcho"));

    let rust_crate = transpile_to_rust(
        src,
        TranspileOptions {
            prelude_mode: PreludeMode::CrateImport,
        },
    )
    .expect("crate transpile should succeed");
    assert!(rust_crate.contains("use rsharp_prelude::*;"));

    let rust_none = transpile_to_rust(
        src,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("none transpile should succeed");
    assert!(!rust_none.contains("rsharp_prelude"));
}

#[test]
fn test_error_display_variants() {
    let sp = Span::new(2, 5, 10, 15);
    let errs = [
        RSharpError::lex(sp, "bad char"),
        RSharpError::parse(sp, "bad tok"),
        RSharpError::codegen("bad gen"),
        RSharpError::from_rust("bad rs"),
        RSharpError::unresolved_type("x", "unknown"),
        RSharpError::compile("rustc failed"),
        RSharpError::io("missing file"),
    ];
    for e in errs {
        assert!(!e.to_string().is_empty());
    }
}

#[test]
fn test_codegen_and_parser_edge_cases() {
    let src = r#"
///
/// Multi-line doc with empty line
[module: Warn(unused)]
[serde.rename("x")]
[Inline(Never)]
[Inline(hint)]
[Test]
[CfgAttr(test, allow(dead_code))]
[Deny(warnings)]
[Repr(C)]
[NoMangle]
[MustUse]
[Deprecated]
[ShouldPanic]
[Ignore]
[NonExhaustive]
[Doc("hi")]
[Cold]
[TrackCaller]
[ProcMacro]
[ProcMacroDerive(Foo)]
[ProcMacroAttribute]
public struct TupleWithAttr([Allow(dead_code)] public sbyte, byte, short, ushort, uint, ulong, int128, uint128, nint, float);

namespace ExternalMod;

extern
{
    void raw_fn();
}

my_macro! { a = 1 }
my_list_macro![1, 2, 3];

public trait ExtraTrait
{
    type Assoc;
    const int NO_DEF;
    void TakeAll(read this);
}

public struct Boxed
{
    public int V;
}

impl Boxed
{
    public const int ID = 1;
    public Self CloneWith(read this, int v)
    {
        return this with { V = v };
    }
}

public void TestEdgeStatements(Boxed b, (int, int) pair)
{
    Boxed b2 = b with { V = 2 };
    _ = b2;
    (int) -> void cb = (int x) => { _ = x; };
    _ = cb;
    mut List<int> l = List<int>.new();
    _ = l;
    string plain = $"plain_only";
    _ = plain;
    match (pair)
    {
        (int x, int y) => { _ = x + y; }
    }
    match (b)
    {
        Boxed { mut int V } => { V += 1; _ = V; }
    }
    match (b)
    {
        Boxed { V: 0, .. } => { return; }
        Boxed { V: int k } => { _ = k; }
    }
    int loopVal = loop
    {
        break 42;
    };
    _ = loopVal;
    for (int k = 0; k < 1; )
    {
        _ = k;
        break;
    }
    int zeroInit;
    zeroInit = 1;
    _ = zeroInit;
    string msg = "boom".ToString();
    if (false)
    {
        panic(msg);
    }
    if (false)
    {
        panic("fmt {}", 1);
    }
    foo.bar![1, 2];
    foo.bar!{ a };
}
"#;
    let rust = transpile_to_rust(
        src,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("edge cases should transpile");
    assert!(rust.contains("mod ExternalMod;"));
    assert!(rust.contains("unsafe extern {"));
    assert!(rust.contains("my_macro!{ a = 1 }"));
    assert!(rust.contains("Boxed { V: v, ..self }"));
    assert!(rust.contains("format!(\"plain_only\")"));
    assert!(rust.contains("break 42;"));
    assert!(rust.contains("#[inline(never)]"));
    assert!(rust.contains("#[inline(hint)]"));
    assert!(rust.contains("#[no_mangle]"));

    // Error paths on `with`, unterminated delimiters, and high-level helpers
    assert!(
        transpile_to_rust(
            "void main() { int a = this with { X = 1 }; }",
            TranspileOptions::default()
        )
        .is_err()
    );
    assert!(
        transpile_to_rust(
            "void main() { int x = 1; int y = x with { A = 1 }; }",
            TranspileOptions::default()
        )
        .is_err()
    );
    assert!(parse_file("[Derive(Debug").is_err());
    assert!(parse_file("void main() { int x = ; }").is_err());
    assert!(parse_file("void main() { mut 123; }").is_err());
    assert!(parse_file("void main() { shadow int 123; }").is_err());
    assert!(parse_file("void main() { int x = 1 +; }").is_err());
    assert!(parse_file("struct 123 {}").is_err());
    assert!(parse_file("public(foo) void f() {}").is_err());
    assert!(parse_file("void f(int x) { return 1").is_err());

    let tmp_bin = std::env::temp_dir().join("rsharp_never_created_bin");
    assert!(rsharp_core::build_rsharp("void main() { let x = 1; }", &tmp_bin).is_err());
    assert!(rsharp_core::run_rsharp("void main() { let x = 1; }").is_err());
    assert!(rsharp_core::check_rsharp("void main() { let x = 1; }").is_err());
    assert!(rsharp_core::run_rsharp("void main() { int x = \"bad\"; }").is_err());

    // Extra parser & codegen branch coverage
    let extra_rsh = r#"
        /// Doc
        public struct Holder<life a, const nuint N>
            where life a : static
        {
            public read<a> str Text;
        }

        public Holder<life static, 4> MakeH(read<static> str s)
        {
            return new Holder { Text = s };
        }

        public void ExtraControl(bool c, int? o)
        {
            if (c)
            {
                return;
            }
            for (int x = 0; ; x += 1)
            {
                _ = x;
                break;
            }
            for (mut int i = 0; i < 2; i++)
            {
                _ = i;
            }
            mut int k = 0;
            for (k = 0; k < 1; k++)
            {
                _ = k;
            }
            int x = match (o)
            {
                Some(int v) => v,
                None => 0,
            };
            _ = x;
            bool m = o is Some(1);
            bool nm = o is not Some(1);
            _ = (m, nm);
            read str lit = "x";
            _ = lit;
            int neg = match (-5)
            {
                -5 => 1,
                -2.0 => 2,
                'a' => 3,
                "s" => 4,
                true => 5,
                false => 6,
                _ => 0,
            };
            _ = neg;
        }
    "#;
    let extra_rs = transpile_to_rust(
        extra_rsh,
        TranspileOptions {
            prelude_mode: PreludeMode::None,
        },
    )
    .expect("extra parser/codegen branches should transpile");
    assert!(extra_rs.contains("Holder<'static, 4>"));
    assert!(extra_rs.contains("where 'a: 'static"));
}
