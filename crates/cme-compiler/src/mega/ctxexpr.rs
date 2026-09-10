//! The compile-time expression language shared by `where` conditions,
//! `context` bindings, and template conditions (WHITEPAPER §8.3.4).
//!
//! Conditions may use equality and comparison, `&&`/`||`/`!` (Appendix A
//! rules: comparisons are non-associative, mixing `&&` with `||` requires
//! parentheses), `some x in xs { … }` / `all x in xs { … }`, `present(x)`,
//! capture accessors (`.matched`, `.line`, `.col`, `.length`), and calls to
//! pure compile-time functions (§8.5; unsupported until that task lands).

use crate::diagnostics::Diagnostic;
use crate::mega::profile::{parse_char_literal, parse_string_literal, skip_ws_and_comments};
use cme_core::Span;
use cme_core::magic::{Accessor, CtxBinOp, CtxExpr};

/// Parses one condition expression from `text` starting at `cursor`.
/// Returns the expression and the cursor just past it. `span` anchors
/// diagnostics inside the enclosing region (pattern parens or template).
pub fn parse_expr(text: &str, cursor: &mut usize, span: Span) -> Result<CtxExpr, Diagnostic> {
    let expression = parse_or(text, cursor, span)?;
    Ok(expression)
}

fn peek_word(text: &str, cursor: usize) -> &str {
    let rest = &text[cursor..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    &rest[..end]
}

fn skip_trivia(text: &str, cursor: &mut usize) {
    skip_ws_and_comments(
        text,
        cursor,
        &crate::mega::profile::checkmate_scan_profile(),
    );
}

fn parse_or(text: &str, cursor: &mut usize, span: Span) -> Result<CtxExpr, Diagnostic> {
    let mut lhs = parse_and(text, cursor, span)?;
    loop {
        skip_trivia(text, cursor);
        if text[*cursor..].starts_with("||") {
            *cursor += 2;
            let rhs = parse_and(text, cursor, span)?;
            lhs = CtxExpr::Bin(CtxBinOp::Or, Box::new(lhs), Box::new(rhs));
        } else {
            return Ok(lhs);
        }
    }
}

fn parse_and(text: &str, cursor: &mut usize, span: Span) -> Result<CtxExpr, Diagnostic> {
    let mut lhs = parse_comparison(text, cursor, span)?;
    loop {
        skip_trivia(text, cursor);
        if text[*cursor..].starts_with("&&") {
            *cursor += 2;
            let rhs = parse_comparison(text, cursor, span)?;
            lhs = CtxExpr::Bin(CtxBinOp::And, Box::new(lhs), Box::new(rhs));
        } else {
            return Ok(lhs);
        }
    }
}

fn parse_comparison(text: &str, cursor: &mut usize, span: Span) -> Result<CtxExpr, Diagnostic> {
    let lhs = parse_unary(text, cursor, span)?;
    skip_trivia(text, cursor);
    let op = if text[*cursor..].starts_with("==") {
        Some((CtxBinOp::Eq, 2))
    } else if text[*cursor..].starts_with("!=") {
        Some((CtxBinOp::Ne, 2))
    } else if text[*cursor..].starts_with("<=") {
        Some((CtxBinOp::Le, 2))
    } else if text[*cursor..].starts_with(">=") {
        Some((CtxBinOp::Ge, 2))
    } else if text[*cursor..].starts_with('<') {
        Some((CtxBinOp::Lt, 1))
    } else if text[*cursor..].starts_with('>') {
        Some((CtxBinOp::Gt, 1))
    } else {
        None
    };
    let Some((op, width)) = op else {
        return Ok(lhs);
    };
    *cursor += width;
    let rhs = parse_unary(text, cursor, span)?;
    // Comparisons are non-associative (§A.3): a second operator at the same
    // level is a compile-time error unless the user parenthesized it.
    skip_trivia(text, cursor);
    if text[*cursor..].starts_with("==")
        || text[*cursor..].starts_with("!=")
        || text[*cursor..].starts_with("<=")
        || text[*cursor..].starts_with(">=")
        || text[*cursor..].starts_with('<')
        || text[*cursor..].starts_with('>')
    {
        return Err(Diagnostic::parse(
            "chained comparison; write `a < b && b < c` (§A.3)",
            offset(span, *cursor),
        ));
    }
    Ok(CtxExpr::Bin(op, Box::new(lhs), Box::new(rhs)))
}

fn parse_unary(text: &str, cursor: &mut usize, span: Span) -> Result<CtxExpr, Diagnostic> {
    skip_trivia(text, cursor);
    if text[*cursor..].starts_with('!') && !text[*cursor..].starts_with("!=") {
        *cursor += 1;
        let inner = parse_unary(text, cursor, span)?;
        return Ok(CtxExpr::Not(Box::new(inner)));
    }
    parse_primary(text, cursor, span)
}

fn parse_primary(text: &str, cursor: &mut usize, span: Span) -> Result<CtxExpr, Diagnostic> {
    skip_trivia(text, cursor);
    let rest = &text[*cursor..];

    // `@fn(…)` — a pure compile-time function call (§8.5). The `@` marks
    // the call; the call itself is parsed as a path + arguments below.
    if rest.starts_with('@') {
        *cursor += 1;
    }
    // `$cap` — the `$` of a capture path is optional in condition positions
    // (plan §1.4.5), but it must not look like "no expression" here.
    if rest.starts_with('$') {
        *cursor += 1;
    }

    if rest.starts_with('(') {
        *cursor += 1;
        let inner = parse_or(text, cursor, span)?;
        skip_trivia(text, cursor);
        if !text[*cursor..].starts_with(')') {
            return Err(Diagnostic::parse(
                "expected `)` to close the condition group",
                offset(span, *cursor),
            ));
        }
        *cursor += 1;
        return Ok(inner);
    }

    if let Some((value, len)) = parse_string_literal(text, *cursor) {
        *cursor += len;
        return Ok(CtxExpr::Str(value));
    }
    if let Some((value, len)) = parse_char_literal(text, *cursor) {
        *cursor += len;
        return Ok(CtxExpr::Str(value.to_string()));
    }

    // Numeric literals.
    let digits = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>();
    if !digits.is_empty() && digits.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        if let Ok(int) = digits.parse::<i64>() {
            *cursor += digits.len();
            return Ok(CtxExpr::Int(int));
        }
        if let Ok(float) = digits.parse::<f64>() {
            *cursor += digits.len();
            return Ok(CtxExpr::Float(float));
        }
    }

    let word = peek_word(text, *cursor);
    if word.is_empty() {
        return Err(Diagnostic::parse(
            "expected a condition expression",
            offset(span, *cursor),
        ));
    }

    match word {
        "true" => {
            *cursor += 4;
            return Ok(CtxExpr::Bool(true));
        }
        "false" => {
            *cursor += 5;
            return Ok(CtxExpr::Bool(false));
        }
        "present" => {
            *cursor += word.len();
            skip_trivia(text, cursor);
            if !text[*cursor..].starts_with('(') {
                return Err(Diagnostic::parse(
                    "expected `(` after `present`",
                    offset(span, *cursor),
                ));
            }
            *cursor += 1;
            let path = parse_path(text, cursor, span)?;
            skip_trivia(text, cursor);
            if !text[*cursor..].starts_with(')') {
                return Err(Diagnostic::parse(
                    "expected `)` to close `present(…)`",
                    offset(span, *cursor),
                ));
            }
            *cursor += 1;
            return Ok(CtxExpr::Present { path });
        }
        "some" | "all" => {
            let quantifier = word.to_string();
            *cursor += word.len();
            skip_trivia(text, cursor);
            let var = peek_word(text, *cursor).to_string();
            if var.is_empty() {
                return Err(Diagnostic::parse(
                    format!("expected a variable name after `{quantifier}`"),
                    offset(span, *cursor),
                ));
            }
            *cursor += var.len();
            skip_trivia(text, cursor);
            if !text[*cursor..].starts_with("in") {
                return Err(Diagnostic::parse(
                    format!("expected `in` in `{quantifier} … in …`"),
                    offset(span, *cursor),
                ));
            }
            *cursor += 2;
            skip_trivia(text, cursor);
            let list = parse_path_with_accessors(text, cursor, span)?;
            skip_trivia(text, cursor);
            if !text[*cursor..].starts_with('{') {
                return Err(Diagnostic::parse(
                    format!("expected `{{` to open the `{quantifier}` body"),
                    offset(span, *cursor),
                ));
            }
            *cursor += 1;
            let cond = parse_or(text, cursor, span)?;
            skip_trivia(text, cursor);
            if !text[*cursor..].starts_with('}') {
                return Err(Diagnostic::parse(
                    format!("expected `}}` to close the `{quantifier}` body"),
                    offset(span, *cursor),
                ));
            }
            *cursor += 1;
            let list = Box::new(list);
            let cond = Box::new(cond);
            return if quantifier == "some" {
                Ok(CtxExpr::SomeIn { var, list, cond })
            } else {
                Ok(CtxExpr::AllIn { var, list, cond })
            };
        }
        _ => {}
    }

    // A capture path or a pure compile-time function call (§8.5).
    let path = parse_path(text, cursor, span)?;
    skip_trivia(text, cursor);
    if text[*cursor..].starts_with('(') {
        *cursor += 1;
        let mut args = Vec::new();
        loop {
            skip_trivia(text, cursor);
            if text[*cursor..].starts_with(')') {
                *cursor += 1;
                break;
            }
            args.push(parse_or(text, cursor, span)?);
            skip_trivia(text, cursor);
            if text[*cursor..].starts_with(',') {
                *cursor += 1;
            }
        }
        return Ok(CtxExpr::Call { path, args });
    }
    Ok(CtxExpr::Capture {
        path,
        accessor: None,
    })
}

/// A dotted capture path with optional trailing accessors
/// (`item.val.matched`, `fields.length`, `x.line`).
pub fn parse_path_with_accessors(
    text: &str,
    cursor: &mut usize,
    span: Span,
) -> Result<CtxExpr, Diagnostic> {
    let path = parse_path(text, cursor, span)?;
    let mut accessor = None;
    loop {
        skip_trivia(text, cursor);
        if !text[*cursor..].starts_with('.') {
            break;
        }
        let dot = *cursor;
        *cursor += 1;
        let word = peek_word(text, *cursor);
        let found = match word {
            "matched" => Some(Accessor::Matched),
            "line" => Some(Accessor::Line),
            "col" => Some(Accessor::Col),
            "length" => Some(Accessor::Length),
            "span" => Some(Accessor::Span),
            _ => None,
        };
        match found {
            Some(found) => {
                accessor = Some(found);
                *cursor += word.len();
            }
            None => {
                return Err(Diagnostic::parse(
                    "unknown accessor (expected `matched`, `line`, `col`, `length`, or `span`)",
                    offset(span, dot),
                ));
            }
        }
    }
    Ok(CtxExpr::Capture { path, accessor })
}

fn parse_path(text: &str, cursor: &mut usize, span: Span) -> Result<Vec<String>, Diagnostic> {
    // The `$` of `$cap.path` is optional in condition positions (the
    // whitepaper's own examples spell it both ways).
    if text[*cursor..].starts_with('$') {
        *cursor += 1;
    }
    let mut segments = Vec::new();
    loop {
        let word = peek_word(text, *cursor);
        if word.is_empty() || !word.chars().next().unwrap().is_ascii_alphabetic() && word != "_" {
            break;
        }
        segments.push(word.to_string());
        *cursor += word.len();
        if text[*cursor..].starts_with('.') {
            *cursor += 1;
            continue;
        }
        break;
    }
    if segments.is_empty() {
        return Err(Diagnostic::parse(
            "expected a capture path",
            offset(span, *cursor),
        ));
    }
    Ok(segments)
}

fn offset(span: Span, at: usize) -> Span {
    let start = (span.start + at).min(span.end);
    Span::new(start, start)
}
