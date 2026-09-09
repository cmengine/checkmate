//! The packrat pattern matcher (WHITEPAPER §8.3): matches a compiled
//! pattern against an invocation region and produces the capture tree.
//!
//! Task-3 scope (plan §3): the full element set — skipper disciplines
//! (flow / line / soft / raw), literals and case folding, classes, `scan`,
//! `until` (literal and pattern stops), `lineRest`, `eol`/`line`/`eof`,
//! `indent` (+ `indent verbatim`), `soft`, `optional`, `each`
//! (+`sep`/`trailing`/bounds/`each+`), `oneof` with complete fall-through,
//! `peek`/`not`, groups, rule references with delegation and `with context`,
//! `recur`, `where`, `label`, and the `$ident $word $tag $int $float $str
//! $text $type $expr $raw $block` fragments. Tail-bounded fragments use
//! exact-anchored tail-matching extents; the parse-integrated boundary check
//! (§8.3.6 "and the captured text parses") lands in Task 6.
//!
//! Task-4 hardening (plan §3): packrat memoization of rule results keyed by
//! (rule, position, environment, continuation identity), a re-entrant cycle
//! cut (§8.7: "a re-entrant rule invocation against an in-progress memo
//! entry fails immediately"), and a deterministic operation-count fuel
//! budget whose exhaustion is a distinct budget error, never a hang
//! (§5.5, §8.7).

use std::collections::{HashMap, HashSet};

use cme_core::Span;
use cme_core::magic::{
    Accessor, Capture, CaptureKind, CtxBinOp, CtxExpr, FragKind, LexProfile, PatElem, PatKind,
    Pattern, TextKind,
};

/// One compiled grammar: profile plus rules.
#[derive(Debug, Clone, Default)]
pub struct CompiledGrammar {
    pub name: String,
    pub profile: LexProfile,
    pub rules: Vec<CompiledRule>,
}

/// One compiled rule: name, declared context fields, parsed pattern.
#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub name: String,
    pub context: Vec<cme_core::magic::ContextField>,
    pub pattern: Pattern,
}

/// A set of grammars, resolvable by name (delegation) — the whole file's
/// megaprogram vocabulary.
#[derive(Debug, Clone, Default)]
pub struct GrammarSet {
    pub grammars: Vec<CompiledGrammar>,
}

impl GrammarSet {
    pub fn grammar(&self, name: &str) -> Option<&CompiledGrammar> {
        self.grammars.iter().find(|grammar| grammar.name == name)
    }
}

/// The input to one pattern match: the region as characters plus the byte
/// offsets that map match positions back to original-file spans.
pub struct MatchRegion<'a> {
    pub source: &'a str,
    /// Byte offset of the region's first character in `source`.
    pub base: usize,
    pub chars: Vec<char>,
    /// Byte offset of each char within the region.
    pub byte_offsets: Vec<usize>,
    /// The region's total byte length.
    pub byte_len: usize,
}

impl<'a> MatchRegion<'a> {
    /// Builds the match input from the region slice of `source`.
    pub fn new(source: &'a str, base: usize, region: &'a str) -> Self {
        let mut chars = Vec::new();
        let mut byte_offsets = Vec::new();
        for (index, c) in region.char_indices() {
            chars.push(c);
            byte_offsets.push(index);
        }
        Self {
            source,
            base,
            byte_len: region.len(),
            byte_offsets,
            chars,
        }
    }
}

/// Skipper modes (§8.3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SkipMode {
    /// Skip per the current grammar's profile (flow grammars also skip
    /// comment forms).
    On,
    /// Skip per the profile plus line terminators (inside `soft`).
    Soft,
    /// The skipper is suspended (`raw { … }`, exact-anchored first elements).
    Off,
}

#[derive(Clone)]
struct Env {
    skip: SkipMode,
    grammar: usize,
    /// Base columns of currently open `indent` blocks (§8.3.5).
    open_blocks: Vec<usize>,
    /// Captures bound so far in the current rule instance, plus context.
    scope: HashMap<String, Capture>,
    /// The rule currently being matched, for `recur` (grammar, rule).
    rule: Option<(usize, usize)>,
}

/// The result of matching one sub-pattern: where it ended, the ordered binds
/// it produced, and the last unbound rule-ref/fragment capture (the list
/// element candidate for repetitions).
struct Out {
    end: usize,
    binds: Vec<(String, Capture)>,
    primary: Option<Capture>,
}

impl Out {
    fn empty(end: usize) -> Self {
        Self {
            end,
            binds: Vec::new(),
            primary: None,
        }
    }
}

/// What follows a tail-bounded fragment outward through enclosing groups and
/// repetition bodies (§8.3.6's "effective tail").
enum Continuation<'p> {
    End,
    /// Siblings after this group/repetition in the enclosing sequence, then
    /// the enclosing continuation.
    Elems(&'p [PatElem], &'p Continuation<'p>),
    /// Inside an `each` body: sep-continued iterations, an optional trailing
    /// separator, then the enclosing continuation.
    Repeat {
        sep: Option<&'p Pattern>,
        trailing: bool,
        body: &'p Pattern,
        after: &'p Continuation<'p>,
    },
}

/// A line/column map over the region (§8.3.5 column arithmetic: tab = 8).
struct LineMap {
    col_of: Vec<usize>,
    /// Char index of the terminator ending each line (or chars.len()).
    line_ends: Vec<usize>,
}

impl LineMap {
    fn build(chars: &[char]) -> Self {
        let mut col_of = Vec::with_capacity(chars.len());
        let mut line_ends = Vec::new();
        let mut col = 0usize;
        let mut index = 0usize;
        while index < chars.len() {
            match chars[index] {
                '\r' => {
                    col_of.push(col);
                    index += 1;
                    line_ends.push(index);
                    if chars.get(index) == Some(&'\n') {
                        col_of.push(col);
                        index += 1;
                    }
                    col = 0;
                    continue;
                }
                '\n' => {
                    col_of.push(col);
                    index += 1;
                    line_ends.push(index);
                    col = 0;
                    continue;
                }
                '\t' => col = (col / 8 + 1) * 8,
                _ => col += 1,
            }
            col_of.push(col);
            index += 1;
        }
        line_ends.push(chars.len());
        Self { col_of, line_ends }
    }

    fn col(&self, pos: usize) -> usize {
        self.col_of.get(pos).copied().unwrap_or(0)
    }

    /// The char index of the terminator ending the line containing `pos`
    /// (or the region length for the final line).
    fn line_end(&self, pos: usize) -> usize {
        for end in &self.line_ends {
            if *end >= pos {
                return *end;
            }
        }
        self.col_of.len()
    }

    /// The char index just past the terminator ending the line at/after pos.
    fn next_line_start(&self, pos: usize) -> usize {
        // Strictly greater: when `pos` is itself a terminator, its line's
        // end already points at the next line's start (§8.3.5).
        for end in &self.line_ends {
            if *end > pos {
                return *end;
            }
        }
        self.col_of.len()
    }
}

/// A pattern match failure: furthest position (char index and region-
/// relative byte offset) plus the diagnostic message. Task-4 failures carry
/// composed context: `label` blocks (§8.3.9), the innermost rule name, and a
/// distinct fuel-budget message (§5.5) when the operation count ran out.
pub struct MatchFailure {
    pub position: usize,
    pub byte_offset: usize,
    pub message: String,
}

/// The default compile-time fuel budget: a deterministic OPERATION count
/// (§5.5 — never wall-clock time), so expansion stays byte-reproducible
/// across platforms while pathological grammars terminate with a budget
/// error instead of hanging.
const FUEL_BUDGET: u64 = 1_000_000;

/// Matches `pattern` (a magic's entry pattern) against the whole region and
/// returns the root capture. The pattern must consume the entire region
/// (§8.3.9): only skippable trailing characters may remain.
pub fn match_entry(
    set: &GrammarSet,
    grammar_index: usize,
    pattern: &Pattern,
    region: &MatchRegion<'_>,
) -> Result<Capture, MatchFailure> {
    match_entry_with_fuel(set, grammar_index, pattern, region, FUEL_BUDGET)
}

/// [`match_entry`] with an explicit fuel budget (operation count). Used by
/// tests to exercise the budget error deterministically.
pub fn match_entry_with_fuel(
    set: &GrammarSet,
    grammar_index: usize,
    pattern: &Pattern,
    region: &MatchRegion<'_>,
    fuel: u64,
) -> Result<Capture, MatchFailure> {
    let mut matcher = Matcher::new(set, region, fuel);
    let mut env = Env {
        skip: SkipMode::On,
        grammar: grammar_index,
        open_blocks: Vec::new(),
        scope: HashMap::new(),
        rule: None,
    };
    let mut out = matcher
        .sequence(&pattern.elems, 0, &mut env, &Continuation::End)
        .ok_or_else(|| matcher.failure())?;
    let tail = matcher.skip(out.end, &env);
    if tail != region.chars.len() {
        matcher.note_failure(tail, "leftover content after the pattern match");
        return Err(matcher.failure());
    }
    let root = if out.binds.len() == 1 {
        out.binds.remove(0).1
    } else if let Some(primary) = out.primary {
        primary
    } else {
        Capture {
            kind: CaptureKind::Record {
                tag: "root".to_string(),
                fields: out.binds,
            },
            matched: String::new(),
            span: Span::missing(region.base),
        }
    };
    Ok(root)
}

struct Matcher<'a> {
    set: &'a GrammarSet,
    region: &'a MatchRegion<'a>,
    lines: LineMap,
    fuel: u64,
    /// Set when an operation was attempted with the budget at zero (§5.5).
    fuel_exhausted: bool,
    depth: usize,
    labels: Vec<String>,
    /// The rule currently being matched, innermost last (diagnostics §8.3.9).
    rule_stack: Vec<String>,
    /// Ordinary furthest-failure records.
    failures: Vec<FailureRecord>,
    /// Committed-block failures (§8.3.5): reported in preference to ordinary
    /// furthest failures because "this line belonged to this block" is the
    /// better diagnosis.
    committed: Vec<FailureRecord>,
    /// Packrat memo (§8.7): rule results keyed by rule + position + match
    /// environment + continuation identity.
    memo: HashMap<MemoKey, Option<(Capture, usize)>>,
    /// Rule invocations currently being matched; a re-entrant call against an
    /// in-progress entry fails immediately (§8.7's cycle-cut backstop).
    in_progress: HashSet<MemoKey>,
}

/// One recorded failure: position, message, and the diagnostic context that
/// was active when it happened (label blocks and the innermost rule).
#[derive(Clone)]
struct FailureRecord {
    position: usize,
    message: String,
    labels: Vec<String>,
    rule: Option<String>,
}

/// The packrat memo key (plan §2.3): the rule, the position, and the match
/// environment — skip mode, open indent-block base columns, and the identity
/// of the rule's evaluated context bindings — plus the continuation identity
/// (tail-bounded fragments inside a rule extend to the caller's boundary, so
/// the same rule at the same position under a different continuation is a
/// different match).
#[derive(Clone, PartialEq, Eq, Hash)]
struct MemoKey {
    grammar: usize,
    rule: usize,
    pos: usize,
    skip: SkipMode,
    blocks: Vec<usize>,
    context: String,
    tail: u64,
}

/// A stable identity for a continuation within one match: pointer-based,
/// because the referenced patterns live in the `GrammarSet` / compiled macro
/// for the whole match. Combined into one FNV-style hash.
fn continuation_id(cont: &Continuation<'_>) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    let mut mix = |value: u64| {
        hash ^= value;
        hash = hash.wrapping_mul(FNV_PRIME);
    };
    match cont {
        Continuation::End => mix(0),
        Continuation::Elems(elems, after) => {
            // Empty links carry no matching elements (the same normalization
            // `has_effective_tail` applies): folding them away gives equal
            // continuations equal identities regardless of which allocation
            // the empty sibling slice points into.
            if elems.is_empty() {
                return continuation_id(after);
            }
            mix(1);
            mix(elems.as_ptr() as usize as u64);
            mix(elems.len() as u64);
            mix(continuation_id(after));
        }
        Continuation::Repeat {
            sep,
            trailing,
            body,
            after,
        } => {
            mix(2);
            let sep_ptr = sep
                .map(|pattern: &Pattern| std::ptr::from_ref(pattern) as usize)
                .unwrap_or(0);
            mix(sep_ptr as u64);
            mix(*trailing as u64);
            mix(std::ptr::from_ref::<Pattern>(*body) as usize as u64);
            mix(continuation_id(after));
        }
    }
    hash
}

/// A canonical signature of the context bindings a rule instance was invoked
/// with (§8.3.7). Matching is stateless across rule instances, so context
/// identity is the only scope a rule's result may depend on.
fn context_signature(pairs: &[(String, Capture)]) -> String {
    let mut signature = String::new();
    for (name, capture) in pairs {
        signature.push_str(name);
        signature.push('=');
        capture_signature(capture, &mut signature);
        signature.push(';');
    }
    signature
}

fn capture_signature(capture: &Capture, out: &mut String) {
    out.push_str(capture.matched());
    out.push('#');
    match &capture.kind {
        CaptureKind::Text(kind) => {
            out.push('t');
            out.push_str(match kind {
                TextKind::Ident => "i",
                TextKind::Word => "w",
                TextKind::Tag => "g",
                TextKind::Str => "s",
                TextKind::Raw => "r",
            });
        }
        CaptureKind::Int(value) => {
            out.push('n');
            out.push_str(&value.to_string());
        }
        CaptureKind::Float(value) => {
            out.push('f');
            out.push_str(&value.to_string());
        }
        CaptureKind::List(items) => {
            out.push('[');
            for item in items {
                capture_signature(item, out);
                out.push(',');
            }
            out.push(']');
        }
        CaptureKind::Record { tag, fields } => {
            out.push('{');
            out.push_str(tag);
            out.push(':');
            for (name, field) in fields {
                out.push_str(name);
                out.push('=');
                capture_signature(field, out);
                out.push(';');
            }
            out.push('}');
        }
        CaptureKind::Opt(inner) => match inner {
            Some(value) => {
                out.push_str("some(");
                capture_signature(value, out);
                out.push(')');
            }
            None => out.push_str("none"),
        },
    }
}

impl<'a> Matcher<'a> {
    fn new(set: &'a GrammarSet, region: &'a MatchRegion<'a>, fuel: u64) -> Self {
        Self {
            set,
            region,
            lines: LineMap::build(&region.chars),
            fuel,
            fuel_exhausted: false,
            depth: 0,
            labels: Vec::new(),
            rule_stack: Vec::new(),
            failures: Vec::new(),
            committed: Vec::new(),
            memo: HashMap::new(),
            in_progress: HashSet::new(),
        }
    }

    /// Composes the final failure report (§8.3.9): the furthest position,
    /// with committed-block failures preferred over ordinary ones, and the
    /// label/rule context rendered after the message.
    fn failure(&self) -> MatchFailure {
        if self.fuel_exhausted {
            return MatchFailure {
                position: 0,
                byte_offset: 0,
                message: "compile-time fuel budget exhausted (operation count limit, §5.5)"
                    .to_string(),
            };
        }
        let pick = |records: &[FailureRecord]| {
            records.iter().max_by_key(|record| record.position).cloned()
        };
        // Committed-block failures outrank ordinary furthest failures (§8.3.9).
        let record = pick(&self.committed).or_else(|| pick(&self.failures));
        let Some(record) = record else {
            return MatchFailure {
                position: 0,
                byte_offset: 0,
                message: "pattern did not match".to_string(),
            };
        };
        let mut message = record.message;
        if let Some(rule) = &record.rule {
            message.push_str(&format!(" (in rule `{rule}`"));
            if !record.labels.is_empty() {
                message.push_str(&format!(
                    ", while matching '{}' )",
                    record.labels.join("' → '")
                ));
            } else {
                message.push(')');
            }
        } else if !record.labels.is_empty() {
            message.push_str(&format!(
                " (while matching '{}' )",
                record.labels.join("' → '")
            ));
        }
        MatchFailure {
            position: record.position,
            byte_offset: self.absolute(record.position) - self.region.base,
            message,
        }
    }

    fn note_failure(&mut self, position: usize, message: impl Into<String>) {
        self.failures.push(FailureRecord {
            position,
            message: message.into(),
            labels: self.labels.clone(),
            rule: self.rule_stack.last().cloned(),
        });
    }

    fn spend(&mut self) -> bool {
        if self.fuel == 0 {
            self.fuel_exhausted = true;
            return false;
        }
        self.fuel -= 1;
        true
    }

    fn profile(&self, env: &Env) -> &LexProfile {
        &self.set.grammars[env.grammar].profile
    }

    // -- skipping -----------------------------------------------------------

    /// Advances past skip-set characters (and, in flow grammars, comment
    /// forms). Never crosses line boundaries unless the profile is
    /// flow-oriented or the mode is `Soft`.
    fn skip(&self, mut pos: usize, env: &Env) -> usize {
        if env.skip == SkipMode::Off {
            return pos;
        }
        let profile = self.profile(env);
        let crosses_lines = profile.is_flow_oriented() || env.skip == SkipMode::Soft;
        loop {
            match self.region.chars.get(pos) {
                Some(c) if profile.skip.matches(*c) => pos += 1,
                Some('\n') | Some('\r') if crosses_lines => pos += 1,
                Some(_) if profile.is_flow_oriented() => {
                    match comment_len_at(self.region, pos, profile) {
                        Some(len) => pos += len,
                        None => break,
                    }
                }
                _ => break,
            }
        }
        pos
    }

    // -- sequences ----------------------------------------------------------
    //
    // `sequence` does NOT skip before the first element: per §8.3.1 rule 1
    // the skipper runs immediately before each ELEMENT, so every element
    // implementation skips at its own start. This also makes `until` stops
    // and tail boundaries exact-anchored (they match at the cursor).

    fn sequence(
        &mut self,
        elems: &[PatElem],
        pos: usize,
        env: &mut Env,
        cont: &Continuation<'_>,
    ) -> Option<Out> {
        if !self.spend() {
            return None;
        }
        let mut cursor = pos;
        let mut binds: Vec<(String, Capture)> = Vec::new();
        let mut primary = None;
        for (index, elem) in elems.iter().enumerate() {
            let siblings = &elems[index + 1..];
            let out = self.element(elem, cursor, env, siblings, cont)?;
            cursor = out.end;
            for (name, capture) in out.binds {
                env.scope.insert(name.clone(), capture.clone());
                binds.push((name, capture));
            }
            if out.primary.is_some() {
                primary = out.primary;
            }
        }
        Some(Out {
            end: cursor,
            binds,
            primary,
        })
    }

    /// Like [`Matcher::sequence`], but the FIRST element matches exactly at
    /// `pos` (skipper suspended for it). Used by `until` stops and tail
    /// boundaries, which must anchor at the candidate position (§8.3.6).
    fn sequence_exact_first(
        &mut self,
        elems: &[PatElem],
        pos: usize,
        env: &mut Env,
        cont: &Continuation<'_>,
    ) -> Option<Out> {
        let Some((first, rest)) = elems.split_first() else {
            return Some(Out::empty(pos));
        };
        let saved_skip = env.skip;
        env.skip = SkipMode::Off;
        let out = self.element(first, pos, env, rest, cont);
        env.skip = saved_skip;
        let mut out = out?;
        let mut binds = std::mem::take(&mut out.binds);
        let mut primary = out.primary;
        let mut cursor = out.end;
        for (index, elem) in rest.iter().enumerate() {
            let siblings = &rest[index + 1..];
            let next = self.element(elem, cursor, env, siblings, cont)?;
            cursor = next.end;
            for (name, capture) in next.binds {
                env.scope.insert(name.clone(), capture.clone());
                binds.push((name, capture));
            }
            if next.primary.is_some() {
                primary = next.primary;
            }
        }
        Some(Out {
            end: cursor,
            binds,
            primary,
        })
    }

    /// Matches one element. Binds returned are inserted into the scope by
    /// the sequence driver.
    fn element(
        &mut self,
        elem: &PatElem,
        pos: usize,
        env: &mut Env,
        siblings: &[PatElem],
        cont: &Continuation<'_>,
    ) -> Option<Out> {
        if !self.spend() {
            return None;
        }
        // What follows this construct outward (for tail-bounded fragments
        // nested inside groups, soft regions, optionals, and each bodies).
        let outer = Continuation::Elems(siblings, cont);
        match &elem.kind {
            PatKind::Lit { text, insensitive } => {
                let start = self.skip(pos, env);
                let end = self.lit_at(start, text, *insensitive)?;
                Some(Out::empty(end))
            }
            PatKind::Class { set, bind } => {
                let start = self.skip(pos, env);
                let c = *self.region.chars.get(start)?;
                if !set.matches(c) {
                    self.note_failure(
                        start,
                        format!("expected a character from the class, found `{c}`"),
                    );
                    return None;
                }
                let capture = bind
                    .clone()
                    .map(|name| (name, self.text_capture(start, start + 1, TextKind::Raw)));
                Some(self.out_with(capture, start + 1, None))
            }
            PatKind::Any { bind } => {
                self.region.chars.get(pos)?;
                let capture = bind
                    .clone()
                    .map(|name| (name, self.text_capture(pos, pos + 1, TextKind::Raw)));
                Some(self.out_with(capture, pos + 1, None))
            }
            PatKind::Scan { set, bind } => {
                let start = self.skip(pos, env);
                let mut end = start;
                while let Some(c) = self.region.chars.get(end) {
                    if set.matches(*c) {
                        end += 1;
                    } else {
                        break;
                    }
                }
                if end == start {
                    self.note_failure(start, "expected a run of at least one character");
                    return None;
                }
                let capture = bind
                    .clone()
                    .map(|name| (name, self.text_capture(start, end, TextKind::Raw)));
                Some(self.out_with(capture, end, None))
            }
            PatKind::Until { stop, bind } => {
                // Atomic: the skipper never runs before `until` (§8.3.1), and
                // the stop is anchored exactly (§8.3.6 whole-match rule).
                let end = self.find_stop(pos, stop, env)?;
                let capture = bind
                    .clone()
                    .map(|name| (name, self.text_capture(pos, end, TextKind::Raw)));
                Some(self.out_with(capture, end, None))
            }
            PatKind::LineRest { bind } => {
                let end = self.lines.line_end(pos);
                let capture = bind
                    .clone()
                    .map(|name| (name, self.text_capture(pos, end, TextKind::Raw)));
                Some(self.out_with(capture, end, None))
            }
            PatKind::Eol => {
                let end = self.eol(pos, env)?;
                Some(Out::empty(end))
            }
            PatKind::Line => {
                if self.line_has_content(pos, env) {
                    Some(Out::empty(pos))
                } else {
                    self.note_failure(pos, "expected content before the line terminator");
                    None
                }
            }
            PatKind::Eof => {
                let end = self.transparent_end(pos, env)?;
                if end == self.region.chars.len() {
                    Some(Out::empty(end))
                } else {
                    self.note_failure(end, "expected end of region");
                    None
                }
            }
            PatKind::Soft(body) => {
                let mut soft_env = env.clone();
                soft_env.skip = SkipMode::Soft;
                let out = self.sequence(&body.elems, pos, &mut soft_env, &outer)?;
                env.scope = soft_env.scope;
                Some(Out {
                    end: out.end,
                    binds: out.binds,
                    primary: out.primary,
                })
            }
            PatKind::Optional { body, bind } => {
                let saved_scope = env.scope.clone();
                match self.sequence(&body.elems, pos, env, &outer) {
                    Some(out) => {
                        let mut result = Out {
                            end: out.end,
                            binds: Vec::new(),
                            primary: out.primary.clone(),
                        };
                        // An unbound optional passes its inner binds through
                        // (so `optional { ":" $type ptype }` surfaces ptype
                        // only when present); a bound one wraps the body's
                        // value in an Opt capture.
                        if let Some(name) = bind {
                            let inner = body_value(&out);
                            result.binds.push((
                                name.clone(),
                                Capture {
                                    kind: CaptureKind::Opt(Some(Box::new(inner))),
                                    matched: self.text(pos, out.end),
                                    span: self.span(pos, out.end),
                                },
                            ));
                        } else {
                            result.binds = out.binds;
                        }
                        Some(result)
                    }
                    None => {
                        // Atomic failure: undo every bind the attempt made.
                        env.scope = saved_scope;
                        Some(Out::empty(pos))
                    }
                }
            }
            PatKind::Each {
                plus,
                sep,
                trailing,
                bounds,
                body,
                bind,
            } => self.each(
                *plus,
                sep.as_deref(),
                *trailing,
                *bounds,
                body,
                bind.clone(),
                pos,
                env,
                &outer,
            ),
            PatKind::OneOf { branches, bind } => {
                for (label, branch) in branches {
                    let saved_scope = env.scope.clone();
                    if let Some(mut out) = self.branch(label, branch, pos, env, &outer) {
                        // `oneof { … } as name` binds the chosen branch's
                        // record (plan §2.2).
                        if let Some(name) = bind
                            && let Some(record) = out.primary.clone()
                        {
                            out.binds.push((name.clone(), record));
                        }
                        return Some(out);
                    }
                    env.scope = saved_scope;
                }
                self.note_failure(pos, "no oneof branch matched");
                None
            }
            PatKind::Peek { negated, body } => {
                let saved_scope = env.scope.clone();
                let matched = self
                    .sequence(&body.elems, pos, env, &Continuation::End)
                    .is_some();
                env.scope = saved_scope;
                if matched != *negated {
                    Some(Out::empty(pos))
                } else {
                    self.note_failure(
                        pos,
                        if *negated {
                            "unexpected match in a negative lookahead"
                        } else {
                            "lookahead did not match"
                        },
                    );
                    None
                }
            }
            PatKind::Group { body, bind } => {
                let out = self.sequence(&body.elems, pos, env, &outer)?;
                let mut result = Out {
                    end: out.end,
                    binds: out.binds,
                    primary: out.primary.clone(),
                };
                if let Some(name) = bind {
                    result
                        .binds
                        .push((name.clone(), self.text_capture(pos, out.end, TextKind::Raw)));
                }
                Some(result)
            }
            PatKind::RuleRef { path, ctx, bind } => {
                // The caller's continuation flows into the rule so a
                // tail-bounded fragment at the rule's edge extends to the
                // caller's boundary (§8.3.6); recursion is cut by depth.
                let (capture, end) = self.rule_ref(path, ctx, pos, env, &outer)?;
                let mut out = Out::empty(end);
                match bind {
                    Some(name) => out.binds.push((name.clone(), capture)),
                    None => out.primary = Some(capture),
                }
                Some(out)
            }
            PatKind::Recur => {
                // `recur` re-enters the enclosing rule (§8.3.3) with a fresh
                // scope; open indent blocks carry over so nested lines stay
                // inside their block.
                let Some((grammar_index, rule_index)) = env.rule else {
                    self.note_failure(pos, "`recur` used outside a rule body");
                    return None;
                };
                let rule = self.set.grammars[grammar_index].rules[rule_index].clone();
                let mut inner = Env {
                    skip: SkipMode::On,
                    grammar: grammar_index,
                    open_blocks: env.open_blocks.clone(),
                    scope: HashMap::new(),
                    rule: Some((grammar_index, rule_index)),
                };
                for field in &rule.context {
                    match &field.default {
                        Some(_) => {
                            inner.scope.insert(
                                field.name.clone(),
                                Capture {
                                    kind: CaptureKind::Opt(None),
                                    matched: String::new(),
                                    span: Span::missing(self.absolute(pos)),
                                },
                            );
                        }
                        None => continue,
                    }
                }
                self.depth += 1;
                self.rule_stack.push(rule.name.clone());
                let out = self.sequence(&rule.pattern.elems, pos, &mut inner, &outer);
                self.rule_stack.pop();
                self.depth -= 1;
                let out = out?;
                // Same record rule as `rule_ref`: a oneof-topped rule yields
                // the chosen branch's record (tag = branch label, plan §2.2).
                let capture = match rule.pattern.elems.first().map(|element| &element.kind) {
                    Some(PatKind::OneOf { .. }) => out.primary.clone().unwrap_or_else(|| Capture {
                        kind: CaptureKind::Record {
                            tag: rule.name.clone(),
                            fields: out.binds.clone(),
                        },
                        matched: self.text(pos, out.end),
                        span: self.span(pos, out.end),
                    }),
                    _ => Capture {
                        kind: CaptureKind::Record {
                            tag: rule.name.clone(),
                            fields: out.binds.clone(),
                        },
                        matched: self.text(pos, out.end),
                        span: self.span(pos, out.end),
                    },
                };
                Some(self.out_with(None, out.end, Some(capture)))
            }
            PatKind::Indent { body, verbatim } => {
                self.indent(body.as_deref(), verbatim.clone(), pos, env, &outer)
            }
            PatKind::Raw(body) => {
                let mut raw_env = env.clone();
                raw_env.skip = SkipMode::Off;
                let out = self.sequence(&body.elems, pos, &mut raw_env, &outer)?;
                env.scope = raw_env.scope;
                Some(Out {
                    end: out.end,
                    binds: out.binds,
                    primary: out.primary,
                })
            }
            PatKind::Where { cond } => {
                if self.eval(cond, env) == Some(CtxVal::Bool(true)) {
                    Some(Out::empty(pos))
                } else {
                    self.note_failure(pos, format!("constraint failed: {}", ctx_summary(cond)));
                    None
                }
            }
            PatKind::Label { message, body } => {
                self.labels.push(message.clone());
                let out = self.sequence(&body.elems, pos, env, &outer);
                self.labels.pop();
                out
            }
            PatKind::Fragment {
                kind,
                insensitive,
                validator,
                bind,
            } => self.fragment(
                kind.clone(),
                *insensitive,
                validator.as_deref(),
                bind.clone(),
                siblings,
                cont,
                pos,
                env,
            ),
        }
    }

    fn out_with(
        &self,
        bind: Option<(String, Capture)>,
        end: usize,
        primary: Option<Capture>,
    ) -> Out {
        let mut out = Out::empty(end);
        if let Some((name, capture)) = bind {
            out.binds.push((name, capture));
        }
        out.primary = primary;
        out
    }

    fn lit_at(&mut self, pos: usize, text: &str, insensitive: bool) -> Option<usize> {
        let mut cursor = pos;
        for expected in text.chars() {
            let c = *self.region.chars.get(cursor)?;
            let hit = if insensitive {
                c.eq_ignore_ascii_case(&expected)
            } else {
                c == expected
            };
            if !hit {
                self.note_failure(pos, format!("expected `{text}`"));
                return None;
            }
            cursor += 1;
        }
        Some(cursor)
    }

    /// The first position at or after `start` where `stop` matches as a
    /// whole, anchored exactly (§8.3.6). A stop matching at `start` yields
    /// an empty capture (Task-1 documented behavior).
    fn find_stop(&mut self, start: usize, stop: &Pattern, env: &mut Env) -> Option<usize> {
        let mut cursor = start;
        loop {
            if cursor > self.region.chars.len() {
                self.note_failure(start, "the stop condition never matched");
                return None;
            }
            let saved_scope = env.scope.clone();
            let hit = self
                .sequence_exact_first(&stop.elems, cursor, env, &Continuation::End)
                .is_some();
            env.scope = saved_scope;
            if hit {
                return Some(cursor);
            }
            if cursor == self.region.chars.len() {
                self.note_failure(start, "the stop condition never matched");
                return None;
            }
            cursor += 1;
        }
    }

    /// `eol`: transparent tail, then the line terminator, then transparent
    /// lines (§8.3.2 with §8.2's transparent-line discipline, so trailing
    /// comments are `eol`'s job exactly as §8.8's TOML grammar requires).
    /// Matches (consuming nothing) at region end.
    fn eol(&mut self, pos: usize, env: &Env) -> Option<usize> {
        let mut cursor = self.skip(pos, env);
        loop {
            if cursor >= self.region.chars.len() {
                return Some(cursor);
            }
            if let Some(len) = comment_len_at(self.region, cursor, self.profile(env)) {
                cursor += len;
                continue;
            }
            match self.region.chars.get(cursor) {
                Some('\r') => {
                    cursor += 1;
                    if self.region.chars.get(cursor) == Some(&'\n') {
                        cursor += 1;
                    }
                }
                Some('\n') => cursor += 1,
                _ => {
                    self.note_failure(cursor, "expected end of line");
                    return None;
                }
            }
            // Consume any further transparent lines.
            loop {
                let after = self.skip(cursor, env);
                if after >= self.region.chars.len() {
                    return Some(after);
                }
                if let Some(len) = comment_len_at(self.region, after, self.profile(env)) {
                    let after_comment = after + len;
                    if after_comment >= self.region.chars.len() {
                        return Some(after_comment);
                    }
                    cursor = self.lines.next_line_start(after_comment);
                    continue;
                }
                match self.region.chars.get(after) {
                    Some('\r') | Some('\n') => {
                        cursor = self.lines.next_line_start(after);
                        continue;
                    }
                    _ => return Some(cursor),
                }
            }
        }
    }

    /// Zero-width: does any non-skip-set character remain before the line
    /// terminator? Comment-only remainders count as empty.
    /// True when nothing but skippable characters precedes `pos` on its
    /// line, i.e. `pos` is the line's first content position.
    fn at_line_content_start(&self, pos: usize, env: &Env) -> bool {
        let mut cursor = pos;
        while cursor > 0 {
            match self.region.chars.get(cursor - 1) {
                Some('\n') | Some('\r') => return true,
                Some(c) if self.profile(env).skip.matches(*c) => cursor -= 1,
                _ => return false,
            }
        }
        true
    }

    fn line_has_content(&self, pos: usize, env: &Env) -> bool {
        let mut cursor = self.skip(pos, env);
        let end = self.lines.line_end(pos);
        while cursor < end {
            if comment_len_at(self.region, cursor, self.profile(env)).is_some() {
                return false;
            }
            match self.region.chars.get(cursor) {
                Some(c) if self.profile(env).skip.matches(*c) => cursor += 1,
                Some('\r') | Some('\n') => return false,
                Some(_) => return true,
                None => return false,
            }
        }
        false
    }

    /// Skips transparent material (skip chars, comments, terminators) to the
    /// region end or the next content position.
    fn transparent_end(&self, pos: usize, env: &Env) -> Option<usize> {
        let mut cursor = self.skip(pos, env);
        loop {
            if cursor >= self.region.chars.len() {
                return Some(cursor);
            }
            if let Some(len) = comment_len_at(self.region, cursor, self.profile(env)) {
                let after_comment = cursor + len;
                if after_comment >= self.region.chars.len() {
                    return Some(after_comment);
                }
                cursor = self.lines.next_line_start(after_comment);
                continue;
            }
            match self.region.chars.get(cursor) {
                Some('\r') | Some('\n') => cursor = self.lines.next_line_start(cursor),
                Some(c) if self.profile(env).skip.matches(*c) => cursor += 1,
                _ => return Some(cursor),
            }
        }
    }

    // -- repetitions --------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn each(
        &mut self,
        plus: bool,
        sep: Option<&Pattern>,
        trailing: bool,
        bounds: Option<(u32, Option<u32>)>,
        body: &Pattern,
        bind: Option<String>,
        pos: usize,
        env: &mut Env,
        _cont: &Continuation<'_>, // tail-bounding lands with Task 4
    ) -> Option<Out> {
        let min = match bounds {
            Some((min, _)) => min.max(if plus { 1 } else { 0 }),
            None if plus => 1,
            None => 0,
        };
        let max = bounds.and_then(|(_, max)| max);
        let mut items: Vec<Capture> = Vec::new();
        let mut cursor = pos;
        let mut iterations = 0usize;

        // What follows a tail-bounded fragment inside the body (§8.3.6):
        // either another sep-continued iteration, or the each's own tail.
        let body_tail = Continuation::Repeat {
            sep,
            trailing,
            body,
            after: _cont,
        };

        // First iteration: no separator.
        let body_start = self.skip(cursor, env);
        match self.sequence(&body.elems, body_start, env, &body_tail) {
            Some(out) => {
                items.push(self.item_capture(&out, body_start, out.end));
                cursor = out.end;
                iterations += 1;
            }
            None => {
                if min > 0 {
                    self.note_failure(body_start, "expected at least one iteration");
                    return None;
                }
                return self.each_result(bind, items, pos, cursor);
            }
        }

        // Separator-continued iterations.
        loop {
            if let Some(max) = max
                && iterations as u32 >= max
            {
                break;
            }
            let before_sep = cursor;
            let mut after_sep = cursor;
            let mut sep_matched = false;
            if let Some(sep_pattern) = sep {
                let Some(out) = self.sequence(&sep_pattern.elems, cursor, env, &Continuation::End)
                else {
                    break;
                };
                after_sep = out.end;
                sep_matched = true;
            }
            // Inside an `indent` block the repetition's lines ARE its
            // iterations: the next iteration begins at the next content
            // line, never mid-line (§8.3.5's line discipline).
            let body_start = if env.open_blocks.is_empty() {
                self.skip(after_sep, env)
            } else {
                self.transparent_end(after_sep, env)?
            };
            // A repetition iteration that begins a new line must sit
            // exactly at the innermost block's base column; any other
            // column ends the repetition, and the block's own termination
            // check judges what follows (§8.3.5).
            if !env.open_blocks.is_empty()
                && self.at_line_content_start(body_start, env)
                && self.lines.col(body_start) != *env.open_blocks.last().unwrap()
            {
                cursor = before_sep;
                break;
            }
            match self.sequence(&body.elems, body_start, env, &body_tail) {
                Some(out) => {
                    if out.end == cursor && sep.is_none() {
                        // Zero-progress iteration ends the repetition.
                        break;
                    }
                    items.push(self.item_capture(&out, body_start, out.end));
                    cursor = out.end;
                    iterations += 1;
                }
                None => {
                    if sep_matched && trailing {
                        cursor = after_sep;
                        break;
                    }
                    cursor = before_sep;
                    break;
                }
            }
            let _ = sep_matched;
        }

        if iterations < min as usize {
            self.note_failure(cursor, "too few iterations");
            return None;
        }
        self.each_result(bind, items, pos, cursor)
    }

    fn each_result(
        &self,
        bind: Option<String>,
        items: Vec<Capture>,
        pos: usize,
        end: usize,
    ) -> Option<Out> {
        let capture = Capture {
            kind: CaptureKind::List(items),
            matched: self.text(pos, end),
            span: self.span(pos, end),
        };
        let mut out = Out::empty(end);
        if let Some(name) = bind {
            out.binds.push((name, capture));
        }
        Some(out)
    }

    /// The capture for one iteration: any binds make an item record; no
    /// binds use the body's primary capture (plan §2.2).
    fn item_capture(&self, out: &Out, start: usize, end: usize) -> Capture {
        if out.binds.is_empty()
            && let Some(primary) = &out.primary
        {
            return primary.clone();
        }
        Capture {
            kind: CaptureKind::Record {
                tag: "item".to_string(),
                fields: out.binds.clone(),
            },
            matched: self.text(start, end),
            span: self.span(start, end),
        }
    }

    // -- oneof branches -----------------------------------------------------

    fn branch(
        &mut self,
        label: &str,
        branch: &Pattern,
        pos: usize,
        env: &mut Env,
        cont: &Continuation<'_>,
    ) -> Option<Out> {
        let out = self.sequence(&branch.elems, pos, env, cont)?;
        // Re-tag rule: a branch whose body is a single bare rule reference
        // inherits that rule's record fields (plan §2.2).
        let mut fields = out.binds.clone();
        let record = if branch.elems.len() == 1 {
            if let Some(primary) = &out.primary
                && let CaptureKind::Record { fields: inner, .. } = &primary.kind
            {
                fields = inner.clone();
            }
            Capture {
                kind: CaptureKind::Record {
                    tag: label.to_string(),
                    fields,
                },
                matched: self.text(pos, out.end),
                span: self.span(pos, out.end),
            }
        } else {
            Capture {
                kind: CaptureKind::Record {
                    tag: label.to_string(),
                    fields,
                },
                matched: self.text(pos, out.end),
                span: self.span(pos, out.end),
            }
        };
        // The branch's binds stay visible to the enclosing sequence (the
        // document rule's `flowNode as root` must surface as a `root`
        // field) while the tagged record is the branch's body value.
        let mut result = self.out_with(None, out.end, Some(record));
        result.binds = out.binds;
        Some(result)
    }

    // -- rule references ----------------------------------------------------

    fn rule_ref(
        &mut self,
        path: &[String],
        ctx: &[(String, CtxExpr)],
        pos: usize,
        env: &mut Env,
        cont: &Continuation<'_>,
    ) -> Option<(Capture, usize)> {
        if self.depth > 256 {
            self.note_failure(pos, "rule recursion too deep");
            return None;
        }
        let (grammar_index, rule_index) = self.resolve(path, env)?;
        let rule = self.set.grammars[grammar_index].rules[rule_index].clone();
        // Context: declared fields with defaults, overridden by bindings
        // evaluated in the CALLER's scope (§8.3.7). Evaluated before the memo
        // check because context identity is part of the memo key.
        let mut context_pairs: Vec<(String, Capture)> = Vec::new();
        for field in &rule.context {
            let value = match ctx.iter().find(|(name, _)| name == &field.name) {
                Some((_, expression)) => self.eval_or_capture(expression, env),
                None => match &field.default {
                    Some(_) => Capture {
                        kind: CaptureKind::Opt(None),
                        matched: String::new(),
                        span: Span::missing(self.absolute(pos)),
                    },
                    None => continue,
                },
            };
            context_pairs.push((field.name.clone(), value));
        }
        let key = MemoKey {
            grammar: grammar_index,
            rule: rule_index,
            pos,
            skip: env.skip,
            blocks: env.open_blocks.clone(),
            context: context_signature(&context_pairs),
            tail: continuation_id(cont),
        };
        // §8.7 backstop: a re-entrant invocation against an in-progress memo
        // entry fails immediately. This is what keeps pathological recursive
        // rules (including zero-width self-recursion inside `peek`) finite
        // even before the static left-recursion check runs.
        if self.in_progress.contains(&key) {
            self.note_failure(
                pos,
                "recursive rule invocation cut at an in-progress memo entry",
            );
            return None;
        }
        if let Some(cached) = self.memo.get(&key) {
            return cached.clone();
        }
        self.in_progress.insert(key.clone());
        let mut inner = Env {
            skip: SkipMode::On,
            grammar: grammar_index,
            open_blocks: env.open_blocks.clone(),
            scope: HashMap::new(),
            rule: Some((grammar_index, rule_index)),
        };
        for (name, value) in &context_pairs {
            inner.scope.insert(name.clone(), value.clone());
        }
        self.depth += 1;
        self.rule_stack.push(rule.name.clone());
        // The caller's continuation bounds fragments at the rule's edge
        // (§8.3.6); the depth counter cuts recursive tail evaluation.
        let out = self.sequence(&rule.pattern.elems, pos, &mut inner, cont);
        self.rule_stack.pop();
        self.depth -= 1;
        self.in_progress.remove(&key);
        let result = out.map(|out| {
            let capture = match rule.pattern.elems.first().map(|element| &element.kind) {
                // If the rule's top-level construct is a oneof, its record (tag
                // = chosen branch) IS the rule's record (plan §2.2).
                Some(PatKind::OneOf { .. }) => out.primary.clone().unwrap_or(Capture {
                    kind: CaptureKind::Record {
                        tag: rule.name.clone(),
                        fields: out.binds.clone(),
                    },
                    matched: self.text(pos, out.end),
                    span: self.span(pos, out.end),
                }),
                _ => Capture {
                    kind: CaptureKind::Record {
                        tag: rule.name.clone(),
                        fields: out.binds.clone(),
                    },
                    matched: self.text(pos, out.end),
                    span: self.span(pos, out.end),
                },
            };
            (capture, out.end)
        });
        self.memo.insert(key, result.clone());
        result
    }

    fn resolve(&self, path: &[String], env: &Env) -> Option<(usize, usize)> {
        let (grammar_index, rule_name) = match path.len() {
            1 => (env.grammar, path[0].as_str()),
            2 => {
                let index = self.set.grammars.iter().position(|g| g.name == path[0])?;
                (index, path[1].as_str())
            }
            _ => return None,
        };
        let rule_index = self.set.grammars[grammar_index]
            .rules
            .iter()
            .position(|rule| rule.name == rule_name)?;
        Some((grammar_index, rule_index))
    }

    // -- indent blocks (§8.3.5) ---------------------------------------------

    fn indent(
        &mut self,
        body: Option<&Pattern>,
        verbatim: Option<String>,
        pos: usize,
        env: &mut Env,
        _cont: &Continuation<'_>, // indent blocks are committed; tail-bounding lands with Task 4
    ) -> Option<Out> {
        let mut cursor = pos;
        let base_column;
        if self.line_has_content(cursor, env) {
            // Mid-line start: B is the current column, matching begins here.
            base_column = self.lines.col(cursor);
        } else {
            // End-of-line start: advance through transparent lines; B is the
            // next content line's column. A cursor whose remaining line is
            // blank counts as end-of-line (plan §1.4.8).
            match self.transparent_end(cursor, env) {
                Some(after) if after >= self.region.chars.len() => {
                    // No further lines: the block matches empty.
                    let mut out = Out::empty(cursor);
                    if let Some(name) = &verbatim {
                        out.binds.push((
                            name.clone(),
                            Capture {
                                kind: CaptureKind::Text(TextKind::Raw),
                                matched: String::new(),
                                span: self.span(cursor, cursor),
                            },
                        ));
                    }
                    return Some(out);
                }
                Some(after) => {
                    cursor = after;
                    base_column = self.lines.col(after);
                }
                None => return None,
            }
        }
        if let Some(last) = env.open_blocks.last()
            && base_column <= *last
        {
            self.note_failure(
                cursor,
                "block is not strictly deeper than the enclosing block",
            );
            return None;
        }

        if let Some(name) = &verbatim {
            return self.indent_verbatim(name.clone(), base_column, cursor, env);
        }
        let body = body?;

        let mut inner = env.clone();
        inner.open_blocks.push(base_column);
        let mut iterations = 0usize;
        let mut last_out = Out::empty(cursor);
        loop {
            if iterations > 0 {
                // Place the cursor at the next non-transparent line's
                // content, which must sit exactly at the base column.
                let next = self.transparent_end(cursor, &inner)?;
                if next >= self.region.chars.len() {
                    break;
                }
                if self.lines.col(next) != base_column {
                    cursor = next;
                    break;
                }
                cursor = next;
            }
            let saved_scope = inner.scope.clone();
            match self.sequence(&body.elems, cursor, &mut inner, &Continuation::End) {
                Some(out) => {
                    // Only skip characters or one comment form may remain.
                    let mut after = self.skip(out.end, &inner);
                    if let Some(len) = comment_len_at(self.region, after, self.profile(&inner)) {
                        after += len;
                    }
                    iterations += 1;
                    cursor = after;
                    last_out = out;
                }
                None => {
                    inner.scope = saved_scope;
                    if iterations == 0 {
                        return None;
                    }
                    self.note_failure(cursor, "a line belonging to this block could not be parsed");
                    return None;
                }
            }
        }
        // Termination: a following line at a column > B must belong to an
        // open block; otherwise it is a committed-block failure.
        if let Some(next) = self.peek_content(cursor, &inner) {
            let column = self.lines.col(next);
            if column > base_column && !inner.open_blocks.contains(&column) {
                self.note_failure(next, "indentation level does not match any open block");
                return None;
            }
        }
        Some(Out {
            end: cursor,
            binds: last_out.binds,
            primary: last_out.primary,
        })
    }

    fn indent_verbatim(
        &mut self,
        name: String,
        base_column: usize,
        start: usize,
        env: &Env,
    ) -> Option<Out> {
        let mut cursor = start;
        let mut end;
        loop {
            if cursor >= self.region.chars.len() {
                end = self.region.chars.len();
                break;
            }
            let content = self.skip(cursor, env);
            if content >= self.region.chars.len() {
                end = self.region.chars.len();
                break;
            }
            if self.lines.col(content) >= base_column {
                let line_end = self.lines.line_end(content);
                end = line_end.min(self.region.chars.len());
                if end >= self.region.chars.len() {
                    break;
                }
                cursor = self.lines.next_line_start(content);
                continue;
            }
            end = cursor;
            break;
        }
        let capture = self.text_capture(start, end, TextKind::Raw);
        let mut out = Out::empty(end);
        out.binds.push((name, capture));
        Some(out)
    }

    fn peek_content(&self, pos: usize, env: &Env) -> Option<usize> {
        let after = self.transparent_end(pos, env)?;
        if after >= self.region.chars.len() {
            None
        } else {
            Some(after)
        }
    }

    // -- fragments ----------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn fragment(
        &mut self,
        kind: FragKind,
        insensitive: bool,
        validator: Option<&[String]>,
        bind: Option<String>,
        siblings: &[PatElem],
        cont: &Continuation<'_>,
        pos: usize,
        env: &mut Env,
    ) -> Option<Out> {
        let (start, end, folded) = match &kind {
            FragKind::Ident => {
                let start = self.skip(pos, env);
                let end = self.ident_at(start)?;
                (start, end, false)
            }
            FragKind::Word => {
                let start = self.skip(pos, env);
                let mut end = start;
                if matches!(
                    self.region.chars.get(end),
                    Some(c) if c.is_ascii_alphabetic() || *c == '_' || *c == '$'
                ) {
                    end += 1;
                    while matches!(
                        self.region.chars.get(end),
                        Some(c) if c.is_ascii_alphanumeric() || *c == '_' || *c == '$'
                    ) {
                        end += 1;
                    }
                } else {
                    self.note_failure(start, "expected a word");
                    return None;
                }
                (start, end, false)
            }
            FragKind::Tag => {
                let start = self.skip(pos, env);
                let mut end = start;
                while matches!(
                    self.region.chars.get(end),
                    Some(c) if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')
                ) {
                    end += 1;
                }
                if end == start {
                    self.note_failure(start, "expected a tag token");
                    return None;
                }
                (start, end, insensitive)
            }
            FragKind::Int => {
                let start = self.skip(pos, env);
                let end = self.int_at(start)?;
                (start, end, false)
            }
            FragKind::Float => {
                let start = self.skip(pos, env);
                let end = self.float_at(start)?;
                (start, end, false)
            }
            FragKind::Str => {
                let start = self.skip(pos, env);
                let Some((value, end)) = self.str_at(start) else {
                    self.note_failure(start, "expected a double-quoted string");
                    return None;
                };
                let capture = Capture {
                    kind: CaptureKind::Text(TextKind::Str),
                    // The capture value of `$str` is the DECODED string
                    // content; splices quote it back (plan §2.2).
                    matched: value,
                    span: self.span(start, end),
                };
                let mut out = Out::empty(end);
                match bind {
                    Some(name) => out.binds.push((name, capture)),
                    None => out.primary = Some(capture),
                }
                return Some(out);
            }
            FragKind::Tt => {
                // Minimal Task-3 form: one whitespace-delimited token. The
                // profile-string-aware balanced form lands in Task 6.
                let start = self.skip(pos, env);
                let mut end = start;
                while matches!(
                    self.region.chars.get(end),
                    Some(c) if !c.is_whitespace() && !matches!(c, '{' | '}' | ',' | ';')
                ) {
                    end += 1;
                }
                if end == start {
                    self.note_failure(start, "expected a token");
                    return None;
                }
                (start, end, false)
            }
            FragKind::Text | FragKind::Template => {
                let start = self.skip(pos, env);
                let end = self.tail_bounded_extent(start, siblings, cont, env)?;
                (start, end, false)
            }
            FragKind::Raw(_) | FragKind::Expr | FragKind::Type | FragKind::Block => {
                let start = self.skip(pos, env);
                let end = self.tail_bounded_extent(start, siblings, cont, env)?;
                (start, end, false)
            }
        };

        if let Some(path) = validator
            && !self.validator_matches(path, start, end, env)
        {
            self.note_failure(start, "fragment validator rejected the match");
            return None;
        }

        let capture = match &kind {
            FragKind::Int => {
                let text = self.text(start, end);
                let value = text.parse::<i64>().unwrap_or(0);
                Capture {
                    kind: CaptureKind::Int(value),
                    matched: text,
                    span: self.span(start, end),
                }
            }
            FragKind::Float => {
                let text = self.text(start, end);
                let value = text.parse::<f64>().unwrap_or(0.0);
                Capture {
                    kind: CaptureKind::Float(value),
                    matched: text,
                    span: self.span(start, end),
                }
            }
            _ => {
                let text = self.text(start, end);
                let text = if folded { text.to_lowercase() } else { text };
                let text_kind = match &kind {
                    FragKind::Ident => TextKind::Ident,
                    FragKind::Word => TextKind::Word,
                    FragKind::Tag => TextKind::Tag,
                    _ => TextKind::Raw,
                };
                Capture {
                    kind: CaptureKind::Text(text_kind),
                    matched: text.trim().to_string(),
                    span: self.span(start, end),
                }
            }
        };
        let mut out = Out::empty(end);
        match bind {
            Some(name) => out.binds.push((name, capture)),
            None => out.primary = Some(capture),
        }
        Some(out)
    }

    fn ident_at(&mut self, start: usize) -> Option<usize> {
        if !matches!(
            self.region.chars.get(start),
            Some(c) if c.is_ascii_alphabetic() || *c == '_'
        ) {
            self.note_failure(start, "expected an identifier");
            return None;
        }
        let mut end = start + 1;
        while matches!(
            self.region.chars.get(end),
            Some(c) if c.is_ascii_alphanumeric() || *c == '_'
        ) {
            end += 1;
        }
        Some(end)
    }

    fn int_at(&mut self, start: usize) -> Option<usize> {
        let mut end = start;
        if self.region.chars.get(end) == Some(&'-') {
            end += 1;
        }
        let digits = end;
        while matches!(self.region.chars.get(end), Some(c) if c.is_ascii_digit()) {
            end += 1;
        }
        if end == digits {
            self.note_failure(start, "expected an integer");
            return None;
        }
        Some(end)
    }

    fn float_at(&mut self, start: usize) -> Option<usize> {
        let mut end = self.int_at(start)?;
        if self.region.chars.get(end) == Some(&'.') {
            end += 1;
            let frac = end;
            while matches!(self.region.chars.get(end), Some(c) if c.is_ascii_digit()) {
                end += 1;
            }
            if end == frac {
                self.note_failure(start, "expected digits after the decimal point");
                return None;
            }
        } else {
            self.note_failure(start, "expected a float");
            return None;
        }
        Some(end)
    }

    /// A double-quoted string with backslash escapes; returns the decoded
    /// value and the end position (past the closing quote).
    fn str_at(&self, start: usize) -> Option<(String, usize)> {
        if self.region.chars.get(start) != Some(&'"') {
            return None;
        }
        let mut cursor = start + 1;
        let mut value = String::new();
        loop {
            match self.region.chars.get(cursor) {
                Some('"') => return Some((value, cursor + 1)),
                Some('\\') => {
                    let escaped = *self.region.chars.get(cursor + 1)?;
                    value.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '0' => '\0',
                        other => other,
                    });
                    cursor += 2;
                }
                Some(_) => {
                    value.push(*self.region.chars.get(cursor).unwrap());
                    cursor += 1;
                }
                None => return None,
            }
        }
    }

    /// The smallest extent (at least one character) whose tail — the
    /// remaining siblings extended outward — matches as a whole at the end
    /// position (§8.3.6, minus Task 6's parse-integration). With no tail at
    /// all, falls back to the region remainder (`$text` semantics).
    fn tail_bounded_extent(
        &mut self,
        start: usize,
        siblings: &[PatElem],
        cont: &Continuation<'_>,
        env: &mut Env,
    ) -> Option<usize> {
        let has_tail = !siblings.is_empty() || has_effective_tail(cont);
        if !has_tail {
            return Some(self.region.chars.len());
        }
        let tail = Continuation::Elems(siblings, cont);
        let mut end = start + 1;
        loop {
            if end > self.region.chars.len() {
                self.note_failure(start, "no boundary satisfied the pattern tail");
                return None;
            }
            let saved_scope = env.scope.clone();
            let hit = self.tail_matches(end, &tail, env);
            env.scope = saved_scope;
            if hit {
                return Some(end);
            }
            end += 1;
        }
    }

    /// Speculatively: does the continuation match at `pos`? Tail sequences
    /// are exact-anchored (§8.3.6): the tail's first element may not skip.
    fn tail_matches(&mut self, pos: usize, cont: &Continuation<'_>, env: &mut Env) -> bool {
        if !self.spend() || self.depth > 128 {
            return false;
        }
        match cont {
            Continuation::End => true,
            Continuation::Elems(elems, after) => {
                if elems.is_empty() {
                    return self.tail_matches(pos, after, env);
                }
                self.depth += 1;
                let saved_scope = env.scope.clone();
                let hit = self.sequence_exact_first(elems, pos, env, after).is_some();
                env.scope = saved_scope;
                self.depth -= 1;
                hit
            }
            Continuation::Repeat {
                sep,
                trailing,
                body,
                after,
            } => {
                self.depth += 1;
                let mut hit = false;
                if let Some(sep_pattern) = sep
                    && let Some(sep_out) =
                        self.sequence(&sep_pattern.elems, pos, env, &Continuation::End)
                {
                    let body_start = self.skip(sep_out.end, env);
                    let next = Continuation::Repeat {
                        sep: *sep,
                        trailing: *trailing,
                        body,
                        after,
                    };
                    // The speculative body inherits the repeat tail so
                    // its own tail-bounded fragments stop at the right
                    // boundary (§8.3.6).
                    if let Some(body_out) = self.sequence(&body.elems, body_start, env, &next) {
                        hit = self.tail_matches(body_out.end, &next, env);
                        if !hit && *trailing {
                            hit = true;
                        }
                    } else if *trailing {
                        hit = true;
                    }
                }
                if !hit {
                    hit = self.tail_matches(pos, after, env);
                }
                self.depth -= 1;
                hit
            }
        }
    }

    fn validator_matches(&mut self, path: &[String], start: usize, end: usize, env: &Env) -> bool {
        let Some((grammar_index, rule_index)) = self.resolve(path, env) else {
            return false;
        };
        let rule = self.set.grammars[grammar_index].rules[rule_index].clone();
        let text = self.text(start, end);
        let sub_region = MatchRegion::new(self.region.source, self.absolute(start), &text);
        match_entry(self.set, grammar_index, &rule.pattern, &sub_region).is_ok()
    }

    // -- captures and values ------------------------------------------------

    fn text(&self, from: usize, to: usize) -> String {
        let from = from.min(self.region.chars.len());
        let to = to.min(self.region.chars.len());
        self.region.chars[from..to].iter().collect()
    }

    fn span(&self, from: usize, to: usize) -> Span {
        let from = from.min(self.region.chars.len());
        let to = to.min(self.region.chars.len());
        let start = self.absolute(from);
        let end = if to >= self.region.chars.len() {
            self.region.base + self.region.byte_len
        } else {
            self.absolute(to)
        };
        Span::new(start, end.max(start))
    }

    fn absolute(&self, char_index: usize) -> usize {
        self.region.base
            + self
                .region
                .byte_offsets
                .get(char_index)
                .copied()
                .unwrap_or(self.region.byte_len)
    }

    fn text_capture(&self, from: usize, to: usize, kind: TextKind) -> Capture {
        let text = self.text(from, to);
        Capture {
            kind: CaptureKind::Text(kind),
            matched: text.trim().to_string(),
            span: self.span(from, to),
        }
    }

    // -- `where` evaluation -------------------------------------------------

    fn eval(&mut self, expression: &CtxExpr, env: &mut Env) -> Option<CtxVal> {
        match expression {
            CtxExpr::Str(value) => Some(CtxVal::Str(value.clone())),
            CtxExpr::Int(value) => Some(CtxVal::Int(*value)),
            CtxExpr::Float(value) => Some(CtxVal::Float(*value)),
            CtxExpr::Bool(value) => Some(CtxVal::Bool(*value)),
            CtxExpr::Capture { path, accessor } => {
                let capture = self.lookup(path, env)?;
                apply_accessor(capture, accessor.clone())
            }
            CtxExpr::Bin(op, lhs, rhs) => {
                let lhs = self.eval(lhs, env)?;
                let rhs = self.eval(rhs, env)?;
                eval_bin(op.clone(), &lhs, &rhs)
            }
            CtxExpr::Not(inner) => match self.eval(inner, env)? {
                CtxVal::Bool(value) => Some(CtxVal::Bool(!value)),
                _ => None,
            },
            CtxExpr::SomeIn { var, list, cond } => {
                let items = self.eval_list(list, env)?;
                for item in items {
                    let saved = env.scope.clone();
                    env.scope.insert(var.clone(), item);
                    let hit = self.eval(cond, env) == Some(CtxVal::Bool(true));
                    env.scope = saved;
                    if hit {
                        return Some(CtxVal::Bool(true));
                    }
                }
                Some(CtxVal::Bool(false))
            }
            CtxExpr::AllIn { var, list, cond } => {
                let items = self.eval_list(list, env)?;
                let mut all = true;
                for item in items {
                    let saved = env.scope.clone();
                    env.scope.insert(var.clone(), item);
                    if self.eval(cond, env) != Some(CtxVal::Bool(true)) {
                        all = false;
                    }
                    env.scope = saved;
                }
                Some(CtxVal::Bool(all))
            }
            CtxExpr::Present { path } => {
                let present = match self.lookup(path, env) {
                    Some(capture) => capture.is_present(),
                    None => false,
                };
                Some(CtxVal::Bool(present))
            }
            CtxExpr::Call { path, .. } => {
                self.note_failure(
                    0,
                    format!(
                        "compile-time function `{}` needs §8.5 support (Task 7)",
                        path.join(".")
                    ),
                );
                None
            }
        }
    }

    fn eval_or_capture(&mut self, expression: &CtxExpr, env: &mut Env) -> Capture {
        match self.eval(expression, env) {
            Some(CtxVal::Str(value)) => Capture {
                kind: CaptureKind::Text(TextKind::Raw),
                matched: value,
                span: Span::missing(0),
            },
            Some(CtxVal::Int(value)) => Capture {
                kind: CaptureKind::Int(value),
                matched: value.to_string(),
                span: Span::missing(0),
            },
            Some(CtxVal::Float(value)) => Capture {
                kind: CaptureKind::Float(value),
                matched: format!("{value}"),
                span: Span::missing(0),
            },
            Some(CtxVal::Bool(value)) => Capture {
                kind: CaptureKind::Text(TextKind::Raw),
                matched: value.to_string(),
                span: Span::missing(0),
            },
            Some(CtxVal::Capture(capture)) => capture,
            None => Capture {
                kind: CaptureKind::Opt(None),
                matched: String::new(),
                span: Span::missing(0),
            },
        }
    }

    fn eval_list(&mut self, expression: &CtxExpr, env: &mut Env) -> Option<Vec<Capture>> {
        match self.eval(expression, env)? {
            CtxVal::Capture(capture) => match capture.kind {
                CaptureKind::List(items) => Some(items),
                _ => None,
            },
            _ => None,
        }
    }

    /// Resolves a capture path against the current scope, unwrapping one
    /// `Opt` layer per navigation step.
    fn lookup(&self, path: &[String], env: &Env) -> Option<Capture> {
        let mut current = env.scope.get(&path[0]).cloned()?;
        for (index, segment) in path[1..].iter().enumerate() {
            if let CaptureKind::Opt(Some(inner)) = current.kind {
                current = *inner;
            }
            match &current.kind {
                CaptureKind::Record { fields, .. } => {
                    match fields.iter().find(|(name, _)| name == segment) {
                        Some((_, capture)) => current = capture.clone(),
                        // Field lookup first; a trailing accessor keyword is
                        // the fallback (plan §1.4.4).
                        None => {
                            let last = index + 2 == path.len();
                            if last {
                                current = apply_accessor_text(current, segment)?;
                            } else {
                                return None;
                            }
                        }
                    }
                }
                _ => return None,
            }
        }
        Some(current)
    }
}

fn body_value(out: &Out) -> Capture {
    if let Some(primary) = &out.primary {
        return primary.clone();
    }
    if out.binds.len() == 1 {
        return out.binds[0].1.clone();
    }
    Capture {
        kind: CaptureKind::Record {
            tag: "some".to_string(),
            fields: out.binds.clone(),
        },
        matched: String::new(),
        span: Span::missing(0),
    }
}

fn comment_len_at(
    region: &MatchRegion<'_>,
    char_pos: usize,
    profile: &LexProfile,
) -> Option<usize> {
    let text: String = region.chars[char_pos..].iter().collect();
    let form = profile
        .comments
        .iter()
        .filter(|form| text.starts_with(&form.opener))
        .max_by_key(|form| form.opener.len())?;
    let after = &text[form.opener.len()..];
    Some(match &form.closer {
        None => {
            let chars_to_eol = after
                .chars()
                .take_while(|c| *c != '\n' && *c != '\r')
                .count();
            form.opener.chars().count() + chars_to_eol
        }
        Some(closer) => {
            // An unterminated block comment is NOT transparent here; the
            // region scanner already rejected such regions.
            let opener_len = form.opener.chars().count();
            let mut scanned = String::new();
            let mut hit = None;
            for (index, c) in after.chars().enumerate() {
                scanned.push(c);
                if scanned.ends_with(closer.as_str()) {
                    hit = Some(opener_len + index + closer.chars().count());
                    break;
                }
            }
            hit?
        }
    })
}

/// A `where`-language value.
#[derive(Debug, Clone, PartialEq)]
enum CtxVal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Capture(Capture),
}

fn apply_accessor(capture: Capture, accessor: Option<Accessor>) -> Option<CtxVal> {
    match accessor {
        None => Some(CtxVal::Capture(capture)),
        // `.matched` trims edge whitespace (plan §1.4.4): indent blocks and
        // skip-run edges would otherwise leak `\n    ` prefixes.
        Some(Accessor::Matched) => Some(CtxVal::Str(capture.matched().trim().to_string())),
        Some(Accessor::Length) => {
            let length = match capture.kind {
                CaptureKind::List(items) => items.len(),
                CaptureKind::Record { fields, .. } => fields.len(),
                _ => 0,
            };
            Some(CtxVal::Int(length as i64))
        }
        // Real source-line mapping lands with Task 4's diagnostics work.
        Some(Accessor::Line) => Some(CtxVal::Int(0)),
        Some(Accessor::Col) => Some(CtxVal::Int(0)),
    }
}

/// Path-navigation fallback: applies an accessor named by the path's final
/// segment (`.matched`, `.length`, `.line`, `.col`), yielding a capture.
fn apply_accessor_text(capture: Capture, segment: &str) -> Option<Capture> {
    let accessor = match segment {
        "matched" => Accessor::Matched,
        "length" => Accessor::Length,
        "line" => Accessor::Line,
        "col" => Accessor::Col,
        _ => return None,
    };
    match apply_accessor(capture, Some(accessor)) {
        Some(CtxVal::Str(value)) => Some(Capture {
            kind: CaptureKind::Text(TextKind::Raw),
            matched: value,
            span: Span::missing(0),
        }),
        Some(CtxVal::Int(value)) => Some(Capture {
            kind: CaptureKind::Int(value),
            matched: value.to_string(),
            span: Span::missing(0),
        }),
        Some(CtxVal::Capture(capture)) => Some(capture),
        _ => None,
    }
}

/// A capture in a scalar position: its number, or its trimmed matched text.
fn coerce_scalar(value: &CtxVal) -> CtxVal {
    match value {
        CtxVal::Capture(capture) => match &capture.kind {
            CaptureKind::Int(value) => CtxVal::Int(*value),
            CaptureKind::Float(value) => CtxVal::Float(*value),
            _ => CtxVal::Str(capture.matched().trim().to_string()),
        },
        other => other.clone(),
    }
}

fn eval_bin(op: CtxBinOp, lhs: &CtxVal, rhs: &CtxVal) -> Option<CtxVal> {
    let result = match op {
        CtxBinOp::And => match (lhs, rhs) {
            (CtxVal::Bool(a), CtxVal::Bool(b)) => Some(*a && *b),
            _ => None,
        },
        CtxBinOp::Or => match (lhs, rhs) {
            (CtxVal::Bool(a), CtxVal::Bool(b)) => Some(*a || *b),
            _ => None,
        },
        CtxBinOp::Eq | CtxBinOp::Ne => {
            // A bare capture compares by its scalar value (the trimmed
            // matched text, or its number) — this makes `close == name`
            // work when both sides are captures (§8.3.4).
            let lhs = coerce_scalar(lhs);
            let rhs = coerce_scalar(rhs);
            let equal = match (&lhs, &rhs) {
                (CtxVal::Str(a), CtxVal::Str(b)) => a == b,
                (CtxVal::Int(a), CtxVal::Int(b)) => a == b,
                (CtxVal::Float(a), CtxVal::Float(b)) => a == b,
                (CtxVal::Bool(a), CtxVal::Bool(b)) => a == b,
                _ => return None,
            };
            Some(if op == CtxBinOp::Eq { equal } else { !equal })
        }
        CtxBinOp::Lt | CtxBinOp::Le | CtxBinOp::Gt | CtxBinOp::Ge => {
            use std::cmp::Ordering;
            let ordering = match (lhs, rhs) {
                (CtxVal::Int(a), CtxVal::Int(b)) => a.cmp(b),
                (CtxVal::Float(a), CtxVal::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
                _ => return None,
            };
            Some(match op {
                CtxBinOp::Lt => ordering == Ordering::Less,
                CtxBinOp::Le => ordering != Ordering::Greater,
                CtxBinOp::Gt => ordering == Ordering::Greater,
                _ => ordering != Ordering::Less,
            })
        }
    };
    result.map(CtxVal::Bool)
}

fn ctx_summary(expression: &CtxExpr) -> String {
    match expression {
        CtxExpr::Capture { path, .. } => path.join("."),
        CtxExpr::Bin(op, lhs, rhs) => {
            let symbol = match op {
                CtxBinOp::Eq => "==",
                CtxBinOp::Ne => "!=",
                CtxBinOp::Lt => "<",
                CtxBinOp::Le => "<=",
                CtxBinOp::Gt => ">",
                CtxBinOp::Ge => ">=",
                CtxBinOp::And => "&&",
                CtxBinOp::Or => "||",
            };
            format!("{} {} {}", ctx_summary(lhs), symbol, ctx_summary(rhs))
        }
        CtxExpr::Not(inner) => format!("!{}", ctx_summary(inner)),
        CtxExpr::Present { path } => format!("present({})", path.join(".")),
        CtxExpr::Str(value) => format!("\"{value}\""),
        CtxExpr::Int(value) => value.to_string(),
        CtxExpr::Float(value) => value.to_string(),
        CtxExpr::Bool(value) => value.to_string(),
        CtxExpr::SomeIn { .. } | CtxExpr::AllIn { .. } => "quantified condition".to_string(),
        CtxExpr::Call { path, .. } => format!("{}(…)", path.join(".")),
    }
}

/// Whether a continuation can still consume content: an `Elems` chain with
/// only empty links normalizes to `End` (no tail), while `Repeat` always
/// offers another iteration (§8.3.6).
fn has_effective_tail(cont: &Continuation<'_>) -> bool {
    match cont {
        Continuation::End => false,
        Continuation::Elems(elems, after) => !elems.is_empty() || has_effective_tail(after),
        Continuation::Repeat { .. } => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mega::pattern::parse_rule_declaration;

    /// Compiles a grammar body (rules only, default profile) into a set.
    fn compile_rules(rules: &[&str]) -> GrammarSet {
        let mut compiled = CompiledGrammar {
            name: "test".to_string(),
            profile: crate::mega::profile::default_profile(),
            rules: Vec::new(),
        };
        for rule in rules {
            let (name, context, pattern, _) =
                parse_rule_declaration(rule, 0, Span::new(0, rule.len())).unwrap();
            compiled.rules.push(CompiledRule {
                name,
                context,
                pattern,
            });
        }
        let mut set = GrammarSet::default();
        set.grammars.push(compiled);
        set.grammars.push(CompiledGrammar {
            name: "\u{0}default".to_string(),
            profile: crate::mega::profile::default_profile(),
            rules: Vec::new(),
        });
        set
    }

    fn match_rule(set: &GrammarSet, rule: &str, region: &str) -> Result<Capture, String> {
        let grammar_index = 0;
        let rule_index = set.grammars[0]
            .rules
            .iter()
            .position(|r| r.name == rule)
            .unwrap();
        let pattern = set.grammars[0].rules[rule_index].pattern.clone();
        let match_region = MatchRegion::new(region, 0, region);
        match_entry(set, grammar_index, &pattern, &match_region).map_err(|failure| failure.message)
    }

    #[test]
    fn oneof_yields_the_chosen_branch_record() {
        let set = compile_rules(&[
            "rule value { oneof { null => \"null\", number => number } }",
            "rule number { scan [0-9] as digits }",
        ]);
        let capture = match_rule(&set, "value", "42").unwrap();
        match capture.kind {
            CaptureKind::Record { tag, fields } => {
                assert_eq!(tag, "number");
                assert!(fields.iter().any(|(name, _)| name == "digits"));
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn where_compares_bare_captures_by_matched_text() {
        let set = compile_rules(&[
            "rule pair { ( scan [a-z] as left \">\" scan [a-z] as right where left == right ) }",
        ]);
        assert!(match_rule(&set, "pair", "a>a").is_ok());
        assert!(match_rule(&set, "pair", "a>b").is_err());
    }

    #[test]
    fn until_stops_at_the_stop_pattern_without_consuming_it() {
        let set = compile_rules(&["rule text { until { \"<\" } as t \"<\" }"]);
        let capture = match_rule(&set, "text", "hello<").unwrap();
        // The single bind IS the entry capture: the until's matched text,
        // with the stop `"<"` consumed by the literal that follows it.
        assert_eq!(capture.matched(), "hello");
    }

    #[test]
    fn each_repeats_with_separator_and_binds_a_list() {
        let set = compile_rules(&[
            "rule list { \"[\" each sep \",\" { scan [0-9] as n } as items \"]\" }",
        ]);
        let capture = match_rule(&set, "list", "[7, 42, 9]").unwrap();
        // The single bind IS the entry capture (match_entry §2.2).
        match capture.kind {
            CaptureKind::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[1].matched(), "42");
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    // -- Task 4: fuel budget, cycle cut, memoization -------------------------

    #[test]
    fn fuel_exhaustion_is_a_distinct_budget_error() {
        // A modest repetition burns far more operations than the tiny budget
        // given here; the failure must name the budget, not the pattern.
        let set = compile_rules(&["rule run { each { \"a\" } }"]);
        let grammar_index = 0;
        let pattern = set.grammars[0].rules[0].pattern.clone();
        let region = MatchRegion::new("aaaaaaaa", 0, "aaaaaaaa");
        let failure = match_entry_with_fuel(&set, grammar_index, &pattern, &region, 5)
            .expect_err("tiny fuel must not be enough");
        assert!(
            failure.message.contains("fuel budget exhausted"),
            "unexpected failure message: {}",
            failure.message
        );
        // The same region under the default budget matches cleanly.
        assert!(match_entry(&set, grammar_index, &pattern, &region).is_ok());
    }

    #[test]
    fn reentrant_rule_invocation_is_cut_not_spun() {
        // `peek { r }` re-enters `r` at the SAME position while it is still
        // in progress: the memo cycle cut fails the lookahead, the optional
        // matches empty, and the literal consumes — before Task 4 this spun
        // until the recursion depth cap.
        let set = compile_rules(&["rule r { optional { peek { r } } \"x\" }"]);
        assert!(match_rule(&set, "r", "x").is_ok());
        assert!(match_rule(&set, "r", "y").is_err());
    }

    #[test]
    fn memoization_preserves_results_across_repeat_visits() {
        // The inner `pair` rule is visited repeatedly at the same positions
        // through the repetition and the `where` re-check; memoized results
        // must be indistinguishable from fresh matches.
        let set = compile_rules(&[
            "rule list { \"[\" each sep \",\" { pair } as items \"]\" }",
            "rule pair { scan [0-9] as n \">\" scan [0-9] as m }",
        ]);
        let capture = match_rule(&set, "list", "[1>2, 3>4, 5>6]").unwrap();
        match capture.kind {
            CaptureKind::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[2].matched(), "5>6");
                match &items[2].kind {
                    CaptureKind::Record { tag, fields } => {
                        assert_eq!(tag, "pair");
                        assert!(fields.iter().any(|(name, _)| name == "m"));
                    }
                    other => panic!("unexpected {:?}", other),
                }
            }
            other => panic!("unexpected {:?}", other),
        }
    }
}
