//! Script-side schema enforcement (WHITEPAPER §2.5, §9.1–§9.5, §10.4):
//! programs checked with [`check_with_schema`] against a registered
//! schema set.

use cme_compiler::Diagnostic;
use cme_compiler::check::check_with_schema;
use cme_compiler::schema::{SchemaContext, SchemaSet, parse_schema_file};

/// Parses one schema file and grants every namespace at its own version —
/// the loose-source default.
fn schema(text: &str) -> SchemaContext {
    let outcome = parse_schema_file(text);
    assert!(
        outcome.is_clean(),
        "test schema must parse cleanly: {:?}",
        outcome
            .diagnostics
            .iter()
            .map(|d| d.message().to_string())
            .collect::<Vec<_>>()
    );
    let set = SchemaSet::build(vec![outcome.file.expect("file")]).expect("set");
    SchemaContext::grant_all(set)
}

/// A schema set granting `engine` at an explicit target version.
fn schema_at(version: &str) -> SchemaContext {
    let outcome = parse_schema_file(ENGINE);
    assert!(outcome.is_clean(), "the test schema must parse cleanly");
    let set = SchemaSet::build(vec![outcome.file.expect("file")]).expect("set");
    SchemaContext::grant_targets(set, vec![("engine".to_string(), version.to_string())])
        .expect("valid target")
}

fn check_with(program: &str, schema: &SchemaContext) -> Vec<Diagnostic> {
    let outcome = cme_compiler::parse_source(program);
    assert!(
        outcome.is_clean(),
        "test program must parse cleanly: {:?}",
        outcome
            .diagnostics
            .iter()
            .map(|d| d.message().to_string())
            .collect::<Vec<_>>()
    );
    check_with_schema(&outcome.statements, Some(schema))
}

fn diagnostic_messages(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|d| d.message().to_string())
        .collect()
}

const ENGINE: &str = "
schema engine 1.4.0

struct TextureHandle {
    int id
}

struct Vec2 {
    float x
    float y
}

struct GameConfig {
    int score
    bool active
}

struct GameState {
    int score
    bool active
}

enum LoadError {
    NotFound()
    Corrupt(str reason)
}

capability graphics {
    since 1.0.0 TextureHandle LoadTexture(str path)
    since 1.0.0 void DrawTexture(TextureHandle tex, Vec2 position)
    since 1.2.0 void DrawSprite(TextureHandle tex, Vec2 position, int frame)
}

capability assets requires loader {
    since 1.0.0 TextureHandle LoadBundled(str name)
}

interface loader {
    since 1.0.0 bool IsAvailable(str name)
}

interface core {
    since 1.0.0 void Tick()
}

interface gamemode requires core {
    since 1.0.0 GameState InitGame(GameConfig config)
    since 1.0.0 void OnTick(GameState state, float deltaTime)
    since 1.4.0 optional void OnSave(str path)
}
";

#[test]
fn a_well_formed_schema_program_checks_clean() {
    let schema = schema(ENGINE);
    let program = "
import engine.graphics

int main() {
    TextureHandle tex = engine.graphics.LoadTexture(\"hero.png\")
    engine.graphics.DrawTexture(tex, Vec2(x: 1.0, y: 2.0))
    engine.graphics.DrawSprite(tex, Vec2(x: 0.0, y: 0.0), 3)
    return tex.id
}

impl engine.core {
    void Tick() {
    }
}

impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: config.score, active: config.active)
    }
    void OnTick(GameState state, float deltaTime) {
    }
    void OnSave(str path) {
    }
}
";
    let diagnostics = check_with(program, &schema);
    assert!(
        diagnostics.is_empty(),
        "expected a clean check, got: {:?}",
        diagnostic_messages(&diagnostics)
    );
}

#[test]
fn capability_calls_type_check_against_the_schema() {
    let schema = schema(ENGINE);
    // Wrong argument type: the schema says LoadTexture(str).
    let program = "
import engine.graphics
int main() {
    TextureHandle tex = engine.graphics.LoadTexture(42)
    return 0
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("LoadTexture") && m.contains("`int`")),
        "expected a parameter type diagnostic, got: {messages:?}"
    );

    // Wrong arity.
    let program = "
import engine.graphics
int main() {
    engine.graphics.DrawTexture(TextureHandle(id: 1))
    return 0
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("DrawTexture") && m.contains("expected 2")),
        "expected an arity diagnostic, got: {messages:?}"
    );

    // Unknown member.
    let program = "
import engine.graphics
int main() {
    engine.graphics.LoadSound(\"boom\")
    return 0
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages.iter().any(|m| m.contains("no member `LoadSound`")),
        "expected an unknown-member diagnostic, got: {messages:?}"
    );

    // Return type flows: the result of a capability call is the schema's
    // declared type.
    let program = "
import engine.graphics
int main() {
    int tex = engine.graphics.LoadTexture(\"hero.png\")
    return 0
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`TextureHandle`") && m.contains("`int`")),
        "expected a return-type diagnostic, got: {messages:?}"
    );
}

#[test]
fn capability_calls_require_the_import() {
    let schema = schema(ENGINE);
    let program = "
int main() {
    engine.graphics.LoadTexture(\"hero.png\")
    return 0
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("requires importing") && m.contains("engine.graphics")),
        "expected an import-required diagnostic, got: {messages:?}"
    );
}

#[test]
fn version_gating_hides_newer_members() {
    // DrawSprite is `since 1.2.0`; target 1.0.0 hides it.
    let granted = schema_at("1.0.0");
    let program = "
import engine.graphics
int main() {
    engine.graphics.DrawSprite(TextureHandle(id: 1), Vec2(x: 0.0, y: 0.0), 1)
    return 0
}
";
    let messages = diagnostic_messages(&check_with(program, &granted));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("DrawSprite") && m.contains("1.2.0") && m.contains("1.0.0")),
        "expected a version-gating diagnostic, got: {messages:?}"
    );

    // The same program checks clean at the schema's own version.
    assert!(check_with(program, &schema_at("1.4.0")).is_empty());
}

#[test]
fn unimplemented_capabilities_are_blocked_by_requires() {
    let schema = schema(ENGINE);
    // `assets requires loader` — no impl of engine.loader: the call is a
    // compile error (§9.4 rule 2).
    let program = "
import engine.assets
int main() {
    engine.assets.LoadBundled(\"title\")
    return 0
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("engine.assets") && m.contains("engine.loader")),
        "expected a requires diagnostic, got: {messages:?}"
    );

    // Implementing the prerequisite unlocks the capability.
    let program = "
import engine.assets
int main() {
    engine.assets.LoadBundled(\"title\")
    return 0
}
impl engine.loader {
    bool IsAvailable(str name) {
        return true
    }
}
";
    assert!(check_with(program, &schema).is_empty());
}

#[test]
fn interface_impls_must_be_complete_and_exact() {
    let schema = schema(ENGINE);

    // Missing a required member (§10.4).
    let program = "
impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: 0, active: true)
    }
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("not fully implemented") && m.contains("OnTick")),
        "expected a completeness diagnostic, got: {messages:?}"
    );

    // Wrong parameter type.
    let program = "
impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: 0, active: true)
    }
    void OnTick(GameState state, int deltaTime) {
    }
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("OnTick") && m.contains("float")),
        "expected a signature diagnostic, got: {messages:?}"
    );

    // Wrong return type.
    let program = "
impl engine.gamemode {
    int InitGame(GameConfig config) {
        return 0
    }
    void OnTick(GameState state, float deltaTime) {
    }
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("InitGame") && m.contains("returns `int`")),
        "expected a return-type diagnostic, got: {messages:?}"
    );

    // A member the interface does not declare.
    let program = "
impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: 0, active: true)
    }
    void OnTick(GameState state, float deltaTime) {
    }
    void OnPause() {
    }
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("OnPause") && m.contains("not a member")),
        "expected an unknown-member diagnostic, got: {messages:?}"
    );

    // Unknown interface target.
    let program = "
impl engine.racing {
    void Drive() {
    }
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("unknown interface `engine.racing`")),
        "expected an unknown-interface diagnostic, got: {messages:?}"
    );

    // Implementing a capability is refused.
    let program = "
impl engine.graphics {
    void Paint() {
    }
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("cannot implement capability")),
        "expected a capability-impl diagnostic, got: {messages:?}"
    );
}

#[test]
fn optional_members_may_be_skipped_until_needed() {
    let schema = schema(ENGINE);
    // OnSave is `optional` and `since 1.4.0`: skipping it is fine at any
    // target version.
    let program = "
impl engine.core {
    void Tick() {
    }
}
impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: 0, active: true)
    }
    void OnTick(GameState state, float deltaTime) {
    }
}
";
    assert!(check_with(program, &schema).is_empty());

    // A program targeting an older version may not implement the newer
    // optional member at all — it is hidden.
    let program = "
impl engine.core {
    void Tick() {
    }
}
impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: 0, active: true)
    }
    void OnTick(GameState state, float deltaTime) {
    }
    void OnSave(str path) {
    }
}
";
    let granted = schema_at("1.2.0");
    let messages = diagnostic_messages(&check_with(program, &granted));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("OnSave") && m.contains("1.4.0") && m.contains("1.2.0")),
        "expected a hidden-member diagnostic, got: {messages:?}"
    );
}

#[test]
fn interface_requires_pulls_in_the_prerequisite() {
    let schema = schema(ENGINE);
    // gamemode requires core (§9.4 rule 1).
    let program = "
impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: 0, active: true)
    }
    void OnTick(GameState state, float deltaTime) {
    }
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("engine.gamemode") && m.contains("engine.core")),
        "expected a requires diagnostic, got: {messages:?}"
    );

    // With core implemented, gamemode checks clean.
    let program = "
impl engine.core {
    void Tick() {
    }
}
impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: 0, active: true)
    }
    void OnTick(GameState state, float deltaTime) {
    }
}
";
    assert!(check_with(program, &schema).is_empty());
}

#[test]
fn boundary_capitalization_is_enforced_against_the_contract() {
    let schema = schema(ENGINE);
    // Lowercase entries stay fine.
    let program = "
int main() {
    return 0
}
";
    assert!(check_with(program, &schema).is_empty());

    // A PascalCase top-level function is a boundary declaration the schema
    // does not declare (§2.5).
    let program = "
int Main() {
    return 0
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`Main`") && m.contains("camelCase")),
        "expected a capitalization diagnostic, got: {messages:?}"
    );

    // A PascalCase script struct likewise.
    let program = "
struct Player {
    int hp
}
int main() {
    return 0
}
";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`Player`") && m.contains("camelCase")),
        "expected a capitalization diagnostic, got: {messages:?}"
    );

    // camelCase internal types stay fine, and schema types remain
    // constructible from scripts (§9.3, §10.4).
    let program = "
struct playerState {
    Vec2 position
    int health
}
int main() {
    playerState state = playerState(position: Vec2(x: 0.0, y: 0.0), health: 100)
    return state.health
}
";
    assert!(check_with(program, &schema).is_empty());
}

#[test]
fn imports_resolve_against_the_registered_set() {
    let schema = schema(ENGINE);
    let program = "import physics.rigid\nint main() {\nreturn 0\n}\n";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("unknown schema namespace `physics`")),
        "expected an unknown-namespace diagnostic, got: {messages:?}"
    );

    // Importing an interface is refused: interfaces are implemented.
    let program = "import engine.gamemode\nint main() {\nreturn 0\n}\n";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("engine.gamemode") && m.contains("interface")),
        "expected an interface-import diagnostic, got: {messages:?}"
    );

    // Deep import paths are rejected (namespace or namespace.capability).
    let program = "import engine.graphics.extra\nint main() {\nreturn 0\n}\n";
    let messages = diagnostic_messages(&check_with(program, &schema));
    assert!(
        messages.iter().any(|m| m.contains("namespace.capability")),
        "expected an import-shape diagnostic, got: {messages:?}"
    );
}

#[test]
fn schema_types_flow_through_scripts_and_impls() {
    let schema = schema(ENGINE);
    // Schema enums construct, match, and flow across the boundary — the
    // same shapes the host bindings pack and unpack.
    let program = "
import engine.graphics

str describe(LoadError error) {
    return match (error) {
        NotFound() => \"missing\"
        Corrupt(str reason) => \"corrupt: \" + reason
    }
}

int main() {
    TextureHandle tex = engine.graphics.LoadTexture(\"bg.png\")
    engine.graphics.DrawTexture(tex, Vec2(x: 1.5, y: 0.0))
    return tex.id
}

impl engine.core {
    void Tick() {
    }
}

impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: config.score, active: config.active)
    }
    void OnTick(GameState state, float deltaTime) {
    }
    void OnSave(str path) {
    }
}
";
    let diagnostics = check_with(program, &schema);
    assert!(
        diagnostics.is_empty(),
        "expected a clean check, got: {:?}",
        diagnostic_messages(&diagnostics)
    );
}

// ---------------------------------------------------------------------------
// §9.2 cross-namespace contracts (§9.4 over qualified requires paths)
// ---------------------------------------------------------------------------

/// Two namespaces, one requiring an interface of the other (§9.2: "Cross-
/// schema interface dependencies use qualified paths: `interface hud
/// requires ui.widgets`").
const CROSS: &str = "
schema engine 1.0.0

interface hud requires ui.widgets {
    since 1.0.0 void DrawHud()
}

capability overlay requires ui.widgets {
    since 1.0.0 void Show(str text)
}

schema ui 1.2.0

interface widgets {
    since 1.0.0 void Layout(int slot)
    since 1.2.0 optional void Blink(int slot)
}
";

#[test]
fn cross_namespace_requires_resolves_at_set_build() {
    // CROSS holds both namespaces in one string; split on the second
    // header and build one set: the qualified edge resolves into `ui`.
    let (first, second) = CROSS.split_once("schema ui").expect("two headers");
    let mut parsed = Vec::new();
    for text in [first.to_string(), format!("schema ui{second}")] {
        let outcome = parse_schema_file(&text);
        assert!(
            outcome.is_clean(),
            "fixtures must parse: {:?}",
            outcome.diagnostics
        );
        parsed.push(outcome.file.expect("file"));
    }
    let set = SchemaSet::build(parsed);
    assert!(
        set.is_ok(),
        "the qualified cross-namespace edge must resolve"
    );
}

#[test]
fn implementing_an_interface_requires_the_cross_namespace_prerequisite() {
    let context = cross_context();
    // hud implemented, ui.widgets NOT: §9.4 rule 1 fails across namespaces.
    let diagnostics = check_with("impl engine.hud {\nvoid DrawHud() {\n}\n}\n", &context);
    let messages = diagnostic_messages(&diagnostics);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("engine.hud") && m.contains("ui.widgets")),
        "the cross-namespace prerequisite must be demanded: {messages:?}"
    );
}

#[test]
fn the_cross_namespace_prerequisite_satisfies_the_rule() {
    let context = cross_context();
    // Both interfaces implemented: the mod passes §9.4 rule 1.
    let diagnostics = check_with(
        "impl engine.hud {\nvoid DrawHud() {\n}\n}\nimpl ui.widgets {\nvoid Layout(int slot) {\n}\n}\n",
        &context,
    );
    assert!(
        diagnostics.is_empty(),
        "both namespaces implemented: {:?}",
        diagnostic_messages(&diagnostics)
    );
}

#[test]
fn a_capability_requires_its_cross_namespace_interface_before_calls() {
    let context = cross_context();
    // The overlay capability requires ui.widgets; calling Show without
    // implementing the prerequisite is blocked (§9.4 rule 2).
    let diagnostics = check_with(
        "import engine.overlay\nint main() {\nengine.overlay.Show(\"hi\")\nreturn 0\n}\n",
        &context,
    );
    let messages = diagnostic_messages(&diagnostics);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("engine.overlay") && m.contains("ui.widgets")),
        "the capability call must demand the prerequisite: {messages:?}"
    );

    // With the prerequisite fully implemented, the call checks clean.
    let diagnostics = check_with(
        "import engine.overlay\nimpl ui.widgets {\nvoid Layout(int slot) {\n}\n}\nint main() {\nengine.overlay.Show(\"hi\")\nreturn 0\n}\n",
        &context,
    );
    assert!(
        diagnostics.is_empty(),
        "prerequisite satisfied: {:?}",
        diagnostic_messages(&diagnostics)
    );
}

#[test]
fn requires_may_not_name_a_capability() {
    const BAD: &str = "
schema a 1.0.0

interface broken requires b.helper {
    since 1.0.0 void Go()
}

schema b 1.0.0

capability helper {
    since 1.0.0 void Do()
}
";
    let (first, second) = BAD.split_once("schema b").expect("two headers");
    let mut files = Vec::new();
    for text in [first.to_string(), format!("schema b{second}")] {
        let outcome = parse_schema_file(&text);
        assert!(outcome.is_clean());
        files.push(outcome.file.expect("file"));
    }
    let issues = SchemaSet::build(files).expect_err("a requires edge must name an INTERFACE");
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("a.broken") && issue.message.contains("b.helper")),
        "the issue names the broken edge: {:?}",
        issues.iter().map(|i| i.message.clone()).collect::<Vec<_>>()
    );
}

/// Builds the CROSS fixture into a grant-all context (the split the other
/// tests repeat).
fn cross_context() -> SchemaContext {
    let (first, second) = CROSS.split_once("schema ui").expect("two headers");
    let mut files = Vec::new();
    for text in [first.to_string(), format!("schema ui{second}")] {
        let outcome = parse_schema_file(&text);
        assert!(outcome.is_clean());
        files.push(outcome.file.expect("file"));
    }
    let set = SchemaSet::build(files).expect("set");
    SchemaContext::grant_all(set)
}

// ---------------------------------------------------------------------------
// §9.4 completeness is version-aware (§9.5)
// ---------------------------------------------------------------------------

/// A prerequisite whose newest member is hidden at older targets: the
/// requires-completeness rule must judge against VISIBLE members only.
const GATING: &str = "
schema gating 2.0.0

interface base {
    since 1.0.0 void Basic()
    since 2.0.0 void Future()
}

interface derived requires base {
    since 1.0.0 void Do()
}
";

fn gating_context(version: &str) -> SchemaContext {
    let outcome = parse_schema_file(GATING);
    assert!(outcome.is_clean());
    let set = SchemaSet::build(vec![outcome.file.expect("file")]).expect("set");
    SchemaContext::grant_targets(set, vec![("gating".to_string(), version.to_string())])
        .expect("valid target")
}

#[test]
fn prerequisite_completeness_follows_the_target_version() {
    // At target 1.0.0, base.Future (since 2.0.0) is HIDDEN: implementing
    // the visible Basic satisfies the requires edge.
    let diagnostics = check_with(
        "impl gating.derived {\nvoid Do() {\n}\n}\nimpl gating.base {\nvoid Basic() {\n}\n}\n",
        &gating_context("1.0.0"),
    );
    assert!(
        diagnostics.is_empty(),
        "visible members complete at 1.0.0: {:?}",
        diagnostic_messages(&diagnostics)
    );

    // At target 2.0.0 base.Future is visible and REQUIRED: the same shape
    // now fails completeness — on the prerequisite, through requires.
    let diagnostics = check_with(
        "impl gating.derived {\nvoid Do() {\n}\n}\nimpl gating.base {\nvoid Basic() {\n}\n}\n",
        &gating_context("2.0.0"),
    );
    let messages = diagnostic_messages(&diagnostics);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("gating.base") && m.contains("Future")),
        "the newly visible member is demanded: {messages:?}"
    );
}

#[test]
fn an_unimplemented_prerequisite_fails_the_requires_rule() {
    // §9.4 rule 1, the minimal negative: the derived interface is fully
    // implemented, but the prerequisite has no impl at all.
    let context = gating_context("2.0.0");
    let diagnostics = check_with("impl gating.derived {\nvoid Do() {\n}\n}\n", &context);
    let messages = diagnostic_messages(&diagnostics);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("gating.derived") && m.contains("gating.base")),
        "requires fires when the prerequisite has no impl at all: {messages:?}"
    );
}

// ---------------------------------------------------------------------------
// §9.4 rule 2 at IMPORT time (the whitepaper's "cannot import or call")
// ---------------------------------------------------------------------------

#[test]
fn importing_a_capability_requires_its_prerequisite_at_import_time() {
    // `assets requires loader`: implementing loader is demanded by the
    // IMPORT itself, before any call site exists (§9.4 rule 2).
    let context = schema(ENGINE);
    let diagnostics = check_with(
        "import engine.assets\n\nvoid run() {\n    int x = 1\n}\n",
        &context,
    );
    let messages = diagnostic_messages(&diagnostics);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("importing `engine.assets`") && m.contains("engine.loader")),
        "the import must be gated by the prerequisite: {messages:?}"
    );
}

#[test]
fn importing_a_capability_with_the_prerequisite_implemented_checks_clean() {
    // The §9.4 import gate lifts as soon as the prerequisite interface is
    // fully implemented — even though the capability is never called.
    let context = schema(ENGINE);
    let diagnostics = check_with(
        "import engine.assets\n\nimpl engine.loader {\n    bool IsAvailable(str name) {\n        return name != \"\"\n    }\n}\n\nvoid run() {\n    int x = 1\n}\n",
        &context,
    );
    assert!(
        diagnostics.is_empty(),
        "an implemented prerequisite lifts the import gate: {:?}",
        diagnostic_messages(&diagnostics)
    );
}

// ---------------------------------------------------------------------------
// §9.5/§7.2: impl targets against ungranted or unknown namespaces
// ---------------------------------------------------------------------------

#[test]
fn implementing_an_interface_of_an_ungranted_namespace_is_an_error() {
    // The namespace is registered but the program was not granted it: the
    // impl would otherwise ship an interface the schema never validated.
    let outcome = parse_schema_file(ENGINE);
    assert!(outcome.is_clean(), "fixture must parse");
    let set = SchemaSet::build(vec![outcome.file.expect("file")]).expect("set");
    let context = SchemaContext::grant_targets(set, Vec::new()).expect("empty grant");
    let diagnostics = check_with(
        "impl engine.gamemode {\n    void OnTick(GameState state, float deltaTime) {\n    }\n}\n",
        &context,
    );
    let messages = diagnostic_messages(&diagnostics);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("not granted") && m.contains("engine.gamemode")),
        "an impl outside the grant must be reported: {messages:?}"
    );
}

#[test]
fn implementing_an_interface_of_an_unknown_namespace_is_an_error() {
    // With a contract active, a dotted impl target must name a schema
    // interface: `ghost` is declared by no registered schema.
    let context = schema(ENGINE);
    let diagnostics = check_with(
        "impl ghost.thing {\n    void DoIt() {\n    }\n}\n",
        &context,
    );
    let messages = diagnostic_messages(&diagnostics);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("unknown schema namespace `ghost`")),
        "an impl of an unknown namespace must be reported: {messages:?}"
    );
}
