//! Expansion templates (WHITEPAPER §8.4): parsing template text into a
//! `cme_core::magic::Template` and elaborating it with a capture tree into
//! generated Checkmate SOURCE TEXT.
//!
//! Template constructs: `$cap` / `$cap.field` splices (with `.matched` /
//! `.length` accessors), `$"…{cap}…"` interpolation, `[each [NAME] in xs
//! { … }]` (implicit element `item`; the element's fields resolve bare),
//! `[when cond { … } else { … }]`, `match ($cap) { label => … }` (exhaustive
//! over `oneof` tags), `let name = $cap`, `require(cond, "message")`, and
//! `@fn(…)` compile-time calls and the `cm.…( … )` builtin namespace (§8.5,
//! evaluated by [`super::cteval`]). The bare `each in xs { … }` statement
//! form is accepted inside blocks.
//!
//! Join rule (plan §1.4.7): `[each]` elements are joined with `", "` when
//! the construct sits inside template `(`/`[` text (argument, parameter, or
//! array-literal position) and with `"\n"` otherwise (statement lists, map
//! entries). Only TEXT brackets drive the decision — construct braces do
//! not.

use crate::diagnostics::Diagnostic;
use cme_core::Span;
use cme_core::magic::{
    Accessor, Capture, CaptureKind, CtxBinOp, CtxExpr, Template, TextKind, TmplNode, TmplStrPart,
    TmplValue,
};

use super::cteval::{self, CtEngine};
use super::ctxexpr;
use super::profile::{parse_string_literal, skip_ws_and_comments};
use cme_interp::Value;

/// Parses a whole template (the text inside a magic declaration's braces).
pub fn parse_template(text: &str, span: Span) -> Result<Template, Diagnostic> {
    let mut parser = TmplParser {
        text,
        span,
        cursor: 0,
    };
    let (nodes, end) = parser.nodes_until(&[], Mode::Top)?;
    match end {
        EndKind::Eof => Ok(Template { nodes }),
        _ => Err(parser.error("unbalanced brackets in template")),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Top,
    Brace,
    MatchBody,
}

enum EndKind {
    Eof,
    Brace,
    Bracket,
    Paren,
    Arm,
}

struct TmplParser<'a> {
    text: &'a str,
    span: Span,
    cursor: usize,
}

impl<'a> TmplParser<'a> {
    fn rest(&self) -> &'a str {
        &self.text[self.cursor..]
    }

    fn error(&self, message: impl Into<String>) -> Diagnostic {
        let start = (self.span.start + self.cursor).min(self.span.end);
        Diagnostic::parse(message, Span::new(start, start))
    }

    fn skip_trivia(&mut self) {
        skip_ws_and_comments(
            self.text,
            &mut self.cursor,
            &crate::mega::profile::checkmate_scan_profile(),
        );
    }

    fn peek_word(&self) -> &'a str {
        let rest = self.rest();
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        &rest[..end]
    }

    /// True when `word` starts at the cursor as a whole word.
    fn at_word(&self, word: &str) -> bool {
        self.rest().starts_with(word)
            && self.rest()[word.len()..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
    }

    fn node_span(&self, start: usize) -> Span {
        let from = (self.span.start + start).min(self.span.end);
        let to = (self.span.start + self.cursor).min(self.span.end);
        Span::new(from, to.max(from))
    }

    /// Parses nodes until a terminator (mode-dependent) or EOF. Returns the
    /// nodes and how the parse ended; terminator characters are consumed.
    fn nodes_until(
        &mut self,
        terminators: &[char],
        mode: Mode,
    ) -> Result<(Vec<TmplNode>, EndKind), Diagnostic> {
        let mut nodes: Vec<TmplNode> = Vec::new();
        let mut depth = 0i32;
        let mut text_start = self.cursor;

        loop {
            if self.cursor >= self.text.len() {
                self.flush_text(text_start, &mut nodes);
                return match mode {
                    Mode::Top => Ok((nodes, EndKind::Eof)),
                    _ => Err(self.error("unterminated template construct")),
                };
            }

            let c = self.text[self.cursor..].chars().next().unwrap();

            // Terminator characters at depth 0 end the parse.
            if depth == 0 && terminators.contains(&c) {
                self.flush_text(text_start, &mut nodes);
                self.cursor += c.len_utf8();
                let kind = match c {
                    '}' => EndKind::Brace,
                    ']' => EndKind::Bracket,
                    _ => EndKind::Paren,
                };
                return Ok((nodes, kind));
            }

            // Match-arm boundary: `IDENT =>` at depth 0.
            if mode == Mode::MatchBody
                && depth == 0
                && let Some(_label) = self.arm_boundary()
            {
                self.flush_text(text_start, &mut nodes);
                return Ok((nodes, EndKind::Arm));
            }

            // Construct starts.
            if c == '$' {
                let next = self.text[self.cursor + 1..].chars().next();
                match next {
                    Some('"') => {
                        self.flush_text(text_start, &mut nodes);
                        nodes.push(self.interp()?);
                        text_start = self.cursor;
                        continue;
                    }
                    Some(n) if n.is_ascii_alphabetic() || n == '_' => {
                        self.flush_text(text_start, &mut nodes);
                        nodes.push(self.splice()?);
                        text_start = self.cursor;
                        continue;
                    }
                    _ => {}
                }
            }
            if c == '[' && self.at_word_after_bracket("each") {
                self.flush_text(text_start, &mut nodes);
                nodes.push(self.each_bracketed()?);
                text_start = self.cursor;
                continue;
            }
            if c == '[' && self.at_word_after_bracket("when") {
                self.flush_text(text_start, &mut nodes);
                nodes.push(self.when()?);
                text_start = self.cursor;
                continue;
            }
            if self.at_word("each") && self.word_ahead_is("each", "in") {
                self.flush_text(text_start, &mut nodes);
                nodes.push(self.each_bare()?);
                text_start = self.cursor;
                continue;
            }
            if self.at_word("match") && self.word_ahead_opens_capture() {
                self.flush_text(text_start, &mut nodes);
                nodes.push(self.match_construct()?);
                text_start = self.cursor;
                continue;
            }
            if self.at_word("let") && self.word_ahead_is_binding() {
                self.flush_text(text_start, &mut nodes);
                nodes.push(self.let_binding()?);
                text_start = self.cursor;
                continue;
            }
            if self.at_word("require") && self.word_ahead_opens_paren() {
                self.flush_text(text_start, &mut nodes);
                nodes.push(self.require()?);
                text_start = self.cursor;
                continue;
            }
            if c == '@' {
                self.flush_text(text_start, &mut nodes);
                nodes.push(self.call()?);
                text_start = self.cursor;
                continue;
            }
            if self.cm_call_ahead() {
                self.flush_text(text_start, &mut nodes);
                nodes.push(self.call_node(false)?);
                text_start = self.cursor;
                continue;
            }

            // Plain template text: track bracket depth, honoring strings.
            match c {
                '"' => {
                    if let Some((_, len)) = parse_string_literal(self.text, self.cursor) {
                        self.cursor += len;
                    } else {
                        self.cursor += 1;
                    }
                }
                '{' | '[' | '(' => {
                    depth += 1;
                    self.cursor += 1;
                }
                '}' | ']' | ')' => {
                    if depth == 0 {
                        // A stray closer at depth 0 outside any construct:
                        // for MatchBody this ends the arm list's parent, for
                        // other modes it is an error the caller reports.
                        self.flush_text(text_start, &mut nodes);
                        self.cursor += c.len_utf8();
                        let kind = match c {
                            '}' => EndKind::Brace,
                            ']' => EndKind::Bracket,
                            _ => EndKind::Paren,
                        };
                        return Ok((nodes, kind));
                    }
                    depth -= 1;
                    self.cursor += 1;
                }
                _ => self.cursor += c.len_utf8(),
            }
        }
    }

    /// Emits pending literal text as a `Text` node.
    fn flush_text(&mut self, text_start: usize, nodes: &mut Vec<TmplNode>) {
        if self.cursor > text_start {
            let text = self.text[text_start..self.cursor].to_string();
            if !text.is_empty() {
                nodes.push(TmplNode::Text {
                    text,
                    span: self.node_span(text_start),
                });
            }
        }
    }

    fn arm_boundary(&self) -> Option<String> {
        let word = self.peek_word();
        if word.is_empty() || !word.chars().next().unwrap().is_ascii_alphabetic() {
            return None;
        }
        let mut cursor = self.cursor + word.len();
        skip_ws_and_comments(
            self.text,
            &mut cursor,
            &crate::mega::profile::checkmate_scan_profile(),
        );
        if self.text[cursor..].starts_with("=>") {
            Some(word.to_string())
        } else {
            None
        }
    }

    fn at_word_after_bracket(&self, word: &str) -> bool {
        self.rest()[1..].starts_with(word)
            && self.rest()[1 + word.len()..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
    }

    fn word_ahead_is(&self, word: &str, next: &str) -> bool {
        let mut cursor = self.cursor + word.len();
        skip_ws_and_comments(
            self.text,
            &mut cursor,
            &crate::mega::profile::checkmate_scan_profile(),
        );
        self.text[cursor..].starts_with(next)
    }

    fn word_ahead_opens_capture(&self) -> bool {
        let mut cursor = self.cursor + "match".len();
        skip_ws_and_comments(
            self.text,
            &mut cursor,
            &crate::mega::profile::checkmate_scan_profile(),
        );
        if !self.text[cursor..].starts_with('(') {
            return false;
        }
        cursor += 1;
        skip_ws_and_comments(
            self.text,
            &mut cursor,
            &crate::mega::profile::checkmate_scan_profile(),
        );
        self.text[cursor..].starts_with('$')
    }

    fn word_ahead_opens_paren(&self) -> bool {
        let mut cursor = self.cursor + "require".len();
        skip_ws_and_comments(
            self.text,
            &mut cursor,
            &crate::mega::profile::checkmate_scan_profile(),
        );
        self.text[cursor..].starts_with('(')
    }

    fn word_ahead_is_binding(&self) -> bool {
        let mut cursor = self.cursor + "let".len();
        skip_ws_and_comments(
            self.text,
            &mut cursor,
            &crate::mega::profile::checkmate_scan_profile(),
        );
        let rest = &self.text[cursor..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end == 0 {
            return false;
        }
        let mut after = cursor + end;
        skip_ws_and_comments(
            self.text,
            &mut after,
            &crate::mega::profile::checkmate_scan_profile(),
        );
        self.text[after..].starts_with('=')
    }

    /// `$cap.path[.accessor]`
    fn splice(&mut self) -> Result<TmplNode, Diagnostic> {
        let start = self.cursor;
        self.cursor += 1; // `$`
        let path = self.dotted_path()?;
        let accessor = self.trailing_accessor()?;
        Ok(TmplNode::Splice {
            path,
            accessor,
            span: self.node_span(start),
        })
    }

    fn dotted_path(&mut self) -> Result<Vec<String>, Diagnostic> {
        let mut segments = Vec::new();
        loop {
            self.skip_trivia();
            // The `$` of `$cap.path` / `{$cap.path}` is optional; hole and
            // each-list positions accept both spellings (plan §1.4.5).
            if self.rest().starts_with('$') && segments.is_empty() {
                self.cursor += 1;
                continue;
            }
            let word = self.peek_word();
            if word.is_empty() {
                break;
            }
            segments.push(word.to_string());
            self.cursor += word.len();
            if self.rest().starts_with('.') {
                self.cursor += 1;
                continue;
            }
            break;
        }
        if segments.is_empty() {
            return Err(self.error("expected a capture path after `$`"));
        }
        Ok(segments)
    }

    /// `.matched` / `.length` after a path.
    fn trailing_accessor(&mut self) -> Result<Option<Accessor>, Diagnostic> {
        if !self.rest().starts_with('.') {
            return Ok(None);
        }
        self.cursor += 1;
        let word = self.peek_word();
        let accessor = match word {
            "matched" => Accessor::Matched,
            "length" => Accessor::Length,
            "line" => Accessor::Line,
            "col" => Accessor::Col,
            _ => {
                self.cursor -= 1;
                return Ok(None);
            }
        };
        self.cursor += word.len();
        Ok(Some(accessor))
    }

    /// `$"…{cap}…"`
    fn interp(&mut self) -> Result<TmplNode, Diagnostic> {
        let start = self.cursor;
        self.cursor += 2; // `$"`
        let mut parts = Vec::new();
        let mut literal = String::new();
        loop {
            match self.rest().chars().next() {
                None => return Err(self.error("unterminated template string")),
                Some('"') => {
                    self.cursor += 1;
                    break;
                }
                Some('\\') => {
                    let escaped = self.rest()[1..].chars().next().unwrap_or('\\');
                    literal.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        other => other,
                    });
                    self.cursor += 1 + escaped.len_utf8();
                }
                Some('{') => {
                    // `{ path }` hole; `{{` stays literal.
                    if self.rest()[1..].starts_with('{') {
                        literal.push('{');
                        self.cursor += 2;
                        continue;
                    }
                    if !literal.is_empty() {
                        parts.push(TmplStrPart::Lit(std::mem::take(&mut literal)));
                    }
                    self.cursor += 1;
                    let path = self.dotted_path()?;
                    let accessor = self.trailing_accessor()?;
                    self.skip_trivia();
                    if !self.rest().starts_with('}') {
                        return Err(self.error("expected `}` to close the hole"));
                    }
                    self.cursor += 1;
                    parts.push(TmplStrPart::Hole { path, accessor });
                }
                Some(c) => {
                    literal.push(c);
                    self.cursor += c.len_utf8();
                }
            }
        }
        if !literal.is_empty() {
            parts.push(TmplStrPart::Lit(literal));
        }
        Ok(TmplNode::Interp {
            parts,
            span: self.node_span(start),
        })
    }

    /// `[each [NAME] in [$]path { body }]`
    fn each_bracketed(&mut self) -> Result<TmplNode, Diagnostic> {
        let start = self.cursor;
        self.cursor += 1; // `[`
        self.cursor += "each".len();
        let (element, list) = self.each_header()?;
        let body = self.braced_nodes()?;
        self.skip_trivia();
        if !self.rest().starts_with(']') {
            return Err(self.error("expected `]` to close the `[each …]` construct"));
        }
        self.cursor += 1;
        Ok(TmplNode::Each {
            element,
            list,
            body: Box::new(Template { nodes: body }),
            span: self.node_span(start),
        })
    }

    /// `each [NAME] in [$]path { body }` (statement form)
    fn each_bare(&mut self) -> Result<TmplNode, Diagnostic> {
        let start = self.cursor;
        self.cursor += "each".len();
        let (element, list) = self.each_header()?;
        let body = self.braced_nodes()?;
        Ok(TmplNode::Each {
            element,
            list,
            body: Box::new(Template { nodes: body }),
            span: self.node_span(start),
        })
    }

    fn each_header(&mut self) -> Result<(String, TmplValue), Diagnostic> {
        self.skip_trivia();
        // Optional explicit element name (must not be `in`).
        let word = self.peek_word();
        let element = if !word.is_empty() && word != "in" {
            self.cursor += word.len();
            word.to_string()
        } else {
            "item".to_string()
        };
        self.skip_trivia();
        if !self.rest().starts_with("in") {
            return Err(self.error("expected `in` in an `each` construct"));
        }
        self.cursor += 2;
        self.skip_trivia();
        if self.rest().starts_with('$') {
            self.cursor += 1;
        }
        let path = self.dotted_path()?;
        Ok((element, TmplValue::Capture { path }))
    }

    fn braced_nodes(&mut self) -> Result<Vec<TmplNode>, Diagnostic> {
        self.skip_trivia();
        if !self.rest().starts_with('{') {
            return Err(self.error("expected `{` to open the construct body"));
        }
        self.cursor += 1;
        let (nodes, end) = self.nodes_until(&['}'], Mode::Brace)?;
        if !matches!(end, EndKind::Brace) {
            return Err(self.error("unterminated construct body"));
        }
        Ok(nodes)
    }

    /// `[when cond { then } else { otherwise }]`
    fn when(&mut self) -> Result<TmplNode, Diagnostic> {
        let start = self.cursor;
        self.cursor += 1; // `[`
        self.cursor += "when".len();
        let cond = ctxexpr::parse_expr(self.text, &mut self.cursor, self.span)?;
        let then = self.braced_nodes()?;
        self.skip_trivia();
        let otherwise = if self.at_word("else") {
            self.cursor += 4;
            self.braced_nodes()?
        } else {
            Vec::new()
        };
        self.skip_trivia();
        if !self.rest().starts_with(']') {
            return Err(self.error("expected `]` to close the `[when …]` construct"));
        }
        self.cursor += 1;
        Ok(TmplNode::When {
            cond,
            then: Box::new(Template { nodes: then }),
            otherwise: Box::new(Template { nodes: otherwise }),
            span: self.node_span(start),
        })
    }

    /// `match ($cap) { label => body, … }` — exhaustive over tags.
    fn match_construct(&mut self) -> Result<TmplNode, Diagnostic> {
        let start = self.cursor;
        self.cursor += "match".len();
        self.skip_trivia();
        self.cursor += 1; // `(`
        self.skip_trivia();
        if !self.rest().starts_with('$') {
            return Err(self.error("expected a capture after `match (`"));
        }
        self.cursor += 1;
        let path = self.dotted_path()?;
        let accessor = self.trailing_accessor()?;
        if accessor.is_some() {
            return Err(self.error("match scrutinee takes a plain capture"));
        }
        self.skip_trivia();
        if !self.rest().starts_with(')') {
            return Err(self.error("expected `)` after the match scrutinee"));
        }
        self.cursor += 1;
        self.skip_trivia();
        if !self.rest().starts_with('{') {
            return Err(self.error("expected `{` to open the match arms"));
        }
        self.cursor += 1;
        let mut arms = Vec::new();
        loop {
            self.skip_trivia();
            if self.rest().starts_with('}') {
                self.cursor += 1;
                break;
            }
            let label = self.peek_word().to_string();
            if label.is_empty() {
                return Err(self.error("expected an arm label"));
            }
            self.cursor += label.len();
            self.skip_trivia();
            if !self.rest().starts_with("=>") {
                return Err(self.error(format!("expected `=>` after arm `{label}`")));
            }
            self.cursor += 2;
            let (body, end) = self.nodes_until(&['}'], Mode::MatchBody)?;
            let done = matches!(end, EndKind::Brace);
            arms.push((label, Template { nodes: body }));
            if done {
                break;
            }
        }
        Ok(TmplNode::Match {
            scrutinee: path,
            arms,
            span: self.node_span(start),
        })
    }

    /// `let name = $cap`
    fn let_binding(&mut self) -> Result<TmplNode, Diagnostic> {
        let start = self.cursor;
        self.cursor += "let".len();
        self.skip_trivia();
        let name = self.peek_word().to_string();
        if name.is_empty() {
            return Err(self.error("expected a name after `let`"));
        }
        self.cursor += name.len();
        self.skip_trivia();
        if !self.rest().starts_with('=') {
            return Err(self.error("expected `=` in a `let` binding"));
        }
        self.cursor += 1;
        // The right-hand side is a full template value: a capture path, a
        // literal, or a compile-time call (§8.5 — `let x = @f(…)` /
        // `let x = cm.parse(…)`).
        let value = self.value()?;
        Ok(TmplNode::Let {
            name,
            value,
            span: self.node_span(start),
        })
    }

    /// `require(cond, "message")`
    fn require(&mut self) -> Result<TmplNode, Diagnostic> {
        let start = self.cursor;
        self.cursor += "require".len();
        self.skip_trivia();
        self.cursor += 1; // `(`
        let cond = ctxexpr::parse_expr(self.text, &mut self.cursor, self.span)?;
        self.skip_trivia();
        if !self.rest().starts_with(',') {
            return Err(self.error("expected `,` before the require message"));
        }
        self.cursor += 1;
        self.skip_trivia();
        let (message, len) = parse_string_literal(self.text, self.cursor)
            .ok_or_else(|| self.error("expected a message string in `require`"))?;
        self.cursor += len;
        self.skip_trivia();
        if !self.rest().starts_with(')') {
            return Err(self.error("expected `)` to close `require`"));
        }
        self.cursor += 1;
        Ok(TmplNode::Require {
            cond,
            message,
            span: self.node_span(start),
        })
    }

    /// `@fn(args)` — a compile-time function call (§8.5).
    fn call(&mut self) -> Result<TmplNode, Diagnostic> {
        self.call_node(true)
    }

    /// True when a `cm.…( … )` builtin call starts at the cursor (the
    /// whitepaper's builtin namespace is spelled without `@`, §8.5).
    fn cm_call_ahead(&self) -> bool {
        if !self.at_word("cm") {
            return false;
        }
        let mut cursor = self.cursor + "cm".len();
        if !self.text[cursor..].starts_with('.') {
            return false;
        }
        cursor += 1;
        loop {
            let word = self.word_len_at(cursor);
            if word == 0 {
                return false;
            }
            cursor += word;
            if self.text[cursor..].starts_with('.') {
                cursor += 1;
                continue;
            }
            break;
        }
        skip_ws_and_comments(
            self.text,
            &mut cursor,
            &crate::mega::profile::checkmate_scan_profile(),
        );
        self.text[cursor..].starts_with('(')
    }

    fn word_len_at(&self, cursor: usize) -> usize {
        self.text[cursor..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .map(char::len_utf8)
            .sum()
    }

    fn call_node(&mut self, at: bool) -> Result<TmplNode, Diagnostic> {
        let start = self.cursor;
        if at {
            self.cursor += 1; // `@`
        }
        let mut path = vec![self.peek_word().to_string()];
        self.cursor += path[0].len();
        loop {
            if self.rest().starts_with('.') {
                self.cursor += 1;
                let segment = self.peek_word().to_string();
                self.cursor += segment.len();
                path.push(segment);
                continue;
            }
            break;
        }
        self.skip_trivia();
        if !self.rest().starts_with('(') {
            return Err(self.error("expected `(` after the function path"));
        }
        self.cursor += 1;
        let is_cm_parse = path.as_slice() == ["cm", "parse"];
        let mut args = Vec::new();
        loop {
            self.skip_trivia();
            if self.rest().starts_with(')') {
                self.cursor += 1;
                break;
            }
            if is_cm_parse && args.is_empty() {
                // `cm.parse(grammar.rule, text)` — the first argument is a
                // rule path, not a capture; take it verbatim as text.
                let mut rule = String::new();
                loop {
                    let word = self.word_len_at(self.cursor);
                    if word == 0 {
                        break;
                    }
                    rule.push_str(&self.text[self.cursor..self.cursor + word]);
                    self.cursor += word;
                    if self.rest().starts_with('.') {
                        rule.push('.');
                        self.cursor += 1;
                        continue;
                    }
                    break;
                }
                args.push(TmplValue::Str(rule));
            } else {
                args.push(self.value()?);
            }
            self.skip_trivia();
            if self.rest().starts_with(',') {
                self.cursor += 1;
            }
        }
        Ok(TmplNode::Call {
            path,
            args,
            span: self.node_span(start),
        })
    }

    fn value(&mut self) -> Result<TmplValue, Diagnostic> {
        self.skip_trivia();
        if self.rest().starts_with('@') {
            let node = self.call_node(true)?;
            let TmplNode::Call { path, args, .. } = node else {
                unreachable!("call_node returns a Call node")
            };
            return Ok(TmplValue::Call { path, args });
        }
        if self.cm_call_ahead() {
            let node = self.call_node(false)?;
            let TmplNode::Call { path, args, .. } = node else {
                unreachable!("call_node returns a Call node")
            };
            return Ok(TmplValue::Call { path, args });
        }
        if self.rest().starts_with('$') {
            self.cursor += 1;
            let path = self.dotted_path()?;
            return Ok(TmplValue::Capture { path });
        }
        if let Some((value, len)) = parse_string_literal(self.text, self.cursor) {
            self.cursor += len;
            return Ok(TmplValue::Str(value));
        }
        let digits: String = self
            .rest()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if !digits.is_empty() {
            if let Ok(int) = digits.parse::<i64>() {
                self.cursor += digits.len();
                return Ok(TmplValue::Int(int));
            }
            if let Ok(float) = digits.parse::<f64>() {
                self.cursor += digits.len();
                return Ok(TmplValue::Float(float));
            }
        }
        if self.at_word("true") {
            self.cursor += 4;
            return Ok(TmplValue::Bool(true));
        }
        if self.at_word("false") {
            self.cursor += 5;
            return Ok(TmplValue::Bool(false));
        }
        // A bare capture path (`let x = item`) — the `$` is optional in
        // value positions, as in conditions (plan §1.4.5).
        if self
            .peek_word()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            let path = self.dotted_path()?;
            return Ok(TmplValue::Capture { path });
        }
        Err(self.error("expected a value"))
    }
}

// ---------------------------------------------------------------------------
// Elaboration: capture tree + template → generated Checkmate source text
// ---------------------------------------------------------------------------

/// Elaborates `template` with `root` bound to the entry pattern's capture
/// name. Returns the generated code text, or the diagnostics that made
/// elaboration impossible (unknown captures, non-exhaustive matches, failed
/// `require`s). `ct` is the compile-time evaluator backing `@`-calls and the
/// `cm.*` builtins (§8.5); `None` only in tests of templates that use none.
pub fn elaborate(
    template: &Template,
    root_name: &str,
    root: Capture,
    source_span: Span,
    ct: Option<&CtEngine>,
) -> Result<String, Vec<Diagnostic>> {
    let mut elaborator = Elaborator {
        out: String::new(),
        enclosures: Vec::new(),
        scopes: vec![Scope {
            lets: vec![(root_name.to_string(), root)],
            element: None,
        }],
        diagnostics: Vec::new(),
        source_span,
        ct,
    };
    elaborator.nodes(&template.nodes);
    if elaborator.diagnostics.is_empty() {
        Ok(elaborator.out)
    } else {
        Err(elaborator.diagnostics)
    }
}

struct Scope {
    lets: Vec<(String, Capture)>,
    element: Option<Capture>,
}

struct Elaborator<'e> {
    out: String,
    enclosures: Vec<char>,
    scopes: Vec<Scope>,
    diagnostics: Vec<Diagnostic>,
    source_span: Span,
    /// The compile-time evaluator backing `@`-calls and `cm.*` (§8.5).
    ct: Option<&'e CtEngine<'e>>,
}

impl<'e> Elaborator<'e> {
    fn error(&mut self, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::parse(message, self.source_span));
    }

    fn nodes(&mut self, nodes: &[TmplNode]) {
        for node in nodes {
            self.node(node);
        }
    }

    fn node(&mut self, node: &TmplNode) {
        match node {
            TmplNode::Text { text, .. } => {
                self.track_enclosures(text);
                self.out.push_str(text);
            }
            TmplNode::Splice { path, accessor, .. } => match self.resolve(path, accessor.clone()) {
                Ok(capture) => match self.splice_capture(&capture) {
                    Ok(text) => self.out.push_str(&text),
                    Err(message) => self.error(message),
                },
                Err(message) => self.error(message),
            },
            TmplNode::Interp { parts, .. } => {
                self.out.push('"');
                for part in parts {
                    match part {
                        TmplStrPart::Lit(text) => self.out.push_str(&escape_checkmate(text)),
                        TmplStrPart::Hole { path, accessor } => {
                            match self.resolve(path, accessor.clone()) {
                                Ok(capture) => match self.hole_text(&capture) {
                                    Ok(text) => self.out.push_str(&escape_checkmate(&text)),
                                    Err(message) => self.error(message),
                                },
                                Err(message) => self.error(message),
                            }
                        }
                    }
                }
                self.out.push('"');
            }
            TmplNode::Each {
                element,
                list,
                body,
                ..
            } => {
                let items = match self.resolve_value(list) {
                    Ok(capture) => match capture.kind {
                        CaptureKind::List(items) => items,
                        CaptureKind::Opt(Some(inner)) => match inner.kind {
                            CaptureKind::List(items) => items,
                            _ => {
                                self.error("`each` list must be a list capture");
                                return;
                            }
                        },
                        _ => {
                            self.error("`each` list must be a list capture");
                            return;
                        }
                    },
                    Err(message) => {
                        self.error(message);
                        return;
                    }
                };
                // Join rule (plan §1.4.7): comma-joined in positional
                // lists/array literals, newline-delimited for statement
                // lists, map entries, and named args.
                let join = match self.enclosures.last() {
                    Some('(') | Some('[') => ", ",
                    _ => "\n",
                };
                let mut snapshots = Vec::new();
                for item in &items {
                    let saved_out = std::mem::take(&mut self.out);
                    let saved_stack = self.enclosures.clone();
                    self.scopes.push(Scope {
                        lets: vec![(element.clone(), item.clone())],
                        element: Some(item.clone()),
                    });
                    self.nodes(&body.nodes);
                    self.scopes.pop();
                    let emitted = std::mem::replace(&mut self.out, saved_out);
                    self.enclosures = saved_stack;
                    // Each repetition's own layout whitespace at its edges
                    // is template-source artifact, not code (plan §1.4.9);
                    // trimming keeps list joins well-formed.
                    snapshots.push(emitted.trim().to_string());
                }
                self.out.push_str(&snapshots.join(join));
            }
            TmplNode::When {
                cond,
                then,
                otherwise,
                ..
            } => {
                let hit = match self.eval(cond) {
                    Some(value) => matches!(value, CtxVal::Bool(true)),
                    None => {
                        self.error("condition did not evaluate");
                        return;
                    }
                };
                if hit {
                    self.nodes(&then.nodes);
                } else {
                    self.nodes(&otherwise.nodes);
                }
            }
            TmplNode::Match {
                scrutinee, arms, ..
            } => {
                let capture = match self.resolve(scrutinee, None) {
                    Ok(capture) => capture,
                    Err(message) => {
                        self.error(message);
                        return;
                    }
                };
                let record = match &capture.kind {
                    CaptureKind::Record { .. } => capture.clone(),
                    _ => {
                        self.error("match scrutinee must be a tagged record (oneof capture)");
                        return;
                    }
                };
                let tag = match &record.kind {
                    CaptureKind::Record { tag, .. } => tag.clone(),
                    _ => unreachable!(),
                };
                let Some((_, arm)) = arms.iter().find(|(label, _)| label == &tag) else {
                    self.error(format!("non-exhaustive template match: no arm for `{tag}`"));
                    return;
                };
                self.scopes.push(Scope {
                    lets: Vec::new(),
                    element: Some(record),
                });
                self.nodes(&arm.nodes);
                self.scopes.pop();
            }
            TmplNode::Let { name, value, .. } => match self.resolve_value(value) {
                Ok(capture) => {
                    if let Some(scope) = self.scopes.last_mut() {
                        scope.lets.push((name.clone(), capture));
                    }
                }
                Err(message) => self.error(message),
            },
            TmplNode::Require { cond, message, .. } => {
                if self.eval(cond) != Some(CtxVal::Bool(true)) {
                    let span = first_capture_span(cond).unwrap_or(self.source_span);
                    self.diagnostics.push(Diagnostic::parse(
                        format!("require failed: {message}"),
                        span,
                    ));
                }
            }
            TmplNode::Call { path, args, span } => {
                self.emit_call(path, args, *span);
            }
        }
    }

    /// Evaluates one compile-time call (a user `@fn` or a `cm.*` builtin)
    /// and splices its result: a `code` result emits its text raw, a plain
    /// result renders as a Checkmate literal (§8.5).
    fn emit_call(&mut self, path: &[String], args: &[TmplValue], span: Span) {
        match self.eval_call(path, args) {
            Ok(result) => {
                let text = if result.is_code {
                    match &result.value {
                        Value::Str(text) => Ok(text.clone()),
                        other => Err(format!(
                            "a `code` result must be text, got {}",
                            value_kind_name(other)
                        )),
                    }
                } else {
                    cteval::render_value(&result.value)
                };
                match text {
                    Ok(text) => {
                        self.track_enclosures(&text);
                        self.out.push_str(&text);
                    }
                    Err(message) => self.error(message),
                }
            }
            Err(message) => self.diagnostics.push(Diagnostic::parse(message, span)),
        }
    }

    /// Resolves call arguments to captures and invokes the compile-time
    /// evaluator. Shared by splices, `let` bindings, and nested call args.
    fn eval_call(&self, path: &[String], args: &[TmplValue]) -> Result<cteval::CtResult, String> {
        let Some(engine) = self.ct else {
            return Err(format!(
                "compile-time function `{}` needs the §8.5 evaluator",
                path.join(".")
            ));
        };
        let mut captures = Vec::new();
        for arg in args {
            captures.push(self.resolve_value(arg)?);
        }
        engine.call(path, &captures)
    }

    /// Tracks template-text brackets for the join rule (strings transparent).
    fn track_enclosures(&mut self, text: &str) {
        let mut rest = text;
        while !rest.is_empty() {
            if let Some((_, len)) = parse_string_literal(rest, 0) {
                rest = &rest[len..];
                continue;
            }
            let c = rest.chars().next().unwrap();
            match c {
                '(' | '[' | '{' => self.enclosures.push(c),
                ')' | ']' | '}' => {
                    self.enclosures.pop();
                }
                _ => {}
            }
            rest = &rest[c.len_utf8()..];
        }
    }

    fn resolve(&self, path: &[String], accessor: Option<Accessor>) -> Result<Capture, String> {
        let capture = self.lookup(path)?;
        apply(capture, accessor)
    }

    fn resolve_value(&self, value: &TmplValue) -> Result<Capture, String> {
        match value {
            TmplValue::Capture { path } => self.lookup(path),
            TmplValue::Str(value) => Ok(Capture {
                kind: CaptureKind::Text(TextKind::Str),
                matched: value.clone(),
                span: self.source_span,
            }),
            TmplValue::Int(value) => Ok(Capture {
                kind: CaptureKind::Int(*value),
                matched: value.to_string(),
                span: self.source_span,
            }),
            TmplValue::Float(value) => Ok(Capture {
                kind: CaptureKind::Float(*value),
                matched: format!("{value}"),
                span: self.source_span,
            }),
            TmplValue::Bool(value) => Ok(Capture {
                kind: CaptureKind::Text(TextKind::Raw),
                matched: value.to_string(),
                span: self.source_span,
            }),
            TmplValue::Call { path, args } => {
                let result = self.eval_call(path, args)?;
                if result.is_code {
                    let Value::Str(text) = &result.value else {
                        return Err("a `code` result must be text".to_string());
                    };
                    return Ok(Capture {
                        kind: CaptureKind::Text(TextKind::Raw),
                        matched: text.clone(),
                        span: self.source_span,
                    });
                }
                cteval::value_to_capture(&result.value).map(|mut capture| {
                    capture.span = self.source_span;
                    capture
                })
            }
        }
    }

    /// Path resolution: innermost scopes' `let` bindings first, then each
    /// scope's current element's fields (bare `$field`), then outer scopes.
    fn lookup(&self, path: &[String]) -> Result<Capture, String> {
        let head = &path[0];
        for scope in self.scopes.iter().rev() {
            if let Some((_, capture)) = scope.lets.iter().rev().find(|(name, _)| name == head) {
                return navigate(capture.clone(), &path[1..]);
            }
            if let Some(element) = &scope.element
                && navigate(element.clone(), path).is_ok()
            {
                return navigate(element.clone(), path);
            }
        }
        Err(format!("unknown capture `${}`", path.join(".")))
    }

    fn splice_capture(&self, capture: &Capture) -> Result<String, String> {
        match &capture.kind {
            CaptureKind::Text(kind) => match kind {
                TextKind::Str => Ok(format!("\"{}\"", escape_checkmate(capture.matched()))),
                _ => Ok(capture.matched().to_string()),
            },
            CaptureKind::Int(_) | CaptureKind::Float(_) => Ok(capture.matched().to_string()),
            CaptureKind::List(items) => {
                let mut parts = Vec::new();
                for item in items {
                    parts.push(self.splice_capture(item)?);
                }
                Ok(format!("[{}]", parts.join(", ")))
            }
            CaptureKind::Opt(Some(inner)) => self.splice_capture(inner),
            CaptureKind::Opt(None) => Err("spliced an absent optional capture".to_string()),
            CaptureKind::Record { .. } => {
                Err("record captures must be dispatched with template `match`".to_string())
            }
        }
    }

    fn hole_text(&self, capture: &Capture) -> Result<String, String> {
        match &capture.kind {
            CaptureKind::Text(_) | CaptureKind::Int(_) | CaptureKind::Float(_) => {
                Ok(capture.matched().to_string())
            }
            CaptureKind::List(items) => {
                let mut parts = Vec::new();
                for item in items {
                    parts.push(self.hole_text(item)?);
                }
                Ok(parts.join(", "))
            }
            CaptureKind::Record { .. } => Ok(capture.matched().to_string()),
            CaptureKind::Opt(Some(inner)) => self.hole_text(inner),
            CaptureKind::Opt(None) => Err("interpolated an absent optional capture".to_string()),
        }
    }

    // -- condition evaluation (mirrors the matcher's `where` evaluator) -----

    fn eval(&mut self, expression: &CtxExpr) -> Option<CtxVal> {
        match expression {
            CtxExpr::Str(value) => Some(CtxVal::Str(value.clone())),
            CtxExpr::Int(value) => Some(CtxVal::Int(*value)),
            CtxExpr::Float(value) => Some(CtxVal::Float(*value)),
            CtxExpr::Bool(value) => Some(CtxVal::Bool(*value)),
            CtxExpr::Capture { path, accessor } => {
                let capture = self.lookup(path).ok()?;
                apply(capture, accessor.clone()).ok().map(CtxVal::Capture)
            }
            CtxExpr::Bin(op, lhs, rhs) => {
                let lhs = self.eval(lhs)?;
                let rhs = self.eval(rhs)?;
                eval_bin(op.clone(), &lhs, &rhs)
            }
            CtxExpr::Not(inner) => match self.eval(inner)? {
                CtxVal::Bool(value) => Some(CtxVal::Bool(!value)),
                _ => None,
            },
            CtxExpr::SomeIn { var, list, cond } => {
                let items = self.eval_list(list)?;
                for item in items {
                    self.scopes.push(Scope {
                        lets: vec![(var.clone(), item)],
                        element: None,
                    });
                    let hit = self.eval(cond) == Some(CtxVal::Bool(true));
                    self.scopes.pop();
                    if hit {
                        return Some(CtxVal::Bool(true));
                    }
                }
                Some(CtxVal::Bool(false))
            }
            CtxExpr::AllIn { var, list, cond } => {
                let items = self.eval_list(list)?;
                let mut all = true;
                for item in items {
                    self.scopes.push(Scope {
                        lets: vec![(var.clone(), item)],
                        element: None,
                    });
                    if self.eval(cond) != Some(CtxVal::Bool(true)) {
                        all = false;
                    }
                    self.scopes.pop();
                }
                Some(CtxVal::Bool(all))
            }
            CtxExpr::Present { path } => {
                let present = self
                    .lookup(path)
                    .map(|capture| capture.is_present())
                    .unwrap_or(false);
                Some(CtxVal::Bool(present))
            }
            CtxExpr::Call { path, args } => {
                // A compile-time call in a condition position: resolve the
                // condition-language arguments into captures, invoke the
                // engine, and convert the result back into a value.
                let result = (|| {
                    let engine = self.ct?;
                    let mut captures = Vec::new();
                    for arg in args {
                        captures.push(ctx_val_to_capture(self.eval(arg)?));
                    }
                    engine.call(path, &captures).ok()
                })();
                match result {
                    Some(result) => ctx_val_from_value(result.value),
                    None => {
                        self.error("condition call did not evaluate");
                        None
                    }
                }
            }
        }
    }

    fn eval_list(&mut self, expression: &CtxExpr) -> Option<Vec<Capture>> {
        match self.eval(expression)? {
            CtxVal::Capture(capture) => match capture.kind {
                CaptureKind::List(items) => Some(items),
                _ => None,
            },
            _ => None,
        }
    }
}

fn navigate(capture: Capture, segments: &[String]) -> Result<Capture, String> {
    let mut current = capture;
    for (index, segment) in segments.iter().enumerate() {
        if let CaptureKind::Opt(Some(inner)) = current.kind {
            current = *inner;
        }
        match &current.kind {
            CaptureKind::Record { fields, .. } => {
                match fields.iter().find(|(name, _)| name == segment) {
                    Some((_, capture)) => current = capture.clone(),
                    // Field lookup first; a trailing accessor keyword
                    // (`.matched`, `.length`, …) is the fallback (plan
                    // §1.4.4 — every capture exposes these).
                    None => {
                        let last = index + 1 == segments.len();
                        if let Some(accessor) = accessor_keyword(segment).filter(|_| last) {
                            return apply(current, Some(accessor));
                        }
                        return Err(format!("no field `{segment}` on the capture"));
                    }
                }
            }
            // Non-record captures (lists, text, …) expose only their
            // trailing accessors: `$item.children.length`.
            _ => {
                let last = index + 1 == segments.len();
                if let Some(accessor) = accessor_keyword(segment).filter(|_| last) {
                    return apply(current, Some(accessor));
                }
                return Err(format!("cannot navigate into `{segment}`"));
            }
        }
    }
    Ok(current)
}

/// The accessor named by a path segment, if it is one of the keywords.
fn accessor_keyword(segment: &str) -> Option<Accessor> {
    match segment {
        "matched" => Some(Accessor::Matched),
        "length" => Some(Accessor::Length),
        "line" => Some(Accessor::Line),
        "col" => Some(Accessor::Col),
        _ => None,
    }
}

fn apply(capture: Capture, accessor: Option<Accessor>) -> Result<Capture, String> {
    match accessor {
        None => Ok(capture),
        // `.matched` trims edge whitespace (plan §1.4.4).
        Some(Accessor::Matched) => Ok(Capture {
            kind: CaptureKind::Text(TextKind::Raw),
            matched: capture.matched().trim().to_string(),
            span: capture.span,
        }),
        Some(Accessor::Length) => {
            let length = match capture.kind {
                CaptureKind::List(items) => items.len() as i64,
                CaptureKind::Record { fields, .. } => fields.len() as i64,
                _ => 0,
            };
            Ok(Capture {
                kind: CaptureKind::Int(length),
                matched: length.to_string(),
                span: capture.span,
            })
        }
        // Real source-line mapping lands with Task 4's diagnostics work.
        Some(Accessor::Line) | Some(Accessor::Col) => Ok(Capture {
            kind: CaptureKind::Int(0),
            matched: "0".to_string(),
            span: capture.span,
        }),
    }
}

pub(crate) fn escape_checkmate(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

fn first_capture_span(expression: &CtxExpr) -> Option<Span> {
    match expression {
        CtxExpr::Capture { .. } => None,
        CtxExpr::Bin(_, lhs, rhs) => first_capture_span(lhs).or_else(|| first_capture_span(rhs)),
        CtxExpr::Not(inner) => first_capture_span(inner),
        CtxExpr::SomeIn { list, cond, .. } | CtxExpr::AllIn { list, cond, .. } => {
            first_capture_span(list).or_else(|| first_capture_span(cond))
        }
        _ => None,
    }
}

/// A capture in a scalar position: its number, or its trimmed matched text.
fn coerce_scalar(value: &CtxVal) -> CtxVal {
    match value {
        CtxVal::Capture(capture) => match &capture.kind {
            CaptureKind::Int(value) => CtxVal::Int(*value),
            CaptureKind::Float(value) => CtxVal::Float(*value),
            _ => CtxVal::Str(capture.matched().trim().to_string()),
        },
        other => other.clone(),
    }
}

fn eval_bin(op: CtxBinOp, lhs: &CtxVal, rhs: &CtxVal) -> Option<CtxVal> {
    let result = match op {
        CtxBinOp::And => match (lhs, rhs) {
            (CtxVal::Bool(a), CtxVal::Bool(b)) => Some(*a && *b),
            _ => None,
        },
        CtxBinOp::Or => match (lhs, rhs) {
            (CtxVal::Bool(a), CtxVal::Bool(b)) => Some(*a || *b),
            _ => None,
        },
        CtxBinOp::Eq | CtxBinOp::Ne => {
            // A bare capture compares by its scalar value (the trimmed
            // matched text, or its number) — this makes `close == name`
            // work when both sides are captures (§8.3.4).
            let lhs = coerce_scalar(lhs);
            let rhs = coerce_scalar(rhs);
            let equal = match (&lhs, &rhs) {
                (CtxVal::Str(a), CtxVal::Str(b)) => a == b,
                (CtxVal::Int(a), CtxVal::Int(b)) => a == b,
                (CtxVal::Float(a), CtxVal::Float(b)) => a == b,
                (CtxVal::Bool(a), CtxVal::Bool(b)) => a == b,
                _ => return None,
            };
            Some(if op == CtxBinOp::Eq { equal } else { !equal })
        }
        CtxBinOp::Lt | CtxBinOp::Le | CtxBinOp::Gt | CtxBinOp::Ge => {
            use std::cmp::Ordering;
            let ordering = match (lhs, rhs) {
                (CtxVal::Int(a), CtxVal::Int(b)) => a.cmp(b),
                (CtxVal::Float(a), CtxVal::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
                _ => return None,
            };
            Some(match op {
                CtxBinOp::Lt => ordering == Ordering::Less,
                CtxBinOp::Le => ordering != Ordering::Greater,
                CtxBinOp::Gt => ordering == Ordering::Greater,
                _ => ordering != Ordering::Less,
            })
        }
    };
    result.map(CtxVal::Bool)
}

/// A condition value in the elaborator (mirrors the matcher's evaluator).
#[derive(Debug, Clone, PartialEq)]
enum CtxVal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Capture(Capture),
}

/// Converts a condition value into a capture for `@`-call arguments in
/// condition positions.
fn ctx_val_to_capture(value: CtxVal) -> Capture {
    match value {
        CtxVal::Bool(value) => Capture {
            kind: CaptureKind::Text(TextKind::Raw),
            matched: value.to_string(),
            span: Span::missing(0),
        },
        CtxVal::Str(value) => Capture {
            kind: CaptureKind::Text(TextKind::Raw),
            matched: value,
            span: Span::missing(0),
        },
        CtxVal::Int(value) => Capture {
            kind: CaptureKind::Int(value),
            matched: value.to_string(),
            span: Span::missing(0),
        },
        CtxVal::Float(value) => Capture {
            kind: CaptureKind::Float(value),
            matched: format!("{value}"),
            span: Span::missing(0),
        },
        CtxVal::Capture(capture) => capture,
    }
}

/// Converts a compile-time result into a condition value: scalars keep
/// their kind, everything else bridges back into a capture.
fn ctx_val_from_value(value: Value) -> Option<CtxVal> {
    match value {
        Value::Bool(value) => Some(CtxVal::Bool(value)),
        Value::Str(value) => Some(CtxVal::Str(value)),
        Value::Int(value) => Some(CtxVal::Int(value)),
        Value::Float(value) => Some(CtxVal::Float(value)),
        other => match cteval::value_to_capture(&other) {
            Ok(capture) => Some(CtxVal::Capture(capture)),
            Err(_) => None,
        },
    }
}

fn value_kind_name(value: &Value) -> &'static str {
    match value {
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::Bool(_) => "bool",
        Value::Void => "void",
        Value::Struct { .. } => "a struct",
        Value::Enum { .. } => "an enum",
        Value::Array(_) => "an array",
        Value::Map(_) => "a map",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mega::matcher::{
        CompiledGrammar, CompiledRule, GrammarSet, MatchRegion, match_entry,
    };
    use crate::mega::pattern::{parse_pattern, parse_rule_declaration};

    fn elaborate_with(
        rules: &[(&str, &str)],
        pattern_text: &str,
        template_text: &str,
        region: &str,
    ) -> Result<String, Vec<Diagnostic>> {
        let mut compiled = CompiledGrammar {
            name: "test".to_string(),
            profile: crate::mega::profile::default_profile(),
            rules: Vec::new(),
        };
        for (name, body) in rules {
            let (rule_name, context, pattern, _) =
                parse_rule_declaration(body, 0, Span::new(0, body.len())).unwrap();
            assert_eq!(&rule_name, name);
            compiled.rules.push(CompiledRule {
                name: rule_name,
                context,
                pattern,
            });
        }
        let mut set = GrammarSet::default();
        set.grammars.push(compiled);
        set.grammars.push(CompiledGrammar {
            name: "\u{0}default".to_string(),
            profile: crate::mega::profile::default_profile(),
            rules: Vec::new(),
        });
        let pattern = parse_pattern(pattern_text, Span::new(0, pattern_text.len())).unwrap();
        let template = parse_template(template_text, Span::new(0, template_text.len())).unwrap();
        let match_region = MatchRegion::new(region, 0, region);
        let root = match_entry(&set, 0, &pattern, &match_region, None)
            .map_err(|failure| vec![Diagnostic::parse(failure.message, Span::missing(0))])?;
        elaborate(&template, "root", root, Span::missing(0), None)
    }

    #[test]
    fn interpolation_and_splices_render_captures() {
        let out = elaborate_with(
            &[("word", "rule word { scan [a-z] as w }")],
            "test.word as root",
            "$\"value: {$root.w}!\"",
            "hello",
        )
        .unwrap();
        assert_eq!(out, "\"value: hello!\"");
    }

    #[test]
    fn each_joins_with_commas_inside_brackets_and_newlines_outside() {
        // Inside template `[ … ]` (an array literal): comma join.
        let out = elaborate_with(
            &[(
                "word",
                "rule word { each sep \",\" { scan [a-z] as w } as ws }",
            )],
            "test.word as root",
            "[[each in $root.ws { $item.w }]]",
            "a,b,c",
        )
        .unwrap();
        assert_eq!(out, "[a, b, c]");
    }

    #[test]
    fn when_selects_on_present() {
        let out = elaborate_with(
            &[(
                "opt",
                "rule opt { optional { \"-\" scan [0-9] as num } scan [a-z] as name }",
            )],
            "test.opt as root",
            "[when present($root.num) { numbered } else { plain }]",
            "-7x",
        )
        .unwrap();
        assert_eq!(out.trim(), "numbered");
        let out = elaborate_with(
            &[(
                "opt",
                "rule opt { optional { \"-\" scan [0-9] as num } scan [a-z] as name }",
            )],
            "test.opt as root",
            "[when present($root.num) { numbered } else { plain }]",
            "x",
        )
        .unwrap();
        assert_eq!(out.trim(), "plain");
    }

    #[test]
    fn match_dispatches_on_the_oneof_tag() {
        let out = elaborate_with(
            &[(
                "value",
                "rule value { oneof { yes => \"y\", no => \"n\" } }",
            )],
            "test.value as root",
            "match ($root) { yes => \"affirmative\", no => \"negative\" }",
            "n",
        )
        .unwrap();
        // The arm body is the template's own quoted text node.
        assert_eq!(out.trim(), "\"negative\"");
    }
}
