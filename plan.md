## 0. How to use this document (agent contract)

1. **Order is mandatory.** Execute phases A → I in order. Within a phase, execute steps
   in order. Do not skip ahead; later phases depend on earlier ones.
2. **Tick as you go.** Every step is a `- [ ]` checkbox. Tick it only when its
   "Done-when" conditions hold. If you abandon a step, write why next to it.
3. **Green before you move on.** After every phase (and after any risky step inside a
   phase) run the validation gate:
   ```sh
   cargo fmt
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
   plus, until Phase G removes it: `cargo test --workspace --features cli`.
   A phase is finished only when the gate is fully green.
4. **Pins travel with the change.** Whenever a step changes user-visible behavior
   (diagnostic counts, message strings, AST shapes), update the pinned tests and the
   fixture-file comments **in the same commit**. Never leave a "I'll fix the pins later" state.
5. **Commit discipline.** One phase = one or more `jj` changes (never raw `git`, per
   `AGENTS.md`). Use the commit message given at the end of each phase.
6. **Repository rules that apply to you:**
   - `AGENTS.md` governs: use `jj`, keep changes in the affected crate, respect
     `cme-core` AST ownership (data models in core, recognition in compiler).
   - Do **not** read `WHITEPAPER.md` with anything other than `mdpeek`. This plan
     deliberately avoids specification changes; if any step seems to require a language
     design decision, **stop and ask the owner** (AGENTS.md rule).
   - `plan.md` (this file) replaces the previously deleted `plan.md`; `AGENTS.md`
     references to it become valid again once this file lands (fixed in Phase A/I).
7. **Baseline pre-flight (do this first):**
   - [ ] `cargo test --workspace` green (and with `--features cli`).
   - [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
   - [ ] `cargo fmt --check` clean.
         If any of these is red **before** you start, stop and report — do not build on red.

---

## 1. Decision points — resolve with the owner before starting

These are the only open decisions. Defaults are given; the owner (repo maintainer)
confirms or overrides them **before** execution begins. Once confirmed, treat them as
fixed for the whole run.

### D1 — Model `infer` as `Option<Type>` instead of `Type::Infer` (from review item 3)

- **Default: YES, do it** (folded into Phase E, step E.5).
- Rationale: `infer` is the _absence_ of an annotation, not a type. `ty: Option<Type>`
  (`None` = `infer`) keeps a future type checker from special-casing a fake type.
  It changes no accepted syntax, only AST modeling.
- If declined: skip step E.5; keep `Type::Infer` and `ty: Type`.

### D2 — Scope of `#[non_exhaustive]` (from review item 21)

- **Default: apply to `cme-compiler` boundary types only** (`Token`, `LexError`,
  `Diagnostic`, `DiagnosticKind`, `ParseOutcome`), **not** to the `cme-core` AST enums.
- Rationale: `#[non_exhaustive]` on `ExprKind`/`StmtKind`/`Type` would remove
  exhaustiveness checking from `cme-compiler`'s own matches (they are cross-crate),
  trading real internal safety for theoretical semver room. The compiler-boundary types
  are matched only inside their own crate, so the attribute costs nothing there.
- If the owner wants it on the AST enums too, add it in Phase F but expect to add
  catch-all arms to every `cme-compiler` match over AST enums.

### D3 — LexError message wording (from review items 6/8)

- **Default (fixed, listed for visibility):**
  `InvalidCharacter` → "invalid character" · `UnterminatedString` → "unterminated string
  literal" · `IntegerOverflow` → "integer literal is too large" · `FloatOverflow` →
  "float literal is too large".

Everything else in this plan is settled design, not open for re-litigation mid-run.

---

## 2. Phase map

| Phase | Title                                                    | Review items         | Risk     | New behavior?                   |
| ----- | -------------------------------------------------------- | -------------------- | -------- | ------------------------------- |
| A     | Packaging & repo hygiene                                 | 14, 15, 17 (CI part) | Low      | No                              |
| B     | Lexer: error kinds, float guard, honest recovery         | 6, 10, 11            | Medium   | Yes (pin counts may shift)      |
| C     | Parser mechanics: Eof token, Copy tokens, cursor helpers | 2, 4, 5, 20          | Medium   | Yes (`{`/`}` become lex errors) |
| D     | Diagnostics model: flattened `Diagnostic`, message style | 7, 8                 | Medium   | Yes (all message strings)       |
| E     | AST overhaul: spans, `Paren`, validator, error ids       | 3, 9 (+D1)           | **High** | Yes (mixed `&&`/`               |     | ` trees survive) |
| F     | Public API: `parse_source`, `ParseOutcome`, facade, docs | 1, 18, 19, 21 (+D2)  | Low      | No                              |
| G     | CLI: dedicated `cme-cli` crate, `LineIndex`              | 12, 13               | Medium   | Caret/columns on non-ASCII      |
| H     | Fixtures: `tests/fixtures/` + data-driven suite          | 16                   | Medium   | No                              |
| I     | Docs closeout: README, AGENTS.md, final audit            | 17 (docs part)       | Low      | No                              |

Full cross-reference (all 21 review items are covered):

| Item | Phase |     | Item | Phase |     | Item | Phase |
| ---- | ----- | --- | ---- | ----- | --- | ---- | ----- |
| 1    | F     |     | 8    | D     |     | 15   | A     |
| 2    | C     |     | 9    | E     |     | 16   | H     |
| 3    | E     |     | 10   | B     |     | 17   | A + I |
| 4    | C     |     | 11   | B     |     | 18   | F     |
| 5    | C     |     | 12   | G     |     | 19   | F     |
| 6    | B     |     | 13   | G     |     | 20   | C     |
| 7    | D     |     | 14   | A     |     | 21   | F     |

---

## Phase A — Packaging & repo hygiene

**Goal:** zero-risk fixes to package metadata, license, placeholder crates, CI, and the
plan file itself. No functional code changes.

**Files:** root `Cargo.toml`, `LICENSE` (new), `.github/workflows/ci.yml` (new),
`crates/cme-interp/src/lib.rs`, `crates/cme-runtime/src/lib.rs`,
`crates/cme-interp/Cargo.toml`, `crates/cme-runtime/Cargo.toml`, `AGENTS.md`, this file.

### Steps

- [ ] **A.1 — Land this plan as `plan.md`.**
      Place this file at the repository root named `plan.md`. Commit:
      `jj new -m "docs: add refactor plan (plan.md)"`.

- [ ] **A.2 — License + repository URL + categories (review item 15).**
  - Download the canonical Apache-2.0 text and save it as `/LICENSE`:
    `curl -o LICENSE https://www.apache.org/licenses/LICENSE-2.0.txt`
    (verify the file starts with `                                 Apache License` and is ~11 KiB;
    if the network is unavailable, copy the text from any local Apache-2.0 crate in
    `~/.cargo/registry` — it must be byte-identical to the canonical text).
  - Root `Cargo.toml` `[workspace.package]`:
    - `repository = "https://github.com/cmengine/checkmate"` (owner-confirmed URL).
    - `categories = ["compilers", "parser-implementations"]` (replaces
      `["development-tools"]`; a language front-end is not a dev tool — this improves
      crates.io discoverability).
  - Done-when: `cargo publish --dry-run --workspace` (informational; some crates will
    complain about missing description/docs — that's fine, we only care that metadata
    parses) and the LICENSE file exists at root.

- [ ] **A.3 — Strip placeholder crates to honest shape (review item 14).**
  - `crates/cme-interp/src/lib.rs` → replace entire content with:
    ```rust
    //! Placeholder for the Checkmate interpreter.
    //!
    //! Intentionally empty until the front-end contract (`cme_compiler::parse_source`)
    //! is stable enough to interpret against. Do not add code here without a plan entry.
    ```
  - Same for `crates/cme-runtime/src/lib.rs` (wording: "runtime services and built-ins").
  - In both crates' `Cargo.toml`: change `publish = true` → `publish = false`.
  - This deletes the starter `add()` and its `it_works` test — that is intended.
  - Done-when: `cargo test --workspace` green with the two `it_works` tests gone.

- [ ] **A.4 — CI workflow (review item 17, CI half).**
      Create `.github/workflows/ci.yml`:

  ```yaml
  name: ci
  on:
    push:
    pull_request:
  jobs:
    check:
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
          with:
            components: rustfmt, clippy
        - run: cargo fmt --all -- --check
        - run: cargo clippy --workspace --all-targets -- -D warnings
        - run: cargo test --workspace
        - run: cargo test --workspace --features cli
  ```

  (Phase G will remove the last line when the `cli` feature dies.)
  Done-when: file exists, YAML is valid (e.g. `python -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"` or a CI run).

- [ ] **A.5 — AGENTS.md minimal touch-up.**
  - In "Repository State Notes", delete the sentence "cme-interp and cme-runtime still
    expose starter `add` functions and their tests..." (no longer true after A.3).
  - The `plan.md` instructions in "Working Rules" now point at a real file again —
    leave them as-is (full AGENTS.md rewrite happens in Phase I).
    Done-when: no AGENTS.md statement contradicts the current tree.

**Validation gate:** full gate (section 0.3) green.
**Commits:** A.1 one commit; A.2–A.5 one commit
`chore: packaging hygiene (license, repo url, categories, placeholder crates, ci)`.
---

## Phase B — Lexer: error kinds, float guard, honest recovery

**Goal:** every lexer failure becomes a _classified_ `LexError` variant with a `Display`
message; float literals can no longer silently become `inf` or panic; the recovery loop
is one documented helper that never swallows an error; the `.unwrap()`/`is_err` noise in
`lex_with_errors` is gone.

**Files:** `crates/cme-compiler/src/lexer.rs`, `src/main.rs` (one match arm),
pinned tests in `crates/cme-compiler/src/lib.rs`, `boom.cm` (§9 comments only).

### Steps

- [ ] **B.1 — Float literal guard (review item 10, parts A+B).**
      Replace the `FloatLit` callback:

  ```rust
  #[regex(r"[0-9]+\.[0-9]+", |lex| lex.slice().parse::<f64>().ok().filter(|v| v.is_finite()).ok_or(()))]
  FloatLit(f64),
  ```

  (Mirrors the `IntLit` failure style: a token that cannot be represented fails the
  match and flows into recovery instead of panicking or producing `inf`.)
  Add test: `float_digit_run_overflowing_f64_is_an_error_not_infinity` — lex
  `"float huge = " + "9".repeat(400) + ".0\n"` via `lex_with_errors`, assert exactly 1
  error classified `FloatOverflow` (see B.2) and that the next line still lexes.

- [ ] **B.2 — `LexError` variants + classification (review item 6, option A).**
      Replace the single-variant enum:

  ```rust
  /// A lexer failure. Each variant points at the offending source region.
  #[derive(Debug, PartialEq, Eq, Clone)]
  pub enum LexError {
      /// A character that cannot begin any token (e.g. `@`, `$`, a stray `.`).
      InvalidCharacter { span: Span },
      /// A `"` with no closing `"` before the end of the line/file.
      UnterminatedString { span: Span },
      /// An integer literal whose digit run does not fit in `i64`.
      IntegerOverflow { span: Span },
      /// A float literal that would parse to infinity.
      FloatOverflow { span: Span },
  }

  impl LexError {
      pub fn span(&self) -> Span {
          match self {
              LexError::InvalidCharacter { span }
              | LexError::UnterminatedString { span }
              | LexError::IntegerOverflow { span }
              | LexError::FloatOverflow { span } => *span,
          }
      }
  }

  impl fmt::Display for LexError {
      fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
          let msg = match self {
              LexError::InvalidCharacter { .. } => "invalid character",
              LexError::UnterminatedString { .. } => "unterminated string literal",
              LexError::IntegerOverflow { .. } => "integer literal is too large",
              LexError::FloatOverflow { .. } => "float literal is too large",
          };
          f.write_str(msg)
      }
  }
  ```

  Add the classifier (free function in `lexer.rs`):

  ```rust
  /// Chooses the `LexError` variant for a failed region by inspecting the source
  /// text: a leading `"` means an unterminated string; an all-digit run is integer
  /// overflow; a `digits.digits` shape is float overflow; anything else is a bad
  /// character.
  fn classify_error(source: &str, span: Span) -> LexError {
      let text = &source[span.start..span.end];
      if text.starts_with('"') {
          LexError::UnterminatedString { span }
      } else if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
          LexError::IntegerOverflow { span }
      } else if is_float_shape(text) {
          LexError::FloatOverflow { span }
      } else {
          LexError::InvalidCharacter { span }
      }
  }

  fn is_float_shape(text: &str) -> bool {
      match text.split_once('.') {
          Some((int_part, frac_part)) => {
              !int_part.is_empty()
                  && !frac_part.is_empty()
                  && int_part.bytes().all(|b| b.is_ascii_digit())
                  && frac_part.bytes().all(|b| b.is_ascii_digit())
          }
          None => false,
      }
  }
  ```

  Notes:
  - Logos error spans are byte ranges at token boundaries, so slicing `source` is safe.
  - A digit-only error span can only be an overflow (a representable digit run would
    have matched `IntLit`); a `d+.d+` span can only be a float overflow after B.1.
  - `1.2.3` fails at the stray `.` (span is `.` or `.3`, both classify as
    `InvalidCharacter` — correct).
  - `Display` is added here (review 6 chose variants; review items 7/8 need each
    variant to render itself — this is where the three hard-coded "invalid token"
    strings begin to die).
    Add `use std::fmt;` at the top of `lexer.rs`.

- [ ] **B.3 — Honest, readable recovery loop (review item 11 — "most future-proof and
      idiomatic", which the owner delegated; the chosen design is (a)+(b): one documented
      helper + record every error seen while resyncing, so **no error is ever swallowed** —
      matching test2.cm's own stated contract. Valid tokens between the error and the
      newline are still dropped (option (c) was explicitly not chosen; line-granular
      recovery is the established design).**
      Rewrite the error arm of `lex_with_errors` and extract the skipper:

  ```rust
  pub fn lex_with_errors(source: &str) -> (Vec<SpannedToken<'_>>, Vec<LexError>) {
      let mut tokens = Vec::new();
      let mut errors = Vec::new();
      let mut lexer = Token::lexer(source);

      while let Some(result) = lexer.next() {
          let span = Span::new(lexer.span().start, lexer.span().end);
          match result {
              Ok(token) => tokens.push(SpannedToken { token, span }),
              Err(()) => {
                  errors.push(classify_error(source, span));
                  if let Some(newline_span) =
                      skip_to_line_end(&mut lexer, source, &mut errors)
                  {
                      tokens.push(SpannedToken {
                          token: Token::Newline,
                          span: newline_span,
                      });
                  }
              }
          }
      }

      (tokens, errors)
  }

  /// Resynchronizes after a lexing error: consumes tokens up to and including the
  /// next newline so the damaged line stays line-granular, and records every lexer
  /// error encountered on the way — errors are never swallowed. Valid tokens inside
  /// the damaged region are dropped (recovery keeps statement boundaries only).
  /// Returns the newline's span if the region ended at a line break.
  fn skip_to_line_end<'src>(
      lexer: &mut logos::Lexer<'src, Token<'src>>,
      source: &'src str,
      errors: &mut Vec<LexError>,
  ) -> Option<Span> {
      while let Some(result) = lexer.next() {
          let span = Span::new(lexer.span().start, lexer.span().end);
          match result {
              Ok(Token::Newline) => return Some(span),
              Ok(_) => {} // dropped: part of the damaged region
              Err(()) => errors.push(classify_error(source, span)),
          }
      }
      None
  }
  ```

  Notes:
  - This removes: the triple condition `skipped.is_ok() && lexer.span().end > span.end
&& matches!(...)` (checked "ok" twice), the stale-span comparison, the duplicated
    Newline push block, and the `result.unwrap()` at the old push site.
  - The old `lexer.span().end > span.end` guard is subsumed: the returned newline was
    produced by `lexer.next()` after the failed region, so it necessarily starts past
    the error.
  - **Behavior change (intended):** consecutive errors on one line are now all
    recorded. Whether `@ $` yields one merged or two separate `Err`s depends on logos;
    whichever it is, nothing is dropped. Expect boom.cm's `errors.len() == 57` and
    `lex count == 7` pins to possibly become 58/8; **run the tests, take the observed
    numbers, and update the pins and the boom.cm §9 comment lines** ("STILL 1 lex
    diagnostic — everything after the first bad char is swallowed" → describe the new
    honest behavior) in this same commit.
  - Add a unit test pinning the multi-bad-char behavior:
    `multiple_bad_chars_on_one_line_are_all_reported` with source `"@ $\nint b = 1\n"`.
    Assert: however many errors logos emits (1 merged or 2 separate), _every_ one is
    present in `errors` and the `int b = 1` line still lexes. Write the assertion
    against the observed count once, and pin it.

- [ ] **B.4 — Update the one downstream match in the CLI.**
      `src/main.rs:72-75` matches `Diagnostic::Lex(LexError::Invalid { span })`, which no
      longer compiles. Change the arm to:

  ```rust
  Diagnostic::Lex(error) => error.span(),
  ```

  (The `"invalid token"` message fallback in `main.rs` stays for now — it is removed in
  Phase D.)

- [ ] **B.5 — Update lexer unit tests for the new shape.**
  - `recovers_from_invalid_token_at_next_newline`: `errors[0]` is now
    `LexError::InvalidCharacter { span: Span::new(10, 11) }`.
  - `digit_run_overflowing_i64_is_an_error_not_a_panic`: error is
    `LexError::IntegerOverflow { span }`.
  - `rejects_unterminated_string_literals`: additionally assert the variant is
    `UnterminatedString` via `lex_with_errors`.
  - Keep `rejects_unrecognized_characters` (`lex("$").is_err()` still holds).

**Validation gate:** green; boom.cm pins (re)verified and updated in-commit.
**Commit:** `refactor: classify lexer errors and never swallow resync errors`.

---

## Phase C — Parser mechanics: Eof token, Copy tokens, cursor helpers

**Goal:** end-of-input becomes a real `Token::Eof` appended by the lexer, so
`Parser::new` stops threading `source_len`, `advance()` becomes infallible and
saturating, and the pos-overrun workaround dies. Tokens become `Copy`, killing the
`.cloned()` noise. The type-keyword set lives in one method. The speculative brace
tokens and dead `can_end_statement` entries are removed. **All user-visible message
strings are preserved in this phase** (message overhaul is Phase D).

**Files:** `crates/cme-compiler/src/lexer.rs`, `crates/cme-compiler/src/parser.rs`,
`crates/cme-compiler/src/lib.rs` (tests), `src/main.rs`.

### Steps

- [ ] **C.1 — `Token` becomes `Copy`; add `Token::Eof` (review items 5 and 2).**
  - `#[derive(Logos, Debug, PartialEq, Eq, Clone, Copy)]` on `Token` (all payloads —
    `&str`, `i64`, `f64`, unit — are `Copy`; logos supports `Copy` tokens).
    Also `#[derive(Debug, PartialEq, Eq, Clone, Copy)]` on `SpannedToken`.
    `PartialEq` on floats: use `PartialEq` (not `Eq`) as today for `Token`.
  - Add the variant **last** in the enum, with a comment:
    ```rust
    /// Synthetic end-of-input marker appended by the lexer. Never produced by a
    /// regex; the parser relies on it to make `advance` infallible.
    Eof,
    ```
  - Append it in `lex_with_errors` after the loop (and therefore `lex` too):
    ```rust
    let eof_span = Span::new(source.len(), source.len());
    tokens.push(SpannedToken { token: Token::Eof, span: eof_span });
    ```
    Note the zero-width span — the old fabricated `(len, len+1)` span violated the
    "spans are valid byte ranges" invariant. The CLI caret already does `.max(1)`.

- [ ] **C.2 — Remove the speculative brace tokens (review item 20).**
  - Delete `LBrace` and `RBrace` from `Token`.
  - **Intended behavior change:** `{` and `}` are now lex errors
    (`InvalidCharacter`), so every bare-brace fixture line changes from a parse error
    to a lex error. Affected boom.cm sections: §1, §6 (`}` head), §11 (empty groups),
    §14 (trailing `}`). Run the suite, update the pinned counts and the boom.cm
    comments that describe `}` behavior in the same commit. (The tokens return
    together with the real blocks feature, with tests for newlines inside braces.)
  - Also remove `Token::RBrace` and `Token::KwReturn` from `can_end_statement`
    (both were speculative — no current statement form can end in them). The remaining
    entries (`Ident`, `IntLit`, `FloatLit`, `StrLit`, `KwTrue`, `KwFalse`, `RParen`)
    are all reachable today.

- [ ] **C.3 — One home for the type-keyword set (review item 4).**

  ```rust
  impl<'a> Token<'a> {
      /// The keywords that can start a variable declaration.
      fn is_type_keyword(&self) -> bool {
          matches!(
              self,
              Token::KwInt | Token::KwFloat | Token::KwStr | Token::KwBool | Token::KwInfer
          )
      }
  }
  ```

  Replace the three _membership_ sites (parser.rs `next_starts_statement` closure,
  `prev_was_type_kw` assignment, `parse_statement` dispatch). The fourth site
  (`parse_variable_declaration`'s keyword→`Type` mapping) is a conversion, not a
  membership test — it stays a `match`.

- [ ] **C.4 — Cursor helpers on `Parser` (review items 2 and 5).**
      New `Parser::new` and helpers:

  ```rust
  pub struct Parser<'a, 'src> {
      tokens: &'a [SpannedToken<'src>],
      pos: usize,
      errors: Vec<Diagnostic>,
  }
  // eof_span field deleted — the Eof token's span is eof: self.tokens.last().

  impl<'a, 'src> Parser<'a, 'src> {
      pub fn new(tokens: &'a [SpannedToken<'src>]) -> Self {
          debug_assert!(
              matches!(tokens.last(), Some(t) if t.token == Token::Eof),
              "token stream must end with a synthetic Eof"
          );
          Self { tokens, pos: 0, errors: Vec::new() }
      }

      /// Current token. Always valid: the stream ends with `Eof`.
      fn peek(&self) -> &SpannedToken<'src> {
          &self.tokens[self.pos]
      }

      /// The end-of-file span (zero-width, at end of input).
      fn eof_span(&self) -> Span {
          self.tokens.last().map_or(Span::new(0, 0), |t| t.span)
      }

      /// True when the current token is `kind` (use with unit variants).
      fn at(&self, kind: Token<'src>) -> bool {
          self.peek().token == kind
      }

      /// Consumes the current token if it is `kind`. Returns whether it was.
      fn eat(&mut self, kind: Token<'src>) -> bool {
          if self.at(kind) {
              self.pos += 1;
              true
          } else {
              false
          }
      }

      /// Consumes and returns the current token; saturates at `Eof`.
      fn advance(&mut self) -> &SpannedToken<'src> {
          let token = &self.tokens[self.pos];
          if token.token != Token::Eof {
              self.pos += 1;
          }
          token
      }

      fn at_eof(&self) -> bool {
          self.at(Token::Eof)
      }
  }
  ```

  Then:
  - `skip_newlines` → `while self.at(Token::Newline) { self.pos += 1; }`.
  - `parse_program_with_errors` loop: `self.skip_newlines(); if self.at_eof() { break; }`
    and the trailing-token match becomes (bind-then-match, since `Token` is `Copy` —
    matching `self.peek().token` directly would hold the borrow through the arms):
    ```rust
    let token = self.peek().token;
    match token {
        Token::Newline => self.pos += 1,
        Token::Eof => {}
        _ => {
            let span = self.peek().span;
            self.errors.push(Diagnostic::Parse {
                message: format!(
                    "expected end of statement (newline), found {:?}",
                    token
                ),
                span,
            });
            self.skip_to_next_statement();
        }
    }
    ```
    (Exact old message kept — including the `(newline)` hint and `{:?}` — Phase D
    restyles it.)
  - `require_token`:
    ```rust
    fn require_token(&mut self, message: &str) -> Result<SpannedToken<'src>, Diagnostic> {
        let token = *self.advance();
        if token.token == Token::Eof {
            return Err(Diagnostic::Parse {
                message: message.to_string(),
                span: token.span,
            });
        }
        Ok(token)
    }
    ```
  - **Delete** the pos-overrun workaround in `parse_recovered_expression` (the whole
    `if self.pos > start_pos { ... .or_else(|| self.tokens.last()) ... }` block plus its
    4-line comment). With saturating `advance`, `self.tokens.get(self.pos - 1)` is
    always valid; simplify to:
    ```rust
    let mut end = start;
    if self.pos > start_pos {
        end = self.tokens[self.pos - 1].span.end;
    }
    ```
  - Every `self.peek().cloned()` / `peek().map(|t| t.token.clone())` becomes
    `*self.peek()` / `self.peek().token` (`Copy`). Every `self.pos += 1` that means
    "consume the token I just peeked" becomes `self.advance()` or `self.eat(kind)`
    (e.g. the `Token::Assign` arm in `parse_variable_declaration` becomes
    `self.eat(Token::Assign)` inside the match arm — mind borrow rules: bind
    `let token = self.peek().token;` first, then match).
  - `parse_statement`'s empty-stream branch (`let Some(first) = ... else`) becomes an
    `at_eof()` check before anything else; the `first` token is then always real
    (`*self.advance()`).
  - In `parse_primary`, keep the explicit `Token::Newline` arm (same message), and let
    `Eof` fall into the generic error arm but special-case its message to the old
    "Expected an expression, but reached end of file" string with the Eof span —
    Phase D unifies the wording.

- [ ] **C.5 — Strip pass loses `source_len` (review item 2 ripple).**
  - `Parser::strip_insignificant_newlines(tokens)` and
    `..._with_errors(tokens)` drop the `source_len: usize` parameter.
  - The "unbalanced opening parenthesis" error used `Span::new(source_len, source_len + 1)`;
    it now uses the Eof token's span: find it via
    `tokens.last().filter(|t| t.token == Token::Eof)` (the stream always ends with Eof),
    i.e. a zero-width span at end of input.
  - The Eof token passes through the `_` arm of the strip loop (pushed, and
    `can_end_statement(Eof)` is false, so a newline before it still counts as ending a
    statement — current behavior preserved).
  - Update **every** call site (search for `Parser::new(` and
    `strip_insignificant_newlines`): `src/main.rs` (two places), and the many test
    helpers/cases in `crates/cme-compiler/src/lib.rs` (mechanical: remove the
    `, source.len()` argument).

- [ ] **C.6 — Verify no behavior changed beyond the intended brace removal.**
      Run the suite. Expected diffs and only these:
  - Brace lines in boom.cm shift from parse to lex diagnostics; pins updated.
  - The "unbalanced opening parenthesis" span becomes `(len, len)` instead of
    `(len, len+1)` — if any test pinned it, update the pin.
    Everything else (message strings, counts for non-brace lines, AST shapes) identical.

**Validation gate:** green.
**Commit:** `refactor: synthetic Eof token, Copy tokens, and unified cursor helpers`.

---

## Phase D — Diagnostics model: flattened `Diagnostic`, consistent messages

**Goal:** `Diagnostic` becomes a single struct with `kind`/`message`/`span` and public
accessors — no consumer ever matches on it again. All parser messages are built through
one `expected()` helper with `Token::describe()`, lowercase, and human-readable. The
dead `From<Diagnostic> for String` dies. A new `diagnostics` module becomes the home
for everything error-reporting (preparing Phase E/F).

**Files:** new `crates/cme-compiler/src/diagnostics.rs`,
`crates/cme-compiler/src/lib.rs`, `crates/cme-compiler/src/parser.rs`,
`crates/cme-compiler/src/lexer.rs` (describe), `src/main.rs`, pinned tests.

### Steps

- [ ] **D.1 — Create `diagnostics.rs` and move `Diagnostic` there (review item 7,
      option B).**

  ```rust
  //! The diagnostics model shared by every compiler stage.

  use crate::lexer::LexError;
  use cme_core::Span;
  use std::fmt;

  /// What stage produced a diagnostic.
  #[derive(Debug, PartialEq, Eq, Clone)]
  #[non_exhaustive] // (D2 default: compiler-boundary types only)
  pub enum DiagnosticKind {
      /// A lexer failure, with its specific shape.
      Lex(LexError),
      /// A parse-stage failure (free-form message).
      Parse,
  }

  /// A single diagnostic: what went wrong, where, and from which stage.
  ///
  /// Consumers never match on this — use `message()`, `span()`, and `kind()`.
  #[derive(Debug, PartialEq, Eq, Clone)]
  #[non_exhaustive]
  pub struct Diagnostic {
      kind: DiagnosticKind,
      message: String,
      span: Span,
  }

  impl Diagnostic {
      /// Wraps a lexer error; message and span come from the error itself.
      pub fn lex(error: LexError) -> Self {
          Self {
              span: error.span(),
              message: error.to_string(),
              kind: DiagnosticKind::Lex(error),
          }
      }

      /// A parse-stage diagnostic pointing at `span`.
      pub fn parse(message: impl Into<String>, span: Span) -> Self {
          Self {
              kind: DiagnosticKind::Parse,
              message: message.into(),
              span,
          }
      }

      pub fn kind(&self) -> &DiagnosticKind {
          &self.kind
      }

      pub fn message(&self) -> &str {
          &self.message
      }

      pub fn span(&self) -> Span {
          self.span
      }
  }

  impl fmt::Display for Diagnostic {
      fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
          f.write_str(&self.message)
      }
  }
  ```

  In `cme-compiler/src/lib.rs`: add `pub mod diagnostics;` **above** `pub mod lexer;`,
  delete the `Diagnostic` enum from `parser.rs` (including `From<Diagnostic> for
String`, the private `span()`, and the old `Display` — all dead or replaced), and
  re-export for convenience: `pub use diagnostics::{Diagnostic, DiagnosticKind};`.
  Update imports in `parser.rs`, `main.rs`, and tests
  (`use cme_compiler::diagnostics::Diagnostic;`).
  - Where `Diagnostic::Parse { message, span }` was constructed in `parser.rs`, use
    `Diagnostic::parse(message, span)`.
  - Where `Diagnostic::Lex(...)` was constructed in `main.rs` and tests, use
    `Diagnostic::lex(error)`.
  - `main.rs::render_error` stops matching variants entirely:
    `let span = error.span();` and `error.message().to_string()`; delete the
    "invalid token" fallback string.
  - Note the deliberate `kind`/`span` duplication for lex errors
    (`Diagnostic::span() == kind.lex.span()`): the constructor maintains the invariant.

- [ ] **D.2 — `Token::describe()` (review item 8, part A).**
      Add to `lexer.rs` (full table in Appendix A):

  ```rust
  impl<'a> Token<'a> {
      /// A human-readable name for use in diagnostics, e.g.
      /// "end of statement", "identifier `x`", "`==`".
      pub fn describe(&self) -> String {
          match self {
              Token::Newline => "end of statement".into(),
              Token::Eof => "end of file".into(),
              Token::Ident(name) => format!("identifier `{name}`"),
              Token::StrLit(_) => "string literal".into(),
              Token::IntLit(value) => format!("integer literal `{value}`"),
              Token::FloatLit(value) => format!("float literal `{value}`"),
              Token::KwInt => "`int`".into(),
              // ... every keyword/operator: backticked source text
          }
      }
  }
  ```

  Add a unit test covering every variant (exhaustive match guarantees the compiler
  reminds you when a variant is added — that is the point).

- [ ] **D.3 — The `expected()` message builder (review item 8, part B).**
      In `parser.rs`:

  ```rust
  /// Builds the canonical "expected X, but found Y" diagnostic.
  /// `span` must point at the offending (found) token.
  fn expected(what: &str, found: &Token<'_>, span: Span) -> Diagnostic {
      Diagnostic::parse(
          format!("expected {what}, but found {}", found.describe()),
          span,
      )
  }
  ```

  Rewrite every message construction to use it (or `Diagnostic::parse` for the
  non-"expected" messages). The complete old→new mapping is **Appendix B**; the rules:
  - all messages lowercase;
  - `{:?}` token dumps → `describe()`;
  - "reached end of file" → "found end of file" (flows automatically from
    `describe(Eof)` — delete the special-case EOF branches from Phase C);
  - the `Newline` special cases collapse ("found end of statement" now comes from
    `describe(Newline)`);
  - `'='`/`')'` quoted names become backticked: ``expected `=`, but found ...``.
    Keep as plain `Diagnostic::parse` literals: "unbalanced closing parenthesis",
    "unbalanced opening parenthesis", "mixed && and || require parentheses" (moves to
    the validator in Phase E — text unchanged), "comparisons are non-associative; add
    parentheses", "unexpected end of file".

- [ ] **D.4 — Update every pinned string.**
      All tests asserting message substrings (`"Expected a variable name"`, `"Expected '='"`,
      `"reached end of file"`, `"expected end of statement (newline)"`, ...) get the new
      strings from Appendix B. Also update the boom.cm **comment lines** that quote message
      text (§3, §6, §7, §8, §12, §13) so the file keeps documenting reality.
      Sweep: `rg -n "Expected" crates/` and `rg -n "reached end of file"` must return
      nothing user-facing afterwards.

- [ ] **D.5 — Delete the leftovers.**
  - Dead `From<Diagnostic> for String` — already deleted in D.1; verify.
  - `parser.rs::parse_error` free function: fold into `expected`/`Diagnostic::parse`
    (its two remaining call sites in the strip pass become `Diagnostic::parse`
    literals).

**Validation gate:** green; message sweep clean.
**Commit:** `refactor: flatten Diagnostic into a struct with consistent messages`.

---

## Phase E — AST overhaul: spans, `Paren`, validator, error ids

**Goal:** the AST becomes rustc-style `Expr { span, kind }` / `Stmt { span, kind }`
with real spans on every node; grouping is preserved as `ExprKind::Paren` and the
mixed `&&`/`||` rule becomes a post-parse validator (deleting the `LogicalKind`
threading and the `(0,0)` span stub); `Invalid` nodes reference the canonical
diagnostics list by `ErrorId` instead of cloning a `SyntaxError`.

This is the largest phase. Execute the five sub-steps **in order, keeping the suite
green after each** (commit per sub-step is allowed and encouraged).

**Files:** `crates/cme-core/src/lib.rs`, `crates/cme-compiler/src/parser.rs`, new
`crates/cme-compiler/src/validate.rs`, `crates/cme-compiler/src/lib.rs` (tests),
`src/main.rs` (only if it names AST nodes — it does not).

### Steps

- [ ] **E.1 — `ErrorId` replaces embedded `SyntaxError` (review item 9, option B).**
      In `cme-core`:

  ```rust
  /// Index into the canonical diagnostics list produced alongside the AST.
  /// `Invalid` nodes carry one so the tree and the diagnostics can never disagree.
  #[derive(Debug, PartialEq, Eq, Clone, Copy)]
  pub struct ErrorId(pub usize);
  ```
  - Delete `SyntaxError` from `cme-core` (public removal at 0.1.0 is fine).
  - `Expr::Invalid { error: SyntaxError, span }` → `Expr::Invalid { error: ErrorId }`
    (span stays inside the variant for now; it moves up in E.2). Same for `Stmt`.
  - In `parser.rs`, `record` becomes id-producing:
    ```rust
    fn record(&mut self, message: impl Into<String>, span: Span) -> ErrorId {
        let id = ErrorId(self.errors.len());
        self.errors.push(Diagnostic::parse(message, span));
        id
    }
    ```
  - `parse_recovered_expression`'s error path becomes: push the failed `diagnostic`
    itself, take its index as the id, plant `Invalid { error: id, span }`. No more
    `diagnostic.to_string()` + clone.
  - `missing_expression` plants `Invalid { error: self.record(...), span: Span::missing(offset) }`.
  - Test churn: every `error.message.contains(...)` inside an `Invalid` match becomes
    `errors[0].message().contains(...)` (the errors vec is already in hand in those
    tests); `SyntaxError::new(...)` constructions in cme-core tests become
    `ErrorId(0)`.
  - Invariant to verify: for every `Invalid { error, .. }` node,
    `outcome.diagnostics[error.0]` exists (add a debug walk in tests if handy).

- [ ] **E.2 — rustc-style nodes with spans (review item 3, spans half).**
      In `cme-core`:

  ```rust
  #[derive(Debug, Clone)]
  pub struct Expr {
      /// Byte range of the source this expression covers.
      pub span: Span,
      pub kind: ExprKind,
  }

  #[derive(Debug, Clone)]
  pub enum ExprKind {
      IntLit(i64),
      FloatLit(f64),
      StrLit(String),
      BoolLit(bool),
      Ident(String),
      Binary {
          op: BinaryOp,
          lhs: Box<Expr>,
          rhs: Box<Expr>,
      },
      Unary {
          op: UnaryOp,
          expr: Box<Expr>,
      },
      /// A parenthesized group. Preserves grouping for validation and tooling;
      /// a future lowering pass may unwrap it.
      Paren {
          expr: Box<Expr>,
      },
      /// A region the parser could not interpret. `error` indexes the canonical
      /// diagnostics list; `span` is the skipped region (zero-width when the
      /// source is missing entirely).
      Invalid {
          error: ErrorId,
      },
  }

  /// Structural equality only — spans are deliberately excluded so tests can
  /// assert tree shapes without span data.
  impl PartialEq for Expr {
      fn eq(&self, other: &Self) -> bool {
          self.kind == other.kind
      }
  }
  ```

  Same treatment for `Stmt`/`StmtKind` (`VarDecl { ty, name, expr }`, `Assign`,
  `CompoundAssign`, `Invalid { error }`) with span-ignoring `PartialEq`.
  Construction helper for real code:

  ```rust
  impl Expr {
      pub fn new(kind: ExprKind, span: Span) -> Self { Self { span, kind } }
  }
  ```

  In `parser.rs`, fill spans mechanically:
  - literals/idents: the token's span;
  - `Binary`: `Span::new(lhs.span.start, rhs.span.end)`;
  - `Unary`: `Span::new(op_token.span.start, inner.span.end)`;
  - `Paren`: `Span::new(lparen.span.start, rparen.span.end)`;
  - statements: first token's start … last consumed token's end
    (`parse_variable_declaration` knows `type_token.span.start`; the statement end is
    the expression's `span.end` or the skipped region's end).
    Test churn: the existing test helpers get span-agnostic constructors:

  ```rust
  fn int(v: i64) -> Expr { Expr::new(ExprKind::IntLit(v), Span::new(0, 0)) }
  // float, str_lit, ident, bool, bin, unary similarly; var_decl/assign/compound
  // likewise for Stmt. Span assertions stay as direct `.span` field checks.
  ```

  (Because `PartialEq` ignores spans, the old expected trees keep working with dummy
  spans.)
  `contains_invalid` walks `kind` (recurse through `Binary`, `Unary`, `Paren`).

- [ ] **E.3 — `Paren` node + post-parse validator (review item 3, validator half).**
  - `parse_primary`'s `LParen` arm wraps: `Expr::new(ExprKind::Paren { expr: Box::new(expr) }, group_span)`.
  - **Delete** `LogicalKind`, `combine_operand_kind`, `expr_span`, and the mixed-logic
    checks inside `parse_logic_or`/`parse_logic_and`. All precedence functions now
    return plain `Result<Expr, Diagnostic>` / `Expr` — the tuple plumbing disappears.
  - The chained-comparison check stays in `parse_comparison` (it is a one-token
    lookahead, not threading).
  - New `crates/cme-compiler/src/validate.rs`:
    ```rust
    //! Post-parse validation over the spanned AST.

    use crate::diagnostics::Diagnostic;
    use cme_core::Span;
    use cme_core::ast::{BinaryOp, Expr, ExprKind, Stmt};

    /// Enforces: mixed `&&`/`||` require parentheses.
    ///
    /// A `Binary{op: Or}` operand may not contain a bare (unparenthesized)
    /// `Binary{op: And}` anywhere inside, and vice versa. `Paren` is the barrier.
    pub(crate) fn validate(stmts: &[Stmt], diagnostics: &mut Vec<Diagnostic>) {
        for stmt in stmts {
            validate_expr(stmt.expression(), diagnostics);
        }
    }

    fn validate_expr(expr: &Expr, diagnostics: &mut Vec<Diagnostic>) {
        match &expr.kind {
            ExprKind::Binary { op, lhs, rhs } => {
                let opposite = match op {
                    BinaryOp::Or => BinaryOp::And,
                    BinaryOp::And => BinaryOp::Or,
                    _ => return recurse(expr, diagnostics), // hmm — see note
                };
                for operand in [lhs, rhs] {
                    if contains_bare_op(operand, opposite) {
                        diagnostics.push(Diagnostic::parse(
                            "mixed && and || require parentheses",
                            operand.span,
                        ));
                        return; // one report per expression is enough
                    }
                }
                recurse(expr, diagnostics);
            }
            _ => recurse(expr, diagnostics),
        }
    }

    /// Does `expr` contain a `Binary{op}` not protected by a `Paren`?
    fn contains_bare_op(expr: &Expr, op: BinaryOp) -> bool {
        match &expr.kind {
            ExprKind::Paren { .. } => false,
            ExprKind::Binary { op: found, lhs, rhs } => {
                *found == op || contains_bare_op(lhs, op) || contains_bare_op(rhs, op)
            }
            ExprKind::Unary { expr, .. } => contains_bare_op(expr, op),
            _ => false,
        }
    }
    ```
    Implementation note: write `recurse` as a plain walk that visits every subexpression
    and calls the Binary check on each (the sketch above indicates intent; structure it
    as: walk all nodes, and for each `Binary{And|Or}` check both operands for the bare
    opposite op, reporting at the _operand's_ span — which is now a real span, fixing
    the old `(0,0)` "renders at 1:1" bug).
  - Wiring — **avoid double-reporting**: `parse_program_with_errors` calls the internal
    statement parser in its loop, then runs `validate` once over all statements before
    returning; the public single-statement entry (`parse_statement`) validates just its
    one statement before returning. Restructure: private `parse_statement_inner` +
    public wrappers.
  - Test cases to add/keep (from test.cm/boom.cm semantics):
    `a && b && c` clean · `a || b || c` clean · `a || (b && c)` clean ·
    `(a || b) && c` clean · `a || b && c` violation at the `And` operand's span ·
    `alive && ready || armed` violation · `!(a && b) || c` clean (Unary→Paren barrier).
  - **Intended behavior change:** `infer mixedBad = alive && ready || armed` now parses
    into a _full tree_ plus one validator diagnostic, instead of an `Invalid` RHS. The
    statement survives (better for tooling; execution gates on non-empty diagnostics
    either way). Update boom.cm §12 comments and the mega-test: the `mixedBad` entry
    moves out of the "carries an Invalid" lists.

- [ ] **E.4 — Verify `PartialEq` and doctest of the new equality semantics.**
  - Existing tree-shape tests pass unchanged thanks to span-ignoring equality.
  - Span-critical tests (`missing_spans_are_zero_width`, all `Span::missing(...)`
    assertions) now read `expr.span` / `stmt.span` directly.
  - Add one test asserting spans are ignored: two `IntLit(1)` exprs with different
    spans compare equal (documents the contract).

- [ ] **E.5 — (D1, if confirmed) `Option<Type>` replaces `Type::Infer`.**
  - `StmtKind::VarDecl { ty: Option<Type>, ... }`; `None` = `infer`.
  - Delete `Type::Infer`.
  - `parse_variable_declaration`: `KwInfer => None`, others => `Some(Type::X)`.
  - Update test helpers: `var_decl(None, ...)` / `var_decl(Some(Type::Int), ...)`.
  - If D1 was declined, skip and keep `Type::Infer` + `ty: Type`.

**Validation gate:** green after each sub-step.
**Commits:** `refactor: error ids reference the canonical diagnostics list`,
`refactor: span-carrying AST with Paren nodes and a post-parse validator`,
(and optionally `refactor: model infer as an absent type annotation`).

---

## Phase F — Public API: `parse_source`, `ParseOutcome`, facade, crate docs

**Goal:** `cme-compiler` finally owns the whole pipeline: one call, source in, tree +
diagnostics out. The root facade stops shadowing `core`, the leaks (`pub use logos`,
partial `Span` re-export) die, and every crate gets real documentation.

**Files:** `crates/cme-compiler/src/lib.rs`, `crates/cme-compiler/src/diagnostics.rs`,
`crates/cme-core/src/lib.rs`, `src/lib.rs`, `crates/cme-compiler/src/lexer.rs` +
`parser.rs` + `src/main.rs` (import touches), tests.

### Steps

- [ ] **F.1 — `ParseOutcome` (review items 1 and 19).**
      In `diagnostics.rs`:

  ```rust
  use cme_core::ast::{ErrorId, Stmt};

  /// The result of running the full front-end pipeline over a source file.
  ///
  /// The statements and the diagnostics travel together: `Invalid` AST nodes
  /// reference diagnostics by [`ErrorId`], so keep this struct intact when
  /// passing results around.
  #[derive(Debug, Clone)]
  #[non_exhaustive]
  pub struct ParseOutcome {
      pub statements: Vec<Stmt>,
      pub diagnostics: Vec<Diagnostic>,
  }

  impl ParseOutcome {
      /// True when the source had no diagnostics — the gate execution
      /// consumers should check before running anything.
      pub fn is_clean(&self) -> bool {
          self.diagnostics.is_empty()
      }

      /// The diagnostic an `Invalid` node's `ErrorId` points at.
      pub fn error(&self, id: ErrorId) -> Option<&Diagnostic> {
          self.diagnostics.get(id.0)
      }
  }
  ```

- [ ] **F.2 — `parse_source` — the pipeline in one place (review item 1).**
      In `cme-compiler/src/lib.rs`:

  ````rust
  /// Runs the whole front-end over `source`: lexing, insignificant-newline
  /// stripping, parsing (with error recovery), and post-parse validation.
  ///
  /// This is the entry point embedders and tools should use. The individual
  /// stages stay public for advanced use (see `lexer` and `parser` modules).
  ///
  /// ```rust
  /// # use cme_compiler::parse_source;
  /// let outcome = parse_source("int hp = 100\nhp += 5\n");
  /// assert!(outcome.is_clean());
  /// assert_eq!(outcome.statements.len(), 2);
  /// ```
  pub fn parse_source(source: &str) -> ParseOutcome {
      let (tokens, lex_errors) = lexer::lex_with_errors(source);
      let mut diagnostics: Vec<Diagnostic> =
          lex_errors.into_iter().map(Diagnostic::lex).collect();
      let (tokens, strip_errors) =
          parser::Parser::strip_insignificant_newlines_with_errors(tokens);
      diagnostics.extend(strip_errors);
      let (statements, parse_errors) =
          parser::Parser::new(&tokens).parse_program_with_errors();
      diagnostics.extend(parse_errors);
      ParseOutcome { statements, diagnostics }
  }
  ````

  (The validator already ran inside `parse_program_with_errors` per E.3, so its
  diagnostics are inside `parse_errors`.)
  - Rewrite the duplicated pipeline in `src/main.rs::run()` (both the `lex` and `ast`
    arms) to use `lexer::lex_with_errors` + `parse_source` — the manual strip/parse
    choreography there is deleted. (The `lex` arm keeps using the lexer directly since
    it prints tokens.)
  - Rewrite the test helper `parse_program_parts` in `cme-compiler/src/lib.rs` to call
    `parse_source` (its assertions keep working — error ordering is unchanged:
    lex, then strip, then parse).

- [ ] **F.3 — Delete the leaks (review item 18).**
  - Remove `pub use logos;` from `cme-compiler/src/lib.rs` (nothing in-repo uses it;
    logos stops being part of the public API surface).
  - Remove `pub use ast::Span;` from `cme-core/src/lib.rs` — one canonical path
    (`cme_core::ast::Span`). Update the imports in `lexer.rs`, `parser.rs`,
    `diagnostics.rs`, `validate.rs`, and tests (`use cme_core::ast::Span;`).
  - Merge `lex_tokens`/`lex_ok` test helpers in `lexer.rs`: keep one (delete
    `lex_tokens`, move its body into `lex_ok`).
  - Keep `lexer::lex` and `Parser::parse_program` (the fail-fast, `Result`-based
    embedding API) but give both doc comments cross-referencing `parse_source`
    ("use `parse_source` for tolerant parsing with recovery; use this when any
    error must abort").

- [ ] **F.4 — Facade: rename `core`, expose the entry point (review item 19).**
      `src/lib.rs` becomes:

  ```rust
  //! `cme` — the Checkmate Engine facade.
  //!
  //! Re-exports the workspace crates as feature-gated modules. `cme::lang`
  //! (was `cme::core` — renamed to stop shadowing the built-in `core` crate
  //! under glob imports) holds the AST; `cme::compiler` holds the pipeline.

  #[cfg(feature = "core")]
  pub use cme_core as lang;

  #[cfg(feature = "compiler")]
  pub use cme_compiler as compiler;

  #[cfg(feature = "interp")]
  pub use cme_interp as interp;

  #[cfg(feature = "runtime")]
  pub use cme_runtime as runtime;
  ```

  Grep for `cme::core` / `as core` usages (there are none in-repo) and fix if found.
  Note: the facade crate names stay (`cme::compiler::parse_source` works through the
  re-export).

- [ ] **F.5 — Crate docs + `#[non_exhaustive]` per D2 (review item 21).**
  - `cme-core/src/lib.rs`: `//!` block: what lives here (the shared AST, spans,
    error ids), the AST-ownership rule from AGENTS.md, and a small example building
    `int x = 1` by hand.
  - `cme-compiler/src/lib.rs`: `//!` block: pipeline overview, the
    error-recovery philosophy (never stop, plant `Invalid`, report everything), and
    the doctest from F.2.
  - `cme-interp`/`cme-runtime`: one-line `//!` placeholders already exist from A.3 —
    extend only if trivial.
  - Apply `#[non_exhaustive]` per the D2 default: `Token`, `LexError`, `Diagnostic`,
    `DiagnosticKind`, `ParseOutcome` (skip if D2 was overridden to include AST enums —
    then also add catch-alls to `cme-compiler`'s AST matches).
  - Verify doctests run: `cargo test --workspace --doc` (CI covers this via
    `cargo test`).

**Validation gate:** green including doctests.
**Commit:** `feat: public parse_source facade and ParseOutcome`.

---

## Phase G — CLI: dedicated `cme-cli` crate, `LineIndex` rendering

**Goal:** the CLI moves to its own workspace crate (`cme-cli`, binary `cme`); the root
package becomes a pure library. All cfg-gating in the binary disappears, the source
stops being cloned through the error type, and rendering goes through a `LineIndex`
that is correct for non-ASCII sources.

**Files:** new `crates/cme-cli/Cargo.toml`, new `crates/cme-cli/src/main.rs`, new
`crates/cme-cli/src/line_index.rs`, root `Cargo.toml`, root `src/main.rs` (deleted),
`.github/workflows/ci.yml`.

### Steps

- [ ] **G.1 — Create the crate (review item 12, option B).**
      `crates/cme-cli/Cargo.toml`:

  ```toml
  [package]
  name = "cme-cli"
  version.workspace = true
  edition.workspace = true
  authors.workspace = true
  license.workspace = true
  description = "Checkmate CLI toolchain"
  repository.workspace = true
  publish = false

  [[bin]]
  name = "cme"
  path = "src/main.rs"

  [dependencies]
  cme-compiler = { path = "../cme-compiler" }
  ```

  Root `Cargo.toml`:
  - delete `[features] cli = [...]` (keep `core`, `compiler`, `interp`, `runtime`);
  - the optional deps stay (the facade keeps its feature-gated structure);
  - delete the `[lints.rust] unexpected_cfgs` block **and verify with clippy**: it
    whitelisted a feature value `"all"` that never occurs anywhere — vestigial. If
    removing it produces new `unexpected_cfgs` warnings (it should not — all used
    features are declared), investigate and restore only what is needed.
  - add `"crates/cme-cli"` is already covered by `members = ["crates/*"]`.
    Add the crate to `[workspace]` — already globbed. Add `README` mention (Phase I).

- [ ] **G.2 — `LineIndex` (review item 13, option A).**
      `crates/cme-cli/src/line_index.rs`:

  ```rust
  //! Byte-offset ↔ (line, column) mapping, computed once per file.
  //!
  //! Columns are 1-based and counted in `char`s, not bytes, so rendering is
  //! correct for non-ASCII sources (string literals may contain any UTF-8).
  //! LSP will later need UTF-16 columns; convert from the char column there.

  pub(crate) struct LineIndex {
      /// Byte offset at which each line (1-based) starts; line 1 starts at 0.
      line_starts: Vec<usize>,
      source_len: usize,
  }

  impl LineIndex {
      pub(crate) fn new(source: &str) -> Self {
          let mut line_starts = vec![0usize];
          for (offset, byte) in source.bytes().enumerate() {
              if byte == b'\n' {
                  line_starts.push(offset + 1);
              }
          }
          Self { line_starts, source_len: source.len() }
      }

      /// 1-based line and 1-based char column for a byte offset.
      pub(crate) fn line_column(&self, source: &str, offset: usize) -> (usize, usize) {
          let offset = offset.min(self.source_len);
          let line = match self.line_starts.binary_search(&offset) {
              Ok(found) => found + 1,
              Err(insertion) => insertion, // offset is strictly inside a line
          };
          let line_start = self.line_starts[line - 1];
          let column = source[line_start..offset].chars().count() + 1;
          (line, column)
      }

      pub(crate) fn line_start(&self, line: usize) -> usize {
          self.line_starts.get(line - 1).copied().unwrap_or(self.source_len)
      }

      /// The text of `line` (1-based), without its line terminator.
      pub(crate) fn line_text<'s>(&self, source: &'s str, line: usize) -> &'s str {
          let start = self.line_start(line);
          let end = self.line_start(line + 1)
              .min(self.source_len);
          let mut text = &source[start..end];
          if text.ends_with('\n') {
              text = &text[..text.len() - 1];
          }
          if text.ends_with('\r') {
              text = &text[..text.len() - 1];
          }
          text
      }
  }
  ```

  Unit tests (in the same file): empty source; no trailing newline; CRLF source;
  multibyte line (`"héllo wörld ✓"`): `line_column` at a byte offset inside a
  multibyte char-boundary token counts chars, and a caret on the `✓` region lines up.

- [ ] **G.3 — The new `main.rs` (review item 12).**
      Move + rewrite; **no cfg gates anywhere**. Target shape:

  ```rust
  use cme_compiler::diagnostics::Diagnostic;
  use cme_compiler::{lexer, parse_source};
  use std::fmt;
  use std::path::PathBuf;
  use std::process::ExitCode;

  mod line_index;
  use line_index::LineIndex;

  const USAGE: &str = "Usage: cme <lex|ast> <file.cm>";

  #[derive(Debug)]
  enum CliError {
      Usage(String),
      Io { path: PathBuf, error: std::io::Error },
  }

  impl fmt::Display for CliError {
      fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
          match self {
              CliError::Usage(message) => write!(f, "{message}\n{USAGE}"),
              CliError::Io { path, error } => write!(f, "failed to read {}: {error}", path.display()),
          }
      }
  }

  impl std::error::Error for CliError {}

  fn main() -> ExitCode {
      let mut args = std::env::args_os().skip(1);
      let Some(command) = args.next() else {
          return fail(&CliError::Usage("expected a command and a file".into()));
      };
      let Some(path) = args.next() else {
          return fail(&CliError::Usage("expected a file path".into()));
      };
      if args.next().is_some() {
          return fail(&CliError::Usage("too many arguments".into()));
      }
      match run(&command, &PathBuf::from(path)) {
          Ok(()) => ExitCode::SUCCESS,
          Err(error) => fail(&error),
      }
  }

  fn fail(error: &CliError) -> ExitCode {
      eprintln!("error: {error}");
      ExitCode::FAILURE
  }

  fn run(command: &std::ffi::OsStr, path: &std::path::Path) -> Result<(), CliError> {
      let source = std::fs::read_to_string(path).map_err(|error| CliError::Io {
          path: path.to_path_buf(),
          error,
      })?;
      let index = LineIndex::new(&source);

      let diagnostics = match command.to_str() {
          Some("lex") => {
              let (tokens, errors) = lexer::lex_with_errors(&source);
              for token in &tokens {
                  println!("{token:?}");
              }
              errors.into_iter().map(Diagnostic::lex).collect::<Vec<_>>()
          }
          Some("ast") => {
              let outcome = parse_source(&source);
              println!("{:#?}", outcome.statements);
              outcome.diagnostics
          }
          Some(other) => {
              return Err(CliError::Usage(format!("unknown command: {other}")));
          }
          None => return Err(CliError::Usage("command must be valid UTF-8".into())),
      };

      for diagnostic in &diagnostics {
          eprintln!("error: {}", render(diagnostic, &source, &index, path));
      }
      if diagnostics.is_empty() {
          Ok(())
      } else {
          ExitCode::FAILURE; // communicated via std::process::exit? No — see note
      }
  }
  ```

  **Design note on exit codes:** `run` as sketched mixes `Result` and `ExitCode`.
  Cleanest resolution: `run` returns `Result<ExitCode, CliError>` where diagnostic
  presence maps to `ExitCode::FAILURE` (the CLI's documented contract: diagnostics on
  stderr + exit 1, tokens/AST on stdout). Implement it that way.

  ```rust
  fn render(
      diagnostic: &Diagnostic,
      source: &str,
      index: &LineIndex,
      path: &std::path::Path,
  ) -> String {
      let span = diagnostic.span();
      let (line, column) = index.line_column(source, span.start);
      let line_text = index.line_text(source, line);
      let prefix = source[index.line_start(line)..span.start.min(source.len())]
          .chars()
          .count();
      let width = source[span.start..span.end.min(source.len())]
          .chars()
          .count()
          .max(1);
      format!(
          "{path}:{line}:{column}: {message}\n{line_text}\n{caret}",
          path = path.display(),
          message = diagnostic.message(),
          caret = format!("{}{}", " ".repeat(prefix), "^".repeat(width)),
      )
  }
  ```

  Notes:
  - `CliError::Compiler(Vec<Diagnostic>, String)` (the source-cloning variant) is
    gone: `run` owns the source and renders before returning.
  - `render_diagnostics` (the old misnomer), `line_column`, `line_start_byte`, and
    the `String::from_utf8_lossy` hack are all deleted.
  - Arguments come from `args_os()` (non-UTF-8 paths work; the old `args.nth(2)`
    duplication in `main` is gone).
  - The stub `#[cfg(not(feature = "cli"))] fn main()` and every `#[cfg(feature =
"cli")]` attribute are gone — the whole reason for choosing 12-B.

- [ ] **G.4 — Delete the old binary; update CI.**
  - Delete root `src/main.rs` (the root package is now lib-only).
  - `.github/workflows/ci.yml`: delete the `cargo test --workspace --features cli`
    line (the feature no longer exists; the plain workspace test now builds
    `cme-cli`'s unit tests through `--all-targets`).
  - Smoke test locally (this replaces the deleted feature path):
    ```sh
    cargo run -p cme-cli -- lex test.cm        # exit 0, token dump
    cargo run -p cme-cli -- ast boom.cm        # exit 1, rendered diagnostics + carets
    cargo build                                # root builds as pure lib
    ```

**Validation gate:** green; smoke commands behave as stated.
**Commit:** `refactor: dedicated cme-cli crate with LineIndex rendering`.

---

## Phase H — Fixtures: `tests/fixtures/` + data-driven suite

**Goal:** all `.cm` fixtures move to `tests/fixtures/` at the workspace root. A root
integration test discovers **every** `.cm` file in that directory dynamically — no
fixture filename is hardcoded in test code — and validates each against a sidecar
`.expected` pin file. The boom.cm mega-test moves to the root package too, so the
whole fixture story lives in one self-contained place.

**Files:** new `tests/fixtures/` (moved `test.cm`, `test2.cm`, `test3.cm`,
`boom.cm` + sidecars), new root `tests/front_end.rs`, new root `tests/boom.rs`,
root `Cargo.toml` (dev-dependency), `crates/cme-compiler/src/lib.rs` (mega-test
removed), fixture headers.

### Steps

- [ ] **H.1 — Move the fixtures.**

  ```sh
  mkdir tests/fixtures
  jj mv test.cm test2.cm test3.cm boom.cm tests/fixtures/
  ```

  (Use `jj mv`; if unavailable, `mv` + `jj` will track it.)

- [ ] **H.2 — Root dev-dependency.**
      Root `Cargo.toml`:

  ```toml
  [dev-dependencies]
  cme-compiler = { path = "crates/cme-compiler" }
  ```

  (A dev-dependency on an optional normal dep is compiled for test targets regardless
  of feature flags — the fixture suite needs no feature gymnastics.)

- [ ] **H.3 — Rewrite `test3.cm` (stale since the overflow fix).**
      Its header claims the lexer _panics_ on out-of-range literals — behavior that no
      longer exists (fixed before this refactor; classified as `IntegerOverflow` since
      Phase B). Rewrite the header to describe current reality:
  - `int boom = 99999999999999999999` produces one lex diagnostic
    ("integer literal is too large") and one parse diagnostic (missing initializer);
    the declaration survives with a zero-width `Invalid`.
  - Mention `-9223372036854775808` still works via unary minus on
    `9223372036854775807`... actually it does _not_ (the whitepaper calls literal
    boundary behavior unspecified) — just describe what the pinned `.expected` says.
    Keep the file to one damaged line + one healthy line.

- [ ] **H.4 — The data-driven suite: `tests/front_end.rs`.**
      Sidecar format (`tests/fixtures/<name>.expected`, one `key: value` per line,
      `#` comments allowed, unknown keys are test failures):

  ```text
  statements: 334
  diagnostics: 0
  lex_diagnostics: 0
  ```

  The suite:

  ```rust
  //! Fixture-driven front-end suite.
  //!
  //! Discovers every `tests/fixtures/*.cm` file — no fixture is hardcoded here.
  //! Each must have a `<name>.expected` sidecar pinning statement/diagnostic
  //! counts. To (re)generate sidecars, run with FIXTURE_UPDATE=1 and review the
  //! diff — never commit regenerated pins unread.

  use cme_compiler::diagnostics::DiagnosticKind;
  use cme_compiler::parse_source;
  use std::collections::BTreeMap;
  use std::fs;
  use std::path::{Path, PathBuf};

  fn fixtures() -> Vec<PathBuf> {
      let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
      let mut files: Vec<PathBuf> = fs::read_dir(&dir)
          .expect("tests/fixtures must exist")
          .filter_map(Result::ok)
          .map(|entry| entry.path())
          .filter(|path| path.extension().is_some_and(|ext| ext == "cm"))
          .collect();
      files.sort();
      assert!(!files.is_empty(), "no .cm fixtures found in tests/fixtures");
      files
  }

  fn sidecar_path(cm: &Path) -> PathBuf {
      let mut name = cm.file_name().expect("fixture file name").to_owned();
      name.set_extension("expected");
      cm.with_file_name(name)
  }

  fn parse_sidecar(text: &str) -> BTreeMap<String, usize> {
      text.lines()
          .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
          .map(|line| line.split_once(':').expect("expected `key: value` line"))
          .map(|(key, value)| (key.trim().to_string(), value.trim().parse().expect("numeric value")))
          .collect()
  }

  fn write_sidecar(cm: &Path, statements: usize, diagnostics: usize, lex: usize) {
      let text = format!(
          "# Pins for {} — regenerate with FIXTURE_UPDATE=1 and review.\nstatements: {statements}\ndiagnostics: {diagnostics}\nlex_diagnostics: {lex}\n",
          cm.file_name().unwrap().to_string_lossy()
      );
      fs::write(sidecar_path(cm), text).expect("write sidecar");
  }

  #[test]
  fn every_fixture_is_pinned_and_parses() {
      let update = std::env::var("FIXTURE_UPDATE").is_ok();
      for cm in fixtures() {
          let source = fs::read_to_string(&cm).expect("fixture readable");
          let outcome = parse_source(&source);
          let lex = outcome
              .diagnostics
              .iter()
              .filter(|d| matches!(d.kind(), DiagnosticKind::Lex(_)))
              .count();

          if update {
              write_sidecar(&cm, outcome.statements.len(), outcome.diagnostics.len(), lex);
              continue;
          }

          let sidecar = sidecar_path(&cm);
          assert!(
              sidecar.exists(),
              "missing pin file {} — run with FIXTURE_UPDATE=1 and review the diff",
              sidecar.display()
          );
          let pins = parse_sidecar(&fs::read_to_string(&sidecar).unwrap());
          assert_eq!(
              pins.get("statements").copied().unwrap_or(usize::MAX),
              outcome.statements.len(),
              "statement count changed for {}",
              cm.display()
          );
          assert_eq!(
              pins.get("diagnostics").copied().unwrap_or(usize::MAX),
              outcome.diagnostics.len(),
              "diagnostic count changed for {}",
              cm.display()
          );
          assert_eq!(
              pins.get("lex_diagnostics").copied().unwrap_or(usize::MAX),
              lex,
              "lex diagnostic count changed for {}",
              cm.display()
          );
      }
      if update {
          panic!("FIXTURE_UPDATE=1: sidecars were regenerated — review the diff, then re-run without the variable");
      }
  }
  ```

  Generate the initial sidecars: `FIXTURE_UPDATE=1 cargo test -p cme --test front_end`,
  then **read every sidecar diff** and sanity-check the numbers against the fixture
  headers (test.cm: `diagnostics: 0`; test2.cm: header claims 20 statements /
  28 diagnostics — verify what current reality is, the header may be stale too; update
  the header to match). Commit sidecars + fixtures together.

- [ ] **H.5 — Move the boom mega-test: `tests/boom.rs`.**
  - Cut the whole `boom_cm_stress_fixture` test, the `BOOM_CM` const, and the
    `statement_label` / `invalid_span` helpers out of
    `crates/cme-compiler/src/lib.rs` (lib.rs keeps only unit tests there).
  - New root `tests/boom.rs`: same test, but
    `const BOOM_CM: &str = include_str!("fixtures/boom.cm");` (self-contained within
    the root package) and the pipeline call becomes
    `let outcome = parse_source(BOOM_CM);`.
  - While moving, replace the magic index lists (`[7usize, 8, 10, 38, 40, 41, 49]` and
    the skipped-region list) with **label-anchored assertions** where feasible, e.g.:
    ```rust
    let index_of = |needle: &str| {
        labels
            .iter()
            .position(|label| label.starts_with(needle))
            .unwrap_or_else(|| panic!("no statement labeled {needle:?}"))
    };
    for needle in ["var:count:Int", "var:k:Int", "var:draft:Infer", ...] {
        let span = invalid_span(&stmts[index_of(needle)])...;
        assert_eq!(span.start, span.end, ...);
    }
    ```
    (Survives fixture edits far better than raw indexes. Recompute the membership of
    both lists after the Phase C/E behavior changes; `mixedBad` left the
    Invalid-bearing list in E.3.)
  - The unit tests inside `cme-compiler/src/lib.rs` that used `parse_program_parts`
    already call `parse_source` since F.2 — verify nothing still references
    `../../boom.cm` (the old out-of-crate `include_str!` is gone with this move).

- [ ] **H.6 — Update every fixture header comment.**
  - `test.cm`: run commands → `cargo run -p cme-cli -- lex tests/fixtures/test.cm`;
    "must produce ZERO diagnostics" stays, now enforced by `front_end.rs`.
  - `boom.cm`: commands likewise; the reference "The Rust test
    `boom_cm_stress_fixture` (cme-compiler/src/lib.rs)" → "(tests/boom.rs)".
  - `test2.cm`: commands likewise; recount the header's claimed numbers against its
    new sidecar and correct the prose.
  - All four files also had message quotes updated during Phases D/E — final read-through.

**Validation gate:** green; `FIXTURE_UPDATE` flow demonstrated once (regenerate →
review → no diff → re-run clean); no stray fixture references anywhere
(`rg -n "boom.cm|test2?\.cm" --glob '!tests/**' --glob '!*.md'` returns only
intended hits).
**Commit:** `test: data-driven fixture suite in tests/fixtures`.

---

## Phase I — Docs closeout & final audit

**Goal:** README and AGENTS.md describe the repository as it now is; the refactor is
audited end-to-end; this plan's checklist is fully ticked.

**Files:** `README.md`, `AGENTS.md`, this file, everything (audit).

### Steps

- [ ] **I.1 — README.md rewrite (review item 17, docs half).**
  - "Current Status": a working front-end — classified lexer errors, error-recovering
    parser with spanned AST, one-call pipeline (`parse_source`), a real CLI
    (`lex`/`ast` with rendered diagnostics). Interpreter and runtime remain
    placeholders. **Remove every "CLI is a placeholder" sentence.**
  - Workspace table: add `cme-cli` row (CLI toolchain, binary `cme`); update
    `cme-compiler` row (lexer, parser, diagnostics, validation); fix the root `cme`
    row (pure facade, no CLI).
  - Development section:
    ```sh
    cargo build                       # facade library
    cargo run -p cme-cli -- ast test.cm   # CLI
    cargo test --workspace            # incl. fixture suite + doctests
    cargo fmt / cargo clippy --workspace --all-targets
    ```
  - Add a short "Embedding" example using `parse_source` + feature flags
    (`cme = { version = "...", features = ["compiler"] }`).
  - Add a "Fixtures" paragraph: `tests/fixtures/` — clean files must stay clean,
    broken files pin recovery outcomes, sidecar `.expected` files + `FIXTURE_UPDATE=1`.

- [ ] **I.2 — AGENTS.md update (review item 17, docs half).**
  - Crates list: add `cme-cli`; update `cme-compiler` description (diagnostics
    module, validator, `parse_source`); root `cme` is a pure facade (no `cli`
    feature anymore).
  - Commands section: replace `--features cli` invocations with `-p cme-cli`.
  - Working Rules: `plan.md` references are now valid (this file) — keep them, and
    note that this refactor's execution state lives in its checkboxes.
  - Repository State Notes: remove stale "add() starter" note (done in A); add the
    fixtures/sidecar workflow note; keep the newline-significance warning.
  - Whitepaper policy and `mdpeek`/`jj` rules: unchanged.

- [ ] **I.3 — Final audit.**
  - Full gate + doctests: `cargo fmt`, `cargo clippy --workspace --all-targets --
D warnings`, `cargo test --workspace`, `cargo test --workspace --doc`.
  - CLI smoke:
    ```sh
    cargo run -p cme-cli -- lex tests/fixtures/test.cm      # exit 0
    cargo run -p cme-cli -- ast tests/fixtures/test.cm      # exit 0
    cargo run -p cme-cli -- ast tests/fixtures/boom.cm      # exit 1, carets aligned
    cargo run -p cme-cli -- ast tests/fixtures/test3.cm     # exit 1, overflow message
    ```
    Visually confirm the caret sits under the offending token on a unicode line
    (boom.cm §4 has non-ASCII-free lines; use test.cm's `"héllo wörld ✓"` line by
    introducing a temporary error, or trust the LineIndex unit tests).
  - Packaging dry run: `cargo publish --dry-run --workspace` — expect success for
    `cme`/`cme-core`/`cme-compiler`, and that `cme-interp`/`cme-runtime`/`cme-cli`
    are excluded by `publish = false`.
  - Review-item sweep: walk the 21-item table in section 2 and confirm each phase's
    checkboxes are ticked. Anything unticked gets either finished or explicitly
    annotated as dropped with a reason.
  - Tick the final boxes of this plan and note the completion date at the top.

**Validation gate:** everything above.
**Commit:** `docs: update README and AGENTS for the new layout`.

---

## Appendix A — `Token::describe()` table

| Token         | describe()               |     | Token       | describe() |     | Token      | describe()     |
| ------------- | ------------------------ | --- | ----------- | ---------- | --- | ---------- | -------------- |
| `Newline`     | `end of statement`       |     | `AddAssign` | `` `+=` `` |     | `KwFloat`  | `` `float` ``  |
| `Eof`         | `end of file`            |     | `SubAssign` | `` `-=` `` |     | `KwInfer`  | `` `infer` ``  |
| `Ident(s)`    | ``identifier `s` ``      |     | `MulAssign` | `` `*=` `` |     | `KwReturn` | `` `return` `` |
| `StrLit(_)`   | `string literal`         |     | `DivAssign` | `` `/=` `` |     | `KwStr`    | `` `str` ``    |
| `IntLit(v)`   | ``integer literal `v` `` |     | `RemAssign` | `` `%=` `` |     | `KwBool`   | `` `bool` ``   |
| `FloatLit(v)` | ``float literal `v` ``   |     | `Plus`      | `` `+` ``  |     | `KwTrue`   | `` `true` ``   |
| `Or`          | `` `\|\|` ``             |     | `Minus`     | `` `-` ``  |     | `KwFalse`  | `` `false` ``  |
| `And`         | `` `&&` ``               |     | `Star`      | `` `*` ``  |     | `LParen`   | `` `(` ``      |
| `Eq`          | `` `==` ``               |     | `Slash`     | `` `/` ``  |     | `RParen`   | `` `)` ``      |
| `Ne`          | `` `!=` ``               |     | `Percent`   | `` `%` ``  |     |            |                |
| `Le`          | `` `<=` ``               |     | `Not`       | `` `!` ``  |     |            |                |
| `Ge`          | `` `>=` ``               |     | `Assign`    | `` `=` ``  |     |            |                |
| `Lt`          | `` `<` ``                |     |             |            |     |            |                |
| `Gt`          | `` `>` ``                |     |             |            |     |            |                |

(`LBrace`/`RBrace` are gone after Phase C.)

## Appendix B — Message rewrite inventory (Phase D)

Parser messages, old → new:

| #   | Old string (site)                                                | New string                                               |
| --- | ---------------------------------------------------------------- | -------------------------------------------------------- |
| 1   | `expected end of statement (newline), found {:?}` (program loop) | `expected end of statement, but found {desc}`            |
| 2   | `Expected a type or assignment target, but found {:?}`           | `expected a type or assignment target, but found {desc}` |
| 3   | `unexpected end of file`                                         | unchanged                                                |
| 4   | `Expected a variable name, but found {:?}`                       | `expected a variable name, but found {desc}`             |
| 5   | `Expected a variable name, but reached end of file`              | `expected a variable name, but found end of file`        |
| 6   | `Expected '=', but found {:?}`                                   | ``expected `=`, but found {desc}``                       |
| 7   | `Expected '=', but found end of statement`                       | ``expected `=`, but found end of statement``             |
| 8   | `Expected '=', but reached end of file`                          | ``expected `=`, but found end of file``                  |
| 9   | `Expected assignment operator, but found {:?}`                   | `expected an assignment operator, but found {desc}`      |
| 10  | `Expected assignment operator, but reached end of file`          | `expected an assignment operator, but found end of file` |
| 11  | `mixed && and \|\| require parentheses`                          | unchanged (moves to validator in E; span becomes real)   |
| 12  | `comparisons are non-associative; add parentheses`               | unchanged                                                |
| 13  | `Expected ')', but reached end of file` (require_token)          | ``expected `)`, but found end of file``                  |
| 14  | `Expected ')', but found {:?}`                                   | ``expected `)`, but found {desc}``                       |
| 15  | `Expected an expression, but reached end of file`                | `expected an expression, but found end of file`          |
| 16  | `Expected an expression, but found end of statement`             | `expected an expression, but found end of statement`     |
| 17  | `Expected an expression, but found {:?}`                         | `expected an expression, but found {desc}`               |
| 18  | `unbalanced closing parenthesis`                                 | unchanged                                                |
| 19  | `unbalanced opening parenthesis`                                 | unchanged                                                |
| 20  | lex fallback `invalid token` (3 hard-coded copies)               | replaced by `LexError` `Display` per D3 wording          |

Pinned-test strings to sweep after D: `"Expected a variable name"`,
`"Expected '='"`, `"Expected an expression"`, `"expected end of statement"`,
`"reached end of file"`, `"Expected assignment operator"`, `"Expected a type or
assignment target"` — plus the boom.cm comment lines quoting them.

## Appendix C — Final public API map (end state)

```text
cme-core::ast
  Span { start, end }            + Span::new, Span::missing
  ErrorId(usize)                 index into the diagnostics list
  Type                           Int | Float | Bool | Str            (Infer gone if D1)
  BinaryOp / UnaryOp / CompoundOp
  Expr { span, kind }            kind: ExprKind
  ExprKind                       IntLit | FloatLit | StrLit | BoolLit | Ident
                                | Binary | Unary | Paren | Invalid { error: ErrorId }
  Stmt { span, kind }            kind: StmtKind
  StmtKind                       VarDecl { ty: Option<Type>, name, expr }
                                | Assign | CompoundAssign | Invalid { error: ErrorId }
  Expr::contains_invalid / Stmt::contains_invalid

cme_compiler
  parse_source(&str) -> ParseOutcome                       [the pipeline in one call]
  diagnostics::{ Diagnostic, DiagnosticKind, ParseOutcome }
      Diagnostic::lex(LexError) / Diagnostic::parse(msg, span)
      Diagnostic::{kind, message, span}                    Display = message
      ParseOutcome { statements, diagnostics } + is_clean + error(ErrorId)
  lexer::{ Token (+Eof, describe), SpannedToken, LexError, lex, lex_with_errors }
  parser::{ Parser::new(&tokens), parse_statement, parse_program,
            parse_program_with_errors, strip_insignificant_newlines(_with_errors) }
  validate (internal)

cme (root facade, feature-gated)
  cme::lang      = cme_core        (renamed from cme::core)
  cme::compiler  = cme_compiler
  cme::interp    = cme_interp      (placeholder, publish = false)
  cme::runtime   = cme_runtime     (placeholder, publish = false)

cme-cli (workspace crate, binary `cme`, publish = false)
  commands: lex | ast <file.cm>    LineIndex-based diagnostics rendering
```
