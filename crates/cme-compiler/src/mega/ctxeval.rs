//! The ONE `where`-condition evaluator (§8.3.4, plan §1.4.5).
//!
//! The condition language over captures — scalars, `&&`/`||`/`!`,
//! comparisons, `some … in` / `all … in`, `present(…)`, capture paths with
//! trailing accessors (`.matched`, `.length`, `.line`, `.col`, `.span`), and
//! `@`-calls into the §8.5 evaluator — was previously implemented twice,
//! once in the packrat matcher (pattern `where`) and once in the template
//! elaborator (`[when]` guards, `require`, `[each … where]` filters). The
//! copies had already diverged (the template's `.line`/`.col` were hard-
//! coded 0), so this module is the shared evaluator: the matcher and the
//! elaborator each provide a [`CondHost`] that resolves capture paths
//! against its own binding structures and reports source positions, and
//! this module owns the recursion, the operator semantics, and the
//! compile-time-call bridge. Identical expressions now evaluate identically
//! in both positions.

use cme_core::Span;
use cme_core::mega::{Accessor, Capture, CaptureKind, CtxBinOp, CtxExpr, TextKind};

use crate::mega::cteval::CtEngine;

/// A condition value: a scalar or a capture (records and lists stay
/// captures; a bare capture in a scalar position coerces to its number or
/// its trimmed matched text).
#[derive(Debug, Clone, PartialEq)]
pub enum CtxVal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Capture(Capture),
}

/// The condition-evaluation host: one per context that evaluates
/// conditions. The matcher implements it over the match environment's
/// scope; the elaborator implements it over its template scopes.
pub trait CondHost {
    /// Resolves a capture path against current bindings: the head from the
    /// scope, the remaining segments by record-field navigation with the
    /// trailing-accessor fallback (§8.3.4, plan §1.4.4).
    fn lookup(&self, path: &[String]) -> Option<Capture>;

    /// The one-based source line of the capture's start (0 when the
    /// position is not resolvable in this context).
    fn capture_line(&self, capture: &Capture) -> usize;

    /// The one-based source column of the capture's start (0 when the
    /// position is not resolvable in this context).
    fn capture_col(&self, capture: &Capture) -> usize;

    /// Shadows `name` with `value` for one quantifier iteration
    /// (`some x in xs { … }` / `all x in xs { … }`).
    fn push_binding(&mut self, name: &str, value: Capture);

    /// Restores the binding pushed most recently.
    fn pop_binding(&mut self);

    /// Reports a condition-evaluation problem (an empty call path, a failed
    /// `@`-call). Hosts without a diagnostic channel ignore it.
    fn note_failure(&mut self, message: String) {
        let _ = message;
    }

    /// The §8.5 compile-time evaluator, when this context carries one.
    fn engine(&self) -> Option<&CtEngine<'_>> {
        None
    }
}

/// A capture in a scalar position: its number, or its trimmed matched text.
/// This is what makes `close == name` work when both sides are captures
/// (§8.3.4).
pub fn coerce_scalar(value: &CtxVal) -> CtxVal {
    match value {
        CtxVal::Capture(capture) => match &capture.kind {
            CaptureKind::Int(value) => CtxVal::Int(*value),
            CaptureKind::Float(value) => CtxVal::Float(*value),
            _ => CtxVal::Str(capture.matched().trim().to_string()),
        },
        other => other.clone(),
    }
}

/// The operator semantics over condition values: `&&`/`||` are bool-only,
/// `==`/`!=` coerce captures to scalars and compare same-type, and the
/// relational operators are numeric-only (§A.4's strictness carried into
/// the condition language).
pub fn eval_bin(op: CtxBinOp, lhs: &CtxVal, rhs: &CtxVal) -> Option<CtxVal> {
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

/// The value an absent evaluation bridges to: an absent optional, so a
/// failed `@`-argument reads as `none` rather than poisoning the call.
pub fn absent_val() -> CtxVal {
    CtxVal::Capture(Capture {
        kind: CaptureKind::Opt(None),
        matched: String::new(),
        span: Span::missing(0),
    })
}

/// Converts a condition value into a capture for `@`-call arguments in
/// condition positions: scalars keep their kind, `bool` bridges as text.
pub fn ctx_val_to_capture(value: CtxVal) -> Capture {
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
pub fn value_to_ctx_val(value: cme_interp::Value) -> Option<CtxVal> {
    match value {
        cme_interp::Value::Bool(value) => Some(CtxVal::Bool(value)),
        cme_interp::Value::Str(value) => Some(CtxVal::Str(value)),
        cme_interp::Value::Int(value) => Some(CtxVal::Int(value)),
        cme_interp::Value::Float(value) => Some(CtxVal::Float(value)),
        other => match crate::mega::cteval::value_to_capture(&other) {
            Ok(capture) => Some(CtxVal::Capture(capture)),
            Err(_) => None,
        },
    }
}

/// Evaluates a condition, coercing a failed evaluation to an absent
/// capture — the argument semantics of compile-time calls in condition
/// positions.
pub fn eval_or_capture<H: CondHost>(expression: &CtxExpr, host: &mut H) -> Capture {
    ctx_val_to_capture(eval(expression, host).unwrap_or_else(absent_val))
}

/// Evaluates one condition expression against `host` (§8.3.4). `None`
/// means the condition could not be evaluated (an unresolvable capture, a
/// failed `@`-call) — a failing condition, like any element failure.
pub fn eval<H: CondHost>(expression: &CtxExpr, host: &mut H) -> Option<CtxVal> {
    match expression {
        CtxExpr::Str(value) => Some(CtxVal::Str(value.clone())),
        CtxExpr::Int(value) => Some(CtxVal::Int(*value)),
        CtxExpr::Float(value) => Some(CtxVal::Float(*value)),
        CtxExpr::Bool(value) => Some(CtxVal::Bool(*value)),
        CtxExpr::Capture { path, accessor } => {
            let capture = host.lookup(path)?;
            Some(apply_accessor(capture, accessor.as_ref(), host))
        }
        CtxExpr::Bin(op, lhs, rhs) => {
            // §A.5 short-circuiting carries into the condition language:
            // `&&`/`||` evaluate the right side only when it can matter,
            // so a guard may protect a quantifier over an absent list.
            let lhs = eval(lhs, host)?;
            if *op == CtxBinOp::And && lhs == CtxVal::Bool(false)
                || *op == CtxBinOp::Or && lhs == CtxVal::Bool(true)
            {
                return Some(lhs);
            }
            let rhs = eval(rhs, host)?;
            eval_bin(op.clone(), &lhs, &rhs)
        }
        CtxExpr::Not(inner) => match eval(inner, host)? {
            CtxVal::Bool(value) => Some(CtxVal::Bool(!value)),
            _ => None,
        },
        CtxExpr::SomeIn { var, list, cond } => {
            let items = eval_list(list, host)?;
            for item in items {
                host.push_binding(var, item);
                let hit = eval(cond, host) == Some(CtxVal::Bool(true));
                host.pop_binding();
                if hit {
                    return Some(CtxVal::Bool(true));
                }
            }
            Some(CtxVal::Bool(false))
        }
        CtxExpr::AllIn { var, list, cond } => {
            let items = eval_list(list, host)?;
            let mut all = true;
            for item in items {
                host.push_binding(var, item);
                if eval(cond, host) != Some(CtxVal::Bool(true)) {
                    all = false;
                }
                host.pop_binding();
            }
            Some(CtxVal::Bool(all))
        }
        CtxExpr::Present { path } => {
            let present = host
                .lookup(path)
                .map(|capture| capture.is_present())
                .unwrap_or(false);
            Some(CtxVal::Bool(present))
        }
        CtxExpr::Call { path, args } => eval_ct_call(path, args, host),
    }
}

/// A compile-time call in a condition position (§8.5): `@fn(…)` against the
/// file's own pure functions, or a `cm.*` builtin. In `cm.parse` the first
/// argument is a rule path (a capture-shaped path in the condition syntax).
fn eval_ct_call<H: CondHost>(path: &[String], args: &[CtxExpr], host: &mut H) -> Option<CtxVal> {
    if path.is_empty() {
        host.note_failure("empty compile-time call path".to_string());
        return None;
    }
    let is_cm_parse = path.len() == 2 && path[0] == "cm" && path[1] == "parse";
    let mut captures = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if is_cm_parse
            && index == 0
            && let CtxExpr::Capture { path: rule, .. } = arg
        {
            captures.push(Capture {
                kind: CaptureKind::Text(TextKind::Raw),
                matched: rule.join("."),
                span: Span::missing(0),
            });
            continue;
        }
        captures.push(eval_or_capture(arg, host));
    }
    // The engine borrow ends before the failure reporting below.
    let outcome = host.engine().map(|engine| engine.call(path, &captures));
    match outcome {
        Some(Ok(result)) => value_to_ctx_val(result.value),
        Some(Err(message)) => {
            host.note_failure(message);
            None
        }
        None => {
            host.note_failure(format!(
                "compile-time function `{}` needs the §8.5 evaluator",
                path.join(".")
            ));
            None
        }
    }
}

fn eval_list<H: CondHost>(expression: &CtxExpr, host: &mut H) -> Option<Vec<Capture>> {
    match eval(expression, host)? {
        CtxVal::Capture(capture) => match capture.kind {
            CaptureKind::List(items) => Some(items),
            _ => None,
        },
        _ => None,
    }
}

/// Applies one trailing accessor to a capture (§8.3.4, plan §1.4.4).
/// `.matched` trims edge whitespace (indent blocks and skip-run edges
/// would otherwise leak `\n    ` prefixes); `.line`/`.col` come from the
/// host's source-position mapping (one-based; 0 when unresolvable);
/// `.span` is the capture's `start:end` byte-offset token.
pub fn apply_accessor<H: CondHost>(
    capture: Capture,
    accessor: Option<&Accessor>,
    host: &H,
) -> CtxVal {
    match accessor {
        None => CtxVal::Capture(capture),
        Some(Accessor::Matched) => CtxVal::Str(capture.matched().trim().to_string()),
        Some(Accessor::Length) => {
            let length = match capture.kind {
                CaptureKind::List(items) => items.len(),
                CaptureKind::Record { fields, .. } => fields.len(),
                _ => 0,
            };
            CtxVal::Int(length as i64)
        }
        Some(Accessor::Line) => CtxVal::Int(host.capture_line(&capture) as i64),
        Some(Accessor::Col) => CtxVal::Int(host.capture_col(&capture) as i64),
        Some(Accessor::Span) => CtxVal::Str(format!("{}:{}", capture.span.start, capture.span.end)),
    }
}

/// The accessor named by a path segment, if it is one of the keywords.
pub fn accessor_keyword(segment: &str) -> Option<Accessor> {
    match segment {
        "matched" => Some(Accessor::Matched),
        "length" => Some(Accessor::Length),
        "line" => Some(Accessor::Line),
        "col" => Some(Accessor::Col),
        "span" => Some(Accessor::Span),
        _ => None,
    }
}

/// Applies one trailing accessor to a capture in capture form (template
/// splices and hole text resolve through captures, not scalars).
pub fn apply_accessor_to_capture<H: CondHost>(
    capture: &Capture,
    accessor: Option<&Accessor>,
    host: &H,
) -> Option<Capture> {
    match apply_accessor(capture.clone(), accessor, host) {
        CtxVal::Str(value) => Some(Capture {
            kind: CaptureKind::Text(TextKind::Raw),
            matched: value,
            span: capture.span,
        }),
        CtxVal::Int(value) => Some(Capture {
            kind: CaptureKind::Int(value),
            matched: value.to_string(),
            span: capture.span,
        }),
        CtxVal::Float(value) => Some(Capture {
            kind: CaptureKind::Float(value),
            matched: format!("{value}"),
            span: capture.span,
        }),
        CtxVal::Capture(capture) => Some(capture),
        CtxVal::Bool(_) => None,
    }
}

/// Path-navigation fallback: applies an accessor named by the path's final
/// segment (`.matched`, `.length`, `.line`, `.col`, `.span`). A segment
/// that names no accessor resolves to nothing — this is how a missing
/// record field reads as an unresolvable path (`present` → false), never
/// as the whole capture.
pub fn accessor_capture<H: CondHost>(
    capture: &Capture,
    segment: &str,
    host: &H,
) -> Option<Capture> {
    let accessor = accessor_keyword(segment)?;
    apply_accessor_to_capture(capture, Some(&accessor), host)
}

/// Navigates a capture along path segments after the head: record fields
/// first, and a trailing accessor keyword as the fallback (plan §1.4.4 —
/// every capture exposes these). Used by both hosts' `lookup`.
pub fn navigate<H: CondHost>(capture: Capture, segments: &[String], host: &H) -> Option<Capture> {
    let mut current = capture;
    for (index, segment) in segments.iter().enumerate() {
        if let CaptureKind::Opt(Some(inner)) = current.kind {
            current = *inner;
        }
        let last = index + 1 == segments.len();
        match &current.kind {
            CaptureKind::Record { fields, .. } => {
                match fields.iter().find(|(name, _)| name == segment) {
                    Some((_, capture)) => current = capture.clone(),
                    None if last => return accessor_capture(&current, segment, host),
                    None => return None,
                }
            }
            // A non-record capture can only be followed by an accessor
            // keyword in the path's final position.
            _ if last => return accessor_capture(&current, segment, host),
            _ => return None,
        }
    }
    Some(current)
}

/// A compact, human-readable rendering of a condition for failure
/// messages (`constraint failed: close == name`).
pub fn ctx_summary(expression: &CtxExpr) -> String {
    match expression {
        CtxExpr::Capture { path, .. } => path.join("."),
        CtxExpr::Bin(op, lhs, rhs) => {
            let symbol = match op {
                CtxBinOp::Eq => "==",
                CtxBinOp::Ne => "!=",
                CtxBinOp::Lt => "<",
                CtxBinOp::Le => "<=",
                CtxBinOp::Gt => ">",
                CtxBinOp::Ge => ">=",
                CtxBinOp::And => "&&",
                CtxBinOp::Or => "||",
            };
            format!("{} {} {}", ctx_summary(lhs), symbol, ctx_summary(rhs))
        }
        CtxExpr::Not(inner) => format!("!{}", ctx_summary(inner)),
        CtxExpr::Present { path } => format!("present({})", path.join(".")),
        CtxExpr::Str(value) => format!("\"{value}\""),
        CtxExpr::Int(value) => value.to_string(),
        CtxExpr::Float(value) => value.to_string(),
        CtxExpr::Bool(value) => value.to_string(),
        CtxExpr::SomeIn { .. } | CtxExpr::AllIn { .. } => "quantified condition".to_string(),
        CtxExpr::Call { path, .. } => format!("{}(…)", path.join(".")),
    }
}
