//! Megaprogramming pattern-language stress suite: every §8.3 combinator,
//! fragment, constraint, and region shape, driven through `expand_source`
//! end to end (expand → check → run) so generated code is held to the same
//! bar as hand-written code.

use cme_compiler::check::check;
use cme_compiler::mega::expand::{expand_source_with, ExpandOptions};
use cme_interp::Value;

/// Expands, checks, and RUNS the source's `main`, expecting `Value::Int(0)`.
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
    let error = expand_source_with(source, ExpandOptions::default())
        .expect_err("expansion must fail");
    assert!(
        error
            .iter()
            .any(|e| e.to_string().to_lowercase().contains(&contains.to_lowercase())),
        "errors {error:?} should mention {contains:?}"
    );
}

/// An echo harness: a line-oriented grammar with custom rules; the macro
/// splices `$w`'s named fields, and `main` pins the result against `pin`.
/// Placeholders: __RULES__ __ENTRY__ __TEMPLATE__ __REGION__ __PIN__.
const ECHO_TEMPLATE: &str = r##"
grammar echo {
    skip [ ' ' ]
__RULES__
}

magic echoIt(echo.__ENTRY__ as w) {
__TEMPLATE__
}

int main() {
    str got = magic(echoIt) {
__REGION__
    }
    if (got != "__PIN__") { return 1 }
    return 0
}
"##;

fn echo(rules: &str, entry: &str, template: &str, region: &str, pin: &str) -> String {
    ECHO_TEMPLATE
        .replace("__RULES__", rules)
        .replace("__ENTRY__", entry)
        .replace("__TEMPLATE__", template)
        .replace("__REGION__", region)
        .replace("__PIN__", pin)
}

// ---------------------------------------------------------------------------
// Literals, classes, any, scan, until
// ---------------------------------------------------------------------------

#[test]
fn literals_and_case_insensitive_literals() {
    expand_run(&echo(
        "    rule word { ( \"alpha\" \"beta\" ) as rest }",
        "word",
        "    $\"{$w.rest}\"",
        "alpha beta",
        "alpha beta",
    ));
    // i"..." folds case.
    expand_run(&echo(
        "    rule word { ( i\"heLLo\" \"world\" ) as rest }",
        "word",
        "    $\"{$w.rest}\"",
        "HELLO world",
        "HELLO world",
    ));
}

#[test]
fn classes_any_and_scan_binds() {
    // A class matches exactly one character; a scan is a maximal run.
    expand_run(&echo(
        "    rule word { [0-9] as d \"x\" scan [a-z] as tail }",
        "word",
        "    $\"{$w.d}:{$w.tail}\"",
        "5xhello",
        "5:hello",
    ));
    // Negated classes: one char not in the set.
    expand_run(&echo(
        "    rule word { [^x] as c \"end\" }",
        "word",
        "    $\"{$w.c}\"",
        "yend",
        "y",
    ));
}

/// `until` stops at the first whole-pattern match and binds the verbatim
/// run. To get a string VALUE from a raw text capture the template must
/// interpolate (§8.4: bare text splices are raw; `$"{…}"` stringifies),
/// and capture values are whitespace-trimmed at the edges (plan §1.4.4).
#[test]
fn until_stops_at_pattern_as_a_whole() {
    expand_run(&echo(
        "    rule word { until \"END\" as body \"END\" }",
        "word",
        "    $\"{$w.body}\"",
        "keep this END",
        "keep this",
    ));
    // A pattern stop (`until { p }`) is anchored the same way, and the
    // stop condition consumes nothing (§8.3.2).
    expand_run(&echo(
        "    rule word { until { \"!!!\" } as body \"!!!\" }",
        "word",
        "    $\"[{$w.body}]\"",
        "shout !!!",
        "[shout]",
    ));
}

/// Raw text splices are emitted verbatim, so they are what builds
/// identifiers in declaration positions (§8.4 position table; the py.def
/// flagship pins the same contract with `$d.ret $d.fname(…)`).
#[test]
fn raw_splices_form_identifiers_and_expressions() {
    // A scan capture spliced as a declaration name, in statement position.
    expand_run(
        r##"
grammar g {
    skip [ ' ' ]
    rule ident { scan [A-Za-z0-9_] as name }
}

magic decl(g.ident as x) {
    int $x.name = 40 + 2
}

int main() {
    magic(decl) { answer }
    if (answer != 42) { return 1 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// Line mode: eol, lineRest, eof, comment tails
// ---------------------------------------------------------------------------

/// `eol` consumes a trailing comment form, the terminator, and the
/// following transparent tail (§8.3.1/§8.3.2) — so the next element can
/// match on the following line even though the skip set has no newline.
#[test]
fn eol_consumes_terminator_and_comment_tail() {
    expand_run(
        r##"
grammar lines {
    skip [ ' ' ]
    comment ( "#" )

    rule entry {
        "name" ":" scan [a-z] as value eol
        "age" ":" scan [0-9] as age eol
        eof
    }
}

magic linesOut(lines.entry as e) {
    $"{$e.value}|{$e.age}"
}

int main() {
    str got = magic(linesOut) {
        name: ada # trailing comment
        age: 36
    }
    if (got != "ada|36") { return 1 }
    return 0
}
"##,
    );
}

/// `lineRest` is atomic: comment forms inside its extent are DATA (§8.3.1
/// rule 2), it runs through the line terminator (plan §2.4 note), and the
/// capture value is edge-trimmed (plan §1.4.4).
#[test]
fn lineRest_is_verbatim_and_atomic() {
    expand_run(
        r##"
grammar lr {
    skip [ ' ' ]
    comment ( "#" )

    rule entry {
        "name" ":" lineRest as value
        "age" ":" lineRest as age
        eof
    }
}

magic lrOut(lr.entry as e) {
    $"{$e.value}|{$e.age}"
}

int main() {
    str got = magic(lrOut) {
        name: ada # trailing comment
        age: 36
    }
    if (got != "ada # trailing comment|36") { return 1 }
    return 0
}
"##,
    );
}

#[test]
fn soft_joins_newlines_inside_the_region() {
    expand_run(
        r##"
grammar flow {
    skip [ ' ' ]

    rule list {
        "[" soft { each sep "," { scan [0-9] as n } as items } "]"
    }
}

magic listOut(flow.list as l) {
    $l.items.length
}

int main() {
    int n = magic(listOut) {
        [11,
         22,
         33]
    }
    if (n != 3) { return 1 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// oneof fall-through and where as a branch selector
// ---------------------------------------------------------------------------

#[test]
fn oneof_falls_through_on_where_failures() {
    expand_run(
        r##"
grammar tags {
    skip [ ' ' ]

    rule tag {
        oneof {
            br    => ( i"br" where true )
            img   => ( i"img" where true )
            other => ( scan [a-z] as name )
        }
    }
}

magic tagOut(tags.tag as t) {
    match ($t) {
        br    => "void"
        img   => "void"
        other => $"{$t.name}"
    }
}

int main() {
    if (magic(tagOut) { br } != "void") { return 1 }
    if (magic(tagOut) { IMG } != "void") { return 2 }
    if (magic(tagOut) { div } != "div") { return 3 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// Repetition and optionality
// ---------------------------------------------------------------------------

#[test]
fn each_bounds_and_separator() {
    expand_run(
        r##"
grammar reps {
    skip [ ' ', ',' ]

    rule digits {
        each [2, 3] { scan [0-9] as d } as nums
    }
}

magic repOut(reps.digits as r) {
    $r.nums.length
}

int main() {
    if (magic(repOut) { 1, 2 } != 2) { return 1 }
    if (magic(repOut) { 1, 2, 3 } != 3) { return 2 }
    return 0
}
"##,
    );
}

/// The §8.4 optional usage: an UNBOUND optional passes its inner binds
/// through only when it matched, `present($x)` tests them (false when out
/// of scope), and string values come from interpolation. A BOUND optional
/// wraps the body value in an `Opt` capture that splices/interpolates
/// unwrapped.
#[test]
fn optional_binds_present_or_absent() {
    expand_run(
        r##"
grammar opt {
    skip [ ' ' ]

    rule kv {
        scan [a-z] as key
        optional { "=" scan [0-9] as value }
    }
}

magic optOut(opt.kv as k) {
    [when present($k.value) { $"{$k.key}={$k.value}" } else { $"{$k.key}-none" }]
}

int main() {
    if (magic(optOut) { count=7 } != "count=7") { return 1 }
    if (magic(optOut) { flag } != "flag-none") { return 2 }
    return 0
}
"##,
    );
    // The bound form: `$rest` holds the body value (an `Opt` capture),
    // present in both the guard and the interpolation.
    expand_run(
        r##"
grammar opt2 {
    skip [ ' ' ]

    rule kv {
        scan [a-z] as key
        optional { "=" scan [0-9] as num } as rest
    }
}

magic optOut2(opt2.kv as k) {
    [when present($k.rest) { $"{$k.key}={$k.rest}" } else { $"{$k.key}-none" }]
}

int main() {
    if (magic(optOut2) { count=7 } != "count=7") { return 1 }
    if (magic(optOut2) { flag } != "flag-none") { return 2 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// peek / raw
// ---------------------------------------------------------------------------

#[test]
fn peek_is_zero_width() {
    expand_run(
        r##"
grammar looks {
    skip [ ' ' ]

    rule item {
        peek { "on" } ( "one" ) as w
    }
}

magic lookOut(looks.item as l) {
    $"{$l.w}"
}

int main() {
    if (magic(lookOut) { one } != "one") { return 1 }
    return 0
}
"##,
    );
}

#[test]
fn raw_suspends_the_skipper() {
    expand_run(
        r##"
grammar raws {
    skip [ ' ' ]

    rule padded {
        ( raw { "a  b" } ) as tight
    }
}

magic rawOut(raws.padded as p) {
    $"{$p.tight}"
}

int main() {
    if (magic(rawOut) { a  b } != "a  b") { return 1 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// Fragments
// ---------------------------------------------------------------------------

#[test]
fn scalar_fragments_bind_typed_values() {
    expand_run(
        r##"
grammar frags {
    skip [ ' ' ]

    rule all {
        $word w "," $ident id "," $tag t "," $int i "," $float f "," $str s
    }
}

magic fragOut(frags.all as a) {
    $"{$a.w}|{$a.id}|{$a.t}|" + $a.i + "|" + $a.f + "|" + $a.s
}

int main() {
    if (magic(fragOut) { hello, world, my-tag, 42, 2.5, "done" }
        != "hello|world|my-tag|42|2.5|done") { return 1 }
    return 0
}
"##,
    );
}

/// `$text` takes the region remainder verbatim when no tail follows
/// (§8.3.3); interpolation stringifies the capture, edge-trimmed.
#[test]
fn text_takes_the_remainder() {
    expand_run(
        r##"
grammar tail {
    skip [ ' ' ]

    rule doc {
        "intro" ":" $text body
    }
}

magic tailOut(tail.doc as d) {
    $"{$d.body}"
}

int main() {
    str got = magic(tailOut) <<END
intro: everything after, exactly as written
END
    if (got != "everything after, exactly as written") { return 1 }
    return 0
}
"##,
    );
}

/// `$template` splits the region at island delimiters into tagged
/// text/expr parts; the arms interpolate the part payloads. The doubled
/// outer brackets are the array-literal idiom (plan §1.4.7: an `each`
/// comma-joins inside literal brackets, newline-joins at statement level).
#[test]
fn template_fragments_split_at_islands() {
    expand_run(
        r##"
grammar tpl {
    skip [ ' ' ]

    rule banner {
        $template body
    }
}

magic tplOut(tpl.banner as b) {
    match ($b) {
        banner => [[each in $body {
            match ($item) {
                text => $"t({$item.text})"
                expr => $"e({$item.value})"
            }
        }]]
    }
}

int main() {
    str[] got = magic(tplOut) {
        hello {{userName}} !
    }
    if (got.length != 3) { return 1 }
    if (got[0] != "t(hello )") { return 2 }
    if (got[1] != "e(userName)") { return 3 }
    if (got[2] != "t( !)") { return 4 }
    return 0
}
"##,
    );
}

/// `$expr` is a CODE capture (§8.3.3): spliced bare it IS the expression
/// it captured; interpolated it yields the captured source text.
#[test]
fn live_islands_carry_checkmate_expressions() {
    expand_run(
        r##"
grammar live {
    skip [ ' ' ]

    rule calc {
        $expr lhs ";" $expr rhs eof
    }
}

magic calcEval(live.calc as c) {
    ($c.lhs) + ($c.rhs)
}

magic calcEcho(live.calc as c) {
    $"{$c.lhs}|{$c.rhs}"
}

int main() {
    if (magic(calcEval) { 2 + 3 * 4 ; 5 + 6 } != 25) { return 1 }
    if (magic(calcEcho) { 2 + 3 * 4 ; 5 + 6 } != "2 + 3 * 4|5 + 6") { return 2 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// Template require
// ---------------------------------------------------------------------------

#[test]
fn template_require_rejects_with_the_message() {
    let source = r##"
grammar g {
    skip [ ' ' ]
    rule num { $int v }
}

magic checked(g.num as n) {
    require($n.v > 10, "value must exceed ten")
    $n.v
}

int main() {
    return magic(checked) { 5 }
}
"##;
    expand_err(source, "value must exceed ten");
}

// ---------------------------------------------------------------------------
// Regions: islands, nesting, heredocs
// ---------------------------------------------------------------------------

/// The region scanner pierces the profile's island string form, expands
/// the nested invocation there, and the outer `$str` capture binds the
/// string's CONTENT (delimiters excluded) with the expansion already in
/// place (§8.6 nested invocation, §8.2 island strings).
#[test]
fn nested_invocations_expand_inside_islands() {
    expand_run(
        r##"
grammar inner {
    skip [ ' ' ]
    rule num { $int v }
}
magic innerNum(inner.num as n) {
    $n.v
}

grammar outer {
    skip [ ' ' ]
    string ( '`' multiline island ( "${" "}" ) )
    rule line { "say" ":" $str text }
}
magic outerSay(outer.line as l) {
    $l.text
}

int main() {
    str got = magic(outerSay) {
        say: `value ${magic(innerNum) { 41 }}!`
    }
    if (got != "value ${41}!") { return 1 }
    return 0
}
"##,
    );
}

#[test]
fn heredoc_regions_are_verbatim_to_the_tag() {
    expand_run(
        r##"
grammar free {
    skip [ ' ' ]
    rule t { $text x }
}
magic freeEcho(free.t as t) {
    $"{$t.x}"
}

int main() {
    str got = magic(freeEcho) <<RAWTEXT
  keep } braces { and " quotes exactly
RAWTEXT
    if (got != "keep } braces { and \" quotes exactly") { return 1 }
    return 0
}
"##,
    );
}

// ---------------------------------------------------------------------------
// Determinism, provenance, ordering, depth cap
// ---------------------------------------------------------------------------

#[test]
fn expansion_is_byte_deterministic_and_provenance_is_opt_in() {
    let source = r##"
grammar g {
    skip [ ' ' ]
    rule num { $int v }
}
magic showNum(g.num as n) {
    $n.v
}
int main() {
    return magic(showNum) { 7 }
}
"##;
    let a = expand_source_with(source, ExpandOptions::default()).expect("expand a");
    let b = expand_source_with(source, ExpandOptions::default()).expect("expand b");
    assert_eq!(a.expanded, b.expanded, "expansion must be byte-deterministic");
    assert_eq!(a.records.len(), b.records.len());

    let provenance =
        expand_source_with(source, ExpandOptions { provenance: true }).expect("expand p");
    assert!(
        provenance.expanded.contains("// @ magic(showNum)"),
        "provenance annotates the root site: {}",
        provenance.expanded
    );
    let parsed = cme_compiler::parse_source(&provenance.expanded);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
}

#[test]
fn undeclared_macros_are_rejected() {
    let source = r##"
int main() {
    str s = magic(nope) { x }
    return 0
}
"##;
    expand_err(source, "");
}

#[test]
fn expansion_depth_cap_reports_a_clean_diagnostic() {
    let source = r##"
grammar g {
    skip [ ' ' ]
    rule t { $text x }
}

magic loop(g.t as t) {
    magic(loop) {
        magic(loop) {
            $t.x
        }
    }
}

int main() {
    str s = magic(loop) { seed }
    return 0
}
"##;
    expand_err(source, "depth");
}
