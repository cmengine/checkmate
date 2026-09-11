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
use cme_core::magic::{FragKind, PatElem, PatKind, Pattern};

use crate::diagnostics::Diagnostic;
use crate::mega::matcher::{
    CompiledGrammar, CompiledRule, GrammarSet, MatchFailure, MatchRegion, match_entry,
    match_entry_binds,
};
use crate::mega::pattern::{parse_pattern, parse_rule_declaration};
use crate::mega::profile::{default_profile, with_default_strings};
use crate::mega::scan::{InvocationScan, MagicScan, REGION_SCAN_HINT, scan_magic};
use crate::mega::template::{elaborate, elaborate_seeded, parse_template};

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

/// Knobs for the expansion pass. The default is byte-deterministic output
/// with no annotations; `provenance` adds a `// @ magic(name) src:L:C`
/// comment above the line of every root invocation of the ORIGINAL file
/// (§8.6: origin is recorded only for diagnostics — it never affects
/// processing, and generated code carries no comments of its own).
#[derive(Debug, Clone, Copy, Default)]
pub struct ExpandOptions {
    pub provenance: bool,
}

/// The whitepaper's expansion-tree depth cap (§8.6), counting all origins.
const DEPTH_CAP: usize = 64;

/// Expands every megaprogram in `source`. Returns the expanded source, or
/// every diagnostic encountered (spans anchored in `source`).
pub fn expand_source(source: &str) -> Result<ExpansionOutcome, Vec<Diagnostic>> {
    expand_source_with(source, ExpandOptions::default())
}

/// [`expand_source`] with options (provenance comments; see
/// [`ExpandOptions`]).
pub fn expand_source_with(
    source: &str,
    options: ExpandOptions,
) -> Result<ExpansionOutcome, Vec<Diagnostic>> {
    let owned = source.to_string();
    run_on_expansion_stack(move || expand_source_inner(&owned, options))
}

/// Stack budget of the dedicated expansion thread. Deeply nested rule
/// invocations cost kilobytes of stack per level (the packrat frames are
/// wide); running the pipeline on its own thread gives matching a fixed,
/// generous budget regardless of how small the calling thread's stack is.
const EXPANSION_STACK_BYTES: usize = 64 * 1024 * 1024;

thread_local! {
    /// Set on the dedicated expansion thread so nested `expand_source`
    /// calls (there are none today, but the evaluator can parse deeply)
    /// reuse the same big stack instead of spawning again.
    static ON_EXPANSION_STACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `work` on a thread with a large stack — or inline when already on
/// one. The work is closure-converted over owned data to satisfy `'static`.
fn run_on_expansion_stack<T, F>(work: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    if ON_EXPANSION_STACK.with(std::cell::Cell::get) {
        return work();
    }
    let run = move || {
        ON_EXPANSION_STACK.with(|cell| cell.set(true));
        work()
    };
    let result = std::thread::Builder::new()
        .stack_size(EXPANSION_STACK_BYTES)
        .spawn(run)
        .and_then(|handle| {
            handle
                .join()
                .map_err(|_| std::io::Error::other("expansion thread aborted"))
        });
    match result {
        Ok(value) => value,
        // The work panicked: resume the unwind so the original panic
        // surfaces at the caller (mirrors `join`'s default behavior).
        Err(_) => std::panic::resume_unwind(Box::new(ExpansionThreadPanic)),
    }
}

/// The panic payload re-raised when the expansion thread aborted. Internal
/// expansion never panics by design (every failure is a diagnostic), so
/// this only ever carries a bug.
struct ExpansionThreadPanic;

/// The actual pipeline; always invoked on the dedicated expansion stack.
fn expand_source_inner(
    source: &str,
    options: ExpandOptions,
) -> Result<ExpansionOutcome, Vec<Diagnostic>> {
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
    check_profiles(&set, &mut diagnostics);
    reject_left_recursion(&set, &mut diagnostics);
    check_declaration_order(&scan, &macros, &mut diagnostics);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    // The §8.5 compile-time evaluator: the file's own pure functions run
    // against the interpreter during expansion (single-file megaprograms
    // make the §8.7.4 import-purity check trivially satisfied).
    let engine = crate::mega::cteval::CtEngine::new(source, &scan, &set);

    // Provenance comments (§8.6, opt-in): computed ONCE against the original
    // file, for its ROOT invocations only (those contained in no other
    // invocation — nested sites are covered by their root's comment, and a
    // comment inside a foreign region could pollute captures). The edits
    // merge into the first pass; they sit outside every invocation span, so
    // later rounds see them as inert comments.
    let provenance_edits = if options.provenance {
        provenance_edits(source, &scan)
    } else {
        Vec::new()
    };

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
            let mut message =
                format!("expansion depth exceeded the cap of {DEPTH_CAP} (§8.6); expansion stack:");
            let chain = expansion_stack(&round_scan.invocations);
            for (index, invocation) in chain.iter().enumerate() {
                message.push_str(&format!(
                    "\n  {}. magic({}) at {}..{}",
                    index + 1,
                    invocation.name,
                    invocation.span.start,
                    invocation.span.end
                ));
            }
            return Err(vec![Diagnostic::parse(
                message,
                round_scan.invocations[0].header_span,
            )]);
        }
        // §8.6's depth-first sweep: only the INNERMOST invocations expand in
        // one pass — an invocation whose span contains no other invocation.
        // Siblings at the same nesting depth expand in the same pass, so
        // breadth is unbounded; nesting depth is what the cap counts.
        let innermost: Vec<&InvocationScan> = round_scan
            .invocations
            .iter()
            .filter(|invocation| {
                !round_scan.invocations.iter().any(|other| {
                    !std::ptr::eq(other, *invocation)
                        && other.span.start >= invocation.span.start
                        && other.span.end <= invocation.span.end
                })
            })
            .collect();
        let mut edits: Vec<(Span, String)> = Vec::new();
        if depth == 1 {
            edits.extend(provenance_edits.iter().cloned());
        }
        for invocation in &innermost {
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
            let generated = match &macro_def.entry {
                MacroEntry::RuleRef { bind } => {
                    match match_entry(
                        &set,
                        macro_def.grammar_index,
                        &macro_def.pattern,
                        &region,
                        Some(&engine),
                    ) {
                        Ok(root) => elaborate(
                            &macro_def.template,
                            bind,
                            root,
                            invocation.span,
                            Some(&engine),
                            Some(&region),
                        ),
                        Err(failure) => Err(vec![region_failure_diagnostic(
                            invocation,
                            &failure,
                            region_failure_message(invocation, &failure),
                        )]),
                    }
                }
                MacroEntry::Inline => {
                    match match_entry_binds(
                        &set,
                        macro_def.grammar_index,
                        &macro_def.pattern,
                        &region,
                        Some(&engine),
                    ) {
                        Ok(binds) => elaborate_seeded(
                            &macro_def.template,
                            binds,
                            invocation.span,
                            Some(&engine),
                            Some(&region),
                        ),
                        Err(failure) => Err(vec![region_failure_diagnostic(
                            invocation,
                            &failure,
                            region_failure_message(invocation, &failure),
                        )]),
                    }
                }
            };
            match generated {
                Ok(text) => {
                    // The template's own layout whitespace at the output's
                    // edges is an artifact of the template source, not code
                    // (plan §1.4.9): trim it so an expression-position
                    // invocation sits flush against its context.
                    records.push(ExpansionRecord {
                        magic: invocation.name.clone(),
                        span: invocation.span,
                    });
                    edits.push((invocation.span, text.trim().to_string()));
                }
                Err(errors) => diagnostics.extend(errors),
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

/// Builds the opt-in provenance edits (§8.6): one `// @ magic(name) src:L:C`
/// comment line inserted at the start of the line containing each ROOT
/// invocation (an invocation contained in no other). Positions refer to the
/// ORIGINAL file. A comment inside a foreign region could pollute captures,
/// so nested sites are deliberately not annotated — their root's comment
/// covers the whole site.
fn provenance_edits(source: &str, scan: &MagicScan) -> Vec<(Span, String)> {
    let mut edits = Vec::new();
    for invocation in &scan.invocations {
        let is_root = !scan.invocations.iter().any(|other| {
            !std::ptr::eq(other, invocation)
                && other.span.start <= invocation.span.start
                && other.span.end >= invocation.span.end
                && (other.span.start != invocation.span.start
                    || other.span.end != invocation.span.end)
        });
        if !is_root {
            continue;
        }
        let line_start = source[..invocation.span.start]
            .rfind('\n')
            .map(|offset| offset + 1)
            .unwrap_or(0);
        let (line, column) = line_column(source, invocation.span.start);
        edits.push((
            Span::new(line_start, line_start),
            format!("// @ magic({}) src:{}:{}\n", invocation.name, line, column),
        ));
    }
    edits
}

/// One-based line/column of `offset` in `source` (for provenance comments).
fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let before = &source[..offset.min(source.len())];
    let line = 1 + before.bytes().filter(|byte| *byte == b'\n').count();
    let column = before
        .rsplit(['\n', '\r'])
        .next()
        .map(|tail| tail.chars().count() + 1)
        .unwrap_or(1);
    (line, column)
}

/// Renders a pattern-match failure in the §8.3.9 shape: the message, then the
/// failing line of the EMBEDDED region with a caret at the failure column.
/// The diagnostic's own span still anchors at the original file, so the CLI's
/// caret and the embedded excerpt agree on the same spot.
fn region_failure_message(invocation: &InvocationScan, failure: &MatchFailure) -> String {
    let region = &invocation.region;
    let offset = failure.byte_offset.min(region.len());
    let line_start = region[..offset].rfind('\n').map(|at| at + 1).unwrap_or(0);
    let line_end = region[offset..]
        .find('\n')
        .map(|at| offset + at)
        .unwrap_or(region.len());
    let line_text = region[line_start..line_end].trim_end_matches('\r');
    let line = 1 + region[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    let column = region[line_start..offset].chars().count();
    let mut message = format!("magic pattern did not match: {}", failure.message);
    message.push_str(&format!("\n  ┆ {line_text}"));
    message.push_str(&format!(
        "\n  ┆ {}^ (region line {line}, column {})",
        " ".repeat(column),
        column + 1
    ));
    // §8.6: a failure at (or past) the region's last character, or leftover
    // content after a complete match, is the signature of a mis-read brace;
    // add the scan hint instead of letting the caller guess.
    let leftover = failure.message.contains("leftover content");
    if leftover || failure.byte_offset >= region.len() {
        message.push_str(&format!("\n  scan hint: {REGION_SCAN_HINT}"));
    }
    message
}

/// Builds the anchored diagnostic for a pattern-match failure: the message
/// is rendered against the region, the span anchors in the original file.
fn region_failure_diagnostic(
    invocation: &InvocationScan,
    failure: &MatchFailure,
    message: String,
) -> Diagnostic {
    let at = invocation.region_span.start + failure.byte_offset.min(invocation.region.len());
    Diagnostic::parse(message, Span::new(at, at))
}

/// The §8.1 resolution rule: "a macro must be imported, or declared earlier
/// in the same file, before it is invoked." Single-file megaprograms have no
/// imports, so every invocation in the ORIGINAL source must name a macro
/// whose declaration starts earlier — except invocations authored INSIDE a
/// declaration's template, which elaborate at the declaration's own position
/// (generated text is re-scanned in later rounds and exempt here).
fn check_declaration_order(
    scan: &MagicScan,
    macros: &[CompiledMacro],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let template_authored = |span: Span| {
        scan.magics
            .iter()
            .any(|magic| magic.span.start <= span.start && span.end <= magic.span.end)
    };
    for invocation in &scan.invocations {
        if template_authored(invocation.span) {
            continue;
        }
        if let Some(macro_def) = macros
            .iter()
            .find(|macro_def| macro_def.name == invocation.name)
            && macro_def.decl_span.start > invocation.span.start
        {
            diagnostics.push(Diagnostic::parse(
                format!(
                    "macro `{}` must be declared before it is invoked (§8.1)",
                    invocation.name
                ),
                invocation.header_span,
            ));
        }
    }
}

/// The containment chain of the deepest invocation: outermost first. This is
/// the expansion stack the depth-cap diagnostic reports (§8.6: "the full
/// expansion stack — each macro, span, and pass").
fn expansion_stack(invocations: &[InvocationScan]) -> Vec<&InvocationScan> {
    let Some(innermost) = invocations.iter().min_by_key(|invocation| {
        invocations
            .iter()
            .filter(|other| {
                !std::ptr::eq(*other, *invocation)
                    && other.span.start >= invocation.span.start
                    && other.span.end <= invocation.span.end
            })
            .count()
    }) else {
        return Vec::new();
    };
    let mut chain = vec![innermost];
    while let Some(current) = chain.first() {
        let Some(container) = invocations.iter().find(|other| {
            !std::ptr::eq(*other, *current)
                && other.span.start <= current.span.start
                && other.span.end >= current.span.end
                && (other.span.start != current.span.start || other.span.end != current.span.end)
        }) else {
            break;
        };
        chain.insert(0, container);
        if chain.len() > DEPTH_CAP + 1 {
            break;
        }
    }
    chain
}

/// Applies non-overlapping span edits, highest position first; a tie on the
/// start position applies the WIDER span first, so a zero-width insertion at
/// the same offset (a provenance comment) lands before the replacement that
/// follows it instead of invalidating its coordinates.
fn apply_edits(text: &str, edits: &[(Span, String)]) -> String {
    let mut edits = edits.to_vec();
    edits.sort_by_key(|(span, _)| (std::cmp::Reverse(span.start), std::cmp::Reverse(span.end)));
    let mut out = text.to_string();
    for (span, replacement) in edits {
        out.replace_range(span.start..span.end, &replacement);
    }
    out
}

/// A compiled macro: name, entry pattern, template, entry grammar index,
/// and the entry shape (§8.1): a leading rule reference binds the rule's
/// record under one name; an inline pattern binds its top-level captures
/// individually (`agent.spawn`'s shape).
struct CompiledMacro {
    name: String,
    pattern: Pattern,
    template: cme_core::magic::Template,
    grammar_index: usize,
    entry: MacroEntry,
    /// The declaration's span — the §8.1 resolution rule (a macro must be
    /// declared, or imported, before it is invoked) is checked against it.
    decl_span: Span,
}

enum MacroEntry {
    /// `magic m(grammar.rule as name) { … }` — the matched rule record is
    /// bound to `name` and seeded as the template's root capture.
    RuleRef { bind: String },
    /// `magic m(#annot … pattern …) { … }` — an inline pattern; every
    /// top-level bind seeds a capture of its own.
    Inline,
}

/// Parses every grammar body into compiled rules. `extends` chains (§8.2)
/// are resolved here: the child's EFFECTIVE profile folds the parent chain
/// (an absent `skip` inherits, comment/string forms append), and the child's
/// rule list is its own rules plus inherited ones it does not override —
/// so bare rule references and `recur` inside inherited rules resolve in
/// the child exactly as they would in the parent.
fn compile_grammars(scan: &MagicScan, diagnostics: &mut Vec<Diagnostic>) -> GrammarSet {
    // Parent links first: unknown parents and cycles are compile errors.
    for grammar in &scan.grammars {
        if let Some(parent) = &grammar.extends {
            let known = scan.grammars.iter().any(|other| &other.name == parent);
            if !known {
                diagnostics.push(Diagnostic::parse(
                    format!("unknown grammar `{parent}` in `extends`"),
                    grammar.extends_span.unwrap_or(grammar.span),
                ));
                continue;
            }
            // Cycle check: walk the chain; a grammar that revisits itself
            // (including `extends` itself) can never inherit.
            let mut current = Some(parent.clone());
            let mut steps = 0usize;
            while let Some(next) = current {
                steps += 1;
                if next == grammar.name {
                    diagnostics.push(Diagnostic::parse(
                        format!(
                            "grammar `{}` extends itself (cyclic `extends` chain)",
                            grammar.name
                        ),
                        grammar.extends_span.unwrap_or(grammar.span),
                    ));
                    break;
                }
                if steps > scan.grammars.len() {
                    diagnostics.push(Diagnostic::parse(
                        format!("cyclic `extends` chain reaching grammar `{}`", grammar.name),
                        grammar.extends_span.unwrap_or(grammar.span),
                    ));
                    break;
                }
                current = scan
                    .grammars
                    .iter()
                    .find(|other| other.name == next)
                    .and_then(|other| other.extends.clone());
            }
        }
    }

    let mut set = GrammarSet::default();
    for grammar in &scan.grammars {
        // The effective profile: the extends chain folded root-first.
        let profile = with_default_strings(
            &scan
                .inherited_profile(&grammar.name)
                .unwrap_or_else(|| grammar.profile.clone()),
        );
        let mut compiled = CompiledGrammar {
            name: grammar.name.clone(),
            profile,
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

    // Materialize inheritance: append each grammar's inherited rules (its
    // own wins on a name clash). The chain is walked root-first so a
    // grandparent's rule overridden by the parent never reappears.
    let indices: Vec<usize> = (0..set.grammars.len()).collect();
    for index in indices {
        let mut chain: Vec<String> = Vec::new();
        let mut current = scan.grammars[index].extends.clone();
        while let Some(next) = current {
            if chain.contains(&next) || next == set.grammars[index].name {
                break;
            }
            chain.push(next.clone());
            current = scan
                .grammars
                .iter()
                .find(|other| other.name == next)
                .and_then(|other| other.extends.clone());
        }
        chain.reverse(); // root first
        for ancestor in &chain {
            let inherited: Vec<CompiledRule> = set
                .grammars
                .iter()
                .find(|grammar| &grammar.name == ancestor)
                .map(|grammar| grammar.rules.clone())
                .unwrap_or_default();
            let grammar = &mut set.grammars[index];
            for rule in inherited {
                if grammar.rules.iter().any(|own| own.name == rule.name) {
                    continue; // the child's own rule overrides
                }
                grammar.rules.push(rule);
            }
        }
    }

    // A synthetic default grammar hosts inline entry patterns.
    set.grammars.push(CompiledGrammar {
        name: "\u{0}default".to_string(),
        profile: default_profile(),
        rules: Vec::new(),
    });
    set
}

// ---------------------------------------------------------------------------
// Profile static checks (§8.2, §8.3.2, §8.3.5)
// ---------------------------------------------------------------------------

/// Rejects line-mode machinery in grammars that cannot host it: `eol`,
/// `line`, `soft`, and `indent` are line-mode constructs (§8.3.2, §8.3.5),
/// so a FLOW-oriented grammar (its skip set crosses line terminators) may
/// not use them; and `indent`/`eol`/`line` are contradictory inside `soft`,
/// whose whole job is joining newlines into the skip set for one construct.
fn check_profiles(set: &GrammarSet, diagnostics: &mut Vec<Diagnostic>) {
    for grammar in &set.grammars {
        if grammar.name.starts_with('\u{0}') {
            continue; // the synthetic inline-pattern host has no rules
        }
        let flow = grammar.profile.is_flow_oriented();
        for rule in &grammar.rules {
            check_pattern_profiles(&rule.pattern, &grammar.name, flow, false, diagnostics);
        }
    }
}

/// Walks one pattern for the profile checks. `in_soft` marks patterns under
/// a `soft` region (the soft switch is per construct, not global).
fn check_pattern_profiles(
    pattern: &Pattern,
    grammar: &str,
    flow: bool,
    in_soft: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for elem in &pattern.elems {
        let line_mode_name = match &elem.kind {
            PatKind::Eol => Some("eol"),
            PatKind::Line => Some("line"),
            PatKind::Indent { .. } => Some("indent"),
            _ => None,
        };
        if let Some(name) = line_mode_name {
            if in_soft {
                diagnostics.push(Diagnostic::parse(
                    format!(
                        "`{name}` inside `soft` is rejected: soft joins newlines into the \
                         skip set, the line machinery depends on them staying separate"
                    ),
                    elem.span,
                ));
                continue;
            }
            if flow {
                diagnostics.push(Diagnostic::parse(
                    format!(
                        "line-mode element `{name}` in flow grammar `{grammar}`: the skip set \
                         crosses line terminators, so `{name}` has no line to act on (§8.3.2)"
                    ),
                    elem.span,
                ));
                continue;
            }
        }
        if flow && !in_soft && matches!(elem.kind, PatKind::Soft(_)) {
            diagnostics.push(Diagnostic::parse(
                format!(
                    "`soft` in flow grammar `{grammar}`: newlines already join the skip set, \
                     so soft is a no-op there (§8.3.2)"
                ),
                elem.span,
            ));
        }
        match &elem.kind {
            PatKind::Soft(body)
            | PatKind::Raw(body)
            | PatKind::Label { body, .. }
            | PatKind::Group { body, .. }
            | PatKind::Peek { body, .. }
            | PatKind::Until { stop: body, .. } => {
                let soft = matches!(&elem.kind, PatKind::Soft(_));
                check_pattern_profiles(body, grammar, flow, in_soft || soft, diagnostics);
            }
            PatKind::Optional { body, .. } => {
                check_pattern_profiles(body, grammar, flow, in_soft, diagnostics);
            }
            PatKind::Each { sep, body, .. } => {
                if let Some(sep) = sep {
                    check_pattern_profiles(sep, grammar, flow, in_soft, diagnostics);
                }
                check_pattern_profiles(body, grammar, flow, in_soft, diagnostics);
            }
            PatKind::OneOf { branches, .. } => {
                for (_, branch) in branches {
                    check_pattern_profiles(branch, grammar, flow, in_soft, diagnostics);
                }
            }
            PatKind::Indent {
                body: Some(body), ..
            } => {
                check_pattern_profiles(body, grammar, flow, in_soft, diagnostics);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Static left-recursion rejection (§8.7)
// ---------------------------------------------------------------------------

/// Rejects left recursion — including nullable-prefix cycles: a rule
/// reaching itself while consuming nothing through `optional`, empty
/// iterations, `peek`/`not`, `where`, or `label` — with a rewrite hint
/// (§8.7). The runtime memo cycle cut stays as the backstop; this static
/// pass catches the common shapes before any region is matched.
fn reject_left_recursion(set: &GrammarSet, diagnostics: &mut Vec<Diagnostic>) {
    // Nullability fixpoint: nullable[gi][ri] — can the rule match zero
    // characters? Monotone (false → true only), so the loop terminates.
    let mut nullable: Vec<Vec<bool>> = set
        .grammars
        .iter()
        .map(|grammar| vec![false; grammar.rules.len()])
        .collect();
    loop {
        let mut changed = false;
        for (gi, grammar) in set.grammars.iter().enumerate() {
            for (ri, rule) in grammar.rules.iter().enumerate() {
                let value = pattern_nullable(&rule.pattern, &nullable, gi);
                if value != nullable[gi][ri] {
                    nullable[gi][ri] = value;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Prefix-edge DFS: rule R can start with rule S at zero consumed input.
    // A cycle in that graph is left recursion.
    let mut color: Vec<Vec<u8>> = set
        .grammars
        .iter()
        .map(|grammar| vec![0u8; grammar.rules.len()])
        .collect();
    let mut chain: Vec<String> = Vec::new();

    for gi in 0..set.grammars.len() {
        for ri in 0..set.grammars[gi].rules.len() {
            if color[gi][ri] == 0 {
                visit_rule(set, gi, ri, &nullable, &mut color, &mut chain, diagnostics);
            }
        }
    }
}

/// DFS over prefix edges, reporting the first cycle found with the full
/// chain and a rewrite hint.
fn visit_rule(
    set: &GrammarSet,
    gi: usize,
    ri: usize,
    nullable: &[Vec<bool>],
    color: &mut [Vec<u8>],
    chain: &mut Vec<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if color[gi][ri] == 2 {
        return;
    }
    if color[gi][ri] == 1 {
        // Cycle: render the chain from the first occurrence of this rule.
        let head = format!(
            "{}.{}",
            set.grammars[gi].name, set.grammars[gi].rules[ri].name
        );
        let start = chain.iter().position(|step| *step == head).unwrap_or(0);
        let cycle = chain[start..].join(" -> ");
        let rule = &set.grammars[gi].rules[ri];
        let span = rule
            .pattern
            .elems
            .first()
            .map(|element| element.span)
            .unwrap_or(cme_core::Span::missing(0));
        diagnostics.push(Diagnostic::parse(
            format!(
                "left recursion: rule `{}` can reach itself without consuming input (cycle: {}); \
                 rewrite the rule so the recursion consumes input first - match a literal or \
                 fragment before the recursive reference, or make the recursive alternative \
                 reachable only after input is consumed (§8.7)",
                rule.name, cycle
            ),
            span,
        ));
        color[gi][ri] = 2;
        return;
    }
    color[gi][ri] = 1;
    chain.push(set.grammars[gi].rules[ri].name.clone());
    let pattern = set.grammars[gi].rules[ri].pattern.clone();
    prefix_edges(
        set,
        &pattern,
        gi,
        ri,
        nullable,
        &mut |target_gi, target_ri| {
            visit_rule(
                set,
                target_gi,
                target_ri,
                nullable,
                color,
                chain,
                diagnostics,
            );
        },
    );
    chain.pop();
    color[gi][ri] = 2;
}

/// Collects the rule references reachable from `pattern` at zero consumed
/// input, invoking `emit` for each. The walk continues past an element only
/// when that element can consume nothing (§8.3.7's statelessness makes this
/// a pure function of the pattern and the nullability table).
fn prefix_edges(
    set: &GrammarSet,
    pattern: &Pattern,
    gi: usize,
    ri: usize,
    nullable: &[Vec<bool>],
    emit: &mut dyn FnMut(usize, usize),
) {
    for elem in &pattern.elems {
        if prefix_elem(set, &elem.kind, gi, ri, nullable, emit) {
            // The element may consume input: anything after it is no longer
            // a zero-input prefix.
            return;
        }
    }
}

/// Handles one element in the zero-input prefix walk. Returns true when the
/// element may consume input (the walk stops), false when the walk continues
/// to the next element at the same position.
fn prefix_elem(
    set: &GrammarSet,
    kind: &PatKind,
    gi: usize,
    ri: usize,
    nullable: &[Vec<bool>],
    emit: &mut dyn FnMut(usize, usize),
) -> bool {
    match kind {
        // Zero-width elements: no input, no refs — the walk continues.
        PatKind::Where { .. }
        | PatKind::Line
        | PatKind::Peek { .. }
        | PatKind::Annotation { .. } => false,
        PatKind::Label { body, .. } => {
            prefix_edges(set, body, gi, ri, nullable, emit);
            false
        }
        // Always-nullable wrappers whose bodies still contribute prefix refs.
        PatKind::Optional { body, .. } => {
            prefix_edges(set, body, gi, ri, nullable, emit);
            false
        }
        PatKind::Indent { body, .. } => {
            // An indent block can match empty (no lines), so the walk
            // continues; the body's refs are reached through iterations,
            // which still begin at zero consumed input for the rule.
            if let Some(body) = body {
                prefix_edges(set, body, gi, ri, nullable, emit);
            }
            false
        }
        PatKind::Soft(body) | PatKind::Raw(body) | PatKind::Group { body, .. } => {
            prefix_edges(set, body, gi, ri, nullable, emit);
            pattern_nullable(body, nullable, gi)
        }
        PatKind::Each { body, .. } => {
            // The first iteration begins at zero consumed input.
            prefix_edges(set, body, gi, ri, nullable, emit);
            pattern_nullable(body, nullable, gi)
        }
        PatKind::OneOf { branches, .. } => {
            let mut any_nullable = false;
            for (_, branch) in branches {
                prefix_edges(set, branch, gi, ri, nullable, emit);
                any_nullable |= pattern_nullable(branch, nullable, gi);
            }
            any_nullable
        }
        PatKind::RuleRef { path, .. } => match resolve_in_set(set, path, gi) {
            Some((target_gi, target_ri)) => {
                let target_nullable = nullable[target_gi][target_ri];
                emit(target_gi, target_ri);
                target_nullable
            }
            // Unresolvable refs fail at match time; assume consuming.
            None => true,
        },
        PatKind::Recur => {
            // `recur` re-enters the enclosing rule; the walk continues past
            // it only when that rule itself is nullable.
            emit(gi, ri);
            nullable[gi][ri]
        }
        // Everything else consumes at least one character in practice
        // (`until`/`lineRest` can match empty only at degenerate positions;
        // the runtime cycle cut covers those).
        _ => true,
    }
}

/// Resolves a rule path for the static walk (bare names resolve inside their
/// own grammar; qualified names across grammars) — the same discipline as
/// the matcher's `resolve`.
fn resolve_in_set(set: &GrammarSet, path: &[String], gi: usize) -> Option<(usize, usize)> {
    let (grammar_index, rule_name) = match path.len() {
        1 => (gi, path[0].as_str()),
        2 => {
            let index = set
                .grammars
                .iter()
                .position(|grammar| grammar.name == path[0])?;
            (index, path[1].as_str())
        }
        _ => return None,
    };
    let rule_index = set.grammars[grammar_index]
        .rules
        .iter()
        .position(|rule| rule.name == rule_name)?;
    Some((grammar_index, rule_index))
}

/// Whether a pattern can match zero characters (nullability fixpoint step).
fn pattern_nullable(pattern: &Pattern, nullable: &[Vec<bool>], gi: usize) -> bool {
    pattern
        .elems
        .iter()
        .all(|elem| elem_nullable(&elem.kind, nullable, gi))
}

fn elem_nullable(kind: &PatKind, nullable: &[Vec<bool>], gi: usize) -> bool {
    match kind {
        PatKind::Lit { .. }
        | PatKind::Class { .. }
        | PatKind::Any { .. }
        | PatKind::Scan { .. }
        | PatKind::Eol
        | PatKind::Eof
        | PatKind::Fragment { .. }
        | PatKind::RuleRef { .. }
        | PatKind::Recur
        | PatKind::Annotation { .. } => false,
        PatKind::LineRest { .. } | PatKind::Until { .. } => true,
        PatKind::Line | PatKind::Peek { .. } | PatKind::Where { .. } => true,
        PatKind::Optional { .. } | PatKind::Indent { .. } => true,
        PatKind::Soft(body) | PatKind::Raw(body) | PatKind::Label { body, .. } => {
            pattern_nullable(body, nullable, gi)
        }
        PatKind::Group { body, .. } => pattern_nullable(body, nullable, gi),
        PatKind::Each {
            body, bounds, plus, ..
        } => {
            let min = match bounds {
                Some((min, _)) => *min,
                None => 0,
            };
            min == 0 && !*plus || pattern_nullable(body, nullable, gi)
        }
        PatKind::OneOf { branches, .. } => branches
            .iter()
            .any(|(_, branch)| pattern_nullable(branch, nullable, gi)),
    }
}

/// True when the pattern contains a `$raw`/`$expr`/`$type`/`$block` capture
/// with no effective tail anywhere after it (§8.3.6's compile-time error).
/// The walk mirrors the matcher's continuation structure: each-body tails
/// are always live (the repetition offers another iteration), and every
/// other construct passes down whether anything follows it.
fn pattern_has_untailed_code_fragment(pattern: &Pattern) -> bool {
    fn walk(pattern: &Pattern, inherited_tail: bool) -> bool {
        let elems = &pattern.elems;
        for (index, elem) in elems.iter().enumerate() {
            let has_tail = index + 1 < elems.len() || inherited_tail;
            match &elem.kind {
                PatKind::Fragment { kind, .. }
                    if matches!(
                        kind,
                        FragKind::Raw(_) | FragKind::Expr | FragKind::Type | FragKind::Block
                    ) && !has_tail =>
                {
                    return true;
                }
                PatKind::Soft(body)
                | PatKind::Raw(body)
                | PatKind::Label { body, .. }
                | PatKind::Group { body, .. }
                | PatKind::Optional { body, .. }
                | PatKind::Peek { body, .. }
                | PatKind::Until { stop: body, .. } => {
                    if walk(body, has_tail) {
                        return true;
                    }
                }
                PatKind::Each { body, .. } => {
                    // Inside an each body the repetition itself is a tail.
                    if walk(body, true) {
                        return true;
                    }
                }
                PatKind::OneOf { branches, .. } => {
                    for (_, branch) in branches {
                        if walk(branch, has_tail) {
                            return true;
                        }
                    }
                }
                PatKind::Indent {
                    body: Some(body), ..
                } if walk(body, has_tail) => {
                    return true;
                }
                _ => {}
            }
        }
        false
    }
    // The entry pattern's own continuation is End: nothing follows it.
    walk(pattern, false)
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
        // §8.3.6's compile-time error: a code fragment with an empty tail
        // (nothing follows it anywhere in the pattern) has no boundary to
        // stop at. Magic entry patterns are fully known continuations
        // (End), so the check is exact here; rule bodies receive their
        // caller's continuation at match time and are exempt.
        if pattern_has_untailed_code_fragment(&pattern) {
            diagnostics.push(Diagnostic::parse(
                "raw capture requires a following terminator; use $text or until (§8.3.6)",
                magic.pattern_span,
            ));
            continue;
        }
        // Entry shape from the pattern's first element: a leading rule
        // reference makes a rule-record entry; anything else is an INLINE
        // pattern (§8.1's agent.spawn) hosted by the synthetic default
        // grammar under the default profile.
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
            // The entry pattern's elements after the leading rule ref run
            // under the ENTRY grammar's orientation (the rule ref delegates
            // to its own grammar): the profile checks apply to them too.
            let entry = &set.grammars[grammar_index];
            let flow = entry.profile.is_flow_oriented();
            for elem in pattern.elems.iter().skip(1) {
                let one = Pattern {
                    elems: vec![elem.clone()],
                };
                check_pattern_profiles(&one, &entry.name, flow, false, diagnostics);
            }
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
                entry: MacroEntry::RuleRef { bind: bind_name },
                decl_span: magic.span,
            });
        } else {
            // Inline pattern (§8.1): literals, fragments, and QUALIFIED rule
            // references under the default profile (horizontal and newline
            // skipping, `"` strings, no comments — §8.2). One-segment rule
            // references have no grammar namespace to resolve in; the
            // profile check runs flow-oriented.
            let default_index = set
                .grammars
                .iter()
                .position(|grammar| grammar.name == "\u{0}default")
                .expect("the synthetic default grammar is always compiled");
            for elem in &pattern.elems {
                check_pattern_profiles(
                    &Pattern {
                        elems: vec![elem.clone()],
                    },
                    "<inline pattern>",
                    true,
                    false,
                    diagnostics,
                );
            }
            macros.push(CompiledMacro {
                name: magic.name.clone(),
                pattern,
                template,
                grammar_index: default_index,
                entry: MacroEntry::Inline,
                decl_span: magic.span,
            });
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

    // -- Task 4: static left-recursion rejection (§8.7) ----------------------

    fn expansion_errors(source: &str) -> Vec<String> {
        expand_source(source)
            .expect_err("expected expansion diagnostics")
            .iter()
            .map(|error| error.message().to_string())
            .collect()
    }

    #[test]
    fn direct_left_recursion_is_rejected_with_a_hint() {
        let source = r#"
grammar bad {
    rule value {
        oneof { word => $word w, loop => value }
    }
}

magic runIt(bad.value as v) {
    "x"
}

magic(runIt) {
    hello
}
"#;
        let errors = expansion_errors(source);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("left recursion")),
            "expected a left-recursion diagnostic: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|message| message.contains("rewrite the rule")),
            "expected a rewrite hint: {errors:?}"
        );
    }

    #[test]
    fn nullable_prefix_cycle_is_rejected() {
        // p can start with q at zero input (optional), q can start with p
        // the same way: a nullable-prefix cycle through two rules.
        let source = r#"
grammar bad {
    rule p {
        optional { q } "!"
    }

    rule q {
        optional { p } "?"
    }
}

magic runIt(bad.p as v) {
    "x"
}

magic(runIt) {
    !
}
"#;
        let errors = expansion_errors(source);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("left recursion") && message.contains("p")),
            "expected a left-recursion diagnostic naming p: {errors:?}"
        );
    }

    #[test]
    fn legitimate_recursion_still_expands() {
        // The JSON value grammar is recursive but every recursive path
        // consumes a bracket first: it must pass the static check.
        let source = r#"
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]

    rule value {
        oneof {
            null   => "null"
            string => $str text
            array  => ( "[" each sep "," { value } as items "]" )
        }
    }
}

magic jsonValue(json.value as v) {
    match ($v) {
        null   => "null"
        string => $v.text
        array  => "[]"
    }
}

str config = magic(jsonValue) {
    "db.local"
}
"#;
        let outcome = expand_source(source).expect("legitimate recursion must expand");
        assert!(outcome.expanded.contains("\"db.local\""));
    }

    // -- Task 6: parse-integrated extents (§8.3.6) ----------------------------

    #[test]
    fn py_conditions_with_nested_calls_expand_and_check() {
        // §8.3.6's flagship example: `useTwo(v, lo) > 0` must capture as ONE
        // condition — the comma inside the call must not end the `$expr`
        // extent (the boundary is parse-integrated), and the `:` + indent
        // tail stops it exactly where Python's grammar says.
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

int useTwo(int a, int b) {
    return a
}

magic(def) {
    def pyNestedFn(v: int, lo: int) -> int:
        if useTwo(v, lo) > 0:
            return lo
        return useTwo(v, v)
}
"##;
        let outcome = expand_source(source).expect("nested-call py def must expand");
        let normalized: String = outcome
            .expanded
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(normalized.contains("if (useTwo(v, lo) > 0) {"));
        assert!(normalized.contains("return useTwo(v, v)"));

        let parsed = crate::parse_source(&outcome.expanded);
        assert!(
            parsed.diagnostics.is_empty(),
            "expanded program must parse: {:?}",
            parsed
                .diagnostics
                .iter()
                .map(|error| error.message().to_string())
                .collect::<Vec<_>>()
        );
        let type_errors = crate::check::check(&parsed.statements);
        assert!(
            type_errors.is_empty(),
            "expanded program must type-check: {:?}",
            type_errors
                .iter()
                .map(|error| error.message().to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn untailed_code_fragments_are_a_compile_time_error() {
        // §8.3.6: a `$raw` with an empty tail has no boundary to stop at.
        // The entry pattern's continuation is fully known (End), so the
        // static check rejects it before any region is matched.
        let source = r#"
grammar test {
    rule thing {
        scan [a-z] as w
    }
}

magic runIt(test.thing as t $raw x) {
    "x"
}

magic(runIt) {
    hello
}
"#;
        let errors = expansion_errors(source);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("requires a following terminator")),
            "expected the untailed-fragment diagnostic: {errors:?}"
        );
    }

    // -- §8.4: where filters on template [each] ------------------------------

    #[test]
    fn template_each_supports_where_filters() {
        // §8.4: `[each in xs where cond { … }]` skips items whose condition
        // is not truthy; the join consumes no slot for them.
        let source = r#"
grammar list {
    skip [ ' ', '\t', '\r', '\n' ]
    rule doc {
        each sep "," { $word name "=" $word flag } as items
    }
}

magic keepers(list.doc as d) {
    [
        [each in $d.items where $item.flag == "yes" { $item.name }]
    ]
}

str[] v = magic(keepers) {
    a = yes, b = no, c = yes
}
"#;
        let outcome = expand_source(source).expect("filtered each expands");
        assert!(
            outcome.expanded.contains("a, c") && !outcome.expanded.contains(", b"),
            "only items passing the filter join the list: {}",
            outcome.expanded
        );
    }

    #[test]
    fn tailed_code_fragments_in_entry_patterns_are_fine() {
        let source = r#"
grammar test {
    rule thing {
        scan [a-z] as w
    }
}

magic runIt(test.thing as t $raw x "!") {
    $"{$t.x}"
}

magic(runIt) {
    hello world!
}
"#;
        let outcome = expand_source(source).expect("tailed $raw must expand");
        // The $raw capture ("world") splices into the interpolation; the
        // rule capture ("hello") was consumed but never spliced.
        assert!(
            outcome.expanded.contains("\"world\""),
            "the $raw text splices"
        );
    }

    // -- §8.1: inert annotations, inline entry patterns, resolution order -----

    #[test]
    fn annotations_are_inert_between_pattern_terms() {
        // The §8.1 agent.spawn shape: #complete/#hover/#token appear between
        // the pattern's terms and never affect matching.
        let source = r#"
magic agent.spawn(
    #complete(engine.availableModels)
    #hover("Target model identifier")
    "model:" $tag model
    "effort:" $word effort
    #token("prompt")
    $text prompt
) {
    $"{$model}|{$effort}|{$prompt}"
}

str cfg = magic(agent.spawn) {
    model: claude-opus-latest
    effort: high
    patrol the routes
}
"#;
        let outcome = expand_source(source).expect("annotated inline pattern expands");
        assert!(
            outcome
                .expanded
                .contains("\"claude-opus-latest|high|patrol the routes\""),
            "the binds splice: {}",
            outcome.expanded
        );
    }

    #[test]
    fn unknown_annotations_are_rejected() {
        let source = r#"
magic m(#complete2(x) "a") {
    "b"
}

str v = magic(m) {
    a
}
"#;
        let errors = expansion_errors(source);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("unknown annotation")),
            "expected the unknown-annotation diagnostic: {errors:?}"
        );
    }

    #[test]
    fn inline_entry_patterns_expand_without_a_rule_ref() {
        // Pure inline pattern: literals + fragments, no grammar at all.
        let source = r#"
magic pair("a =" $word left "b =" $word right) {
    infer both = $"{$left}-{$right}"
    both
}

str v = magic(pair) {
    a = one b = two
}
"#;
        let outcome = expand_source(source).expect("inline pattern expands");
        assert!(
            outcome.expanded.contains("\"one-two\""),
            "both binds splice: {}",
            outcome.expanded
        );
    }

    #[test]
    fn macros_must_be_declared_before_they_are_invoked() {
        let source = r#"
str v = magic(later) {
    x
}

magic later($word w) {
    $"{$w}!"
}
"#;
        let errors = expansion_errors(source);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("must be declared before it is invoked")),
            "expected the §8.1 order diagnostic: {errors:?}"
        );
    }

    #[test]
    fn invocations_inside_a_template_are_exempt_from_the_order_check() {
        // The nested `magic(inner)` sits inside `outer`'s TEMPLATE (a
        // declaration span), so the §8.1 source-order rule does not apply;
        // it expands when `outer` runs.
        let source = r#"
magic outer($word w) {
    magic(inner) {
        inner text
    }
}

magic inner($text t) {
    $"[{$t}]"
}

str v = magic(outer) {
    hello
}
"#;
        let outcome = expand_source(source).expect("template-nested invocation expands");
        assert!(
            outcome.expanded.contains("[inner text]"),
            "the nested magic expanded: {}",
            outcome.expanded
        );
    }

    // -- §8.3.3: function validators; §8.3.4: the .span accessor -------------

    #[test]
    fn fragment_validators_may_name_pure_functions() {
        let source = r#"
bool isValidTag(str t) {
    return t == "on" || t == "off"
}

magic flip($word<isValidTag> state) {
    $"{$state}!"
}

str v = magic(flip) {
    on
}
"#;
        let outcome = expand_source(source).expect("a function validator accepts `on`");
        assert!(outcome.expanded.contains("\"on!\""), "{}", outcome.expanded);

        let rejected = r#"
bool isValidTag(str t) {
    return t == "on" || t == "off"
}

magic flip($word<isValidTag> state) {
    $"{$state}!"
}

str v = magic(flip) {
    sideways
}
"#;
        let errors = expansion_errors(rejected);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("validator `isValidTag` rejected")),
            "the function validator rejects non-members: {errors:?}"
        );
    }

    #[test]
    fn span_accessor_compares_for_identity() {
        // §8.3.4's capture accessors include `.span`; two captures over the
        // same extent share one span text, distinct extents differ.
        let source = r#"
magic twin($word as a $word as b where a.span != b.span) {
    "spans differ"
}

str v = magic(twin) {
    one two
}
"#;
        let outcome = expand_source(source).expect("distinct spans compare");
        assert!(
            outcome.expanded.contains("spans differ"),
            "{}",
            outcome.expanded
        );

        let same = r#"
magic twin($word as a $word as b where a.span == b.span) {
    "same"
}

str v = magic(twin) {
    echo
}
"#;
        let errors = expansion_errors(same);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("magic pattern did not match")),
            "a single word cannot bind twice, so the where never passes: {errors:?}"
        );
    }

    // -- §8.3.4: context accumulation via append steers matching -------------

    #[test]
    fn nested_duplicate_tags_are_rejected_through_context() {
        // §8.3.4's third row: the open-element list accumulated downward
        // through `context` STEERS matching — a nested duplicate tag makes
        // the `where` fail, and no alternative saves the branch.
        let source = r#"
grammar nest {
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
magic nestTree(nest.doc as d) {
    [ [each in $d.items { $"{$item.name}" }] ]
}
str[] v = magic(nestTree) {
    <section>
        <section>
            deep
        </section>
    </section>
}
"#;
        let errors = expansion_errors(source);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("magic pattern did not match")),
            "a nested duplicate tag must fail the match: {errors:?}"
        );
    }

    #[test]
    fn context_accumulation_accepts_distinct_nesting() {
        let source = r#"
grammar nest {
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
magic nestTree(nest.doc as d) {
    [ [each in $d.items { $"{$item.name}[{$item.children.length}]" }] ]
}
str[] v = magic(nestTree) {
    <section>
        <subsection>
            deep
        </subsection>
    </section>
    <div></div>
}
"#;
        let outcome = expand_source(source).expect("distinct nesting passes the context guard");
        assert!(
            outcome.expanded.contains("\"section[1]\"") && outcome.expanded.contains("\"div[0]\""),
            "{}",
            outcome.expanded
        );
    }

    #[test]
    fn duplicate_toml_table_headers_are_rejected_by_the_require() {
        let source = r##"
grammar toml {
    skip    [ ' ', '\t' ]
    comment ( "#" )
    rule document {
        each { oneof { table => tableHeader, kv => keyval } } as items
    }
    rule tableHeader {
        oneof {
            arrayTable => ( "[[" dottedKey path "]]" eol )
            table      => ( "[" dottedKey path "]" eol )
        }
    }
    rule keyval {
        dottedKey key "=" value as val eol
    }
    rule dottedKey {
        each sep "." {
            oneof { bare => scan [A-Za-z0-9_-] as part, quoted => $str part }
        } as parts
    }
    rule value {
        oneof {
            basicStr => $str text
            integer  => ( optional { "-" } scan [0-9] as digits )
        }
    }
}

bool tablesConsistent(Capture doc) {
    match (doc) {
        Rec(str tag, map<str, Capture> fields) => {
            match (fields["items"]) {
                List(Capture[] items) => {
                    int seen = 0
                    for (Capture item in items) {
                        match (item) {
                            Rec(str itag, map<str, Capture> f) => {
                                if (itag == "table") { seen += 1 }
                            }
                            _ => {}
                        }
                    }
                    return seen < 2
                }
                _ => { return true }
            }
        }
        _ => { return true }
    }
}

magic tomlValue(toml.document as doc) {
    require(@tablesConsistent($doc), "table redefined or reopened with a conflicting type")
    "obj"
}

str v = magic(tomlValue) {
    [a]
    x = 1

    [a]
    y = 2
}
"##;
        let errors = expansion_errors(source);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("table redefined")),
            "the require anchors at the capture: {errors:?}"
        );
    }

    #[test]
    fn context_fields_accept_array_types() {
        // §8.3.4's `context { str[] open }` — the array type suffix used to
        // send the field parser into a zero-progress loop.
        let source = r#"
grammar nest {
    skip [ ' ', '\t', '\r', '\n' ]
    rule doc {
        each { element } as items
        eof
    }
    rule element(context { str[] open = none, int depth = 0 }) {
        "<" $word name ">"
        each { element with context { open: append(open, name), depth: 1 } } as children
        "</" $word close ">"
    }
}
magic nestTree(nest.doc as d) {
    [ [each in $d.items { $"{$item.name}" }] ]
}
str[] v = magic(nestTree) {
    <div></div>
}
"#;
        let outcome = expand_source(source).expect("array-typed context fields parse");
        assert!(outcome.expanded.contains("\"div\""), "{}", outcome.expanded);
    }

    #[test]
    fn short_circuit_or_protects_a_quantifier_over_an_absent_list() {
        // §A.5's short-circuiting carries into `where`: `!present(open) ||`
        // is true at the top level, so the `some … in` over the absent list
        // is never evaluated (a strict evaluation would fail the guard).
        let source = r#"
grammar nest {
    skip [ ' ', '\t', '\r', '\n' ]
    rule doc {
        each { element } as items
        eof
    }
    rule element(context { str[] open = none }) {
        "<" $word name ">"
        where !present(open) || !some x in open { x == name }
        each { element with context { open: append(open, name) } } as children
        "</" $word close ">"
        where close == name
    }
}
magic nestTree(nest.doc as d) {
    [ [each in $d.items { $"{$item.name}" }] ]
}
str[] v = magic(nestTree) {
    <div></div>
}
"#;
        let outcome = expand_source(source).expect("top-level guard passes");
        assert!(outcome.expanded.contains("\"div\""), "{}", outcome.expanded);
    }

    // -- §8.3.3: the parameterized $tt fragment ------------------------------

    #[test]
    fn parameterized_tt_balances_the_given_delimiters() {
        // $tt<"{{" "}}"> roots the balanced tree at the explicit pair;
        // braces inside (even in strings) stay interior to the tree.
        let source = r#"
magic cell($word key "=" $tt<"{{" "}}"> value) {
    $"{$key}={$value.matched}"
}

str v = magic(cell) {
    alpha = {{ f(1, { x: 2 }) + g("}}") }}
}
"#;
        let outcome = expand_source(source).expect("the explicit tree balances");
        // The emitted Checkmate literal escapes the inner quotes, so the
        // tree text appears with \" inside it.
        assert!(
            outcome
                .expanded
                .contains(r#"alpha={{ f(1, { x: 2 }) + g(\"}}\") }}"#),
            "the whole tree is one capture: {}",
            outcome.expanded
        );
    }

    // -- Task 10: diagnostics & provenance polish (§8.3.9, §8.6) --------------

    #[test]
    fn pattern_failures_render_the_embedded_source_caret() {
        // The member pattern needs `:` between key and value; the region's
        // second line lacks it, so the furthest failure sits there.
        let source = r#"
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]

    rule value {
        oneof {
            string => $str text
            object => ( "{" each sep "," { member } as fields "}" )
        }
    }

    rule member {
        $str key ":" value as val
    }
}

magic jsonValue(json.value as v) {
    "x"
}

str config = magic(jsonValue) {
    {
        "host" "db.local",
        "retries": 3
    }
}
"#;
        let errors = expansion_errors(source);
        let failure = errors
            .iter()
            .find(|message| message.contains("magic pattern did not match"))
            .expect("a pattern-failure diagnostic");
        // §8.3.9 shape: the embedded region line (interior indentation is
        // verbatim), and the caret annotated with region-relative coords.
        assert!(
            failure.contains("┆         \"host\" \"db.local\","),
            "expected the failing region line in the message: {failure}"
        );
        assert!(
            failure.contains("^ (region line 2, column 16)"),
            "expected the caret annotation: {failure}"
        );
    }

    #[test]
    fn leftover_content_and_region_edge_failures_carry_the_scan_hint() {
        let source = r#"
grammar test {
    rule thing { $word w }
}

magic runIt(test.thing as t) {
    "x"
}

str s = magic(runIt) {
    hello world
}
"#;
        let errors = expansion_errors(source);
        let failure = errors
            .iter()
            .find(|message| message.contains("magic pattern did not match"))
            .expect("a leftover-content failure");
        assert!(
            failure.contains("leftover content"),
            "expected the leftover diagnosis: {failure}"
        );
        assert!(
            failure.contains("scan hint:") && failure.contains("heredoc"),
            "expected the §8.6 region-scan hint: {failure}"
        );
    }

    #[test]
    fn provenance_comments_annotate_root_sites_in_the_original_file() {
        let source = r#"
grammar test {
    rule thing { $word w }
}

magic runIt(test.thing as t) {
    "x"
}

str a = magic(runIt) {
    alpha
}

str b = magic(runIt) { beta }
"#;
        let with_provenance =
            expand_source_with(source, ExpandOptions { provenance: true }).expect("expands");
        // Line 10 is `str a = magic(runIt) {`, line 14 is `str b = …`; both
        // roots are annotated with their ORIGINAL-file position (`magic`
        // starts at column 9 in both).
        assert!(
            with_provenance
                .expanded
                .contains("// @ magic(runIt) src:10:9"),
            "expected the root-site comment for `a`: {}",
            with_provenance.expanded
        );
        assert!(
            with_provenance
                .expanded
                .contains("// @ magic(runIt) src:14:9"),
            "expected the root-site comment for `b`: {}",
            with_provenance.expanded
        );

        // Default output is byte-deterministic and carries no annotations.
        let plain = expand_source(source).expect("expands");
        let stripped = with_provenance
            .expanded
            .replace("// @ magic(runIt) src:10:9\n", "")
            .replace("// @ magic(runIt) src:14:9\n", "");
        assert_eq!(plain.expanded, stripped);
        assert!(!plain.expanded.contains("// @ magic("));
    }

    #[test]
    fn provenance_covers_only_the_roots_not_nested_sites() {
        // The inner `magic(b)` is plain foreign text (no island): only the
        // outer invocation is a site, so exactly one comment appears and the
        // region text is never polluted.
        let source = r#"
grammar js {
    skip [ ' ', '\t' ]

    rule run { $text body }
}

magic a(js.run as r) {
    "A"
}

str s = magic(a) {
    magic(b) { 1 }
}
"#;
        let with_provenance =
            expand_source_with(source, ExpandOptions { provenance: true }).expect("expands");
        assert_eq!(
            with_provenance.expanded.matches("// @ magic(").count(),
            1,
            "only the root site is annotated: {}",
            with_provenance.expanded
        );
        // The generated code itself still expanded around the comment.
        assert!(with_provenance.expanded.contains("\"A\""));
    }

    #[test]
    fn require_failures_anchor_at_the_referenced_capture() {
        let source = r#"
grammar test {
    rule thing { $word w }
}

magic runIt(test.thing as t) {
    require($t.w == "nope", "wrong word")
    "ok"
}

str s = magic(runIt) {
    hello
}
"#;
        let errors = expand_source(source).expect_err("require must fail");
        let require_error = errors
            .iter()
            .find(|error| error.message().contains("require failed"))
            .expect("a require diagnostic");
        // The anchor is the matched word `hello` inside the region — the
        // user's source — not the template or the invocation header.
        let anchor = source.find("hello").expect("region text in source");
        assert_eq!(require_error.span().start, anchor);
    }

    // -- Task 11: grammar extension + profile validation (§8.2) ---------------

    /// A js/ts pair: `ts` extends `js`, overrides `stmt`, adds a `decl`
    /// branch, and appends a `'` string form to the inherited profile.
    const TS_EXTENDS_JS: &str = r#"
grammar js {
    skip    [ ' ' ]
    string  ( '"' )
    comment ( "//" )

    rule program { each { stmt } as body }

    rule stmt {
        oneof {
            call => ( $word callee "(" ")" eol )
            ret  => ( "return" $word value eol )
        }
    }
}

grammar ts extends js {
    string  ( '\'' )

    rule stmt {
        oneof {
            call => ( $word callee "(" ")" eol )
            ret  => ( "return" $word value eol )
            decl => ( "let" $word name "=" $int init eol )
        }
    }
}

magic runJs(js.program as p) {
    "js"
}

magic runTs(ts.program as p) {
    "ts"
}

str a = magic(runJs) {
    return done
}

str b = magic(runTs) {
    return done
}

str c = magic(runTs) {
    let n = 7
}
"#;

    #[test]
    fn ts_extends_js_inherits_overrides_and_adds_rules() {
        let outcome = expand_source(TS_EXTENDS_JS).expect("ts extends js must expand");
        // `return done` matches through BOTH grammars: js's own `stmt` and
        // ts's overridden `stmt` (with the added `decl` branch).
        assert!(outcome.expanded.contains("\"js\""));
        assert_eq!(outcome.records.len(), 3);
    }

    #[test]
    fn inherited_profiles_compose_for_region_balancing() {
        // The region holds a `}` inside a `'` string. ts's OWN string form
        // makes that brace transparent while balancing; js's profile alone
        // would have closed the region early at it. The pattern is lineRest
        // based so both lines match cleanly once the region is right.
        let source = r#"
grammar js {
    skip    [ ' ' ]

    rule program { each { stmt } as body }

    rule stmt { lineRest as rest }
}

grammar ts extends js {
    string  ( '\'' )
}

magic runTs(ts.program as p) {
    "ts"
}

str b = magic(runTs) {
    done
    map('}') called
}
"#;
        let outcome = expand_source(source).expect("the composed profile balances the region");
        assert_eq!(outcome.records.len(), 1);
        assert!(!outcome.expanded.contains("map('}')"));
    }

    #[test]
    fn unknown_extends_parent_is_a_compile_error() {
        let source = r#"
grammar ts extends ghost {
    rule r { $word w }
}

magic runIt(ts.r as x) {
    "x"
}

str s = magic(runIt) {
    hi
}
"#;
        let errors = expansion_errors(source);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("unknown grammar `ghost` in `extends`")),
            "expected the unknown-parent diagnostic: {errors:?}"
        );
    }

    #[test]
    fn cyclic_extends_chains_are_rejected() {
        let source = r#"
grammar a extends b {
    rule r { $word w }
}

grammar b extends a {
    rule s { $word w }
}

magic runIt(a.r as x) {
    "x"
}

str s = magic(runIt) {
    hi
}
"#;
        let errors = expansion_errors(source);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("cyclic `extends`")),
            "expected a cycle diagnostic: {errors:?}"
        );
    }

    #[test]
    fn line_mode_elements_in_flow_grammars_are_rejected() {
        let source = r#"
grammar bad {
    skip [ ' ', '\t', '\r', '\n' ]

    rule r {
        $word w eol
    }
}

magic runIt(bad.r as x) {
    "x"
}

str s = magic(runIt) {
    hi
}
"#;
        let errors = expansion_errors(source);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("line-mode element `eol` in flow grammar `bad`")),
            "expected the flow-grammar rejection: {errors:?}"
        );
    }

    #[test]
    fn line_machinery_inside_soft_is_rejected() {
        let source = r#"
grammar pyish {
    skip [ ' ' ]

    rule def {
        "def" $word name ":" soft { indent { $word body } }
    }
}

magic runIt(pyish.def as x) {
    "x"
}

str s = magic(runIt) {
    def f: x
}
"#;
        let errors = expansion_errors(source);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("`indent` inside `soft` is rejected")),
            "expected the in-soft rejection: {errors:?}"
        );
    }

    #[test]
    fn line_grammars_keep_their_line_machinery() {
        // The same shapes in a LINE grammar and OUTSIDE soft stay legal:
        // eol and indent are exactly what line mode is for.
        let source = r#"
grammar py {
    skip [ ' ' ]

    rule def {
        "def" $word name ":"
        indent { $word body eol }
    }
}

magic runIt(py.def as x) {
    "x"
}

str s = magic(runIt) {
    def f: run
}
"#;
        expand_source(source).expect("line-mode machinery in a line grammar is legal");
    }
}
