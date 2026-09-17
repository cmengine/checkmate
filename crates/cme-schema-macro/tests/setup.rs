//! The `cme_schema_setup!` quick-start flow, end to end, through the
//! DEFAULT api-crate path (`::cme::api`, the facade re-export) — the
//! shape a newcomer's host takes. The invocation asks for everything at
//! once: two schema namespaces, §5.5 limits, two proxies (one qualified
//! and renamed, one unqualified with the derived field name), and one
//! capability provider; the mod program implements both interfaces,
//! calls the capability, and `main` returns a value the context can
//! invoke directly.

// The SCHEMAS_HASH recipe's doc attribute sits on a macro invocation,
// which rustdoc does not render — the note can only be silenced from an
// enclosing scope, not by a sibling attribute.
#![allow(unused_doc_comments)]

use std::sync::atomic::{AtomicI64, Ordering};

// The macro under test: reads the fixture schemas relative to this
// crate's manifest dir at expansion time and emits the bindings modules
// (`demo`, `extra`) plus the host glue. The `#[doc]` attribute above the
// invocation mirrors the documented SCHEMAS_HASH recipe — an attribute
// changing the invocation's tokens must not change the expansion.
#[doc = "cme_schema_setup! quick-start test"]
cme_schema_macro::cme_schema_setup! {
    schema = "tests/fixtures/setup_demo.cm",
    schema = "tests/fixtures/setup_extra.cm",
    program = mod "tests/fixtures/demo_mod",
    limits = { fuel: 100_000, max_call_depth: 64 },
    proxy = demo.DemoReporterProxy as reporter,
    proxy = ExtraPingProxy,
    provider = speaker => Speaker,
}

/// Shout count of the provider `Host::new` constructs from the captured
/// `provider = speaker => Speaker` expression.
static SHOUTS: AtomicI64 = AtomicI64::new(0);

/// The host side of capability `demo.speaker` (§9.1): implementing the
/// GENERATED trait is the compile-time verification.
struct Speaker;

impl demo::DemoSpeakerCapability for Speaker {
    fn shout(&self, message: String) {
        assert_eq!(message, "hello from the mod");
        SHOUTS.fetch_add(1, Ordering::SeqCst);
    }

    fn compose(&self, name: String) -> demo::Greeting {
        demo::Greeting {
            text: format!("hello, {name}"),
            loudness: 3,
        }
    }
}

#[test]
fn host_new_registers_schemas_providers_and_loads_the_program() {
    let host = Host::new().expect("host setup succeeds");

    // The §5.5 limits from the invocation reached the stored defaults:
    assert_eq!(host.limits().fuel, Some(100_000));
    assert_eq!(host.limits().deadline_ms, None);
    assert_eq!(host.limits().max_call_depth, 64);

    // Both namespaces registered, in declaration order:
    assert_eq!(host.engine().schema_namespaces(), vec!["demo", "extra"]);
    // The provider registered under the §9.1 capability path:
    assert_eq!(host.engine().capability_paths(), vec!["demo.speaker"]);

    // The mod program loaded (§10): entry point present, both impl
    // targets linked.
    assert!(host.program().entry_points().contains(&"main".to_string()));
    let targets = host.program().interface_targets();
    assert!(targets.contains(&"demo.reporter".to_string()));
    assert!(targets.contains(&"extra.ping".to_string()));
}

#[test]
fn run_builds_the_context_and_every_proxy_in_one_pass() {
    let host = Host::new().expect("host setup succeeds");
    let shouts_before = SHOUTS.load(Ordering::SeqCst);

    let result = host.run(|session| {
        // The qualified+aliased proxy (`as reporter`):
        assert_eq!(
            session.reporter.report(21),
            Ok(42),
            "the demo.reporter proxy calls INTO the script with typed args"
        );

        // The unqualified proxy under its derived field name
        // (ExtraPingProxy → extra_ping):
        assert_eq!(session.extra_ping.ping(), Ok(9));

        // Session derefs to the context: plain invocations work without
        // naming the field.
        let main = session.invoke("main", &[]).expect("main runs");
        assert_eq!(main, cme::Value::Int(7));

        // The context reports the configured limits:
        assert_eq!(session.context.limits().fuel, Some(100_000));

        "session result"
    });
    assert_eq!(result, "session result");

    // The capability dispatch crossed to the host service (the mod's
    // main called demo.speaker.Shout). Tests share the static and run in
    // parallel, so assert on the delta, not the total.
    let shouts_after = SHOUTS.load(Ordering::SeqCst);
    assert!(
        shouts_after > shouts_before,
        "the Shout dispatch never reached the provider (before: {shouts_before}, after: {shouts_after})"
    );
}

#[test]
fn context_method_matches_the_manual_flow() {
    // The advanced-user escape hatch the macro keeps open: work with a
    // bare context exactly as the embedding guide describes.
    let host = Host::new().expect("host setup succeeds");
    let context = host.context();
    let main = context.invoke("main", &[]).expect("main runs");
    assert_eq!(main, cme::Value::Int(7));
}
