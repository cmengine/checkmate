//! The C API exercised from Rust threads: engines and programs are shared
//! freely, one context issues concurrent invocations, and every future /
//! value handle stays single-threaded (per the header's threading
//! contract).

use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cme_ffi::{
    CmError, CmValue, cm_context_destroy, cm_engine_create_context, cm_engine_destroy,
    cm_engine_load_source, cm_engine_new, cm_error_free, cm_future_destroy, cm_future_get_error,
    cm_future_poll, cm_future_take_value, cm_invoke, cm_program_destroy, cm_value_destroy,
    cm_value_int,
};

fn cstring(text: &str) -> CString {
    CString::new(text).unwrap()
}

/// A raw handle wrapper the test threads may carry (the header's threading
/// contract covers the pointee; raw pointers themselves are !Send).
struct SendPtr<T>(*mut T);
unsafe impl<T> Send for SendPtr<T> {}
impl<T> SendPtr<T> {
    fn get(&self) -> *mut T {
        self.0
    }
}

struct Fixture {
    engine: *mut cme_ffi::CmEngine,
    program: *mut cme_ffi::CmProgram,
    context: *mut cme_ffi::CmContext,
}

unsafe fn fixture(source: &str) -> Fixture {
    unsafe {
        let engine = cm_engine_new();
        let mut error: CmError = CmError::zeroed();
        let program = cm_engine_load_source(engine, cstring(source).as_ptr(), &mut error);
        assert!(
            !program.is_null(),
            "{}",
            CStr::from_ptr(error.message).to_str().unwrap()
        );
        cm_error_free(&mut error);
        let context = cm_engine_create_context(engine, program, ptr::null());
        assert!(!context.is_null());
        Fixture {
            engine,
            program,
            context,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            cm_context_destroy(self.context);
            cm_program_destroy(self.program);
            cm_engine_destroy(self.engine);
        }
    }
}

/// SAFETY: the fixture's raw handles are shared only through the contract
/// the header documents — the engine/program/context are safe for
/// concurrent invocation; futures and values never cross threads here.
unsafe impl Send for Fixture {}
unsafe impl Sync for Fixture {}

#[test]
fn one_context_many_threads_through_the_abi() {
    unsafe {
        let fixture = Arc::new(fixture("int add(int a, int b) {\nreturn a + b\n}\n"));
        let successes = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for thread in 0..6usize {
            let fixture = Arc::clone(&fixture);
            let successes = Arc::clone(&successes);
            handles.push(std::thread::spawn(move || {
                // the fixture handles are Send; call sites are unsafe blocks
                for i in 0..60usize {
                    let a = cm_value_int((thread * 1000 + i) as i64);
                    let b = cm_value_int((thread * 1000 + i) as i64);
                    let args: [*const CmValue; 2] = [a, b];
                    let future = cm_invoke(
                        fixture.context,
                        ptr::null(),
                        cstring("add").as_ptr(),
                        args.as_ptr(),
                        2,
                    );
                    cm_value_destroy(a);
                    cm_value_destroy(b);
                    assert_eq!(cm_future_poll(future, ptr::null_mut()), 1);
                    let value = cm_future_take_value(future);
                    let mut out: i64 = 0;
                    cme_ffi::cm_value_as_int(value, &mut out);
                    assert_eq!(out, ((thread * 1000 + i) * 2) as i64);
                    cm_value_destroy(value);
                    cm_future_destroy(future);
                    successes.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(successes.load(Ordering::SeqCst), 360);
    }
}

#[test]
fn concurrent_budget_exhaustions_are_independent() {
    // Six threads share a context whose every invocation runs on a tiny
    // fuel budget: each gets a fresh cell, so all 6 × 40 runs fail
    // identically and independently — no budget ever bleeds.
    unsafe {
        let engine = cm_engine_new();
        let mut error: CmError = CmError::zeroed();
        let program = cm_engine_load_source(
            engine,
            cstring("int spin(int n) {\nreturn spin(n + 1)\n}\n").as_ptr(),
            &mut error,
        );
        assert!(!program.is_null());
        cm_error_free(&mut error);

        let limits = cme_ffi::CmLimits {
            fuel: 32,
            deadline_ms: 0,
            max_call_depth: 0,
        };
        let context = cm_engine_create_context(engine, program, &limits);
        assert!(!context.is_null());
        let context = SendPtr(context);

        let mut handles = Vec::new();
        for _ in 0..6 {
            let context = SendPtr(context.get());
            handles.push(std::thread::spawn(move || {
                // the fixture handles are Send; call sites are unsafe blocks
                for _ in 0..40 {
                    let zero = cm_value_int(0);
                    let args: [*const CmValue; 1] = [zero];
                    let future = cm_invoke(
                        context.get(),
                        ptr::null(),
                        cstring("spin").as_ptr(),
                        args.as_ptr(),
                        1,
                    );
                    cm_value_destroy(zero);
                    assert_eq!(cm_future_poll(future, ptr::null_mut()), 2);
                    let mut report = cm_future_get_error(future);
                    assert_eq!(report.kind, 3); // CM_ERROR_LIMIT
                    let message = CStr::from_ptr(report.message).to_str().unwrap();
                    assert!(message.contains("fuel"), "{message}");
                    cm_error_free(&mut report);
                    cm_future_destroy(future);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        cm_context_destroy(context.get());
        cm_program_destroy(program);
        cm_engine_destroy(engine);
    }
}

#[test]
fn deadline_through_the_abi_stops_a_runaway_loop() {
    unsafe {
        let engine = cm_engine_new();
        let mut error: CmError = CmError::zeroed();
        let program = cm_engine_load_source(
            engine,
            cstring("int loop() {\nwhile (true) {\n}\nreturn 0\n}\n").as_ptr(),
            &mut error,
        );
        assert!(!program.is_null());
        cm_error_free(&mut error);

        let limits = cme_ffi::CmLimits {
            fuel: 0,
            deadline_ms: 200,
            max_call_depth: 0,
        };
        let context = cm_engine_create_context(engine, program, &limits);
        let future = cm_invoke(
            context,
            ptr::null(),
            cstring("loop").as_ptr(),
            ptr::null(),
            0,
        );
        assert_eq!(cm_future_poll(future, ptr::null_mut()), 2);
        let mut report = cm_future_get_error(future);
        assert_eq!(report.kind, 3); // CM_ERROR_LIMIT
        let message = CStr::from_ptr(report.message).to_str().unwrap();
        assert!(message.contains("deadline"), "{message}");
        cm_error_free(&mut report);
        cm_future_destroy(future);
        cm_context_destroy(context);
        cm_program_destroy(program);
        cm_engine_destroy(engine);
    }
}

#[test]
fn interface_member_invocation_through_the_abi() {
    // The §13.2 whitepaper shape from a Rust thread: cm_invoke with a
    // target routes into the impl registry.
    unsafe {
        let fixture = Arc::new(fixture(
            "impl engine.gamemode {\nint InitGame(int seed) {\nreturn seed * 2\n}\n}\n",
        ));
        let mut handles = Vec::new();
        for thread in 0..4usize {
            let fixture = Arc::clone(&fixture);
            handles.push(std::thread::spawn(move || {
                // the fixture handles are Send; call sites are unsafe blocks
                for i in 0..25usize {
                    let seed = cm_value_int((thread * 100 + i) as i64);
                    let args: [*const CmValue; 1] = [seed];
                    let future = cm_invoke(
                        fixture.context,
                        cstring("engine.gamemode").as_ptr(),
                        cstring("InitGame").as_ptr(),
                        args.as_ptr(),
                        1,
                    );
                    cm_value_destroy(seed);
                    assert_eq!(cm_future_poll(future, ptr::null_mut()), 1);
                    let value = cm_future_take_value(future);
                    let mut out: i64 = 0;
                    cme_ffi::cm_value_as_int(value, &mut out);
                    assert_eq!(out, ((thread * 100 + i) * 2) as i64);
                    cm_value_destroy(value);
                    cm_future_destroy(future);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
    }
}

#[test]
fn values_and_futures_are_send_but_confined_by_contract() {
    fn assert_send<T: Send>() {}
    assert_send::<cme_ffi::CmValue>();
    assert_send::<cme_ffi::CmFuture>();
    assert_send::<cme_ffi::CmContext>();
    assert_send::<cme_ffi::CmEngine>();
    assert_send::<cme_ffi::CmProgram>();
}

#[test]
fn host_app_main_symbol_reports_zero_via_pointer_call() {
    // The embedded C app entry is addressable through the ABI surface,
    // proving the build-script linkage from a second angle.
    unsafe extern "C" {
        fn cme_host_app_main() -> c_int;
    }
    let entry: unsafe extern "C" fn() -> c_int = cme_host_app_main;
    assert_eq!(unsafe { entry() }, 0);
}
