//! The salsa incremental database (WHITEPAPER §14).
//!
//! Every opened document is a salsa [`SourceFile`] input. Parsing,
//! megaprogram expansion, and type checking are tracked queries, so an edit
//! recomputes only the queries the edit invalidated and results for
//! unchanged inputs are reused from the memo tables.
//!
//! Anchoring rule: editor-visible diagnostics always refer to the text the
//! user sees. For files that mention megaprogram constructs (§8) the
//! compiler pipeline expands them to a virtual text first, and diagnostics
//! from that virtual text cannot be trusted to anchor against the original
//! buffer — so such files surface ONLY the expansion diagnostics, which the
//! expander anchors in the original file (plan §2). Pure-Checkmate files
//! surface the full parse + check pipeline. Position features are the other
//! way around: they analyze [`parse_original`], the ORIGINAL text, so
//! hover/definition/references/symbols/tokens anchor in the buffer for mega
//! files too (see `server::CheckmateLsp::with_analysis`).

use cme_compiler::diagnostics::Diagnostic;
use cme_compiler::schema::SchemaContext;
use cme_core::ast::Stmt;
use salsa::Database as Db;

/// What pipeline a document runs through. Detected once per edit by
/// [`sniff_kind`] and stored as part of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// A `.cm` script (the §2 language surface, possibly with §8
    /// megaprogramming).
    Script,
    /// A §9 schema file: its first significant token is `schema`.
    Schema,
}

/// Decides the pipeline for a document from its text. Schema files start
/// with the `schema` keyword (after comments and whitespace); everything
/// else is a script. The main lexer has no schema keywords — §9 files go
/// through a dedicated scanner — so the keyword arrives as an identifier.
pub fn sniff_kind(text: &str) -> FileKind {
    let (tokens, _) = cme_compiler::lexer::lex_with_errors(text);
    for token in &tokens {
        match token.token {
            // Newline tokens carry the line breaks that comments leave
            // behind, so they (and block comments) do not settle the kind.
            cme_compiler::lexer::Token::Newline | cme_compiler::lexer::Token::BlockComment => {
                continue;
            }
            cme_compiler::lexer::Token::Ident("schema") => return FileKind::Schema,
            _ => return FileKind::Script,
        }
    }
    FileKind::Script
}

/// An opened document: its text plus the pipeline it runs through.
#[salsa::input]
pub struct SourceFile {
    #[returns(deref)]
    pub text: String,
    #[returns(copy)]
    pub kind: FileKind,
}

/// The §8 megaprogram expansion of a document. For files that do not
/// mention megaprogram constructs this is the identity; for schema files
/// expansion never runs.
#[salsa::tracked]
pub struct Expansion<'db> {
    /// The expanded (pure Checkmate) text. Empty when expansion failed.
    #[tracked]
    #[returns(deref)]
    pub text: String,

    /// Expansion diagnostics, anchored in the ORIGINAL file text.
    #[tracked]
    #[returns(deref)]
    pub diagnostics: Vec<Diagnostic>,
}

/// The recovered AST of the analysis text plus its parse-stage diagnostics.
/// The analysis text is the expanded text for megaprogram files and the
/// original text otherwise; spans always refer to that analysis text.
#[salsa::tracked]
pub struct Parsed<'db> {
    #[tracked]
    #[returns(deref)]
    pub statements: Vec<Stmt>,

    #[tracked]
    #[returns(deref)]
    pub diagnostics: Vec<Diagnostic>,
}

/// Step 1: expand megaprograms (§8). Identity for plain scripts and schema
/// files.
#[salsa::tracked]
pub fn expansion(db: &dyn Db, file: SourceFile) -> Expansion<'_> {
    let text = file.text(db);
    if file.kind(db) != FileKind::Script || !cme_compiler::mega::expand::mentions_megaprogram(text)
    {
        return Expansion::new(db, text.to_string(), Vec::new());
    }
    match cme_compiler::mega::expand::expand_source(text) {
        Ok(outcome) => Expansion::new(db, outcome.expanded, Vec::new()),
        // Expansion failed: the expander's diagnostics anchor in the
        // original text, so they are editor-visible as-is.
        Err(diagnostics) => Expansion::new(db, String::new(), diagnostics),
    }
}

/// Step 2: the tolerant parse of the analysis text. Never fails: the parser
/// plants `Invalid` placeholders and keeps every diagnostic it can.
#[salsa::tracked(returns(copy))]
pub fn parse(db: &dyn Db, file: SourceFile) -> Parsed<'_> {
    let expansion = expansion(db, file);
    if !expansion.diagnostics(db).is_empty() {
        // A failed expansion leaves nothing to parse.
        return Parsed::new(db, Vec::new(), Vec::new());
    }
    let text = expansion.text(db);
    let outcome = cme_compiler::parse_source(text);
    let mut diagnostics = outcome.diagnostics;
    // Single-file builds reject self-rooted imports (§10.3), matching the
    // CLI's standalone gate.
    diagnostics.extend(cme_compiler::mods::standalone_import_diagnostics(
        &outcome.statements,
    ));
    Parsed::new(db, outcome.statements, diagnostics)
}

/// The tolerant parse of the ORIGINAL text — the user's own coordinates —
/// used by the position features for megaprogram files (§8). The expanded
/// pipeline above remains authoritative for diagnostics (its spans live in
/// expanded-text coordinates the editor never sees), but hover, definition,
/// references, symbols, and semantic tokens must anchor against the buffer,
/// so they analyze this tree instead. The mega constructs themselves parse
/// as recovery placeholders (the sub-language inside a region or pattern is
/// not Checkmate); the ordinary code around them parses exactly like a
/// plain script, and the analysis skips the placeholder shapes.
#[salsa::tracked(returns(copy))]
pub fn parse_original(db: &dyn Db, file: SourceFile) -> Parsed<'_> {
    let text = file.text(db);
    let outcome = cme_compiler::parse_source(text);
    Parsed::new(db, outcome.statements, outcome.diagnostics)
}

/// Step 3: type-check the recovered program (§2.6–§2.16, §11, §A.4–§A.7,
/// and §9 when a schema contract is active). Runs on the recovered AST so
/// partial results survive parse errors, just like the CLI's `check`.
/// The single-file pipeline serves loose sources; a document inside a mod
/// is checked through the mod assembly instead (see
/// [`crate::workspace::ModPlan`]).
///
/// This step is deliberately NOT a tracked query: a `SchemaContext` cannot
/// be a salsa argument (it is not internable), and the parse beneath it is
/// memoized, which is where the reuse matters.
pub fn check_diags(
    db: &dyn Db,
    file: SourceFile,
    schema: Option<&SchemaContext>,
) -> Vec<Diagnostic> {
    let parsed = parse(db, file);
    cme_compiler::check::check_with_schema(parsed.statements(db), schema)
}

use std::sync::Arc;

/// The editor-visible diagnostics for a document under the SINGLE-FILE
/// pipeline, anchored in the text the user sees (see the module docs for
/// the anchoring rule). `schema` is the §9 contract auto-detected for the
/// document's mod, when one exists.
pub fn diagnostics(
    db: &dyn Db,
    file: SourceFile,
    schema: Option<&SchemaContext>,
) -> Arc<Vec<Diagnostic>> {
    match file.kind(db) {
        FileKind::Schema => {
            let text = file.text(db);
            let outcome = cme_compiler::schema::parse_schema_file(text);
            Arc::new(outcome.diagnostics)
        }
        FileKind::Script => {
            let expansion = expansion(db, file);
            if !expansion.diagnostics(db).is_empty() {
                return Arc::new(expansion.diagnostics(db).to_vec());
            }
            let parsed = parse(db, file);
            let mut all = parsed.diagnostics(db).to_vec();
            all.extend(check_diags(db, file, schema));
            Arc::new(all)
        }
    }
}

/// The salsa database for the Checkmate language server. Handlers share one
/// database behind a mutex; every document operation is a short synchronous
/// query burst, so there is nothing to await while holding it.
#[salsa::db]
#[derive(Clone, Default)]
pub struct Database {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Database {}

#[cfg(test)]
mod tests {
    use super::*;
    use salsa::Setter;

    #[test]
    fn sniff_detects_schema_files() {
        assert_eq!(sniff_kind("schema engine 1.4.0\n"), FileKind::Schema);
        // Comments and blank lines precede the keyword.
        assert_eq!(
            sniff_kind("// engine contract\n\nschema engine 1.0.0\n"),
            FileKind::Schema
        );
        assert_eq!(sniff_kind("int hp = 100\n"), FileKind::Script);
        assert_eq!(sniff_kind(""), FileKind::Script);
    }

    #[test]
    fn script_diagnostics_flow_through_the_pipeline() {
        let mut db = Database::default();
        let file = SourceFile::new(
            &db,
            "int main() {\n    int hp = 100\n    return hp\n}\n".to_string(),
            FileKind::Script,
        );
        assert!(diagnostics(&db, file, None).is_empty());

        file.set_text(&mut db)
            .to("int main() {\n    int hp = tr\n    return hp\n}\n".to_string());
        let diags = diagnostics(&db, file, None);
        assert_eq!(diags.len(), 1, "one type error: bool into int");
    }

    #[test]
    fn schema_diagnostics_flow_through_their_pipeline() {
        let mut db = Database::default();
        let file = SourceFile::new(&db, "schema engine 1.4.0\n".to_string(), FileKind::Schema);
        assert!(diagnostics(&db, file, None).is_empty());
        file.set_text(&mut db).to("schema 1.4.0\n".to_string());
        assert!(!diagnostics(&db, file, None).is_empty());
    }

    #[test]
    fn expansion_diagnostics_anchor_for_megaprogram_files() {
        let db = Database::default();
        // An unterminated mega region fails expansion; a clean but broken
        // template body fails expansion too (template compile error).
        let broken = "mega twice(\n    $int value\n) {\n    ???\n}\n\ntwice! {\n    21\n}\n";
        let file = SourceFile::new(&db, broken.to_string(), FileKind::Script);
        let diags = diagnostics(&db, file, None);
        assert!(!diags.is_empty(), "expansion errors surface to the editor");
    }

    #[test]
    fn parse_exposes_recovered_statements_for_broken_code() {
        let db = Database::default();
        // The trailing valid declaration survives the broken one.
        let file = SourceFile::new(
            &db,
            "int = ???\nint hp = 100\n".to_string(),
            FileKind::Script,
        );
        let parsed = parse(&db, file);
        assert!(
            !parsed.diagnostics(&db).is_empty(),
            "parse reports the broken statement"
        );
        assert!(
            parsed
                .statements(&db)
                .iter()
                .any(|stmt| matches!(&stmt.kind, cme_core::ast::StmtKind::VarDecl { name, .. } if name == "hp")),
            "the healthy declaration is still in the tree"
        );
    }

    #[test]
    fn parse_original_keeps_user_coordinates_for_mega_files() {
        let db = Database::default();
        // A mega declaration, an invocation, and plain code around them.
        // The original text is NOT valid Checkmate at the mega sites, but
        // the surrounding functions must survive with their own spans.
        let source = "\
mega twice($int value) {
    int doubled = value * 2
    return doubled
}

int base() {
    return 21
}

int main() {
    return twice! {
        base()
    }
}
";
        let file = SourceFile::new(&db, source.to_string(), FileKind::Script);
        let parsed = parse_original(&db, file);
        let statements = parsed.statements(&db);
        let base = statements
            .iter()
            .find(|stmt| matches!(&stmt.kind, cme_core::ast::StmtKind::FuncDecl { name, .. } if name == "base"))
            .expect("the plain function between the mega sites is recovered");
        // `int base() {` starts at offset 71: the declaration's span must
        // anchor at the ORIGINAL text, not any expanded coordinate.
        assert_eq!(
            source[base.span.start..].starts_with("int base()"),
            true,
            "span must point into the user's text"
        );
        let main = statements
            .iter()
            .find(|stmt| matches!(&stmt.kind, cme_core::ast::StmtKind::FuncDecl { name, .. } if name == "main"))
            .expect("the function after the invocation is recovered");
        assert!(
            source[main.span.start..].starts_with("int main()"),
            "main's span must anchor in the original text"
        );
    }
}
