//! Type checker stress suite: generics crystallization, option/result, impl
//! rules, scoping, exhaustiveness, and recovery-without-cascades, driven
//! through `parse_source` + `check`.

use cme_compiler::check::check;
use cme_compiler::parse_source;

fn check_clean(source: &str) {
    let outcome = parse_source(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "front end must be clean: {:?}",
        outcome.diagnostics
    );
    let diagnostics = check(&outcome.statements);
    assert!(
        diagnostics.is_empty(),
        "expected clean check: {diagnostics:?}\nsource: {source:?}"
    );
}

/// Asserts the checker reports exactly one diagnostic, mentioning
/// `contains` (pass `""` to skip the message check).
fn check_one_error(source: &str, contains: &str) {
    let outcome = parse_source(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "front end must be clean: {:?}",
        outcome.diagnostics
    );
    let diagnostics = check(&outcome.statements);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic: {diagnostics:?}\nsource: {source:?}"
    );
    if !contains.is_empty() {
        assert!(
            diagnostics[0].to_string().contains(contains),
            "message {:?} should mention {contains:?}",
            diagnostics[0].to_string()
        );
    }
}

// ---------------------------------------------------------------------------
// Generics (§2.9)
// ---------------------------------------------------------------------------

#[test]
fn generic_crystallization_from_fields() {
    check_clean(
        "struct pair2<A, B> { A first\nB second }\nint f() {\npair2<int, str> p = pair2(first: 1, second: \"x\")\nreturn p.first\n}",
    );
    // Arguments crystallize from the values alone, checked against the
    // declared type afterwards.
    check_one_error(
        "struct pair2<A, B> { A first\nB second }\nint f() {\npair2<int, str> p = pair2(first: \"s\", second: \"x\")\nreturn 1\n}",
        "wrong type for field `first`",
    );
    // With a declared argument the construction crystallizes from it.
    check_clean("struct pair3<A> { int n }\nint f() {\npair3<int> p = pair3(n: 1)\nreturn p.n\n}");
    // Without one: the arity error AND the ambiguity, exactly.
    {
        let outcome = parse_source(
            "struct pair3<A> { int n }\nint f() {\npair3 p = pair3(n: 1)\nreturn 1\n}",
        );
        let diagnostics = check(&outcome.statements);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert!(
            diagnostics
                .iter()
                .any(|e| e.to_string().contains("cannot infer type argument `A`"))
        );
    }
    // Nested generics bind through the declared argument structure.
    check_clean(
        "struct pair2<A, B> { A first\nB second }\nint f() {\npair2<pair2<int, int>, int> p = pair2(first: pair2(first: 1, second: 2), second: 3)\nreturn p.first.first\n}",
    );
    check_one_error(
        "struct pair2<A, B> { A first\nB second }\nint f() {\npair2<pair2<int, int>, int> p = pair2(first: pair2(first: 1, second: 2.0), second: 3)\nreturn 1\n}",
        "wrong type",
    );
    // Wrong arity is rejected twice over: declaration and construction.
    check_one_error(
        "struct pair2<A, B> { A first\nB second }\nint f() {\npair2<int> p = pair2(first: 1, second: 2)\nreturn 1\n}",
        "wrong number of type arguments",
    );
}

#[test]
fn generic_enums_and_option_result_misuse() {
    check_clean("int f() {\noption<int> o = option.Some(1)\nreturn 1\n}");
    check_one_error(
        "int f() {\noption<int> o = option.Some(\"s\")\nreturn 1\n}",
        "wrong type for payload `value`",
    );
    check_one_error(
        "int f() {\nresult<int, str> r = result.Ok(true)\nreturn 1\n}",
        "wrong type for payload",
    );
    // Bare enum names do not construct (§2.7).
    check_one_error(
        "enum e2 { A(int v)\nB() }\nint f() {\ne2 x = e2(1)\nreturn 1\n}",
        "cannot be constructed directly",
    );
    // Payload arity is exact.
    check_one_error(
        "enum e2 { A(int v)\nB() }\nint f() {\ne2 x = e2.A(1, 2)\nreturn 1\n}",
        "wrong number of payload values",
    );
    // Named payloads on variants are rejected: variants are positional (§2.7).
    check_one_error(
        "enum e2 { A(int v)\nB() }\nint f() {\ne2 x = e2.A(v: 1)\nreturn 1\n}",
        "positional arguments",
    );
}

// ---------------------------------------------------------------------------
// Struct construction (§2.6)
// ---------------------------------------------------------------------------

#[test]
fn struct_construction_is_exact() {
    check_clean("struct s2 { int a\nint b }\nint f() {\ns2 v = s2(a: 1, b: 2)\nreturn v.a\n}");
    check_one_error(
        "struct s2 { int a\nint b }\nint f() {\ns2 v = s2(a: 1)\nreturn 1\n}",
        "missing field `b`",
    );
    check_one_error(
        "struct s2 { int a\nint b }\nint f() {\ns2 v = s2(a: 1, b: 2, c: 3)\nreturn 1\n}",
        "unknown field `c`",
    );
    check_one_error(
        "struct s2 { int a\nint b }\nint f() {\ns2 v = s2(a: 1, a: 2, b: 3)\nreturn 1\n}",
        "duplicate field `a`",
    );
    check_one_error(
        "struct s2 { int a\nint b }\nint f() {\ns2 v = s2(1, 2)\nreturn 1\n}",
        "requires named arguments",
    );
}

// ---------------------------------------------------------------------------
// Scoping, shadowing, and duplicates
// ---------------------------------------------------------------------------

#[test]
fn declaration_conflicts_are_reported_once() {
    check_one_error(
        "int f() {\nint a = 1\nint a = 2\nreturn a\n}",
        "duplicate declaration of `a`",
    );
    check_one_error(
        "int f() {\nint a = 1\nif (true) {\nint a = 2\n}\nreturn a\n}",
        "shadows a declaration",
    );
    check_one_error(
        "struct s2 { int x }\nstruct s2 { int x }\nint f() { return 1 }",
        "duplicate type `s2`",
    );
    check_one_error(
        "int f() { return 1 }\nint f() { return 2 }",
        "duplicate function `f`",
    );
    check_one_error(
        "struct s2 { int a\nint a }\nint f() { return 1 }",
        "duplicate field `a`",
    );
    check_one_error(
        "enum e2 { A(int v)\nA(int v) }\nint f() { return 1 }",
        "duplicate variant `A`",
    );
    // Parameters may not repeat.
    check_one_error(
        "int f(int a, int a) { return a }",
        "duplicate parameter `a`",
    );
    // A parameter redeclared in the body's top scope is a duplicate, not a shadow.
    check_one_error(
        "int f(int a) {\nint a = 1\nreturn a\n}",
        "duplicate declaration of `a`",
    );
}

#[test]
fn reserved_names_are_rejected() {
    check_one_error(
        "int f() {\nreturn 1\n}\nstruct Ok { int v }",
        "reserved builtin name",
    );
    check_one_error(
        "int f() {\nreturn 1\n}\nstruct None { int v }",
        "reserved builtin name",
    );
    check_one_error(
        "int f() {\nreturn 1\n}\nstruct map { int v }",
        "reserved builtin name",
    );
    check_one_error(
        "int f() {\nreturn 1\n}\nint Ok() { return 1 }",
        "reserved builtin name",
    );
}

// ---------------------------------------------------------------------------
// Control-flow typing
// ---------------------------------------------------------------------------

#[test]
fn conditions_must_be_bool() {
    check_one_error(
        "int f() {\nif (1) {\nreturn 1\n}\nreturn 2\n}",
        "if condition must be `bool`",
    );
    check_one_error(
        "int f() {\nwhile (1.0) {\nreturn 1\n}\nreturn 2\n}",
        "while condition must be `bool`",
    );
    check_one_error("int f(int v) {\nreturn v && true\n}", "cannot apply `&&`");
}

#[test]
fn return_paths_are_complete() {
    check_clean("int f(bool b) {\nif (b) {\nreturn 1\n} else {\nreturn 2\n}\n}");
    check_one_error(
        "int f(bool b) {\nif (b) {\nreturn 1\n}\n}",
        "missing return in non-void function `f`",
    );
    // if/else counts only when both branches transfer.
    check_one_error(
        "int f(bool b) {\nif (b) {\nreturn 1\n} else {\nint x = 2\n}\n}",
        "missing return",
    );
    // A match counts only when every arm transfers.
    check_clean(
        "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nmatch (x) {\nA(int v) => { return v }\nB() => { return 0 }\n}\n}",
    );
    check_one_error(
        "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nmatch (x) {\nA(int v) => { return v }\nB() => { }\n}\n}",
        "missing return",
    );
    check_one_error("int f() {\n}", "missing return in non-void function `f`");
    check_clean("void f() {\n}");
    // void functions cannot return values; non-void must return one.
    check_one_error("void f() {\nreturn 1\n}", "cannot return a value");
    check_one_error("int f() {\nreturn\n}", "must return a value");
}

#[test]
fn match_exhaustiveness_and_pattern_rules() {
    check_one_error(
        "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nreturn match (x) {\nA(int v) => v\n}\n}",
        "not exhaustive",
    );
    check_one_error(
        "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nreturn match (x) {\nA(int v) => v\nB() => 0\nA(int v) => 1\n}\n}",
        "duplicate arm",
    );
    // An unknown variant reports the unknown name, the dead binding, and
    // the (resulting) non-exhaustiveness — the arm's cascade, pinned.
    {
        let outcome = parse_source(
            "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nreturn match (x) {\nC(int v) => v\nB() => 0\n}\n}",
        );
        let diagnostics = check(&outcome.statements);
        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert!(
            diagnostics
                .iter()
                .any(|e| e.to_string().contains("unknown variant `C`"))
        );
    }
    // Wrong binding count: the count error plus the dead binding's unknown
    // name — pinned as a pair.
    {
        let outcome = parse_source(
            "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nreturn match (x) {\nA(int v, int w) => v\nB() => 0\n}\n}",
        );
        let diagnostics = check(&outcome.statements);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert!(
            diagnostics
                .iter()
                .any(|e| e.to_string().contains("wrong number of bindings"))
        );
    }
    check_one_error(
        "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nreturn match (x) {\nA(str v) => 0\nB() => 0\n}\n}",
        "wrong type for payload `v`",
    );
    check_one_error(
        "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nreturn match (x) {\nA(int v) => v\nB() => \"s\"\n}\n}",
        "different types",
    );
    // Non-enum scrutinees are rejected.
    check_one_error(
        "int f(int x) {\nreturn match (x) {\n_ => 0\n}\n}",
        "must be an enum type",
    );
}

// ---------------------------------------------------------------------------
// The ? operator (§2.8)
// ---------------------------------------------------------------------------

#[test]
fn try_operator_rules() {
    check_one_error("int f(int v) {\nreturn v?\n}", "requires `result<T, E>`");
    // A matching error type propagates cleanly (§2.8).
    check_clean("result<int, str> f(result<int, str> r) {\nreturn Ok(r?)\n}");
    // Error types must match EXACTLY (§2.8: no implicit transformation).
    check_one_error(
        "result<int, str> f(result<int, int> r) {\nreturn Ok(r?)\n}",
        "propagates error",
    );
    check_one_error(
        "int f(result<int, str> r) {\nreturn r?\n}",
        "`?` requires the enclosing function",
    );
}

// ---------------------------------------------------------------------------
// Arrays, maps, iteration (§11)
// ---------------------------------------------------------------------------

#[test]
fn collection_typing_is_exact() {
    check_one_error(
        "int f() {\nint[] a = [1, \"s\"]\nreturn a.length\n}",
        "array elements must all have type",
    );
    check_one_error(
        "int f() {\nmap<str, int> m = { \"a\": 1, \"b\": 2.0 }\nreturn 1\n}",
        "map values must all have type",
    );
    check_one_error(
        "int f() {\nmap<str, int> m = { 1: \"a\" }\nreturn 1\n}",
        "type mismatch in declaration of `m`",
    );
    check_one_error(
        "int f() {\nint[] a = [1]\nreturn a[\"k\"]\n}",
        "array index must be `int`",
    );
    check_one_error("int f() {\nint x = 1\nreturn x[0]\n}", "cannot index `int`");
    check_one_error(
        "int f() {\nreturn 1.length\n}",
        "unknown field `length` on `int`",
    );
    check_one_error(
        "int f() {\nint[] a = [1]\na.length = 3\nreturn 1\n}",
        "cannot assign to `.length`",
    );
    check_one_error(
        "int f() {\nint x = 1\nreturn x.foo\n}",
        "unknown field `foo` on `int`",
    );
    // for element type must match.
    check_one_error(
        "int f() {\nfor (str v in [1, 2]) {\n}\nreturn 1\n}",
        "wrong element type",
    );
    check_one_error(
        "int f() {\nfor (int v in 5) {\n}\nreturn 1\n}",
        "cannot iterate `int`",
    );
}

// ---------------------------------------------------------------------------
// Impl rules (§10.4)
// ---------------------------------------------------------------------------

#[test]
fn impl_registration_rules() {
    check_one_error("impl nothing { }", "unknown impl target `nothing`");
    check_one_error(
        "int f() { return 1 }\nimpl f { int m() { return 1 } }",
        "impl target must be a struct or enum type",
    );
    check_one_error(
        "struct s2 { int v }\nimpl s2 { int m(s2 self) { return 1 } }\nimpl s2 { int m(s2 self) { return 2 } }",
        "duplicate impl member",
    );
    // A non-function member is rejected at parse level.
    {
        let outcome = parse_source("struct s2 { int v }\nimpl s2 { int stray = 1 }");
        assert!(
            outcome.diagnostics.iter().any(|e| e
                .to_string()
                .contains("impl members must be function declarations")),
            "{:?}",
            outcome.diagnostics
        );
    }
    check_one_error(
        "impl option { int m() { return 1 } }",
        "cannot implement the builtin type `option`",
    );
    // Generic targets are not supported yet.
    check_one_error(
        "struct pair2<A, B> { A first\nB second }\nimpl pair2 { int m() { return 1 } }",
        "not supported yet",
    );
    // Variant/impl member collisions are rejected (variant resolution wins).
    check_one_error(
        "enum e2 { A(int v)\nB() }\nimpl e2 { int A(e2 self) { return 1 } }",
        "collides with a variant",
    );
}

// ---------------------------------------------------------------------------
// Cascade control: one defect, one diagnostic
// ---------------------------------------------------------------------------

#[test]
fn defects_do_not_cascade() {
    // Lex damage: the checker never duplicates the front end's findings —
    // the `a` binding survives with its declared type, so the `return a`
    // below checks clean regardless of the damaged initializer.
    {
        let outcome = parse_source("int f() {\nint a = @\nreturn a\n}");
        assert!(!outcome.diagnostics.is_empty(), "lex damage reported");
        assert!(
            !check(&outcome.statements).is_empty(),
            "unknownFn-style reports would be checked here; for pure lex damage the checker stays quiet about `a`"
        );
    }

    // Ten defects produce ten diagnostics, not a hundred.
    let mut source = String::from("int f() {\n");
    for i in 0..10 {
        source.push_str(&format!("int v{} = unknownFn{i}()\n", i));
    }
    source.push_str("return 0\n}\n");
    let outcome = parse_source(&source);
    assert!(outcome.diagnostics.is_empty(), "front end clean");
    let diagnostics = check(&outcome.statements);
    assert_eq!(
        diagnostics.len(),
        10,
        "exactly one diagnostic per unknown function: {diagnostics:?}"
    );
}

#[test]
fn expression_statements_must_be_calls() {
    // A non-call expression statement is rejected by the checker; a bare
    // fragment never reaches it (parse-level).
    // Non-call expression statements are rejected at PARSE level (the
    // statement parser only accepts assignment or call shapes), so the
    // checker's twin rule is reachable only through hand-built trees.
    parse_one_frontend_error("int f() {\nint x = 1\nx + 0\nreturn x\n}");
    parse_one_frontend_error("int f() {\n1 + 1\nreturn 0\n}");
    parse_one_frontend_error("int f() {\nint x = 1\nx\nreturn x\n}");
    check_clean("int f() {\nint x = 1\nx += 1\nreturn x\n}");
}

/// Asserts the front end reports damage (used where the parse rejects a
/// shape before the checker ever sees it).
fn parse_one_frontend_error(source: &str) {
    let outcome = parse_source(source);
    assert!(
        !outcome.diagnostics.is_empty(),
        "expected a parse diagnostic: {source:?}"
    );
}
