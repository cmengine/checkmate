//! The ABI-level test suite: handles, ownership, UTF-8 boundaries, misuse
//! responses, and the C host application itself. These tests call the
//! `cm_*` functions exactly as a C host would — through the C ABI.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::ptr;

use cme_ffi::{
    CmError, cm_context_destroy, cm_engine_create_context, cm_engine_destroy,
    cm_engine_load_source, cm_engine_new, cm_error_free, cm_future_destroy, cm_future_get_error,
    cm_future_poll, cm_future_take_value, cm_invoke, cm_program_destroy, cm_program_entry_count,
    cm_program_entry_name, cm_value_clone, cm_value_destroy, cm_value_int, cm_value_str,
    cm_version,
};

fn cstring(text: &str) -> CString {
    CString::new(text).unwrap()
}

#[test]
fn version_is_a_valid_c_string() {
    let version = unsafe { CStr::from_ptr(cm_version()) };
    assert_eq!(version.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn engine_and_program_lifecycle_round_trip() {
    unsafe {
        let engine = cm_engine_new();
        let source = cstring("int f() {\nreturn 1\n}\n");
        let mut error: CmError = CmError::zeroed();
        let program = cm_engine_load_source(engine, source.as_ptr(), &mut error);
        assert!(!program.is_null());
        assert_eq!(error.kind, 0); // CM_ERROR_NONE
        let message = CStr::from_ptr(error.message).to_str().unwrap();
        assert_eq!(message, "");
        cm_error_free(&mut error);

        assert_eq!(cm_program_entry_count(program), 1);
        let name = cm_program_entry_name(program, 0);
        assert_eq!(CStr::from_ptr(name).to_str().unwrap(), "f");

        cm_program_destroy(program);
        cm_engine_destroy(engine);
    }
}

#[test]
fn load_failure_fills_a_compile_report() {
    unsafe {
        let engine = cm_engine_new();
        let source = cstring("int f() {\nreturn 1 + \"x\"\n}\n");
        let mut error: CmError = CmError::zeroed();
        let program = cm_engine_load_source(engine, source.as_ptr(), &mut error);
        assert!(program.is_null());
        assert_eq!(error.kind, 1); // CM_ERROR_COMPILE
        let message = CStr::from_ptr(error.message).to_str().unwrap();
        assert!(message.contains("2:1"), "rendered: {message}");
        cm_error_free(&mut error);
        // Freeing twice is a no-op on zeroed fields.
        cm_error_free(&mut error);
        cm_engine_destroy(engine);
    }
}

#[test]
fn load_reports_misuse_without_touching_the_report() {
    unsafe {
        let engine = cm_engine_new();
        let mut error: CmError = CmError::zeroed();
        // NULL source → INVALID_ARG.
        let program = cm_engine_load_source(engine, ptr::null(), &mut error);
        assert!(program.is_null());
        assert_eq!(error.kind, 5); // CM_ERROR_INVALID_ARG
        cm_error_free(&mut error);
        // NULL engine is refused too.
        let source = cstring("int f() {\nreturn 1\n}\n");
        let program = cm_engine_load_source(ptr::null_mut(), source.as_ptr(), &mut error);
        assert!(program.is_null());
        assert_eq!(error.kind, 5);
        cm_error_free(&mut error);
        cm_engine_destroy(engine);
    }
}

#[test]
fn invoke_error_path_carries_kind_message_and_position() {
    unsafe {
        let engine = cm_engine_new();
        let source = cstring("int f() {\nreturn 1 / 0\n}\n");
        let mut error: CmError = CmError::zeroed();
        let program = cm_engine_load_source(engine, source.as_ptr(), &mut error);
        assert!(!program.is_null());
        cm_error_free(&mut error);
        let context = cm_engine_create_context(engine, program, ptr::null());
        assert!(!context.is_null());

        let future = cm_invoke(context, ptr::null(), cstring("f").as_ptr(), ptr::null(), 0);
        assert!(!future.is_null());
        assert_eq!(cm_future_poll(future, ptr::null_mut()), 2); // CM_ERROR
        let mut report = cm_future_get_error(future);
        assert_eq!(report.kind, 2); // CM_ERROR_RUNTIME
        let message = CStr::from_ptr(report.message).to_str().unwrap();
        assert!(message.contains("zero"), "{message}");
        assert_eq!(report.line, 2);
        let file = CStr::from_ptr(report.file).to_str().unwrap();
        assert_eq!(file, "", "runtime errors on loose sources carry no file");
        cm_error_free(&mut report);

        cm_future_destroy(future);
        cm_context_destroy(context);
        cm_program_destroy(program);
        cm_engine_destroy(engine);
    }
}

#[test]
fn unknown_entry_reports_without_position() {
    unsafe {
        let engine = cm_engine_new();
        let source = cstring("int f() {\nreturn 1\n}\n");
        let mut error: CmError = CmError::zeroed();
        let program = cm_engine_load_source(engine, source.as_ptr(), &mut error);
        cm_error_free(&mut error);
        let context = cm_engine_create_context(engine, program, ptr::null());

        let future = cm_invoke(
            context,
            ptr::null(),
            cstring("ghost").as_ptr(),
            ptr::null(),
            0,
        );
        assert_eq!(cm_future_poll(future, ptr::null_mut()), 2);
        let mut report = cm_future_get_error(future);
        assert_eq!(report.kind, 4); // CM_ERROR_UNKNOWN_ENTRY
        let message = CStr::from_ptr(report.message).to_str().unwrap();
        assert_eq!(message, "unknown function `ghost`");
        assert_eq!(report.line, 0);
        assert_eq!(report.column, 0);
        cm_error_free(&mut report);
        cm_future_destroy(future);

        cm_context_destroy(context);
        cm_program_destroy(program);
        cm_engine_destroy(engine);
    }
}

#[test]
fn invoke_misuse_returns_null_futures() {
    unsafe {
        let engine = cm_engine_new();
        let source = cstring("int add(int a, int b) {\nreturn a + b\n}\n");
        let mut error: CmError = CmError::zeroed();
        let program = cm_engine_load_source(engine, source.as_ptr(), &mut error);
        cm_error_free(&mut error);
        let context = cm_engine_create_context(engine, program, ptr::null());

        // NULL context.
        assert!(
            cm_invoke(
                ptr::null_mut(),
                ptr::null(),
                cstring("add").as_ptr(),
                ptr::null(),
                0
            )
            .is_null()
        );
        // NULL member.
        assert!(cm_invoke(context, ptr::null(), ptr::null(), ptr::null(), 0).is_null());
        // args NULL with argc > 0.
        assert!(
            cm_invoke(
                context,
                ptr::null(),
                cstring("add").as_ptr(),
                ptr::null(),
                2
            )
            .is_null()
        );
        // A NULL element inside args.
        let bad: [*const cme_ffi::CmValue; 2] = [ptr::null(), ptr::null()];
        assert!(
            cm_invoke(
                context,
                ptr::null(),
                cstring("add").as_ptr(),
                bad.as_ptr(),
                2
            )
            .is_null()
        );

        cm_context_destroy(context);
        cm_program_destroy(program);
        cm_engine_destroy(engine);
    }
}

#[test]
fn arguments_are_borrowed_by_invoke() {
    unsafe {
        let engine = cm_engine_new();
        let source = cstring("int add(int a, int b) {\nreturn a + b\n}\n");
        let mut error: CmError = CmError::zeroed();
        let program = cm_engine_load_source(engine, source.as_ptr(), &mut error);
        cm_error_free(&mut error);
        let context = cm_engine_create_context(engine, program, ptr::null());

        let a = cm_value_int(20);
        let b = cm_value_int(22);
        let args: [*const cme_ffi::CmValue; 2] = [a, b];
        let future = cm_invoke(
            context,
            ptr::null(),
            cstring("add").as_ptr(),
            args.as_ptr(),
            2,
        );
        // The caller still owns the argument handles and must free them —
        // and the invocation already consumed their VALUES.
        cm_value_destroy(a);
        cm_value_destroy(b);

        assert_eq!(cm_future_poll(future, ptr::null_mut()), 1); // CM_READY
        let value = cm_future_take_value(future);
        assert!(!value.is_null());
        cm_value_destroy(value);
        cm_future_destroy(future);

        cm_context_destroy(context);
        cm_program_destroy(program);
        cm_engine_destroy(engine);
    }
}

#[test]
fn unicode_and_nul_text_round_trip_losslessly() {
    unsafe {
        let emoji = cm_value_str(cstring("héllo 🌍").as_ptr());
        assert!(!emoji.is_null());
        let mut text: *mut c_char = ptr::null_mut();
        let mut length: usize = 0;
        assert_eq!(cme_ffi::cm_value_as_str(emoji, &mut text, &mut length), 0);
        assert_eq!(CStr::from_ptr(text).to_str().unwrap(), "héllo 🌍");
        // Byte length of the multibyte string: é and 🌍 are multibyte.
        assert_eq!(length, "héllo 🌍".len());
        cm_string_free_wrapper(text);
        cm_value_destroy(emoji);

        // str_len accepts embedded NULs; the C projection renders U+FFFD.
        let bytes = b"a\0b";
        let nul = cme_ffi::cm_value_str_len(bytes.as_ptr() as *const c_char, 3);
        assert!(!nul.is_null());
        assert_eq!(cme_ffi::cm_value_len(nul), 3);
        let shown: *mut c_char = cme_ffi::cm_value_to_string(nul);
        assert!(!shown.is_null());
        let rendered = CStr::from_ptr(shown).to_bytes();
        assert_eq!(rendered, "a\u{FFFD}b".as_bytes());
        cm_string_free_wrapper(shown);
        cm_value_destroy(nul);

        // Invalid UTF-8 is refused at construction.
        let bad: [u8; 2] = [0xff, 0xfe];
        assert!(cme_ffi::cm_value_str_len(bad.as_ptr() as *const c_char, 2).is_null());
        assert!(cm_value_str(ptr::null()).is_null());
    }
}

fn cm_string_free_wrapper(text: *mut c_char) {
    unsafe { cme_ffi::cm_string_free(text) }
}

#[test]
fn clone_is_deep_and_independent() {
    unsafe {
        let strukt = cme_ffi::cm_value_struct(cstring("bag").as_ptr());
        let gold = cm_value_int(100);
        assert_eq!(
            cme_ffi::cm_struct_set_field(strukt, cstring("gold").as_ptr(), gold),
            0
        );
        let copy = cm_value_clone(strukt);
        cm_value_destroy(strukt);
        // The clone survives and carries the field.
        assert_eq!(cme_ffi::cm_value_len(copy), 1);
        let field = cme_ffi::cm_value_struct_field(copy, cstring("gold").as_ptr());
        let mut out: i64 = 0;
        assert_eq!(cme_ffi::cm_value_as_int(field, &mut out), 0);
        assert_eq!(out, 100);
        cm_value_destroy(copy);
    }
}

#[test]
fn deep_handles_are_real_addresses_not_zst_dangling() {
    // A regression guard: engine handles used to be zero-sized, so
    // cm_engine_new handed back a dangling aligned pointer (0x1). C hosts
    // must receive distinct, freeable addresses.
    unsafe {
        let a = cm_engine_new();
        let b = cm_engine_new();
        assert_ne!(a, b);
        let a_value = a as usize;
        assert!(a_value > 0xffff, "engine handle must be a real address");
        cm_engine_destroy(a);
        cm_engine_destroy(b);
    }
}

#[test]
fn the_c_host_application_passes_all_its_checks() {
    // The real C consumer, compiled by the build script into this test
    // binary, runs its full self-check suite in-process.
    unsafe extern "C" {
        fn cme_host_app_main() -> c_int;
    }
    unsafe {
        assert_eq!(
            cme_host_app_main(),
            0,
            "the C host app must pass every check"
        );
    }
}

#[test]
fn program_name_borrows_survive_across_queries() {
    unsafe {
        let engine = cm_engine_new();
        let source = cstring("int one() {\nreturn 1\n}\nint two() {\nreturn 2\n}\n");
        let mut error: CmError = CmError::zeroed();
        let program = cm_engine_load_source(engine, source.as_ptr(), &mut error);
        cm_error_free(&mut error);
        let first = cm_program_entry_name(program, 0);
        let second = cm_program_entry_name(program, 1);
        assert_eq!(CStr::from_ptr(first).to_str().unwrap(), "one");
        assert_eq!(CStr::from_ptr(second).to_str().unwrap(), "two");
        // Both pointers stay valid until the program is destroyed.
        assert_eq!(CStr::from_ptr(first).to_str().unwrap(), "one");
        cm_program_destroy(program);
        cm_engine_destroy(engine);
    }
}
