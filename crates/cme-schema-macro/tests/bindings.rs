//! The compile-time-verified bindings flow (WHITEPAPER §9.6): the macro
//! reads the real schema file at expansion time, the host implements the
//! generated capability trait (signature errors here are HOST COMPILE
//! errors), and the whole flow runs end to end over the engine.

use std::sync::Arc;

// The macro under test: reads tests/fixtures/engine.cm relative to this
// crate's manifest dir at expansion time.
cme_schema_macro::cme_schema_bindings!("tests/fixtures/engine.cm");

use engine::{
    EngineAssetsCapability, EngineGamemodeProxy, EngineGraphicsCapability, GameConfig, GameState,
    Inventory, LoadError, TextureHandle, Vec2,
};

/// The host's graphics service: implements the GENERATED trait, so every
/// signature below is schema-verified at host compile time. Changing the
/// schema (a parameter type, a member name, a return type) breaks THIS
/// file's build — that is the §9.6 guarantee.
struct HostGraphics;

impl EngineGraphicsCapability for HostGraphics {
    fn load_texture(&self, path: String) -> TextureHandle {
        TextureHandle {
            id: path.len() as i64,
        }
    }

    fn draw_texture(&self, tex: TextureHandle, position: Vec2) {
        let _ = (tex.id, position.x, position.y);
    }

    fn draw_sprite(&self, tex: TextureHandle, position: Vec2, frame: i64) -> i64 {
        let _ = (tex.id, position.x, position.y);
        frame * 10
    }

    fn find_texture(&self, path: String) -> Option<TextureHandle> {
        if path.is_empty() {
            None
        } else {
            Some(TextureHandle { id: 7 })
        }
    }

    fn load_inventory(&self, owner: String) -> Result<Inventory, LoadError> {
        if owner.is_empty() {
            return Err(LoadError::NotFound);
        }
        Ok(Inventory {
            names: vec!["sword".to_string(), "shield".to_string()],
            counts: vec![("sword".to_string(), 1)],
            label: Some("starter".to_string()),
            attempt: Ok(3),
        })
    }

    fn texture_names(&self) -> Vec<String> {
        vec!["hero.png".to_string(), "bg.png".to_string()]
    }

    fn texture_scales(&self) -> Vec<(String, f64)> {
        vec![("hero.png".to_string(), 2.0)]
    }
}

/// The host side of capability `engine.assets` (§9.1): the schema's
/// `requires loader` edge is script-side; the host provides the assets
/// functions themselves.
struct HostAssets;

impl EngineAssetsCapability for HostAssets {
    fn load_bundled(&self, name: String) -> TextureHandle {
        TextureHandle {
            id: name.len() as i64,
        }
    }
}

#[test]
fn generated_types_round_trip_through_values() {
    let tex = TextureHandle { id: 42 };
    let value = tex.to_value();
    assert_eq!(
        value,
        cme_api::Value::Struct {
            name: "TextureHandle".to_string(),
            fields: vec![("id".to_string(), cme_api::Value::Int(42))],
        }
    );
    assert_eq!(TextureHandle::from_value(&value), Ok(tex));

    let error = LoadError::Corrupt {
        reason: "truncated".to_string(),
    };
    let value = error.to_value();
    assert_eq!(
        value,
        cme_api::Value::Enum {
            name: "LoadError".to_string(),
            variant: "Corrupt".to_string(),
            payload: vec![cme_api::Value::Str("truncated".to_string())],
        }
    );
    assert_eq!(LoadError::from_value(&value), Ok(error));

    // Containers: arrays, maps, options, results (§2.4, §2.8, §11).
    let inventory = Inventory {
        names: vec!["a".to_string(), "b".to_string()],
        counts: vec![("gold".to_string(), 5)],
        label: Some("hero".to_string()),
        attempt: Err(LoadError::NotFound),
    };
    let value = inventory.to_value();
    assert_eq!(Inventory::from_value(&value), Ok(inventory));
}

#[test]
fn the_descriptor_registers_and_exposes_metadata() {
    let mut engine = cme_api::Engine::new();
    engine
        .register_schema(engine::schema())
        .expect("the generated descriptor is a valid schema");
    assert_eq!(engine.schema_namespaces(), ["engine"]);
    assert_eq!(engine::NAMESPACE, "engine");
    assert_eq!(engine::SCHEMA_VERSION.major, 1);
    assert_eq!(engine::SCHEMA_VERSION.minor, 4);
}

#[test]
fn the_full_host_flow_loads_invokes_and_dispatches() {
    let mut engine = cme_api::Engine::new();

    // The host registers bindings generated from the SAME file the script
    // toolchain checks against — one source of truth, no re-parsing.
    engine.register_schema(engine::schema()).expect("registers");
    engine::register_engine_graphics(&mut engine, Arc::new(HostGraphics))
        .expect("valid capability path");
    engine::register_engine_assets(&mut engine, Arc::new(HostAssets))
        .expect("valid capability path");

    let program = engine
        .load_source(
            "import engine.graphics\n\
             int main() {\n\
             \x20TextureHandle tex = engine.graphics.LoadTexture(\"hero.png\")\n\
             \x20engine.graphics.DrawTexture(tex, Vec2(x: 1.0, y: 2.0))\n\
             \x20return tex.id\n\
             }\n",
        )
        .expect("loads against the schema");
    let context = engine.create_context(&program, cme_api::ExecutionLimits::default());
    assert_eq!(context.invoke("main", &[]), Ok(cme_api::Value::Int(8)));

    // The every-shape capability member: option, result, arrays, maps.
    let program = engine
        .load_source(
            "import engine.graphics\n\
             str main() {\n\
             \x20option<TextureHandle> found = engine.graphics.FindTexture(\"hero.png\")\n\
             \x20result<Inventory, LoadError> inv = engine.graphics.LoadInventory(\"hero\")\n\
             \x20str[] names = engine.graphics.TextureNames()\n\
             \x20map<str, float> scales = engine.graphics.TextureScales()\n\
             \x20return names[0] + \":\" + names.length\n\
             }\n",
        )
        .expect("every member shape type-checks");
    let context = engine.create_context(&program, cme_api::ExecutionLimits::default());
    assert_eq!(
        context.invoke("main", &[]),
        Ok(cme_api::Value::Str("hero.png:2".to_string()))
    );
}

#[test]
fn interface_proxies_call_into_the_script_with_types() {
    let mut engine = cme_api::Engine::new();
    engine.register_schema(engine::schema()).expect("registers");

    // The SCRIPT implements engine.gamemode — and its `requires core`
    // prerequisite (§9.4 rule 1), or the load would fail.
    let program = engine
        .load_source(
            "impl engine.core {\n\
             \x20void Tick() {\n\
             \x20}\n\
             }\n\
             impl engine.gamemode {\n\
             \x20GameState InitGame(GameConfig config) {\n\
             \x20\x20return GameState(score: config.score, active: config.active)\n\
             \x20}\n\
             \x20void OnTick(GameState state, float deltaTime) {\n\
             \x20}\n\
             \x20result<int, LoadError> DamageRoll(int seed) {\n\
             \x20\x20if (seed > 10) {\n\
             \x20\x20\x20return Err(LoadError.Corrupt(\"bad seed\"))\n\
             \x20\x20}\n\
             \x20\x20return Ok(seed * 2)\n\
             \x20}\n\
             }\n",
        )
        .expect("the program implements the interfaces");

    let context = engine.create_context(&program, cme_api::ExecutionLimits::default());
    // The direct constructor and the §13.1 generic one agree: both build
    // the proxy over the same context.
    let _ = EngineGamemodeProxy::new(&context).expect("the program implements gamemode");
    let proxy: EngineGamemodeProxy = context
        .get_interface()
        .expect("get_interface routes to InterfaceProxy::from_context");

    // Typed host → script call (§9.6 proxy shape).
    let state = proxy
        .init_game(GameConfig {
            score: 30,
            active: true,
        })
        .expect("the call succeeds");
    assert_eq!(
        state,
        GameState {
            score: 30,
            active: true,
        }
    );

    // A schema `result<int, LoadError>` return: Ok flows through.
    let roll = proxy.damage_roll(4).expect("the call succeeds");
    assert_eq!(roll, Ok(8));

    // …and the script's Err payload surfaces.
    let roll = proxy.damage_roll(50).expect("the invocation itself ran");
    assert!(matches!(roll, Err(LoadError::Corrupt { .. })));

    // A proxy for an interface the program does not implement fails at
    // construction — through BOTH constructors, as an UnknownEntry error
    // (the host asked for an entry the program does not declare).
    let missing = engine
        .load_source("int main() {\nreturn 0\n}\n")
        .expect("loads");
    let context = engine.create_context(&missing, cme_api::ExecutionLimits::default());
    let error = EngineGamemodeProxy::new(&context).unwrap_err();
    assert_eq!(error.kind, cme_api::ErrorKind::UnknownEntry);
    let error = context.get_interface::<EngineGamemodeProxy>().unwrap_err();
    assert_eq!(error.kind, cme_api::ErrorKind::UnknownEntry);
}

#[test]
fn the_loader_capability_stays_wired_for_requires() {
    // engine.assets requires engine.loader (§9.4 rule 2); the host
    // provides both, and a script calling assets dispatches to HostLoader.
    let mut engine = cme_api::Engine::new();
    engine.register_schema(engine::schema()).expect("registers");
    engine::register_engine_graphics(&mut engine, Arc::new(HostGraphics))
        .expect("graphics provider");
    engine::register_engine_assets(&mut engine, Arc::new(HostAssets)).expect("assets provider");

    let program = engine
        .load_source(
            "impl engine.loader {\n\
             \x20bool IsAvailable(str name) {\n\
             \x20\x20return name != \"\"\n\
             \x20}\n\
             }\n\
             import engine.assets\n\
             int main() {\n\
             \x20TextureHandle tex = engine.assets.LoadBundled(\"title\")\n\
             \x20return tex.id\n\
             }\n",
        )
        .expect("the program implements the prerequisite interface");
    let context = engine.create_context(&program, cme_api::ExecutionLimits::default());
    assert_eq!(context.invoke("main", &[]), Ok(cme_api::Value::Int(5)));
}
