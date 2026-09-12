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

// ---------------------------------------------------------------------------
// The engine-agnostic seam (§9, §13.1): providers dispatch identically
// whichever execution engine sits underneath — the tree walker today, the
// bytecode VM / LLVM AOT (§5) tomorrow — because both sides only share
// `CapabilityHost` + `Value`.
// ---------------------------------------------------------------------------

/// The same provider logic, expressed through cme-interp's raw
/// `CapabilityHost` seam (what a different execution engine would call
/// through): it must behave EXACTLY like the `Context` path.
struct SeamProvider;

impl cme_interp::CapabilityHost for SeamProvider {
    fn call(&self, path: &[&str], member: &str, args: &[Value]) -> Result<Value, String> {
        assert_eq!(path, &["engine", "graphics"]);
        match member {
            "LoadTexture" => {
                let Value::Str(path) = &args[0] else {
                    return Err("LoadTexture expects a str path".to_string());
                };
                Ok(Value::Struct {
                    name: "TextureHandle".to_string(),
                    fields: vec![("id".to_string(), Value::Int(path.len() as i64))],
                })
            }
            other => Err(format!("seam provider does not implement `{other}`")),
        }
    }
}

#[test]
fn the_raw_interpreter_seam_and_the_context_dispatch_identically() {
    let program_text = "import engine.graphics\n\
         int main() {\n\
         \x20TextureHandle tex = engine.graphics.LoadTexture(\"hero.png\")\n\
         \x20return tex.id\n\
         }\n";

    // Path 1: the raw interpreter with a capability host attached — the
    // shape ANY future execution engine reuses.
    let outcome = cme_compiler::parse_source(program_text);
    assert!(outcome.is_clean(), "the fixture parses");
    let schema_outcome = cme_compiler::schema::parse_schema_file(ENGINE_SCHEMA);
    assert!(schema_outcome.is_clean());
    let set = cme_compiler::schema::SchemaSet::build(vec![schema_outcome.file.expect("file")])
        .expect("set");
    let context_schema = cme_compiler::schema::SchemaContext::grant_all(set);
    let declarations = cme_compiler::schema::declaration_statements(&context_schema);
    let interpreter = cme_interp::Interpreter::new(&outcome.statements)
        .with_declarations(&declarations)
        .with_capabilities(&SeamProvider);
    let raw = interpreter
        .invoke("main", &[])
        .expect("the seam dispatches");
    assert_eq!(raw, Value::Int(8));

    // Path 2: the same program through Engine/Context, provider logic
    // equivalent — the host-visible result must not differ.
    let mut engine = engine_with_schema();
    engine
        .register_capability(
            "engine.graphics",
            Arc::new(GraphicsProvider {
                loads: AtomicUsize::new(0),
            }),
        )
        .expect("registers");
    let program = engine.load_source(program_text).expect("loads");
    let context = engine.create_context(&program, ExecutionLimits::default());
    let api = context.invoke("main", &[]).expect("the context dispatches");
    assert_eq!(api, raw);
}

#[test]
fn a_mod_with_a_manifest_dispatches_capability_calls_to_providers() {
    // The §10 mod shape of the same contract: [schemas] narrows the grant,
    // the capability call inside a module file dispatches, and the impl
    // member the host enters stays invocable.
    let root = std::env::temp_dir().join(format!("cme_schema_mod_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("mod tree");
    std::fs::write(
        root.join("mod.toml"),
        "name = \"schema_mod\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.2.0\"\n\n[schemas]\nengine = \"1.2.0\"\n",
    )
    .expect("manifest");
    std::fs::write(
        root.join("src/main.cm"),
        "import engine.graphics\n\
         impl engine.gamemode {\n\
         \x20GameState InitGame(GameConfig config) {\n\
         \x20\x20return GameState(score: config.score, active: config.active)\n\
         \x20}\n\
         \x20void OnTick(GameState state, float deltaTime) {\n\
         \x20}\n\
         }\n\
         int main() {\n\
         \x20TextureHandle tex = engine.graphics.LoadTexture(\"poster\")\n\
         \x20engine.graphics.DrawTexture(tex, Vec2(x: 0.0, y: 0.0))\n\
         \x20return tex.id\n\
         }\n",
    )
    .expect("main module");

    let mut engine = engine_with_schema();
    engine
        .register_capability(
            "engine.graphics",
            Arc::new(GraphicsProvider {
                loads: AtomicUsize::new(0),
            }),
        )
        .expect("registers");

    let program = engine.load_mod(&root).expect("the mod loads");
    let context = engine.create_context(&program, ExecutionLimits::default());
    assert_eq!(
        context.invoke("main", &[]),
        Ok(Value::Int("poster".len() as i64))
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_mod_calling_a_capability_without_a_provider_fails_the_load() {
    // The provider-presence gate covers mods too — a call the host cannot
    // dispatch is a compile-time failure of the load, not a runtime one.
    let root = std::env::temp_dir().join(format!("cme_schema_mod_noprov_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("mod tree");
    std::fs::write(
        root.join("mod.toml"),
        "name = \"noprov\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.2.0\"\n\n[schemas]\nengine = \"1.0.0\"\n",
    )
    .expect("manifest");
    std::fs::write(
        root.join("src/main.cm"),
        "import engine.graphics\nint main() {\nengine.graphics.DrawTexture(TextureHandle(id: 1), Vec2(x: 0.0, y: 0.0))\nreturn 0\n}\n",
    )
    .expect("main module");

    let engine = engine_with_schema();
    let error = engine.load_mod(&root).expect_err("no provider registered");
    assert!(
        error
            .messages()
            .iter()
            .any(|m| m.contains("engine.graphics") && m.contains("provider")),
        "the load names the missing provider: {:?}",
        error.messages()
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_host_side_handle_implements_the_proxy_trait_over_the_value_seam() {
    // The proxy surface is not reserved for the macro: ANY host handle
    // built on Context + Value (i.e. the engine-agnostic seam) implements
    // InterfaceProxy and plugs into get_interface. A future execution
    // engine behind Context would serve this call unchanged.
    struct OnTickHandle<'a, 'p> {
        context: &'a cme_api::Context<'p>,
    }

    impl<'a, 'p> cme_api::InterfaceProxy<'a, 'p> for OnTickHandle<'a, 'p> {
        fn from_context(context: &'a cme_api::Context<'p>) -> Result<Self, ExecutionError> {
            if !context.has_interface("engine.gamemode") {
                return Err(ExecutionError {
                    kind: cme_api::ErrorKind::UnknownEntry,
                    message: "engine.gamemode is not implemented".to_string(),
                    line: 0,
                    column: 0,
                    file: None,
                    span: None,
                });
            }
            Ok(OnTickHandle { context })
        }
    }

    impl<'a, 'p> OnTickHandle<'a, 'p> {
        fn on_tick(&self, state: Value, delta: f64) -> Result<Value, ExecutionError> {
            self.context
                .invoke_member("engine.gamemode", "OnTick", &[state, Value::Float(delta)])
        }
    }

    let engine = engine_with_schema();
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
        .expect("loads");
    let context = engine.create_context(&program, ExecutionLimits::default());

    let handle: OnTickHandle = context.get_interface().expect("implements");
    let state = Value::Struct {
        name: "GameState".to_string(),
        fields: vec![
            ("score".to_string(), Value::Int(1)),
            ("active".to_string(), Value::Bool(true)),
        ],
    };
    assert_eq!(handle.on_tick(state, 0.5), Ok(Value::Void));

    // The guard fires for programs without the interface.
    let other = engine
        .load_source("int main() {\nreturn 0\n}\n")
        .expect("loads");
    let context = engine.create_context(&other, ExecutionLimits::default());
    let error = match context.get_interface::<OnTickHandle>() {
        Err(error) => error,
        Ok(_) => panic!("the guard must fail for a program without the interface"),
    };
    assert_eq!(error.kind, cme_api::ErrorKind::UnknownEntry);
}

// ---------------------------------------------------------------------------
// §5.7 reentrancy protection
// ---------------------------------------------------------------------------

use std::sync::Mutex;

/// The provider captures a `&'static Context` and invokes into it from
/// INSIDE a script-triggered dispatch — the §5.7 shape a host capability
/// is forbidden from exercising on the same context.
struct Reentrant {
    /// The context to invoke into (installed after creation).
    slot: Mutex<Option<&'static cme_api::Context<'static>>>,
    /// The error kind name observed by the provider, for assertions.
    observed: Mutex<Option<String>>,
    /// Whether the nested call unexpectedly succeeded.
    succeeded: Mutex<bool>,
}

impl CapabilityProvider for Reentrant {
    fn call(&self, _member: &str, _args: &[Value]) -> Result<Value, String> {
        let context = self
            .slot
            .lock()
            .unwrap()
            .expect("the test must install the context first");
        match context.invoke("helper", &[]) {
            Ok(_) => {
                *self.succeeded.lock().unwrap() = true;
                Err("reentrancy must have been rejected".to_string())
            }
            Err(error) => {
                *self.observed.lock().unwrap() = Some(format!("{:?}", error.kind));
                // The original invocation continues normally.
                Ok(Value::Int(5))
            }
        }
    }
}

const TRACER_SCHEMA: &str = "
schema tracer v1.0.0

capability probe {
    since 1.0.0 int Ping()
}
";

/// Leaks program and context so the provider can hold `&'static` handles:
/// the provider outlives the test body through the engine's Arc.
fn leaked_context(
    engine: &Engine,
    source: &str,
) -> (
    &'static cme_api::Context<'static>,
    &'static cme_api::CompiledProgram,
) {
    let program: &'static cme_api::CompiledProgram =
        Box::leak(Box::new(engine.load_source(source).expect("loads")));
    let context = engine.create_context(program, ExecutionLimits::default());
    let context: &'static cme_api::Context<'static> = Box::leak(Box::new(context));
    (context, program)
}

#[test]
fn a_reentrant_provider_call_is_rejected_and_the_invocation_survives() {
    let mut engine = Engine::new();
    engine
        .load_schema_text(TRACER_SCHEMA, "tracer.cm")
        .expect("schema");
    let provider = Arc::new(Reentrant {
        slot: Mutex::new(None),
        observed: Mutex::new(None),
        succeeded: Mutex::new(false),
    });
    engine
        .register_capability(
            "tracer.probe",
            provider.clone() as Arc<dyn CapabilityProvider>,
        )
        .expect("registers");

    let (context, _program) = leaked_context(
        &engine,
        "import tracer.probe\n\nint helper() {\n    return 42\n}\n\nint entry() {\n    return tracer.probe.Ping()\n}\n",
    );
    *provider.slot.lock().unwrap() = Some(context);

    // The OUTER invocation succeeds: the rejection hits only the nested call.
    let result = context.invoke("entry", &[]).expect("invocation survives");
    assert_eq!(result, Value::Int(5));

    // The nested call failed with the §5.7 kind and never ran `helper`.
    let observed = provider.observed.lock().unwrap().take().expect("observed");
    assert_eq!(observed, "Reentrant");
    assert!(!*provider.succeeded.lock().unwrap());

    // After the invocation ends, the SAME context invokes normally again —
    // the guard is per-invocation, not a permanent lockout.
    let again = context.invoke("helper", &[]).expect("guard released");
    assert_eq!(again, Value::Int(42));
}

#[test]
fn a_reentrant_call_through_a_cloned_context_is_rejected_too() {
    // A clone IS the same logical context (shared identity), so a provider
    // capturing the clone exercises the same §5.7 prohibition.
    let mut engine = Engine::new();
    engine
        .load_schema_text(TRACER_SCHEMA, "tracer.cm")
        .expect("schema");
    let provider = Arc::new(Reentrant {
        slot: Mutex::new(None),
        observed: Mutex::new(None),
        succeeded: Mutex::new(false),
    });
    engine
        .register_capability(
            "tracer.probe",
            provider.clone() as Arc<dyn CapabilityProvider>,
        )
        .expect("registers");

    let (context, _program) = leaked_context(
        &engine,
        "import tracer.probe\n\nint helper() {\n    return 1\n}\n\nint entry() {\n    return tracer.probe.Ping()\n}\n",
    );
    // The clone is already `Context<'static>` (it borrows the leaked
    // program); leaking the box gives the provider its `&'static` handle.
    let clone = context.clone();
    let clone_ref: &'static cme_api::Context<'static> = Box::leak(Box::new(clone));
    *provider.slot.lock().unwrap() = Some(clone_ref);

    let result = context.invoke("entry", &[]).expect("invocation survives");
    assert_eq!(result, Value::Int(5));
    assert_eq!(
        provider.observed.lock().unwrap().take().expect("observed"),
        "Reentrant"
    );
}

#[test]
fn a_provider_may_invoke_a_different_context() {
    // §5.7 forbids re-entry into the SAME context only. Independent
    // contexts over independent programs stay composable.
    struct Cross {
        other: Mutex<Option<&'static cme_api::Context<'static>>>,
        failed: Mutex<Option<String>>,
    }

    impl CapabilityProvider for Cross {
        fn call(&self, _member: &str, _args: &[Value]) -> Result<Value, String> {
            let other = self.other.lock().unwrap().expect("installed");
            match other.invoke("helper", &[]) {
                Ok(Value::Int(42)) => Ok(Value::Int(7)),
                Ok(other) => Err(format!("unexpected helper result: {other:?}")),
                Err(error) => {
                    *self.failed.lock().unwrap() = Some(error.message);
                    Err("the cross-context invoke must succeed".to_string())
                }
            }
        }
    }

    let mut engine = Engine::new();
    engine
        .load_schema_text(TRACER_SCHEMA, "tracer.cm")
        .expect("schema");
    let provider = Arc::new(Cross {
        other: Mutex::new(None),
        failed: Mutex::new(None),
    });
    engine
        .register_capability(
            "tracer.probe",
            provider.clone() as Arc<dyn CapabilityProvider>,
        )
        .expect("registers");

    let (context, _program) = leaked_context(
        &engine,
        "import tracer.probe\n\nint entry() {\n    return tracer.probe.Ping()\n}\n",
    );
    let (other, _other_program) = leaked_context(&engine, "int helper() {\n    return 42\n}\n");
    *provider.other.lock().unwrap() = Some(other);

    let result = context.invoke("entry", &[]).expect("cross-context allowed");
    assert_eq!(result, Value::Int(7));
    assert!(
        provider.failed.lock().unwrap().is_none(),
        "the nested invoke must not have failed"
    );
}
