//! Compile-time computation (WHITEPAPER §8.5): the `@`-call evaluator that
//! runs pure Checkmate functions during expansion, plus the `cm.*` parsing
//! and code-builder API.
//!
//! Architecture (text-level, plan §1.4.1): the whitepaper runs `@`-calls in
//! the sandboxed bytecode interpreter over an AST of `code` fragments. This
//! implementation expands to SOURCE TEXT, so a `code` value here is a text
//! fragment (its payload is a `str`); a function whose declared return type
//! is `code` splices its result raw, while every other result is rendered as
//! a Checkmate literal (quoted strings, bracketed arrays, `Name.Variant(…)`
//! enum constructions). The checker accepts `code` as an alias of `str` —
//! the type exists only during compilation and degenerates to its textual
//! payload at runtime.
//!
//! Bridges:
//! * captures → interpreter values, **per the function's declared parameter
//!   types** (§8.5: "functions receive capture values — records, lists,
//!   texts, numbers, spans"). A parameter declared `int`/`float`/`str`/
//!   `bool` receives the scalar; a parameter declared as the `Capture` enum
//!   a megaprogram file declares receives the full tree:
//!   `enum Capture { Text(str), Int(int), Float(float), List(Capture[]),
//!   Rec(str, map<str, Capture>), Absent() }` — magic.cm declares exactly
//!   this shape, and any file whose templates pass records to `@`-functions
//!   must declare it; the bridge constructs the enum by name.
//! * interpreter values → captures, for `let` bindings and template
//!   `match` over `@`-results. A foreign enum (e.g. a `JsonTree`) bridges
//!   back as raw text rendered in its Checkmate source form.
//!
//! Purity (§8.7.4): compile-time code may only see the single file's own
//! declarations — there is no import system yet, so the import check is
//! trivially satisfied (plan Task 7). Evaluation is metered by an operation
//! count (each `@`-call consumes one unit of [`CT_FUEL`]), deterministic
//! and wall-clock free (§5.5), and the interpreter's own call-depth cap
//! bounds recursion.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use cme_core::Span;
use cme_core::ast::{PrimitiveType, Stmt, StmtKind, Type};
use cme_core::magic::{Capture, CaptureKind, PatKind, Pattern, TextKind};
use cme_interp::{Interpreter, Value};

use crate::mega::matcher::{GrammarSet, MatchRegion, match_entry};
use crate::mega::scan::MagicScan;
use crate::mega::template::escape_checkmate;

/// The compile-time fuel budget (§5.5): one unit per `@`-call / `cm.*`
/// invocation. An operation attempted at zero remaining budget is a
/// deterministic budget error — never wall-clock time.
const CT_FUEL: u64 = 100_000;

/// The enum name the bridge materializes capture values as. A megaprogram
/// file using `@`-functions declares `enum Capture { … }` with the variant
/// set documented in the module header.
const CAPTURE_TYPE: &str = "Capture";

/// One compile-time call result: the interpreter's value plus whether the
/// function's declared return type was `code` (splice raw) or a plain value
/// (render as a Checkmate literal).
#[derive(Debug, Clone)]
pub struct CtResult {
    pub value: Value,
    pub is_code: bool,
}

/// The compile-time evaluator for one expansion run.
///
/// Owns the host program: the file's Checkmate statements with every magic
/// construct (grammar declarations, magic declarations, invocation sites)
/// blanked out — the `@`-functions, their helper types, and everything they
/// reference parse from the remainder. Tolerant parsing keeps unrelated
/// blanks as `Invalid` nodes; only the functions actually invoked must be
/// intact, and an `Invalid` node reached at runtime surfaces as a clean
/// interpreter error, never a panic.
pub struct CtEngine<'g> {
    program: Vec<Stmt>,
    /// Top-level functions whose declared return type is `code`.
    code_returning: HashSet<String>,
    /// Declared parameter kinds per top-level function, driving the
    /// capture → value bridge at each call.
    param_kinds: HashMap<String, Vec<ParamKind>>,
    set: &'g GrammarSet,
    fuel: Cell<u64>,
}

/// How a function's declared parameter type receives a capture (§8.5).
#[derive(Debug, Clone, Copy, PartialEq)]
enum ParamKind {
    Int,
    Float,
    Str,
    Bool,
    /// The full capture tree, materialized as the `Capture` enum.
    CaptureTree,
}

impl<'g> CtEngine<'g> {
    /// Builds the engine from the file being expanded. `scan` must be the
    /// scan of `source` (its spans drive the blanking); `set` is the
    /// compiled grammar set used by late delegation (`cm.parse`).
    pub fn new(source: &str, scan: &MagicScan, set: &'g GrammarSet) -> Self {
        let mut blanked = source.to_string();
        let mut spans: Vec<Span> = Vec::new();
        for grammar in &scan.grammars {
            spans.push(grammar.span);
        }
        for magic in &scan.magics {
            spans.push(magic.span);
        }
        for invocation in &scan.invocations {
            spans.push(invocation.span);
        }
        // Apply highest-position first so earlier replacements do not shift
        // later spans.
        spans.sort_by_key(|span| std::cmp::Reverse(span.start));
        for span in spans {
            let masked: String = blanked[span.start..span.end]
                .chars()
                .map(|c| if c == '\n' { '\n' } else { ' ' })
                .collect();
            blanked.replace_range(span.start..span.end, &masked);
        }
        let outcome = crate::parse_source(&blanked);
        let mut code_returning = HashSet::new();
        let mut param_kinds: HashMap<String, Vec<ParamKind>> = HashMap::new();
        for statement in &outcome.statements {
            if let StmtKind::FuncDecl {
                name,
                return_ty,
                params,
                ..
            } = &statement.kind
            {
                if is_code_type(return_ty) {
                    code_returning.insert(name.clone());
                }
                param_kinds.insert(
                    name.clone(),
                    params.iter().map(|param| param_kind(&param.ty)).collect(),
                );
            }
        }
        Self {
            program: outcome.statements,
            code_returning,
            param_kinds,
            set,
            fuel: Cell::new(CT_FUEL),
        }
    }

    /// Evaluates one template `@`-call / `cm.*`-call. `path` comes from the
    /// parsed call node; `args` are the resolved argument captures.
    /// Resolution rules:
    /// * `cm.…` — the §8.5 builtin namespace;
    /// * otherwise a top-level function of the file, named by the path's
    ///   last segment (single-file megaprograms have no imports, so the
    ///   whitepaper's `std.`-style prefixes carry no meaning yet; spelling
    ///   `@py.emitBody` resolves to `fn emitBody`).
    pub fn call(&self, path: &[String], args: &[Capture]) -> Result<CtResult, String> {
        if path.is_empty() {
            return Err("empty compile-time call path".to_string());
        }
        if path.first().map(String::as_str) == Some("cm") {
            return self.call_builtin(path, args);
        }
        self.call_user(path, args)
    }

    /// A user function: fuel, bridge per declared parameter types, invoke,
    /// tag by declared return type.
    fn call_user(&self, path: &[String], args: &[Capture]) -> Result<CtResult, String> {
        self.spend_fuel(path)?;
        let name = path.last().unwrap().as_str();
        let Some(kinds) = self.param_kinds.get(name).map(Vec::as_slice) else {
            return Err(format!(
                "unknown compile-time function `{}` (declare a top-level function `{name}`; \
                 the std-library prefixes arrive with imports, plan §1.4.3)",
                path.join(".")
            ));
        };
        let is_code = self.code_returning.contains(name);
        if args.len() != kinds.len() {
            return Err(format!(
                "compile-time call `{}` expects {} argument(s), got {}",
                path.join("."),
                kinds.len(),
                args.len()
            ));
        }
        let mut values = Vec::new();
        for (capture, kind) in args.iter().zip(kinds) {
            values.push(bridge_capture(capture, *kind).map_err(|message| {
                format!(
                    "compile-time call `{}` argument: {}",
                    path.join("."),
                    message
                )
            })?);
        }
        let interpreter = Interpreter::new(&self.program);
        let value = interpreter.invoke(name, &values).map_err(|error| {
            format!(
                "compile-time call `{}` failed: {}",
                path.join("."),
                error.message
            )
        })?;
        Ok(CtResult { value, is_code })
    }

    /// The `cm` namespace (§8.5): the parsing API and the code builders.
    fn call_builtin(&self, path: &[String], args: &[Capture]) -> Result<CtResult, String> {
        self.spend_fuel(path)?;
        let names: Vec<&str> = path.iter().map(String::as_str).collect();
        match names.as_slice() {
            ["cm", "parseExpr"] => {
                let text = str_arg(path, args, 0)?;
                crate::parser::parse_expr_text(text)
                    .map_err(|message| format!("cm.parseExpr: {message}"))?;
                Ok(CtResult {
                    value: Value::Str(text.to_string()),
                    is_code: true,
                })
            }
            ["cm", "parseStmts"] => {
                let text = str_arg(path, args, 0)?;
                crate::parser::parse_stmts_text(text)
                    .map_err(|message| format!("cm.parseStmts: {message}"))?;
                Ok(CtResult {
                    value: Value::Str(text.to_string()),
                    is_code: true,
                })
            }
            ["cm", "parse"] => {
                let rule = str_arg(path, args, 0)?;
                let text = str_arg(path, args, 1)?;
                self.parse_by_rule(rule, text)
            }
            ["cm", "code", "str"] => {
                let text = str_arg(path, args, 0)?;
                Ok(CtResult {
                    value: Value::Str(format!("\"{}\"", escape_checkmate(text))),
                    is_code: true,
                })
            }
            ["cm", "code", "call"] => {
                let name = str_arg(path, args, 0)?;
                let mut parts = Vec::new();
                for (index, arg) in args.iter().enumerate().skip(1) {
                    parts.push(code_arg_text(path, index, arg)?);
                }
                Ok(CtResult {
                    value: Value::Str(format!("{}({})", name, parts.join(", "))),
                    is_code: true,
                })
            }
            _ => Err(format!(
                "unknown `cm` builtin `{}` (available: cm.parseExpr, cm.parseStmts, cm.parse, cm.code.str, cm.code.call)",
                path.join(".")
            )),
        }
    }

    /// `cm.parse(grammar.rule, text)` — late delegation (§8.3.8): the rule
    /// runs over `text` as a whole region and the resulting capture bridges
    /// back as a `Capture` value (records included, so templates can
    /// `match` on the delegated grammar's `oneof` tags).
    fn parse_by_rule(&self, rule: &str, text: &str) -> Result<CtResult, String> {
        let segments: Vec<&str> = rule.split('.').collect();
        let (grammar_index, display) = match segments.as_slice() {
            [grammar, rule_name] => {
                let index = self
                    .set
                    .grammars
                    .iter()
                    .position(|candidate| &candidate.name == grammar)
                    .ok_or_else(|| format!("cm.parse: unknown grammar `{grammar}`"))?;
                (index, format!("{grammar}.{rule_name}"))
            }
            [rule_name] => {
                let found = self
                    .set
                    .grammars
                    .iter()
                    .position(|candidate| candidate.rules.iter().any(|r| r.name == *rule_name))
                    .ok_or_else(|| format!("cm.parse: unknown rule `{rule_name}`"))?;
                (found, rule_name.to_string())
            }
            _ => {
                return Err(format!(
                    "cm.parse: the rule path must be `grammar.rule` or `rule`, got `{rule}`"
                ));
            }
        };
        let canonical: Vec<String> = match segments.as_slice() {
            [grammar, rule_name] => vec![grammar.to_string(), rule_name.to_string()],
            [rule_name] => {
                vec![
                    self.set.grammars[grammar_index].name.clone(),
                    rule_name.to_string(),
                ]
            }
            _ => unreachable!(),
        };
        let pattern = Pattern::single(
            PatKind::RuleRef {
                path: canonical,
                ctx: Vec::new(),
                bind: Some("root".to_string()),
            },
            Span::missing(0),
        );
        let region = MatchRegion::new(text, 0, text);
        let root = match_entry(self.set, grammar_index, &pattern, &region, Some(self)).map_err(
            |failure| {
                format!(
                    "cm.parse: the text does not match `{display}`: {}",
                    failure.message
                )
            },
        )?;
        Ok(CtResult {
            value: capture_to_capture_value(&root),
            is_code: false,
        })
    }

    /// One deterministic fuel unit per call (§5.5).
    fn spend_fuel(&self, path: &[String]) -> Result<(), String> {
        let remaining = self.fuel.get();
        if remaining == 0 {
            return Err(format!(
                "compile-time fuel budget exhausted at `{}` (§5.5: \
                 compile-time evaluation is metered by operation count)",
                path.join(".")
            ));
        }
        self.fuel.set(remaining - 1);
        Ok(())
    }
}

/// Whether a declared return type is the compile-time `code` type. `code`
/// is a lowercase builtin-style name (like `int`/`str`); the checker treats
/// it as `str` (see check.rs), and the engine tags results by it.
fn is_code_type(ty: &Type) -> bool {
    matches!(ty, Type::Named { name, args } if name == "code" && args.is_empty())
}

/// Classifies a declared parameter type for the capture bridge.
fn param_kind(ty: &Type) -> ParamKind {
    match ty {
        Type::Prim(PrimitiveType::Int) => ParamKind::Int,
        Type::Prim(PrimitiveType::Float) => ParamKind::Float,
        Type::Prim(PrimitiveType::Str) => ParamKind::Str,
        Type::Prim(PrimitiveType::Bool) => ParamKind::Bool,
        // `code` parameters are text (a code value IS its text).
        Type::Named { name, .. } if name == "code" => ParamKind::Str,
        _ => ParamKind::CaptureTree,
    }
}

/// Reads a builtin argument as text: the capture's matched text (a string
/// capture contributes its unquoted content; a rule-path argument arrives
/// as raw text).
fn str_arg<'a>(path: &[String], args: &'a [Capture], index: usize) -> Result<&'a str, String> {
    match args.get(index) {
        Some(capture) => Ok(capture.matched.as_str()),
        None => Err(format!(
            "`{}` is missing argument {}",
            path.join("."),
            index + 1
        )),
    }
}

/// Renders one `cm.code.call` argument as code text: string-capture values
/// render as quoted literals, raw text splices verbatim (it IS code), and
/// scalars render as literals.
fn code_arg_text(path: &[String], index: usize, arg: &Capture) -> Result<String, String> {
    match &arg.kind {
        CaptureKind::Text(kind) => match kind {
            TextKind::Str => Ok(format!("\"{}\"", escape_checkmate(&arg.matched))),
            _ => Ok(arg.matched.clone()),
        },
        CaptureKind::Int(value) => Ok(value.to_string()),
        CaptureKind::Float(value) => Ok(format!("{value}")),
        _ => Err(format!(
            "`{}` argument {} cannot be a code argument (pass text or a number)",
            path.join("."),
            index + 1
        )),
    }
}

// ---------------------------------------------------------------------------
// Bridges: captures ↔ interpreter values
// ---------------------------------------------------------------------------

/// Bridges one capture into an interpreter value per the declared
/// parameter kind (§8.5: functions receive capture values).
fn bridge_capture(capture: &Capture, kind: ParamKind) -> Result<Value, String> {
    match kind {
        ParamKind::Int => match &capture.kind {
            CaptureKind::Int(value) => Ok(Value::Int(*value)),
            CaptureKind::Text(_) => capture
                .matched
                .trim()
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| format!("expected an int, got `{}`", capture.matched.trim())),
            other => Err(format!(
                "expected an int capture, got {}",
                capture_kind_name(other)
            )),
        },
        ParamKind::Float => match &capture.kind {
            CaptureKind::Float(value) => Ok(Value::Float(*value)),
            CaptureKind::Int(value) => Ok(Value::Float(*value as f64)),
            CaptureKind::Text(_) => capture
                .matched
                .trim()
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| format!("expected a float, got `{}`", capture.matched.trim())),
            other => Err(format!(
                "expected a float capture, got {}",
                capture_kind_name(other)
            )),
        },
        ParamKind::Str => match &capture.kind {
            CaptureKind::Text(_) => Ok(Value::Str(capture.matched.clone())),
            CaptureKind::Int(value) => Ok(Value::Str(value.to_string())),
            CaptureKind::Float(value) => Ok(Value::Str(format!("{value}"))),
            other => Err(format!(
                "expected a text capture, got {}",
                capture_kind_name(other)
            )),
        },
        ParamKind::Bool => match &capture.kind {
            CaptureKind::Text(_) => match capture.matched.trim() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                other => Err(format!("expected a bool, got `{other}`")),
            },
            other => Err(format!(
                "expected a bool capture, got {}",
                capture_kind_name(other)
            )),
        },
        // The full tree, materialized as the `Capture` enum.
        ParamKind::CaptureTree => Ok(capture_to_capture_value(capture)),
    }
}

fn capture_kind_name(kind: &CaptureKind) -> &'static str {
    match kind {
        CaptureKind::Text(_) => "text",
        CaptureKind::Int(_) => "an int",
        CaptureKind::Float(_) => "a float",
        CaptureKind::List(_) => "a list",
        CaptureKind::Record { .. } => "a record",
        CaptureKind::Opt(_) => "an optional",
    }
}

/// Materializes a capture as a `Capture` enum value (the shape documented in
/// the module header). Records become `Rec(tag, map<str, Capture>)`; an
/// absent optional becomes `Absent()`; a string capture's payload is its
/// unquoted content.
fn capture_to_capture_value(capture: &Capture) -> Value {
    match &capture.kind {
        CaptureKind::Text(_) => Value::Enum {
            name: CAPTURE_TYPE.to_string(),
            variant: "Text".to_string(),
            payload: vec![Value::Str(capture.matched.clone())],
        },
        CaptureKind::Int(value) => Value::Enum {
            name: CAPTURE_TYPE.to_string(),
            variant: "Int".to_string(),
            payload: vec![Value::Int(*value)],
        },
        CaptureKind::Float(value) => Value::Enum {
            name: CAPTURE_TYPE.to_string(),
            variant: "Float".to_string(),
            payload: vec![Value::Float(*value)],
        },
        CaptureKind::List(items) => Value::Enum {
            name: CAPTURE_TYPE.to_string(),
            variant: "List".to_string(),
            payload: vec![Value::Array(
                items.iter().map(capture_to_capture_value).collect(),
            )],
        },
        CaptureKind::Record { tag, fields } => Value::Enum {
            name: CAPTURE_TYPE.to_string(),
            variant: "Rec".to_string(),
            payload: vec![
                Value::Str(tag.clone()),
                Value::Map(
                    fields
                        .iter()
                        .map(|(name, value)| {
                            (Value::Str(name.clone()), capture_to_capture_value(value))
                        })
                        .collect(),
                ),
            ],
        },
        CaptureKind::Opt(Some(inner)) => capture_to_capture_value(inner),
        CaptureKind::Opt(None) => Value::Enum {
            name: CAPTURE_TYPE.to_string(),
            variant: "Absent".to_string(),
            payload: Vec::new(),
        },
    }
}

/// Bridges an interpreter result back into a capture, for template `let`
/// bindings, `match` dispatch over `@`-results, and `when` conditions.
/// A foreign enum/struct (e.g. a `jsonTree` built by `@toValue`) bridges as
/// raw text in its Checkmate source form — splicing it emits exactly that
/// text.
pub fn value_to_capture(value: &Value) -> Result<Capture, String> {
    let span = Span::missing(0);
    match value {
        Value::Str(text) => Ok(Capture {
            kind: CaptureKind::Text(TextKind::Raw),
            matched: text.clone(),
            span,
        }),
        Value::Int(value) => Ok(Capture {
            kind: CaptureKind::Int(*value),
            matched: value.to_string(),
            span,
        }),
        Value::Float(value) => Ok(Capture {
            kind: CaptureKind::Float(*value),
            matched: format!("{value}"),
            span,
        }),
        Value::Bool(value) => Ok(Capture {
            kind: CaptureKind::Text(TextKind::Raw),
            matched: value.to_string(),
            span,
        }),
        Value::Array(items) => {
            let mut captures = Vec::new();
            for item in items {
                captures.push(value_to_capture(item)?);
            }
            Ok(Capture {
                kind: CaptureKind::List(captures),
                matched: String::new(),
                span,
            })
        }
        Value::Enum {
            name,
            variant,
            payload,
        } if name == CAPTURE_TYPE => match (variant.as_str(), payload.as_slice()) {
            ("Text", [Value::Str(text)]) => Ok(Capture {
                kind: CaptureKind::Text(TextKind::Raw),
                matched: text.clone(),
                span,
            }),
            ("Int", [Value::Int(value)]) => Ok(Capture {
                kind: CaptureKind::Int(*value),
                matched: value.to_string(),
                span,
            }),
            ("Float", [Value::Float(value)]) => Ok(Capture {
                kind: CaptureKind::Float(*value),
                matched: format!("{value}"),
                span,
            }),
            ("List", [Value::Array(items)]) => {
                let mut captures = Vec::new();
                for item in items {
                    captures.push(value_to_capture(item)?);
                }
                Ok(Capture {
                    kind: CaptureKind::List(captures),
                    matched: String::new(),
                    span,
                })
            }
            ("Rec", [Value::Str(tag), Value::Map(entries)]) => {
                let mut fields = Vec::new();
                for (key, value) in entries {
                    let Value::Str(name) = key else {
                        return Err("Capture.Rec map keys must be strings".to_string());
                    };
                    fields.push((name.clone(), value_to_capture(value)?));
                }
                Ok(Capture {
                    kind: CaptureKind::Record {
                        tag: tag.clone(),
                        fields,
                    },
                    matched: String::new(),
                    span,
                })
            }
            ("Absent", []) => Ok(Capture {
                kind: CaptureKind::Opt(None),
                matched: String::new(),
                span,
            }),
            _ => Err(format!(
                "malformed `Capture.{variant}` payload from a compile-time call"
            )),
        },
        // A foreign value: render its Checkmate source form and splice that.
        Value::Enum { .. } | Value::Struct { .. } => Ok(Capture {
            kind: CaptureKind::Text(TextKind::Raw),
            matched: render_value(value)?,
            span,
        }),
        Value::Map(_) => Err(
            "a map result cannot splice directly; wrap it in a declared enum or \
             build code with cm.code.*"
                .to_string(),
        ),
        Value::Void => Err("a void result cannot splice".to_string()),
    }
}

/// Renders a value as Checkmate source text (the splice form of a plain —
/// non-`code` — result). Strings are quoted and escaped; enums render as
/// `Name.Variant(payloads)`; structs as named-argument constructions; map
/// entries are newline-separated (plan §1.4.7's map-entry convention).
pub fn render_value(value: &Value) -> Result<String, String> {
    match value {
        Value::Str(text) => Ok(format!("\"{}\"", escape_checkmate(text))),
        Value::Int(value) => Ok(value.to_string()),
        Value::Float(value) => Ok(format!("{value}")),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                parts.push(render_value(item)?);
            }
            Ok(format!("[{}]", parts.join(", ")))
        }
        Value::Map(entries) => {
            if entries.is_empty() {
                return Ok("{}".to_string());
            }
            let mut lines = Vec::new();
            for (key, value) in entries {
                lines.push(format!("{}: {}", render_value(key)?, render_value(value)?));
            }
            Ok(format!("{{\n{}\n}}", lines.join("\n")))
        }
        Value::Enum {
            name,
            variant,
            payload,
        } => {
            let mut parts = Vec::new();
            for item in payload {
                parts.push(render_value(item)?);
            }
            Ok(format!("{name}.{variant}({})", parts.join(", ")))
        }
        Value::Struct { name, fields } => {
            let mut parts = Vec::new();
            for (field, value) in fields {
                parts.push(format!("{}: {}", field, render_value(value)?));
            }
            Ok(format!("{name}({})", parts.join(", ")))
        }
        Value::Void => Err("a void result cannot render as code".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use crate::mega::expand::expand_source;

    const DOUBLE_PROGRAM: &str = r#"
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]
    rule value {
        oneof {
            number => $int n
            string => $str s
        }
    }
}

int twice(int x) {
    return x * 2
}

magic jsonNum(json.value as v) {
    @twice($v.n)
}

magic jsonStr(json.value as v) {
    @twiceText($v.s)
}

code twiceText(str s) {
    return s + s
}

int main() {
    return magic(jsonNum) { 42 }
}
"#;

    #[test]
    fn user_functions_receive_captures_and_splice_results() {
        let outcome = expand_source(DOUBLE_PROGRAM).expect("expansion succeeds");
        // `@twice($v.n)` receives Int(42) and returns Int(84); the plain
        // result renders as the literal `84`.
        assert!(
            outcome.expanded.contains("return 84"),
            "{}",
            outcome.expanded
        );
    }

    #[test]
    fn code_returning_functions_splice_raw() {
        let source = DOUBLE_PROGRAM.replace(
            "return magic(jsonNum) { 42 }",
            "str joined = magic(jsonStr) { \"ab\" }\n    return 0",
        );
        let outcome = expand_source(&source).expect("expansion succeeds");
        // `code` splices raw: `abab`, unquoted.
        assert!(
            outcome.expanded.contains("str joined = abab"),
            "{}",
            outcome.expanded
        );
    }

    #[test]
    fn where_conditions_call_compile_time_functions() {
        let source = r#"
grammar html {
    skip [ ' ', '\t', '\r', '\n' ]
    rule tag {
        "<" $word name ">" where @isVoid(name)
    }
}

bool isVoid(str name) {
    return name == "br" || name == "img"
}

magic voidTag(html.tag as t) {
    $"void:{ $t.name }"
}

str main() {
    return magic(voidTag) { <br> }
}
"#;
        let outcome = expand_source(source).expect("expansion succeeds");
        assert!(outcome.expanded.contains("void:br"), "{}", outcome.expanded);
    }

    #[test]
    fn cm_parse_expr_validates_and_returns_code() {
        let source = r#"
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]
    rule value {
        oneof {
            number => $int n
            string => $str s
        }
    }
}

magic calc(json.value as v) {
    let e = cm.parseExpr("1 + 2 * 3")
    $e
}

int main() {
    return magic(calc) { 7 }
}
"#;
        let outcome = expand_source(source).expect("expansion succeeds");
        assert!(
            outcome.expanded.contains("1 + 2 * 3"),
            "{}",
            outcome.expanded
        );

        let bad = source.replace("\"1 + 2 * 3\"", "\"1 + * 2\"");
        let error = expand_source(&bad).expect_err("invalid expression is rejected");
        assert!(
            error
                .iter()
                .any(|diagnostic| diagnostic.message().contains("cm.parseExpr")),
            "{error:?}"
        );
    }

    #[test]
    fn cm_code_builders_compose_code() {
        let source = r#"
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]
    rule value {
        oneof {
            number => $int n
            string => $str s
        }
    }
}

magic greet(json.value as v) {
    cm.code.call("usePy", cm.code.str($v.s), 5)
}

int usePy(str a, int b) {
    return b
}

int main() {
    return magic(greet) { "x" }
}
"#;
        let outcome = expand_source(source).expect("expansion succeeds");
        assert!(
            outcome.expanded.contains("usePy(\"x\", 5)"),
            "{}",
            outcome.expanded
        );
    }

    #[test]
    fn cm_parse_delegates_to_a_grammar_rule() {
        let source = r#"
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]
    rule value {
        oneof {
            string => $str s
            number => $int n
        }
    }
}

bool isString(Capture v) {
    match (v) {
        Rec(str tag, map<str, Capture> fields) => { return tag == "string" }
        _ => { return false }
    }
}

magic pick(json.value as v) {
    let again = cm.parse(json.value, "\"hi\"")
    [when @isString(again) { "was-string" } else { "other" }]
}

str main() {
    return magic(pick) { 1 }
}
"#;
        let outcome = expand_source(source).expect("expansion succeeds");
        assert!(
            outcome.expanded.contains("was-string"),
            "{}",
            outcome.expanded
        );
    }

    #[test]
    fn records_bridge_round_trip_and_match_in_checkmate() {
        // A user function that pattern-matches a bridged record capture and
        // builds a foreign enum value, rendered back into the template.
        let source = r#"
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]
    rule value {
        oneof {
            number => $int n
            string => $str s
        }
    }
}

enum JsonTree {
    Int(int value)
    Str(str text)
}

JsonTree toTree(Capture v) {
    match (v) {
        Rec(str tag, map<str, Capture> fields) => {
            match (fields["n"]) {
                Int(int n) => { return JsonTree.Int(n) }
                _ => { return JsonTree.Str("?") }
            }
        }
        Text(str s) => { return JsonTree.Str(s) }
        _ => { return JsonTree.Str("?") }
    }
}

magic tree(json.value as v) {
    @toTree($v)
}

int main() {
    JsonTree t = magic(tree) { 7 }
    match (t) {
        Int(int n) => { if (n != 7) { return 1 } }
        _ => { return 2 }
    }
    return 0
}
"#;
        let outcome = expand_source(source).expect("expansion succeeds");
        assert!(
            outcome.expanded.contains("JsonTree.Int(7)"),
            "{}",
            outcome.expanded
        );
    }

    #[test]
    fn compile_time_recursion_fails_cleanly() {
        let source = r#"
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]
    rule value {
        oneof {
            number => $int n
            string => $str s
        }
    }
}

magic loop(json.value as v) {
    @loop(1)
}

int loop(int x) {
    return loop(x)
}

int main() {
    return magic(loop) { 1 }
}
"#;
        let error = std::thread::Builder::new()
            .stack_size(96 * 1024 * 1024)
            .spawn(move || expand_source(source))
            .expect("spawn")
            .join()
            .expect("no panic")
            .expect_err("unbounded recursion is caught");
        assert!(
            error
                .iter()
                .any(|diagnostic| diagnostic.message().contains("compile-time call")),
            "{error:?}"
        );
    }

    #[test]
    fn unknown_compile_time_function_is_a_clear_diagnostic() {
        let source = r#"
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]
    rule value {
        oneof {
            number => $int n
            string => $str s
        }
    }
}

magic m(json.value as v) {
    @nope($v)
}

int main() {
    return magic(m) { 1 }
}
"#;
        let error = expand_source(source).expect_err("unknown function is rejected");
        assert!(
            error.iter().any(|diagnostic| {
                diagnostic
                    .message()
                    .contains("unknown compile-time function `nope`")
            }),
            "{error:?}"
        );
    }
}
