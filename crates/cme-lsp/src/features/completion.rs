//! Completion: context-sensitive suggestions.
//!
//! Contexts, in priority order:
//!
//! 1. **Suppression** — nothing is offered inside comments or string
//!    literals, except inside the `{expr}` islands of a `$"…"`
//!    interpolation (§2.8), which stay completable.
//! 2. **Import paths** (§2.3) — detected from the raw line, because a
//!    statement that is still being typed (a single segment like
//!    `import engine`, or a trailing dot) parses as `Invalid` and has no
//!    AST import to anchor against. Offers `self` at the root and, at any
//!    depth, the segments the file's other imports use at that depth.
//! 3. **Member access** (`player.` and `player.na`) — struct fields with
//!    their (generically substituted) types, enum variants after the
//!    enum's name, the built-in constructors after `option.`/`result.`,
//!    and the array `.length` (§11).
//! 4. **Argument lists** (`vec2(`, `movePlayer(`, `vec2.dot(`) — named
//!    arguments (§2.12) from struct fields, function parameters, impl
//!    member parameters, or variant payloads, skipping names already
//!    present. Comma- and newline-delimited lists both work (§2.6); a
//!    positional argument switches the call to positional mode (§2.12),
//!    where named suggestions would be wrong.
//! 5. **Statement/expression position** — keywords, locals in scope, and
//!    every top-level declaration.
//!
//! All of it is derived from the same analysis the checker accepts, so
//! completions never contradict the compiler.

use tower_lsp_server::ls_types;

use crate::analysis::{Analysis, Tok, render_type};
use crate::resolve::{self, substitute};

/// Computes completions at `offset`.
pub fn completions(
    analysis: &Analysis<'_>,
    text: &str,
    offset: usize,
) -> Vec<ls_types::CompletionItem> {
    let offset = offset.min(text.len());

    // Comments and strings are not code; nothing to complete there.
    if is_in_comment_or_string(analysis, text, offset) {
        return Vec::new();
    }

    // Typing right after a `.` — including when the cursor sits past the
    // end of a line — is member context, detected from the text itself so
    // clamped positions still resolve.
    if let Some(items) = member_completions(analysis, text, offset) {
        return items;
    }

    if let Some(items) = argument_completions(analysis, text, offset) {
        return items;
    }

    if let Some(items) = import_completions(analysis, text, offset) {
        return items;
    }
    scope_completions(analysis, offset)
}

/// True when `offset` sits inside a comment or a string literal. The
/// lexer skips line comments and folds strings (plain and interpolated)
/// into single tokens, so the token stream alone cannot see them: the
/// comment openers are recovered from the raw text (string-token spans
/// protect `//` and `/*` sequences that are literal content), and a
/// string token claims every offset inside it — except the `{…}`
/// interpolation islands of a `$"…"` literal, which stay completable.
fn is_in_comment_or_string(analysis: &Analysis<'_>, text: &str, offset: usize) -> bool {
    if in_string_literal(analysis, text, offset) {
        return true;
    }
    if in_line_comment(analysis, text, offset) {
        return true;
    }
    in_block_comment(analysis, text, offset)
}

/// True when any string-literal token covers `offset`.
fn covered_by_string(analysis: &Analysis<'_>, offset: usize) -> bool {
    analysis.tokens.iter().any(|token| {
        matches!(token.kind, Tok::StrLit) && token.span.start <= offset && offset < token.span.end
    })
}

fn in_string_literal(analysis: &Analysis<'_>, text: &str, offset: usize) -> bool {
    for token in &analysis.tokens {
        if !matches!(token.kind, Tok::StrLit) {
            continue;
        }
        if offset <= token.span.start || offset >= token.span.end {
            continue;
        }
        // An interpolated literal (`$"…"`) keeps its `{…}` islands
        // completable; everything else inside a string is suppressed.
        if text.as_bytes().get(token.span.start) == Some(&b'$')
            && offset_in_interpolation_island(text, token.span, offset)
        {
            return false;
        }
        return true;
    }
    false
}

/// True when `offset` falls inside a `{…}` island of the interpolated
/// literal at `span`. Braces inside nested strings do not count, matching
/// the lexer's island scanning.
fn offset_in_interpolation_island(text: &str, span: cme_core::Span, offset: usize) -> bool {
    let bytes = text.as_bytes();
    // The interior starts after the `$"` opener.
    let mut index = span.start + 2;
    let mut depth = 0usize;
    let mut island_start = 0usize;
    while index < span.end && index < offset {
        match bytes[index] {
            b'\\' => {
                index += 2; // the escape pair never toggles braces
                continue;
            }
            b'"' => {
                // A nested string inside an island: skip it whole.
                index += 1;
                while index < span.end {
                    match bytes[index] {
                        b'\\' => index += 1,
                        b'"' => break,
                        _ => {}
                    }
                    index += 1;
                }
            }
            b'{' => {
                if depth == 0 {
                    island_start = index + 1;
                }
                depth += 1;
            }
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 && island_start <= offset && offset <= index {
                    return true;
                }
            }
            _ => {}
        }
        index += 1;
    }
    // An island that is still open (the expression is being typed)
    // contains the cursor.
    depth > 0 && island_start <= offset
}

fn in_line_comment(analysis: &Analysis<'_>, text: &str, offset: usize) -> bool {
    let line_start = text[..offset].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    let prefix = &text[line_start..offset];
    let mut from = 0;
    while let Some(rel) = prefix[from..].find("//") {
        let absolute = line_start + from + rel;
        // `//` inside a string literal is content, not a comment.
        if !covered_by_string(analysis, absolute) {
            return true;
        }
        from += rel + 2;
    }
    false
}

fn in_block_comment(analysis: &Analysis<'_>, text: &str, offset: usize) -> bool {
    let prefix = &text[..offset];
    let mut from = 0;
    let mut opener = None;
    while let Some(rel) = prefix[from..].find("/*") {
        let absolute = from + rel;
        if !covered_by_string(analysis, absolute) {
            opener = Some(absolute);
        }
        from = absolute + 2;
    }
    let Some(opener) = opener else {
        return false;
    };
    // Block comments end at the first `*/`; nothing after the opener and
    // before the cursor closed it.
    !text[opener + 2..offset].contains("*/")
}

/// Completions after a dot, while the member is still being typed
/// (`player.na`) as well as right after the dot (`player.`). Returns
/// `None` when the cursor is not in member context.
fn member_completions(
    analysis: &Analysis<'_>,
    text: &str,
    offset: usize,
) -> Option<Vec<ls_types::CompletionItem>> {
    // The last dot at or before the cursor, with only a bare identifier
    // prefix (possibly empty, possibly spaced) between it and the cursor,
    // on the same line.
    let dot_index = analysis
        .tokens
        .iter()
        .rposition(|token| token.kind == Tok::Dot && token.span.end <= offset)?;
    let dot_end = analysis.tokens[dot_index].span.end;
    let gap = text.get(dot_end..offset)?;
    if gap.contains('\n') {
        return None;
    }
    let prefix = gap.trim_start();
    let valid_prefix = prefix.is_empty()
        || (prefix.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'));
    if !valid_prefix {
        return None;
    }

    let receiver = analysis.receiver_range_for_completion(dot_index)?;
    // The built-in enums are not declared types, so a bare `option.` /
    // `result.` receiver types as nothing — recognize it by spelling.
    if let Some(token) = analysis.tokens.get(receiver.0)
        && receiver.1 == receiver.0 + 1
        && let Tok::Ident(name) = &token.kind
        && (name == "option" || name == "result")
    {
        return Some(match name.as_str() {
            "option" => builtin_constructors(["Some", "None"]),
            _ => builtin_constructors(["Ok", "Err"]),
        });
    }
    let ty = analysis.type_of_receiver(receiver, offset)?;
    Some(match &ty {
        cme_core::ast::Type::Named { name, args } => {
            if let Some(struct_type) = analysis.structs.iter().find(|s| &s.name == name) {
                return Some(
                    struct_type
                        .fields
                        .iter()
                        .map(|(field_name, field_ty, _)| {
                            let field_ty = substitute(field_ty, &struct_type.type_params, args);
                            item(
                                field_name.clone(),
                                ls_types::CompletionItemKind::FIELD,
                                render_type(&field_ty),
                                None,
                            )
                        })
                        .collect(),
                );
            }
            if let Some(enum_type) = analysis.enums.iter().find(|e| &e.name == name) {
                return Some(
                    enum_type
                        .variants
                        .iter()
                        .map(|variant| {
                            // Variant constructors take POSITIONAL payload
                            // arguments only, so the payload renders as
                            // detail and the insert stays the label.
                            let payload: Vec<String> = variant
                                .fields
                                .iter()
                                .map(|(name, ty)| format!("{}: {}", name, render_type(ty)))
                                .collect();
                            item(
                                variant.name.clone(),
                                ls_types::CompletionItemKind::ENUM_MEMBER,
                                format!(
                                    "{}.{}({})",
                                    enum_type.name,
                                    variant.name,
                                    payload.join(", ")
                                ),
                                None,
                            )
                        })
                        .collect(),
                );
            }
            // The built-in enums surface their constructors too (§2.8).
            match name.as_str() {
                "option" => builtin_constructors(["Some", "None"]),
                "result" => builtin_constructors(["Ok", "Err"]),
                _ => Vec::new(),
            }
        }
        cme_core::ast::Type::Array(_) => vec![item(
            "length".to_string(),
            ls_types::CompletionItemKind::FIELD,
            "int — the number of elements (§11)".to_string(),
            None,
        )],
        _ => Vec::new(),
    })
}

/// `Some`/`None` and `Ok`/`Err` offered after `option.` / `result.`
fn builtin_constructors(names: [&str; 2]) -> Vec<ls_types::CompletionItem> {
    names
        .into_iter()
        .filter_map(|name| {
            resolve::BUILTIN_CONSTRUCTORS
                .iter()
                .find(|(candidate, _)| *candidate == name)
                .map(|(name, detail)| {
                    item(
                        name.to_string(),
                        ls_types::CompletionItemKind::ENUM_MEMBER,
                        detail.to_string(),
                        None,
                    )
                })
        })
        .collect()
}

/// Named-argument completions inside a call's argument list. Returns
/// `None` when the cursor is not in an argument list, when the call is
/// positional (§2.12), or when the user is typing a value (scope
/// completions apply there).
fn argument_completions(
    analysis: &Analysis<'_>,
    text: &str,
    offset: usize,
) -> Option<Vec<ls_types::CompletionItem>> {
    // Find the innermost unclosed `(` before the cursor.
    let visible = analysis
        .tokens
        .partition_point(|token| token.span.end <= offset);
    let mut depth = 0i32;
    let mut paren_index = None;
    for index in (0..visible).rev() {
        match analysis.tokens[index].kind {
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

    // The callee is the identifier before the `(` — possibly qualified
    // (`vec2.dot(`, `gameEvent.Damage(`).
    let callee_index = paren_index.checked_sub(1)?;
    let Tok::Ident(callee_name) = &analysis.tokens[callee_index].kind else {
        return None;
    };
    let callee_name = callee_name.clone();

    // A callee preceded by its own return type is a DECLARATION, not a
    // call: `int add(`, `float dot(`, `vec2[] make(`, `pair<int, str> f(`.
    if declaration_context(analysis, callee_index) {
        return None;
    }

    // The used names: every completed argument must be named (`name:
    // value`); one positional argument switches the whole call to
    // positional mode, where named suggestions would be wrong (§2.12).
    let paren_end = analysis.tokens[paren_index].span.end;
    let inside = text.get(paren_end..offset).unwrap_or("");
    let mut segments: Vec<&str> = Vec::new();
    let mut segment_start = 0usize;
    let mut bracket_depth = 0i32;
    for (index, byte) in inside.bytes().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => bracket_depth += 1,
            b')' | b']' | b'}' => bracket_depth -= 1,
            // Named arguments may be separated by commas or newlines
            // (§2.6, §2.12); newlines only split outside nested brackets.
            b',' | b'\n' if bracket_depth == 0 => {
                segments.push(&inside[segment_start..index]);
                segment_start = index + 1;
            }
            _ => {}
        }
    }
    segments.push(&inside[segment_start..]);

    // The last segment is the one being typed. If it already names a
    // value (`name:` / `name =`), the user is typing the VALUE — scope
    // completions apply there.
    let last = segments.last().copied().unwrap_or("");
    if last.contains(':') || last.contains('=') {
        return None;
    }
    let mut used: Vec<&str> = Vec::new();
    for segment in &segments[..segments.len() - 1] {
        let segment = segment.trim();
        if segment.is_empty() {
            continue; // a trailing separator, or an empty first line
        }
        let Some((name, _value)) = segment.split_once(':') else {
            return None; // positional mode
        };
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        used.push(name);
    }

    // Qualified callee: `Target.member(` — an impl member's parameters,
    // or a variant constructor's positional payload (detail only).
    if callee_index > 0 && analysis.tokens[callee_index - 1].kind == Tok::Dot {
        let receiver = analysis.receiver_range_for_completion(callee_index - 1)?;
        let ty = analysis.type_of_receiver(receiver, offset)?;
        let cme_core::ast::Type::Named { name: target, .. } = &ty else {
            return None;
        };
        // An impl member: the impl target's last segment names the type.
        if let Some(function) = analysis.functions.iter().find(|function| {
            function.name == callee_name
                && function.impl_index.is_some()
                && function
                    .impl_index
                    .and_then(|index| analysis.impls.get(index))
                    .and_then(|impl_block| impl_block.target.last())
                    .map(|(segment, _)| segment == target)
                    .unwrap_or(false)
        }) {
            return Some(named_items(
                function
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), render_type(&param.ty))),
                &used,
                ls_types::CompletionItemKind::VARIABLE,
            ));
        }
        // A variant constructor: positional payloads, so the payload
        // fields render as detail without insert text.
        if let Some((_enum_type, variant)) = analysis
            .enums
            .iter()
            .find(|e| &e.name == target)
            .and_then(|enum_type| {
                enum_type
                    .variants
                    .iter()
                    .find(|variant| variant.name == callee_name)
                    .map(|variant| (enum_type, variant))
            })
        {
            return Some(
                variant
                    .fields
                    .iter()
                    .map(|(name, ty)| {
                        item(
                            name.clone(),
                            ls_types::CompletionItemKind::FIELD,
                            format!("{}: {}", name, render_type(ty)),
                            None,
                        )
                    })
                    .collect(),
            );
        }
        return None;
    }

    // Struct construction: field list.
    if let Some(struct_type) = analysis.structs.iter().find(|s| s.name == callee_name) {
        return Some(named_items(
            struct_type
                .fields
                .iter()
                .map(|(name, ty, _)| (name.clone(), render_type(ty))),
            &used,
            ls_types::CompletionItemKind::FIELD,
        ));
    }
    // Function call: parameter list.
    if let Some(function) = analysis
        .functions
        .iter()
        .find(|f| f.name == callee_name && f.impl_index.is_none())
    {
        return Some(named_items(
            function
                .params
                .iter()
                .map(|param| (param.name.clone(), render_type(&param.ty))),
            &used,
            ls_types::CompletionItemKind::VARIABLE,
        ));
    }
    None
}

/// Builds `name: ` fill items for the names not used yet.
fn named_items(
    names: impl Iterator<Item = (String, String)>,
    used: &[&str],
    kind: ls_types::CompletionItemKind,
) -> Vec<ls_types::CompletionItem> {
    names
        .filter(|(name, _)| !used.contains(&name.as_str()))
        .map(|(name, ty)| {
            item(
                name.clone(),
                kind,
                format!("{}: {}", name, ty),
                Some(format!("{}: ", name)),
            )
        })
        .collect()
}

/// True when the `(` after `callee_index` opens a DECLARATION's parameter
/// list rather than a call: the token before the callee is a type keyword
/// or a declared type name, possibly after array suffixes (`vec2[]`) and
/// generic argument groups (`pair<int, str>`).
fn declaration_context(analysis: &Analysis<'_>, callee_index: usize) -> bool {
    let mut cursor = callee_index;
    loop {
        if cursor == 0 {
            return false;
        }
        cursor -= 1;
        match &analysis.tokens[cursor].kind {
            Tok::RBracket => {
                cursor = match match_group(analysis, cursor, Tok::LBracket, Tok::RBracket) {
                    Some(opener_before) => opener_before,
                    None => return false,
                };
            }
            Tok::Gt => {
                cursor = match match_group(analysis, cursor, Tok::Lt, Tok::Gt) {
                    Some(opener_before) => opener_before,
                    None => return false,
                };
            }
            Tok::KwInt | Tok::KwFloat | Tok::KwBool | Tok::KwStr | Tok::KwVoid | Tok::KwMap => {
                return true;
            }
            Tok::Ident(name) => {
                return analysis.structs.iter().any(|s| &s.name == name)
                    || analysis.enums.iter().any(|e| &e.name == name);
            }
            _ => return false,
        }
    }
}

/// Walks left from the closer at `cursor` to its opener's index. The
/// caller re-examines the token BEFORE the opener on the next loop
/// iteration (the loop's `cursor -= 1`), so `int[] make(` and
/// `pair<int, str> f(` both resolve through their type headers.
fn match_group(analysis: &Analysis<'_>, cursor: usize, open: Tok, close: Tok) -> Option<usize> {
    let mut scan = cursor;
    let mut depth = 0i32;
    loop {
        scan = scan.checked_sub(1)?;
        if analysis.tokens[scan].kind == close {
            depth += 1;
        } else if analysis.tokens[scan].kind == open {
            if depth == 0 {
                return Some(scan);
            }
            depth -= 1;
        }
    }
}

/// Import path completion (§2.3), detected from the raw line so a
/// statement that is still being typed completes alongside the already
/// valid ones: `self` at the root, and at any depth the segments this
/// file's other imports use under the same prefix.
fn import_completions(
    analysis: &Analysis<'_>,
    text: &str,
    offset: usize,
) -> Option<Vec<ls_types::CompletionItem>> {
    let line_start = text[..offset].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    let line = &text[line_start..offset];

    // `import` followed only by a dotted identifier path (possibly
    // partial, possibly empty).
    let rest = line.trim_start().strip_prefix("import")?;
    if !rest.is_empty() && !rest.starts_with(|c: char| c.is_whitespace()) {
        return None; // a different word (`importx`)
    }
    let path = rest.trim_start();
    if !path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    // Everything before the last dot is complete; the tail (if any) is
    // the segment being typed.
    let (completed, _partial) = match path.rfind('.') {
        Some(pos) => (&path[..pos], &path[pos + 1..]),
        None => ("", path),
    };
    let prefix: Vec<&str> = completed.split('.').filter(|s| !s.is_empty()).collect();
    let depth = prefix.len();

    let mut candidates: Vec<String> = Vec::new();
    if depth == 0 {
        // `self` is the reserved mod root (§2.3).
        candidates.push("self".to_string());
    }
    for other in &analysis.imports {
        let matches_prefix = other.segments.len() > depth
            && other.segments[..depth]
                .iter()
                .zip(&prefix)
                .all(|((segment, _), expected)| segment == expected);
        if matches_prefix && let Some((name, _)) = other.segments.get(depth) {
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
    const KEYWORDS: [(&str, &str); 22] = [
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
        ("in", "for-loop element binding (§2.14)"),
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

    // Locals of the enclosing function, declared before the cursor and
    // scoped to a block containing it (§2.10 block scoping).
    if let Some(function_index) = analysis.enclosing_function_index(offset) {
        for local in &analysis.locals {
            if local.function != function_index || local.span.start > offset {
                continue;
            }
            if !local.contains(offset) {
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

    // Top-level declarations. Accepting a function completes to a call,
    // so the fill opens the argument list (functions have no
    // non-call references); types also appear in TYPE positions, where a
    // parenthesized fill would be wrong, so they fill as their name.
    for function in &analysis.functions {
        if function.impl_index.is_some() {
            continue;
        }
        let params: Vec<String> = function
            .params
            .iter()
            .map(|param| format!("{} {}", render_type(&param.ty), param.name))
            .collect();
        let fill = if function.params.is_empty() {
            format!("{}()", function.name)
        } else {
            format!("{}(", function.name)
        };
        let mut entry = item(
            function.name.clone(),
            ls_types::CompletionItemKind::FUNCTION,
            format!(
                "{} {}({})",
                render_type(&function.return_ty),
                function.name,
                params.join(", ")
            ),
            Some(fill),
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
            // Structs also appear in TYPE positions, where a field-list
            // fill would be wrong; the fill stays the plain name.
            None,
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
            None,
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
