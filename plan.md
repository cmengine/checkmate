# plan.md — Megaprogramming (§8) implementation plan for Checkmate

> **Handoff document.** This file is self-contained: an agent with no other context
> should be able to read this file plus the repo and continue the work. Check off
> `[x]` every finished step, in the same commit that finishes it.

## 0. Mission

Implement Checkmate megaprogramming (WHITEPAPER.md §8) end to end:

1. Write `magic.cm` (repo root) as the TDD anchor: grammars ("parsers") for
   **HTML, CSS, JS, Python, JSON, YAML, SQL** and others (TOML, RE), plus a
   `magic` macro declaration for each, per the whitepaper.
2. Implement the megaprogramming system ("All of it") and loop until
   `magic.cm` compiles fine and every megaprogram in it works perfectly.
3. Add a new CLI command: **`cme expand <file.cm>`** which expands megaprograms
   in the places they were called into normal Checkmate code, written to a
   labeled file side by side with the original
   (`cme expand magic.cm` → `magic_expanded.cm`).

**Architecture mandate (owner instruction, overrides whitepaper §8.6 detail):**
The main lexer+parser+AST only **detect** magic declarations and usages and hand
the code inside the magic blocks to the **megaprogram pass** — a separate
subsystem inside the compiler (`cme-compiler::mega`) which takes magic
definitions and expands magic calls into **normal Checkmate code (text, not
AST**, because we need it for `cme expand`**)**, then passes the result (normal
Checkmate code) back to the main compiler, which parses/checks/runs it like
hand-written code.

## 1. Ground truth and context (what an agent must know)

### 1.1 Repo layout (as of base commit `1af9f94`)

- Cargo workspace, edition 2024. Root package `cme` = facade + optional CLI
  (feature `cli` enables `core`,`compiler`,`interp`,`runtime`).
- `crates/cme-core/src/lib.rs` — spanned AST (`Span`, `Type`, `Stmt`, `StmtKind`,
  `Expr`, `ExprKind`, …). **Language data models belong here** (AGENTS.md rule).
- `crates/cme-compiler/src/`
  - `lexer.rs` — logos-based `Token`, error recovery (`lex_with_errors`).
  - `parser.rs` — `Parser` (tolerant, plants `Invalid` nodes),
    `strip_insignificant_newlines` (newlines significant except inside `(`),
    `parse_program_with_errors`.
  - `validate.rs` — post-parse statement-list validation.
  - `check.rs` — type checker `check(&[Stmt]) -> Vec<Diagnostic>`.
  - `diagnostics.rs` — `Diagnostic` (Lex/Parse/Type), `ParseOutcome`.
  - `lib.rs` — `parse_source` one-call front end.
- `crates/cme-interp/src/lib.rs` — tree-walking interpreter for the full
  language surface (`Interpreter::new(&[Stmt])`, `invoke("main", &[])`).
- Root CLI `src/main.rs` — commands `lex`, `ast`, `check`, `run`
  (`run` = parse + check + invoke `main`).
- Fixtures at repo root: `basic.cm`, `syntax.cm` (full surface, returns 0),
  `boom.cm`, `broken_syntax.cm` (recovery stress, pinned diagnostic counts),
  `test.cm`. Root integration test `tests/run_pipeline.rs` pins fixtures.

### 1.2 Current language surface (available to generated code)

structs/enums (+generics), `option`/`result` + `?`, `match` (stmt+expr),
`for`-in, arrays `T[]` + `.length`, maps `map<K,V>`, `$"…{expr}…"` interpolation,
`infer`, impl blocks, named args, compound assignment. **No** `import` statement
yet, no host schemas, no char type, no string indexing. Generated code must
compile+run on the tree walker using ONLY this surface. Newlines are
significant at statement level; **insignificant inside parentheses** (§A.8) —
this makes `\n`-joined template element lists safe inside `( )`.

### 1.3 Whitepaper §8 summary (the spec being implemented)

- Three artifacts: `grammar` (named rule library + lexical profile), `magic`
  (entry point: pattern in parens → expansion template in braces), pure
  compile-time functions called with `@`.
- Declaration form `magic name(pattern) { template }` (identifier between
  `magic` and `(`); invocation form `magic(name) { region }`. Invocation
  positions: declaration, statement, expression, type.
- Grammar profile: `skip [ … ]` (charset), `comment ( "…" [ until "…" ] )`,
  `string ( '"' ) [multiline] [island ( "${" "}" )]`, `rule name(context …) { pattern }`.
  Line-oriented (skip set has no `\n`) vs flow-oriented. `eol`/`line`/`indent`
  only in line mode.
- Pattern language (§8.3): literals `i"…"`; classes `[a-z0-9_]`/`[^…]`;
  `any`; `scan […]`; `until "lit"`/`until { p }`; `lineRest`; `eol`; `line`;
  `eof`; `soft { p }`; `optional { p }`; `each [+] [sep p] [trailing] [ [n,m] ] { p }`;
  `oneof { label => ( p ), … }`; `peek`/`not`; groups `( p )` with `as`;
  rule refs (bare/qualified/recursion, `with context { … }`, `as`);
  `indent { p }` / `indent verbatim`; `raw { p }`; `where cond`; `label "msg" { p }`;
  fragments `$ident $word $tag $int $float $str $tt $text $template $raw $expr $type $block`
  (with `i` prefix, `<validator>` and `as` bindings). Skipper runs before
  elements except `eol`/`eof`/`line`/`where`/`label`/`peek`/`not`; atomic
  matchers never skip internally; zero-width assertions restore the cursor.
- Ordered choice with complete fall-through; `where` failures backtrack.
- Templates (§8.4): `$cap` splices (typed by capture kind and position),
  `$"…{cap}…"` interpolation, `[each in xs { … }]` (implicit element binding:
  the element is available as `$item`, its fields bare as `$field`),
  `[when cond { … } else { … }]`, `match ($cap) { label => … }` (exhaustive
  over `oneof` tags), `let`, `require(cond, "msg")`, `@fn(…)`.
- §8.5 compile-time computation: `@`-calls run in a sandboxed evaluator;
  `cm.parseExpr/cm.parseStmts/cm.parse`; `code` fragments.
- §8.6 region location: brace balancing under the **composed profile** (comment,
  string, island forms of the entry grammar and every grammar its pattern
  references, transitively); edge normalization (trim one newline after `{`,
  one before `}`, horizontal whitespace); heredoc `magic(name) <<tag … tag`;
  fixpoint expansion queue, depth cap 64.
- §8.7 guarantees: determinism, packrat memoization, termination (left-recursion
  rejection), purity, span faithfulness.

### 1.4 Deviations from the whitepaper (deliberate, owner-approved)

1. **Text-level expansion.** The pass emits Checkmate *source text*, not AST
   (owner mandate, needed for `cme expand`). Consequence: post-expansion
   diagnostics point into the expanded text; the pass's own diagnostics point
   into the original file. Provenance comments (`// magic(name) @ line:col`)
   keep the mapping visible.
2. **No `@`-functions in early tasks.** Until the §8.5 evaluator exists
   (Task 7), templates avoid `@`; e.g. HTML void-tag checks are written as
   `where name == "br" || …` chains instead of `where @isVoid(name)`.
   Task 8 upgrades `magic.cm` to the whitepaper shapes.
3. **Single-file megaprograms.** No `import` exists yet, so grammars/magics in
   `magic.cm` are declared in the same file, before use. The std grammar
   library files (`std.json` …) arrive with the mod/import system (out of scope
   here). Grammar bodies still use the §8.2 declaration syntax verbatim.
4. **`.matched` accessor.** Every capture (records included) additionally
   exposes `.matched` (the exact matched source text) to `where`/templates —
   needed to express bool/number JSON leaves without `@`-codegen. Named
   `.matched` (not `.text`) because grammars legitimately bind a field called
   `text`. `.matched` TRIMS leading/trailing whitespace (interior untouched):
   indent blocks and skip-run edges would otherwise leak `\n    ` prefixes
   into template strings. The exact extent stays available via `.span`.
   Documented as an extension.
5. **Bind syntax reconciliation (§8.3.10 vs §8.8).** The EBNF shows `as BIND`,
   but every §8.8 grammar binds with a bare trailing identifier (`selector sel`,
   `$word fname`, `statement then`). Both forms are supported and identical:
   `ruleref IDENT` ≡ `ruleref as IDENT`, same for fragments. Classes, `any`,
   `scan`, `until`, `lineRest`, groups, `each`, `optional` use `as BIND`.
6. **`each` element naming.** `[each in xs { … }]` binds the element as `$item`
   (whitepaper usage) and makes its fields available bare. As an extension,
   `[each NAME in xs { … }]` picks an explicit element name so nested loops
   don't shadow outer captures (needed by `cssSheet`).
7. **Template list joins.** `[each …]` elements are joined with `", "` when the
   construct sits inside template `(` or `[` (arg/param/element-list or array
   literal position), with `"\n"` otherwise (statement lists, map entries).
   Rationale: Checkmate needs commas in positional lists/arrays but
   newline-delimits statements, map entries, and named args.
8. **Whitepaper errata fixed in `magic.cm`** (documented in the fixture):
   JSON `number` needs `optional { scan [0-9] as rest }` (bare `scan` requires
   ≥1 char, so `"3"` would fail); py `def`'s body must be `each { stmt }` not
   `each { recur }` (`recur` would re-enter `def`, contradicting §8.4's own
   worked example); `indent` step 1 treats a mid-line cursor whose remaining
   line is blank as an end-of-line start (otherwise YAML `limits:` could never
   open a block at the child column, contradicting §8.3.5's worked example).

### 1.5 Working rules (from AGENTS.md — binding)

- Conventional Commits (`feat(compiler): …`, `test: …`, `docs: …`).
- Data models → `cme-core`; recognition/parsing → `cme-compiler`.
- Keep the root facade feature-gated; no new default features.
- Update lexer/parser/validator/AST together for language-facing changes;
  add focused unit tests in the affected crate.
- Validate with: `cargo fmt` (then `--check`), `cargo test --workspace`,
  `cargo clippy --workspace --all-targets`, and `cargo test --workspace
  --features cli` + a real `cargo run --features cli -- …` smoke for CLI work.
- Toolchain: `export PATH="$HOME/.cargo/bin:$PATH"` (rustup install was needed
  in this environment).
- **Never use mdpeek** (owner override of the AGENTS.md whitepaper policy).
- Whitepaper is the language source of truth; where this plan simplifies it,
  the deviation is listed in §1.4 and flagged in code comments.

## 2. Target architecture

```
source text
   │
   ▼
cme-compiler::mega (the megaprogram subsystem — NEW)
   scan      → find grammar decls, magic decls, magic(name){region} invocations
               (brace balancing under the composed profile; region normalization)
   ir        → (cme_core::magic) Grammar/Rule/Pattern/Template data models
   pattern   → pattern-text parser  (§8.3 grammar → Pattern AST)
   matcher   → packrat PEG engine over region text → capture tree
   template  → template-text parser (§8.4) + elaborator → generated CODE TEXT
   expand    → orchestrator: collect decls, expand invocations, fixpoint
               (re-scan generated text; depth cap 64), reassemble source
   ▼
expanded source text (pure Checkmate; decl sites become comments)
   │
   ▼
existing front end unchanged: lex → strip newlines → parse → validate → check → run
```

- `cme expand <file.cm>`: run scan+expand, write `<stem>_expanded.cm` next to
  the original, then parse+check the expanded file and report (spans refer to
  the expanded file, which exists on disk).
- `cme check|run|ast <file.cm>`: if the file contains `magic`/`grammar` tokens
  (cheap word-boundary pre-check), expand first, then proceed on the expanded
  text; expansion diagnostics are rendered against the ORIGINAL file.

### 2.1 Key data models (goes to `crates/cme-core/src/magic.rs`, module `magic`)

```rust
pub struct Grammar { name, span, skip: CharSet, comments: Vec<CommentForm>,
                     strings: Vec<StringForm>, rules: Vec<Rule> }
pub struct CommentForm { opener: String, closer: Option<String> } // None ⇒ line comment
pub struct StringForm { quote: char, multiline: bool, island: Option<(String, String)> }
pub struct CharSet { negated: bool, items: Vec<CharItem> }  // Char(char) | Range(char,char)
pub struct Rule { name, span, context: Vec<ContextField>, pattern: Pattern }
pub struct ContextField { name, default: Option<ContextDefault> } // `= none` etc.

pub enum Pattern {  // every variant carries Span
    Lit { text, insensitive }, Class { set, bind }, Any { bind },
    Scan { set, bind }, Until { stop: Box<Pattern>, bind }, LineRest { bind },
    Eol, Line, Eof, Soft(Box<Pattern>),
    Optional { body: Box<Pattern>, bind },
    Each { plus, sep: Option<Box<Pattern>>, trailing, bounds: Option<(u32,u32)>,
           body: Box<Pattern>, bind },
    OneOf { branches: Vec<(String, Pattern)> },          // label => pattern
    Peek { negated, body: Box<Pattern> }, Group { body: Box<Pattern>, bind },
    RuleRef { path: Vec<String>, ctx: Vec<(String, CtxExpr)>, bind },
    Recur, Indent { body: Option<Box<Pattern>>, verbatim: Option<String> },
    Raw(Box<Pattern>), Where { cond: CtxExpr }, Label { msg, body: Box<Pattern> },
    Fragment { kind: FragKind, insensitive, validator: Option<String>,
               bind: Option<String> },
}
pub enum FragKind { Ident, Word, Tag, Int, Float, Str, Tt, Text, Template,
                    Raw(Option<Vec<String>>), Expr, Type, Block }

pub enum CtxExpr {  // `where`/context expressions (§8.3.4)
    Lit(Str Lit / Int / Float), Capture { path: Vec<String>, accessor: Option<Accessor> },
    Bin(BinOp, Box, Box), Not(Box), SomeIn { var, list, cond }, AllIn { … },
    Present { path }, Call { path, args }, …
}

pub enum Template { // §8.4
    Text(String), Splice { path }, Interp { parts: Vec<TmplStrPart> },
    Each { list: TmplExpr, body: Box<Template> },
    When { cond: CtxExpr, then, else_ },
    Match { cap: String, arms: Vec<(String, Template)> },
    Let { name, value }, Require { cond, message },
    Call { path, args },  // @fn
}
```

### 2.2 Capture values (matcher → template)

```rust
pub enum CaptureValue {
    Text { text: String, kind: TextKind /*Ident|Word|Tag|Raw|Str(quoted)*/ , span },
    Int { value: i64, span }, Float { value: f64, span },
    List { items: Vec<CaptureValue>, span },
    Record { tag: String, fields: Vec<(String, CaptureValue)>, text: String, span },
    Opt { value: Option<Box<CaptureValue>>, span },
}
```

- `oneof` branch → `Record` tagged with the branch label; binds inside the
  branch become fields; **a branch body that is a single bare ruleref inherits
  that rule's record fields (re-tagged)**. A ruleref capture yields the rule's
  record — if the rule body's top-level construct is a `oneof`, the record's
  tag is the chosen branch label and its fields the branch's binds.
- `each … as xs` → `List`. `optional … as x` → `Opt` (an unbound `optional`
  binds its inner captures through when present; absent ⇒ fields absent).
  Groups/classes/scans/until/lineRest → `Text{kind: Raw}` (group = matched text).
- Splicing rules (§8.4 table): Text{Ident/Word/Tag} → raw identifier text in
  name/type/expression positions; Text{Str} → quoted+escaped Checkmate string
  literal; Int/Float → literal; List → repeats elements in element-list
  position / array literal in expression position; Record → must be `match`ed;
  Opt → single element if present (splicing an absent optional is an error).

### 2.3 Matcher notes (packrat PEG)

- Cursor over `&[char]` with byte-offset mapping (spans!). Memoize rule
  results keyed by `(rule_id, pos, env)` where `env` = (skip-mode, indent
  base column, context values identity). A re-entrant call against an
  in-progress memo entry fails immediately (cycle cut).
- Failure reporting: track furthest position + expected-set + `label` context;
  committed-block (`indent`) failures outrank ordinary furthest failures.
- `indent` protocol per §8.3.5 (block stack of base columns; tab = 8; mixed
  tabs+spaces = committed failure; transparent lines skipped; strictly-deeper
  rule; the four termination cases).
- Line mode: skip set without `\n`; `eol` consumes the terminator + transparent
  tail; `eof` requires only skip/transparent lines to region end.
- Region text is matched against the ORIGINAL file's spans (byte offsets),
  which is what keeps diagnostics faithful.

### 2.4 Region scanner notes (§8.6)

- Balance braces from the invocation's `{` under the composed profile:
  at each position try comment forms longest-first, then string forms
  longest-first (unclosed single-line string ⇒ ordinary text), islands
  transparent (recurse inside), else count `{`/`}`. Heredoc `<<tag` form:
  region ends at the line whose content is exactly `tag` (Task 9).
- Normalization: drop one newline after `{` and before `}`, trim horizontal
  whitespace at both edges. Region spans remain original-file spans.
- Composed profile = profile of the entry pattern's grammar (rule-ref entry)
  or the default profile (inline pattern: skip space+newline, `"` strings, no
  comments) UNION the profiles of every grammar the pattern references
  transitively. If a grammar declares no string forms, the default `"`
  single-line string form is added (pragmatic; JSON regions need it).

### 2.5 Expansion orchestrator (text level)

1. Scan the file → grammar decls, magic decls (pattern text + template text +
   spans), invocations (name + region text + spans).
2. Parse grammars; parse magic patterns/templates. Verify: rule refs resolve
   (same file, declared anywhere — two-pass), `recur` has an enclosing rule,
   line-mode elements only in line grammars, no left recursion (nullable-prefix
   cycle check) — Task 4/5 hardening; Task 3 does minimal validation.
3. For each invocation (source order): match its pattern against the region →
   capture tree; elaborate the template → generated code text.
4. Reassemble: invocation sites ← generated code; grammar/magic decl sites ←
   `// [megaprogram declaration '<name>' removed by expansion]` followed by the
   SAME number of newlines the decl occupied (keeps later line numbers stable).
5. Fixpoint: re-scan the assembled text for NEW invocations (generated text may
   contain them) and repeat; depth cap 64 counting all origins. (Task 3:
   single pass over the original file + re-scan loop; island-piercing for
   nested invocations inside a region lands with islands in Task 9.)

## 3. Task list (ordered; first 3 are this session's scope)

### Task 1 — `magic.cm` TDD fixture  `[x] DONE`
- [x] Write `magic.cm` at repo root, structured as:
  1. header comment (what the file is, how it is tested, expected expansions);
  2. `grammar json`, `grammar toml`, `grammar yaml`, `grammar css`,
     `grammar html`, `grammar re`, `grammar js`, `grammar py`, `grammar sql`
     — whitepaper §8.8/§8.4 shapes, elided rules completed, minimal documented
     deviations (extra `as` binds for template access; `.text` usage);
  3. one `magic` declaration per grammar (`jsonValue`, `tomlValue`,
     `yamlValue`, `cssSheet`, `htmlFragment`, `reCompile`, `jsRun`, `def`,
     `sqlQuery`) — templates avoid `@` for now (§1.4.2);
  4. a real consumer Checkmate program (structs/enums used by the templates +
     `main()` that exercises every megaprogram and returns 0 on success).
- [x] The file deliberately does NOT compile yet (no magic support) — it is
  the TDD north star for Tasks 3–10. Documented in the header.
- [x] Commit: `test: add magic.cm megaprogramming fixture (TDD spec)`.

### Task 2 — Detection: IR + scanner  `[x] DONE`
- [x] `crates/cme-core/src/magic.rs`: data models of §2.1 (Grammar/Rule/
      Pattern/Template/CharSet/CtxExpr + spans) + unit tests. Wire
      `pub mod magic;` into cme-core lib.
- [x] `crates/cme-compiler/src/mega/mod.rs` + `scan.rs`:
  - word-boundary scan for `magic` / `grammar` keywords (comment/string aware
    at the Checkmate level for headers);
  - `grammar <ident> { … }` → verbatim body extraction (default-profile brace
    balancing);
  - `magic <ident> ( … ) { … }` decl → header span, pattern text, template text;
  - `magic ( qualified.name ) { … }` invocation → composed-profile region
    balancing (§2.4) + normalization; heredoc `<<tag` deferred to Task 9;
  - output `MagicScan { grammars, magics, invocations, … }` preserving spans.
- [x] `crates/cme-compiler/src/mega/profile.rs`: CharSet matching helpers +
      composed-profile computation.
- [x] Focused unit tests: region balancing with `console.log("}")`,
      `<!-- -->`, `'` apostrophes, nested `{}` in strings, island forms;
      decl/invocation distinction; spans exact.
- [x] Commit: `feat(compiler): scan magic declarations and invocation regions`.

### Task 3 — Expansion core + `cme expand` CLI  `[x] DONE`
- [x] `mega/pattern.rs`: parse pattern text → `cme_core::magic::Pattern`
      (all §8.3 forms parse; matcher support may lag — unimplemented matcher
      forms fail with a clear diagnostic at match time, never a panic).
      Also: `oneof` takes a trailing `as` bind; implicit (bare) binds are
      line-aware so a next-line construct is never stolen (§1.4.5 errata).
- [x] `mega/matcher.rs`: packrat engine — flow mode first, then line mode:
      skip/comment handling, literals (`i"…"`), classes, `any`, `scan`,
      `until` (literal and `{ p }`), `lineRest`, `eol`/`line`/`eof`,
      `soft`, `optional`, `each` (+`sep`/`trailing`/bounds/`each+`), `oneof`
      with fall-through, `peek`/`not`, groups+binds, rule refs+`recur`+memo,
      `indent` (+`indent verbatim`), `raw`, `where` (equality/comparison,
      `&&`/`||`/`!`, `present`, `some/all … in`, `.text`), `label`;
      fragments `$ident $word $tag $int $float $str` + `$text` (remainder
      fallback) + `$type`/`$expr`/`$raw` with tail-matching extents
      (parse-integration deferred to Task 6). Capture tree per §2.2.
      Notes: the caller's continuation flows through `rule_ref` and
      `each` bodies (`Continuation::Repeat`) so tail-bounded fragments
      find their real boundary; inside `indent` blocks each iterations
      begin at the next content line and must sit at the block's base
      column; bare captures compare by scalar value in `where`.
- [x] `mega/template.rs`: parse template text → `Template`; elaborator →
      generated code TEXT (splice/interp/each/when/match/let/require).
- [x] `mega/expand.rs`: orchestrator (§2.5) + `pub fn expand_source(source)
      -> Result<ExpansionOutcome, Vec<Diagnostic>>` exported from
      cme-compiler; `ExpansionOutcome { expanded: String, records: Vec<…> }`.
- [x] Root CLI: `cme expand <file.cm>` writes `<stem>_expanded.cm` next to the
      original + header comment; then parse+check the expanded file (reporting
      against it). `check|run|ast` expand first when magic blocks are present.
      `USAGE` string updated.
- [x] Tests: cme-compiler unit tests (pattern parser, matcher, template,
      expand) + root integration `tests/megaprogram.rs` pinning: JSON region →
      expanded `map<str, str>`; `py.def` region → real Checkmate function
      (returns/calls; nested-if limit documented); `cme expand` output file
      name and clean parse/check.
- [x] Commit: `feat(compiler): megaprogram expansion core and cme expand command`.

### Task 4 — Pattern-language completion + hardening  `[ ]`
- [ ] Left-recursion (nullable-prefix cycle) static rejection with rewrite hint.
- [ ] Memoization keyed by env; re-entrant cycle cut; fuel counter
      (operation count) with budget error.
- [ ] Furthest-failure diagnostics with `label`/rule context; committed-block
      diagnostics; `where` failures recorded like element failures.
- [ ] `indent` full §8.3.5 protocol incl. the four termination cases and
      mixed-tabs check; `indent`-after-`eol` static rejection.
- [ ] Fragment validators (`$tag<rule>` / `$ident<fn>` where fn = pure
      CtxExpr-callable) — parser + matcher.
- [ ] `$tt` (profile-string-aware balanced token tree).
- [ ] Tests per feature in cme-compiler.

### Task 5 — All `magic.cm` megaprograms green  `[ ]`
- [ ] Loop: `cargo run --features cli -- expand magic.cm` until clean; then
      `cargo run --features cli -- run magic_expanded.cm` until it exits 0.
- [ ] Fix grammar/matcher/template gaps surfaced (expected: CSS `context`
      threads, HTML `until { "</" i$tag close where close == name }`,
      JS ASI `semi` + `soft` postfix chains, TOML dotted keys + `soft`
      arrays, YAML `indent`/`indent verbatim` + transparent comment lines,
      SQL flow grammar).
- [ ] Upgrade the root integration test to run the REAL `magic.cm` fixture
      end to end (expand → check → run expanded → `main` returns 0).
- [ ] Commit(s): `feat(compiler): …` per subsystem gap fixed, then
      `test: pin magic.cm megaprograms end to end`.

### Task 6 — Parse-integrated extents  `[ ]`
- [ ] Public sub-parser helpers in cme-compiler (`parse_expr_text`,
      `parse_type_text`, `parse_block_text`) returning success/failure
      (used ONLY for boundary checking + capture parsing).
- [ ] `$expr`/`$type`/`$block`/`$raw`: boundary accepted only when the tail
      matches as a whole AND the candidate text parses (§8.3.6); speculative
      tail evaluation with cycle cut; furthest-boundary diagnostics.
- [ ] `$template` islands (default `{{ }}` + Checkmate expression; `\{{`
      escapes; `<open close rule>` parameterization).
- [ ] py/JS tests with commas/nested calls (`if clamp(v, lo) > hi:`).
- [ ] Commit: `feat(compiler): parse-integrated raw/expr/type/template extents`.

### Task 7 — §8.5 compile-time computation  `[ ]`
- [ ] Compile-time evaluator for pure Checkmate functions (bridge capture
      values ↔ interpreter values; spans as an opaque value; `code` type).
- [ ] `@fn(…)` in templates; `cm.parseExpr/cm.parseStmts/cm.parse`;
      `cm.code.*` builder subset (`cm.code.str`, `cm.code.call` minimal).
- [ ] Purity enforcement: only core/self imports (single-file: trivially true).
- [ ] Commit: `feat(compiler): compile-time function evaluation for megaprograms`.

### Task 8 — `magic.cm` upgraded to whitepaper templates  `[ ]`
- [ ] `@toValue`-style codegen: JSON/TOML/YAML → a `jsonTree`-style enum value
      (declared in magic.cm) instead of summary maps.
- [ ] `where @isVoid(name)` / `@isRawText(name)` replace the `||` chains;
      `@emitElement` for HTML (late `cm.parse(css.sheet/js.program, body)`);
      `@py.emitBody` recursion; `@emitMatcher` for RE; `@anchorsResolve`
      YAML `require`.
- [ ] Re-pin magic.cm; commit: `test: upgrade magic.cm templates to §8.5 codegen`.

### Task 9 — Nested invocations, islands, heredocs, fixpoint  `[ ]`
- [ ] Region scanning with islands (piercing `"${ … }"`), nested
      `magic(...)` discovery + expansion inside regions (queue, source order,
      depth cap 64, full expansion-stack diagnostics).
- [ ] Heredoc `magic(name) <<tag … tag` regions.
- [ ] Tests: the §8.8 JS-template-literal example (`magic(json.value)` inside
      `` `Hello, ${ … }` ``).
- [ ] Commit: `feat(compiler): nested magic invocations, islands, heredoc regions`.

### Task 10 — Diagnostics & provenance polish  `[ ]`
- [ ] Region-scan hint ("an inner `}` invisible to every composed profile …
      the heredoc form is exact"), pattern-failure rendering with the
      embedded-source caret (§8.3.9 shape), `require` anchored at capture spans.
- [ ] `cme expand`: optional provenance comments (`// @ magic(name) src:line:col`)
      — keep default output byte-deterministic.
- [ ] Commit: `feat(compiler): megaprogram diagnostics polish`.

### Task 11 — Grammar extension + profile validation  `[ ]`
- [ ] `grammar ts extends js { … }` (override/add rules; lexical profile
      inheritance), validators, profile static checks (line-mode elements in
      flow grammars rejected; `indent`/`eol`/`line` inside `soft` rejected).
- [ ] Commit: `feat(compiler): grammar extension and profile validation`.

### Task 12 — Docs & status  `[ ]`
- [ ] Update AGENTS.md "Repository State Notes" (megaprogramming implemented:
      which parts, where), README status + `cme expand` usage.
- [ ] Final run: fmt/clippy/tests all green; worklog + plan checkboxes done.
- [ ] Commit: `docs: record megaprogramming support and cme expand`.

## 4. Definition of done (overall)

- `cargo test --workspace` and `cargo test --workspace --features cli` green.
- `cme expand magic.cm` → `magic_expanded.cm` parses + checks clean;
  `cme run magic_expanded.cm` (and `cme run magic.cm`) exit 0 with every
  internal check passing.
- `cme expand` on files without magic: unchanged passthrough semantics.
- All prior fixtures (`basic.cm`, `syntax.cm`, `boom.cm`, `broken_syntax.cm`,
  `test.cm`) keep their pinned behavior.
- plan.md checkboxes reflect reality.

## 5. Session log / handoff notes

- Session 1: Tasks 1–3 completed, committed on top of base `1af9f94`.
  Per-commit patches exported (see delivery note below).
- The megaprogram subsystem lives in `crates/cme-compiler/src/mega/`
  (`mod.rs`, `scan.rs`, `profile.rs`, `pattern.rs`, `matcher.rs`,
  `template.rs`, `expand.rs`, `ctxexpr.rs`) with data models in
  `crates/cme-core/src/magic.rs`. `cme expand` is implemented in
  `src/main.rs` (root, `cli` feature).
- Known intentional gaps after Task 3 are exactly §3 Task 4+ items; the
  matcher returns `unsupported` diagnostics (never panics) for forms added
  later.
- Errata added while landing Task 3 (see §1.4): implicit binds are
  line-aware; generated output is edge-trimmed per invocation and per
  `[each]` item; `[each]` joins are brace-aware (map entries newline,
  lists/arrays commas); bare captures compare by scalar value in
  conditions; non-record captures expose trailing accessors
  (`$item.children.length`).
- Task 5 loop status at the end of Session 1: `cme expand magic.cm`
  expands all 10 invocations and the expansion parses + type-checks;
  `cme run magic.cm` exits 0 with `fails = 2` remaining, both from the
  HTML child-counting semantics (zero-width text node at the element's
  stop position, and `1 < 2` not splitting into two text nodes) — the
  expected-vs-actual decisions are the first order of Task 5.
