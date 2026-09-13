//! End-to-end pipeline tests: Checkmate source parses and checks with
//! the real front end, then runs on the tree-walking interpreter.
//!
//! These live in the compiler crate (rather than `cme-interp` unit tests)
//! so `cme-interp` needs no dev-dependency on `cme-compiler`: the crates
//! would otherwise form a publish-blocking dependency cycle.

mod tests {
    use cme_core::Span;
    use cme_interp::{
        CapabilityHost, InterpError, InterpErrorKind, Interpreter, MAX_CALL_DEPTH, Value,
    };
    use std::cell::Cell;

    /// The full pipeline with the same gate a host applies: the source must
    /// parse AND check clean before the interpreter runs.
    fn run_main(source: &str) -> Result<Value, InterpError> {
        let outcome = cme_compiler::parse_source(source);
        assert!(
            outcome.is_clean(),
            "test source must parse clean: {:?}",
            outcome.diagnostics
        );
        let diagnostics = cme_compiler::check::check(&outcome.statements);
        assert!(
            diagnostics.is_empty(),
            "test source must check clean: {diagnostics:?}"
        );
        Interpreter::new(&outcome.statements).invoke("main", &[])
    }

    /// Parse-only pipeline for defensive pins: the source parses clean but
    /// the checker would reject it, so the interpreter must raise a clean
    /// error (never panic) when it meets the bad shape.
    fn run_ungated(source: &str) -> Result<Value, InterpError> {
        let outcome = cme_compiler::parse_source(source);
        assert!(
            outcome.is_clean(),
            "test source must parse clean: {:?}",
            outcome.diagnostics
        );
        Interpreter::new(&outcome.statements).invoke("main", &[])
    }

    /// Parse-only statements for tests that invoke something other than
    /// `main` (fuel metering, direct function calls).
    fn parse_statements_for_interp(source: &str) -> Vec<cme_core::ast::Stmt> {
        let outcome = cme_compiler::parse_source(source);
        assert!(
            outcome.is_clean(),
            "test source must parse clean: {:?}",
            outcome.diagnostics
        );
        outcome.statements
    }

    fn ok(source: &str) -> Value {
        run_main(source).expect("test program should run to completion")
    }

    /// Span helper: `source[start..end]` located by substring.
    fn span_of(source: &str, needle: &str) -> Span {
        let start = source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in source"));
        Span::new(start, start + needle.len())
    }

    #[test]
    fn value_display_matches_canonical_forms() {
        assert_eq!(Value::Int(-42).to_string(), "-42");
        assert_eq!(Value::Int(0).to_string(), "0");
        assert_eq!(Value::Float(3.75).to_string(), "3.75");
        // Shortest round-trip: trailing ".0" is dropped, imprecision is
        // shown exactly.
        assert_eq!(Value::Float(3.0).to_string(), "3");
        assert_eq!(Value::Float(0.1 + 0.2).to_string(), "0.30000000000000004");
        assert_eq!(Value::Bool(true).to_string(), "true");
        assert_eq!(Value::Bool(false).to_string(), "false");
        assert_eq!(Value::Str("hp".to_string()).to_string(), "hp");
        assert_eq!(Value::Void.to_string(), "");
    }

    #[test]
    fn integer_division_and_remainder_truncate_toward_zero() {
        // §A.5: 7 / 2 is 3, -7 / 2 is -3, -7 % 2 is -1, 7 % -2 is 1.
        assert_eq!(ok("int main() {\nreturn 7 / 2\n}\n"), Value::Int(3));
        assert_eq!(ok("int main() {\nreturn -7 / 2\n}\n"), Value::Int(-3));
        assert_eq!(ok("int main() {\nreturn -7 % 2\n}\n"), Value::Int(-1));
        assert_eq!(ok("int main() {\nreturn 7 % -2\n}\n"), Value::Int(1));
    }

    #[test]
    fn logical_operators_short_circuit() {
        // `boom()` errors at runtime if it is ever evaluated; the programs
        // only complete when && and || skip the right operand.
        let source = "bool boom() {\nreturn 1 / 0 == 1\n}\nint main() {\nbool both = false && boom()\nbool either = true || boom()\nif (both) {\nreturn 1\n}\nif (!either) {\nreturn 2\n}\nreturn 0\n}\n";
        assert_eq!(ok(source), Value::Int(0));

        // The right operand IS evaluated when the left one does not
        // decide: the error from boom() propagates.
        let source = "bool boom() {\nreturn 1 / 0 == 1\n}\nint main() {\nbool trapped = true && boom()\nreturn 0\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer division by zero");
    }

    #[test]
    fn string_concatenation_uses_canonical_forms() {
        // §A.6 examples, including the left-associativity consequences.
        assert_eq!(
            ok("str main() {\nreturn \"HP: \" + 100\n}\n"),
            Value::Str("HP: 100".to_string())
        );
        assert_eq!(
            ok("str main() {\nreturn \"ok: \" + true\n}\n"),
            Value::Str("ok: true".to_string())
        );
        assert_eq!(
            ok("str main() {\nreturn 1.5 + \"x\"\n}\n"),
            Value::Str("1.5x".to_string())
        );
        assert_eq!(
            ok("str main() {\nreturn \"a\" + 1 + 2\n}\n"),
            Value::Str("a12".to_string())
        );
        assert_eq!(
            ok("str main() {\nreturn 1 + 2 + \"a\"\n}\n"),
            Value::Str("3a".to_string())
        );
        // A str on either side stringifies the other; never the reverse.
        assert_eq!(
            ok("str main() {\nreturn 100 + \"!\"\n}\n"),
            Value::Str("100!".to_string())
        );
    }

    #[test]
    fn integer_division_and_remainder_by_zero_are_runtime_errors() {
        let source = "int main() {\nreturn 1 / 0\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer division by zero");
        assert_eq!(error.span, span_of(source, "1 / 0"));

        let source = "int main() {\nreturn 1 % 0\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer remainder by zero");
        assert_eq!(error.span, span_of(source, "1 % 0"));

        // Float division by zero is ordinary IEEE 754: not an error.
        assert_eq!(
            ok("int main() {\nfloat inf = 1.0 / 0.0\nif (inf > 0.0) {\nreturn 1\n}\nreturn 0\n}\n"),
            Value::Int(1)
        );
    }

    #[test]
    fn arithmetic_overflow_terminates_the_invocation() {
        // The pinned case: i64::MAX + 1 via compound assignment.
        let source = "int main() {\nint x = 9223372036854775807\nx += 1\nreturn x\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer overflow in `+`");
        assert_eq!(error.span, span_of(source, "x += 1"));

        // Negating i64::MIN.
        let source = "int main() {\nint min = -9223372036854775807 - 1\nreturn -min\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer overflow in `-`");

        // i64::MIN / -1 is the one overflowing division.
        let source = "int main() {\nint min = -9223372036854775807 - 1\nreturn min / -1\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer overflow in `/`");

        // i64::MIN % -1 overflows the remainder too; checked, never a panic.
        let source = "int main() {\nint min = -9223372036854775807 - 1\nreturn min % -1\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer overflow in `%`");

        // Multiplication wraps into checked territory as well.
        let source = "int main() {\nint big = 3037000500\nbig *= big\nreturn big\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer overflow in `*`");
    }

    #[test]
    fn while_bodies_rebind_fresh_each_iteration() {
        // One fresh frame per loop iteration: `per` is (re)declared in a
        // new frame every time the body executes, with the initializer
        // evaluated per iteration (0, 10, 20).
        let source = "int main() {\nint i = 0\nint total = 0\nwhile (i < 3) {\nint per = i * 10\ntotal = total + per\ni += 1\n}\nreturn total\n}\n";
        assert_eq!(ok(source), Value::Int(30));
    }

    #[test]
    fn return_early_from_inside_a_while() {
        let source = "int main() {\nint i = 0\nwhile (true) {\nif (i == 3) {\nreturn i\n}\ni += 1\n}\nreturn 0\n}\n";
        assert_eq!(ok(source), Value::Int(3));
    }

    #[test]
    fn recursion_computes_fibonacci() {
        let source = "int fib(int n) {\nif (n <= 1) {\nreturn n\n}\nreturn fib(n - 1) + fib(n - 2)\n}\nint main() {\nreturn fib(10)\n}\n";
        assert_eq!(ok(source), Value::Int(55));
    }

    #[test]
    fn infinite_recursion_hits_the_depth_limit_cleanly() {
        let source = "int spin() {\nreturn spin()\n}\nint main() {\nreturn spin()\n}\n";
        // 1024 nested CME frames each occupy several native Rust frames in
        // a debug build, more than a default test thread's stack can hold.
        // Run the invocation on a dedicated thread with a generous stack so
        // the depth guard — not the native stack — is what stops the
        // recursion. (A host embedding the interpreter must similarly
        // provide adequate stack for `MAX_CALL_DEPTH`-deep recursion, or
        // configure a lower limit once the Engine API allows it.)
        let handle = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || run_main(source))
            .expect("spawn the depth-limit thread");
        let result = handle.join().expect("depth-limit thread must not panic");
        let error = result.unwrap_err();
        assert_eq!(
            error.message,
            format!("call depth limit of {MAX_CALL_DEPTH} exceeded")
        );
    }

    #[test]
    fn call_statements_discard_results_silently() {
        // A call statement may discard a non-void result (owner ruling).
        let source = "int double(int value) {\nreturn value * 2\n}\nint main() {\ndouble(21)\nreturn 21\n}\n";
        assert_eq!(ok(source), Value::Int(21));

        // Void calls run their body and return nothing printable.
        let source = "void emit(int value) {\nint doubled = value * 2\n}\nint main() {\nemit(21)\nreturn 5\n}\n";
        assert_eq!(ok(source), Value::Int(5));
    }

    #[test]
    fn float_equality_follows_ieee_754() {
        // NaN == NaN is false; NaN arises from 0.0 / 0.0.
        let source = "int main() {\nfloat nan = 0.0 / 0.0\nif (nan == nan) {\nreturn 1\n}\nif (nan != nan) {\nreturn 2\n}\nreturn 0\n}\n";
        assert_eq!(ok(source), Value::Int(2));
    }

    #[test]
    fn invoke_rejects_unknown_functions_and_arity_mismatches() {
        let source = "int add(int a, int b) {\nreturn a + b\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let interpreter = Interpreter::new(&outcome.statements);

        let error = interpreter.invoke("missing", &[]).unwrap_err();
        assert_eq!(error.message, "unknown function `missing`");

        let error = interpreter.invoke("add", &[Value::Int(1)]).unwrap_err();
        assert_eq!(
            error.message,
            "wrong number of arguments to `add`: expected 2, found 1"
        );

        assert_eq!(
            interpreter.invoke("add", &[Value::Int(1), Value::Int(2)]),
            Ok(Value::Int(3))
        );
    }

    #[test]
    fn wrong_condition_shapes_raise_errors_not_panics() {
        // Checker-bug shapes: an int condition must produce a clean
        // InterpError, never a panic.
        let source = "int main() {\nif (7) {\nreturn 1\n}\nreturn 0\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(error.message, "if condition must be `bool`, found `int`");

        let source = "int main() {\nwhile (7) {\nreturn 1\n}\nreturn 0\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(error.message, "while condition must be `bool`, found `int`");
    }

    #[test]
    fn falling_off_a_non_void_function_raises_an_error() {
        // The checker's structural return analysis would reject this; the
        // interpreter defends itself anyway.
        let source = "int leak() {\nint unused = 1\n}\nint main() {\nreturn leak()\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(
            error.message,
            "non-void function `leak` fell off the end without returning a value"
        );
    }

    #[test]
    fn assignment_to_undeclared_name_is_a_defensive_error() {
        let source = "int main() {\nghost = 1\nreturn 0\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(error.message, "assignment to undeclared name `ghost`");
    }

    #[test]
    fn calls_to_unknown_functions_are_defensive_errors() {
        let source = "int main() {\nreturn missing(1)\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(error.message, "unknown function `missing`");

        let source = "int main() {\nreturn helper()\n}\nint helper() {\nreturn missing()\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(error.message, "unknown function `missing`");
    }

    // ------------------------------------------------------------------
    // Full-surface runtime: structs, enums, match, for, collections,
    // interpolation, and ?.
    // ------------------------------------------------------------------

    fn ok_full(source: &str) -> Value {
        run_main(source).expect("test program should run to completion")
    }

    #[test]
    fn struct_construction_and_field_semantics() {
        // §2.6 / §2.13: named-field construction, field mutation, and the
        // caller-isolated copy behavior of damage.
        let source = "struct vec2 {\n    float x\n    float y\n}\nstruct player {\n    str name\n    int health\n    bool alive\n}\nplayer damage(player p, int amount) {\np.health = p.health - amount\nif (p.health <= 0) {\np.alive = false\n}\nreturn p\n}\nint main() {\nplayer hero = player(name: \"Hero\", health: 100, alive: true)\nplayer hurt = damage(hero, 30)\nif (hero.health == 100) {\nif (hurt.health == 70) {\nif (hurt.alive == hero.alive) {\nreturn 1\n}\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn deep_field_and_index_assignment() {
        // §2.13 / §A.7: nested chains assign through the resolved path.
        let source = "struct vec2 {\n    float x\n    float y\n}\nstruct party {\n    int[] scores\n    vec2 base\n}\nint main() {\nparty squad = party(\n    scores: [10, 20, 30]\n    base: vec2(x: 1.0, y: 2.0)\n)\nsquad.scores[1] += 5\nsquad.base.x = 40.0\nif (squad.scores[1] == 25) {\nif (squad.base.x == 40.0) {\nreturn 1\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn struct_equality_is_structural() {
        let source = "struct vec2 {\n    float x\n    float y\n}\nint main() {\nvec2 a = vec2(x: 1.0, y: 2.0)\nvec2 b = vec2(x: 1.0, y: 2.0)\nvec2 c = vec2(x: 9.0, y: 2.0)\nif (a == b) {\nif (a != c) {\nreturn 1\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn enum_match_dispatches_and_destructures() {
        let source = "struct vec2 {\n    float x\n    float y\n}\nenum gameEvent {\n    Damage(int amount)\n    Spawn(str kind, vec2 position)\n    PlayerDied()\n}\nstr describe(gameEvent evt) {\nreturn match (evt) {\n    Damage(int amount) => \"d:\" + amount\n    Spawn(str kind, vec2 position) => \"s:\" + kind + \":\" + position.x\n    PlayerDied() => \"dead\"\n}\n}\nint main() {\nstr a = describe(gameEvent.Damage(25))\nstr b = describe(gameEvent.Spawn(\"orc\", vec2(x: 3.0, y: 1.0)))\nstr c = describe(gameEvent.PlayerDied())\nif (a == \"d:25\") {\nif (b == \"s:orc:3\") {\nif (c == \"dead\") {\nreturn 1\n}\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn try_operator_propagates_through_call_boundaries() {
        // §2.8: ? inside chain returns Err from chain; the caller observes
        // it as an ordinary value.
        let source = "result<int, str> safeDiv(int a, int b) {\nif (b == 0) {\nreturn Err(\"div0\")\n}\nreturn Ok(a / b)\n}\nresult<int, str> chain(int a, int b) {\nint v = safeDiv(a, b)?\nreturn Ok(v * 10)\n}\nint main() {\nresult<int, str> good = chain(8, 2)\nresult<int, str> bad = chain(8, 0)\nmatch (good) {\n    Ok(int v) => {\n        match (bad) {\n            Ok(int w) => { return 0 }\n            Err(str reason) => { if (reason == \"div0\") { return 1 } }\n        }\n    }\n    Err(str reason) => { return 0 }\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn arrays_iterate_index_and_clone() {
        // §11 / §2.13 / §2.14.
        let source = "int sum(int[] xs) {\nint total = 0\nfor (int v in xs) {\ntotal += v\n}\nreturn total\n}\nint main() {\nint[] a = [1, 2, 3, 4]\nint[] copy = a\ncopy[0] = 99\nif (a[0] == 1) {\nif (a.length == 4) {\nif (sum(a) == 10) {\nreturn 1\n}\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn map_entries_read_write_and_insert() {
        let source = "int main() {\nmap<str, int> m = {\n\"gold\": 120\n\"gems\": 3\n}\nm[\"gold\"] += 30\nm[\"arrows\"] = 60\nif (m[\"gold\"] == 150) {\nif (m[\"arrows\"] == 60) {\nreturn 1\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn map_equality_is_order_insensitive() {
        let source = "int main() {\nmap<str, int> a = {\"x\": 1\n\"y\": 2\n}\nmap<str, int> b = {\"y\": 2\n\"x\": 1\n}\nif (a == b) {\nreturn 1\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn interpolated_strings_evaluate_islands() {
        // §2.8/§4.1 + §A.6 canonical forms.
        let source = "struct vec2 {\n    float x\n    float y\n}\nint fib(int n) {\nif (n <= 1) {\nreturn n\n}\nreturn fib(n - 1) + fib(n - 2)\n}\nstr main() {\nint hp = 100\nvec2 p = vec2(x: 3.5, y: -1.5)\nbool armed = true\nstr s = $\"hp={hp} pos=({p.x},{p.y}) next={fib(7)} armed={armed}\"\nreturn s\n}\n";
        assert_eq!(
            ok_full(source),
            Value::Str("hp=100 pos=(3.5,-1.5) next=13 armed=true".to_string())
        );
    }

    #[test]
    fn out_of_bounds_and_missing_key_are_clean_errors() {
        let source = "int main() {\nint[] a = [1, 2]\nreturn a[5]\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "array index 5 out of bounds (length 2)");

        let source = "int main() {\nmap<str, int> m = {\"a\": 1\n}\nreturn m[\"b\"]\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "map key b not found");
    }

    #[test]
    fn struct_values_display_cmon_style() {
        // §11.1: the Display of a struct value is CMON-shaped. Structs do
        // not interpolate (§A.6), so the value comes back directly.
        let source = "struct vec2 {\n    float x\n    float y\n}\nstruct box {\n    vec2 corner\n    int[] items\n}\nbox main() {\nbox b = box(\n    corner: vec2(x: 1.0, y: 2.0)\n    items: [7, 8]\n)\nreturn b\n}\n";
        let value = ok_full(source);
        assert_eq!(
            value.to_string(),
            "box(corner: vec2(x: 1, y: 2), items: [7, 8])"
        );
    }

    #[test]
    fn enum_and_array_values_display_cmon_style() {
        let source = "enum gameEvent {\n    Damage(int amount)\n}\ngameEvent main() {\nreturn gameEvent.Damage(25)\n}\n";
        assert_eq!(ok_full(source).to_string(), "gameEvent.Damage(25)");

        let source = "int[] main() {\nreturn [1, 2]\n}\n";
        assert_eq!(ok_full(source).to_string(), "[1, 2]");

        let source = "map<str, int> main() {\nreturn {\"a\": 1\n\"b\": 2\n}\n}\n";
        assert_eq!(ok_full(source).to_string(), "{a: 1, b: 2}");
    }

    #[test]
    fn value_semantics_for_arrays_passed_to_functions() {
        // resetFirst mutates its own copy; the caller's array is untouched.
        let source = "void resetFirst(int[] xs) {\nxs[0] = 0\n}\nint main() {\nint[] a = [1, 2, 3]\nresetFirst(a)\nif (a[0] == 1) {\nreturn 1\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    // ------------------------------------------------------------------
    // §10.4 — impl blocks
    // ------------------------------------------------------------------

    #[test]
    fn impl_members_on_structs_execute() {
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int peek(counter c) {\n        return c.value\n    }\n}\nint main() {\ncounter c = counter(value: 41)\nreturn counter.peek(c)\n}\n";
        assert_eq!(ok_full(source), Value::Int(41));
    }

    #[test]
    fn impl_member_value_semantics_never_escape_the_caller() {
        // bump mutates its own clone; the caller's counter is untouched
        // (§2.13), and bump returns the incremented copy.
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    counter bump(counter c) {\n        c.value += 1\n        return c\n    }\n}\nint main() {\ncounter c = counter(value: 41)\ncounter bumped = counter.bump(c)\nif (c.value != 41) {\nreturn 0\n}\nreturn bumped.value\n}\n";
        assert_eq!(ok_full(source), Value::Int(42));
    }

    #[test]
    fn impl_blocks_union_and_members_call_each_other() {
        // Two blocks for the same target; a member of the second calls a
        // member of the first (forward reference across blocks, §10.4).
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int peek(counter c) {\n        return c.value\n    }\n}\nimpl counter {\n    int peekTwice(counter c) {\n        return counter.peek(c) + counter.peek(c)\n    }\n}\nint main() {\ncounter c = counter(value: 21)\nreturn counter.peekTwice(c)\n}\n";
        assert_eq!(ok_full(source), Value::Int(42));
    }

    #[test]
    fn impl_members_on_enums_execute() {
        let source = "enum suit {\n    Clubs()\n    Hearts()\n}\nimpl suit {\n    str label(suit s) {\nstr name = match (s) {\n    Clubs() => \"clubs\"\n    Hearts() => \"hearts\"\n}\nreturn name\n    }\n}\nint main() {\nif (suit.label(suit.Clubs()) == \"clubs\") {\nreturn 1\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn host_style_path_impl_members_execute() {
        // §10.4 + §2.13: a member that updates its state parameter returns
        // the updated value (a void member's mutation would be silently
        // lost — the checker now errors on exactly that shape), and the
        // caller reassigns to observe it.
        let source = "struct GameConfig {\n    int startingScore\n}\nstruct GameState {\n    int score\n}\nimpl engine.gamemode {\n    GameState InitGame(GameConfig config) {\n        return GameState(score: config.startingScore)\n    }\n    GameState OnTick(GameState state) {\n        state.score += 1\n        return state\n    }\n}\nint main() {\nGameConfig config = GameConfig(startingScore: 100)\nGameState state = engine.gamemode.InitGame(config)\nstate = engine.gamemode.OnTick(state)\nreturn state.score\n}\n";
        assert_eq!(ok_full(source), Value::Int(101));
    }

    #[test]
    fn impl_members_accept_named_arguments_bound_by_name() {
        // Named arguments bind by NAME (§2.12): the out-of-order call still
        // binds low/high correctly, for impl members and plain functions.
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int clampAround(counter c, int low, int high) {\nif (c.value < low) {\n    return low\n}\nif (c.value > high) {\n    return high\n}\nreturn c.value\n    }\n}\nint main() {\ncounter c = counter(value: 50)\nreturn counter.clampAround(high: 10, low: 0, c: c)\n}\n";
        assert_eq!(ok_full(source), Value::Int(10));
    }

    #[test]
    fn plain_function_named_arguments_bind_by_name() {
        // The same fix applies to top-level functions: evaluation order
        // stays source-left-to-right, binding follows parameter names.
        let source = "int clamp(int value, int low, int high) {\nif (value < low) {\nreturn low\n}\nif (value > high) {\nreturn high\n}\nreturn value\n}\nint main() {\nreturn clamp(high: 10, value: 50, low: 0)\n}\n";
        assert_eq!(ok_full(source), Value::Int(10));
    }

    #[test]
    fn impl_member_recursion_runs() {
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int sumTo(counter c) {\nif (c.value <= 0) {\n    return 0\n}\nc.value -= 1\nreturn c.value + counter.sumTo(c)\n    }\n}\nint main() {\ncounter c = counter(value: 5)\nreturn counter.sumTo(c)\n}\n";
        assert_eq!(ok_full(source), Value::Int(10));
    }

    #[test]
    fn unknown_impl_members_are_clean_errors() {
        // Gated off at check time in the host pipeline; the interpreter
        // still errors cleanly (never panics) on the unregistered member.
        let source =
            "struct counter {\n    int value\n}\nint main() {\nreturn counter.peek(1)\n}\n";
        let error = run_ungated(source).expect_err("unknown member must error");
        assert!(error.message.contains("unknown member `peek` in `counter`"));

        let source = "int main() {\nreturn engine.graphics.DrawTexture(1)\n}\n";
        let error = run_ungated(source).expect_err("unknown path must error");
        assert!(
            error
                .message
                .contains("unknown function `engine.graphics.DrawTexture`")
        );
    }

    #[test]
    fn fuel_exhaustion_is_a_clean_budget_error() {
        // An infinite loop used to hang the caller forever: the tree-walker
        // had no operation meter. With a fuel meter attached (§5.5 — a
        // deterministic operation count, never wall-clock time), the loop
        // terminates with a clean budget error, which is what keeps the
        // compile-time evaluator (§8.7.3) from hanging the compiler on a
        // runaway `@`-function.
        let spin = "int spin() {\nint x = 0\nwhile (true) {\nx = x\n}\nreturn x\n}\n";
        let statements = parse_statements_for_interp(spin);
        let fuel = Cell::new(100);
        let interpreter = Interpreter::new(&statements).with_fuel(&fuel);
        let error = interpreter
            .invoke("spin", &[])
            .expect_err("100 operations must not finish an infinite loop");
        assert!(
            error.message.starts_with("fuel budget exhausted"),
            "{error:?}"
        );
        assert_eq!(error.kind(), InterpErrorKind::Budget);

        // The same meter on a bounded program runs to completion with the
        // correct result — it charges, it does not interfere.
        let bounded = "int bounded() {\nint total = 0\nfor (int i in [1, 2, 3, 4, 5]) {\ntotal = total + i\n}\nreturn total\n}\n";
        let statements = parse_statements_for_interp(bounded);
        let fuel = Cell::new(1_000_000);
        let interpreter = Interpreter::new(&statements).with_fuel(&fuel);
        assert_eq!(interpreter.invoke("bounded", &[]), Ok(Value::Int(15)));
    }

    #[test]
    fn error_kinds_classify_entry_misses_and_runtime_failures() {
        // An unknown entry point is a host-side mistake (§2.1: hosts target
        // specific entry points), classified apart from script failures.
        let statements = parse_statements_for_interp("int main() {\nreturn 1\n}\n");
        let interpreter = Interpreter::new(&statements);
        let error = interpreter
            .invoke("nope", &[])
            .expect_err("unknown functions must fail");
        assert_eq!(error.kind(), InterpErrorKind::UnknownEntry);

        let error = interpreter
            .invoke_member("engine.gamemode", "OnTick", &[])
            .expect_err("unknown impl members must fail");
        assert_eq!(error.kind(), InterpErrorKind::UnknownEntry);

        // Ordinary script failures stay Runtime.
        let source = "int main() {\nreturn 1 / 0\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let interpreter = Interpreter::new(&outcome.statements);
        let error = interpreter.invoke("main", &[]).unwrap_err();
        assert_eq!(error.kind(), InterpErrorKind::Runtime);
    }

    #[test]
    fn host_conversions_build_scalars_and_extractors_unpack_them() {
        assert_eq!(Value::from(7i64), Value::Int(7));
        assert_eq!(Value::from(0.5f64), Value::Float(0.5));
        assert_eq!(Value::from(true), Value::Bool(true));
        assert_eq!(Value::from("hi"), Value::Str("hi".into()));
        assert_eq!(Value::from(String::from("hi")), Value::Str("hi".into()));

        assert_eq!(Value::Int(-3).as_int(), Some(-3));
        assert_eq!(Value::Int(-3).as_float(), None);
        assert_eq!(Value::Float(1.5).as_float(), Some(1.5));
        assert_eq!(Value::Bool(true).as_bool(), Some(true));
        assert_eq!(Value::Str("s".into()).as_str(), Some("s"));
        assert_eq!(Value::Str("s".into()).as_int(), None);
        assert!(Value::Void.is_void());
        assert!(!Value::Int(0).is_void());

        // Non-scalar kinds never satisfy scalar accessors.
        assert_eq!(
            Value::Array(vec![Value::Int(1)]).as_array().unwrap()[0],
            Value::Int(1)
        );
        assert_eq!(Value::Array(vec![]).as_int(), None);
    }

    #[test]
    fn configured_call_depth_limit_bounds_recursion_tighter_than_the_default() {
        // The depth check is the FIRST thing a call does after the arity
        // check, so a limit of 1 stops the recursion after exactly one
        // nested frame: main is already running, spin must not enter.
        let source = "int spin(int n) {\nreturn spin(n)\n}\nint main() {\nreturn spin(1)\n}\n";
        let statements = parse_statements_for_interp(source);
        let interpreter = Interpreter::new(&statements).with_call_depth_limit(1);
        let error = interpreter.invoke("main", &[]).unwrap_err();
        assert_eq!(error.kind(), InterpErrorKind::CallDepth);
        assert_eq!(error.message, "call depth limit of 1 exceeded");

        // A limit of 0 is the unset sentinel: the default applies, so a
        // bounded program still runs.
        let bounded = "int id(int v) {\nreturn v\n}\nint main() {\nreturn id(3)\n}\n";
        let statements = parse_statements_for_interp(bounded);
        let interpreter = Interpreter::new(&statements).with_call_depth_limit(0);
        assert_eq!(interpreter.invoke("main", &[]), Ok(Value::Int(3)));
    }

    #[test]
    fn the_default_depth_limit_is_unchanged_by_a_limit_at_the_maximum() {
        // Configuring the builder with MAX_CALL_DEPTH itself must keep the
        // interpreter's pinned default behavior: runaway recursion dies at
        // the same limit with the same message. 1024 nested CME frames
        // occupy several native Rust frames each in a debug build, so this
        // runs on a dedicated thread with a generous stack — the depth
        // guard, not the native stack, must be what stops the recursion.
        let source = "int spin() {\nreturn spin()\n}\nint main() {\nreturn spin()\n}\n";
        let handle = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                let statements = parse_statements_for_interp(source);
                Interpreter::new(&statements)
                    .with_call_depth_limit(MAX_CALL_DEPTH)
                    .invoke("main", &[])
            })
            .expect("spawn the default-limit thread");
        let result = handle.join().expect("default-limit thread must not panic");
        assert_eq!(
            result.unwrap_err().message,
            format!("call depth limit of {MAX_CALL_DEPTH} exceeded")
        );
    }

    #[test]
    fn a_low_depth_limit_runs_on_the_ordinary_test_stack() {
        // A handful of nested CME frames fit anywhere — this pins that a
        // lowered limit makes deep-recursion testing runnable without a
        // big-stack thread.
        let source = "int down(int n) {\nif (n <= 0) {\nreturn 0\n}\nreturn down(n - 1)\n}\nint main() {\nreturn down(4)\n}\n";
        let statements = parse_statements_for_interp(source);
        let interpreter = Interpreter::new(&statements).with_call_depth_limit(16);
        assert_eq!(interpreter.invoke("main", &[]), Ok(Value::Int(0)));

        let interpreter = Interpreter::new(&statements).with_call_depth_limit(4);
        let error = interpreter.invoke("main", &[]).unwrap_err();
        assert_eq!(error.kind(), InterpErrorKind::CallDepth);
        assert_eq!(error.message, "call depth limit of 4 exceeded");
    }

    #[test]
    fn an_expired_deadline_stops_at_the_first_safepoint() {
        // The deadline has already passed: the very first statement's
        // safepoint ends the invocation with a clean deadline error.
        let source = "int spin() {\nint x = 0\nwhile (true) {\nx = x + 1\n}\nreturn x\n}\n";
        let statements = parse_statements_for_interp(source);
        let expired = std::time::Instant::now() - std::time::Duration::from_millis(1);
        let interpreter = Interpreter::new(&statements).with_deadline(expired);
        let error = interpreter.invoke("spin", &[]).unwrap_err();
        assert_eq!(error.kind(), InterpErrorKind::Deadline);
        assert!(
            error.message.starts_with("execution deadline exceeded"),
            "{error:?}"
        );
    }

    #[test]
    fn a_future_deadline_lets_bounded_work_finish() {
        // A deadline 5 seconds out never fires on a bounded program: the
        // check observes real time only at safepoints and must not
        // interfere with ordinary execution.
        let source = "int sum() {\nint total = 0\nfor (int i in [1, 2, 3, 4, 5, 6, 7]) {\ntotal = total + i\n}\nreturn total\n}\n";
        let statements = parse_statements_for_interp(source);
        let soon = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let interpreter = Interpreter::new(&statements).with_deadline(soon);
        assert_eq!(interpreter.invoke("sum", &[]), Ok(Value::Int(28)));
    }

    /// A capability host stub: counts calls, echoes deterministic results.
    struct CountingHost {
        calls: std::cell::Cell<usize>,
    }

    impl CapabilityHost for CountingHost {
        fn call(&self, path: &[&str], member: &str, args: &[Value]) -> Result<Value, String> {
            self.calls.set(self.calls.get() + 1);
            match (path, member) {
                (["engine", "graphics"], "LoadTexture") => {
                    let name = args[0].as_str().expect("str arg").to_string();
                    Ok(Value::Struct {
                        name: "TextureHandle".to_string(),
                        fields: vec![("id".to_string(), Value::Int(name.len() as i64))],
                    })
                }
                (["engine", "math"], "Sqrt") => Ok(Value::Float(
                    (args[0].as_float().expect("float arg")).sqrt(),
                )),
                _ => Err(format!(
                    "capability `{}` has no member `{member}`",
                    path.join(".")
                )),
            }
        }
    }

    /// The typed seam: a provider may hand back schema ENUMS and the
    /// script matches on them — the value shapes, not interpreter
    /// internals, are the whole contract any execution engine inherits.
    struct EnumHost;

    impl CapabilityHost for EnumHost {
        fn call(&self, _path: &[&str], member: &str, _args: &[Value]) -> Result<Value, String> {
            match member {
                "NextEvent" => Ok(Value::Enum {
                    name: "Event".to_string(),
                    variant: "Scored".to_string(),
                    payload: vec![Value::Int(25)],
                }),
                other => Err(format!("no member `{other}`")),
            }
        }
    }

    #[test]
    fn capability_values_carry_schema_enums_into_scripts() {
        let source = "\
enum Event {
    Started()
    Scored(int points)
}
int main() {
    Event event = engine.feed.NextEvent()
    return match (event) {
        Started() => 0
        Scored(int points) => points * 2
    }
}
";
        let statements = parse_statements_for_interp(source);
        let interpreter = Interpreter::new(&statements).with_capabilities(&EnumHost);
        assert_eq!(interpreter.invoke("main", &[]), Ok(Value::Int(50)));
    }

    #[test]
    fn capability_errors_report_the_member_and_path() {
        let source = "\
int main() {
    engine.missing.Nope()
    return 0
}
";
        let statements = parse_statements_for_interp(source);
        let host = CountingHost {
            calls: std::cell::Cell::new(0),
        };
        let interpreter = Interpreter::new(&statements).with_capabilities(&host);
        let error = interpreter.invoke("main", &[]).unwrap_err();
        assert!(
            error.message.contains("engine.missing"),
            "{}",
            error.message
        );
        assert!(error.message.contains("Nope"), "{}", error.message);
    }

    #[test]
    fn capability_calls_dispatch_to_the_registered_host() {
        let source = "\
import engine.graphics
int main() {
    TextureHandle tex = engine.graphics.LoadTexture(\"hero.png\")
    return tex.id
}
";
        let statements = parse_statements_for_interp(source);
        let host = CountingHost {
            calls: std::cell::Cell::new(0),
        };
        let interpreter = Interpreter::new(&statements).with_capabilities(&host);
        assert_eq!(interpreter.invoke("main", &[]), Ok(Value::Int(8)));
        assert_eq!(host.calls.get(), 1);
    }

    #[test]
    fn capability_host_errors_surface_as_clean_runtime_errors() {
        let source = "\
import engine.graphics
int main() {
    engine.graphics.Missing(1)
    return 0
}
";
        let statements = parse_statements_for_interp(source);
        let host = CountingHost {
            calls: std::cell::Cell::new(0),
        };
        let interpreter = Interpreter::new(&statements).with_capabilities(&host);
        let error = interpreter.invoke("main", &[]).unwrap_err();
        assert_eq!(error.kind(), InterpErrorKind::Runtime);
        assert!(
            error
                .message
                .contains("capability `engine.graphics` has no member `Missing`"),
            "{error:?}"
        );
    }

    #[test]
    fn without_a_capability_host_the_calls_stay_unknown() {
        // The pre-schema behavior: an interpreter with no capability
        // surface attached reports the plain unknown-function error —
        // which is exactly what compile-time evaluation relies on (§8.5
        // purity: the megaprogram evaluator never attaches one).
        let source = "\
import engine.graphics
int main() {
    engine.graphics.LoadTexture(\"hero.png\")
    return 0
}
";
        let statements = parse_statements_for_interp(source);
        let interpreter = Interpreter::new(&statements);
        let error = interpreter.invoke("main", &[]).unwrap_err();
        assert_eq!(
            error.message,
            "unknown function `engine.graphics.LoadTexture`"
        );
    }
}
