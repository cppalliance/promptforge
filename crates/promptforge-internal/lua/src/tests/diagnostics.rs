//! Compiled programs and their failures: bytecode round trips, chunk-line
//! mapping to prompt lines, compile and runtime error reporting, and reads
//! that fail closed on a poisoned lock.

use super::*;
use crate::program::map_chunk_line_to_absolute;
use crate::vm::LocalTools;

#[test]
fn lua_program_retains_source_and_round_trips_bytecode() {
    let source = "return greeting .. ' world'";
    let program = LuaProgram::compile(
        source,
        "section Gather prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        &null_emitter(),
        "Gather",
    )
    .expect("valid Lua must compile");
    assert_eq!(program.source(), source);

    for greeting in ["hello", "goodbye"] {
        let lua = Lua::new();
        lua.globals()
            .set("greeting", greeting)
            .expect("the test global must install");
        let function = program.load(&lua).expect("bytecode must load");
        let returned: String = function.call(()).expect("bytecode must execute");
        assert_eq!(returned, format!("{greeting} world"));
    }
}

#[test]
fn runtime_assert_failure_reports_chunk_name_and_line() {
    let location = "section `Web Search` epilog";
    let program = LuaProgram::compile(
        "local x = 1\nassert(false)\nreturn x",
        location,
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        &null_emitter(),
        "Web Search",
    )
    .expect("valid Lua must compile");
    let lua = Lua::new();
    let function = program.load(&lua).expect("bytecode must load");
    let error = function
        .call::<()>(())
        .expect_err("assert(false) must fail at runtime");
    let message = error.to_string();
    assert!(
        message.contains(location),
        "runtime error must name the chunk: {message}"
    );
    assert!(
        message.contains(":2:") || message.contains(":2\n"),
        "runtime error must include the failing line number: {message}"
    );
    assert!(
        !message.contains("?:"),
        "stripped debug info must not leave '?:' in the traceback: {message}"
    );
}

#[test]
fn current_sys_returns_fallback_when_unset_and_errors_on_poison() {
    // An unset live slot is a legitimate state and yields the
    // fallback; a poisoned lock is a real failure and must NOT masquerade as
    // the fallback.
    let vm = SectionVm::new(&test_nonce(), &null_emitter(), "Section").expect("VM must build");
    let fallback = json!({ "id": 7 });
    let got = vm
        .current_sys(&fallback)
        .expect("an unset live slot yields the fallback");
    assert_eq!(got, fallback, "unset must return the fallback verbatim");

    // Poison the live mutex via a panicking guard, then a snapshot must be a
    // concrete error rather than a silent fallback.
    let handle = vm.sys_live_handle();
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = handle.lock().expect("first lock is not poisoned");
        panic!("poison the sys_live mutex");
    }));
    assert!(
        poisoned.is_err(),
        "the panic must unwind and poison the lock"
    );
    let error = vm
        .current_sys(&fallback)
        .expect_err("a poisoned live slot must surface a concrete error");
    assert!(
        error.to_string().contains("poisoned"),
        "the error must name the poison: {error}"
    );
}

#[test]
fn local_tools_schema_and_membership_reads_fail_closed_on_poison() {
    let local = LocalTools::default();
    let schema = promptforge_model_client::detail::tool_schema_new(
        "grab".to_owned(),
        "Grab a value".to_owned(),
        serde_json::json!({ "type": "object", "properties": {} }),
    )
    .expect("the schema builds");
    local
        .register("grab".to_owned(), schema)
        .expect("registration succeeds before the poison");
    let handle = local.entries_handle();
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = handle.lock().expect("first lock is not poisoned");
        panic!("poison the local tools registry");
    }));
    assert!(poisoned.is_err(), "the panic must poison the registry");

    let schemas = local
        .schemas()
        .expect_err("schema reads must fail on a poisoned registry");
    let contains = local
        .contains("anything")
        .expect_err("membership reads must fail on a poisoned registry");
    for error in [schemas, contains] {
        assert!(
            error
                .to_string()
                .contains("local tools registry was poisoned"),
            "the concrete poison error must surface: {error}"
        );
    }
}

#[test]
fn map_chunk_line_to_absolute_rewrites_line_numbers() {
    let location = "section `Web Search` epilog";
    let msg = r#"[string "section `Web Search` epilog"]:2: assertion failed!"#;
    let result =
        map_chunk_line_to_absolute(msg, NonZeroU32::new(50).expect("50 is non-zero"), location);
    assert_eq!(
        result,
        r#"section `Web Search` epilog:51: [string "section `Web Search` epilog"]:51: assertion failed!"#
    );
}

#[test]
fn map_chunk_line_to_absolute_only_rewrites_matching_chunk() {
    let msg = r#"[string "section `Web Search` epilog"]:51: assertion failed!
stack traceback:
        [string "section `Main` prologue"]:3: in main chunk"#;
    let result = map_chunk_line_to_absolute(
        msg,
        NonZeroU32::new(22).expect("22 is non-zero"),
        "section `Main` prologue",
    );
    assert!(
        result.contains("[string \"section `Web Search` epilog\"]:51:"),
        "child absolute line must stay intact: {result}"
    );
    assert!(
        result.contains("[string \"section `Main` prologue\"]:24:")
            || result.starts_with("section `Main` prologue:24:"),
        "parent chunk line must map with parent source_line: {result}"
    );
    assert!(
        !result.contains("[string \"section `Main` prologue\"]:3:"),
        "parent chunk-relative line must be rewritten: {result}"
    );
}

#[test]
fn map_chunk_line_to_absolute_keeps_original_digits_on_overflow() {
    // source_line + chunk_line - 1 must not wrap; on overflow the original
    // chunk-relative digits are preserved rather than a wrong absolute line.
    let msg = r#"[string "x"]:5: boom"#;
    let result = map_chunk_line_to_absolute(msg, NonZeroU32::MAX, "x");
    assert!(
        result.contains(r#"[string "x"]:5:"#),
        "overflowing mapping must keep the original line 5: {result}"
    );
    assert!(
        !result.contains(":4294967300:"),
        "no wrapped absolute line may appear: {result}"
    );
}

#[test]
fn map_chunk_line_to_absolute_no_match_passthrough() {
    let msg = "some other error without chunk info";
    let result = map_chunk_line_to_absolute(
        msg,
        NonZeroU32::new(10).expect("10 is non-zero"),
        "section `Main` prologue",
    );
    assert_eq!(result, msg);
}

#[test]
fn runtime_error_maps_to_absolute_prompt_line() {
    let location = "section `Web Search` epilog";
    let source_line = NonZeroU32::new(50).expect("50 is non-zero");
    let program = LuaProgram::compile(
        "local x = 1\nassert(false)\nreturn x",
        location,
        source_line,
        &null_emitter(),
        "Web Search",
    )
    .expect("valid Lua must compile");

    let lua = Lua::new();
    let function = program.load(&lua).expect("bytecode must load");
    let raw_error = function
        .call::<()>(())
        .expect_err("assert(false) must fail at runtime");

    let mapped = program.map_runtime_error(&raw_error);
    let msg = mapped.to_string();
    // chunk line 2 + source_line 50 - 1 = 51
    assert!(
        msg.contains(":51:"),
        "mapped error must contain absolute line 51: {msg}"
    );
    assert!(
        msg.contains(location),
        "mapped error must preserve the chunk name: {msg}"
    );
}

#[test]
fn malformed_lua_reports_location_and_retains_source_diagnostic() {
    let source = "local secret =\nreturn secret";
    let location = "section Gather prologue";
    let error = LuaProgram::compile(
        source,
        location,
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        &null_emitter(),
        "Gather",
    )
    .expect_err("malformed Lua must not compile");

    match &error {
        Error::LuaCompile {
            location: actual_location,
            lua_source: actual_source,
            message,
            ..
        } => {
            assert_eq!(actual_location, location);
            assert_eq!(actual_source, source);
            assert!(
                message.contains(location),
                "the Lua diagnostic must identify its source region: {message}"
            );
        }
        other => panic!("expected Error::LuaCompile, got {other:?}"),
    }
    assert!(
        error.to_string().contains(location),
        "the displayed error must identify its source region"
    );
}

#[test]
fn lua_compilation_reports_are_ordered_exact_and_payload_free() {
    let recorder = Recorder::default();
    let source = "return 'private source payload'";
    let location = "private/location";
    LuaProgram::compile(
        source,
        location,
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        recorder.emitter(),
        "Gather",
    )
    .expect("valid Lua must compile");
    assert_eq!(
        recorder.observations(),
        vec![
            ("Gather".to_owned(), detail::LUA_COMPILATION_STARTED.clone(),),
            (
                "Gather".to_owned(),
                detail::LUA_COMPILATION_SUCCEEDED.clone(),
            ),
        ]
    );

    let recorder = Recorder::default();
    LuaProgram::compile(
        "local private =",
        location,
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        recorder.emitter(),
        "Gather",
    )
    .expect_err("malformed Lua must fail");
    let observations = recorder.observations();
    assert_eq!(
        observations,
        vec![
            ("Gather".to_owned(), detail::LUA_COMPILATION_STARTED.clone(),),
            ("Gather".to_owned(), detail::LUA_COMPILATION_FAILED.clone(),),
        ]
    );
    let trace = format!("{observations:?}");
    assert!(!trace.contains("private"));
    assert!(!trace.contains(location));
}

#[test]
fn lua_runtime_error_preserves_its_mlua_source() {
    // A Lua runtime failure is the source-bearing `LuaRuntime` variant and
    // retains the originating `mlua` error as a private `source()` instead of
    // flattening it to a string.
    let err = run("error('boom')", "").expect_err("an explicit error() must raise");
    assert!(
        matches!(err, Error::LuaRuntime { .. }),
        "a Lua runtime failure must use the source-bearing variant, got {err:?}"
    );
    assert!(
        std::error::Error::source(&err).is_some(),
        "the originating mlua error must be preserved as the error source"
    );
}
