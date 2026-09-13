//! The conversion layer (byte spans ↔ LSP positions in UTF-16 units) and
//! the diagnostics pipeline (sniffing, parse/check surfacing, anchoring).

use salsa::Setter;
use tower_lsp_server::ls_types::Position;

// ---------------------------------------------------------------------------
// LineIndex
// ---------------------------------------------------------------------------

#[test]
fn position_offset_round_trip_ascii() {
    let text = "int main() {\n    return 0\n}\n";
    let index = cme_lsp::convert::LineIndex::new(text);
    for (line, column) in [(0u32, 2u32), (1, 4), (1, 12), (2, 1)] {
        let offset = index.offset(text, Position::new(line, column));
        let back = index.position(text, offset);
        assert_eq!(back, Position::new(line, column), "{line}:{column}");
    }
}

#[test]
fn positions_count_utf16_units() {
    let text = "str s = \"hé\"\nstr t = \"漢字\"\n";
    let index = cme_lsp::convert::LineIndex::new(text);
    // After 'é' (2 bytes in UTF-8, 1 UTF-16 unit): 10 ASCII + é.
    let after_e = text.find("é").unwrap() + 2;
    let pos = index.position(text, after_e);
    assert_eq!(pos, Position::new(0, 11));
    // After '漢' (3 bytes in UTF-8, 1 UTF-16 unit): 9 ASCII + 漢.
    let after_han = text.find("漢").unwrap() + 3;
    let pos = index.position(text, after_han);
    assert_eq!(pos, Position::new(1, 10));
}

#[test]
fn surrogate_pairs_count_as_two_units() {
    // U+1F600 (😀) is 4 bytes in UTF-8 and 2 UTF-16 units.
    let text = "str s = \"a😀b\"\nint x = 1\n";
    let index = cme_lsp::convert::LineIndex::new(text);
    let after_emoji = text.find('b').unwrap();
    let pos = index.position(text, after_emoji);
    // "str s = \"a" is 10 UTF-16 units, the emoji is 2 more.
    assert_eq!(pos, Position::new(0, 12));
    // The important half: offset(position(offset)) round-trips.
    let back = index.offset(text, pos);
    assert_eq!(back, after_emoji);
}

#[test]
fn crlf_lines_position_correctly() {
    let text = "int a = 1\r\nint b = 2\r\n";
    let index = cme_lsp::convert::LineIndex::new(text);
    // Line 1 starts after the \r\n.
    let line1_start = index.offset(text, Position::new(1, 0));
    assert_eq!(&text[line1_start..], "int b = 2\r\n");
    // A character position in the middle of line 1 skips the \r.
    let offset = index.offset(text, Position::new(1, 6));
    assert_eq!(&text[offset..offset + 1], "=");
}

#[test]
fn offsets_clamp_to_line_content_end() {
    let text = "int a = 1\nint b = 2\n";
    let index = cme_lsp::convert::LineIndex::new(text);
    // Far past the line length: clamps to the content end, not the next line.
    let offset = index.offset(text, Position::new(0, 50));
    assert_eq!(offset, 9);
    assert!(text[..offset].ends_with('1'));
    // One past the last character: same place.
    let offset = index.offset(text, Position::new(0, 10));
    assert_eq!(offset, 9);
}

#[test]
fn lines_past_eof_clamp_to_document_end() {
    let text = "int a = 1\n";
    let index = cme_lsp::convert::LineIndex::new(text);
    assert_eq!(index.offset(text, Position::new(50, 0)), text.len());
    assert_eq!(index.offset(text, Position::new(50, 99)), text.len());
}

#[test]
fn zero_width_spans_stay_zero_width() {
    let text = "int x = 1\n";
    let index = cme_lsp::convert::LineIndex::new(text);
    let range = index.range(text, cme_core::Span::missing(6));
    assert_eq!(range.start, range.end);
    assert_eq!(range.start, Position::new(0, 6));
}

#[test]
fn line_of_handles_every_offset() {
    let text = "ab\ncd\r\nefg\n";
    let index = cme_lsp::convert::LineIndex::new(text);
    assert_eq!(index.line_of(0), 0);
    assert_eq!(index.line_of(3), 1);
    assert_eq!(
        index.line_of(6),
        1,
        "the \\n of \\r\\n is still its own line"
    );
    assert_eq!(index.line_of(7), 2);
    assert_eq!(index.line_of(11), 3, "the empty last line");
    assert_eq!(index.line_of(999), 3, "clamped past EOF");
}

#[test]
fn diagnostics_carry_source_severity_and_range() {
    let src = "int hp = tr\n";
    let db = cme_lsp::db::Database::default();
    let file = cme_lsp::db::SourceFile::new(&db, src.to_string(), cme_lsp::db::FileKind::Script);
    let diagnostics = cme_lsp::features::diagnostics::publishable(&db, file);
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp_server::ls_types::DiagnosticSeverity::ERROR)
    );
    assert_eq!(diagnostic.source.as_deref(), Some("cme"));
    // The top-level statement error spans the whole statement.
    assert_eq!(diagnostic.range.start, Position::new(0, 0));
    assert_eq!(diagnostic.range.end, Position::new(0, 11));
    assert!(
        diagnostic.message.contains("top level"),
        "{}",
        diagnostic.message
    );
}

// ---------------------------------------------------------------------------
// Pipeline: sniffing, parse + check, megaprograms, mods
// ---------------------------------------------------------------------------

#[test]
fn sniffing_detects_schema_files() {
    use cme_lsp::db::{FileKind, sniff_kind};
    assert_eq!(sniff_kind("schema engine v1.4.0\n"), FileKind::Schema);
    assert_eq!(
        sniff_kind("// engine contract\n\nschema engine v1.0.0\n"),
        FileKind::Schema
    );
    assert_eq!(
        sniff_kind("/* header */\nschema engine v1.0.0\n"),
        FileKind::Schema
    );
    assert_eq!(sniff_kind("int hp = 100\n"), FileKind::Script);
    assert_eq!(
        sniff_kind("// schema is mentioned in a comment\nint x = 1\n"),
        FileKind::Script
    );
    assert_eq!(sniff_kind(""), FileKind::Script);
    assert_eq!(sniff_kind("int schema = 1\n"), FileKind::Script);
}

#[test]
fn script_diagnostics_surface_parse_and_check_errors() {
    let db = cme_lsp::db::Database::default();
    let file = cme_lsp::db::SourceFile::new(
        &db,
        "int = ???\nint hp = tr\n".to_string(),
        cme_lsp::db::FileKind::Script,
    );
    let diagnostics = cme_lsp::db::diagnostics(&db, file);
    assert!(
        diagnostics.len() >= 2,
        "a parse error AND a check error surface: {:?}",
        diagnostics.iter().map(|d| d.message()).collect::<Vec<_>>()
    );
}

#[test]
fn edits_recompute_diagnostics_incrementally() {
    let mut db = cme_lsp::db::Database::default();
    let file = cme_lsp::db::SourceFile::new(
        &db,
        "int main() {\n    int hp = 100\n    return hp\n}\n".to_string(),
        cme_lsp::db::FileKind::Script,
    );
    assert!(cme_lsp::db::diagnostics(&db, file).is_empty());
    file.set_text(&mut db)
        .to("int main() {\n    int hp = tr\n    return hp\n}\n".to_string());
    assert_eq!(cme_lsp::db::diagnostics(&db, file).len(), 1);
    file.set_text(&mut db)
        .to("int main() {\n    int hp = 100\n    return hp\n}\n".to_string());
    assert!(cme_lsp::db::diagnostics(&db, file).is_empty());
}

#[test]
fn standalone_self_imports_are_rejected_like_the_cli() {
    let db = cme_lsp::db::Database::default();
    let file = cme_lsp::db::SourceFile::new(
        &db,
        "import self.rules\n\nint main() {\n    return 0\n}\n".to_string(),
        cme_lsp::db::FileKind::Script,
    );
    let diagnostics = cme_lsp::db::diagnostics(&db, file);
    assert!(
        !diagnostics.is_empty(),
        "a single-file build rejects self-rooted imports (§10.3)"
    );
}

#[test]
fn broken_megaprograms_surface_expansion_diagnostics() {
    let db = cme_lsp::db::Database::default();
    let broken = "mega twice(\n    $int value\n) {\n    ???\n}\n\ntwice! {\n    21\n}\n";
    let file = cme_lsp::db::SourceFile::new(&db, broken.to_string(), cme_lsp::db::FileKind::Script);
    assert!(!cme_lsp::db::diagnostics(&db, file).is_empty());
}

#[test]
fn healthy_megaprograms_run_the_full_pipeline() {
    let db = cme_lsp::db::Database::default();
    let healthy = "int log(int x) {\n    return x\n}\n\nmega twice(\n    $int value\n) {\n    log($value)\n}\n\nint main() {\n    twice! {\n        21\n    }\n    return 0\n}\n";
    let file =
        cme_lsp::db::SourceFile::new(&db, healthy.to_string(), cme_lsp::db::FileKind::Script);
    let diagnostics = cme_lsp::db::diagnostics(&db, file);
    assert!(
        diagnostics.is_empty(),
        "{:?}",
        diagnostics.iter().map(|d| d.message()).collect::<Vec<_>>()
    );
}

#[test]
fn schema_files_check_through_the_schema_parser() {
    let mut db = cme_lsp::db::Database::default();
    let file = cme_lsp::db::SourceFile::new(
        &db,
        "schema engine v1.4.0\n".to_string(),
        cme_lsp::db::FileKind::Schema,
    );
    assert!(cme_lsp::db::diagnostics(&db, file).is_empty());
    file.set_text(&mut db).to("schema 1.4.0\n".to_string());
    assert!(!cme_lsp::db::diagnostics(&db, file).is_empty());
}

#[test]
fn recovered_statements_survive_parse_errors() {
    let db = cme_lsp::db::Database::default();
    let file = cme_lsp::db::SourceFile::new(
        &db,
        "int = ???\nint hp = 100\n".to_string(),
        cme_lsp::db::FileKind::Script,
    );
    let parsed = cme_lsp::db::parse(&db, file);
    assert!(!parsed.diagnostics(&db).is_empty());
    assert!(parsed.statements(&db).iter().any(
        |stmt| matches!(&stmt.kind, cme_core::ast::StmtKind::VarDecl { name, .. } if name == "hp")
    ));
}
