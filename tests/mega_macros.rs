//! The macro gallery: full-shaped, real-world-flavored megaprograms —
//! config maps, HTML lists, code-generating compile-time functions,
//! recursive capture-tree folds, grammar extension, inline entries, and
//! context-steered nesting — every one driven through the whole pipeline
//! (expand → parse → check → run to `main == 0`), so a macro's OUTPUT is
//! held to the same bar as hand-written code.

use cme_compiler::check::check;
use cme_compiler::mega::expand::{ExpandOptions, expand_source_with};
use cme_interp::Value;

/// Expands, checks, and RUNS the source's `main`, expecting `Value::Int(0)`.
/// Returns the expanded text for extra pins.
fn expand_run(source: &str) -> String {
    let outcome = expand_source_with(source, ExpandOptions::default())
        .unwrap_or_else(|errors| panic!("expansion failed: {errors:?}\n{source}"));
    let parsed = cme_compiler::parse_source(&outcome.expanded);
    assert!(
        parsed.diagnostics.is_empty(),
        "expanded output must parse clean: {:?}\n---\n{}",
        parsed
            .diagnostics
            .iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>(),
        outcome.expanded
    );
    let type_errors = check(&parsed.statements);
    assert!(
        type_errors.is_empty(),
        "expanded output must check: {type_errors:?}\n---\n{}",
        outcome.expanded
    );
    let interpreter = cme_interp::Interpreter::new(&parsed.statements);
    let result = interpreter.invoke("main", &[]).unwrap_or_else(|error| {
        panic!(
            "expanded output must run: {error:?}\n---\n{}",
            outcome.expanded
        )
    });
    let Value::Int(0) = result else {
        panic!("main returned {result:?}\n---\n{}", outcome.expanded);
    };
    outcome.expanded
}

/// Expands and expects the error list to mention `contains`.
fn expand_err(source: &str, contains: &str) {
    let error =
        expand_source_with(source, ExpandOptions::default()).expect_err("expansion must fail");
    assert!(
        error.iter().any(|e| e
            .to_string()
            .to_lowercase()
            .contains(&contains.to_lowercase())),
        "errors {error:?} should mention {contains:?}"
    );
}

// ---------------------------------------------------------------------------
// 1. Flow config grammar → map<str, str>
// ---------------------------------------------------------------------------

/// A JSON-ish `key = value` config: quoted and bare branches collapse to
/// strings through `match`, and the map-literal each joins with newlines
/// (plan §1.4.7).
#[test]
fn flow_config_macro_builds_a_typed_map() {
    expand_run(
        r##"
grammar cfg {
    skip [ ' ', '\t', '\r', '\n' ]
    comment ( "#" )
    string  ( '"' )

    rule document {
        each sep "," { pair } as pairs
        eof
    }

    rule pair {
        $word key "=" oneof {
            quoted => $str value
            bare   => scan [A-Za-z0-9_.-] as value
        }
    }
}

mega cfgMap(cfg.document as d) {
    {
        [each in $d.pairs {
            $"{$item.key}": $"{$item.value}"
        }]
    }
}

int main() {
    map<str, str> config = cfgMap! {
        host = "db.local",          # the primary host
        port = 5432,
        env = prod-x
    }
    if (config["host"] != "db.local") { return 1 }
    if (config["port"] != "5432") { return 2 }
    if (config["env"] != "prod-x") { return 3 }
    return 0
}
"##,
    );
}

// A rule whose body is not oneof-topped tags its record with the RULE
// name; the branch label is internal. Pin that: match dispatches on
// `pair` for every branch, and both branches' `value` bind surfaces.
#[test]
fn non_oneof_topped_rules_tag_with_the_rule_name() {
    expand_err(
        r##"
grammar cfg2 {
    skip [ ' ', '\t', '\r', '\n' ]
    string  ( '"' )

    rule document {
        each sep "," { pair } as pairs
        eof
    }

    rule pair {
        $word key "=" oneof {
            quoted => $str value
            bare   => scan [A-Za-z0-9_.-] as value
        }
    }
}

mega cfgMap2(cfg2.document as d) {
    {
        [each in $d.pairs {
            $"{$item.key}": match ($item) {
                quoted => $item.value
            }
        }]
    }
}

int main() {
    map<str, str> config = cfgMap2! {
        host = "db.local"
    }
    return 0
}
"##,
        "non-exhaustive template match: no arm for `pair`",
    );
}

// ---------------------------------------------------------------------------
// 2. HTML list macro → str[]
// ---------------------------------------------------------------------------

/// `until { "</li>" }` stops at the whole pattern; the doubled outer
/// brackets comma-join the array elements.
#[test]
fn html_list_macro_builds_string_arrays() {
    expand_run(
        r##"
grammar hlist {
    skip [ ' ', '\t', '\r', '\n' ]
    comment ( "<!--" until "-->" )

    rule list {
        "<ul>" each { item } as items "</ul>"
    }

    rule item {
        "<li>" until { "</li>" } as body "</li>"
    }
}

mega listStrings(hlist.list as l) {
    [
        [each in $l.items {
            $"li: {$item.body}"
        }]
    ]
}

int main() {
    str[] got = listStrings! {
        <ul>
            <li>alpha</li>
            <!-- a comment that must not become an item -->
            <li>beta</li>
        </ul>
    }
    if (got.length != 2) { return 1 }
    if (got[0] != "li: alpha") { return 2 }
    if (got[1] != "li: beta") { return 3 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// 3. Compile-time code generation (§8.5): `code` returns + cm.code.fn
// ---------------------------------------------------------------------------

/// A pure function builds the array-literal TEXT and returns it as `code`,
/// so the template splices it raw into an initializer. The `code` return
/// type is what marks the result as raw (a plain str renders quoted).
#[test]
fn code_returning_functions_generate_declarations() {
    expand_run(
        r##"
grammar fill {
    skip [ ' ' ]
    rule shape {
        $word name "=" $int count ";" $str unit
    }
}

str repeated(str unit, int n) {
    if (n <= 0) {
        return ""
    }
    if (n == 1) {
        return unit
    }
    return unit + ", " + repeated(unit, n - 1)
}
code mkArray(str unit, int n) {
    return "[" + repeated(unit, n) + "]"
}

mega mkFill(fill.shape as s) {
    int[] $s.name = @mkArray($"{$s.unit}", $s.count)
}

int main() {
    mkFill! { ones = 4 ; "1" }
    mkFill! { squares = 3 ; "9" }
    if (ones.length != 4 || ones[3] != 1) { return 1 }
    if (squares.length != 3 || squares[2] != 9) { return 2 }
    return 0
}
"##,
    );
}

/// A pure function builds a whole function DECLARATION as `code` text and
/// the template splices it raw at top level. (The `cm.code.*` builders are
/// engine-level builtins — callable from templates, not inside fn bodies.)
#[test]
fn cm_code_fn_builds_whole_functions() {
    expand_run(
        r##"
grammar tag {
    skip [ ' ' ]
    rule shape {
        $word name ";" $int v
    }
}

mega mkGetter(tag.shape as s) {
    @emitGetter($"{$s.name}", $s.v)
}

mkGetter! { answer ; 42 }

code emitGetter(str name, int v) {
    return "int " + name + "_value() {\n    return " + v + "\n}"
}

int main() {
    if (answer_value() != 42) { return 1 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// 4. Recursive capture-tree folds (§8.5 + the Capture enum)
// ---------------------------------------------------------------------------

/// Nested groups recurse through a rule reference; a pure function walks
/// the materialized `Capture` tree and folds it to a leaf count, which the
/// template splices as a plain int literal.
#[test]
fn capture_tree_functions_fold_recursively() {
    expand_run(
        r##"
grammar tree {
    skip [ ' ', '\t', '\r', '\n' ]

    rule node {
        oneof {
            leaf  => $int v
            group => ( "(" each sep "," { node } as kids ")" )
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

int leaves(Capture v) {
    match (v) {
        Int(int n) => { return 1 }
        List(Capture[] items) => {
            int total = 0
            for (Capture item in items) {
                total += leaves(item)
            }
            return total
        }
        Rec(str tag, map<str, Capture> fields) => {
            int total = 0
            if (tag == "leaf") {
                return 1
            }
            for (str k in fields) {
                total += leaves(fields[k])
            }
            return total
        }
        _ => { return 0 }
    }
}

mega leafCount(tree.node as n) {
    @leaves($n)
}

int main() {
    if (leafCount! { 5 } != 1) { return 1 }
    if (leafCount! { (1, 2, 3) } != 3) { return 2 }
    if (leafCount! { (1, (2, 3), ((4, 5), 6)) } != 6) { return 3 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// 5. Grammar extension (§8.2): override + inherit
// ---------------------------------------------------------------------------

/// `grammar jext extends base` overrides one rule and inherits the rest;
/// each grammar's macro works against its own rule set.
#[test]
fn grammar_extension_overrides_and_inherits() {
    expand_run(
        r##"
grammar base {
    skip [ ' ' ]
    string  ( '"' )

    rule lit {
        oneof {
            quoted => $str text
            word   => $word text
        }
    }
}

grammar jext extends base {
    rule lit {
        oneof {
            word   => $word text
            quoted => $str text
        }
    }
}

mega baseLit(base.lit as b) {
    match ($b) {
        quoted => $"q:{ $b.text }"
        word   => $"w:{ $b.text }"
    }
}

mega jextLit(jext.lit as j) {
    match ($j) {
        word   => $"w:{ $j.text }"
        quoted => $"q:{ $j.text }"
    }
}

int main() {
    if (baseLit! { "hi" } != "q:hi") { return 1 }
    if (baseLit! { yo } != "w:yo") { return 2 }
    if (jextLit! { yo } != "w:yo") { return 3 }
    if (jextLit! { "hi" } != "q:hi") { return 4 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// 6. Inline entry points (§8.1) at declaration position
// ---------------------------------------------------------------------------

/// An INLINE entry pattern (no leading rule reference) with raw splices
/// generating whole declarations; two instances coexist by name.
#[test]
fn inline_entry_generates_stepper_functions() {
    expand_run(
        r##"
mega stepper($word name "(" $int from ".." $int to ")") {
    int $name(int current) {
        if (current >= $to) {
            return $from
        }
        return current + 1
    }
}

stepper! { page ( 0 .. 10 ) }
stepper! { depth ( 1 .. 5 ) }

int main() {
    if (page(9) != 10) { return 1 }
    if (page(10) != 0) { return 2 }
    if (depth(4) != 5) { return 3 }
    if (depth(5) != 1) { return 4 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// 7. Context accumulation (§8.3.7) steers nesting
// ---------------------------------------------------------------------------

/// The HTML open-tag stack: `context { str[] open }` threads the ancestor
/// list downward, `append` extends it, `some x in` rejects duplicate names,
/// and a mismatched closer fails the whole match.
#[test]
fn context_accumulation_rejects_duplicate_and_mismatched_nesting() {
    expand_run(
        r##"
grammar nest2 {
    skip [ ' ', '\t', '\r', '\n' ]

    rule doc {
        each { element } as items
        eof
    }

    rule element(context { str[] open = none }) {
        "<" $word name ">"
        where !present(open) || !some x in open { x == name }
        optional { until { "<" } as text }
        each { element with context { open: append(open, name) } } as children
        "</" $word close ">"
        where close == name
    }
}

mega nestStrings(nest2.doc as d) {
    [
        [each in $d.items {
            $"node:{ $item.name }"
        }]
    ]
}

int main() {
    str[] got = nestStrings! {
        <html>
            <body>hi</body>
        </html>
        <p>text</p>
    }
    if (got.length != 2) { return 1 }
    if (got[0] != "node:html") { return 2 }
    if (got[1] != "node:p") { return 3 }
    return 0
}
"##,
    );
    let _ = expand_source_with(
        r##"
grammar nest4 {
    skip [ ' ', '\t', '\r', '\n' ]

    rule doc {
        element
    }

    rule element(context { str[] open = none }) {
        "<" $word name ">"
        where !present(open) || !some x in open { x == name }
        each { element with context { open: append(open, name) } } as children
        "</" $word close ">"
        where close == name
    }
}

mega nestEcho(nest4.element as e) {
    $"ok"
}

int main() {
    str s = nestEcho! {
        <a><a>x</a></a>
    }
    return 0
}
"##,
        ExpandOptions::default(),
    )
    .expect_err("<a> inside <a> must fail the context where");
}

// ---------------------------------------------------------------------------
// 8. Parameterized $template + $raw delegation
// ---------------------------------------------------------------------------

/// `$template<open close rule>` splits at CUSTOM delimiters and parses the
/// islands with the referenced rule; `.matched`/`.length` accessors expose
/// capture extents. `\` escapes a literal delimiter.
#[test]
fn parameterized_template_islands_use_custom_delimiters_and_rules() {
    expand_run(
        r##"
grammar tpl2 {
    skip [ ' ' ]

    rule expr {
        oneof {
            word => $word w
            num  => $int n
        }
    }

    rule banner {
        $template<"[[" "]]" expr> parts
    }
}

mega bannerParts(tpl2.banner as b) {
    [
        [each in $b.parts {
            match ($item) {
                text => $"t({$item.text})"
                expr => $"e({$item.value})"
            }
        }]
    ]
}

int main() {
    str[] got = bannerParts! {
        total [[count]] and [[42]] end
    }
    if (got.length != 5) { return 1 }
    if (got[0] != "t(total )") { return 2 }
    if (got[1] != "e(count)") { return 3 }
    if (got[2] != "t( and )") { return 4 }
    if (got[3] != "e(42)") { return 5 }
    if (got[4] != "t( end)") { return 6 }
    return 0
}
"##,
    );
}

/// `$raw<grammar.rule>` delegates the tail parse to the referenced rule:
/// the capture is a record of THAT rule's shape, re-emittable directly.
#[test]
fn raw_delegation_parses_by_the_referenced_rule() {
    expand_run(
        r##"
grammar json2 {
    skip [ ' ', '\t', '\r', '\n' ]
    string ( '"' )

    rule value {
        oneof {
            number => $int n
            text   => $str s
        }
    }

    rule pair {
        $str key ":" $raw<json2.value> v eof
    }
}

mega pairEcho(json2.pair as p) {
    match ($p.v) {
        number => $"{$p.key}=<num {$p.v.n}>"
        text   => $"{$p.key}=<str {$p.v.s}>"
    }
}

int main() {
    if (pairEcho! { "port" : 8080 } != "port=<num 8080>") { return 1 }
    if (pairEcho! { "name" : "atlas" } != "name=<str atlas>") { return 2 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// 9. Fragment validators
// ---------------------------------------------------------------------------

/// `$word<rule>` gates the fragment through a grammar rule; a pure
/// function validator gates a `$int`. Violations are expansion errors.
#[test]
fn validators_gate_fragments() {
    expand_run(
        r##"
grammar valid {
    skip [ ' ' ]

    rule digitsOnly {
        scan [0-9] as d
    }

    rule identish {
        scan [a-z] as d
    }

    rule shape {
        $word<valid.identish> code ";" $int<positive> n
    }
}

bool positive(int n) {
    return n > 0
}

mega shapeEcho(valid.shape as s) {
    $"{$s.code}/{$s.n}"
}

int main() {
    if (shapeEcho! { code ; 7 } != "code/7") { return 1 }
    return 0
}
"##,
    );
    expand_err(
        r##"
grammar valid2 {
    skip [ ' ' ]

    rule identish {
        scan [a-z] as d
    }

    rule shape {
        $word<valid2.identish> code
    }
}

mega shapeEcho(valid2.shape as s) {
    $"{$s.code}"
}

int main() {
    str s = shapeEcho! { mixedCase }
    return 0
}
"##,
        "",
    );
    let _ = expand_source_with(
        r##"
grammar valid3 {
    skip [ ' ' ]

    rule shape {
        $word<valid3.identish> code
    }

    rule identish {
        scan [a-z] as d
    }
}

mega shapeEcho(valid3.shape as s) {
    $"{$s.code}"
}

int main() {
    str s = shapeEcho! { mixedCase }
    return 0
}
"##,
        ExpandOptions::default(),
    )
    .expect_err("a word with uppercase must fail the lowercase validator");
}
