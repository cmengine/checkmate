//! §10 multi-file mod loading through the host API: discovery, manifests,
//! cross-file linking, impl unions, diagnostics re-anchoring, and the
//! failure shapes for broken trees.

use cme_api::{Engine, ErrorKind, ExecutionLimits, ProgramKind, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A unique throwaway mod directory per test, torn down on drop.
struct TempMod {
    root: PathBuf,
    counter: &'static AtomicUsize,
}

impl TempMod {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("cme_api_mod_{name}_{id}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        TempMod {
            root,
            counter: &COUNTER,
        }
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn path(&self) -> &std::path::Path {
        &self.root
    }
}

impl Drop for TempMod {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = self.counter;
    }
}

const MANIFEST: &str = "name = \"api_fixture\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.2.0\"\n\n[schemas]\nengine = \"1.0.0\"\n";

#[test]
fn a_well_formed_mod_loads_links_and_runs() {
    let temp = TempMod::new("happy");
    temp.write("mod.toml", MANIFEST);
    temp.write(
        "src/main.cm",
        "import self.helpers.math\nint main() {\nreturn add(2, 3)\n}\n",
    );
    temp.write(
        "src/helpers/math.cm",
        "int add(int a, int b) {\nreturn a + b\n}\n",
    );

    let program = Engine::new().load_mod(temp.path()).expect("mod compiles");
    assert!(program.is_mod());
    assert!(matches!(program.kind(), ProgramKind::Mod { .. }));
    // Linked modules share one namespace: every top-level function of
    // every module is an entry point of the assembled program.
    assert_eq!(program.entry_points(), ["add", "main"]);
    let manifest = program.manifest().expect("manifest surfaced");
    assert_eq!(manifest.name, "api_fixture");
    assert_eq!(manifest.version, "1.0.0");
    assert_eq!(
        manifest.schemas,
        vec![("engine".to_string(), "1.0.0".to_string())]
    );
    assert_eq!(program.modules().expect("modules").len(), 2);

    let ctx = Engine::new().create_context(&program, ExecutionLimits::default());
    assert_eq!(ctx.invoke("main", &[]), Ok(Value::Int(5)));
    assert_eq!(
        ctx.invoke("add", &[Value::Int(1), Value::Int(2)]),
        Ok(Value::Int(3))
    );
}

#[test]
fn impl_blocks_union_across_files_and_invoke_by_member() {
    // §10.4: two files implement one interface; the host enters through
    // the impl registry exactly like the C API does.
    let temp = TempMod::new("union");
    temp.write("mod.toml", MANIFEST);
    temp.write(
        "src/main.cm",
        concat!(
            "import self.rules\n",
            "import self.heal\n",
            "int main() {\n",
            "int health = 100\n",
            "health = engine.gamemode.Damage(health, 30)\n",
            "return health\n",
            "}\n",
        ),
    );
    temp.write(
        "src/rules.cm",
        concat!(
            "impl engine.gamemode {\n",
            "int Damage(int health, int amount) {\nreturn health - amount\n}\n",
            "}\n",
        ),
    );
    temp.write(
        "src/heal.cm",
        concat!(
            "impl engine.gamemode {\n",
            "int Heal(int health, int amount) {\nreturn health + amount\n}\n",
            "}\n",
        ),
    );

    let program = Engine::new().load_mod(temp.path()).unwrap();
    assert_eq!(program.interface_targets(), ["engine.gamemode"]);
    let ctx = Engine::new().create_context(&program, ExecutionLimits::default());
    assert_eq!(ctx.invoke("main", &[]), Ok(Value::Int(70)));
    assert_eq!(
        ctx.invoke_member("engine.gamemode", "Heal", &[Value::Int(70), Value::Int(10)]),
        Ok(Value::Int(80))
    );
    // Qualified calls from script side enter the same registry.
    assert_eq!(
        ctx.invoke_member(
            "engine.gamemode",
            "Damage",
            &[Value::Int(50), Value::Int(20)]
        ),
        Ok(Value::Int(30))
    );
}

#[test]
fn a_missing_mod_toml_fails_with_a_clear_issue() {
    let temp = TempMod::new("nomanifest");
    temp.write("src/main.cm", "int main() {\nreturn 1\n}\n");
    let error = Engine::new()
        .load_mod(temp.path())
        .expect_err("no manifest");
    assert!(error.message().contains("no mod.toml"), "{}", error);
}

#[test]
fn a_missing_directory_fails_cleanly() {
    let error = Engine::new()
        .load_mod("/definitely/not/a/mod/dir")
        .expect_err("missing dir");
    assert!(!error.messages().is_empty());
}

#[test]
fn a_broken_manifest_fails_with_line_information() {
    let temp = TempMod::new("badmanifest");
    temp.write("mod.toml", "name = \"x\"\nversion = \"not-a-version\"\n");
    temp.write("src/main.cm", "int main() {\nreturn 1\n}\n");
    let error = Engine::new()
        .load_mod(temp.path())
        .expect_err("bad version");
    let message = error.message();
    assert!(message.contains("mod.toml"), "{message}");
    assert!(message.contains('2'), "{message}");
}

#[test]
fn module_syntax_errors_render_against_the_owning_module() {
    let temp = TempMod::new("badsyntax");
    temp.write("mod.toml", MANIFEST);
    temp.write("src/main.cm", "int main() {\nreturn 1\n}\n");
    temp.write("src/broken.cm", "int oops() {\nreturn 1 +\n}\n");
    let error = Engine::new()
        .load_mod(temp.path())
        .expect_err("broken module");
    let message = error.message();
    assert!(
        message.contains("src/broken.cm:3:"),
        "must name the module and line: {message}"
    );
    assert!(
        !message.contains("assembled"),
        "no virtual-text leaks: {message}"
    );
}

#[test]
fn module_type_errors_render_against_the_owning_module() {
    let temp = TempMod::new("badtype");
    temp.write("mod.toml", MANIFEST);
    temp.write("src/main.cm", "int main() {\nreturn 1\n}\n");
    temp.write("src/broken.cm", "float oops() {\nreturn \"nope\"\n}\n");
    let error = Engine::new().load_mod(temp.path()).expect_err("type error");
    let message = error.message();
    assert!(message.contains("src/broken.cm:2:"), "{message}");
}

#[test]
fn cross_file_duplicate_names_collapse_into_shared_namespace_errors() {
    // §10.3: assembly concatenates modules into ONE program — duplicate
    // top-level names collide in the shared namespace, reported in the
    // colliding file.
    let temp = TempMod::new("dupes");
    temp.write("mod.toml", MANIFEST);
    temp.write(
        "src/main.cm",
        "int main() {\nreturn 1\n}\nint clash() {\nreturn 1\n}\n",
    );
    temp.write("src/other.cm", "int clash() {\nreturn 2\n}\n");
    let error = Engine::new()
        .load_mod(temp.path())
        .expect_err("duplicate top-level names collide");
    let message = error.message();
    assert!(message.contains("duplicate function `clash`"), "{message}");
    assert!(
        message.contains("src/other.cm"),
        "the collision names its file: {message}"
    );
}

#[test]
fn a_mod_runtime_error_reanchors_to_the_executing_module() {
    let temp = TempMod::new("runtimeerr");
    temp.write("mod.toml", MANIFEST);
    temp.write(
        "src/main.cm",
        "import self.boom\nint main() {\nreturn hit()\n}\n",
    );
    temp.write("src/boom.cm", "int hit() {\nreturn 1 / 0\n}\n");

    let program = Engine::new().load_mod(temp.path()).unwrap();
    let ctx = Engine::new().create_context(&program, ExecutionLimits::default());
    let error = ctx.invoke("main", &[]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Runtime);
    let expected_file = format!("{}/src/boom.cm", temp.path().display());
    assert_eq!(
        error.file.as_deref(),
        Some(expected_file.as_str()),
        "error names the executing module: {error:?}"
    );
    assert_eq!(error.line, 2);
    assert!(
        error
            .render()
            .starts_with(&format!("{}/src/boom.cm:2:", temp.path().display()))
    );
}

#[test]
fn a_mod_member_runtime_error_reanchors_the_same_way() {
    let temp = TempMod::new("membererr");
    temp.write("mod.toml", MANIFEST);
    temp.write("src/main.cm", "int main() {\nreturn 0\n}\n");
    temp.write(
        "src/iface.cm",
        "impl engine.gamemode {\nvoid OnTick() {\nint[] xs = [1]\nint v = xs[9]\n}\n}\n",
    );
    let program = Engine::new().load_mod(temp.path()).unwrap();
    let ctx = Engine::new().create_context(&program, ExecutionLimits::default());
    let error = ctx
        .invoke_member("engine.gamemode", "OnTick", &[])
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Runtime);
    let expected_file = format!("{}/src/iface.cm", temp.path().display());
    assert_eq!(error.file.as_deref(), Some(expected_file.as_str()));
}

#[test]
fn a_mod_directory_accepts_the_manifest_path_too() {
    let temp = TempMod::new("manifestpath");
    temp.write("mod.toml", MANIFEST);
    temp.write("src/main.cm", "int main() {\nreturn 3\n}\n");
    let manifest_path = temp.root.join("mod.toml");
    let program = Engine::new()
        .load_mod(&manifest_path)
        .expect("mod.toml path works");
    let ctx = Engine::new().create_context(&program, ExecutionLimits::default());
    assert_eq!(ctx.invoke("main", &[]), Ok(Value::Int(3)));
}

#[test]
fn megaprogram_modules_expand_before_linking() {
    let temp = TempMod::new("mega");
    temp.write("mod.toml", MANIFEST);
    temp.write(
        "src/main.cm",
        concat!("import self.data\n", "int main() {\nreturn total()\n}\n",),
    );
    temp.write(
        "src/data.cm",
        concat!(
            "grammar json {\nskip [ ' ', '\\t', '\\r', '\\n' ]\n",
            "rule value {\noneof {\nnull => \"null\"\nbool => oneof { t => \"true\", f => \"false\" }\n",
            "number => number\nstring => $str text\n",
            "array => ( \"[\" each sep \",\" { value } as items \"]\" )\n",
            "object => ( \"{\" each sep \",\" { member } as fields \"}\" )\n}\n}\n",
            "rule member {\n$str key \":\" value\n}\n",
            "rule number {\noptional { \"-\" }\n",
            "oneof { zero => \"0\", pos => ( [1-9] as first scan [0-9] as rest ) }\n",
            "optional { \".\" scan [0-9] as frac }\n}\n}\n",
            "magic value(json.value as v) {\n@toValue($v)\n}\n",
            "int total() {\nreturn 21\n}\n",
        ),
    );
    // The module contains only a grammar/magic DECLARATION; expansion must
    // strip it, leaving the function to link and run. A mod's top level
    // admits only declarations — no loose statements.
    let program = Engine::new()
        .load_mod(temp.path())
        .expect("mega module links");
    let ctx = Engine::new().create_context(&program, ExecutionLimits::default());
    assert_eq!(ctx.invoke("main", &[]), Ok(Value::Int(21)));
}

#[test]
fn an_empty_mod_source_tree_is_a_structure_error() {
    // A mod with no modules cannot link anything: discovery reports it.
    let temp = TempMod::new("empty");
    temp.write("mod.toml", MANIFEST);
    let error = Engine::new()
        .load_mod(temp.path())
        .expect_err("no .cm files under src/");
    assert!(error.message().contains("no `.cm` files"), "{}", error);
}

#[test]
fn nested_module_paths_map_to_self_imports() {
    let temp = TempMod::new("nested");
    temp.write("mod.toml", MANIFEST);
    temp.write(
        "src/main.cm",
        concat!(
            "import self.a.b.deep\n",
            "int main() {\nreturn value()\n}\n",
        ),
    );
    temp.write("src/a/b/deep.cm", "int value() {\nreturn 11\n}\n");
    let program = Engine::new().load_mod(temp.path()).unwrap();
    let ctx = Engine::new().create_context(&program, ExecutionLimits::default());
    assert_eq!(ctx.invoke("main", &[]), Ok(Value::Int(11)));
}

#[test]
fn an_unresolvable_self_import_names_the_missing_file() {
    let temp = TempMod::new("badimport");
    temp.write("mod.toml", MANIFEST);
    temp.write(
        "src/main.cm",
        "import self.ghost\nint main() {\nreturn 1\n}\n",
    );
    let error = Engine::new()
        .load_mod(temp.path())
        .expect_err("import miss");
    let message = error.message();
    assert!(message.contains("ghost"), "{message}");
    assert!(message.contains("src/main.cm"), "{message}");
}
