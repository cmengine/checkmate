//! The vendored `c_src/` copies must stay byte-identical to `apps/c_host/`.
//! Skips silently when the `apps/` tree is absent (registry consumers).

use std::path::PathBuf;

fn check(name: &str) {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest.join("../../apps/c_host").join(name);
    if !repo.is_file() {
        return;
    }
    let vendored = manifest.join("c_src").join(name);
    assert_eq!(
        std::fs::read(&vendored).expect("vendored copy exists"),
        std::fs::read(&repo).expect("repo copy exists"),
        "{name} drifted: refresh c_src/ from apps/c_host/"
    );
}

#[test]
fn vendored_main_c_matches_apps() {
    check("main.c");
}

#[test]
fn vendored_engine_cm_matches_apps() {
    check("engine.cm");
}
