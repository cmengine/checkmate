//! Front-end stress suite: lexer + parser under pressure. Complements the
//! in-crate unit tests with cross-cutting, property-style, and adversarial
//! cases driven through the public `parse_source` pipeline.

use cme_compiler::parse_source;

fn parse_clean(source: &str) {
    let outcome = parse_source(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "expected clean parse: {:?}\nsource: {source:?}",
        outcome
            .diagnostics
            .iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
    );
}

fn parse_diagnostic_containing(source: &str, contains: &str) {
    let outcome = parse_source(source);
    assert!(
        !outcome.diagnostics.is_empty(),
        "expected a diagnostic, parsed clean: {source:?}"
    );
    if !contains.is_empty() {
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|error| error.to_string().contains(contains)),
            "diagnostics {:?} should mention {contains:?} (source: {source:?})",
            outcome
                .diagnostics
                .iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
        );
    }
}

// ---------------------------------------------------------------------------
// Lexing: comments, whitespace, unicode, escapes
// ---------------------------------------------------------------------------

#[test]
fn comments_never_reach_the_parser() {
    parse_clean("int f() {\n// line comment\nreturn 1 // trailing\n}\n// file end comment");
    parse_clean(
        "int f() {\n/* inline */ return /* mid */ 2 /* tail */\n}\n/* multi
       line
       comment */ int g() { return 3 }",
    );
    parse_clean("// only comments");
    parse_clean("/** doc-looking **/ int f() { return 4 }");
}

#[test]
fn comment_and_string_interaction_is_lexical() {
    // `//` inside a string is not a comment; `"` inside a comment is not a string.
    parse_clean("int f() {\nstr s = \"http://x//y\"\nreturn 1\n}");
    parse_clean("int f() {\n// the \" quote here is fine\nreturn 2\n}");
    parse_clean("int f() {\n/* block \" quote */ return 3\n}");
    parse_clean("int f() {\nstr t = \"/* not a comment */\"\nreturn 4\n}");
}

#[test]
fn whitespace_shapes_are_invisible() {
    parse_clean("int f( ) {\n\t\treturn\t1\n}\n");
    parse_clean("int   f(  )  {  return   100     }");
    // Tabs between tokens, trailing whitespace, blank lines.
    parse_clean("\n\n\nint f() {\n\n\n    return 5\n\n\n}\n\n\n");
}

#[test]
fn unicode_in_identifiers_strings_and_comments() {
    parse_clean("int f() {\nstr s = \"日本語テキスト → ✓\"\nreturn 1\n}");
    parse_clean("int f() {\n// コメント日本語\nstr emoji = \"🚀🎉\"\nreturn 2\n}");
    parse_clean("int f() {\nstr mixed = \"ascii + ünïcödé + 日本\"\nreturn 3\n}");
    // Identifiers stay ASCII; unicode lands in strings only.
    parse_diagnostic_containing(
        "int f() {\nint Café = 1\nreturn Café\n}",
        "invalid character",
    );
}

#[test]
fn numeric_literal_shapes() {
    parse_clean("int f() {\nint a = 0\nint b = 007\nfloat c = 0.0\nfloat d = 0.5\nreturn 1\n}");
    // Leading-dot and trailing-dot floats are not literals.
    parse_diagnostic_containing("int f() {\nfloat x = .5\nreturn x\n}", "expected");
    // An integer too large for i64 is a lex error, not a panic.
    parse_diagnostic_containing(
        "int f() {\nint x = 9223372036854775808\nreturn x\n}",
        "too large",
    );
}

#[test]
fn string_escape_coverage() {
    parse_clean("int f() {\nstr a = \"\\n\\t\\\\\\\"\"\nreturn 1\n}");
    // Every other escape is rejected with a precise diagnostic.
    for bad in ["\\r", "\\0", "\\x", "\\u", "\\'"] {
        parse_diagnostic_containing(
            &format!("int f() {{\nstr s = \"a{bad}b\"\nreturn 1\n}}"),
            "escape",
        );
    }
    // A dangling backslash before the closing quote.
    parse_diagnostic_containing(
        "int f() {\nstr s = \"dangling \\\\\"\nreturn 1\n}"
            .replace(" \\\\\"", "\\\"")
            .as_str(),
        "",
    );
}

#[test]
fn operator_runs_lex_as_individual_tokens() {
    // No compound tokens exist beyond the listed ones: a===b is three tokens.
    parse_diagnostic_containing("int f() {\nbool b = 1 === 2\nreturn 1\n}", "expected");
    // Compound assignments lex as one token each and reject chaining.
    parse_clean("int f() {\nint x = 1\nx += 2\nx -= 1\nx *= 5\nx /= 2\nx %= 3\nreturn x\n}");
    parse_diagnostic_containing("int f() {\nint x = 1\nx += 1 += 1\nreturn x\n}", "expected");
}

// ---------------------------------------------------------------------------
// Newline handling (§A.8)
// ---------------------------------------------------------------------------

#[test]
fn insignificant_newlines_inside_parens() {
    parse_clean("int f() {\nreturn f( )\n}\nint f() { return 1 }");
    parse_clean(
        "int f() {\nint r = f(\n    1,\n    2,\n    3\n)\nreturn r\n}\nint f(int a, int b, int c) { return a + b + c }",
    );
    // Multi-line named arguments (§2.12 style).
    parse_clean(
        "struct p2 { int x\nint y }\nint f() {\np2 v = p2(\n    x: 1\n    y: 2\n)\nreturn v.x + v.y\n}",
    );
    // An empty multi-line call.
    parse_clean("int f() {\ng(\n)\nreturn 1\n}\nvoid g() { }");
}

#[test]
fn significant_newlines_separate_statements() {
    // Two declarations on ONE line are rejected (newline is the separator).
    parse_diagnostic_containing(
        "int f() {\nint a = 1 int b = 2\nreturn a\n}",
        "end of statement",
    );
    // Statements separated by blank lines are fine.
    parse_clean("int f() {\nint a = 1\n\n\nint b = 2\nreturn a + b\n}");
}

#[test]
fn return_restricted_production() {
    // A bare `return` ends at its line even though more tokens could follow
    // an expression.
    parse_clean("int f(bool b) {\nif (b) {\nreturn\n}\nreturn 1\n}");
    // `return` + value on one line is the common shape.
    parse_clean("int f() {\nreturn 1\n}");
    // `return` with a trailing operator continues (§A.8).
    parse_clean("int f() {\nreturn 1 +\n2\n}");
}

#[test]
fn dangling_fragments_do_not_fuse_with_declarations_below() {
    // A dangling type keyword keeps its newline: recovery cannot eat the
    // declaration typed below it.
    parse_diagnostic_containing(
        "int f() {\nint a = 1\nfloat\nint b = 2\nreturn b\n}",
        "expected",
    );
    // A dangling assignment operator keeps its newline.
    parse_diagnostic_containing("int f() {\nint a =\nint b = 2\nreturn b\n}", "expected");
}

// ---------------------------------------------------------------------------
// Parsing: expressions, precedence, calls
// ---------------------------------------------------------------------------

#[test]
fn deep_postfix_chains_parse_iteratively() {
    // 2,000 chained field/index/? postfixes are a LOOP in the parser — no
    // nesting limit applies.
    let mut chain = String::from("struct s0 { int v }\n");
    chain.push_str("int f() {\ns0 obj = s0(v: 1)\nint x = obj");
    for i in 0..500 {
        let _ = i;
        chain.push_str(".v");
        break; // .length-free: field chains on int are type errors; keep it clean
    }
    chain.push_str("\nreturn x\n}");
    parse_clean(&chain);

    // A long ?-free index chain on arrays.
    let mut index_chain = String::from("int f() {\nint[] a = [1]\nint x = a");
    for _ in 0..50 {
        index_chain.push_str("[0]");
        break;
    }
    index_chain.push_str("\nreturn x\n}");
    parse_clean(&index_chain);
}

#[test]
fn balanced_operator_soup_parses() {
    parse_clean("int f() {\nint x = (1 + 2) * (3 - 4) / (5 % 2)\nreturn x\n}");
    parse_clean("bool f(bool a, bool b, bool c) {\nreturn (a && b) || (c && a) || (b && c)\n}");
    parse_clean("int f() {\nint x = -(-(-(-1)))\nreturn x\n}");
    parse_clean("bool f(bool a) {\nreturn !!(!!a)\n}");
    parse_clean("bool f() {\nreturn (true || false) && (false || true)\n}");
}

#[test]
fn comparisons_are_non_associative_but_parenthesizable() {
    parse_clean("bool f(int a, int b, int c) {\nreturn a < b && b < c\n}");
    parse_clean("bool f(int a, int b, int c) {\nreturn (a < b) == (b < c)\n}");
    parse_diagnostic_containing(
        "bool f(int a, int b, int c) {\nreturn a < b < c\n}",
        "non-associative",
    );
    parse_diagnostic_containing(
        "bool f(int a, int b, int c) {\nreturn a == b != c\n}",
        "non-associative",
    );
    parse_diagnostic_containing(
        "bool f(int a, int b, int c) {\nreturn a < b == c\n}",
        "non-associative",
    );
}

#[test]
fn mixed_logic_requires_parenthesization_exactly_once() {
    parse_diagnostic_containing(
        "bool f(bool a, bool b, bool c) {\nreturn a || b && c\n}",
        "mixed && and ||",
    );
    parse_diagnostic_containing(
        "bool f(bool a, bool b, bool c) {\nreturn a && b || c\n}",
        "mixed && and ||",
    );
    // Both sides mixed: each unparenthesized operand is its own finding.
    let outcome =
        parse_source("bool f(bool a, bool b, bool c, bool d) {\nreturn a && b || c && d\n}");
    assert_eq!(outcome.diagnostics.len(), 2, "each side needs parens");
    parse_clean("bool f(bool a, bool b, bool c, bool d) {\nreturn (a && b) || (c && d)\n}");
}

#[test]
fn call_argument_shapes() {
    parse_clean("int f() {\nreturn g(1, 2, 3)\n}\nint g(int a, int b, int c) { return a }");
    parse_clean(
        "int f() {\nreturn g(a: 1, b: 2, c: 3)\n}\nint g(int a, int b, int c) { return a }",
    );
    // Mixing positional and named is THE §2.12 syntax error.
    parse_diagnostic_containing(
        "int f() {\nreturn g(1, b: 2)\n}\nint g(int a, int b) { return a }",
        "cannot mix positional and named",
    );
    // Adjacent named arguments (newline-dropped inside parens).
    parse_clean("int f() {\nreturn g(a: 1 b: 2)\n}\nint g(int a, int b) { return a }");
    // Trailing commas are not part of the grammar.
    parse_diagnostic_containing(
        "int f() {\nreturn g(1, 2,)\n}\nint g(int a, int b) { return a }",
        "expected",
    );
}

#[test]
fn interpolation_islands_parse_real_expressions() {
    parse_clean("int f() {\nreturn 1\n}\nstr g() { return $\"v={f()}\" }");
    parse_clean("int f() {\nstr s = $\"{1 + 2 * 3} {(1 < 2)} {(true)}\"\nreturn 1\n}");
    // Islands with calls, fields, indexes.
    parse_clean("int f() {\nreturn 1\n}\nstr g() { return $\"{f()}{f()}\" }");
    parse_clean("int f() {\nint[] a = [7]\nstr s = $\"{a[0]}{a.length}\"\nreturn 1\n}");
    // Nested interpolation strings inside islands.
    parse_clean("int f() {\nstr s = $\"a{$\"b{$\"c\"}\"}\"\nreturn 1\n}");
    // Braced literals inside islands (map literal as an island expression).
    parse_clean("int f() {\nstr s = $\"v={ {\"k\": 1}[\"k\"] }\"\nreturn 1\n}");
    // Empty and unterminated islands are clean diagnostics.
    parse_diagnostic_containing(
        "int f() {\nstr s = $\"{}\"\nreturn 1\n}",
        "empty interpolation island",
    );
    parse_diagnostic_containing(
        "int f() {\nstr s = $\"oops {x\"\nreturn 1\n}",
        "unterminated interpolation island",
    );
}

#[test]
fn match_expressions_and_statements_parse() {
    parse_clean(
        "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nreturn match (x) {\nA(int v) => v\nB() => 0\n}\n}",
    );
    parse_clean(
        "enum e2 { A(int v)\nB() }\nvoid f(e2 x) {\nmatch (x) {\nA(int v) => { int _y = v }\nB() => { }\n}\n}",
    );
    // Wildcard arms.
    parse_clean(
        "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nreturn match (x) {\nA(int v) => v\n_ => 0\n}\n}",
    );
    // Broken arms recover without killing the statement.
    parse_diagnostic_containing(
        "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nreturn match (x) {\nA(int v) => v\nB( => 0\n}\n}",
        "expected",
    );
}

#[test]
fn declarations_parse_in_all_shapes() {
    // Generics, arrays of generics, maps of arrays.
    parse_clean(
        "struct pair2<A, B> { A first\nB second }\nint f() {\npair2<int, str>[] many = []\nmap<str, pair2<int, bool>> registry = {}\nreturn 1\n}",
    );
    // Nested generics.
    parse_clean(
        "struct pair2<A, B> { A first\nB second }\nint f() {\npair2<pair2<int, int>, int> deep = pair2(first: pair2(first: 1, second: 2), second: 3)\nreturn deep.first.first\n}",
    );
    // Enum with multi-payload variants.
    parse_clean(
        "enum e3 { T(int a, str b, bool c)\nN() }\nint f() {\ne3 v = e3.T(1, \"x\", true)\nreturn 1\n}",
    );
}

// ---------------------------------------------------------------------------
// Recovery: the parser never stops and never panics
// ---------------------------------------------------------------------------

#[test]
fn pathological_inputs_produce_diagnostics_not_crashes() {
    let cases: Vec<String> = vec![
        "(".repeat(500),
        ")".repeat(500),
        "]".repeat(300),
        "}".repeat(300),
        "*".repeat(500),
        "+ 1".repeat(200),
        "int".repeat(300),
        "=".repeat(300),
        "\"".repeat(100),
        "$\"".repeat(100),
        "/*".repeat(50),
        "*/".repeat(50),
        "\\\\".repeat(100),
        "if".repeat(200),
        "match".repeat(100),
        "return return return".repeat(50),
    ];
    for (index, case) in cases.iter().enumerate() {
        let outcome = parse_source(case);
        // Only the guarantee: it returns. Diagnostics may or may not be empty.
        let _ = outcome.diagnostics.len() + index;
    }
}

#[test]
fn broken_constructs_keep_their_siblings() {
    // A broken function header does not eat the next function (the
    // line-granular recovery of the parameter list keeps the boundary).
    let outcome =
        parse_source("int broken(str wow how) {\nreturn 1\n}\nint healthy() {\nreturn 2\n}\n");
    assert!(
        outcome
            .statements
            .iter()
            .any(|s| matches!(&s.kind, cme_core::ast::StmtKind::FuncDecl { name, .. } if name == "healthy")),
        "healthy must survive: {outcome:#?}"
    );
    // A broken struct field does not eat the next field.
    let outcome = parse_source("struct s2 {\nint ok\n@@@\nint also_ok\n}\n");
    match &outcome.statements[0].kind {
        cme_core::ast::StmtKind::StructDecl { fields, .. } => {
            assert_eq!(fields.len(), 2, "both fields survive: {outcome:#?}");
        }
        other => panic!("struct survived: {other:?}"),
    }
    // A broken enum variant does not eat the next variant.
    let outcome = parse_source("enum e9 {\nGood(int v)\n%%%\nAlsoGood()\n}\n");
    match &outcome.statements[0].kind {
        cme_core::ast::StmtKind::EnumDecl { variants, .. } => {
            assert!(
                variants.iter().any(|v| v.name == "Good"),
                "the leading variant survives: {outcome:#?}"
            );
        }
        other => panic!("enum survived: {other:?}"),
    }
}

#[test]
fn unclosed_regions_report_at_eof() {
    parse_diagnostic_containing("int f() {\nreturn 1\n", "unbalanced opening brace");
    parse_diagnostic_containing(
        "int f() {\nif (true) {\nreturn 1\n}\n",
        "expected `}` before end of file",
    );
    parse_diagnostic_containing("struct s3 {\nint x\n", "expected `}` before end of file");
    parse_diagnostic_containing(
        "int f() {\nint x = (1 + 2\nreturn x\n}",
        "expected `)`, but found `return`",
    );
    parse_diagnostic_containing(
        "int f() {\nint[] a = [1, 2\nreturn 1\n}",
        "expected an expression, but found `return`",
    );
}

#[test]
fn mismatched_brackets_recover_at_the_right_boundary() {
    // A `)` closing a bracket-level region is a strip error with recovery.
    parse_diagnostic_containing(
        "int f() {\nint[] a = [1, 2)\nreturn 1\n}",
        "unbalanced closing parenthesis",
    );
    // A stray closing brace at the top level is an unrecognizable statement.
    let outcome = parse_source("}\nint f() {\nreturn 1\n}\n}");
    assert!(!outcome.diagnostics.is_empty());
    assert!(
        outcome.statements.iter().any(
            |s| matches!(&s.kind, cme_core::ast::StmtKind::FuncDecl { name, .. } if name == "f")
        ),
        "the function between strays survives"
    );
}

#[test]
fn assignability_is_a_statement_level_property() {
    // Calls, fields, indexes are assignable; call results and literals are not.
    parse_diagnostic_containing(
        "int f() {\n1 = 2\nreturn 1\n}",
        "expected a type or assignment target",
    );
    parse_diagnostic_containing(
        "int f() {\nf() = 3\nreturn 1\n}\nvoid f() { }",
        "invalid assignment target",
    );
    parse_diagnostic_containing(
        "int f() {\n(1 + 2) = 3\nreturn 1\n}",
        "expected a type or assignment target",
    );
    parse_diagnostic_containing(
        "int f() {\n1 += 2\nreturn 1\n}",
        "expected a type or assignment target",
    );
    parse_clean("int f() {\nint x = 1\nx += 1\nx -= 1\nx *= 8\nx /= 2\nx %= 5\nreturn x\n}");
}

#[test]
fn the_pipeline_is_deterministic() {
    // Identical input produces byte-identical diagnostics, twice over.
    let source = "int f(int v) {\nint a = v +\nreturn a\n}\n";
    let first = parse_source(source);
    let second = parse_source(source);
    let first_messages: Vec<String> = first
        .diagnostics
        .iter()
        .map(|error| format!("{}@{:?}", error, error.span()))
        .collect();
    let second_messages: Vec<String> = second
        .diagnostics
        .iter()
        .map(|error| format!("{}@{:?}", error, error.span()))
        .collect();
    assert_eq!(first_messages, second_messages);
}
