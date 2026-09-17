//! Schema-file authoring features (§9): completion, hover, and document
//! symbols for `.cm` schema documents.
//!
//! A schema file is its own small language (the dedicated scanner + parser
//! in `cme_compiler::schema`), so it gets its own analysis here — built
//! from the same recognition the parser uses, never disagreeing with it.
//! The contexts:
//!
//! - **Top level** — the five declaration keywords plus the block version
//!   tag `since` (§9.5), and the contract keyword after a block version;
//! - **contract body** — `optional` (interfaces only, §9.5), and return
//!   types (built-ins + the file's own types);
//! - **parameter and field positions** — types without `void`;
//! - **hover** — every declaration renders from the parsed [`SchemaFile`].
//!
//! The parse is tolerant: features answer from whatever recovered, and a
//! broken file still completes (authoring a fix is exactly when help
//! matters most).

use cme_compiler::schema::{
    ContractKind, MemberRequirement, SchemaFile, SchemaToken, SchemaTokenKind, parse_schema_file,
    schema_tokens,
};
use tower_lsp_server::ls_types;

use crate::convert::LineIndex;

/// The precomputed analysis of one schema document revision.
pub struct SchemaDoc {
    tokens: Vec<SchemaToken>,
    file: Option<SchemaFile>,
}

impl SchemaDoc {
    /// Parses one schema document (tolerantly).
    pub fn build(source: &str) -> SchemaDoc {
        SchemaDoc {
            tokens: schema_tokens(source),
            file: parse_schema_file(source).file,
        }
    }

    /// The parsed (possibly partial) file.
    fn file(&self) -> Option<&SchemaFile> {
        self.file.as_ref()
    }

    /// The token containing `offset` (strictly, then boundaries).
    fn token_at(&self, offset: usize) -> Option<&SchemaToken> {
        self.tokens
            .iter()
            .find(|token| token.span.start < offset && offset < token.span.end)
            .or_else(|| {
                self.tokens
                    .iter()
                    .find(|token| token.span.start <= offset && offset < token.span.end)
            })
    }

    /// The identifier spelling at `offset`, if any.
    fn ident_at(&self, offset: usize) -> Option<&str> {
        match self.token_at(offset)? {
            SchemaToken {
                kind: SchemaTokenKind::Ident(name),
                ..
            } => Some(name.as_str()),
            _ => None,
        }
    }

    /// The tokens strictly before `offset` on the current line.
    fn line_tokens_before(&self, offset: usize) -> Vec<&SchemaToken> {
        let line_start = self.tokens.iter().rev().find_map(|token| {
            (token.kind == SchemaTokenKind::Newline && token.span.end <= offset)
                .then_some(token.span.end)
        });
        let start = line_start.unwrap_or(0);
        self.tokens
            .iter()
            .filter(|token| token.span.start >= start && token.span.end <= offset)
            .collect()
    }

    /// The braces opened before `offset` (positive depth).
    fn brace_depth_at(&self, offset: usize) -> usize {
        let mut depth = 0usize;
        for token in &self.tokens {
            if token.span.start >= offset {
                break;
            }
            match token.kind {
                SchemaTokenKind::Punct('{') => depth += 1,
                SchemaTokenKind::Punct('}') => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        depth
    }

    /// The depth-0 declaration keyword enclosing `offset`, when the cursor
    /// sits inside a body: `"capability"`, `"interface"`, `"struct"`, or
    /// `"enum"`.
    fn enclosing_decl_kind(&self, offset: usize) -> Option<&'static str> {
        let mut depth = 0usize;
        let mut keyword: Option<&'static str> = None;
        for token in &self.tokens {
            if token.span.start >= offset {
                break;
            }
            match &token.kind {
                SchemaTokenKind::Punct('{') => depth += 1,
                SchemaTokenKind::Punct('}') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        keyword = None;
                    }
                }
                SchemaTokenKind::Ident(name) if depth == 0 => {
                    keyword = match name.as_str() {
                        "capability" => Some("capability"),
                        "interface" => Some("interface"),
                        "struct" => Some("struct"),
                        "enum" => Some("enum"),
                        _ => keyword,
                    };
                }
                _ => {}
            }
        }
        keyword.filter(|_| depth > 0)
    }

    /// Whether the cursor is inside a `( … )` parameter list.
    fn inside_param_list(&self, offset: usize) -> bool {
        let mut depth = 0i32;
        for token in &self.tokens {
            if token.span.start >= offset {
                break;
            }
            match token.kind {
                SchemaTokenKind::Punct('(') => depth += 1,
                SchemaTokenKind::Punct(')') => depth -= 1,
                _ => {}
            }
        }
        depth > 0
    }
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

/// The declaration keywords of the top level (§9.1, §9.3, §9.5).
const TOP_LEVEL_KEYWORDS: [(&str, &str); 6] = [
    (
        "schema",
        "namespace root header: `schema <name> <X.Y.Z>` (§9.1)",
    ),
    (
        "capability",
        "host-provided functions the script imports and calls (§9.1)",
    ),
    (
        "interface",
        "script-implemented functions the host calls into (§9.1)",
    ),
    ("struct", "boundary data type shared across the FFI (§9.3)"),
    ("enum", "boundary tagged union shared across the FFI (§9.3)"),
    (
        "since",
        "version the members of the capability/interface block that \
         follows: `since X.Y.Z capability name { ... }` (§9.5)",
    ),
];

/// The contract keywords that may follow a block-level `since` version.
const CONTRACT_KEYWORDS: [(&str, &str); 2] = [
    (
        "capability",
        "host-provided functions the script imports and calls (§9.1)",
    ),
    (
        "interface",
        "script-implemented functions the host calls into (§9.1)",
    ),
];

/// The scalar types every schema type position accepts (§2.4, §9.3).
const BUILTIN_TYPES: [(&str, &str); 5] = [
    ("int", "signed 64-bit integer (§2.4)"),
    ("float", "64-bit IEEE 754 float (§2.4)"),
    ("bool", "`true` or `false` (§2.4)"),
    ("str", "immutable UTF-8 string (§2.4)"),
    ("void", "no-value return — return position only (§2.4)"),
];

/// Computes completions at `offset` for a schema document. The decision
/// ladder, most specific first:
///
/// 1. inside a `( … )` parameter list → parameter types (§2.11);
/// 2. a `since` line — at the top level the block version tag (§9.5):
///    nothing while typing the version, the contract keyword after it;
///    inside a body it is the retired member spelling, and the return
///    type still completes after the version (recovery quality);
/// 3. a member line after its `optional` modifier → return types
///    (§9.1, §9.5);
/// 4. the first word of the line decides: declaration names are free-form,
///    and an otherwise-empty line offers its enclosing context's shape.
pub fn completions(doc: &SchemaDoc, text: &str, offset: usize) -> Vec<ls_types::CompletionItem> {
    if in_comment(text, offset) {
        return Vec::new();
    }
    let line = doc.line_tokens_before(offset);
    let first_word = line.first().and_then(|token| match &token.kind {
        SchemaTokenKind::Ident(name) => Some(name.as_str()),
        _ => None,
    });

    // 1. Parameter lists own their contents entirely (§2.11).
    if doc.inside_param_list(offset) {
        return type_items(doc, false);
    }

    // 2. A `since` line: block tag at the top level, retired member
    // spelling inside a body.
    if first_word == Some("since") {
        let version_typed = line.len() >= 2 && line[1].kind == SchemaTokenKind::Version;
        if doc.brace_depth_at(offset) == 0 {
            // `since X.Y.Z` done — the contract keyword follows (§9.5).
            if version_typed {
                return keyword_items(&CONTRACT_KEYWORDS);
            }
            // Typing the version: versions are typed, not completed.
            return Vec::new();
        }
        if version_typed {
            return type_items(doc, true);
        }
        return Vec::new();
    }

    // 3. A member line whose `optional` modifier is complete: the return
    // type.
    if let Some(ModifierPoint::AfterOptional) = member_after_modifiers(&line) {
        return type_items(doc, true);
    }

    // 4. The enclosing shape decides an otherwise-unchosen line.
    match (first_word, doc.brace_depth_at(offset) == 0) {
        // Top level: the declaration keywords + the block version tag.
        (None, true) => keyword_items(&TOP_LEVEL_KEYWORDS),
        // Typing a declaration's name: the name is free-form.
        (Some("schema" | "capability" | "interface" | "struct" | "enum"), _) => Vec::new(),
        // Inside a contract body: the member shape.
        (None, false) | (Some("void"), false) => match doc.enclosing_decl_kind(offset) {
            Some("capability") => member_start_items(doc, false),
            Some("interface") => member_start_items(doc, true),
            Some("struct") => type_items(doc, false),
            // Enum variants are PascalCase names, not types.
            _ => Vec::new(),
        },
        // Anything else (unknown words at top level, enum bodies): nothing.
        _ => Vec::new(),
    }
}

/// How far the member line's modifier sequence has progressed.
enum ModifierPoint {
    /// `optional` is present; the return type follows.
    AfterOptional,
}

fn member_after_modifiers(line: &[&SchemaToken]) -> Option<ModifierPoint> {
    if !line.is_empty()
        && matches!(&line[0].kind, SchemaTokenKind::Ident(name) if name == "optional")
    {
        return Some(ModifierPoint::AfterOptional);
    }
    None
}

/// The member-start completions inside a contract body: the `optional`
/// modifier (interfaces only) plus the return-type candidates (§9.1,
/// §9.5). `since` is a top-level block tag now, not a member modifier.
fn member_start_items(doc: &SchemaDoc, is_interface: bool) -> Vec<ls_types::CompletionItem> {
    let mut items = Vec::new();
    if is_interface {
        items.push(modifier_item());
    }
    items.extend(type_items(doc, true));
    items
}

/// The type names valid at the cursor: built-ins plus the file's own
/// declared types (§9.3). `allow_void` marks a return position.
fn type_items(doc: &SchemaDoc, allow_void: bool) -> Vec<ls_types::CompletionItem> {
    let mut items = Vec::new();
    for (name, detail) in BUILTIN_TYPES {
        if name == "void" && !allow_void {
            continue;
        }
        items.push(item(
            name.to_string(),
            ls_types::CompletionItemKind::KEYWORD,
            detail.to_string(),
            None,
        ));
    }
    if let Some(file) = doc.file() {
        for decl in &file.items {
            let (name, detail) = match decl {
                cme_compiler::schema::SchemaItem::Struct(decl) => (
                    decl.name.clone(),
                    format!("struct {{ {} fields }} (§9.3)", decl.fields.len()),
                ),
                cme_compiler::schema::SchemaItem::Enum(decl) => (
                    decl.name.clone(),
                    format!("enum {{ {} variants }} (§9.3)", decl.variants.len()),
                ),
                _ => continue,
            };
            items.push(item(
                name,
                ls_types::CompletionItemKind::STRUCT,
                detail,
                None,
            ));
        }
    }
    items
}

fn keyword_items(keywords: &[(&str, &str)]) -> Vec<ls_types::CompletionItem> {
    keywords
        .iter()
        .map(|(name, detail)| {
            item(
                name.to_string(),
                ls_types::CompletionItemKind::KEYWORD,
                detail.to_string(),
                None,
            )
        })
        .collect()
}

/// The `optional` modifier as a completion with its fill.
fn modifier_item() -> ls_types::CompletionItem {
    let mut entry = item(
        "optional".to_string(),
        ls_types::CompletionItemKind::KEYWORD,
        "a mod may skip this member without breaking (§9.5)".to_string(),
        Some("optional ".to_string()),
    );
    entry.sort_text = Some("0optional".to_string());
    entry
}

fn item(
    label: String,
    kind: ls_types::CompletionItemKind,
    detail: String,
    insert: Option<String>,
) -> ls_types::CompletionItem {
    ls_types::CompletionItem {
        label,
        kind: Some(kind),
        detail: Some(detail),
        insert_text: insert,
        ..ls_types::CompletionItem::default()
    }
}

/// True when `offset` sits in a `//` or `/* … */` comment. The schema
/// scanner skips comments, so this is a plain text scan.
fn in_comment(text: &str, offset: usize) -> bool {
    let prefix = &text[..offset];
    let line_start = prefix.rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    if prefix[line_start..].contains("//") {
        return true;
    }
    match prefix.rfind("/*") {
        Some(opener) => !text[opener + 2..].contains("*/"),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Hover
// ---------------------------------------------------------------------------

/// Builds the hover response for `offset`, if something resolvable is
/// there.
pub fn hover(doc: &SchemaDoc, offset: usize) -> Option<ls_types::Hover> {
    let name = doc.ident_at(offset)?;
    let file = doc.file()?;

    // The header: hovering the namespace names the contract's root.
    let mut markdown = if file.namespace == name {
        Some(format!(
            "```checkmate\nschema {} {}\n```\n*§9.1 namespace root — {} declaration(s)*",
            file.namespace,
            file.version,
            file.items.len()
        ))
    } else {
        None
    };

    for item in &file.items {
        match item {
            cme_compiler::schema::SchemaItem::Struct(decl) if decl.name == name => {
                let fields: Vec<String> = decl
                    .fields
                    .iter()
                    .map(|field| format!("    {}: {}", field.name, render_type(&field.ty)))
                    .collect();
                markdown = Some(format!(
                    "```checkmate\nstruct {} {{\n{}\n}}\n```\n*§9.3 boundary type — {} field(s)*",
                    decl.name,
                    fields.join("\n"),
                    fields.len()
                ));
                break;
            }
            cme_compiler::schema::SchemaItem::Enum(decl) if decl.name == name => {
                let variants: Vec<String> = decl
                    .variants
                    .iter()
                    .map(|variant| {
                        let payload: Vec<String> = variant
                            .fields
                            .iter()
                            .map(|field| format!("{}: {}", field.name, render_type(&field.ty)))
                            .collect();
                        if payload.is_empty() {
                            format!("    {}()", variant.name)
                        } else {
                            format!("    {}({})", variant.name, payload.join(", "))
                        }
                    })
                    .collect();
                markdown = Some(format!(
                    "```checkmate\nenum {} {{\n{}\n}}\n```\n*§9.3 boundary type — {} variant(s)*",
                    decl.name,
                    variants.join("\n"),
                    variants.len()
                ));
                break;
            }
            cme_compiler::schema::SchemaItem::Contract(decl) => {
                if decl.name == name {
                    let head = match decl.kind {
                        ContractKind::Capability => "capability",
                        ContractKind::Interface => "interface",
                    };
                    let members: Vec<String> = decl
                        .members
                        .iter()
                        .map(|member| {
                            let params: Vec<String> = member
                                .params
                                .iter()
                                .map(|param| format!("{} {}", render_type(&param.ty), param.name))
                                .collect();
                            let mut tags = String::new();
                            if member.requirement == MemberRequirement::Optional {
                                tags.push_str(" optional");
                            }
                            format!(
                                "    since {} {} {}({}){}",
                                member.since,
                                render_type(&member.return_ty),
                                member.name,
                                params.join(", "),
                                tags
                            )
                        })
                        .collect();
                    let requires = decl
                        .requires
                        .as_ref()
                        .map(|path| format!("\n    requires {}", path.segments.join(".")));
                    markdown = Some(format!(
                        "```checkmate\n{head} {} {{{}\n}}\n```\n*{} member(s) — §9.1*",
                        decl.name,
                        requires.unwrap_or_default(),
                        members.join("\n")
                    ));
                    break;
                }
                // A member of this contract.
                if let Some(member) = decl.members.iter().find(|member| member.name == name) {
                    let params: Vec<String> = member
                        .params
                        .iter()
                        .map(|param| format!("{} {}", render_type(&param.ty), param.name))
                        .collect();
                    let mut notes = vec![
                        format!("since {}", member.since),
                        decl.kind.keyword().to_string(),
                    ];
                    if member.requirement == MemberRequirement::Optional {
                        notes.push("optional".to_string());
                    }
                    markdown = Some(format!(
                        "```checkmate\n{} {}.{}({})\n```\n*{}*",
                        render_type(&member.return_ty),
                        decl.name,
                        member.name,
                        params.join(", "),
                        notes.join(", ")
                    ));
                    break;
                }
            }
            _ => {}
        }
    }

    // Built-in scalar types.
    markdown = markdown.or_else(|| {
        BUILTIN_TYPES
            .iter()
            .find(|(builtin, _)| *builtin == name)
            .map(|(name, detail)| format!("```checkmate\n{name}\n```\n{detail}"))
    });

    markdown.map(|value| ls_types::Hover {
        contents: ls_types::HoverContents::Markup(ls_types::MarkupContent {
            kind: ls_types::MarkupKind::Markdown,
            value,
        }),
        range: None,
    })
}

/// Renders a declared type back to schema source form.
fn render_type(ty: &cme_core::ast::Type) -> String {
    use cme_core::ast::PrimitiveType;
    match ty {
        cme_core::ast::Type::Infer => "infer".to_string(),
        cme_core::ast::Type::Prim(PrimitiveType::Int) => "int".to_string(),
        cme_core::ast::Type::Prim(PrimitiveType::Float) => "float".to_string(),
        cme_core::ast::Type::Prim(PrimitiveType::Bool) => "bool".to_string(),
        cme_core::ast::Type::Prim(PrimitiveType::Str) => "str".to_string(),
        cme_core::ast::Type::Void => "void".to_string(),
        cme_core::ast::Type::Named { name, args } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner: Vec<String> = args.iter().map(render_type).collect();
                format!("{name}<{}>", inner.join(", "))
            }
        }
        cme_core::ast::Type::Array(elem) => format!("{}[]", render_type(elem)),
        cme_core::ast::Type::Map { key, value } => {
            format!("map<{}, {}>", render_type(key), render_type(value))
        }
    }
}

// ---------------------------------------------------------------------------
// Document symbols
// ---------------------------------------------------------------------------

/// The outline of a schema document: every declaration with its members as
/// children (§9.1, §9.3).
pub fn document_symbols(
    doc: &SchemaDoc,
    line_index: &LineIndex,
    text: &str,
) -> Vec<ls_types::DocumentSymbol> {
    let range = |span: cme_core::Span| line_index.range(text, span);
    let Some(file) = doc.file() else {
        return Vec::new();
    };
    let mut symbols = Vec::new();
    for item in &file.items {
        match item {
            cme_compiler::schema::SchemaItem::Struct(decl) => symbols.push(symbol(
                decl.name.clone(),
                ls_types::SymbolKind::STRUCT,
                range(decl.span),
                decl.fields
                    .iter()
                    .map(|field| {
                        symbol(
                            field.name.clone(),
                            ls_types::SymbolKind::FIELD,
                            range(decl.span),
                            Vec::new(),
                        )
                    })
                    .collect(),
            )),
            cme_compiler::schema::SchemaItem::Enum(decl) => symbols.push(symbol(
                decl.name.clone(),
                ls_types::SymbolKind::ENUM,
                range(decl.span),
                decl.variants
                    .iter()
                    .map(|variant| {
                        symbol(
                            variant.name.clone(),
                            ls_types::SymbolKind::ENUM_MEMBER,
                            range(decl.span),
                            Vec::new(),
                        )
                    })
                    .collect(),
            )),
            cme_compiler::schema::SchemaItem::Contract(decl) => symbols.push(symbol(
                decl.name.clone(),
                match decl.kind {
                    ContractKind::Capability => ls_types::SymbolKind::INTERFACE,
                    ContractKind::Interface => ls_types::SymbolKind::INTERFACE,
                },
                range(decl.span),
                decl.members
                    .iter()
                    .map(|member| {
                        symbol(
                            member.name.clone(),
                            ls_types::SymbolKind::METHOD,
                            range(member.span),
                            Vec::new(),
                        )
                    })
                    .collect(),
            )),
        }
    }
    symbols
}

fn symbol(
    name: String,
    kind: ls_types::SymbolKind,
    range: ls_types::Range,
    children: Vec<ls_types::DocumentSymbol>,
) -> ls_types::DocumentSymbol {
    let children = if children.is_empty() {
        None
    } else {
        Some(children)
    };
    #[allow(deprecated)] // the field is mandatory in this ls-types revision
    ls_types::DocumentSymbol {
        name,
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children,
    }
}
