//! Diagnostic rendering shared by compile and execution errors: byte spans
//! to 1-based, CHARACTER-based line/column positions (columns count
//! characters, so positions stay meaningful on multibyte UTF-8 lines), and
//! `path:line:column: message` text. Ported from the CLI's reporter so
//! embedded hosts render exactly what `cme check` prints, minus the carets.

use cme_core::Span;

/// A one-shot index of a source's line starts. Building it once turns a
/// whole-diagnostics render from O(errors × file) byte scans into
/// O(errors) — recovery-heavy files produce thousands of diagnostics, and
/// per-diagnostic rescans made reporting the dominant cost.
pub(crate) struct SourceLayout {
    line_starts: Vec<usize>,
}

impl SourceLayout {
    pub(crate) fn new(source: &str) -> Self {
        let mut line_starts = vec![0usize];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        Self { line_starts }
    }

    /// The 1-based (line, column) of `offset`, columns counted in
    /// characters. Offsets past the end clamp to the last position.
    pub(crate) fn line_column(&self, source: &str, offset: usize) -> (usize, usize) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(index) => index + 1,
            Err(index) => index,
        }
        .max(1);
        let line_start = self
            .line_starts
            .get(line - 1)
            .copied()
            .unwrap_or_else(|| *self.line_starts.last().unwrap_or(&0));
        let column_chars = source
            .get(line_start..offset.min(source.len()))
            .map_or(0, |text| text.chars().count());
        (line, column_chars + 1)
    }
}

/// Renders `message` as `path:line:column: message` (path omitted when
/// there is none — plain `line:column: message`), positions computed
/// against `source`.
pub(crate) fn render_located(
    message: &str,
    span: Span,
    source: &str,
    path: Option<&str>,
    layout: &SourceLayout,
) -> String {
    let (line, column) = layout.line_column(source, span.start);
    match path {
        Some(path) => format!("{path}:{line}:{column}: {message}"),
        None => format!("{line}:{column}: {message}"),
    }
}
