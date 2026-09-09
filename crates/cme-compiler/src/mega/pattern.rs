//! The pattern-text parser (WHITEPAPER §8.3): turns a `magic` declaration's
//! pattern source into a `cme_core::magic::Pattern`.
//!
//! Bind syntax reconciliation (plan §1.4.5): both `ruleref IDENT` /
//! `$fragment IDENT` (the §8.8 house style) and the EBNF's `as BIND` are
//! accepted and identical. Elements are whitespace-separated; `//` and
//! `/* */` comments are transparent inside pattern text.

use crate::diagnostics::Diagnostic;
use crate::mega::ctxexpr;
use crate::mega::profile::{parse_string_literal, skip_ws_and_comments};
use cme_core::Span;
use cme_core::magic::{
    CharItem, CharSet, ContextField, CtxExpr, FragKind, PatElem, PatKind, Pattern,
};

/// Words that can never be implicit capture names because they head pattern
/// constructs or bind syntax. Also the word set of the `notReserved`
/// fragment validator (§8.3.3).
pub(crate) const RESERVED: &[&str] = &[
    "each", "sep", "trailing", "optional", "oneof", "peek", "not", "until", "lineRest", "eol",
    "line", "eof", "soft", "indent", "raw", "where", "label", "as", "with", "context", "recur",
    "verbatim", "in",
];

/// Parses a whole pattern (the text inside a magic declaration's parens or a
/// rule body's braces).
pub fn parse_pattern(text: &str, span: Span) -> Result<Pattern, Diagnostic> {
    let mut parser = PatternParser {
        text,
        span,
        cursor: 0,
        pending_validator: None,
    };
    let pattern = parser.sequence(&[])?;
    parser.skip_trivia();
    if parser.cursor < parser.text.len() {
        return Err(parser.error(format!(
            "unexpected `{}` after the pattern",
            parser.rest().chars().next().unwrap()
        )));
    }
    reject_indent_after_eol(&pattern)?;
    Ok(pattern)
}

/// Parses a `rule name [context] { … }` declaration out of a grammar body,
/// returning the rule name, its context fields, and its parsed pattern.
pub fn parse_rule_declaration(
    text: &str,
    start: usize,
    span: Span,
) -> Result<(String, Vec<cme_core::magic::ContextField>, Pattern, usize), Diagnostic> {
    let mut parser = PatternParser {
        text,
        span,
        cursor: start,
        pending_validator: None,
    };
    parser.cursor += "rule".len();
    parser.skip_trivia();
    let name = parser.ident();
    parser.skip_trivia();
    let mut context = Vec::new();
    if parser.rest().starts_with('(') {
        parser.cursor += 1;
        parser.skip_trivia();
        if parser.rest().starts_with("context") {
            parser.cursor += "context".len();
            parser.skip_trivia();
            context = parser.context_fields()?;
        } else {
            return Err(parser.error("expected `context` in the rule signature"));
        }
        parser.skip_trivia();
        if !parser.rest().starts_with(')') {
            return Err(parser.error("expected `)` to close the rule signature"));
        }
        parser.cursor += 1;
        parser.skip_trivia();
    }
    if !parser.rest().starts_with('{') {
        return Err(parser.error("expected `{` to open the rule body"));
    }
    parser.cursor += 1;
    let pattern = parser.sequence(&["}"])?;
    parser.skip_trivia();
    if !parser.rest().starts_with('}') {
        return Err(parser.error("expected `}` to close the rule body"));
    }
    parser.cursor += 1;
    reject_indent_after_eol(&pattern)?;
    Ok((name, context, pattern, parser.cursor))
}

/// Static check (§8.3.5): `indent` may not directly follow `eol` in a
/// sequence — `indent` performs its own line advancement, so the pair would
/// double-advance. Checked over every nested sequence of the pattern.
fn reject_indent_after_eol(pattern: &Pattern) -> Result<(), Diagnostic> {
    for window in pattern.elems.windows(2) {
        if matches!(window[0].kind, PatKind::Eol)
            && matches!(window[1].kind, PatKind::Indent { .. })
        {
            return Err(Diagnostic::parse(
                "`indent` may not directly follow `eol`: indent performs its own line advancement (§8.3.5)",
                window[1].span,
            ));
        }
    }
    for elem in &pattern.elems {
        match &elem.kind {
            PatKind::Soft(body)
            | PatKind::Raw(body)
            | PatKind::Label { body, .. }
            | PatKind::Group { body, .. } => reject_indent_after_eol(body)?,
            PatKind::Optional { body, .. } => reject_indent_after_eol(body)?,
            PatKind::Each { sep, body, .. } => {
                if let Some(sep) = sep {
                    reject_indent_after_eol(sep)?;
                }
                reject_indent_after_eol(body)?;
            }
            PatKind::OneOf { branches, .. } => {
                for (_, branch) in branches {
                    reject_indent_after_eol(branch)?;
                }
            }
            PatKind::Peek { body, .. } => reject_indent_after_eol(body)?,
            PatKind::Until { stop, .. } => reject_indent_after_eol(stop)?,
            PatKind::Indent {
                body: Some(body), ..
            } => {
                reject_indent_after_eol(body)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// A parsed fragment: kind, case-insensitive flag, validator path, bind.
type FragmentParts = (FragKind, bool, Option<Vec<String>>, Option<String>);

struct PatternParser<'a> {
    text: &'a str,
    span: Span,
    cursor: usize,
    /// A `<rule.path>` parameter parsed before the fragment's bind is read;
    /// lifted into `validator` by `fragment()`.
    pending_validator: Option<Vec<String>>,
}

impl<'a> PatternParser<'a> {
    fn rest(&self) -> &'a str {
        &self.text[self.cursor..]
    }

    fn skip_trivia(&mut self) {
        skip_ws_and_comments(
            self.text,
            &mut self.cursor,
            &crate::mega::profile::checkmate_scan_profile(),
        );
    }

    fn error(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic::parse(message, self.here())
    }

    fn here(&self) -> Span {
        let start = (self.span.start + self.cursor).min(self.span.end);
        Span::new(start, start)
    }

    fn element_span(&self, start: usize) -> Span {
        let from = (self.span.start + start).min(self.span.end);
        let to = (self.span.start + self.cursor).min(self.span.end);
        Span::new(from, to.max(from))
    }

    /// Reads an identifier word at the cursor (no trivia skipping).
    fn peek_word(&self) -> &'a str {
        let rest = self.rest();
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        &rest[..end]
    }

    fn ident(&mut self) -> String {
        let word = self.peek_word().to_string();
        self.cursor += word.len();
        word
    }

    /// True when the next word is an identifier usable as an implicit bind.
    fn at_implicit_bind(&self) -> bool {
        let word = self.peek_word();
        !word.is_empty()
            && !RESERVED.contains(&word)
            && word.chars().next().unwrap().is_ascii_alphabetic()
    }

    /// Parses the `context { [Type] name [= default], … }` field list.
    fn context_fields(&mut self) -> Result<Vec<ContextField>, Diagnostic> {
        if !self.rest().starts_with('{') {
            return Err(self.error("expected `{` to open the context fields"));
        }
        self.cursor += 1;
        let mut fields = Vec::new();
        loop {
            self.skip_trivia();
            if self.rest().starts_with('}') {
                self.cursor += 1;
                return Ok(fields);
            }
            // `[Type] name [= default]` — the whitepaper's `selector parent
            // = none`: an optional rule-typed first word, then the name.
            let first = self.ident();
            self.skip_trivia();
            let second = self.peek_word();
            let (name, _type) = if !second.is_empty()
                && !RESERVED.contains(&second)
                && second.chars().next().unwrap().is_ascii_alphabetic()
            {
                let name = self.ident();
                (name, Some(first))
            } else {
                (first, None)
            };
            self.skip_trivia();
            let default = if self.rest().starts_with('=') {
                self.cursor += 1;
                Some(ctxexpr::parse_expr(self.text, &mut self.cursor, self.span)?)
            } else {
                None
            };
            fields.push(ContextField { name, default });
            self.skip_trivia();
            if self.rest().starts_with(',') {
                self.cursor += 1;
            }
        }
    }

    /// Parses elements until a terminator token (`}` / `)`) or a
    /// `label =>` lookahead inside a oneof. Terminators are not consumed.
    fn sequence(&mut self, terminators: &[&str]) -> Result<Pattern, Diagnostic> {
        let mut elems = Vec::new();
        loop {
            self.skip_trivia();
            if self.cursor >= self.text.len() {
                if terminators.is_empty() {
                    return Ok(Pattern { elems });
                }
                return Err(self.error(format!(
                    "expected {} before the end of the pattern",
                    terminators.join(" or ")
                )));
            }
            if terminators.iter().any(|t| self.rest().starts_with(t)) {
                return Ok(Pattern { elems });
            }
            // A `label =>` lookahead ends a oneof branch body.
            if self.at_branch_boundary() {
                return Ok(Pattern { elems });
            }
            let element = self.element()?;
            elems.push(element);
        }
    }

    /// True when the cursor sits at `IDENT =>` (the start of the next oneof
    /// branch) at the current nesting level.
    fn at_branch_boundary(&self) -> bool {
        let word = self.peek_word();
        if word.is_empty() || RESERVED.contains(&word) {
            return false;
        }
        let mut cursor = self.cursor + word.len();
        skip_ws_and_comments(
            self.text,
            &mut cursor,
            &crate::mega::profile::checkmate_scan_profile(),
        );
        self.text[cursor..].starts_with("=>")
    }

    fn element(&mut self) -> Result<PatElem, Diagnostic> {
        self.skip_trivia();
        let start = self.cursor;
        let rest = self.rest();
        let first = rest.chars().next().unwrap();

        let kind = match first {
            '"' => {
                let (value, len) = parse_string_literal(self.text, self.cursor)
                    .ok_or_else(|| self.error("malformed string literal"))?;
                self.cursor += len;
                PatKind::Lit {
                    text: value,
                    insensitive: false,
                }
            }
            'i' if rest[1..].starts_with('"') => {
                self.cursor += 1;
                let (value, len) = parse_string_literal(self.text, self.cursor)
                    .ok_or_else(|| self.error("malformed string literal"))?;
                self.cursor += len;
                PatKind::Lit {
                    text: value,
                    insensitive: true,
                }
            }
            '[' => {
                let set = self.class_set()?;
                let bind = self.trailing_bind()?;
                PatKind::Class { set, bind }
            }
            'i' if rest[1..].starts_with('$') => {
                self.cursor += 1;
                let (kind, insensitive, validator, bind) = self.fragment()?;
                let _ = insensitive;
                PatKind::Fragment {
                    kind,
                    insensitive: true,
                    validator,
                    bind,
                }
            }
            '$' => {
                let (kind, insensitive, validator, bind) = self.fragment()?;
                let _ = insensitive;
                PatKind::Fragment {
                    kind,
                    insensitive: false,
                    validator,
                    bind,
                }
            }
            '(' => {
                self.cursor += 1;
                let body = self.sequence(&[")"])?;
                self.skip_trivia();
                if !self.rest().starts_with(')') {
                    return Err(self.error("expected `)` to close the group"));
                }
                self.cursor += 1;
                let bind = self.as_bind()?;
                PatKind::Group {
                    body: Box::new(body),
                    bind,
                }
            }
            other if other.is_ascii_alphabetic() || other == '_' => match self.peek_word() {
                "any" => {
                    self.cursor += 3;
                    let bind = self.as_bind()?;
                    PatKind::Any { bind }
                }
                "scan" => {
                    self.cursor += 4;
                    self.skip_trivia();
                    if !self.rest().starts_with('[') {
                        return Err(self.error("expected `[` after `scan`"));
                    }
                    let set = self.class_set()?;
                    let bind = self.as_bind()?;
                    PatKind::Scan { set, bind }
                }
                "until" => {
                    self.cursor += 5;
                    self.skip_trivia();
                    let stop = if self.rest().starts_with('{') {
                        self.cursor += 1;
                        let stop = self.sequence(&["}"])?;
                        self.skip_trivia();
                        if !self.rest().starts_with('}') {
                            return Err(self.error("expected `}` to close the until stop"));
                        }
                        self.cursor += 1;
                        stop
                    } else {
                        let (value, len) = parse_string_literal(self.text, self.cursor)
                            .ok_or_else(|| {
                                self.error("expected a literal or `{{` after `until`")
                            })?;
                        self.cursor += len;
                        Pattern::single(
                            PatKind::Lit {
                                text: value,
                                insensitive: false,
                            },
                            self.element_span(start),
                        )
                    };
                    let bind = self.as_bind()?;
                    PatKind::Until {
                        stop: Box::new(stop),
                        bind,
                    }
                }
                "lineRest" => {
                    self.cursor += 8;
                    let bind = self.as_bind()?;
                    PatKind::LineRest { bind }
                }
                "eol" => {
                    self.cursor += 3;
                    PatKind::Eol
                }
                "line" => {
                    self.cursor += 4;
                    PatKind::Line
                }
                "eof" => {
                    self.cursor += 3;
                    PatKind::Eof
                }
                "soft" => {
                    self.cursor += 4;
                    let body = self.braced_pattern("soft")?;
                    PatKind::Soft(Box::new(body))
                }
                "optional" => {
                    self.cursor += 8;
                    let body = self.braced_pattern("optional")?;
                    let bind = self.as_bind()?;
                    PatKind::Optional {
                        body: Box::new(body),
                        bind,
                    }
                }
                "each" => self.each()?,
                "oneof" => self.oneof()?,
                "peek" => {
                    self.cursor += 4;
                    let body = self.braced_pattern("peek")?;
                    PatKind::Peek {
                        negated: false,
                        body: Box::new(body),
                    }
                }
                "not" => {
                    self.cursor += 3;
                    let body = self.braced_pattern("not")?;
                    PatKind::Peek {
                        negated: true,
                        body: Box::new(body),
                    }
                }
                "recur" => {
                    self.cursor += 5;
                    PatKind::Recur
                }
                "indent" => {
                    self.cursor += 6;
                    self.skip_trivia();
                    if self.peek_word() == "verbatim" {
                        self.cursor += 8;
                        let bind = self.as_bind()?;
                        PatKind::Indent {
                            body: None,
                            verbatim: bind,
                        }
                    } else {
                        let body = self.braced_pattern("indent")?;
                        PatKind::Indent {
                            body: Some(Box::new(body)),
                            verbatim: None,
                        }
                    }
                }
                "raw" => {
                    self.cursor += 3;
                    let body = self.braced_pattern("raw")?;
                    PatKind::Raw(Box::new(body))
                }
                "where" => {
                    self.cursor += 5;
                    let cond = ctxexpr::parse_expr(self.text, &mut self.cursor, self.span)?;
                    PatKind::Where { cond }
                }
                "label" => {
                    self.cursor += 5;
                    self.skip_trivia();
                    let (message, len) = parse_string_literal(self.text, self.cursor)
                        .ok_or_else(|| self.error("expected a message string after `label`"))?;
                    self.cursor += len;
                    let body = self.braced_pattern("label")?;
                    PatKind::Label {
                        message,
                        body: Box::new(body),
                    }
                }
                word if !RESERVED.contains(&word) => {
                    // A rule reference, possibly qualified, possibly with
                    // `with context`, then an implicit or `as` bind.
                    let mut path = vec![word.to_string()];
                    self.cursor += word.len();
                    loop {
                        // Path segments (`json.value`) stay on the element's
                        // own line; trivia past a line break belongs to the
                        // next construct (§1.4.5 errata).
                        let before = self.cursor;
                        self.skip_trivia();
                        if self.text[before..self.cursor].contains('\n') {
                            self.cursor = before;
                            break;
                        }
                        if self.rest().starts_with('.')
                            && self
                                .text
                                .get(self.cursor + 1..)
                                .and_then(|rest| rest.chars().next())
                                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                        {
                            self.cursor += 1;
                            let segment = self.peek_word().to_string();
                            self.cursor += segment.len();
                            path.push(segment);
                            continue;
                        }
                        break;
                    }
                    let mut ctx = Vec::new();
                    let before_with = self.cursor;
                    self.skip_trivia();
                    if self.text[before_with..self.cursor].contains('\n') {
                        // A `with context` clause stays on the element's
                        // own line (§1.4.5 errata); rewind so the bind
                        // check sees the line break.
                        self.cursor = before_with;
                    } else if self.peek_word() == "with" {
                        self.cursor += 4;
                        self.skip_trivia();
                        if self.peek_word() != "context" {
                            return Err(self.error("expected `context` after `with`"));
                        }
                        self.cursor += 7;
                        self.skip_trivia();
                        ctx = self.context_bindings()?;
                    }
                    let bind = self.trailing_bind()?;
                    PatKind::RuleRef { path, ctx, bind }
                }
                other => {
                    return Err(self.error(format!("unexpected `{other}` in a pattern")));
                }
            },
            other => {
                return Err(self.error(format!("unexpected `{other}` in a pattern")));
            }
        };
        let span = self.element_span(start);
        Ok(PatElem { span, kind })
    }

    /// `{ pattern }` after a keyword head (`soft`, `optional`, `peek`, …).
    fn braced_pattern(&mut self, head: &str) -> Result<Pattern, Diagnostic> {
        self.skip_trivia();
        if !self.rest().starts_with('{') {
            return Err(self.error(format!("expected `{{` after `{head}`")));
        }
        self.cursor += 1;
        let body = self.sequence(&["}"])?;
        self.skip_trivia();
        if !self.rest().starts_with('}') {
            return Err(self.error(format!("expected `}}` to close `{head}`")));
        }
        self.cursor += 1;
        Ok(body)
    }

    /// `each [+] [sep elem] [trailing] [[n, m]] { body } [as bind]`.
    fn each(&mut self) -> Result<PatKind, Diagnostic> {
        self.cursor += 4;
        self.skip_trivia();
        let mut plus = false;
        if self.rest().starts_with('+') {
            plus = true;
            self.cursor += 1;
            self.skip_trivia();
        }
        let mut sep = None;
        if self.peek_word() == "sep" {
            self.cursor += 3;
            let element = self.element()?;
            sep = Some(Box::new(Pattern {
                elems: vec![element],
            }));
            self.skip_trivia();
        }
        let mut trailing = false;
        if self.peek_word() == "trailing" {
            trailing = true;
            self.cursor += 8;
            self.skip_trivia();
        }
        let mut bounds = None;
        if self.rest().starts_with('[') {
            self.cursor += 1;
            self.skip_trivia();
            let min = self.int()?;
            self.skip_trivia();
            if !self.rest().starts_with(',') {
                return Err(self.error("expected `,` in each bounds"));
            }
            self.cursor += 1;
            self.skip_trivia();
            let max = if self.rest().starts_with(']') {
                None
            } else {
                Some(self.int()?)
            };
            self.skip_trivia();
            if !self.rest().starts_with(']') {
                return Err(self.error("expected `]` to close each bounds"));
            }
            self.cursor += 1;
            bounds = Some((min, max));
        }
        let body = self.braced_pattern("each")?;
        let bind = self.as_bind()?;
        Ok(PatKind::Each {
            plus,
            sep,
            trailing,
            bounds,
            body: Box::new(body),
            bind,
        })
    }

    fn int(&mut self) -> Result<u32, Diagnostic> {
        let digits: String = self
            .rest()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            return Err(self.error("expected a number"));
        }
        self.cursor += digits.len();
        digits.parse::<u32>().map_err(|_| {
            let span = self.here();
            Diagnostic::parse("number too large", span)
        })
    }

    /// `oneof { label => ( body ) | single, … }` — branches separated by
    /// commas and/or newlines.
    fn oneof(&mut self) -> Result<PatKind, Diagnostic> {
        self.cursor += 5;
        self.skip_trivia();
        if !self.rest().starts_with('{') {
            return Err(self.error("expected `{` after `oneof`"));
        }
        self.cursor += 1;
        let mut branches = Vec::new();
        loop {
            self.skip_trivia();
            if self.rest().starts_with('}') {
                self.cursor += 1;
                let bind = self.trailing_bind()?;
                return Ok(PatKind::OneOf { branches, bind });
            }
            let label = self.ident();
            self.skip_trivia();
            if !self.rest().starts_with("=>") {
                return Err(self.error(format!("expected `=>` after branch `{label}`")));
            }
            self.cursor += 2;
            self.skip_trivia();
            let body = if self.rest().starts_with('(') {
                self.cursor += 1;
                let body = self.sequence(&[")"])?;
                self.skip_trivia();
                if !self.rest().starts_with(')') {
                    return Err(self.error("expected `)` to close the branch"));
                }
                self.cursor += 1;
                body
            } else {
                let element = self.element()?;
                Pattern {
                    elems: vec![element],
                }
            };
            branches.push((label, body));
            self.skip_trivia();
            if self.rest().starts_with(',') {
                self.cursor += 1;
            }
        }
    }

    /// `with context { name: expr, … }` bindings at a rule reference.
    fn context_bindings(&mut self) -> Result<Vec<(String, CtxExpr)>, Diagnostic> {
        if !self.rest().starts_with('{') {
            return Err(self.error("expected `{` after `context`"));
        }
        self.cursor += 1;
        let mut bindings = Vec::new();
        loop {
            self.skip_trivia();
            if self.rest().starts_with('}') {
                self.cursor += 1;
                return Ok(bindings);
            }
            let name = self.ident();
            self.skip_trivia();
            if !self.rest().starts_with(':') {
                return Err(self.error(format!("expected `:` after `{name}`")));
            }
            self.cursor += 1;
            let value = ctxexpr::parse_expr(self.text, &mut self.cursor, self.span)?;
            bindings.push((name, value));
            self.skip_trivia();
            if self.rest().starts_with(',') {
                self.cursor += 1;
            }
        }
    }

    /// A `$fragment`, with `<…>` parameters and a direct or `as` bind.
    fn fragment(&mut self) -> Result<FragmentParts, Diagnostic> {
        self.cursor += 1; // `$`
        let mut insensitive = false;
        if self.rest().starts_with('i') && self.rest()[1..].starts_with('$') {
            insensitive = true;
            self.cursor += 1;
        }
        let name = self.ident();
        let kind = match name.as_str() {
            "ident" => FragKind::Ident,
            "word" => FragKind::Word,
            "tag" => FragKind::Tag,
            "int" => FragKind::Int,
            "float" => FragKind::Float,
            "str" => FragKind::Str,
            "tt" => FragKind::Tt,
            "text" => FragKind::Text,
            "template" => FragKind::Template,
            "raw" => FragKind::Raw(None),
            "expr" => FragKind::Expr,
            "type" => FragKind::Type,
            "block" => FragKind::Block,
            other => {
                return Err(self.error(format!("unknown fragment `${other}`")));
            }
        };
        let kind = if matches!(self.rest().chars().next(), Some('<')) {
            self.cursor += 1;
            self.skip_trivia();
            // Either a validator rule path (`$ident<self.notReserved>`) or a
            // parameterized spec (`$raw<grammar.rule>`,
            // `$template<open close rule>`).
            if matches!(self.rest().chars().next(), Some('"')) {
                // Template delimiters: parse and drop for now (Task 6).
                while !self.rest().starts_with('>') && self.cursor < self.text.len() {
                    if matches!(self.rest().chars().next(), Some('"')) {
                        let (_, len) = parse_string_literal(self.text, self.cursor)
                            .ok_or_else(|| self.error("malformed template delimiter"))?;
                        self.cursor += len;
                    } else {
                        self.cursor += self.rest().chars().next().unwrap().len_utf8();
                    }
                    self.skip_trivia();
                }
                kind
            } else {
                let mut path = vec![self.ident()];
                loop {
                    self.skip_trivia();
                    if self.rest().starts_with('.') {
                        self.cursor += 1;
                        path.push(self.ident());
                        continue;
                    }
                    break;
                }
                self.skip_trivia();
                if !self.rest().starts_with('>') {
                    return Err(self.error("expected `>` to close the fragment parameter"));
                }
                self.cursor += 1;
                match &kind {
                    FragKind::Raw(None) => FragKind::Raw(Some(path)),
                    _ => {
                        // A validator on any other fragment.
                        self.pending_validator = Some(path);
                        kind
                    }
                }
            }
        } else {
            kind
        };
        let validator = self.pending_validator.take();
        let bind = self.trailing_bind()?;
        Ok((kind, insensitive, validator, bind))
    }

    /// `as NAME` if present.
    fn as_bind(&mut self) -> Result<Option<String>, Diagnostic> {
        self.skip_trivia();
        if self.peek_word() == "as" {
            self.cursor += 2;
            self.skip_trivia();
            let bind = self.ident();
            return Ok(Some(bind));
        }
        Ok(None)
    }

    /// An implicit identifier bind or `as NAME`.
    fn trailing_bind(&mut self) -> Result<Option<String>, Diagnostic> {
        let start = self.cursor;
        self.skip_trivia();
        if self.peek_word() == "as" {
            return self.as_bind();
        }
        // An implicit (bare-identifier) bind stays on the element's own
        // line: a word on the NEXT line begins a new construct (the next
        // `oneof` branch label, the next element), never a bind. The
        // explicit `as` form may span lines (plan §1.4.5 errata).
        if self.text[start..self.cursor].contains('\n') {
            self.cursor = start;
            return Ok(None);
        }
        if self.at_implicit_bind() {
            let bind = self.ident();
            return Ok(Some(bind));
        }
        self.cursor = start;
        Ok(None)
    }

    /// `[ … ]` character class: bare characters, `a-z` ranges, `\[`-style
    /// escapes, optional leading `^`. A `-` at either edge is literal.
    fn class_set(&mut self) -> Result<CharSet, Diagnostic> {
        if !self.rest().starts_with('[') {
            return Err(self.error("expected `[` to open a character class"));
        }
        self.cursor += 1;
        let mut set = CharSet::empty();
        let mut first = true;
        loop {
            let c = match self.rest().chars().next() {
                Some(c) => c,
                None => return Err(self.error("unterminated character class")),
            };
            if c == ']' {
                self.cursor += 1;
                return Ok(set);
            }
            if c == '^' && first {
                set.negated = true;
                self.cursor += 1;
                first = false;
                continue;
            }
            first = false;
            let value = if c == '\\' {
                self.cursor += 1;
                match self.rest().chars().next() {
                    Some(escaped) => {
                        self.cursor += escaped.len_utf8();
                        match escaped {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '0' => '\0',
                            other => other,
                        }
                    }
                    None => return Err(self.error("dangling escape in a character class")),
                }
            } else {
                self.cursor += c.len_utf8();
                c
            };
            // Range? `x-y` where `-` is followed by something other than `]`.
            if self.rest().starts_with('-')
                && self
                    .rest()
                    .chars()
                    .nth(1)
                    .is_some_and(|next| next != ']' && next != '-')
            {
                self.cursor += 1;
                let hi = match self.rest().chars().next() {
                    Some('\\') => {
                        self.cursor += 1;
                        let escaped = self.rest().chars().next().unwrap();
                        self.cursor += escaped.len_utf8();
                        match escaped {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            other => other,
                        }
                    }
                    Some(c) => {
                        self.cursor += c.len_utf8();
                        c
                    }
                    None => return Err(self.error("unterminated range")),
                };
                set.items.push(CharItem::Range(value, hi));
            } else {
                set.items.push(CharItem::Char(value));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Pattern {
        parse_pattern(text, Span::new(0, text.len()))
            .unwrap_or_else(|error| panic!("pattern parse failed: {}", error.message()))
    }

    fn branch_labels(pattern: &Pattern) -> Vec<&str> {
        pattern
            .elems
            .iter()
            .find_map(|element| match &element.kind {
                PatKind::OneOf { branches, .. } => Some(
                    branches
                        .iter()
                        .map(|(label, _)| label.as_str())
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .expect("no top-level oneof")
    }

    #[test]
    fn oneof_branch_labels_survive_a_bare_ruleref_branch() {
        // `number => number` on its own line must not steal the next
        // branch's label as an implicit bind (§1.4.5 errata).
        let pattern = parse(
            "oneof {\n null => \"null\"\n bool => oneof { t => \"true\" }\n number => number\n string => $str text\n }",
        );
        assert_eq!(
            branch_labels(&pattern),
            vec!["null", "bool", "number", "string"]
        );
    }

    #[test]
    fn qualified_rule_ref_binds_and_reads_context() {
        let pattern = parse("json.value as v");
        match &pattern.elems[0].kind {
            PatKind::RuleRef { path, ctx, bind } => {
                assert_eq!(path, &["json".to_string(), "value".to_string()]);
                assert_eq!(bind.as_deref(), Some("v"));
                assert!(ctx.is_empty());
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn oneof_takes_a_trailing_bind() {
        let pattern = parse("oneof { eq => \"=\", ne => \"!=\" } as op");
        match &pattern.elems[0].kind {
            PatKind::OneOf { branches, bind } => {
                assert_eq!(branches.len(), 2);
                assert_eq!(bind.as_deref(), Some("op"));
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn fragments_parse_with_kinds_and_binds() {
        let pattern = parse("\"def\" $word fname \"(\" each sep \",\" { param } as params \")\"");
        assert_eq!(pattern.elems.len(), 5);
        match &pattern.elems[1].kind {
            PatKind::Fragment { kind, bind, .. } => {
                assert!(matches!(kind, FragKind::Word));
                assert_eq!(bind.as_deref(), Some("fname"));
            }
            other => panic!("unexpected {:?}", other),
        }
        match &pattern.elems[3].kind {
            PatKind::Each { sep, bind, .. } => {
                assert!(sep.is_some());
                assert_eq!(bind.as_deref(), Some("params"));
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn rules_parse_with_context_fields() {
        let (name, context, _pattern, _) = parse_rule_declaration(
            "rule pick(context { selector parent = none }) { oneof { a => \"a\", b => \"b\" } }",
            0,
            Span::new(0, 90),
        )
        .unwrap();
        assert_eq!(name, "pick");
        assert_eq!(context.len(), 1);
        assert_eq!(context[0].name, "parent");
    }

    #[test]
    fn indent_directly_after_eol_is_rejected() {
        // §8.3.5's static check: indent performs its own line advancement,
        // so the pair would double-advance.
        let error = parse_pattern("eol indent { \"x\" }", Span::new(0, 20))
            .expect_err("indent after eol must be rejected");
        assert!(
            error
                .message()
                .contains("`indent` may not directly follow `eol`"),
            "unexpected message: {}",
            error.message()
        );

        // Nested sequences are checked too.
        let error = parse_pattern("( eol indent { \"x\" } )", Span::new(0, 24))
            .expect_err("nested indent after eol must be rejected");
        assert!(
            error
                .message()
                .contains("`indent` may not directly follow `eol`"),
            "unexpected message: {}",
            error.message()
        );

        // An element between them makes it legal.
        assert!(parse_pattern("eol \"x\" indent { \"y\" }", Span::new(0, 24)).is_ok());
    }
}
