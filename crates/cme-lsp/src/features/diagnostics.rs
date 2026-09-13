//! Diagnostics push: the editor-visible diagnostics of a document,
//! converted to LSP form and anchored in the text the user sees (see the
//! anchoring rule in [`crate::db`]).

use salsa::Database as Db;
use tower_lsp_server::ls_types;

use crate::convert;
use crate::db::{self, SourceFile};

/// Computes the publishable diagnostics for a document.
pub fn publishable(db: &dyn Db, file: SourceFile) -> Vec<ls_types::Diagnostic> {
    let text = file.text(db);
    let index = convert::line_index(db, file);
    db::diagnostics(db, file)
        .iter()
        .map(|diagnostic| convert::diagnostic(&index, text, diagnostic))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, FileKind};

    #[test]
    fn diagnostics_carry_source_and_error_severity() {
        let db = Database::default();
        let file = SourceFile::new(&db, "int hp = tr\n".to_string(), FileKind::Script);
        let diagnostics = publishable(&db, file);
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = &diagnostics[0];
        assert_eq!(
            diagnostic.severity,
            Some(ls_types::DiagnosticSeverity::ERROR)
        );
        assert_eq!(diagnostic.source.as_deref(), Some("cme"));
        assert_eq!(diagnostic.range.start.line, 0);
    }
}
