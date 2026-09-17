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

/// The top-level declaration keywords of a script — shared with the §8
/// megaprogram completion's top level, where declarations still apply.
pub const SCRIPT_DECL_KEYWORDS: [(&str, &str); 5] = [
    ("struct", "record type (§2.6)"),
    ("enum", "algebraic data type (§2.7)"),
    ("impl", "interface or type implementation (§10.4)"),
    ("import", "host capability or mod module (§2.3)"),
    ("infer", "explicit type crystallization (§2.16)"),
];

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

    // Completing an `impl <namespace>.<interface>` member (§9.1, §10.4):
    // the interface's missing members, signature-exact.
    if let Some(items) = impl_member_completions(analysis, offset) {
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
    if in_unterminated_string(analysis, text, offset) {
        return true;
    }
    if in_line_comment(analysis, text, offset) {
        return true;
    }
    in_block_comment(analysis, text, offset)
}

/// True when the cursor sits inside a string that is still being typed —
/// an UNTERMINATED quote folds no string token, so `in_string_literal`
/// cannot see it and completions would leak scope members (or argument
/// names) into string content. The opener is found from the raw text: a
/// quote NOT covered by any string token (a complete string's quotes are
/// token-covered, nested island strings included) opens an unterminated
/// string. A plain `"` silences completion; a `$"` opener stays
/// completable only while an interpolation island is open between it and
/// the cursor — the same island rule the complete-token path applies.
fn in_unterminated_string(analysis: &Analysis<'_>, text: &str, offset: usize) -> bool {
    let line_start = text[..offset].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    let bytes = text.as_bytes();
    let mut index = line_start;
    while index < offset {
        match bytes[index] {
            b'\\' => {
                index += 2; // the escape pair never opens a string
                continue;
            }
            b'"' if !covered_by_string(analysis, index) => {
                let interpolated = index > 0 && bytes[index - 1] == b'$';
                if !interpolated {
                    return true;
                }
                let mut depth = 0i32;
                let mut cursor = index + 1;
                while cursor < offset {
                    match bytes[cursor] {
                        b'\\' => cursor += 1,
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    cursor += 1;
                }
                // Open island: island code is being typed — completable.
                // Literal part: still inside the string — silenced.
                return depth <= 0;
            }
            _ => {}
        }
        index += 1;
    }
    false
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

    // A dotted host path rooted at a granted schema namespace
    // (`game.` / `game.window.` — §2.3, §9.1): the contract surface, not a
    // script type.
    if let Some(items) = schema_path_completions(analysis, receiver) {
        return Some(items);
    }
    let ty = analysis.type_of_receiver(receiver, offset)?;
    Some(match &ty {
        cme_core::ast::Type::Named { name, args } => {
            if let Some(struct_type) = analysis
                .structs
                .iter()
                .find(|s| &s.name == name)
                .or_else(|| analysis.schema_structs.iter().find(|s| &s.name == name))
            {
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
            if let Some(enum_type) = analysis
                .enums
                .iter()
                .find(|e| &e.name == name)
                .or_else(|| analysis.schema_enums.iter().find(|e| &e.name == name))
            {
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

/// Member completions on a granted host path (§2.3, §9.1):
///
/// - `game.` offers the namespace's contracts (capabilities to import and
///   call, interfaces to implement);
/// - `game.window.` offers the capability's visible members (§9.5) with
///   their exact signatures.
fn schema_path_completions(
    analysis: &Analysis<'_>,
    receiver: (usize, usize),
) -> Option<Vec<ls_types::CompletionItem>> {
    let (file, segments) = analysis.host_path(receiver)?;
    match segments.as_slice() {
        [] => {
            let mut items = Vec::new();
            for contract in file.contracts() {
                let (kind, note) = match contract.kind {
                    cme_core::schema::ContractKind::Capability => (
                        ls_types::CompletionItemKind::MODULE,
                        "capability — import and call its members (§9.1)",
                    ),
                    cme_core::schema::ContractKind::Interface => (
                        ls_types::CompletionItemKind::INTERFACE,
                        "interface — implement with `impl` (§9.1, §10.4)",
                    ),
                };
                items.push(item(
                    contract.name.clone(),
                    kind,
                    format!("{} {}", contract.kind.keyword(), note),
                    None,
                ));
            }
            Some(items)
        }
        [contract_name] => {
            let contract = file.contracts().find(|decl| decl.name == *contract_name)?;
            // Calling interface members from a script is the host's
            // direction (§9.1); only capabilities complete here.
            if contract.kind != cme_core::schema::ContractKind::Capability {
                return Some(Vec::new());
            }
            let target = analysis
                .schema
                .as_ref()
                .and_then(|schema| schema.target(&file.namespace));
            Some(
                contract
                    .members
                    .iter()
                    .filter(|member| target.is_none_or(|target| member.visible_at(target)))
                    .map(|member| schema_member_item(&file.namespace, contract, member))
                    .collect(),
            )
        }
        _ => None,
    }
}

/// A capability member as a completion: the signature in the detail, the
/// plain name as the insert (named-argument completion fills the call).
fn schema_member_item(
    namespace: &str,
    contract: &cme_core::schema::SchemaContract,
    member: &cme_core::schema::SchemaMember,
) -> ls_types::CompletionItem {
    let params: Vec<String> = member
        .params
        .iter()
        .map(|param| format!("{} {}", render_type(&param.ty), param.name))
        .collect();
    item(
        member.name.clone(),
        ls_types::CompletionItemKind::METHOD,
        format!(
            "{} {}.{} ({})",
            render_type(&member.return_ty),
            namespace,
            contract.name,
            params.join(", ")
        ),
        None,
    )
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
    // Typing a string-valued argument (`OpenWindow("h`): the quote is
    // still unterminated, so the lexer folds no string token and the
    // in-string gate above cannot see it. A segment opening with a quote
    // is a value being typed, not an argument-name position — offering
    // `title:` there would splice into the string.
    if last.trim_start().starts_with('"') {
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
    // a capability member's parameters (§9.1), or a variant constructor's
    // positional payload (detail only).
    if callee_index > 0 && analysis.tokens[callee_index - 1].kind == Tok::Dot {
        let receiver = analysis.receiver_range_for_completion(callee_index - 1)?;

        // A capability call (`game.window.OpenWindow(`): named arguments
        // from the schema member's parameter list (§2.12, §9.1).
        if let Some((file, segments)) = analysis.host_path(receiver)
            && let [contract_name] = segments.as_slice()
            && let Some(contract) = file.contracts().find(|decl| decl.name == *contract_name)
            && let Some(target) = analysis
                .schema
                .as_ref()
                .and_then(|schema| schema.target(&file.namespace))
            && let Some(member) = contract
                .members
                .iter()
                .find(|decl| decl.name == callee_name && decl.visible_at(target))
        {
            return Some(named_items(
                member
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), render_type(&param.ty))),
                &used,
                ls_types::CompletionItemKind::VARIABLE,
            ));
        }

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
/// valid ones:
///
/// - `self` at the root, then the owning mod's module tree (§10.3);
/// - the granted schema namespaces at the root, their capabilities one dot
///   deeper (§2.3, §9.1);
/// - at any depth, the segments this file's other imports use under the
///   same prefix.
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
        // `self` is the reserved mod root (§2.3); schema namespaces are
        // the host roots (§2.3, §7.2).
        candidates.push("self".to_string());
        if let Some(schema) = &analysis.schema {
            for file in schema.set.namespaces() {
                if schema.target(&file.namespace).is_some() {
                    candidates.push(file.namespace.clone());
                }
            }
        }
    }

    // The mod's own module tree under `self` (§10.3).
    for module in &analysis.mod_modules {
        let matches_prefix = module.len() > depth
            && module[..depth]
                .iter()
                .zip(&prefix)
                .all(|(segment, expected)| segment == expected);
        if matches_prefix && let Some(name) = module.get(depth) {
            candidates.push(name.clone());
        }
    }

    // Capabilities complete one segment under a granted namespace (§9.1).
    if depth == 1
        && let Some(schema) = &analysis.schema
        && let Some(file) = schema.set.namespace(prefix[0])
    {
        for contract in file.contracts() {
            if contract.kind == cme_core::schema::ContractKind::Capability {
                candidates.push(contract.name.clone());
            }
        }
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

/// Completing a member of `impl <namespace>.<interface> { … }` (§9.1,
/// §10.4): the interface's visible members the block does not implement
/// yet, offered with their exact signature as the insert — the checker
/// requires every required member, spelled exactly.
///
/// Only fires at true member position: inside the impl's braces with no
/// enclosing function body (typing INSIDE a member's body keeps the
/// ordinary scope completions).
fn impl_member_completions(
    analysis: &Analysis<'_>,
    offset: usize,
) -> Option<Vec<ls_types::CompletionItem>> {
    if analysis.enclosing_function_index(offset).is_some() {
        return None;
    }
    let block_index = analysis.impls.iter().position(|impl_block| {
        impl_block.span.start <= offset
            && offset <= impl_block.span.end
            && impl_block.target.len() == 2
            && impl_open_brace(analysis, impl_block.span).is_some_and(|brace| offset > brace)
    })?;
    let block = &analysis.impls[block_index];
    let (namespace, contract_name) = (&block.target[0].0, &block.target[1].0);
    let members = analysis.visible_schema_members(namespace, contract_name)?;
    let implemented: Vec<&str> = analysis
        .functions
        .iter()
        .filter(|function| function.impl_index == Some(block_index))
        .map(|function| function.name.as_str())
        .collect();

    let mut items = Vec::new();
    for member in members {
        if implemented.contains(&member.name.as_str()) {
            continue;
        }
        let params: Vec<String> = member
            .params
            .iter()
            .map(|param| format!("{} {}", render_type(&param.ty), param.name))
            .collect();
        let optional_note = if member.requirement == cme_core::schema::MemberRequirement::Optional {
            " (optional — a mod may skip it, §9.5)"
        } else {
            ""
        };
        let mut entry = item(
            member.name.clone(),
            ls_types::CompletionItemKind::METHOD,
            format!(
                "{} {}.{} {}({}){}",
                render_type(&member.return_ty),
                namespace,
                contract_name,
                member.name,
                params.join(", "),
                optional_note
            ),
            Some(format!(
                "{} {}({}) {{\n    \n}}",
                render_type(&member.return_ty),
                member.name,
                params.join(", ")
            )),
        );
        entry.sort_text = Some(format!("0{}", member.name));
        items.push(entry);
    }
    Some(items)
}

/// The byte offset of the impl block's opening `{`, when its token range
/// has one (a recovered impl may not).
fn impl_open_brace(analysis: &Analysis<'_>, span: cme_core::Span) -> Option<usize> {
    let (start, end) = analysis.token_span_range(span);
    analysis.tokens[start..end]
        .iter()
        .find(|token| token.kind == Tok::LBrace)
        .map(|token| token.span.start)
}

/// Statement/expression position: keywords, locals in scope, top-level
/// declarations, and the built-in constructors.
fn scope_completions(analysis: &Analysis<'_>, offset: usize) -> Vec<ls_types::CompletionItem> {
    const KEYWORDS: [(&str, &str); 25] = [
        ("bool", "built-in type (§2.4)"),
        ("byte", "unsigned 8-bit integer type, 0..=255 (§2.4)"),
        ("else", "conditional alternative (§2.14)"),
        ("enum", "algebraic data type (§2.7)"),
        ("false", "boolean literal"),
        ("float", "built-in type (§2.4)"),
        ("for", "iteration over an array (§2.14)"),
        ("grammar", "§8 named library of matching rules"),
        ("if", "conditional (§2.14)"),
        ("impl", "interface or type implementation (§10.4)"),
        ("import", "host capability or mod module (§2.3)"),
        ("infer", "explicit type crystallization (§2.16)"),
        ("int", "built-in type (§2.4)"),
        ("in", "for-loop element binding (§2.14)"),
        ("map", "keyed collection type map<K, V> (§11)"),
        ("match", "exhaustive pattern matching (§2.15)"),
        ("mega", "§8 megaprogram: `mega name(pattern) { template }`"),
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

    // The mod schema's §9.3 boundary types — constructing a Sprite or
    // matching an Event variant is exactly what a mod author types here.
    for struct_type in &analysis.schema_structs {
        let fields: Vec<String> = struct_type
            .fields
            .iter()
            .map(|(name, ty, _)| format!("{}: {}", name, render_type(ty)))
            .collect();
        let mut entry = item(
            struct_type.name.clone(),
            ls_types::CompletionItemKind::STRUCT,
            format!(
                "struct {}<{} fields> — schema boundary type (§9.3)",
                struct_type.name,
                fields.len()
            ),
            None,
        );
        entry.sort_text = Some(format!("1{}", struct_type.name));
        items.push(entry);
    }
    for enum_type in &analysis.schema_enums {
        let variants: Vec<String> = enum_type
            .variants
            .iter()
            .map(|variant| variant.name.clone())
            .collect();
        let mut entry = item(
            enum_type.name.clone(),
            ls_types::CompletionItemKind::ENUM,
            format!(
                "enum {} {{ {} }} — schema boundary type (§9.3)",
                enum_type.name,
                variants.join(", ")
            ),
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
