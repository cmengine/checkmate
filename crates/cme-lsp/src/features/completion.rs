//! Completion: context-sensitive suggestions.
//!
//! Contexts, in priority order:
//!
//! 1. **Member access** (`player.`): struct fields with their types, enum
//!    variants after the enum's name, and the array `.length` (§11).
//! 2. **Argument lists** (`vec2(` or `movePlayer(`): named arguments
//!    (§2.12) from struct fields or function parameters, skipping names
//!    already present.
//! 3. **Import paths** (§2.3): the segments seen in other `import`
//!    statements of this file.
//! 4. **Statement/expression position**: keywords, locals in scope, and
//!    every top-level declaration.
//!
//! All of it is derived from the same analysis the checker accepts, so
//! completions never contradict the compiler.

use tower_lsp_server::ls_types;

use crate::analysis::{Analysis, Tok, render_type};
use crate::resolve;

/// Computes completions at `offset`.
pub fn completions(
    analysis: &Analysis<'_>,
    text: &str,
    offset: usize,
) -> Vec<ls_types::CompletionItem> {
    // Typing right after a `.` — including when the cursor sits past the
    // end of a line — is member context, detected from the text itself so
    // clamped positions still resolve. The dot is the last token whose end
    // does not exceed the cursor.
    if offset > 0
        && text[..offset].ends_with('.')
        && let Some((index, token)) = analysis
            .tokens
            .iter()
            .enumerate()
            .rfind(|(_, token)| token.span.end <= offset)
        && matches!(token.kind, Tok::Dot)
    {
        return member_completions(analysis, index);
    }

    // The token immediately before the cursor decides the remaining
    // contexts.
    let prev = analysis
        .tokens
        .iter()
        .enumerate()
        .rfind(|(_, token)| token.span.end <= offset);

    if let Some((index, token)) = prev
        && matches!(token.kind, Tok::LParen | Tok::Comma)
        && let Some(items) = argument_completions(analysis, text, offset, index)
    {
        return items;
    }

    if let Some(items) = import_completions(analysis, offset) {
        return items;
    }
    scope_completions(analysis, offset)
}

/// Completions after a dot: the receiver's fields, an enum's variants, or
/// the array `.length`.
fn member_completions(analysis: &Analysis<'_>, dot_index: usize) -> Vec<ls_types::CompletionItem> {
    let Some(receiver) = analysis.receiver_range_for_completion(dot_index) else {
        return Vec::new();
    };
    let use_site = analysis
        .tokens
        .get(dot_index + 1)
        .map(|t| t.span.start)
        .unwrap_or(usize::MAX);
    let Some(ty) = analysis.type_of_receiver(receiver, use_site) else {
        return Vec::new();
    };
    match &ty {
        cme_core::ast::Type::Named { name, .. } => {
            if let Some(struct_type) = analysis.structs.iter().find(|s| &s.name == name) {
                return struct_type
                    .fields
                    .iter()
                    .map(|(field_name, field_ty, _)| {
                        item(
                            field_name.clone(),
                            ls_types::CompletionItemKind::FIELD,
                            render_type(field_ty),
                            None,
                        )
                    })
                    .collect();
            }
            if let Some(enum_type) = analysis.enums.iter().find(|e| &e.name == name) {
                return enum_type
                    .variants
                    .iter()
                    .map(|variant| {
                        let payload: Vec<String> = variant
                            .fields
                            .iter()
                            .map(|(name, ty)| format!("{}: {}", name, render_type(ty)))
                            .collect();
                        item(
                            variant.name.clone(),
                            ls_types::CompletionItemKind::ENUM_MEMBER,
                            format!("{}.{}", enum_type.name, variant.name),
                            (!payload.is_empty()).then(|| payload.join(", ")),
                        )
                    })
                    .collect();
            }
            Vec::new()
        }
        cme_core::ast::Type::Array(_) => vec![item(
            "length".to_string(),
            ls_types::CompletionItemKind::FIELD,
            "int — the number of elements (§11)".to_string(),
            None,
        )],
        _ => Vec::new(),
    }
}

/// Named-argument completions inside a call's argument list. Returns `None`
/// when the cursor is not in an argument list.
fn argument_completions(
    analysis: &Analysis<'_>,
    text: &str,
    offset: usize,
    prev_index: usize,
) -> Option<Vec<ls_types::CompletionItem>> {
    // Find the innermost unclosed `(` before the cursor.
    let mut depth = 0i32;
    let mut paren_index = None;
    for index in (0..prev_index).rev() {
        match &analysis.tokens[index].kind {
            Tok::RParen => depth += 1,
            Tok::LParen => {
                if depth == 0 {
                    paren_index = Some(index);
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    let paren_index = paren_index?;

    // The callee is the identifier before the `(`.
    let callee = analysis.tokens.get(paren_index.checked_sub(1)?)?;
    let Tok::Ident(name) = &callee.kind else {
        return None;
    };
    // A callee followed by its own name in a declaration (`int main(`) is
    // not a call site: `(` must be preceded by an identifier that is a
    // use, which is everything we care about; declarations are filtered by
    // the same lookup below.

    // Named and positional arguments are exclusive (§2.12): offering named
    // arguments is only meaningful when no positional argument text exists
    // in the list so far. A `:`-free list that is empty or ends with `,`
    // qualifies; an `=` anywhere means the user is already naming.
    let paren_end = analysis.tokens.get(paren_index).map(|t| t.span.end)?;
    let inside = text.get(paren_end..offset).unwrap_or("");
    if inside.contains('=') || inside.contains(':') {
        return None;
    }
    let used: Vec<&str> = inside
        .split(',')
        .map(|segment| segment.trim())
        .filter_map(|segment| {
            let (name, rest) = segment.split_once(':')?;
            let name = name.trim();
            (!name.is_empty() && rest.trim().is_empty()).then_some(name)
        })
        .collect();

    // Struct construction: field list.
    if let Some(struct_type) = analysis.structs.iter().find(|s| s.name == *name) {
        let items = struct_type
            .fields
            .iter()
            .filter(|(field_name, _, _)| !used.contains(&field_name.as_str()))
            .map(|(field_name, field_ty, _)| {
                item(
                    field_name.clone(),
                    ls_types::CompletionItemKind::FIELD,
                    format!("{}: {}", field_name, render_type(field_ty)),
                    Some(format!("{field_name}: ")),
                )
            })
            .collect();
        return Some(items);
    }
    // Function call: parameter list.
    if let Some(function) = analysis
        .functions
        .iter()
        .find(|f| f.name == *name && f.impl_index.is_none())
    {
        let items = function
            .params
            .iter()
            .filter(|param| !used.contains(&param.name.as_str()))
            .map(|param| {
                item(
                    param.name.clone(),
                    ls_types::CompletionItemKind::VARIABLE,
                    format!("{}: {}", param.name, render_type(&param.ty)),
                    Some(format!("{}: ", param.name)),
                )
            })
            .collect();
        return Some(items);
    }
    None
}

/// Import path completion: `self`, plus the segments seen in this file's
/// other import statements (§2.3).
fn import_completions(
    analysis: &Analysis<'_>,
    offset: usize,
) -> Option<Vec<ls_types::CompletionItem>> {
    let import = analysis
        .imports
        .iter()
        .find(|import| import.span.start <= offset && offset <= import.span.end)?;

    // How deep is the cursor? Count dots before the cursor within the
    // statement.
    let before = analysis
        .tokens
        .iter()
        .take_while(|token| token.span.end <= offset)
        .filter(|token| token.span.start >= import.span.start && matches!(token.kind, Tok::Dot))
        .count();

    let mut candidates: Vec<String> = Vec::new();
    if before == 0 {
        candidates.push("self".to_string());
    }
    for other in &analysis.imports {
        if let Some((name, _)) = other.segments.get(before) {
            candidates.push(name.clone());
        }
    }
    candidates.sort();
    candidates.dedup();
    Some(
        candidates
            .into_iter()
            .map(|name| {
                item(
                    name,
                    ls_types::CompletionItemKind::MODULE,
                    "import path segment".to_string(),
                    None,
                )
            })
            .collect(),
    )
}

/// Statement/expression position: keywords, locals in scope, top-level
/// declarations, and the built-in constructors.
fn scope_completions(analysis: &Analysis<'_>, offset: usize) -> Vec<ls_types::CompletionItem> {
    const KEYWORDS: [(&str, &str); 21] = [
        ("bool", "built-in type (§2.4)"),
        ("else", "conditional alternative (§2.14)"),
        ("enum", "algebraic data type (§2.7)"),
        ("false", "boolean literal"),
        ("float", "built-in type (§2.4)"),
        ("for", "iteration over an array (§2.14)"),
        ("if", "conditional (§2.14)"),
        ("impl", "interface or type implementation (§10.4)"),
        ("import", "host capability or mod module (§2.3)"),
        ("infer", "explicit type crystallization (§2.16)"),
        ("int", "built-in type (§2.4)"),
        ("map", "keyed collection type map<K, V> (§11)"),
        ("match", "exhaustive pattern matching (§2.15)"),
        ("return", "function return (§2.11)"),
        ("str", "built-in type (§2.4)"),
        ("struct", "record type (§2.6)"),
        ("true", "boolean literal"),
        ("void", "no-value return type (§2.4)"),
        ("while", "loop (§2.14)"),
        ("option", "optional value enum<T> (§2.8)"),
        ("result", "error-carrying enum result<T, E> (§2.8)"),
    ];

    let mut items = Vec::new();

    // Locals of the enclosing function, declared before the cursor.
    if let Some(function_index) = analysis.enclosing_function_index(offset) {
        for local in &analysis.locals {
            if local.function != function_index || local.span.start > offset {
                continue;
            }
            let kind = match local.kind {
                crate::analysis::LocalKind::Param => ls_types::CompletionItemKind::VARIABLE,
                _ => ls_types::CompletionItemKind::VARIABLE,
            };
            let mut entry = item(
                local.name.clone(),
                kind,
                format!("{}: {}", local.name, render_type(&local.ty)),
                None,
            );
            entry.sort_text = Some(format!("0{}", local.name));
            items.push(entry);
        }
    }

    // Top-level declarations.
    for function in &analysis.functions {
        if function.impl_index.is_some() {
            continue;
        }
        let params: Vec<String> = function
            .params
            .iter()
            .map(|param| format!("{} {}", render_type(&param.ty), param.name))
            .collect();
        let mut entry = item(
            function.name.clone(),
            ls_types::CompletionItemKind::FUNCTION,
            format!(
                "{} {}({})",
                render_type(&function.return_ty),
                function.name,
                params.join(", ")
            ),
            Some(params.join(", ")),
        );
        entry.sort_text = Some(format!("1{}", function.name));
        items.push(entry);
    }
    for struct_type in &analysis.structs {
        let fields: Vec<String> = struct_type
            .fields
            .iter()
            .map(|(name, ty, _)| format!("{}: {}", name, render_type(ty)))
            .collect();
        let mut entry = item(
            struct_type.name.clone(),
            ls_types::CompletionItemKind::STRUCT,
            format!("struct {}<{} fields>", struct_type.name, fields.len()),
            Some(fields.join(", ")),
        );
        entry.sort_text = Some(format!("1{}", struct_type.name));
        items.push(entry);
    }
    for enum_type in &analysis.enums {
        let variants: Vec<String> = enum_type
            .variants
            .iter()
            .map(|variant| variant.name.clone())
            .collect();
        let mut entry = item(
            enum_type.name.clone(),
            ls_types::CompletionItemKind::ENUM,
            format!("enum {} {{ {} }}", enum_type.name, variants.join(", ")),
            Some(variants.join(", ")),
        );
        entry.sort_text = Some(format!("1{}", enum_type.name));
        items.push(entry);
    }

    // Built-in constructors (§2.8) and keywords.
    for (name, detail) in resolve::BUILTIN_CONSTRUCTORS {
        let mut entry = item(
            name.to_string(),
            ls_types::CompletionItemKind::ENUM_MEMBER,
            detail.to_string(),
            None,
        );
        entry.sort_text = Some(format!("1{name}"));
        items.push(entry);
    }
    for (name, detail) in KEYWORDS {
        let mut entry = item(
            name.to_string(),
            ls_types::CompletionItemKind::KEYWORD,
            detail.to_string(),
            None,
        );
        entry.sort_text = Some(format!("2{name}"));
        items.push(entry);
    }
    items
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
