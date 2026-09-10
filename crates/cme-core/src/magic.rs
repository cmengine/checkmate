//! Megaprogramming data model (WHITEPAPER §8): lexical profiles, patterns,
//! `where` expressions, expansion templates, and capture values.
//!
//! This module is the shared vocabulary of the megaprogram subsystem: the
//! scanner, the pattern/template parsers, the packrat matcher, and the
//! template elaborator all speak in these types. Recognition (parsing source
//! text into these models) stays in `cme-compiler`; ownership of the data
//! model stays here, mirroring the `ast` module's role for the main language.
//!
//! Two deviations from the whitepaper text are load-bearing and documented in
//! `plan.md` §1.4: captures expose a `.matched` accessor (the matched source
//! text, whitespace-trimmed at the edges), and every pattern element carries a
//! span so megaprogram diagnostics can point at the embedded-language source.

use crate::Span;

// ---------------------------------------------------------------------------
// Lexical profile (§8.2)
// ---------------------------------------------------------------------------

/// One item of a character set: a single character or an inclusive range
/// (`a-z`). `[^…]` sets negate the whole list at match time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CharItem {
    Char(char),
    Range(char, char),
}

/// A character set: `[a-z0-9_]` / `[^…]` / the empty `skip [ ]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CharSet {
    pub negated: bool,
    pub items: Vec<CharItem>,
}

impl CharSet {
    pub fn empty() -> Self {
        Self {
            negated: false,
            items: Vec::new(),
        }
    }

    /// Builds a set from single characters (no ranges, no negation).
    pub fn of(chars: &[char]) -> Self {
        Self {
            negated: false,
            items: chars.iter().map(|c| CharItem::Char(*c)).collect(),
        }
    }

    /// True when `c` is in the set (negation flips the verdict).
    pub fn matches(&self, c: char) -> bool {
        let hit = self.items.iter().any(|item| match item {
            CharItem::Char(x) => *x == c,
            CharItem::Range(lo, hi) => *lo <= c && c <= *hi,
        });
        hit != self.negated
    }
}

/// A comment form (§8.2): a line comment (`comment ( "//" )` — runs to end of
/// line) or a block comment (`comment ( "/*" until "*/" )`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentForm {
    pub opener: String,
    pub closer: Option<String>,
}

impl CommentForm {
    /// A line comment: everything from `opener` to the line terminator.
    pub fn line(opener: &str) -> Self {
        Self {
            opener: opener.to_string(),
            closer: None,
        }
    }

    /// A block comment: from `opener` through the first `closer`.
    pub fn block(opener: &str, closer: &str) -> Self {
        Self {
            opener: opener.to_string(),
            closer: Some(closer.to_string()),
        }
    }
}

/// A string form (§8.2): opens and closes on `quote`, honors backslash
/// escapes. `multiline` forms may span line terminators; `island` delimiters
/// are transparent to brace balancing (§8.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringForm {
    pub quote: char,
    pub multiline: bool,
    pub island: Option<(String, String)>,
}

/// The lexical profile of a grammar: skip set, comment forms, string forms.
/// A grammar whose skip set contains a line terminator is flow-oriented;
/// otherwise it is line-oriented and `eol`/`line`/`indent` are first-class.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LexProfile {
    pub skip: CharSet,
    pub comments: Vec<CommentForm>,
    pub strings: Vec<StringForm>,
}

impl LexProfile {
    /// True when the skip set lets the skipper cross line boundaries (§8.2).
    pub fn is_flow_oriented(&self) -> bool {
        self.skip.matches('\n') || self.skip.matches('\r')
    }
}

// ---------------------------------------------------------------------------
// `where` conditions and context bindings (§8.3.4, §8.3.7)
// ---------------------------------------------------------------------------

/// An accessor applied to a capture inside a `where` condition or template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accessor {
    /// The matched source text, whitespace-trimmed at the edges (plan §1.4.4).
    Matched,
    /// One-based line of the capture's start.
    Line,
    /// One-based column of the capture's start.
    Col,
    /// A list capture's element count.
    Length,
    /// The capture's source span (§8.3.4). The span is the value the
    /// whitepaper threads into `cm.parse*(text, span)` / `cm.code.*`;
    /// plan §1.4.9 accepts and ignores that argument at the text level, so
    /// the accessor yields the span's region-relative `line:col` text when
    /// one is needed as a plain value.
    Span,
}

/// Binary operators available to `where` conditions (Appendix A subset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CtxBinOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// A compile-time expression over captures: `where` conditions, `context`
/// binding values, and template condition guards share this language.
#[derive(Debug, Clone, PartialEq)]
pub enum CtxExpr {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// A capture path (`name`, `item.field`, `doc.root.fields`) with an
    /// optional trailing accessor (`$x.matched`, `$xs.length`).
    Capture {
        path: Vec<String>,
        accessor: Option<Accessor>,
    },
    Bin(CtxBinOp, Box<CtxExpr>, Box<CtxExpr>),
    Not(Box<CtxExpr>),
    /// `some x in xs { cond }`
    SomeIn {
        var: String,
        list: Box<CtxExpr>,
        cond: Box<CtxExpr>,
    },
    /// `all x in xs { cond }`
    AllIn {
        var: String,
        list: Box<CtxExpr>,
        cond: Box<CtxExpr>,
    },
    /// `present(x)` — true when an optional capture (or context field with a
    /// default) holds a value.
    Present {
        path: Vec<String>,
    },
    /// A call to a pure compile-time function (§8.5). Evaluators that do not
    /// implement §8.5 report an unsupported diagnostic instead of guessing.
    Call {
        path: Vec<String>,
        args: Vec<CtxExpr>,
    },
}

/// A declared `context` field of a rule (§8.3.7): `context { selector
/// parent = none }` — an optional compile-time default (`= none` means an
/// absent value, surfacing as an `Opt(None)` capture).
#[derive(Debug, Clone, PartialEq)]
pub struct ContextField {
    pub name: String,
    pub default: Option<CtxExpr>,
}

// ---------------------------------------------------------------------------
// Patterns (§8.3)
// ---------------------------------------------------------------------------

/// A pattern: a sequence of elements matched left to right against the
/// invocation region (§8.3.10 `pattern → { term }`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pattern {
    pub elems: Vec<PatElem>,
}

impl Pattern {
    pub fn single(kind: PatKind, span: Span) -> Self {
        Self {
            elems: vec![PatElem { span, kind }],
        }
    }
}

/// One element of a pattern, with the source span it was parsed from.
#[derive(Debug, Clone, PartialEq)]
pub struct PatElem {
    pub span: Span,
    pub kind: PatKind,
}

/// The kind of a fragment (`$ident`, `$word`, …) — §8.3.3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FragKind {
    Ident,
    Word,
    Tag,
    Int,
    Float,
    Str,
    /// A single token or balanced delimiter tree honoring profile strings
    /// (§8.3.3). `explicit` carries the `$tt<"open" "close">` form: the
    /// balanced tree roots at the given delimiter pair.
    Tt {
        explicit: Option<(String, String)>,
    },
    /// Effective tail if one exists, else the region remainder.
    Text,
    /// A template with islands split by `{{ }}` (or parameterized
    /// delimiters); island content is a Checkmate expression, or parsed by
    /// the referenced rule (§8.3.3).
    Template {
        open: String,
        close: String,
        rule: Option<Vec<String>>,
    },
    /// Verbatim tail parsed as Checkmate code, or by the referenced rule.
    Raw(Option<Vec<String>>),
    /// A live Checkmate expression island.
    Expr,
    /// A live Checkmate type island.
    Type,
    /// A live Checkmate block island.
    Block,
}

/// The kind of an inert editor annotation (§8.1, §8.3.10). The payload is
/// deliberately not retained: annotations are consumed by `cme-lsp` (§8.9),
/// never affect matching, and are the only place host registries are
/// visible — which the megaprogram pass must not observe (§8.5 purity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationKind {
    /// `#complete(host.registry.expr)` — completion proposals.
    Complete,
    /// `#hover("tooltip text")` — hover documentation.
    Hover,
    /// `#token("token text")` — semantic-token hint.
    Token,
}

/// A pattern element kind (§8.3.2, §8.3.10).
#[derive(Debug, Clone, PartialEq)]
pub enum PatKind {
    /// `"lit"` or `i"lit"` — an exact (possibly case-insensitive) sequence.
    Lit { text: String, insensitive: bool },
    /// `[a-z0-9_]` / `[^…]` — exactly one character.
    Class { set: CharSet, bind: Option<String> },
    /// `any` — any single character.
    Any { bind: Option<String> },
    /// `scan […]` — a maximal run of at least one character from the set.
    Scan { set: CharSet, bind: Option<String> },
    /// `until "lit"` / `until { p }` — verbatim run stopping before `p`
    /// matching as a whole; the stop consumes nothing.
    Until {
        stop: Box<Pattern>,
        bind: Option<String>,
    },
    /// `lineRest` — verbatim run to end of line (terminator excluded).
    LineRest { bind: Option<String> },
    /// `eol` — line mode only: terminator plus the transparent tail.
    Eol,
    /// `line` — zero-width: a non-skip character remains before the terminator.
    Line,
    /// `eof` — only skip characters and transparent lines remain.
    Eof,
    /// `soft { p }` — newlines join the skip set inside `p`; atomic.
    Soft(Box<Pattern>),
    /// `optional { p }` — `p` or nothing, atomically.
    Optional {
        body: Box<Pattern>,
        bind: Option<String>,
    },
    /// `each [+] [sep p] [trailing] [[n, m]] { p }` — repetition.
    Each {
        plus: bool,
        sep: Option<Box<Pattern>>,
        trailing: bool,
        bounds: Option<(u32, Option<u32>)>,
        body: Box<Pattern>,
        bind: Option<String>,
    },
    /// `oneof { label => ( p ), … }` — ordered choice, first match wins.
    OneOf {
        branches: Vec<(String, Pattern)>,
        /// `oneof { … } as name` — binds the chosen branch's record.
        bind: Option<String>,
    },
    /// `peek { p }` / `not { p }` — zero-width lookahead.
    Peek { negated: bool, body: Box<Pattern> },
    /// `( p )` — grouping.
    Group {
        body: Box<Pattern>,
        bind: Option<String>,
    },
    /// A rule reference, qualified across grammars, with optional `with
    /// context` bindings and capture name (§8.3.7).
    RuleRef {
        path: Vec<String>,
        ctx: Vec<(String, CtxExpr)>,
        bind: Option<String>,
    },
    /// `recur` — the innermost enclosing rule.
    Recur,
    /// `indent { p }` or `indent verbatim as name` (§8.3.5).
    Indent {
        body: Option<Box<Pattern>>,
        verbatim: Option<String>,
    },
    /// `raw { p }` — the skipper is suspended inside `p`.
    Raw(Box<Pattern>),
    /// `where cond` — consumes nothing; fails when the condition is falsy.
    Where { cond: CtxExpr },
    /// `label "msg" { p }` — diagnostic context for failures inside `p`.
    Label { message: String, body: Box<Pattern> },
    /// `$ident` / `$word` / … with optional validator and capture name.
    Fragment {
        kind: FragKind,
        insensitive: bool,
        /// Validator: a rule path the matched text must match, or a pure
        /// function name (§8.3.3). Parsed but resolved later.
        validator: Option<Vec<String>>,
        bind: Option<String>,
    },
    /// An inert editor annotation (`#complete(…)`, `#hover(…)`, `#token(…)`)
    /// between terms (§8.3.10): consumes nothing, never affects matching.
    Annotation { kind: AnnotationKind },
}

// ---------------------------------------------------------------------------
// Templates (§8.4)
// ---------------------------------------------------------------------------

/// An expansion template: the target code with holes. Literal text is emitted
/// verbatim; the remaining nodes are the §8.4 constructs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Template {
    pub nodes: Vec<TmplNode>,
}

/// One part of a template string `$"…{cap}…"`.
#[derive(Debug, Clone, PartialEq)]
pub enum TmplStrPart {
    Lit(String),
    Hole {
        path: Vec<String>,
        accessor: Option<Accessor>,
    },
}

/// A value position in a template: a capture path or a literal.
///
/// `Call` (a plan extension) allows `@fn(…)` results to feed other `@`-calls
/// and `let` bindings — nested compile-time computation (§8.5). A capture
/// path appearing as the first argument of `cm.parse(…)` is parsed as a
/// grammar rule path by the template parser and arrives as `Str`.
#[derive(Debug, Clone, PartialEq)]
pub enum TmplValue {
    Capture {
        path: Vec<String>,
    },
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Call {
        path: Vec<String>,
        args: Vec<TmplValue>,
    },
}

/// One template node. Every node carries the span of the template element it
/// was parsed from, so generated-code diagnostics can point at the template.
#[derive(Debug, Clone, PartialEq)]
pub enum TmplNode {
    /// Verbatim Checkmate text emitted as-is.
    Text { text: String, span: Span },
    /// `$cap` / `$cap.field` — splices the capture per §8.4's position table.
    /// An optional trailing accessor (`.matched`, `.length`) is applied
    /// before splicing (plan §1.4 extensions).
    Splice {
        path: Vec<String>,
        accessor: Option<Accessor>,
        span: Span,
    },
    /// `$"…{cap}…"` — interpolates captures into a Checkmate string literal.
    Interp { parts: Vec<TmplStrPart>, span: Span },
    /// `[each NAME in xs { … }]` — repetition. `NAME` defaults to `item`;
    /// the element's fields are also reachable bare (§8.4).
    Each {
        element: String,
        list: TmplValue,
        body: Box<Template>,
        span: Span,
    },
    /// `[when cond { … } else { … }]` — selection.
    When {
        cond: CtxExpr,
        then: Box<Template>,
        otherwise: Box<Template>,
        span: Span,
    },
    /// `match ($cap) { label => … }` — dispatch on a `oneof` tag; the arm set
    /// must cover every branch label of the tagged capture.
    Match {
        scrutinee: Vec<String>,
        arms: Vec<(String, Template)>,
        span: Span,
    },
    /// `let name = value` — binds a template-local alias; emits nothing.
    Let {
        name: String,
        value: TmplValue,
        span: Span,
    },
    /// `require(cond, "message")` — a failing condition is a compile-time
    /// error anchored at the referenced capture's span (§8.3.4).
    Require {
        cond: CtxExpr,
        message: String,
        span: Span,
    },
    /// `@fn(args)` — a compile-time function call (§8.5).
    Call {
        path: Vec<String>,
        args: Vec<TmplValue>,
        span: Span,
    },
}

// ---------------------------------------------------------------------------
// Capture values (matcher output → elaborator input)
// ---------------------------------------------------------------------------

/// The splice behavior of a text capture (§8.4 position table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextKind {
    /// `$ident` — splices as a Checkmate identifier.
    Ident,
    /// `$word` — foreign identifier; splices raw.
    Word,
    /// `$tag` — relaxed foreign token (possibly case-folded); splices raw.
    Tag,
    /// `$str` — splices as a quoted, escaped Checkmate string literal.
    Str,
    /// Everything else (groups, scans, `until`, `lineRest`, records) splices
    /// raw in expression/name positions.
    Raw,
}

/// One capture: the kind-specific payload plus the exact source extent and
/// the matched text (whitespace-trimmed at the edges, plan §1.4.4).
#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    pub kind: CaptureKind,
    pub matched: String,
    pub span: Span,
}

impl Capture {
    /// A raw text capture with identical `matched` and payload.
    pub fn raw_text(text: String, span: Span) -> Self {
        let trimmed = text.trim().to_string();
        Self {
            kind: CaptureKind::Text(TextKind::Raw),
            matched: trimmed.clone(),
            span,
        }
    }

    /// The `.matched` accessor: the kind-specific text form.
    pub fn matched(&self) -> &str {
        &self.matched
    }

    /// `present(x)` semantics: optional captures report their inner presence.
    pub fn is_present(&self) -> bool {
        match &self.kind {
            CaptureKind::Opt(inner) => inner.as_ref().is_some_and(|c| c.is_present()),
            _ => true,
        }
    }
}

/// The payload of a capture (§2.2 of plan.md).
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureKind {
    Text(TextKind),
    Int(i64),
    Float(f64),
    List(Vec<Capture>),
    /// A `oneof` branch result or a rule invocation result: a tag plus named
    /// fields. A branch whose body is a single bare rule reference inherits
    /// that rule's record fields (re-tagged with the branch label).
    Record {
        tag: String,
        fields: Vec<(String, Capture)>,
    },
    /// An `optional { p } as x` capture: `some(value)` or `none`.
    Opt(Option<Box<Capture>>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_sets_match_ranges_and_negation() {
        let set = CharSet {
            negated: false,
            items: vec![CharItem::Range('a', 'z'), CharItem::Char('_')],
        };
        assert!(set.matches('q'));
        assert!(set.matches('_'));
        assert!(!set.matches('Q'));
        assert!(!set.matches('0'));

        let negated = CharSet {
            negated: true,
            items: vec![CharItem::Char('\n')],
        };
        assert!(negated.matches('x'));
        assert!(!negated.matches('\n'));
    }

    #[test]
    fn flow_orientation_follows_the_skip_set() {
        let flow = LexProfile {
            skip: CharSet::of(&[' ', '\t', '\r', '\n']),
            comments: Vec::new(),
            strings: Vec::new(),
        };
        assert!(flow.is_flow_oriented());

        let line = LexProfile {
            skip: CharSet::of(&[' ', '\t']),
            comments: vec![CommentForm::line("#")],
            strings: Vec::new(),
        };
        assert!(!line.is_flow_oriented());
    }

    #[test]
    fn captures_report_presence_and_trimmed_matched_text() {
        let text = Capture {
            kind: CaptureKind::Text(TextKind::Word),
            matched: "  fuel  ".to_string(),
            span: Span::new(0, 8),
        };
        // The stored `matched` is what the scanner produced; `matched()` is
        // the accessor. Trimming happens at capture construction time in the
        // matcher, so a hand-built capture round-trips verbatim here.
        assert_eq!(text.matched(), "  fuel  ");
        assert!(text.is_present());

        let none = Capture {
            kind: CaptureKind::Opt(None),
            matched: String::new(),
            span: Span::missing(0),
        };
        assert!(!none.is_present());

        let some = Capture {
            kind: CaptureKind::Opt(Some(Box::new(Capture {
                kind: CaptureKind::Int(7),
                matched: "7".to_string(),
                span: Span::missing(0),
            }))),
            matched: "7".to_string(),
            span: Span::missing(0),
        };
        assert!(some.is_present());
    }

    #[test]
    fn raw_text_captures_trim_their_edges() {
        let capture = Capture::raw_text("  body  \n".to_string(), Span::new(0, 9));
        assert_eq!(capture.matched(), "body");
        assert_eq!(capture.kind, CaptureKind::Text(TextKind::Raw));
    }
}
