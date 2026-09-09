//! The expansion orchestrator (the megaprogram pass): takes magic
//! definitions, expands magic calls into normal Checkmate SOURCE TEXT, and
//! hands the result back to the main compiler (owner architecture mandate —
//! code, not AST, so `cme expand` can write it out).
//!
//! Pipeline (plan §2.5): scan → compile grammars and macros → expand every
//! invocation (match pattern over region, elaborate template to text) →
//! fixpoint (generated text is re-scanned for new invocations, depth-capped)
//! → strip declaration sites (replaced by marker comments that preserve line
//! counts).

use cme_core::Span;
use cme_core::magic::{PatElem, PatKind, Pattern};

use crate::diagnostics::Diagnostic;
use crate::mega::matcher::{CompiledGrammar, CompiledRule, GrammarSet, MatchRegion, match_entry};
use crate::mega::pattern::{parse_pattern, parse_rule_declaration};
use crate::mega::profile::default_profile;
use crate::mega::scan::{MagicScan, scan_magic};
use crate::mega::template::{elaborate, parse_template};

/// One expanded invocation, recorded for tooling and provenance.
#[derive(Debug, Clone)]
pub struct ExpansionRecord {
    /// The macro name.
    pub magic: String,
    /// The invocation's span in the text the pass ran on.
    pub span: Span,
}

/// The result of a successful expansion pass.
#[derive(Debug, Clone)]
pub struct ExpansionOutcome {
    /// Pure Checkmate source: every magic declaration removed, every magic
    /// invocation replaced by its generated code.
    pub expanded: String,
    pub records: Vec<ExpansionRecord>,
}

/// The whitepaper's expansion-tree depth cap (§8.6), counting all origins.
const DEPTH_CAP: usize = 64;

/// Expands every megaprogram in `source`. Returns the expanded source, or
/// every diagnostic encountered (spans anchored in `source`).
pub fn expand_source(source: &str) -> Result<ExpansionOutcome, Vec<Diagnostic>> {
    let (scan, scan_errors) = scan_magic(source);
    if !scan_errors.is_empty() {
        return Err(scan_errors);
    }
    if scan.is_empty() {
        return Ok(ExpansionOutcome {
            expanded: source.to_string(),
            records: Vec::new(),
        });
    }

    let mut diagnostics = Vec::new();
    let set = compile_grammars(&scan, &mut diagnostics);
    let macros = compile_macros(&scan, &set, &mut diagnostics);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let mut current = source.to_string();
    let mut records = Vec::new();
    let mut depth = 0usize;
    loop {
        let (round_scan, round_errors) = scan_magic(&current);
        if !round_errors.is_empty() {
            return Err(round_errors);
        }
        if round_scan.invocations.is_empty() {
            break;
        }
        depth += 1;
        if depth > DEPTH_CAP {
            let span = round_scan.invocations[0].span;
            return Err(vec![Diagnostic::parse(
                format!("expansion depth exceeded the cap of {DEPTH_CAP}"),
                span,
            )]);
        }
        let mut edits: Vec<(Span, String)> = Vec::new();
        for invocation in &round_scan.invocations {
            let Some(macro_def) = macros
                .iter()
                .find(|macro_def| macro_def.name == invocation.name)
            else {
                diagnostics.push(Diagnostic::parse(
                    format!("unknown magic macro `{}`", invocation.name),
                    invocation.header_span,
                ));
                continue;
            };
            // Region spans anchor in `current` (the text being processed);
            // match positions map back through the region's base offset.
            let region = MatchRegion::new(
                invocation.region.as_str(),
                invocation.region_span.start,
                &invocation.region,
            );
            match match_entry(&set, macro_def.grammar_index, &macro_def.pattern, &region) {
                Ok(root) => {
                    let generated = elaborate(
                        &macro_def.template,
                        &macro_def.bind_name,
                        root,
                        invocation.span,
                    );
                    match generated {
                        Ok(text) => {
                            // The template's own layout whitespace at the
                            // output's edges is an artifact of the template
                            // source, not code (plan §1.4.9): trim it so an
                            // expression-position invocation sits flush
                            // against its context.
                            records.push(ExpansionRecord {
                                magic: invocation.name.clone(),
                                span: invocation.span,
                            });
                            edits.push((invocation.span, text.trim().to_string()));
                        }
                        Err(errors) => diagnostics.extend(errors),
                    }
                }
                Err(failure) => {
                    let at = invocation.region_span.start + failure.byte_offset;
                    diagnostics.push(Diagnostic::parse(
                        format!("magic pattern did not match: {}", failure.message),
                        Span::new(at, at),
                    ));
                }
            }
        }
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        let next = apply_edits(&current, &edits);
        current = next;
    }

    // Strip declaration sites (grammar and magic declarations are
    // compile-time constructs; the expanded file is pure Checkmate).
    let (final_scan, _) = scan_magic(&current);
    let mut edits: Vec<(Span, String)> = Vec::new();
    for grammar in &final_scan.grammars {
        let newlines = count_newlines(&current, grammar.span);
        edits.push((
            grammar.span,
            format!(
                "// [megaprogram grammar '{}' removed by expansion]{}",
                grammar.name,
                "\n".repeat(newlines)
            ),
        ));
    }
    for magic in &final_scan.magics {
        let newlines = count_newlines(&current, magic.span);
        edits.push((
            magic.span,
            format!(
                "// [megaprogram magic '{}' removed by expansion]{}",
                magic.name,
                "\n".repeat(newlines)
            ),
        ));
    }
    let next = apply_edits(&current, &edits);
    current = next;

    Ok(ExpansionOutcome {
        expanded: current,
        records,
    })
}

fn count_newlines(text: &str, span: Span) -> usize {
    text[span.start..span.end]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
}

/// Applies non-overlapping span edits, highest position first.
fn apply_edits(text: &str, edits: &[(Span, String)]) -> String {
    let mut edits = edits.to_vec();
    edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    let mut out = text.to_string();
    for (span, replacement) in edits {
        out.replace_range(span.start..span.end, &replacement);
    }
    out
}

/// A compiled macro: name, entry pattern, template, entry grammar index,
/// and the entry capture's bind name.
struct CompiledMacro {
    name: String,
    pattern: Pattern,
    template: cme_core::magic::Template,
    grammar_index: usize,
    bind_name: String,
}

/// Parses every grammar body into compiled rules.
fn compile_grammars(scan: &MagicScan, diagnostics: &mut Vec<Diagnostic>) -> GrammarSet {
    let mut set = GrammarSet::default();
    for grammar in &scan.grammars {
        let mut compiled = CompiledGrammar {
            name: grammar.name.clone(),
            profile: grammar.profile.clone(),
            rules: Vec::new(),
        };
        let body = grammar.body.clone();
        let mut cursor = 0usize;
        while cursor < body.len() {
            let mut probe = cursor;
            crate::mega::profile::skip_ws_and_comments(
                &body,
                &mut probe,
                &crate::mega::profile::checkmate_scan_profile(),
            );
            if probe >= body.len() {
                break;
            }
            let at_rule = body[probe..].starts_with("rule")
                && !body[probe + "rule".len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            if at_rule {
                match parse_rule_declaration(&body, probe, grammar.body_span) {
                    Ok((name, context, pattern, end)) => {
                        compiled.rules.push(CompiledRule {
                            name,
                            context,
                            pattern,
                        });
                        cursor = end;
                    }
                    Err(error) => {
                        diagnostics.push(error);
                        break;
                    }
                }
            } else {
                cursor = probe + body[probe..].chars().next().unwrap().len_utf8();
            }
        }
        set.grammars.push(compiled);
    }
    // A synthetic default grammar hosts inline entry patterns.
    set.grammars.push(CompiledGrammar {
        name: "\u{0}default".to_string(),
        profile: default_profile(),
        rules: Vec::new(),
    });
    set
}

/// Parses every magic declaration into a compiled macro.
fn compile_macros(
    scan: &MagicScan,
    set: &GrammarSet,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<CompiledMacro> {
    let mut macros = Vec::new();
    for magic in &scan.magics {
        let pattern = match parse_pattern(&magic.pattern, magic.pattern_span) {
            Ok(pattern) => pattern,
            Err(error) => {
                diagnostics.push(error);
                continue;
            }
        };
        let template = match parse_template(&magic.template, magic.template_span) {
            Ok(template) => template,
            Err(error) => {
                diagnostics.push(error);
                continue;
            }
        };
        // Entry grammar + bind name from the pattern's leading rule ref.
        let first = pattern.elems.first();
        if let Some(PatElem {
            kind: PatKind::RuleRef { path, bind, .. },
            ..
        }) = first
        {
            let grammar_index = match path.len() {
                2 => match set
                    .grammars
                    .iter()
                    .position(|grammar| grammar.name == path[0])
                {
                    Some(index) => index,
                    None => {
                        diagnostics.push(Diagnostic::parse(
                            format!("unknown grammar `{}` in the entry pattern", path[0]),
                            magic.pattern_span,
                        ));
                        continue;
                    }
                },
                _ => match set
                    .grammars
                    .iter()
                    .position(|grammar| grammar.rules.iter().any(|rule| rule.name == path[0]))
                {
                    Some(index) => index,
                    None => {
                        diagnostics.push(Diagnostic::parse(
                            format!("unknown rule `{}` in the entry pattern", path[0]),
                            magic.pattern_span,
                        ));
                        continue;
                    }
                },
            };
            let Some(bind_name) = bind.clone() else {
                diagnostics.push(Diagnostic::parse(
                    "the entry pattern must bind its capture (`grammar.rule as name`)",
                    magic.pattern_span,
                ));
                continue;
            };
            macros.push(CompiledMacro {
                name: magic.name.clone(),
                pattern,
                template,
                grammar_index,
                bind_name,
            });
        } else {
            diagnostics.push(Diagnostic::parse(
                "the entry pattern must begin with a rule reference (`grammar.rule as name`)",
                magic.pattern_span,
            ));
        }
    }
    macros
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Task 3 acceptance shape: a JSON object region expands to a
    /// `map<str, str>` literal with one entry per top-level field.
    const JSON_MIN: &str = r#"
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]

    rule value {
        oneof {
            null   => "null"
            bool   => oneof { t => "true", f => "false" }
            number => number
            string => $str text
            array  => ( "[" each sep "," { value } as items "]" )
            object => ( "{" each sep "," { member } as fields "}" )
        }
    }

    rule member {
        $str key ":" value as val
    }

    rule number {
        ( optional { "-" }
          oneof { zero => "0", pos => ( [1-9] as first optional { scan [0-9] as rest } ) }
          optional { "." scan [0-9] as frac } ) as num
    }
}

magic jsonValue(json.value as v) {
    match ($v) {
        null   => "null"
        bool   => $"{$v.matched}"
        number => $"{$v.matched}"
        string => $v.text
        array  => "[]"
        object => (
            {
                [each in $v.fields {
                    $item.key: match ($item.val) {
                        null   => "null"
                        bool   => $"{$item.val.matched}"
                        number => $"{$item.val.matched}"
                        string => $item.val.text
                        array  => "[]"
                        object => "{}"
                    }
                }]
            }
        )
    }
}

map<str, str> config = magic(jsonValue) {
    {
        "host": "db.local",
        "retries": 3,
        "debug": true,
        "name": null
    }
}
"#;

    #[test]
    fn json_region_expands_to_a_map_literal() {
        let outcome = match expand_source(JSON_MIN) {
            Ok(outcome) => outcome,
            Err(errors) => panic!(
                "expansion failed: {:?}",
                errors
                    .iter()
                    .map(|error| error.message().to_string())
                    .collect::<Vec<_>>()
            ),
        };
        // The exact layout spacing of generated entries is not pinned;
        // whitespace runs are normalized for the assertions.
        let normalized = normalize_ws(&outcome.expanded);
        assert!(normalized.contains("\"host\": \"db.local\""));
        assert!(normalized.contains("\"retries\": \"3\""));
        assert!(normalized.contains("\"debug\": \"true\""));
        assert!(normalized.contains("\"name\": \"null\""));
        assert!(!outcome.expanded.contains("magic(jsonValue)"));
        assert!(!outcome.expanded.contains("grammar json"));
    }

    /// Collapses whitespace runs to single spaces (test helper).
    fn normalize_ws(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut pending_space = false;
        for c in text.chars() {
            if c.is_whitespace() {
                pending_space = true;
            } else {
                if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_space = false;
                out.push(c);
            }
        }
        out
    }

    /// The `py.def` flagship shape (the real `magic.cm` grammar and
    /// template, §8.4's worked example): a Python-flavored function expands
    /// to a real Checkmate function declaration. Nested `if` bodies are
    /// handled one level deep until §8.5 compile-time recursion (Task 8).
    const PY_MIN: &str = r##"
grammar py {
    skip    [ ' ' ]
    comment ( "#" )

    rule def {
        "def" $word fname
        "(" soft { each sep "," { $word param optional { ":" $type ptype } } as params ")" }
        "->" $type ret ":"
        indent { each { stmt } as body }
    }

    rule stmt {
        oneof {
            ifStmt => (
                "if" $expr cond ":"
                indent { each { recur } as body }
            )
            return => ( "return" optional { $expr value } eol )
            call   => ( $word callee "(" soft { each sep "," { $expr arg } as args ")" } eol )
        }
    }
}

magic def(py.def as d) {
    $d.ret $d.fname(each in d.params {
        [when present($ptype) { $ptype $param } else { infer $param }]
    }) {
        each in d.body {
            match ($item) {
                return => return $item.value
                call   => $item.callee(each in $item.args { $item.arg })
            }
        }
    }
}

magic(def) {
    def pyClampFn(v: int, lo: int) -> int:
        usePy(lo)
        return v
}
"##;

    #[test]
    fn py_def_expands_to_a_function_declaration() {
        let outcome = match expand_source(PY_MIN) {
            Ok(outcome) => outcome,
            Err(errors) => panic!(
                "expansion failed: {:?}",
                errors
                    .iter()
                    .map(|error| error.message().to_string())
                    .collect::<Vec<_>>()
            ),
        };
        // Layout spacing of the generated code is not pinned; whitespace
        // runs are normalized (the expanded program still parses because
        // newlines are insignificant inside parens, §A.8).
        let normalized = normalize_ws(&outcome.expanded);
        assert!(normalized.contains("int pyClampFn(int v, int lo)"));
        assert!(normalized.contains("usePy(lo)"));
        assert!(normalized.contains("return v"));
        assert!(!outcome.expanded.contains("magic(def)"));
        assert!(!outcome.expanded.contains("grammar py"));
    }

    #[test]
    fn file_without_magic_is_unchanged() {
        let source = "int x = 1\n";
        let outcome = expand_source(source).expect("passthrough");
        assert_eq!(outcome.expanded, source);
        assert!(outcome.records.is_empty());
    }
}
