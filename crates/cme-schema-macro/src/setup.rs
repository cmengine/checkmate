//! `cme_schema_setup!` — one invocation from schema file to a running
//! host: parse the schema(s), register them, load the program, apply the
//! §5.5 limits, create the context, and construct the typed proxies.
//!
//! The macro REUSES the bindings generator (it emits the exact
//! [`cme_schema_bindings!`](crate::cme_schema_bindings) modules for every
//! `schema =` setting) and adds the host glue around them: a [`Host`]
//! struct owning the engine and the compiled program, a [`Session`]
//! carrying the context and every requested proxy, and the `run` /
//! `try_run` pair that wires the two together per invocation.
//!
//! The manual flow stays the advanced-user surface — this macro is a
//! layer over it, not a replacement: everything it generates calls the
//! same public API (`Engine::register_schema`, `Engine::load_*`,
//! `Engine::create_context`, `Proxy::new`) a hand-written host would.

use std::path::PathBuf;

use proc_macro::{Delimiter, Group, TokenStream, TokenTree};

use cme_core::schema::{ContractKind, SchemaFile, SchemaItem};

use crate::codegen::{self, is_rust_keyword, pascal_case, snake_case};
use crate::input::CompileErrorMessage;

use std::fmt::Write as _;

// ---------------------------------------------------------------------------
// Parsed invocation
// ---------------------------------------------------------------------------

/// Where the host program comes from. The literal is kept as WRITTEN (it
/// is re-emitted verbatim into the generated load call) plus unescaped
/// for error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramKind {
    /// `program = mod "checkmate"` → `Engine::load_mod`
    Mod,
    /// `program = file "scripts/hud.cm"` → `Engine::load_file`
    File,
    /// `program = source "int main() { … }"` → `Engine::load_source`
    Source,
}

impl ProgramKind {
    fn parse(name: &str) -> Option<ProgramKind> {
        match name {
            "mod" => Some(ProgramKind::Mod),
            "file" => Some(ProgramKind::File),
            "source" => Some(ProgramKind::Source),
            _ => None,
        }
    }

    fn describe(&self) -> &'static str {
        match self {
            ProgramKind::Mod => "mod",
            ProgramKind::File => "file",
            ProgramKind::Source => "source",
        }
    }
}

/// The `program = …` setting.
pub struct ProgramSetting {
    pub kind: ProgramKind,
    /// The string literal exactly as written (quotes and all) — emitted
    /// verbatim so escapes and raw-string forms survive round-tripping.
    pub literal: String,
    /// The unescaped text, for diagnostics.
    pub display: String,
}

/// The optional `limits = { … }` setting. Each key is
/// `Option<Option<String>>`: the outer `None` means "not specified"
/// (keep the API default), the inner `None` means the literal `none`
/// (explicitly unset), the inner `Some` holds the integer text.
#[derive(Default)]
pub struct LimitsSetting {
    pub fuel: Option<Option<String>>,
    pub deadline_ms: Option<Option<String>>,
    pub max_call_depth: Option<String>,
}

/// A `proxy = …` setting: the generated proxy type, optionally qualified
/// by its namespace module, optionally renamed for the session field.
pub struct ProxySetting {
    pub namespace: Option<String>,
    pub name: String,
    pub alias: Option<String>,
}

/// A `provider = <[namespace.]capability> => <expr>` setting. The
/// expression is captured verbatim; the generated glue wraps it in
/// `Arc::new(...)` and hands it to the generated registration bridge.
pub struct ProviderSetting {
    pub namespace: Option<String>,
    pub capability: String,
    pub expr: String,
}

/// Everything one invocation asked for.
pub struct SetupSettings {
    pub schemas: Vec<PathBuf>,
    pub program: ProgramSetting,
    pub api_crate: String,
    pub limits: Option<LimitsSetting>,
    pub proxies: Vec<ProxySetting>,
    pub providers: Vec<ProviderSetting>,
}

/// Parses the invocation grammar (hand-rolled — see the crate-level note
/// about towers for huts):
///
/// ```text
/// setting   → ident "=" value ","
/// schema    → "schema"    "=" STRING
/// program   → "program"   "=" ("mod" | "file" | "source") STRING
/// crate     → "crate"     "=" PATH
/// limits    → "limits"    "=" "{" [ident ":" ("none" | INT)]* "}"
/// proxy     → "proxy"     "=" [IDENT "."] IDENT ["as" IDENT]
/// provider  → "provider"  "=" [IDENT "."] IDENT "=>" tokens-to-comma
/// ```
pub fn parse_settings(input: TokenStream) -> Result<SetupSettings, CompileErrorMessage> {
    let usage = || {
        CompileErrorMessage::new(
            "cme_schema_setup! usage:\n\
             cme_schema_setup! {\n\
             \x20   schema = \"schemas/app.cm\",             // repeatable\n\
             \x20   program = mod \"checkmate\",             // mod | file | source\n\
             \x20   crate = ::cme::api,                      // optional, default ::cme::api\n\
             \x20   limits = { fuel: 1_000_000, deadline_ms: 50, max_call_depth: 64 }, // optional\n\
             \x20   proxy = AppProxy,                        // optional, repeatable\n\
             \x20   provider = capability => MyService,      // optional, repeatable\n\
             }",
        )
    };

    let trees = flatten_invisible_groups(input.into_iter().collect());
    let mut position = 0;

    let mut schemas: Vec<PathBuf> = Vec::new();
    let mut program: Option<ProgramSetting> = None;
    let mut api_crate: Option<String> = None;
    let mut limits: Option<LimitsSetting> = None;
    let mut proxies: Vec<ProxySetting> = Vec::new();
    let mut providers: Vec<ProviderSetting> = Vec::new();

    while position < trees.len() {
        let TokenTree::Ident(name) = &trees[position] else {
            return Err(CompileErrorMessage::new(format!(
                "cme_schema_setup!: expected a setting name (`schema`, `program`, `crate`, \
                 `limits`, `proxy`, or `provider`), found {tree}",
                tree = trees[position]
            )));
        };
        let name = name.to_string();
        position += 1;
        expect_punct(&trees, &mut position, '=')?;
        position += 1;

        match name.as_str() {
            "schema" => {
                let literal = string_value(&trees, &mut position, &name)?;
                if literal.1.is_empty() {
                    return Err(CompileErrorMessage::new(
                        "cme_schema_setup!: `schema` path cannot be empty",
                    ));
                }
                schemas.push(PathBuf::from(literal.1));
            }
            "program" => {
                let kind_name = match trees.get(position) {
                    Some(TokenTree::Ident(ident)) => ident.to_string(),
                    _ => return Err(usage()),
                };
                let Some(kind) = ProgramKind::parse(&kind_name) else {
                    return Err(CompileErrorMessage::new(format!(
                        "cme_schema_setup!: `program` expects `mod`, `file`, or `source`, \
                         found `{kind_name}`"
                    )));
                };
                position += 1;
                let (literal, display) = string_value(&trees, &mut position, &name)?;
                program = Some(ProgramSetting {
                    kind,
                    literal,
                    display,
                });
            }
            "crate" => {
                // The value is a path: `::a::b` or `a::b`.
                let mut rendered = String::new();
                while position < trees.len() {
                    match &trees[position] {
                        TokenTree::Ident(ident) => {
                            rendered.push_str(&ident.to_string());
                            position += 1;
                        }
                        TokenTree::Punct(punct) if punct.as_char() == ':' => {
                            rendered.push(':');
                            position += 1;
                        }
                        _ => break,
                    }
                }
                if rendered.is_empty() {
                    return Err(usage());
                }
                api_crate = Some(rendered);
            }
            "limits" => {
                let Some(TokenTree::Group(group)) = trees.get(position) else {
                    return Err(CompileErrorMessage::new(
                        "cme_schema_setup!: `limits` expects a brace group \
                         `limits = { fuel: …, deadline_ms: …, max_call_depth: … }`",
                    ));
                };
                if group.delimiter() != Delimiter::Brace {
                    return Err(CompileErrorMessage::new(
                        "cme_schema_setup!: `limits` expects a BRACE group \
                         `limits = { fuel: …, deadline_ms: …, max_call_depth: … }`",
                    ));
                }
                position += 1;
                limits = Some(parse_limits(group.clone())?);
            }
            "proxy" => {
                proxies.push(parse_qualified_name(&trees, &mut position, "proxy", "as")?);
            }
            "provider" => {
                let target = parse_qualified_name(&trees, &mut position, "provider", "=>")?;
                expect_punct(&trees, &mut position, '=')?;
                position += 1;
                expect_punct(&trees, &mut position, '>')?;
                position += 1;
                // The value is an arbitrary Rust expression: capture every
                // token up to the next comma. Groups arrive as single
                // token trees, so a comma inside `Arc::new(Service { … })`
                // is invisible here and the capture stays whole.
                let mut captured: Vec<TokenTree> = Vec::new();
                while position < trees.len() {
                    if matches!(&trees[position], TokenTree::Punct(punct) if punct.as_char() == ',')
                    {
                        break;
                    }
                    captured.push(trees[position].clone());
                    position += 1;
                }
                if captured.is_empty() {
                    return Err(CompileErrorMessage::new(
                        "cme_schema_setup!: `provider` requires a value expression after `=>`",
                    ));
                }
                let expr: TokenStream = captured.into_iter().collect();
                providers.push(ProviderSetting {
                    namespace: target.namespace,
                    capability: target.name,
                    expr: expr.to_string(),
                });
            }
            other => {
                return Err(CompileErrorMessage::new(format!(
                    "cme_schema_setup!: unknown setting `{other}` (expected `schema`, \
                     `program`, `crate`, `limits`, `proxy`, or `provider`)"
                )));
            }
        }

        // Settings separate by an optional trailing comma.
        skip_comma(&trees, &mut position);
    }

    let Some(program) = program else {
        return Err(CompileErrorMessage::new(
            "cme_schema_setup!: missing `program = mod \"…\" | file \"…\" | source \"…\"",
        ));
    };
    if schemas.is_empty() {
        return Err(CompileErrorMessage::new(
            "cme_schema_setup!: at least one `schema = \"app.cm\"` is required",
        ));
    }

    Ok(SetupSettings {
        schemas,
        program,
        api_crate: api_crate.unwrap_or_else(|| "::cme::api".to_string()),
        limits,
        proxies,
        providers,
    })
}

/// Parses the `limits = { … }` body: `ident: ("none" | INT)` pairs.
fn parse_limits(group: Group) -> Result<LimitsSetting, CompileErrorMessage> {
    let trees = flatten_invisible_groups(group.stream().into_iter().collect());
    let mut position = 0;
    let mut limits = LimitsSetting::default();

    while position < trees.len() {
        let TokenTree::Ident(key) = &trees[position] else {
            return Err(CompileErrorMessage::new(format!(
                "cme_schema_setup!: expected a limit name (`fuel`, `deadline_ms`, \
                 `max_call_depth`) inside `limits`, found {tree}",
                tree = trees[position]
            )));
        };
        let key = key.to_string();
        position += 1;
        expect_punct(&trees, &mut position, ':')?;
        position += 1;

        let value = trees.get(position).cloned();
        position += 1;
        match (key.as_str(), value) {
            ("fuel", Some(TokenTree::Ident(ident))) if ident.to_string() == "none" => {
                limits.fuel = Some(None);
            }
            ("fuel", Some(TokenTree::Literal(literal))) => {
                limits.fuel = Some(Some(parse_int(&literal.to_string(), "fuel")?));
            }
            ("deadline_ms", Some(TokenTree::Ident(ident))) if ident.to_string() == "none" => {
                limits.deadline_ms = Some(None);
            }
            ("deadline_ms", Some(TokenTree::Literal(literal))) => {
                limits.deadline_ms = Some(Some(parse_int(&literal.to_string(), "deadline_ms")?));
            }
            ("max_call_depth", Some(TokenTree::Literal(literal))) => {
                limits.max_call_depth = Some(parse_int(&literal.to_string(), "max_call_depth")?);
            }
            ("max_call_depth", _) => {
                return Err(CompileErrorMessage::new(
                    "cme_schema_setup!: `max_call_depth` takes a non-negative integer \
                     (`none` is meaningless for a depth bound — omit the key instead)",
                ));
            }
            (other, _) => {
                return Err(CompileErrorMessage::new(format!(
                    "cme_schema_setup!: unknown limit `{other}` (expected `fuel`, \
                     `deadline_ms`, or `max_call_depth`)"
                )));
            }
        }

        skip_comma(&trees, &mut position);
    }
    Ok(limits)
}

/// Parses `[namespace .] name`, with an optional follow-up keyword +
/// identifier pair (`proxy = Name as field`). `follow` is the keyword
/// that introduces the second identifier (`as` for proxies, `=>` for
/// providers — the provider variant stops before it).
fn parse_qualified_name(
    trees: &[TokenTree],
    position: &mut usize,
    setting: &str,
    follow: &str,
) -> Result<ProxySetting, CompileErrorMessage> {
    let first = match trees.get(*position) {
        Some(TokenTree::Ident(ident)) => ident.to_string(),
        other => {
            return Err(CompileErrorMessage::new(format!(
                "cme_schema_setup!: `{setting}` expects a name, found {}",
                other
                    .map(|tree| tree.to_string())
                    .unwrap_or_else(|| "end of invocation".to_string())
            )));
        }
    };
    *position += 1;

    // `namespace.name`
    let mut namespace = None;
    let mut name = first;
    if matches!(trees.get(*position), Some(TokenTree::Punct(punct)) if punct.as_char() == '.') {
        *position += 1;
        namespace = Some(name);
        match trees.get(*position) {
            Some(TokenTree::Ident(ident)) => name = ident.to_string(),
            _ => {
                return Err(CompileErrorMessage::new(format!(
                    "cme_schema_setup!: `{setting}` expects a name after the dot"
                )));
            }
        }
        *position += 1;
    }

    // `as field` (proxies only; providers never see `as`).
    let mut alias = None;
    if follow == "as"
        && let Some(TokenTree::Ident(ident)) = trees.get(*position)
        && ident.to_string() == "as"
    {
        *position += 1;
        match trees.get(*position) {
            Some(TokenTree::Ident(field)) => alias = Some(field.to_string()),
            _ => {
                return Err(CompileErrorMessage::new(
                    "cme_schema_setup!: `as` expects a field name",
                ));
            }
        }
        *position += 1;
    }

    Ok(ProxySetting {
        namespace,
        name,
        alias,
    })
}

// ---------------------------------------------------------------------------
// Token helpers
// ---------------------------------------------------------------------------

/// Removes `#[path = …]`-style invisible groups so literals wrapped in
/// them parse like bare tokens. Recurses: invisible groups may nest.
fn flatten_invisible_groups(trees: Vec<TokenTree>) -> Vec<TokenTree> {
    let mut out = Vec::with_capacity(trees.len());
    for tree in trees {
        match tree {
            TokenTree::Group(ref group) if group.delimiter() == Delimiter::None => {
                out.extend(flatten_invisible_groups(
                    group.stream().into_iter().collect(),
                ));
            }
            other => out.push(other),
        }
    }
    out
}

fn expect_punct(
    trees: &[TokenTree],
    position: &mut usize,
    expected: char,
) -> Result<(), CompileErrorMessage> {
    match trees.get(*position) {
        Some(TokenTree::Punct(punct)) if punct.as_char() == expected => Ok(()),
        other => Err(CompileErrorMessage::new(format!(
            "cme_schema_setup!: expected `{expected}`, found {}",
            other
                .map(|tree| tree.to_string())
                .unwrap_or_else(|| "end of invocation".to_string())
        ))),
    }
}

fn skip_comma(trees: &[TokenTree], position: &mut usize) {
    if matches!(trees.get(*position), Some(TokenTree::Punct(punct)) if punct.as_char() == ',') {
        *position += 1;
    }
}

/// Reads a string-literal value: returns the literal TEXT as written
/// (for verbatim re-emission) plus the unescaped content (for messages
/// and paths). `setting` names the offending key in diagnostics.
fn string_value(
    trees: &[TokenTree],
    position: &mut usize,
    setting: &str,
) -> Result<(String, String), CompileErrorMessage> {
    let Some(TokenTree::Literal(literal)) = trees.get(*position) else {
        return Err(CompileErrorMessage::new(format!(
            "cme_schema_setup!: `{setting}` expects a string literal"
        )));
    };
    *position += 1;
    let text = literal.to_string();
    let unquoted = crate::input::unquote(&text).ok_or_else(|| {
        CompileErrorMessage::new(format!(
            "cme_schema_setup!: `{setting}` is not a plain string literal: {text}"
        ))
    })?;
    Ok((text, unquoted))
}

/// Validates an unsigned-integer literal (unsuffixed; `0x`/`0o`/`0b`
/// forms accepted) and returns the token text for verbatim re-emission.
fn parse_int(text: &str, setting: &str) -> Result<String, CompileErrorMessage> {
    let invalid = |reason: &str| {
        CompileErrorMessage::new(format!(
            "cme_schema_setup!: `{setting}` must be a plain non-negative integer \
             literal ({reason}), found {text}"
        ))
    };
    if text.is_empty() || !text.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        return Err(invalid("no trailing suffix is allowed"));
    }
    if !text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(invalid("digits only"));
    }
    let normalized = text.replace('_', "");
    let parsed = if let Some(rest) = normalized.strip_prefix("0x") {
        u64::from_str_radix(rest, 16)
    } else if let Some(rest) = normalized.strip_prefix("0o") {
        u64::from_str_radix(rest, 8)
    } else if let Some(rest) = normalized.strip_prefix("0b") {
        u64::from_str_radix(rest, 2)
    } else {
        normalized.parse::<u64>()
    };
    parsed
        .map(|_| text.to_string())
        .map_err(|_| invalid("out of u64 range or not a number"))
}

// ---------------------------------------------------------------------------
// Resolution: settings against the parsed schemas
// ---------------------------------------------------------------------------

/// What one schema file contributes to the generated glue: its namespace
/// (== the generated module name), the proxy types for its interfaces,
/// and the registration bridges for its capabilities.
struct SchemaBundle {
    namespace: String,
    /// `(proxy type, qualified interface path)` per interface.
    proxies: Vec<(String, String)>,
    /// `(capability name, generated register fn)` per capability.
    capabilities: Vec<(String, String)>,
}

fn bundle(schema: &SchemaFile) -> SchemaBundle {
    let namespace = &schema.namespace;
    let mut proxies = Vec::new();
    let mut capabilities = Vec::new();
    for item in &schema.items {
        let SchemaItem::Contract(contract) = item else {
            continue;
        };
        match contract.kind {
            ContractKind::Interface => proxies.push((
                format!(
                    "{}{}Proxy",
                    pascal_case(namespace),
                    pascal_case(&contract.name)
                ),
                format!("{}.{}", namespace, contract.name),
            )),
            ContractKind::Capability => capabilities.push((
                contract.name.clone(),
                format!(
                    "register_{}_{}",
                    snake_case(namespace),
                    snake_case(&contract.name)
                ),
            )),
        }
    }
    SchemaBundle {
        namespace: namespace.clone(),
        proxies,
        capabilities,
    }
}

/// A `proxy = …` setting resolved against the bundles.
struct ResolvedProxy {
    namespace: String,
    proxy_type: String,
    /// The qualified interface path, for the field's doc comment.
    interface: String,
    field: String,
}

/// A `provider = …` setting resolved against the bundles.
struct ResolvedProvider {
    namespace: String,
    register_fn: String,
    expr: String,
}

fn available_proxies(bundles: &[SchemaBundle]) -> String {
    bundles
        .iter()
        .flat_map(|bundle| {
            bundle
                .proxies
                .iter()
                .map(|(proxy, _)| format!("{}::{}", bundle.namespace, proxy))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn available_capabilities(bundles: &[SchemaBundle]) -> String {
    bundles
        .iter()
        .flat_map(|bundle| {
            bundle
                .capabilities
                .iter()
                .map(|(capability, _)| format!("{}.{}", bundle.namespace, capability))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn resolve_proxies(
    settings: &[ProxySetting],
    bundles: &[SchemaBundle],
) -> Result<Vec<ResolvedProxy>, CompileErrorMessage> {
    let available = available_proxies(bundles);
    let mut resolved = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    for setting in settings {
        // Locate the proxy type: an explicit namespace narrows the search;
        // otherwise the name must be unique across every namespace.
        let candidates: Vec<(&SchemaBundle, &(String, String))> = bundles
            .iter()
            .filter_map(|bundle| {
                if let Some(namespace) = &setting.namespace
                    && &bundle.namespace != namespace
                {
                    return None;
                }
                bundle
                    .proxies
                    .iter()
                    .find(|(proxy, _)| *proxy == setting.name)
                    .map(|found| (bundle, found))
            })
            .collect();
        let (bundle, (proxy_type, interface)) = match candidates.as_slice() {
            [] if setting.namespace.is_some() => {
                return Err(CompileErrorMessage::new(format!(
                    "cme_schema_setup!: schema namespace `{}` has no proxy `{}` \
                     (available: {available})",
                    setting.namespace.as_ref().expect("checked above"),
                    setting.name
                )));
            }
            [] => {
                return Err(CompileErrorMessage::new(format!(
                    "cme_schema_setup!: no generated proxy named `{}` (available: {available})",
                    setting.name
                )));
            }
            [single] => *single,
            _ => {
                let namespaces = candidates
                    .iter()
                    .map(|(bundle, _)| bundle.namespace.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(CompileErrorMessage::new(format!(
                    "cme_schema_setup!: proxy `{}` is ambiguous across namespaces \
                     ({namespaces}) — qualify it as `proxy = namespace.{}`",
                    setting.name, setting.name
                )));
            }
        };

        // The session field: the alias, or the proxy type's snake_case
        // with its `Proxy` suffix dropped.
        let field = setting.alias.clone().unwrap_or_else(|| {
            let snake = snake_case(proxy_type);
            match snake.strip_suffix("_proxy") {
                Some(stripped) if !stripped.is_empty() => stripped.to_string(),
                _ => snake,
            }
        });
        if is_rust_keyword(&field) || matches!(field.as_str(), "self" | "Self" | "crate" | "super")
        {
            return Err(CompileErrorMessage::new(format!(
                "cme_schema_setup!: proxy `{proxy_type}` derives the session field name \
                 `{field}`, which is a Rust keyword — rename it with `proxy = {} as field`",
                setting.name
            )));
        }
        if field == "context" {
            return Err(CompileErrorMessage::new(format!(
                "cme_schema_setup!: session field name `context` is taken by the context \
                 field — rename it with `proxy = {} as field`",
                setting.name
            )));
        }
        if fields.contains(&field) {
            return Err(CompileErrorMessage::new(format!(
                "cme_schema_setup!: duplicate session field name `{field}` — \
                 disambiguate with `as`"
            )));
        }
        fields.push(field.clone());

        resolved.push(ResolvedProxy {
            namespace: bundle.namespace.clone(),
            proxy_type: proxy_type.clone(),
            interface: interface.clone(),
            field,
        });
    }
    Ok(resolved)
}

fn resolve_providers(
    settings: &[ProviderSetting],
    bundles: &[SchemaBundle],
) -> Result<Vec<ResolvedProvider>, CompileErrorMessage> {
    let available = available_capabilities(bundles);
    let mut resolved = Vec::new();
    for setting in settings {
        let candidates: Vec<(&SchemaBundle, &(String, String))> = bundles
            .iter()
            .filter_map(|bundle| {
                if let Some(namespace) = &setting.namespace
                    && &bundle.namespace != namespace
                {
                    return None;
                }
                bundle
                    .capabilities
                    .iter()
                    .find(|(capability, _)| *capability == setting.capability)
                    .map(|found| (bundle, found))
            })
            .collect();
        let (bundle, (_, register_fn)) = match candidates.as_slice() {
            [] if setting.namespace.is_some() => {
                return Err(CompileErrorMessage::new(format!(
                    "cme_schema_setup!: schema namespace `{}` has no capability `{}` \
                     (available: {available})",
                    setting.namespace.as_ref().expect("checked above"),
                    setting.capability
                )));
            }
            [] => {
                return Err(CompileErrorMessage::new(format!(
                    "cme_schema_setup!: no schema capability named `{}` (available: {available})",
                    setting.capability
                )));
            }
            [single] => *single,
            _ => {
                let namespaces = candidates
                    .iter()
                    .map(|(bundle, _)| bundle.namespace.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(CompileErrorMessage::new(format!(
                    "cme_schema_setup!: capability `{}` exists in several namespaces \
                     ({namespaces}) — qualify it as `provider = namespace.{} => …`",
                    setting.capability, setting.capability
                )));
            }
        };
        resolved.push(ResolvedProvider {
            namespace: bundle.namespace.clone(),
            register_fn: register_fn.clone(),
            expr: setting.expr.clone(),
        });
    }
    Ok(resolved)
}

// ---------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------

/// Emits the whole expansion: one bindings module per schema (the exact
/// `cme_schema_bindings!` output) plus the host glue. Every generated
/// path is fully qualified, so the output is hygiene-safe under any
/// host prelude.
pub fn generate(
    settings: &SetupSettings,
    schemas: &[SchemaFile],
) -> Result<String, Vec<CompileErrorMessage>> {
    // One module is generated per namespace: duplicate namespaces would
    // collide in Rust long before the engine ever saw the second schema.
    let mut errors = Vec::new();
    for (index, schema) in schemas.iter().enumerate() {
        if schemas[index + 1..]
            .iter()
            .any(|other| other.namespace == schema.namespace)
        {
            errors.push(CompileErrorMessage::new(format!(
                "cme_schema_setup!: schema namespace `{}` appears more than once — \
                 one module is generated per namespace",
                schema.namespace
            )));
        }
    }
    let bundles: Vec<SchemaBundle> = schemas.iter().map(bundle).collect();
    let proxies = match resolve_proxies(&settings.proxies, &bundles) {
        Ok(proxies) => proxies,
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };
    let providers = match resolve_providers(&settings.providers, &bundles) {
        Ok(providers) => providers,
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };
    if !errors.is_empty() {
        return Err(errors);
    }

    let api = &settings.api_crate;
    let mut out = String::new();

    // 1. The bindings, verbatim from the shared generator.
    for schema in schemas {
        match codegen::generate(schema, api) {
            Ok(module) => out.push_str(&module),
            Err(mut module_errors) => errors.append(&mut module_errors),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    // 2. The limits expression.
    let limits = render_limits(settings, api);

    // 3. The host glue.
    let program_display = &settings.program.display;
    let schema_list = schemas
        .iter()
        .map(|schema| format!("`{} {}`", schema.namespace, schema.version))
        .collect::<Vec<_>>()
        .join(", ");
    write!(
        out,
        r#"/// The one-call host built by `cme_schema_setup!`: schema(s) {schema_list}
/// registered, program `{program_kind} {program_display}` loaded, and the
/// §5.5 execution limits configured. [`Host::new`] performs the setup —
/// schema registration first, then the capability providers, then the
/// program load, exactly as the manual flow in the embedding guide does —
/// and `run` / `try_run` create a context and the requested proxies in
/// one pass. Generated code; DO NOT EDIT.
#[allow(dead_code)]
pub struct Host {{
    engine: {api}::Engine,
    program: {api}::CompiledProgram,
    limits: {api}::ExecutionLimits,
}}

/// Why [`Host::new`] can fail, in the order the setup performs it: the
/// schema registration (§9.2), a capability provider bridge (§13.1), the
/// program compile, or the program file's I/O.
#[derive(Debug)]
pub enum HostError {{
    Schema({api}::SchemaError),
    Provider(::std::string::String),
    Compile({api}::CompileError),
    Io(::std::string::String),
}}

impl ::core::fmt::Display for HostError {{
    fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {{
        match self {{
            HostError::Schema(error) => {{
                write!(formatter, "schema registration failed: {{error}}")
            }}
            HostError::Provider(message) => {{
                write!(formatter, "capability provider registration failed: {{message}}")
            }}
            HostError::Compile(error) => {{
                write!(formatter, "the program failed to compile:\n{{error}}")
            }}
            HostError::Io(message) => formatter.write_str(message),
        }}
    }}
}}

impl ::std::error::Error for HostError {{}}

impl Host {{
    /// Registers every schema, wires every capability provider, loads
    /// the program through the schema-gated pipeline, and stores the
    /// configured §5.5 limits. The engine, the program, and the limits
    /// stay individually reachable for advanced use.
    pub fn new() -> ::core::result::Result<Host, HostError> {{
        let mut engine = {api}::Engine::new();
{schema_registrations}{provider_registrations}
        let program = {program_load}?;
        ::core::result::Result::Ok(Host {{ engine, program, limits: {limits} }})
    }}

    /// The engine the setup configured (schemas + providers registered).
    pub fn engine(&self) -> &{api}::Engine {{
        &self.engine
    }}

    /// The compiled program (immutable, `Send + Sync`).
    pub fn program(&self) -> &{api}::CompiledProgram {{
        &self.program
    }}

    /// The §5.5 limits every context of this host runs under.
    pub fn limits(&self) -> &{api}::ExecutionLimits {{
        &self.limits
    }}

    /// Creates a context over the program under the configured limits —
    /// the same shape `Engine::create_context` returns, so advanced
    /// hosts can keep working with the context directly.
    pub fn context(&self) -> {api}::Context<'_> {{
        self.engine.create_context(&self.program, self.limits.clone())
    }}

    /// One execution pass: builds the context, constructs every proxy,
    /// and hands the [`Session`] to `body`. A proxy whose interface the
    /// program never implemented panics with the rendered error — use
    /// [`Host::try_run`] to get the failure as a `Result` instead.
    pub fn run<R>(&self, body: impl ::core::ops::FnOnce(&Session<'_, '_>) -> R) -> R {{
        match self.try_run(body) {{
            ::core::result::Result::Ok(result) => result,
            ::core::result::Result::Err(error) => ::std::panic!(
                "cme_schema_setup!: the loaded program cannot serve this session: {{error}}"
            ),
        }}
    }}

    /// [`Host::run`] without the panic: proxy construction failures come
    /// back as the [`{api}::ExecutionError`] the proxy constructors
    /// produce (kind `UnknownEntry` when the program never implemented
    /// the interface — §10.4 completeness is a script compile error, and
    /// this is the host-side guard).
    pub fn try_run<R>(
        &self,
        body: impl ::core::ops::FnOnce(&Session<'_, '_>) -> R,
    ) -> ::core::result::Result<R, {api}::ExecutionError> {{
        let context = self.context();
        let session = match Session::new(&context) {{
            ::core::result::Result::Ok(session) => session,
            ::core::result::Result::Err(error) => {{
                return ::core::result::Result::Err(error);
            }}
        }};
        ::core::result::Result::Ok(body(&session))
    }}
}}
"#,
        api = api,
        schema_list = schema_list,
        program_kind = settings.program.kind.describe(),
        program_display = program_display,
        schema_registrations = render_schema_registrations(schemas),
        provider_registrations = render_provider_registrations(&providers),
        program_load = render_program_load(&settings.program, api),
        limits = limits,
    )
    .expect("writing to a String cannot fail");

    // 4. The session: the context plus every requested proxy.
    let proxy_fields = proxies
        .iter()
        .map(|proxy| {
            format!(
                "    /// The `{interface}` interface proxy (§9.6): typed calls into the\n\
                 \x20   /// script's `impl` members. Construction already verified the\n\
                 \x20   /// program implements the interface.\n\
                 \x20   pub {field}: {namespace}::{proxy_type}<'a, 'p>,\n",
                interface = proxy.interface,
                field = proxy.field,
                namespace = proxy.namespace,
                proxy_type = proxy.proxy_type,
            )
        })
        .collect::<String>();
    let proxy_constructions = proxies
        .iter()
        .map(|proxy| {
            format!(
                "            {field}: {namespace}::{proxy_type}::new(context)?,\n",
                field = proxy.field,
                namespace = proxy.namespace,
                proxy_type = proxy.proxy_type,
            )
        })
        .collect::<String>();
    write!(
        out,
        r#"/// One execution pass over the host: the execution context plus every
/// proxy the invocation asked for. `context` is the underlying
/// [`{api}::Context`] (and `Session` derefs to it, so plain invocations
/// read `session.invoke(…)`); each proxy field calls INTO the script
/// through its §10.4 impl target with typed arguments and results.
#[allow(dead_code)]
pub struct Session<'a, 'p> {{
    /// The context this pass runs under — the configured §5.5 limits and
    /// the registered capability providers, per invocation.
    pub context: &'a {api}::Context<'p>,
{proxy_fields}}}

impl<'a, 'p> Session<'a, 'p> {{
    fn new(context: &'a {api}::Context<'p>) -> ::core::result::Result<Self, {api}::ExecutionError> {{
        ::core::result::Result::Ok(Session {{
            context,
{proxy_constructions}        }})
    }}
}}

impl<'a, 'p> ::core::ops::Deref for Session<'a, 'p> {{
    type Target = {api}::Context<'p>;

    fn deref(&self) -> &Self::Target {{
        self.context
    }}
}}
"#,
            api = api,
            proxy_fields = proxy_fields,
            proxy_constructions = proxy_constructions,
        )
    .expect("writing to a String cannot fail");

    Ok(out)
}

fn render_schema_registrations(schemas: &[SchemaFile]) -> String {
    schemas
        .iter()
        .map(|schema| {
            format!(
                "        {namespace}::register_schema(&mut engine).map_err(HostError::Schema)?;\n",
                namespace = schema.namespace,
            )
        })
        .collect()
}

fn render_provider_registrations(providers: &[ResolvedProvider]) -> String {
    providers
        .iter()
        .map(|provider| {
            format!(
                "        {namespace}::{register_fn}(&mut engine, \
                 ::std::sync::Arc::new({expr}))\n\
                 \x20           .map_err(HostError::Provider)?;\n",
                namespace = provider.namespace,
                register_fn = provider.register_fn,
                expr = provider.expr,
            )
        })
        .collect()
}

fn render_program_load(program: &ProgramSetting, api: &str) -> String {
    let literal = &program.literal;
    match program.kind {
        ProgramKind::Mod => format!(
            "engine.load_mod({literal}).map_err(HostError::Compile)",
            literal = literal
        ),
        ProgramKind::Source => format!(
            "engine.load_source({literal}).map_err(HostError::Compile)",
            literal = literal
        ),
        ProgramKind::File => format!(
            "match engine.load_file({literal}) {{\n\
             \x20           ::core::result::Result::Ok(program) => {{\n\
             \x20               ::core::result::Result::Ok(program)\n\
             \x20           }}\n\
             \x20           ::core::result::Result::Err({api}::LoadError::Io(message)) => {{\n\
             \x20               ::core::result::Result::Err(HostError::Io(message))\n\
             \x20           }}\n\
             \x20           ::core::result::Result::Err({api}::LoadError::Compile(error)) => {{\n\
             \x20               ::core::result::Result::Err(HostError::Compile(error))\n\
             \x20           }}\n\
             \x20       }}",
            literal = literal,
            api = api,
        ),
    }
}

fn render_limits(settings: &SetupSettings, api: &str) -> String {
    let Some(limits) = &settings.limits else {
        return format!("{api}::ExecutionLimits::default()");
    };
    let fuel = match &limits.fuel {
        None => "::core::option::Option::None".to_string(),
        Some(None) => "::core::option::Option::None".to_string(),
        Some(Some(text)) => format!("::core::option::Option::Some({text}u64)"),
    };
    let deadline = match &limits.deadline_ms {
        None => "::core::option::Option::None".to_string(),
        Some(None) => "::core::option::Option::None".to_string(),
        Some(Some(text)) => format!("::core::option::Option::Some({text}u64)"),
    };
    let depth = limits
        .max_call_depth
        .as_ref()
        .map(|text| format!("{text}usize"))
        .unwrap_or_else(|| format!("{api}::MAX_CALL_DEPTH"));
    format!(
        "{api}::ExecutionLimits {{ fuel: {fuel}, deadline_ms: {deadline}, max_call_depth: {depth} }}"
    )
}
