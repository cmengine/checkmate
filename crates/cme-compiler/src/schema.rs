//! The §9 schema file front end: lexing, parsing, and the multi-namespace
//! [`SchemaSet`] a host assembles from its `.cm` schema files.
//!
//! A schema file (WHITEPAPER §9.1) is one namespace root of the host
//! contract:
//!
//! ```text
//! schema engine 1.4.0
//!
//! capability graphics {
//!     since 1.0.0 TextureHandle LoadTexture(str path)
//!     since 1.0.0 void DrawTexture(TextureHandle tex, vec2 position)
//! }
//!
//! interface gamemode requires core {
//!     since 1.0.0 GameState InitGame(GameConfig config)
//!     since 1.0.0 void OnTick(GameState state, float deltaTime)
//! }
//! ```
//!
//! The grammar is deliberately its own small language, not the Checkmate
//! program grammar: no statements, no expressions, no imports — just the
//! header, `capability`/`interface` contract blocks, and the §9.3 boundary
//! types (`struct`/`enum` spelled exactly like their Checkmate
//! counterparts). A dedicated scanner keeps `1.4.0` a single token and
//! keeps the diagnostics pointed at the schema text; program-level lexer
//! quirks (float literals, string interpolation) never leak in.
//!
//! Versions are spelled `X.Y.Z` everywhere — the header, `since` tags, and
//! a mod manifest's `[schemas]` targets all share one shape. A `v` prefix
//! (`v1.4.0`) is NOT accepted: the whitepaper's `since` tags, every
//! manifest, and the `Version` parser are unprefixed, and the header is
//! the odd one out.
//!
//! Enforcement here (the contract must be well-formed BEFORE any script is
//! checked against it):
//!
//! - one namespace root per file, `schema <ident> <X.Y.Z>` (§9.1);
//! - PascalCase for every schema declaration and member (§9.3, §2.5);
//!   camelCase for parameters and fields;
//! - members are `since X.Y.Z`-tagged (default 0.0.0), optionally
//!   `optional` (§9.5); `suspend` members are parsed and rejected — the
//!   async system (§4) is out of scope for this milestone;
//! - no duplicate items, no duplicate members, `void` only in return
//!   position (§2.4);
//! - [`SchemaSet::build`] then checks the CROSS-file invariants: one
//!   version per namespace, type names unique across namespaces (the
//!   script-side type space is flat), and every `requires` edge resolving
//!   to a real interface (§9.4).

use std::collections::BTreeMap;

use cme_core::Span;
use cme_core::ast::{FieldDef, Param, PrimitiveType, Stmt, StmtKind, Type, VariantDecl};
// The §9 data model lives in `cme-core` (the AST-ownership rule); this
// module re-exports it so consumers of the front end need only
// `cme_compiler::schema`.
pub use cme_core::schema::{
    ContractKind, MemberRequirement, RequiresPath, SchemaContract, SchemaEnum, SchemaFile,
    SchemaItem, SchemaMember, SchemaStruct, Version, is_camel_case, is_pascal_case,
};

use crate::diagnostics::Diagnostic;

mod codegen_c;
pub use codegen_c::codegen_c;

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

/// One schema-file token. Deliberately minimal: the schema grammar has no
/// strings, no operators, no expressions.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    /// `1.4.0` — scanned whole so `since` tags and the header version
    /// parse without dot-splitting heuristics. The `v`-prefixed spelling
    /// is not a token: the header takes the same `X.Y.Z` shape as every
    /// other version in the toolchain.
    Version(Version),
    Punct(char),
    Newline,
    Eof,
}

impl Token {
    fn describe(&self) -> String {
        match self {
            Token::Ident(name) => format!("identifier `{name}`"),
            Token::Version(version) => format!("version `{version}`"),
            Token::Punct(punct) => format!("`{punct}`"),
            Token::Newline => "end of line".to_string(),
            Token::Eof => "end of file".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Public token surface — tooling reads the same recognition the parser uses
// ---------------------------------------------------------------------------

/// The class of one schema-file token, for tooling (the language server's
/// schema-file completion and hover ride the exact scanner the parser
/// uses, so they never disagree with it about what a token is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaTokenKind {
    /// An identifier with its spelling.
    Ident(String),
    /// A `X.Y.Z` version (the `v`-prefixed spelling scans here too, with
    /// its migration diagnostic).
    Version,
    /// One of `{ } ( ) < > , . [ ]`.
    Punct(char),
    Newline,
    Eof,
}

/// One schema-file token with its source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaToken {
    pub kind: SchemaTokenKind,
    pub span: Span,
}

/// Lexes a schema file into the public token shape. Comments are skipped
/// (they are not tokens); newlines are kept, since schema layout is
/// newline-delimited.
pub fn schema_tokens(source: &str) -> Vec<SchemaToken> {
    let (tokens, _) = lex_schema(source);
    tokens
        .into_iter()
        .map(|token| SchemaToken {
            kind: match token.token {
                Token::Ident(name) => SchemaTokenKind::Ident(name),
                Token::Version(_) => SchemaTokenKind::Version,
                Token::Punct(punct) => SchemaTokenKind::Punct(punct),
                Token::Newline => SchemaTokenKind::Newline,
                Token::Eof => SchemaTokenKind::Eof,
            },
            span: token.span,
        })
        .collect()
}

#[derive(Debug, Clone)]
struct SpannedToken {
    token: Token,
    span: Span,
}

/// Lexes a schema file. Newlines are significant (struct fields and members
/// are newline-delimited, §2.6/§2.7 shape), comments are skipped.
fn lex_schema(source: &str) -> (Vec<SpannedToken>, Vec<Diagnostic>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let bytes = source.as_bytes();
    let mut position = 0usize;

    while position < bytes.len() {
        let byte = bytes[position];
        match byte {
            b' ' | b'\t' | b'\r' => position += 1,
            b'\n' => {
                tokens.push(SpannedToken {
                    token: Token::Newline,
                    span: Span::new(position, position + 1),
                });
                position += 1;
            }
            b'/' if bytes.get(position + 1) == Some(&b'/') => {
                while position < bytes.len() && bytes[position] != b'\n' {
                    position += 1;
                }
            }
            b'/' if bytes.get(position + 1) == Some(&b'*') => {
                let start = position;
                position += 2;
                let mut closed = false;
                while position + 1 < bytes.len() {
                    if bytes[position] == b'*' && bytes[position + 1] == b'/' {
                        position += 2;
                        closed = true;
                        break;
                    }
                    position += 1;
                }
                if !closed {
                    errors.push(Diagnostic::parse(
                        "unterminated block comment in schema file",
                        Span::new(start, source.len()),
                    ));
                    position = bytes.len();
                }
            }
            b'{' | b'}' | b'(' | b')' | b'<' | b'>' | b',' | b'.' | b'[' | b']' => {
                tokens.push(SpannedToken {
                    token: Token::Punct(byte as char),
                    span: Span::new(position, position + 1),
                });
                position += 1;
            }
            b'0'..=b'9' => {
                let start = position;
                match scan_version(bytes, position) {
                    Some((version, end)) => {
                        tokens.push(SpannedToken {
                            token: Token::Version(version),
                            span: Span::new(start, end),
                        });
                        position = end;
                    }
                    None => {
                        while position < bytes.len() && bytes[position].is_ascii_digit() {
                            position += 1;
                        }
                        errors.push(Diagnostic::parse(
                            "a schema version must be `X.Y.Z`",
                            Span::new(start, position),
                        ));
                    }
                }
            }
            _ if byte == b'_' || byte.is_ascii_alphabetic() => {
                let start = position;
                while position < bytes.len()
                    && (bytes[position] == b'_' || bytes[position].is_ascii_alphanumeric())
                {
                    position += 1;
                }
                // The retired `v1.4.0` header spelling: one pointed
                // diagnostic over the whole token, and the version value is
                // still recovered so parsing continues without cascades.
                if bytes[start] == b'v'
                    && let Some((version, end)) = scan_version_prefixed(bytes, start, position)
                {
                    errors.push(Diagnostic::parse(
                        "schema versions are written `X.Y.Z` — drop the `v` prefix",
                        Span::new(start, end),
                    ));
                    tokens.push(SpannedToken {
                        token: Token::Version(version),
                        span: Span::new(start, end),
                    });
                    position = end;
                    continue;
                }
                tokens.push(SpannedToken {
                    token: Token::Ident(source[start..position].to_string()),
                    span: Span::new(start, position),
                });
            }
            other => {
                let span = Span::new(position, position + 1);
                errors.push(Diagnostic::parse(
                    format!("unexpected byte `0x{other:02x}` in schema file"),
                    span,
                ));
                position += 1;
            }
        }
    }

    tokens.push(SpannedToken {
        token: Token::Eof,
        span: Span::new(source.len(), source.len()),
    });
    (tokens, errors)
}

/// Attempts to scan `X.Y.Z` starting at `start` (which must point at a
/// digit). Returns the version and the end offset when the full shape is
/// present.
fn scan_version(bytes: &[u8], start: usize) -> Option<(Version, usize)> {
    let mut position = start;
    let component = |position: &mut usize| -> Option<u32> {
        let begin = *position;
        while *position < bytes.len() && bytes[*position].is_ascii_digit() {
            *position += 1;
        }
        if begin == *position {
            return None;
        }
        std::str::from_utf8(&bytes[begin..*position])
            .ok()?
            .parse()
            .ok()
    };
    let major = component(&mut position)?;
    if bytes.get(position) != Some(&b'.') {
        return None;
    }
    position += 1;
    let minor = component(&mut position)?;
    if bytes.get(position) != Some(&b'.') {
        return None;
    }
    position += 1;
    let patch = component(&mut position)?;
    // The next character must not continue a longer number or identifier.
    if matches!(bytes.get(position), Some(byte) if byte.is_ascii_alphanumeric() || *byte == b'.' || *byte == b'_')
    {
        return None;
    }
    Some((Version::new(major, minor, patch), position))
}

/// Recognizes the retired `vX.Y.Z` spelling: `start` points at the `v`,
/// `ident_end` is where the identifier scan stopped (the version may
/// continue past it through the dots). Returns the version and the full
/// token end when the whole shape is a `v`-prefixed version, so the
/// scanner can report the migration with one diagnostic.
fn scan_version_prefixed(bytes: &[u8], start: usize, ident_end: usize) -> Option<(Version, usize)> {
    debug_assert_eq!(bytes[start], b'v');
    // The identifier consumed at least one character (`v` alone would have
    // been re-scanned as an identifier); the version continues from the
    // digits that followed it. An identifier that continues past the major
    // component (`v10x`) is just a name.
    let (version, end) = scan_version(bytes, start + 1)?;
    if end < ident_end {
        return None; // the identifier continued past the version (`v10x`)
    }
    Some((version, end))
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// The outcome of parsing one schema file: the file (when the header
/// parsed) plus every diagnostic. Like [`crate::ParseOutcome`], ANY
/// diagnostic means the schema must not be used.
#[derive(Debug, Clone)]
pub struct SchemaParseOutcome {
    pub file: Option<SchemaFile>,
    pub diagnostics: Vec<Diagnostic>,
}

impl SchemaParseOutcome {
    pub fn is_clean(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Parses one schema file (§9.1). Recovery is line-oriented: a broken
/// member or item is reported and skipped, so the remaining declarations
/// still parse and the host sees every defect at once.
pub fn parse_schema_file(source: &str) -> SchemaParseOutcome {
    let (tokens, mut diagnostics) = lex_schema(source);
    let mut parser = SchemaParser {
        tokens,
        position: 0,
        diagnostics: Vec::new(),
    };
    let file = parser.parse_file();
    diagnostics.append(&mut parser.diagnostics);
    SchemaParseOutcome { file, diagnostics }
}

struct SchemaParser {
    tokens: Vec<SpannedToken>,
    position: usize,
    diagnostics: Vec<Diagnostic>,
}

impl SchemaParser {
    fn peek(&self) -> &SpannedToken {
        self.tokens
            .get(self.position)
            .unwrap_or_else(|| self.tokens.last().expect("scanner always emits Eof"))
    }

    fn bump(&mut self) -> SpannedToken {
        let token = self.peek().clone();
        if self.position < self.tokens.len() - 1 {
            self.position += 1;
        }
        token
    }

    fn at_punct(&self, punct: char) -> bool {
        matches!(self.peek().token, Token::Punct(p) if p == punct)
    }

    fn at_ident(&self, name: &str) -> bool {
        matches!(&self.peek().token, Token::Ident(ident) if ident == name)
    }

    fn record(&mut self, message: impl Into<String>, span: Span) {
        self.diagnostics.push(Diagnostic::parse(message, span));
    }

    /// Skips tokens through the end of the current line (the newline
    /// included), so parsing resumes at the next declaration.
    fn skip_line(&mut self) {
        while !matches!(self.peek().token, Token::Newline | Token::Eof) {
            self.bump();
        }
        if matches!(self.peek().token, Token::Newline) {
            self.bump();
        }
    }

    /// Skips newlines and expects nothing; returns true when not at EOF.
    fn skip_newlines(&mut self) {
        while matches!(self.peek().token, Token::Newline) {
            self.bump();
        }
    }

    fn parse_file(&mut self) -> Option<SchemaFile> {
        self.skip_newlines();
        // Header: `schema <namespace> <X.Y.Z>`
        let header_start = self.peek().span.start;
        if !self.at_ident("schema") {
            let span = self.peek().span;
            self.record(
                format!(
                    "a schema file must start with `schema <namespace> <X.Y.Z>`, found {}",
                    self.peek().token.describe()
                ),
                span,
            );
            return None;
        }
        self.bump();
        let namespace = match self.bump().token {
            Token::Ident(name) if is_identifier(&name) => name,
            other => {
                self.record(
                    format!(
                        "expected the schema namespace identifier, found {}",
                        other.describe()
                    ),
                    self.peek().span,
                );
                return None;
            }
        };
        let version = match self.bump().token {
            Token::Version(version) => version,
            other => {
                self.record(
                    format!(
                        "expected the schema version (`1.4.0`), found {}",
                        other.describe()
                    ),
                    self.peek().span,
                );
                return None;
            }
        };
        self.expect_line_end("the schema header");

        let header_end = self.tokens[self.position.saturating_sub(1)].span.end;
        let mut items: Vec<SchemaItem> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        let mut last_item_end: Option<usize> = None;
        loop {
            self.skip_newlines();
            if matches!(self.peek().token, Token::Eof) {
                break;
            }
            match self.parse_item(&mut seen) {
                Some(item) => {
                    last_item_end = Some(item.span().end);
                    items.push(item);
                }
                // The item parser already reported and skipped the line.
                None => continue,
            }
        }
        let end = last_item_end.unwrap_or(header_end);

        Some(SchemaFile {
            namespace,
            version,
            items,
            span: Span::new(header_start, end),
        })
    }

    /// One top-level item: capability, interface, struct, or enum.
    fn parse_item(&mut self, seen: &mut Vec<String>) -> Option<SchemaItem> {
        let start = self.peek().span.start;
        let keyword = match &self.peek().token {
            Token::Ident(name) => name.clone(),
            other => {
                let span = self.peek().span;
                self.record(
                    format!(
                        "expected `capability`, `interface`, `struct`, or `enum`, found {}",
                        other.describe()
                    ),
                    span,
                );
                self.skip_line();
                return None;
            }
        };
        match keyword.as_str() {
            "capability" | "interface" => self.parse_contract(&keyword, start, seen),
            "struct" => self.parse_struct(start, seen),
            "enum" => self.parse_enum(start, seen),
            _ => {
                let span = self.peek().span;
                self.record(
                    format!(
                        "expected `capability`, `interface`, `struct`, or `enum`, found `{keyword}`"
                    ),
                    span,
                );
                self.skip_line();
                None
            }
        }
    }

    /// A `capability`/`interface` block (§9.1) with its optional `requires`
    /// edge (§9.4) and member list.
    fn parse_contract(
        &mut self,
        keyword: &str,
        start: usize,
        seen: &mut Vec<String>,
    ) -> Option<SchemaItem> {
        self.bump(); // keyword
        let name_token = self.bump();
        let name = match name_token.token {
            Token::Ident(name) => name,
            other => {
                self.record(
                    format!("expected the {keyword} name, found {}", other.describe()),
                    name_token.span,
                );
                self.skip_line();
                return None;
            }
        };
        // Capability and interface names carry no case rule: §9.3 calls
        // contracts boundary elements, yet every path the whitepaper
        // spells (`engine.graphics`, `engine.gamemode`, `ui.widgets`) is
        // camelCase — the examples are the observable convention.
        if seen.contains(&name) {
            self.record(
                format!("duplicate schema declaration `{name}`"),
                name_token.span,
            );
        } else {
            seen.push(name.clone());
        }

        let mut requires = if self.at_ident("requires") {
            // §9.4 shape one: `interface gamemode requires core { … }`.
            self.bump();
            Some(self.parse_requires_path())
        } else {
            None
        };

        if !self.at_punct('{') {
            let span = self.peek().span;
            self.record(
                format!(
                    "expected `{{` to open the {keyword} body, found {}",
                    self.peek().token.describe()
                ),
                span,
            );
            self.skip_line();
            return None;
        }
        self.bump();

        let kind = if keyword == "capability" {
            ContractKind::Capability
        } else {
            ContractKind::Interface
        };
        let mut members: Vec<SchemaMember> = Vec::new();
        let mut last_end: Option<usize> = None;
        loop {
            self.skip_newlines();
            if self.at_punct('}') {
                last_end = Some(self.bump().span.end);
                break;
            }
            if matches!(self.peek().token, Token::Eof) {
                self.record(
                    format!("expected `}}` before end of file in {keyword} `{name}`"),
                    self.peek().span,
                );
                break;
            }
            // §9.1 shape two: `requires` may also open the body —
            // `capability network { requires auth … }`.
            if self.at_ident("requires") {
                if requires.is_some() {
                    self.record(
                        format!("duplicate `requires` in {keyword} `{name}`"),
                        self.peek().span,
                    );
                    self.bump();
                    let _ = self.parse_requires_path();
                    continue;
                }
                self.bump();
                requires = Some(self.parse_requires_path());
                self.skip_newlines();
                continue;
            }
            match self.parse_member(&name, kind, &members) {
                Some(member) => {
                    last_end = Some(member.span.end);
                    members.push(member);
                }
                None => continue,
            }
        }
        let end = last_end.unwrap_or(start);

        Some(SchemaItem::Contract(SchemaContract {
            kind,
            name,
            requires,
            members,
            span: Span::new(start, end),
        }))
    }

    /// `requires <ident> | <ident>.<ident>` (§9.2, §9.4).
    fn parse_requires_path(&mut self) -> RequiresPath {
        let start = self.peek().span.start;
        let mut segments = Vec::new();
        loop {
            match self.bump().token {
                Token::Ident(segment) => segments.push(segment),
                other => {
                    self.record(
                        format!(
                            "expected an interface name in `requires`, found {}",
                            other.describe()
                        ),
                        self.peek().span,
                    );
                    break;
                }
            }
            if self.at_punct('.') {
                self.bump();
                continue;
            }
            break;
        }
        let end = self.tokens[self.position.saturating_sub(1)].span.end;
        RequiresPath {
            segments,
            span: Span::new(start, end),
        }
    }

    /// One contract member:
    /// `since X.Y.Z`? `optional`? (`suspend` rejected) TYPE Name(params) —
    /// newline-delimited (§9.1). `kind` decides whether `optional` is
    /// legal: §9.5 defines it for INTERFACE members a mod may skip;
    /// capability members are host-provided, so the flag has no meaning
    /// there and is rejected.
    fn parse_member(
        &mut self,
        contract: &str,
        kind: ContractKind,
        existing: &[SchemaMember],
    ) -> Option<SchemaMember> {
        let start = self.peek().span.start;
        let mut since = Version::ZERO;
        if self.at_ident("since") {
            self.bump();
            match self.bump().token {
                Token::Version(version) => since = version,
                other => {
                    self.record(
                        format!("expected `X.Y.Z` after `since`, found {}", other.describe()),
                        self.peek().span,
                    );
                    self.skip_line();
                    return None;
                }
            }
        }
        let mut requirement = MemberRequirement::Required;
        if self.at_ident("optional") {
            let optional_span = self.peek().span;
            self.bump();
            if kind == ContractKind::Capability {
                self.record(
                    "`optional` is an interface-member concept (§9.5): a mod may skip an \
                     optional interface function, but a capability member is provided by \
                     the host and must always exist — drop `optional` here",
                    optional_span,
                );
            }
            requirement = MemberRequirement::Optional;
        }
        if self.at_ident("suspend") {
            let span = self.peek().span;
            self.record(
                "suspend members are not supported yet: the async/continuation \
                 system (WHITEPAPER §4) is out of scope for this milestone",
                span,
            );
            self.bump();
        }

        let return_ty = self.parse_type("member return type", true)?;
        let name_token = self.bump();
        let name = match name_token.token {
            Token::Ident(name) => name,
            other => {
                self.record(
                    format!("expected the member name, found {}", other.describe()),
                    name_token.span,
                );
                self.skip_line();
                return None;
            }
        };
        if !is_pascal_case(&name) {
            self.record(
                format!(
                    "schema member `{name}` must be PascalCase: boundary functions \
                     cross the host/script boundary (§2.5)"
                ),
                name_token.span,
            );
        }
        if existing.iter().any(|member| member.name == name) {
            self.record(
                format!("duplicate member `{name}` in `{contract}`"),
                name_token.span,
            );
        }
        if !self.at_punct('(') {
            let span = self.peek().span;
            self.record(
                format!(
                    "expected `(` to open the parameter list, found {}",
                    self.peek().token.describe()
                ),
                span,
            );
            self.skip_line();
            return None;
        }
        self.bump();
        let params = self.parse_params(&name);
        if !self.at_punct(')') {
            let span = self.peek().span;
            self.record(
                format!(
                    "expected `)` after the parameters of `{name}`, found {}",
                    self.peek().token.describe()
                ),
                span,
            );
            self.skip_line();
            return None;
        }
        let close = self.bump();
        // A member ends at a line break; a same-line `}` closes the
        // contract (the §2.6 compact shape).
        if !matches!(
            self.peek().token,
            Token::Newline | Token::Eof | Token::Punct('}')
        ) {
            let span = self.peek().span;
            self.record(
                format!(
                    "expected a new line after member `{name}`, found {}",
                    self.peek().token.describe()
                ),
                span,
            );
            self.skip_line();
        }

        Some(SchemaMember {
            name,
            params,
            return_ty,
            since,
            requirement,
            span: Span::new(start, close.span.end),
        })
    }

    /// `(TYPE name, …)` — the §2.11 parameter shape, comma-separated.
    fn parse_params(&mut self, member: &str) -> Vec<Param> {
        let mut params: Vec<Param> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        if self.at_punct(')') {
            return params;
        }
        loop {
            let Some(ty) = self.parse_type("parameter type", false) else {
                self.skip_line();
                return params;
            };
            let name = match self.bump().token {
                Token::Ident(name) => name,
                other => {
                    self.record(
                        format!("expected a parameter name, found {}", other.describe()),
                        self.peek().span,
                    );
                    self.skip_line();
                    return params;
                }
            };
            if !is_camel_case(&name) {
                self.record(
                    format!("parameter `{name}` of `{member}` must be camelCase (§2.5)"),
                    self.peek().span,
                );
            }
            if names.contains(&name) {
                self.record(
                    format!("duplicate parameter `{name}` in `{member}`"),
                    self.peek().span,
                );
            } else {
                names.push(name.clone());
            }
            params.push(Param { ty, name });
            if self.at_punct(',') {
                self.bump();
                continue;
            }
            break;
        }
        params
    }

    /// A schema `struct` (§9.3, §2.6 shape): newline-delimited fields.
    fn parse_struct(&mut self, start: usize, seen: &mut Vec<String>) -> Option<SchemaItem> {
        self.bump(); // `struct`
        let name = match self.bump().token {
            Token::Ident(name) => name,
            other => {
                self.record(
                    format!("expected the struct name, found {}", other.describe()),
                    self.peek().span,
                );
                self.skip_line();
                return None;
            }
        };
        self.check_declared_name("struct", &name, seen);
        if !self.at_punct('{') {
            let span = self.peek().span;
            self.record(
                format!(
                    "expected `{{` to open struct `{name}`, found {}",
                    self.peek().token.describe()
                ),
                span,
            );
            self.skip_line();
            return None;
        }
        self.bump();
        let mut fields: Vec<FieldDef> = Vec::new();
        let mut field_names: Vec<String> = Vec::new();
        let end;
        loop {
            self.skip_newlines();
            if self.at_punct('}') {
                end = self.bump().span.end;
                break;
            }
            if matches!(self.peek().token, Token::Eof) {
                self.record(
                    format!("expected `}}` before end of file in struct `{name}`"),
                    self.peek().span,
                );
                end = self.peek().span.end;
                break;
            }
            let Some(ty) = self.parse_type("field type", false) else {
                self.skip_line();
                continue;
            };
            let field_name = match self.bump().token {
                Token::Ident(name) => name,
                other => {
                    self.record(
                        format!("expected a field name, found {}", other.describe()),
                        self.peek().span,
                    );
                    self.skip_line();
                    continue;
                }
            };
            if !is_camel_case(&field_name) {
                self.record(
                    format!("field `{field_name}` of struct `{name}` must be camelCase (§2.5)"),
                    self.peek().span,
                );
            }
            if field_names.contains(&field_name) {
                self.record(
                    format!("duplicate field `{field_name}` in struct `{name}`"),
                    self.peek().span,
                );
            } else {
                field_names.push(field_name.clone());
            }
            if !matches!(self.peek().token, Token::Newline | Token::Punct('}')) {
                let span = self.peek().span;
                self.record(
                    format!(
                        "expected a new line after field `{field_name}`, found {}",
                        self.peek().token.describe()
                    ),
                    span,
                );
                self.skip_line();
            }
            fields.push(FieldDef {
                ty,
                name: field_name,
            });
        }
        Some(SchemaItem::Struct(SchemaStruct {
            name,
            fields,
            span: Span::new(start, end),
        }))
    }

    /// A schema `enum` (§9.3, §2.7 shape): newline-delimited variants with
    /// comma-separated typed payloads.
    fn parse_enum(&mut self, start: usize, seen: &mut Vec<String>) -> Option<SchemaItem> {
        self.bump(); // `enum`
        let name = match self.bump().token {
            Token::Ident(name) => name,
            other => {
                self.record(
                    format!("expected the enum name, found {}", other.describe()),
                    self.peek().span,
                );
                self.skip_line();
                return None;
            }
        };
        self.check_declared_name("enum", &name, seen);
        if !self.at_punct('{') {
            let span = self.peek().span;
            self.record(
                format!(
                    "expected `{{` to open enum `{name}`, found {}",
                    self.peek().token.describe()
                ),
                span,
            );
            self.skip_line();
            return None;
        }
        self.bump();
        let mut variants: Vec<VariantDecl> = Vec::new();
        let mut variant_names: Vec<String> = Vec::new();
        let end;
        loop {
            self.skip_newlines();
            if self.at_punct('}') {
                end = self.bump().span.end;
                break;
            }
            if matches!(self.peek().token, Token::Eof) {
                self.record(
                    format!("expected `}}` before end of file in enum `{name}`"),
                    self.peek().span,
                );
                end = self.peek().span.end;
                break;
            }
            let variant_name = match self.bump().token {
                Token::Ident(name) => name,
                other => {
                    self.record(
                        format!("expected a variant name, found {}", other.describe()),
                        self.peek().span,
                    );
                    self.skip_line();
                    continue;
                }
            };
            if !is_pascal_case(&variant_name) {
                self.record(
                    format!(
                        "variant `{variant_name}` of enum `{name}` must be PascalCase \
                         (variant payloads cross the boundary, §2.5/§2.7)"
                    ),
                    self.peek().span,
                );
            }
            if variant_names.contains(&variant_name) {
                self.record(
                    format!("duplicate variant `{variant_name}` in enum `{name}`"),
                    self.peek().span,
                );
            } else {
                variant_names.push(variant_name.clone());
            }
            let mut fields: Vec<FieldDef> = Vec::new();
            if self.at_punct('(') {
                self.bump();
                if !self.at_punct(')') {
                    // Variant payloads are fields (§2.7): the parameter
                    // reader shares the `TYPE name` grammar, so the shapes
                    // convert.
                    fields = self
                        .parse_params(&format!("{name}.{variant_name}"))
                        .into_iter()
                        .map(|param| FieldDef {
                            ty: param.ty,
                            name: param.name,
                        })
                        .collect();
                }
                if !self.at_punct(')') {
                    let span = self.peek().span;
                    self.record(
                        format!(
                            "expected `)` after the payload of `{name}.{variant_name}`, found {}",
                            self.peek().token.describe()
                        ),
                        span,
                    );
                    self.skip_line();
                    continue;
                }
                self.bump();
            }
            if !matches!(self.peek().token, Token::Newline | Token::Punct('}')) {
                let span = self.peek().span;
                self.record(
                    format!(
                        "expected a new line after variant `{variant_name}`, found {}",
                        self.peek().token.describe()
                    ),
                    span,
                );
                self.skip_line();
            }
            variants.push(VariantDecl {
                name: variant_name,
                fields,
            });
        }
        Some(SchemaItem::Enum(SchemaEnum {
            name,
            variants,
            span: Span::new(start, end),
        }))
    }

    fn check_declared_name(&mut self, kind: &str, name: &str, seen: &mut Vec<String>) {
        if !is_pascal_case(name) {
            self.record(
                format!(
                    "schema {kind} `{name}` must be PascalCase: boundary types cross the \
                     host/script boundary (§2.5, §9.3)"
                ),
                self.peek().span,
            );
        }
        if seen.contains(&name.to_string()) {
            self.record(
                format!("duplicate schema declaration `{name}`"),
                self.peek().span,
            );
        } else {
            seen.push(name.to_string());
        }
    }

    /// A schema type (§9.3): the built-in scalars, declared names with
    /// optional generic arguments (`option<str>`, `result<T, E>`), arrays
    /// (`T[]`), and maps (`map<K, V>`). `allow_void` is true only in
    /// return position (§2.4).
    fn parse_type(&mut self, context: &str, allow_void: bool) -> Option<Type> {
        let base = match self.bump().token {
            Token::Ident(name) => match name.as_str() {
                "int" => Type::Prim(PrimitiveType::Int),
                "float" => Type::Prim(PrimitiveType::Float),
                "bool" => Type::Prim(PrimitiveType::Bool),
                "str" => Type::Prim(PrimitiveType::Str),
                "void" => {
                    if !allow_void {
                        self.record(
                            "`void` is only valid as a member's return type, never as a \
                             parameter or field type (§2.4)",
                            self.peek().span,
                        );
                    }
                    Type::Void
                }
                "map" => {
                    // `map<K, V>` (§11): the one built-in generic with two
                    // parameters and its own spelling.
                    if !self.at_punct('<') {
                        self.record(
                            format!(
                                "expected `<` after the map type in the {context}, found {}",
                                self.peek().token.describe()
                            ),
                            self.peek().span,
                        );
                        return None;
                    }
                    self.bump();
                    let key = self.parse_type("map key type", false)?;
                    if !self.at_punct(',') {
                        self.record(
                            format!(
                                "expected `,` between the map key and value types, found {}",
                                self.peek().token.describe()
                            ),
                            self.peek().span,
                        );
                        return None;
                    }
                    self.bump();
                    let value = self.parse_type("map value type", false)?;
                    if !self.at_punct('>') {
                        self.record(
                            format!(
                                "expected `>` after the map type, found {}",
                                self.peek().token.describe()
                            ),
                            self.peek().span,
                        );
                        return None;
                    }
                    self.bump();
                    Type::Map {
                        key: Box::new(key),
                        value: Box::new(value),
                    }
                }
                _ => {
                    let args = if self.at_punct('<') {
                        self.bump();
                        let mut args =
                            vec![self.parse_type(&format!("type argument of `{name}`"), false)?];
                        while self.at_punct(',') {
                            self.bump();
                            args.push(
                                self.parse_type(&format!("type argument of `{name}`"), false)?,
                            );
                        }
                        if !self.at_punct('>') {
                            self.record(
                                format!(
                                    "expected `>` after the type arguments of `{name}`, found {}",
                                    self.peek().token.describe()
                                ),
                                self.peek().span,
                            );
                            return None;
                        }
                        self.bump();
                        args
                    } else {
                        Vec::new()
                    };
                    Type::Named { name, args }
                }
            },
            other => {
                self.record(
                    format!(
                        "expected a type for the {context}, found {}",
                        other.describe()
                    ),
                    self.peek().span,
                );
                return None;
            }
        };
        Some(self.parse_type_suffix(base, context))
    }

    /// Applies trailing `[]` suffixes: `str[]`, `int[][]`.
    fn parse_type_suffix(&mut self, mut base: Type, context: &str) -> Type {
        while self.at_punct('[') {
            // `[]` is the only array shape the grammar allows; the scanner
            // has no `]` bracket beyond these.
            self.bump();
            if !self.at_punct(']') {
                self.record(
                    format!(
                        "expected `]` to close the array type in the {context}, found {}",
                        self.peek().token.describe()
                    ),
                    self.peek().span,
                );
                return base;
            }
            self.bump();
            base = Type::Array(Box::new(base));
        }
        base
    }

    fn expect_line_end(&mut self, context: &str) {
        match &self.peek().token {
            Token::Newline => {
                self.bump();
            }
            Token::Eof => {}
            other => {
                let span = self.peek().span;
                self.record(
                    format!(
                        "expected a new line after {context}, found {}",
                        other.describe()
                    ),
                    span,
                );
                self.skip_line();
            }
        }
    }
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

/// Synthesizes the §9.3 boundary-type declarations of every GRANTED
/// namespace into AST statements: scripts construct schema structs and
/// enums exactly like local ones, so every execution engine (the tree
/// walker now, the bytecode VM and AOT engines later) receives their
/// shapes through the one contract it already consumes — the AST.
pub fn declaration_statements(context: &SchemaContext) -> Vec<Stmt> {
    let mut declarations = Vec::new();
    for file in context.set.namespaces() {
        if context.target(&file.namespace).is_none() {
            continue;
        }
        for item in &file.items {
            match item {
                SchemaItem::Struct(decl) => declarations.push(Stmt {
                    span: Span::new(0, 0),
                    kind: StmtKind::StructDecl {
                        name: decl.name.clone(),
                        type_params: Vec::new(),
                        fields: decl.fields.clone(),
                    },
                }),
                SchemaItem::Enum(decl) => declarations.push(Stmt {
                    span: Span::new(0, 0),
                    kind: StmtKind::EnumDecl {
                        name: decl.name.clone(),
                        type_params: Vec::new(),
                        variants: decl.variants.clone(),
                    },
                }),
                SchemaItem::Contract(_) => {}
            }
        }
    }
    declarations
}

// ---------------------------------------------------------------------------
// SchemaSet — multiple namespaces, cross-file invariants
// ---------------------------------------------------------------------------

/// A cross-file schema problem: not anchored to one file's spans the way a
/// parse diagnostic is (it may compare two namespaces), so it carries the
/// owning namespace for rendering instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaIssue {
    pub message: String,
    /// The namespace whose file the host should blame (rendered against its
    /// text); `None` when the issue is about the set as a whole.
    pub namespace: Option<String>,
}

/// The host's registered schema surface (§9.2): every namespace root the
/// host ships, checked as a set. Built once per engine; the script-side
/// checker and the generated bindings both read from it.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaSet {
    namespaces: Vec<SchemaFile>,
}

impl SchemaSet {
    /// Assembles the set and enforces the cross-file invariants:
    ///
    /// - one file per namespace (§9.2: "one schema file represents exactly
    ///   one namespace root");
    /// - declared type names are unique across namespaces — the
    ///   script-side type space is flat, so two `Vec2`s from different
    ///   roots could never be disambiguated;
    /// - every `requires` edge (§9.4) resolves to an interface of the
    ///   declared namespace or a qualified other one.
    pub fn build(files: Vec<SchemaFile>) -> Result<SchemaSet, Vec<SchemaIssue>> {
        let mut issues: Vec<SchemaIssue> = Vec::new();
        let mut sorted = files;
        sorted.sort_by(|a, b| a.namespace.cmp(&b.namespace));

        let mut seen_namespaces: Vec<&str> = Vec::new();
        let mut type_owner: BTreeMap<&str, &str> = BTreeMap::new();
        for file in &sorted {
            if seen_namespaces.contains(&file.namespace.as_str()) {
                issues.push(SchemaIssue {
                    message: format!(
                        "duplicate schema namespace `{}`: one schema file represents exactly \
                         one namespace root (§9.2)",
                        file.namespace
                    ),
                    namespace: Some(file.namespace.clone()),
                });
            } else {
                seen_namespaces.push(&file.namespace);
            }
            for item in &file.items {
                if matches!(item, SchemaItem::Struct(_) | SchemaItem::Enum(_)) {
                    if let Some(owner) = type_owner.get(item.name()) {
                        issues.push(SchemaIssue {
                            message: format!(
                                "schema type `{}` is declared by both `{}` and `{}`: the \
                                 script-side type space is flat, so boundary type names must \
                                 be unique across namespaces",
                                item.name(),
                                owner,
                                file.namespace
                            ),
                            namespace: Some(file.namespace.clone()),
                        });
                    } else {
                        type_owner.insert(item.name(), &file.namespace);
                    }
                }
            }
        }

        let set = SchemaSet { namespaces: sorted };
        for file in set.namespaces.clone() {
            for contract in file.contracts() {
                // §9.5: a member's `since` tag places it on the schema's own
                // version timeline. A member introduced AFTER the version the
                // schema declares can never be visible (no target may exceed
                // the schema's own version), so it is a schema-authoring bug:
                // the version bump was forgotten.
                for member in &contract.members {
                    if member.since > file.version {
                        issues.push(SchemaIssue {
                            message: format!(
                                "{} `{}.{}` member `{}` is tagged `since {}`, but the schema \
                                 declares {} — a member cannot be introduced after the \
                                 schema version that carries it (§9.5)",
                                contract.kind.keyword(),
                                file.namespace,
                                contract.name,
                                member.name,
                                member.since,
                                file.version
                            ),
                            namespace: Some(file.namespace.clone()),
                        });
                    }
                }
                let Some(requires) = &contract.requires else {
                    continue;
                };
                let target = requires.qualified(&file.namespace);
                let resolved = match target.split_once('.') {
                    Some((ns, name)) => set
                        .namespace(ns)
                        .and_then(|other| other.interface(name))
                        .is_some(),
                    None => false,
                };
                if !resolved {
                    issues.push(SchemaIssue {
                        message: format!(
                            "{} `{}.{}` requires `{target}`, which is not an interface of any \
                             registered schema (§9.4)",
                            contract.kind.keyword(),
                            file.namespace,
                            contract.name
                        ),
                        namespace: Some(file.namespace.clone()),
                    });
                }
            }
        }

        if issues.is_empty() {
            Ok(set)
        } else {
            Err(issues)
        }
    }

    /// An empty set: no schema contract is active.
    pub fn empty() -> SchemaSet {
        SchemaSet {
            namespaces: Vec::new(),
        }
    }

    pub fn namespace(&self, name: &str) -> Option<&SchemaFile> {
        self.namespaces.iter().find(|file| file.namespace == name)
    }

    pub fn namespaces(&self) -> &[SchemaFile] {
        &self.namespaces
    }

    pub fn is_empty(&self) -> bool {
        self.namespaces.is_empty()
    }
}

/// A schema set plus the per-namespace TARGET versions scripts are checked
/// against (§9.5). A namespace absent from `targets` is invisible to the
/// program — the §7.2 capability sandbox: only namespaces the host granted
/// (via a mod manifest's `[schemas]`, or every namespace for loose sources)
/// can be imported.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaContext {
    pub set: SchemaSet,
    targets: BTreeMap<String, Version>,
}

impl SchemaContext {
    /// Every namespace visible at its own declared version — the loose
    /// source default, where no manifest narrows the grant.
    pub fn grant_all(set: SchemaSet) -> SchemaContext {
        let targets = set
            .namespaces()
            .iter()
            .map(|file| (file.namespace.clone(), file.version))
            .collect();
        SchemaContext { set, targets }
    }

    /// Explicit targets (a mod manifest's `[schemas]` table, §10.2/§9.5).
    /// Every named namespace must exist in the set, and no target may
    /// exceed the schema's own version — a mod cannot claim a newer host
    /// than the one that is present.
    pub fn grant_targets(
        set: SchemaSet,
        targets: Vec<(String, String)>,
    ) -> Result<SchemaContext, Vec<SchemaIssue>> {
        let mut issues = Vec::new();
        let mut resolved: BTreeMap<String, Version> = BTreeMap::new();
        for (namespace, version) in targets {
            let Some(file) = set.namespace(&namespace) else {
                issues.push(SchemaIssue {
                    message: format!(
                        "the mod declares schema `{namespace}`, which the host has not \
                         registered; add the schema file or drop it from `[schemas]`"
                    ),
                    namespace: Some(namespace.clone()),
                });
                continue;
            };
            match Version::parse(&version) {
                Some(target) if target <= file.version => {
                    resolved.insert(namespace, target);
                }
                Some(target) => {
                    issues.push(SchemaIssue {
                        message: format!(
                            "the mod targets `{namespace}` {target}, but the host provides \
                             {only} — a mod cannot target a newer schema than the host ships",
                            only = file.version
                        ),
                        namespace: Some(namespace),
                    });
                }
                None => {
                    issues.push(SchemaIssue {
                        message: format!(
                            "schema target version for `{namespace}` must be `X.Y.Z`, got {version:?}"
                        ),
                        namespace: Some(namespace),
                    });
                }
            }
        }
        if issues.is_empty() {
            Ok(SchemaContext {
                set,
                targets: resolved,
            })
        } else {
            Err(issues)
        }
    }

    /// The visible target version of `namespace`, or `None` when the host
    /// did not grant it to this program.
    pub fn target(&self, namespace: &str) -> Option<Version> {
        self.targets.get(namespace).copied()
    }

    /// Whether `namespace` is visible at all.
    pub fn grants(&self, namespace: &str) -> bool {
        self.targets.contains_key(namespace)
    }
}

#[cfg(test)]
mod tests;
