//! Focused tests for the §9 schema front end: the scanner, the parser, and
//! the cross-file [`SchemaSet`] invariants.

use cme_core::ast::{PrimitiveType, Type};
use cme_core::schema::{ContractKind, MemberRequirement, SchemaItem, Version};

use super::{SchemaContext, SchemaSet, parse_schema_file};

fn parse_clean(source: &str) -> cme_core::schema::SchemaFile {
    let outcome = parse_schema_file(source);
    assert!(
        outcome.is_clean(),
        "expected a clean parse, got: {:?}",
        outcome
            .diagnostics
            .iter()
            .map(|d| d.message().to_string())
            .collect::<Vec<_>>()
    );
    outcome.file.expect("a clean parse yields the file")
}

fn parse_errors(source: &str) -> Vec<String> {
    parse_schema_file(source)
        .diagnostics
        .iter()
        .map(|d| d.message().to_string())
        .collect()
}

const FULL_SCHEMA: &str = r#"
// File: schemas/engine.cm — the §9.1 example shape.
schema engine 1.4.0

struct TextureHandle {
    int id
}

struct Vec2 {
    float x
    float y
}

enum HttpError {
    NotFound(str message)
    Timeout()
}

struct HttpResponse {
    int status
    str body
}

struct HttpRequest {
    str url
}

struct GameConfig {
    int score
    bool active
}

struct GameState {
    int score
    bool active
}

since 1.0.0 capability graphics {
    TextureHandle LoadTexture(str path)
    void DrawTexture(TextureHandle tex, Vec2 position)
}

since 1.2.0 capability graphics {
    void DrawSprite(TextureHandle tex, Vec2 position, int frame)
}

capability network {
    requires auth
}

since 1.0.0 capability network {
    HttpResponse Send(HttpRequest request)
}

since 1.0.0 interface auth {
    bool ValidateToken(str token)
}

since 1.4.0 interface auth {
    optional void InvalidateSession(str token)
}

since 1.0.0 interface gamemode requires core {
    GameState InitGame(GameConfig config)
    void OnTick(GameState state, float deltaTime)
}

since 1.0.0 interface core {
    void Tick()
}
"#;

#[test]
fn parses_the_whitepaper_schema_shape() {
    let file = parse_clean(FULL_SCHEMA);
    assert_eq!(file.namespace, "engine");
    assert_eq!(file.version, Version::new(1, 4, 0));
    // Types + contracts, in declaration order.
    assert_eq!(file.structs().count(), 6);
    assert_eq!(file.enums().count(), 1);
    assert_eq!(
        file.capability("graphics").map(|c| c.members.len()),
        Some(3)
    );
    assert_eq!(
        file.interface("auth").map(|c| c.members.len()),
        Some(2),
        "optional member counts as a declared member"
    );
    let gamemode = file.interface("gamemode").unwrap();
    assert_eq!(
        gamemode.requires.as_ref().unwrap().segments,
        vec!["core".to_string()]
    );
    let network = file.capability("network").unwrap();
    assert_eq!(
        network.requires.as_ref().unwrap().segments,
        vec!["auth".to_string()]
    );

    // Member shape: since + signature.
    let load = &file.capability("graphics").unwrap().members[0];
    assert_eq!(load.name, "LoadTexture");
    assert_eq!(load.since, Version::new(1, 0, 0));
    assert_eq!(load.requirement, MemberRequirement::Required);
    assert_eq!(load.params.len(), 1);
    assert_eq!(load.params[0].name, "path");
    assert_eq!(
        load.return_ty,
        Type::Named {
            name: "TextureHandle".to_string(),
            args: vec![]
        }
    );

    // optional surfaces as MemberRequirement::Optional.
    let invalidate = &file.interface("auth").unwrap().members[1];
    assert_eq!(invalidate.requirement, MemberRequirement::Optional);
    assert_eq!(invalidate.since, Version::new(1, 4, 0));
}

#[test]
fn header_requires_the_full_shape() {
    assert!(
        parse_errors("capability x {}")
            .iter()
            .any(|m| m.contains("schema"))
    );
    assert!(
        parse_errors("schema engine")
            .iter()
            .any(|m| m.contains("version"))
    );
    assert!(
        parse_errors("schema 1.4.0")
            .iter()
            .any(|m| m.contains("namespace"))
    );
    assert!(
        parse_errors("schema engine 1.4")
            .iter()
            .any(|m| m.contains("version"))
    );
}

#[test]
fn the_v_prefix_is_no_longer_a_version() {
    // Versions share one shape across the toolchain: `X.Y.Z` in the header,
    // in `since` tags, and in a manifest's `[schemas]` targets. The old
    // `v`-prefixed header spelling is rejected with one pointed diagnostic,
    // and the version value itself is still recovered so the rest of the
    // file parses without cascades.
    let outcome =
        parse_schema_file("schema engine v1.4.0\n\ninterface core {\n    int Tick()\n}\n");
    assert_eq!(
        outcome
            .diagnostics
            .iter()
            .map(|d| d.message().to_string())
            .collect::<Vec<_>>(),
        vec!["schema versions are written `X.Y.Z` — drop the `v` prefix".to_string()],
        "exactly one migration diagnostic"
    );
    let file = outcome.file.expect("the header still recovers its version");
    assert_eq!(file.version, Version::new(1, 4, 0));
    assert_eq!(file.interface("core").map(|c| c.members.len()), Some(1));

    assert_eq!(parse_clean("schema engine 1.4.0\n").contracts().count(), 0);
}

#[test]
fn members_carry_the_declared_metadata() {
    // §9.5: the block's `since` versions every member it contains; blocks
    // of one contract union in declaration order.
    let file = parse_clean(
        "schema engine 1.0.0\n\
         since 2.1.3 interface core {\n\
         \x20 optional float Score()\n\
         }\n\
         interface core {\n\
         \x20 int Bare()\n\
         }\n",
    );
    let core = file.interface("core").unwrap();
    assert_eq!(core.members[0].since, Version::new(2, 1, 3));
    assert_eq!(core.members[0].requirement, MemberRequirement::Optional);
    // Untagged members exist since the beginning (§9.5).
    assert_eq!(core.members[1].since, Version::ZERO);
    assert_eq!(core.members[1].requirement, MemberRequirement::Required);
}

#[test]
fn suspend_members_parse_but_are_rejected() {
    let messages = parse_errors(
        "schema engine 1.0.0\n\
         capability net {\n\
         \x20 suspend httpResponse Get(str url)\n\
         }\n",
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("suspend") && m.contains("not supported"))
    );
}

#[test]
fn capitalization_is_enforced_on_schema_declarations() {
    let messages = parse_errors(
        "schema engine 1.0.0\n\
         capability graphics {\n\
         \x20 void drawTexture(str path)\n\
         }\n",
    );
    assert!(messages.iter().any(|m| m.contains("PascalCase")));

    let messages = parse_errors(
        "schema engine 1.0.0\n\
         capability Graphics {\n\
         \x20 void Draw(str Path)\n\
         }\n",
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("camelCase") && m.contains("Path"))
    );
}

#[test]
fn duplicates_and_void_placement_are_rejected() {
    let messages = parse_errors(
        "schema engine 1.0.0\n\
         struct Id { int value }\n\
         struct Id { int value }\n",
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("duplicate schema declaration"))
    );

    let messages = parse_errors(
        "schema engine 1.0.0\n\
         interface core {\n\
         \x20 int Tick()\n\
         \x20 int Tick()\n\
         }\n",
    );
    assert!(messages.iter().any(|m| m.contains("duplicate member")));

    let messages = parse_errors(
        "schema engine 1.0.0\n\
         struct Bad { void field }\n",
    );
    assert!(messages.iter().any(|m| m.contains("`void`")));
}

#[test]
fn types_cover_the_whole_surface() {
    let file = parse_clean(
        "schema engine 1.0.0\n\
         struct Bag {\n\
         \x20 int[] numbers\n\
         \x20 map<str, float> scores\n\
         \x20 option<str> label\n\
         \x20 result<int, str> attempt\n\
         \x20 str[][] grid\n\
         }\n\
         since 1.0.0 capability c {\n\
         \x20 map<str, int[]> Index(str key)\n\
         }\n",
    );
    let bag = match &file.items[0] {
        SchemaItem::Struct(decl) => decl,
        other => panic!("expected a struct, got {other:?}"),
    };
    assert_eq!(
        bag.fields[0].ty,
        Type::Array(Box::new(Type::Prim(PrimitiveType::Int)))
    );
    assert!(matches!(&bag.fields[1].ty, Type::Map { .. }));
    assert!(matches!(
        &bag.fields[2].ty,
        Type::Named { name, args } if name == "option" && args.len() == 1
    ));
    assert!(matches!(&bag.fields[4].ty, Type::Array(inner) if matches!(**inner, Type::Array(_))));
}

#[test]
fn set_build_enforces_cross_file_invariants() {
    // Duplicate namespace (§9.2).
    let a = parse_clean("schema engine 1.0.0\ninterface core { int Tick() }\n");
    let b = parse_clean("schema engine 2.0.0\ninterface core { int Tick() }\n");
    let issues = SchemaSet::build(vec![a.clone(), b]).unwrap_err();
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("duplicate schema namespace"))
    );

    // Cross-namespace type collision — the script-side type space is flat.
    let a = parse_clean(
        "schema engine 1.0.0\ninterface core { int Tick() }\nstruct Shared { int v }\n",
    );
    let b = parse_clean("schema physics 1.0.0\nstruct Shared { int at }\n");
    let issues = SchemaSet::build(vec![a.clone(), b]).unwrap_err();
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("unique across namespaces"))
    );

    // Unresolved requires (§9.4).
    let c = parse_clean("schema app 1.0.0\ncapability net requires missing { int Send() }\n");
    let issues = SchemaSet::build(vec![a.clone(), c]).unwrap_err();
    assert!(issues.iter().any(|i| i.message.contains("requires")));

    // A valid set with cross-namespace requires (§9.2 qualified shape).
    let d = parse_clean("schema hud 1.0.0\ninterface widgets { int Draw() }\n");
    let e = parse_clean("schema app 1.0.0\ninterface panel requires hud.widgets { int Show() }\n");
    let set = SchemaSet::build(vec![a, d, e]).expect("valid set");
    assert_eq!(set.namespaces().len(), 3);
    assert_eq!(set.namespace("hud").unwrap().version, Version::new(1, 0, 0));
}

#[test]
fn schema_context_grants_targets() {
    let set = SchemaSet::build(vec![
        parse_clean("schema engine 1.4.0\ninterface core { int Tick() }\n"),
        parse_clean("schema physics 1.0.0\ninterface rigid { int Step() }\n"),
    ])
    .expect("valid set");

    // Loose sources see everything at each schema's own version.
    let all = SchemaContext::grant_all(set.clone());
    assert_eq!(all.target("engine"), Some(Version::new(1, 4, 0)));
    assert_eq!(all.target("physics"), Some(Version::new(1, 0, 0)));
    assert_eq!(all.target("nope"), None);

    // Manifest targets narrow the grant and cap at the schema version.
    let granted = SchemaContext::grant_targets(
        set.clone(),
        vec![("engine".to_string(), "1.2.0".to_string())],
    )
    .expect("valid targets");
    assert_eq!(granted.target("engine"), Some(Version::new(1, 2, 0)));
    assert!(
        !granted.grants("physics"),
        "unlisted namespaces are invisible"
    );

    // A newer target than the host ships is rejected (§9.5).
    let issues = SchemaContext::grant_targets(
        set.clone(),
        vec![("engine".to_string(), "2.0.0".to_string())],
    )
    .unwrap_err();
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("cannot target a newer"))
    );

    // An unknown namespace is rejected.
    let issues =
        SchemaContext::grant_targets(set, vec![("audio".to_string(), "1.0.0".to_string())])
            .unwrap_err();
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("has not registered"))
    );
}

#[test]
fn comments_and_layout_are_accepted() {
    let file = parse_clean(
        "schema engine 1.0.0\n\
         /* block\n\
            comment */\n\
         interface core {\n\
         \x20 // a member\n\
         \x20 int Tick() // trailing\n\
         }\n",
    );
    assert_eq!(file.namespace, "engine");
    assert_eq!(file.contracts().count(), 1);
    assert_eq!(
        file.contracts().next().unwrap().kind,
        ContractKind::Interface
    );
}

#[test]
fn an_optional_capability_member_is_rejected() {
    // §9.5 defines `optional` for INTERFACE members a mod may skip. A
    // capability member is host-provided and must always exist, so the
    // flag has no meaning there — the parser rejects it with a pointed
    // diagnostic.
    let outcome = parse_schema_file(
        "schema engine 1.0.0\n\
         \n\
         capability gfx {\n\
         \x20   optional void Draw()\n\
         }\n",
    );
    assert!(
        !outcome.is_clean(),
        "`optional` on a capability member must be a schema defect"
    );
    assert!(
        outcome.diagnostics.iter().any(|d| d
            .message()
            .contains("`optional` is an interface-member concept")),
        "the diagnostic names the rule: {:?}",
        outcome.diagnostics
    );

    // The same flag on an interface member stays legal.
    let clean = parse_clean(
        "schema engine 1.0.0\n\
         \n\
         interface svc {\n\
         \x20   optional void Draw()\n\
         }\n",
    );
    assert_eq!(clean.contracts().count(), 1);
}

#[test]
fn a_member_introduced_after_the_schema_version_is_a_set_issue() {
    // §9.5: no target may exceed the schema's own version, so a member
    // tagged beyond it could never be visible — a forgotten version bump.
    let outcome = parse_clean(
        "schema shop 1.0.0\n\
         \n\
         interface backend {\n\
         \x20   bool Ping()\n\
         }\n\
         since 9.9.9 interface backend {\n\
         \x20   bool Pong()\n\
         }\n",
    );
    let issues = SchemaSet::build(vec![outcome]).expect_err("the set must reject it");
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("`shop.backend` member `Pong`")
                && i.message.contains("since 9.9.9")
                && i.message.contains("1.0.0")),
        "the issue names the member and both versions: {:?}",
        issues.iter().map(|i| &i.message).collect::<Vec<_>>()
    );
}

#[test]
fn option_and_result_member_types_parse() {
    // The §2.8 built-in sum types are legal anywhere a schema type can
    // appear — member returns, parameters, and field shapes.
    let file = parse_clean(
        r#"
schema shop 0.3.0

struct Price {
    int cents
}

since 0.1.0 capability pricing {
    result<Price, str> GetPrice(str sku)
    option<Price> PeekPrice(str sku)
}

since 0.2.0 capability pricing {
    result<int, str>[] BulkPrices(str[] skus)
}

struct Holder {
    option<Price> maybe
    result<Price, str> attempt
}
"#,
    );
    let get_price = &file.capability("pricing").unwrap().members[0];
    assert_eq!(
        get_price.return_ty,
        Type::Named {
            name: "result".to_string(),
            args: vec![
                Type::Named {
                    name: "Price".to_string(),
                    args: vec![]
                },
                Type::Prim(PrimitiveType::Str),
            ]
        }
    );
    let holder = file
        .structs()
        .find(|s| s.name == "Holder")
        .expect("the Holder struct");
    assert_eq!(
        holder.fields[0].ty,
        Type::Named {
            name: "option".to_string(),
            args: vec![Type::Named {
                name: "Price".to_string(),
                args: vec![]
            }]
        }
    );
}

#[test]
fn builtin_generic_arity_is_enforced_at_parse_time() {
    // A schema file is the contract's source of truth: a mis-arity
    // option/result is rejected here, not later inside a script.
    let errors = parse_errors(
        r#"
schema bad 0.1.0

struct Price {
    int cents
}

since 0.1.0 capability pricing {
    option<Price, int> Weird(str sku)
    result<Price> Missing(str sku)
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("the builtin `option` takes exactly 1 type argument, found 2")),
        "missing the option arity diagnostic: {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("the builtin `result` takes exactly 2 type arguments, found 1")),
        "missing the result arity diagnostic: {errors:?}"
    );
}

#[test]
fn reserved_mega_identifiers_are_rejected_in_schema_files() {
    let errors = parse_errors(
        r#"
schema engine 1.0.0

since 1.0.0 capability mega0 {
    void Tick()
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("identifier `mega0` is reserved for future use")),
        "missing the reserved-ident diagnostic: {errors:?}"
    );
}

#[test]
fn block_since_versions_every_member_of_the_block() {
    // §9.5's new shape: `since X.Y.Z capability fs { ... }` — every member
    // inside carries the block's version, and the same contract may be
    // declared in several blocks whose members union.
    let file = parse_clean(
        r#"
schema fs 0.2.0

since 0.1.0 capability fs {
    str GetConfigDir()
    bool PathExists(str path)
}

since 0.2.0 capability fs {
    bool CreateFile(str path)
}
"#,
    );
    let fs = file.capability("fs").expect("the fs capability");
    assert_eq!(fs.members.len(), 3, "blocks union into one member set");
    assert_eq!(fs.members[0].name, "GetConfigDir");
    assert_eq!(fs.members[0].since, Version::new(0, 1, 0));
    assert_eq!(fs.members[1].name, "PathExists");
    assert_eq!(fs.members[1].since, Version::new(0, 1, 0));
    assert_eq!(fs.members[2].name, "CreateFile");
    assert_eq!(fs.members[2].since, Version::new(0, 2, 0));
}

#[test]
fn a_member_level_since_reports_the_migration_diagnostic() {
    let messages = parse_errors(
        "schema engine 1.0.0\n\
         capability net {\n\
         \x20 since 1.0.0 bool Send(str url)\n\
         }\n",
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("member-level `since` was removed")),
        "the migration diagnostic points at the retired spelling: {messages:?}"
    );
}

#[test]
fn since_before_a_type_declaration_is_rejected() {
    let messages = parse_errors(
        "schema engine 1.0.0\n\
         since 1.0.0 struct Bag {\n\
         \x20 int id\n\
         }\n",
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`since` versions a capability or interface block")),
        "versioning is a contract-member concept: {messages:?}"
    );
}

#[test]
fn contract_unions_reject_cross_block_duplicates_and_double_requires() {
    let messages = parse_errors(
        "schema engine 1.0.0\n\
         since 1.0.0 interface gamemode requires core {\n\
         \x20 void Tick()\n\
         }\n\
         since 1.1.0 interface gamemode requires core {\n\
         \x20 void Tick()\n\
         \x20 void Save()\n\
         }\n",
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("duplicate member `Tick`")),
        "a member declared twice across blocks is reported: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("duplicate `requires` in interface `gamemode`")),
        "a second requires across blocks is reported: {messages:?}"
    );
}

#[test]
fn requires_may_land_in_any_block_of_the_contract() {
    let file = parse_clean(
        "schema engine 1.0.0\n\
         since 1.0.0 interface gamemode {\n\
         \x20 void Tick()\n\
         }\n\
         since 1.1.0 interface gamemode requires core {\n\
         \x20 void Save()\n\
         }\n",
    );
    let gamemode = file.interface("gamemode").unwrap();
    assert_eq!(
        gamemode.requires.as_ref().unwrap().segments,
        vec!["core".to_string()],
        "the requires declared in the second block attaches to the union"
    );
}

#[test]
fn contract_and_type_name_collisions_stay_rejected() {
    // Same name, different kind: a capability and an interface cannot
    // share a name, and neither can share one with a type.
    let messages = parse_errors(
        "schema engine 1.0.0\n\
         since 1.0.0 capability fs {\n\
         \x20 void Tick()\n\
         }\n\
         since 1.0.0 interface fs {\n\
         \x20 void Tick()\n\
         }\n",
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("duplicate schema declaration `fs`")),
        "a capability/interface name collision is reported: {messages:?}"
    );

    let messages = parse_errors(
        "schema engine 1.0.0\n\
         since 1.0.0 capability fs {\n\
         \x20 void Tick()\n\
         }\n\
         struct fs {\n\
         \x20 int id\n\
         }\n",
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("duplicate schema declaration `fs`")),
        "a contract/type name collision is reported: {messages:?}"
    );
}
