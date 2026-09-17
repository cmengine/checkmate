//! The `cme_schema_setup!` option surface: an explicit `crate = ::cme_api`
//! path (the shape hosts that consume the API crate directly take), the
//! `program = file "…"` source shape, and the failure paths — a session
//! whose proxy targets an interface the program never implemented fails
//! `try_run` with `UnknownEntry` and panics under `run`.

use cme_api::ErrorKind;

cme_schema_macro::cme_schema_setup! {
    schema = "tests/fixtures/setup_demo.cm",
    program = file "tests/fixtures/loose.cm",
    crate = ::cme_api,
    proxy = demo.DemoReporterProxy as reporter,
}

#[test]
fn file_programs_load_and_the_bare_context_still_works() {
    let host = Host::new().expect("host setup succeeds");

    // No limits key was given, so the API defaults apply:
    assert_eq!(host.limits().fuel, None);
    assert_eq!(host.limits().deadline_ms, None);

    // The loose program loaded under the registered schema (loose
    // sources inherit the whole grant) and invokes through the context
    // the setup macro hands out:
    let main = host.context().invoke("main", &[]).expect("main runs");
    assert_eq!(main, cme_api::Value::Int(5));
}

#[test]
fn try_run_reports_an_unimplemented_interface_as_unknown_entry() {
    let host = Host::new().expect("host setup succeeds");

    // loose.cm implements NO interface: the proxy cannot be constructed,
    // and try_run surfaces the exact §10.4 host-side guard error instead
    // of a panic.
    let error = host
        .try_run(|session| session.reporter.report(1))
        .expect_err("loose.cm never implemented demo.reporter");
    assert_eq!(error.kind, ErrorKind::UnknownEntry);
    assert!(
        error.message.contains("demo.reporter"),
        "the error names the missing interface: {}",
        error.message
    );
}

#[test]
#[should_panic(expected = "cannot serve this session")]
fn run_panics_when_the_session_cannot_be_built() {
    let host = Host::new().expect("host setup succeeds");
    let _ = host.run(|session| session.reporter.report(1));
}
