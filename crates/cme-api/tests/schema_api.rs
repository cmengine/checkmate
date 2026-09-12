//! End-to-end schema-driven host flows (WHITEPAPER §9, §13.1): register a
//! schema, register providers, load programs, invoke, and call back in
//! through the interface.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cme_api::{CapabilityProvider, Engine, ExecutionError, ExecutionLimits, Value};

const ENGINE_SCHEMA: &str = "
schema engine v1.4.0

struct TextureHandle {
    int id
}

struct Vec2 {
    float x
    float y
}

struct GameState {
    int score
    bool active
}

struct GameConfig {
    int score
    bool active
}

capability graphics {
    since 1.0.0 TextureHandle LoadTexture(str path)
    since 1.0.0 void DrawTexture(TextureHandle tex, Vec2 position)
    since 1.2.0 int DrawSprite(TextureHandle tex, Vec2 position, int frame)
}

interface gamemode {
    since 1.0.0 GameState InitGame(GameConfig config)
    since 1.0.0 void OnTick(GameState state, float deltaTime)
}
";

/// A provider that verifies the runtime contract it is called with: args
/// arrive in schema order, and results convert back into script values.
struct GraphicsProvider {
    loads: AtomicUsize,
}

impl CapabilityProvider for GraphicsProvider {
    fn call(&self, member: &str, args: &[Value]) -> Result<Value, String> {
        match member {
            "LoadTexture" => {
                self.loads.fetch_add(1, Ordering::SeqCst);
                let Value::Str(path) = &args[0] else {
                    return Err("LoadTexture expects a str path".to_string());
                };
                Ok(Value::Struct {
                    name: "TextureHandle".to_string(),
                    fields: vec![("id".to_string(), Value::Int(path.len() as i64))],
                })
            }
            "DrawTexture" => Ok(Value::Void),
            "DrawSprite" => {
                let Value::Int(frame) = args[2] else {
                    return Err("DrawSprite expects an int frame".to_string());
                };
                Ok(Value::Int(frame * 10))
            }
            other => Err(format!("graphics provider does not implement `{other}`")),
        }
    }
}

fn engine_with_schema() -> Engine {
    let mut engine = Engine::new();
    engine
        .load_schema_text(ENGINE_SCHEMA, "schemas/engine.cm")
        .expect("the test schema registers");
    engine
}

#[test]
fn a_schema_gated_program_loads_with_a_registered_provider() {
    let mut engine = engine_with_schema();
    engine
        .register_capability(
            "engine.graphics",
            Arc::new(GraphicsProvider {
                loads: AtomicUsize::new(0),
            }),
        )
        .expect("valid capability path");

    let program = engine
        .load_source(
            "import engine.graphics\n\
             int main() {\n\
             \x20TextureHandle tex = engine.graphics.LoadTexture(\"hero.png\")\n\
             \x20return tex.id\n\
             }\n",
        )
        .expect("the program satisfies the schema");
    assert_eq!(program.entry_points(), ["main"]);
}

#[test]
fn a_called_capability_without_a_provider_fails_the_load() {
    let engine = engine_with_schema();
    let error = engine
        .load_source(
            "import engine.graphics\n\
             int main() {\n\
             \x20engine.graphics.LoadTexture(\"hero.png\")\n\
             \x20return 0\n\
             }\n",
        )
        .expect_err("no provider registered");
    assert!(
        error.message().contains("engine.graphics") && error.message().contains("provider"),
        "expected a provider-presence diagnostic, got: {}",
        error.message()
    );
}

#[test]
fn capability_invocations_dispatch_to_the_provider() {
    let mut engine = engine_with_schema();
    let provider = Arc::new(GraphicsProvider {
        loads: AtomicUsize::new(0),
    });
    engine
        .register_capability("engine.graphics", provider.clone())
        .expect("valid capability path");

    let program = engine
        .load_source(
            "import engine.graphics\n\
             int main() {\n\
             \x20TextureHandle tex = engine.graphics.LoadTexture(\"hero.png\")\n\
             \x20engine.graphics.DrawTexture(tex, Vec2(x: 1.0, y: 2.0))\n\
             \x20int drawn = engine.graphics.DrawSprite(tex, Vec2(x: 0.0, y: 0.0), 7)\n\
             \x20return tex.id + drawn\n\
             }\n",
        )
        .expect("loads with the provider registered");
    let context = engine.create_context(&program, ExecutionLimits::default());
    let result = context.invoke("main", &[]).expect("runs clean");
    assert_eq!(result, Value::Int(8 + 70));
    assert_eq!(provider.loads.load(Ordering::SeqCst), 1);
}

#[test]
fn version_targets_come_from_the_mod_manifest() {
    let mut engine = engine_with_schema();
    engine
        .register_capability(
            "engine.graphics",
            Arc::new(GraphicsProvider {
                loads: AtomicUsize::new(0),
            }),
        )
        .expect("valid capability path");

    // The mod root with a manifest targeting 1.0.0: DrawSprite (since
    // 1.2.0) is hidden.
    let root = std::env::temp_dir().join(format!("cme_schema_mod_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("mod dir");
    std::fs::write(
        root.join("mod.toml"),
        "name = \"schema_mod\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.2.0\"\n\n[schemas]\nengine = \"1.0.0\"\n",
    )
    .expect("manifest");
    std::fs::write(
        root.join("src/main.cm"),
        "import engine.graphics\nint main() {\nreturn 0\n}\n",
    )
    .expect("source");

    engine.load_mod(&root).expect("1.0.0 target compiles");

    // Raise the mod's target to 1.4.0 and the same capability works.
    std::fs::write(
        root.join("mod.toml"),
        "name = \"schema_mod\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.2.0\"\n\n[schemas]\nengine = \"1.4.0\"\n",
    )
    .expect("manifest v2");
    std::fs::write(
        root.join("src/main.cm"),
        "import engine.graphics\nint main() {\nreturn engine.graphics.DrawSprite(TextureHandle(id: 1), Vec2(x: 0.0, y: 0.0), 3)\n}\n",
    )
    .expect("source v2");
    let program = engine.load_mod(&root).expect("1.4.0 target compiles");
    let context = engine.create_context(&program, ExecutionLimits::default());
    assert_eq!(context.invoke("main", &[]), Ok(Value::Int(30)));
    drop(program);

    // DrawSprite at target 1.0.0 is hidden.
    std::fs::write(
        root.join("mod.toml"),
        "name = \"schema_mod\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.2.0\"\n\n[schemas]\nengine = \"1.0.0\"\n",
    )
    .expect("manifest v3");
    let error = engine
        .load_mod(&root)
        .expect_err("DrawSprite is hidden at 1.0.0");
    assert!(
        error.message().contains("1.2.0"),
        "expected a version diagnostic, got: {}",
        error.message()
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_host_calls_back_in_through_the_interface() {
    let mut engine = engine_with_schema();
    engine
        .register_capability(
            "engine.graphics",
            Arc::new(GraphicsProvider {
                loads: AtomicUsize::new(0),
            }),
        )
        .expect("valid capability path");

    let program = engine
        .load_source(
            "impl engine.gamemode {\n\
             \x20GameState InitGame(GameConfig config) {\n\
             \x20\x20return GameState(score: config.score, active: config.active)\n\
             \x20}\n\
             \x20void OnTick(GameState state, float deltaTime) {\n\
             \x20}\n\
             }\n",
        )
        .expect("the interface is fully implemented");

    let context = engine.create_context(&program, ExecutionLimits::default());
    assert!(context.has_interface("engine.gamemode"));
    assert!(!context.has_interface("engine.core"));

    // The proxy-shaped call: typed construction happens host-side; here we
    // drive the raw invoke_member the generated proxy wraps.
    let config = Value::Struct {
        name: "GameConfig".to_string(),
        fields: vec![
            ("score".to_string(), Value::Int(42)),
            ("active".to_string(), Value::Bool(true)),
        ],
    };
    let state = context
        .invoke_member("engine.gamemode", "InitGame", &[config])
        .expect("the script implements the member");
    assert_eq!(
        state,
        Value::Struct {
            name: "GameState".to_string(),
            fields: vec![
                ("score".to_string(), Value::Int(42)),
                ("active".to_string(), Value::Bool(true)),
            ],
        }
    );
}

#[test]
fn schema_errors_surface_with_file_positions() {
    let mut engine = Engine::new();
    let error = engine
        .load_schema_text(
            "schema engine v1.4\ncapability broken {\nint NoParens\n}\n",
            "schemas/bad.cm",
        )
        .expect_err("a malformed schema is rejected");
    assert!(
        error
            .messages()
            .iter()
            .any(|m| m.contains("schemas/bad.cm")),
        "expected a positioned diagnostic, got: {:?}",
        error.messages()
    );

    // A well-formed schema with an unresolved requires is rejected as a
    // SET error.
    let error = engine
        .load_schema_text(
            "schema app v1.0.0\ncapability net requires missing { int Send() }\n",
            "schemas/app.cm",
        )
        .expect_err("requires must resolve");
    assert!(
        error.message().contains("requires"),
        "expected a requires diagnostic, got: {}",
        error.message()
    );
}

#[test]
fn capability_paths_are_validated_at_registration() {
    let mut engine = Engine::new();
    assert!(
        engine
            .register_capability(
                "graphics",
                Arc::new(GraphicsProvider {
                    loads: AtomicUsize::new(0)
                })
            )
            .is_err()
    );
    assert!(
        engine
            .register_capability(
                "engine.graphics.extra",
                Arc::new(GraphicsProvider {
                    loads: AtomicUsize::new(0)
                })
            )
            .is_err()
    );
    assert!(
        engine
            .register_capability(
                "engine.graphics",
                Arc::new(GraphicsProvider {
                    loads: AtomicUsize::new(0)
                })
            )
            .is_ok()
    );
}

#[test]
fn without_a_schema_the_old_behavior_holds() {
    // No schema registered: host-style paths stay accepted, nothing is
    // gated (the pre-schema behavior hosts may rely on).
    let mut engine = Engine::new();
    engine
        .register_capability(
            "engine.graphics",
            Arc::new(GraphicsProvider {
                loads: AtomicUsize::new(0),
            }),
        )
        .expect("valid path");
    let program = engine
        .load_source("impl engine.legacy {\nint Legacy(str name) {\nreturn 1\n}\n}\n")
        .expect("no schema, no gate");
    let context = engine.create_context(&program, ExecutionLimits::default());
    assert_eq!(
        context.invoke_member("engine.legacy", "Legacy", &[Value::Str("x".into())]),
        Ok(Value::Int(1))
    );
}

/// The [`ExecutionError`] shape a generated proxy converts its
/// `from_value` failures into (kind Runtime, no position).
#[test]
fn conversion_errors_are_ordinary_runtime_errors() {
    let error = ExecutionError {
        kind: cme_api::ErrorKind::Runtime,
        message: "expected TextureHandle".to_string(),
        line: 0,
        column: 0,
        file: None,
        span: None,
    };
    assert_eq!(error.render(), "expected TextureHandle");
}
