//! Smoke tests for the LSP helpers. Feature-gated — only run when the
//! `lsp` feature is on, since otherwise the `gulf::lsp` module isn't even
//! compiled. Run with `cargo test --features lsp`.

#![cfg(feature = "lsp")]

use gulf::lsp::{
    byte_offset_at, collect_symbols, compute_diagnostics, find_definition, hover_at, lookup_docs,
    position_at, word_at,
};
use tower_lsp::lsp_types::{HoverContents, Position, SymbolKind};

#[test]
fn position_byte_offset_roundtrip_ascii() {
    let text = "abc\ndef\nghi";
    let cases: &[(usize, u32, u32)] = &[
        (0, 0, 0),
        (3, 0, 3),
        (4, 1, 0),
        (7, 1, 3),
        (8, 2, 0),
        (11, 2, 3),
    ];
    for &(offset, line, character) in cases {
        let pos = position_at(text, offset);
        assert_eq!(pos, Position { line, character }, "offset {offset}");
        let back = byte_offset_at(text, pos);
        assert_eq!(back, offset, "round-trip for offset {offset}");
    }
}

#[test]
fn position_at_handles_utf16_surrogate_pairs() {
    // Each "💧" is one char, four UTF-8 bytes, two UTF-16 code units.
    let text = "💧💧";
    assert_eq!(
        position_at(text, 4),
        Position {
            line: 0,
            character: 2
        }
    );
    assert_eq!(
        position_at(text, 8),
        Position {
            line: 0,
            character: 4
        }
    );
    // Going back the other way:
    assert_eq!(
        byte_offset_at(
            text,
            Position {
                line: 0,
                character: 2
            }
        ),
        4
    );
}

#[test]
fn word_at_finds_the_identifier_under_the_cursor() {
    let text = "const const greeting = 1!";
    // Cursor on 'g' of "greeting".
    let (word, span) = word_at(text, 13).unwrap();
    assert_eq!(word, "greeting");
    assert_eq!(span.start, 12);
    assert_eq!(span.end, 20);
}

#[test]
fn word_at_returns_none_on_whitespace() {
    let text = "let   x";
    assert!(word_at(text, 4).is_none());
}

#[test]
fn lookup_docs_covers_keywords_and_http() {
    assert!(lookup_docs("function").is_some());
    assert!(lookup_docs("when").is_some());
    assert!(lookup_docs("import").is_some());
    assert!(lookup_docs("http").is_some());
    assert!(lookup_docs("nonsense_token_zzz").is_none());
}

#[test]
fn hover_renders_markdown_for_known_word() {
    let text = "import http!";
    // Position over "http".
    let h = hover_at(
        text,
        Position {
            line: 0,
            character: 8,
        },
    )
    .expect("expected hover");
    let HoverContents::Markup(content) = h.contents else {
        panic!("expected markup");
    };
    assert!(content.value.contains("http"), "value: {}", content.value);
}

#[test]
fn diagnostics_empty_for_well_formed_program() {
    let diags = compute_diagnostics("print(1 + 2)!");
    assert!(diags.is_empty(), "diags: {diags:?}");
}

#[test]
fn diagnostics_reports_parse_error_with_range() {
    // Missing closing quote — the lexer should complain.
    let diags = compute_diagnostics(r#"print("oops)!"#);
    assert_eq!(diags.len(), 1, "diags: {diags:?}");
    let d = &diags[0];
    assert_eq!(d.severity, Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR));
    assert_eq!(d.source.as_deref(), Some("gulf"));
    // The range must be inside the document.
    assert_eq!(d.range.start.line, 0);
}

#[test]
fn document_symbols_extracts_functions_and_classes() {
    let src = r#"
function greet(who) => print(who)!

class Box {
   const const name = "boxy"!
   function unbox() => name!
}

const const top = 1!
"#;
    let symbols = collect_symbols(src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"greet"), "names: {names:?}");
    assert!(names.contains(&"Box"), "names: {names:?}");
    assert!(names.contains(&"top"), "names: {names:?}");

    let kinds: Vec<SymbolKind> = symbols.iter().map(|s| s.kind).collect();
    assert!(kinds.contains(&SymbolKind::FUNCTION));
    assert!(kinds.contains(&SymbolKind::CLASS));
    assert!(kinds.contains(&SymbolKind::VARIABLE));

    // The class symbol should have its members nested as children.
    let class = symbols.iter().find(|s| s.name == "Box").unwrap();
    let children = class.children.as_ref().expect("class has children");
    let child_names: Vec<&str> = children.iter().map(|s| s.name.as_str()).collect();
    assert!(child_names.contains(&"name"), "child names: {child_names:?}");
    assert!(child_names.contains(&"unbox"), "child names: {child_names:?}");
}

#[test]
fn find_definition_locates_top_level_binding() {
    let src = r#"
const const greeting = "hi"!
print(greeting)!
"#;
    let span = find_definition(src, "greeting").expect("expected to find decl");
    let snippet = &src[span.start..span.end];
    assert!(snippet.contains("greeting"), "snippet: {snippet:?}");
    // Sanity: the definition must come before the use site.
    let use_site = src.find("print").unwrap();
    assert!(span.start < use_site, "decl at {} vs use at {use_site}", span.start);
}

#[test]
fn find_definition_returns_none_for_unknown_name() {
    let src = "const const x = 1!";
    assert!(find_definition(src, "nope").is_none());
}
