//! Conversions between compiler data (byte-offset spans, diagnostics) and
//! LSP data (line/UTF-16-character positions, `lsp_types` diagnostics).
//!
//! LSP positions are line + UTF-16 code-unit offsets by default, so the
//! line index precomputes line starts once per document revision and each
//! conversion is a binary search plus a bounded UTF-16 walk.

use tower_lsp_server::ls_types;

use crate::db::SourceFile;
use salsa::Database as Db;

/// The line-start table of one document revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    /// Byte offset of every line start, including line 0's.
    line_starts: Vec<usize>,
    /// The document length in bytes; the end bound of the last line.
    len: usize,
}

impl LineIndex {
    /// Builds the index for `text`.
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(offset + 1);
            }
        }
        Self {
            line_starts,
            len: text.len(),
        }
    }

    /// The 0-based line containing `offset`. Offsets past the end clamp to
    /// the last line, mirroring how spans from recovery can touch EOF.
    pub fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(insertion) => insertion.saturating_sub(1).min(self.line_starts.len() - 1),
        }
    }

    /// The byte offset where `line` starts. Unknown lines clamp to the
    /// document end.
    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts.get(line).copied().unwrap_or(self.len)
    }

    /// Converts a byte offset into an LSP position (UTF-16 character
    /// counting). Offsets past the end clamp to the document end.
    pub fn position(&self, text: &str, offset: usize) -> ls_types::Position {
        let offset = offset.min(self.len);
        let line = self.line_of(offset);
        let line_start = self.line_start(line);
        let column = utf16_width(&text[line_start..offset]);
        ls_types::Position::new(line as u32, column as u32)
    }

    /// Converts a byte span into an zero-width-preserving LSP range.
    /// Recovery can produce zero-width spans (missing nodes); those stay
    /// zero-width so clients place the cursor at the gap.
    pub fn range(&self, text: &str, span: cme_core::Span) -> ls_types::Range {
        ls_types::Range::new(
            self.position(text, span.start),
            self.position(text, span.end),
        )
    }

    /// The inverse of [`LineIndex::position`]: an LSP position (line +
    /// UTF-16 character) back into a byte offset. Characters past the line
    /// length clamp to the line's content end (before the terminator, per
    /// the LSP spec); lines past EOF clamp to the document end.
    pub fn offset(&self, text: &str, position: ls_types::Position) -> usize {
        let line = position.line as usize;
        let line_start = self.line_start(line);
        let mut line_end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.len)
            .min(self.len);
        // The content end sits before the line terminator.
        if line_end > line_start {
            let content = &text[line_start..line_end];
            let content = content.strip_suffix('\n').unwrap_or(content);
            let content = content.strip_suffix('\r').unwrap_or(content);
            line_end = line_start + content.len();
        }
        let target = position.character as usize;
        let mut units = 0usize;
        for (byte_index, ch) in text[line_start.min(self.len)..line_end].char_indices() {
            if units >= target {
                return line_start + byte_index;
            }
            units += ch.len_utf16();
        }
        line_end
    }
}

/// The UTF-16 code-unit width of `text`.
fn utf16_width(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Converts a compiler diagnostic into an LSP diagnostic. Everything the
/// front end reports is an error; the stage is recorded as the source so
/// clients can group them.
pub fn diagnostic(
    line_index: &LineIndex,
    text: &str,
    diagnostic: &cme_compiler::diagnostics::Diagnostic,
) -> ls_types::Diagnostic {
    ls_types::Diagnostic {
        range: line_index.range(text, diagnostic.span()),
        severity: Some(ls_types::DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("cme".to_string()),
        message: diagnostic.message().to_string(),
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Reads the tracked [`LineIndex`] for a document.
pub fn line_index(db: &dyn Db, file: SourceFile) -> LineIndex {
    LineIndex::new(file.text(db))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_cover_crlf_and_multibyte() {
        let index = LineIndex::new("ab\ncd\r\nefg\n");
        assert_eq!(index.line_of(0), 0);
        assert_eq!(index.line_of(3), 1);
        assert_eq!(
            index.line_of(6),
            1,
            "the \\n of \\r\\n is still its own line"
        );
        assert_eq!(index.line_of(7), 2, "offset of 'e' starts line 2");
        assert_eq!(index.line_of(10), 2);
        assert_eq!(index.line_of(11), 3, "the empty last line");
        assert_eq!(index.line_of(999), 3, "clamped past EOF");
    }

    #[test]
    fn positions_count_utf16_units() {
        let text = "str s = \"hé\"\nstr t = \"漢字\"\n";
        let index = LineIndex::new(text);
        // After 'é' (2 bytes in UTF-8, 1 UTF-16 unit): 10 ASCII + é.
        let after_e = text.find("é").unwrap() + 2;
        let pos = index.position(text, after_e);
        assert_eq!(pos, ls_types::Position::new(0, 11));
        // After '漢' (3 bytes in UTF-8, 1 UTF-16 unit): 9 ASCII + 漢.
        let after_han = text.find("漢").unwrap() + 3;
        let pos = index.position(text, after_han);
        assert_eq!(pos, ls_types::Position::new(1, 10));
    }

    #[test]
    fn zero_width_spans_stay_zero_width() {
        let index = LineIndex::new("int x = 1\n");
        let range = index.range("int x = 1\n", cme_core::Span::missing(6));
        assert_eq!(range.start, range.end);
        assert_eq!(range.start, ls_types::Position::new(0, 6));
    }

    #[test]
    fn positions_clamp_to_line_content_end() {
        let text = "int a = 1\nint b = 2\n";
        let index = LineIndex::new(text);
        // Far past the line length: clamps to the content end (before the
        // terminator), not to the next line.
        let offset = index.offset(text, ls_types::Position::new(0, 50));
        assert_eq!(offset, 9);
        assert!(text[..offset].ends_with('1'));
        // Exactly one past the last character: same place.
        let offset = index.offset(text, ls_types::Position::new(0, 10));
        assert_eq!(offset, 9);
        // Line 1 is untouched.
        let offset = index.offset(text, ls_types::Position::new(1, 50));
        assert_eq!(offset, 19);
    }
}
