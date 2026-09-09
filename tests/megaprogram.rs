//! Megaprogramming (WHITEPAPER §8) end-to-end pins: expansion shapes, the
//! `cme expand` sidecar file, and clean parse/check of expanded output.
//!
//! Task 3 pins the JSON and `py.def` acceptance shapes plus the CLI; the
//! full `magic.cm` run-through (every megaprogram returning 0) is Task 5.

use cme_compiler::diagnostics::ParseOutcome;

fn parse_clean(source: &str) -> ParseOutcome {
    let outcome = cme_compiler::parse_source(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        outcome
            .diagnostics
            .iter()
            .map(|error| error.message().to_string())
            .collect::<Vec<_>>()
    );
    outcome
}

/// A JSON object region expands to a `map<str, str>` literal with one entry
/// per top-level field (leaves stringified, per the jsonValue contract).
#[test]
fn json_region_expands_to_a_map_literal() {
    let source = r#"
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

int checkConfig() {
    map<str, str> config = magic(jsonValue) {
        {
            "host": "db.local",
            "retries": 3,
            "debug": true,
            "name": null
        }
    }
    int fails = 0
    if (config["host"] != "db.local") { fails += 1 }
    if (config["retries"] != "3") { fails += 1 }
    if (config["debug"] != "true") { fails += 1 }
    if (config["name"] != "null") { fails += 1 }
    return fails
}

int main() {
    return checkConfig()
}
"#;
    let outcome = cme_compiler::mega::expand::expand_source(source).unwrap();
    let normalized: String = outcome
        .expanded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(normalized.contains("\"host\": \"db.local\""));
    assert!(normalized.contains("\"retries\": \"3\""));
    assert!(normalized.contains("\"debug\": \"true\""));
    assert!(normalized.contains("\"name\": \"null\""));
    assert!(!outcome.expanded.contains("magic(jsonValue)"));
    assert!(!outcome.expanded.contains("grammar json"));

    // The expanded program parses and type-checks on the untouched front end.
    let parsed = parse_clean(&outcome.expanded);
    let type_errors = cme_compiler::check::check(&parsed.statements);
    assert!(
        type_errors.is_empty(),
        "expanded program failed to check: {:?}",
        type_errors
            .iter()
            .map(|error| error.message().to_string())
            .collect::<Vec<_>>()
    );
}

/// The `py.def` flagship shape (§8.4's worked example): a Python-flavored
/// function expands to a real Checkmate function declaration. Nested `if`
/// bodies are handled one level deep until §8.5 compile-time recursion.
#[test]
fn py_def_region_expands_to_a_function_declaration() {
    let source = r##"
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
                ifStmt => if ($item.cond) {
                    [each in $item.body {
                        match ($item) {
                            return => return $item.value
                            call   => $item.callee(each in $item.args { $item.arg })
                        }
                    }]
                }
                return => return $item.value
                call   => $item.callee(each in $item.args { $item.arg })
            }
        }
    }
}

void usePy(int x) {
    return
}

magic(def) {
    def pySign(v: int) -> str:
        if v < 0:
            return "neg"
        return "pos"
}

magic(def) {
    def pyClamp(v: int, lo: int) -> int:
        usePy(lo)
        return v
}
"##;
    let outcome = cme_compiler::mega::expand::expand_source(source).unwrap();
    let normalized: String = outcome
        .expanded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(normalized.contains("str pySign(int v) {"));
    assert!(normalized.contains("if (v < 0) {"));
    assert!(normalized.contains("return \"neg\""));
    assert!(normalized.contains("return \"pos\""));
    assert!(normalized.contains("int pyClamp(int v, int lo) {"));
    assert!(normalized.contains("usePy(lo)"));
    assert!(normalized.contains("return v"));
    assert!(!outcome.expanded.contains("magic(def)"));
    assert!(!outcome.expanded.contains("grammar py"));

    let parsed = parse_clean(&outcome.expanded);
    let type_errors = cme_compiler::check::check(&parsed.statements);
    assert!(type_errors.is_empty());
}

/// Files without megaprogram constructs pass through byte-for-byte.
#[test]
fn expansion_passthrough_without_magic() {
    let source = "int x = 1\nint y = x + 2\n";
    let outcome = cme_compiler::mega::expand::expand_source(source).unwrap();
    assert_eq!(outcome.expanded, source);
    assert!(outcome.records.is_empty());
}

/// The REAL `magic.cm` fixture end to end (Task 5): expand → parse → check →
/// run on the tree walker → `main` returns 0, meaning every megaprogram
/// (JSON, TOML, YAML, CSS, HTML, RE, JS, Python, SQL) expanded and verified.
#[test]
fn magic_cm_expands_checks_and_runs_clean() {
    let source = include_str!("../magic.cm");
    let outcome = cme_compiler::mega::expand::expand_source(source).unwrap();

    // Every macro in the fixture was invoked exactly once, and no
    // invocation or declaration sites survive (comments do, so a raw
    // substring test would false-positive; re-scan instead).
    assert_eq!(outcome.records.len(), 12, "expected 12 invocations");
    let rescan = cme_compiler::mega::scan::scan_magic(&outcome.expanded).0;
    assert!(rescan.invocations.is_empty(), "invocations remain");
    assert!(rescan.magics.is_empty(), "magic declarations remain");
    assert!(rescan.grammars.is_empty(), "grammar declarations remain");

    // The expanded program is pure Checkmate: clean parse and type-check.
    let parsed = parse_clean(&outcome.expanded);
    let type_errors = cme_compiler::check::check(&parsed.statements);
    assert!(
        type_errors.is_empty(),
        "expanded magic.cm failed to check: {:?}",
        type_errors
            .iter()
            .map(|error| error.message().to_string())
            .collect::<Vec<_>>()
    );

    // And it runs: main() aggregates every check's failure count.
    let has_main = parsed
        .statements
        .iter()
        .any(|stmt| matches!(&stmt.kind, cme_core::ast::StmtKind::FuncDecl { name, .. } if name == "main"));
    assert!(has_main, "magic.cm must declare main");
    let interpreter = cme_interp::Interpreter::new(&parsed.statements);
    let result = interpreter
        .invoke("main", &[])
        .expect("magic.cm must run without interpreter errors");
    assert_eq!(
        result,
        cme_interp::Value::Int(0),
        "every megaprogram check must pass"
    );
}

/// §8.6 island piercing + the §8.8 JS-template-literal example: a
/// `magic(jsonValue)` invocation inside a template-literal island is a REAL
/// nested invocation — discovered by the scanner, expanded innermost-first
/// (the outer region then re-matches with the expanded island), and gone
/// from the final output.
#[test]
fn nested_invocation_inside_a_template_literal_island_expands() {
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

grammar js {
    skip    [ ' ', '\t' ]
    string  ( '`' multiline island ( "${" "}" ) )
    rule program { each { statement } as stmts }
    rule statement {
        oneof {
            log => ( "log" "(" primary value ")" semi )
        }
    }
    rule semi { oneof { explicit => ( ";" optional { eol } ), inserted => eol } }
    rule expression { primary }
    rule primary {
        oneof {
            template => $str literal
            ident    => $word name
        }
    }
}

enum Capture {
    Text(str content)
    Int(int value)
    Float(float value)
    List(Capture[] items)
    Rec(str tag, map<str, Capture> fields)
    Absent()
}

int toValue(Capture v) {
    match (v) {
        Rec(str tag, map<str, Capture> fields) => {
            for (str k in fields) {
                if (k == "n") {
                    match (fields["n"]) {
                        Int(int n) => { return n }
                        _ => {}
                    }
                }
            }
            return 0
        }
        _ => { return 0 }
    }
}

magic jsonValue(json.value as v) {
    @toValue($v)
}

magic jsEcho(js.program as p) {
    [
        [each in $p.stmts {
            match ($item) {
                log => $item.value.literal
            }
        }]
    ]
}

int main() {
    str[] echoed = magic(jsEcho) {
        log(`val: ${magic(jsonValue) { 7 }}`)
    }
    if (echoed[0] != "val: ${7}") { return 1 }
    return 0
}
"#;
    let outcome = cme_compiler::mega::expand::expand_source(source).unwrap();
    // Both the outer jsEcho and the island-nested jsonValue expanded.
    assert_eq!(outcome.records.len(), 2, "outer + nested invocation");
    // The nested invocation's result (7) sits inside the island of the
    // spliced template literal; the raw `magic(...)` text is gone.
    let normalized: String = outcome
        .expanded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        normalized.contains("val: ${7}"),
        "nested expansion should be inside the island: {}",
        outcome.expanded
    );
    let rescan = cme_compiler::mega::scan::scan_magic(&outcome.expanded).0;
    assert!(rescan.invocations.is_empty(), "invocations remain");

    let parsed = parse_clean(&outcome.expanded);
    let type_errors = cme_compiler::check::check(&parsed.statements);
    assert!(type_errors.is_empty(), "{type_errors:?}");
    let interpreter = cme_interp::Interpreter::new(&parsed.statements);
    let result = interpreter.invoke("main", &[]).expect("runs");
    assert_eq!(result, cme_interp::Value::Int(0));
}

/// §8.6 heredoc regions: `magic(name) <<tag … tag` extends the region
/// verbatim to the tag line — braces no composed profile could balance stay
/// exactly as authored.
#[test]
fn heredoc_regions_are_taken_verbatim() {
    let source = r#"
grammar freeform {
    skip [ ]
    rule text { $text t }
}

magic rawEcho(freeform.text as t) {
    $"{$t.t}"
}

int main() {
    str echoed = magic(rawEcho) <<END
  } unbalanced { brace - no profile needs to understand this
END
    if (echoed != "} unbalanced { brace - no profile needs to understand this") { return 1 }
    return 0
}
"#;
    let outcome = cme_compiler::mega::expand::expand_source(source).unwrap();
    assert_eq!(outcome.records.len(), 1);
    let normalized: String = outcome
        .expanded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        normalized.contains("} unbalanced { brace"),
        "{}",
        outcome.expanded
    );

    let parsed = parse_clean(&outcome.expanded);
    let type_errors = cme_compiler::check::check(&parsed.statements);
    assert!(type_errors.is_empty(), "{type_errors:?}");
    let interpreter = cme_interp::Interpreter::new(&parsed.statements);
    let result = interpreter.invoke("main", &[]).expect("runs");
    assert_eq!(result, cme_interp::Value::Int(0));
}

/// `cme expand magic.cm` writes a labeled sidecar file next to the original
/// whose parse+check is clean; `cme run magic.cm` runs the expansion.
#[cfg(feature = "cli")]
#[test]
fn cme_expand_writes_the_sidecar_file() {
    use std::process::Command;

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = std::path::Path::new(manifest_dir).join("magic.cm");
    let sidecar = std::path::Path::new(manifest_dir).join("magic_expanded.cm");
    let _ = std::fs::remove_file(&sidecar);

    let output = Command::new(env!("CARGO_BIN_EXE_cme"))
        .arg("expand")
        .arg(&fixture)
        .output()
        .expect("cme expand should run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cme expand failed: {stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("magic_expanded.cm"),
        "expand should name the sidecar: {stdout}"
    );
    assert!(sidecar.exists(), "the sidecar file must exist");

    // The sidecar is pure Checkmate: it parses and checks cleanly, and the
    // declarations were replaced by labeled marker comments.
    let expanded = std::fs::read_to_string(&sidecar).unwrap();
    assert!(expanded.contains("Generated by `cme expand"));
    assert!(!expanded.contains("magic(jsonValue)"));
    let parsed = parse_clean(&expanded);
    let type_errors = cme_compiler::check::check(&parsed.statements);
    assert!(type_errors.is_empty());
    let _ = std::fs::remove_file(&sidecar);
}
