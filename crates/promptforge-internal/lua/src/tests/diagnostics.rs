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
        message.contains(r#"[string "line 1"]:2:"#),
        "runtime error must name the chunk by its first prompt line and give the failing line: {message}"
    );
    assert!(
        !message.contains(location),
        "the display location must not be the chunk name: {message}"
    );
    assert!(
        !message.contains("?:"),
        "stripped debug info must not leave '?:' in the traceback: {message}"
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
        .register(
            crate::alias::Alias::parse("grab".to_owned()).expect("a valid alias"),
            schema,
        )
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
    let msg = r#"[string "line 50"]:2: assertion failed!"#;
    let result = map_chunk_line_to_absolute(
        msg,
        &[(
            "line 50",
            location,
            NonZeroU32::new(50).expect("50 is non-zero"),
        )],
    );
    assert_eq!(
        result,
        "section `Web Search` epilog:51: section `Web Search` epilog:51: assertion failed!"
    );
}

#[test]
fn map_chunk_line_to_absolute_only_rewrites_matching_chunk() {
    let msg = r#"[string "line 50"]:51: assertion failed!
stack traceback:
        [string "line 22"]:3: in main chunk"#;
    let result = map_chunk_line_to_absolute(
        msg,
        &[(
            "line 22",
            "section `Main` prologue",
            NonZeroU32::new(22).expect("22 is non-zero"),
        )],
    );
    assert!(
        result.contains("[string \"line 50\"]:51:"),
        "child absolute line must stay intact: {result}"
    );
    assert!(
        result.starts_with("section `Main` prologue:24: ")
            && result.contains("\n        section `Main` prologue:24: in main chunk"),
        "parent chunk line must map with parent source_line: {result}"
    );
    assert!(
        !result.contains("[string \"line 22\"]"),
        "parent chunk-relative marker must be rewritten: {result}"
    );
}

#[test]
fn map_chunk_line_to_absolute_leaves_an_already_mapped_marker_alone() {
    let lib = (
        "line 50",
        "section `lib` prologue",
        NonZeroU32::new(50).expect("50 is non-zero"),
    );
    let child = map_chunk_line_to_absolute(r#"[string "line 50"]:2: boom"#, &[lib]);
    assert_eq!(
        child,
        "section `lib` prologue:51: section `lib` prologue:51: boom"
    );
    let parent = map_chunk_line_to_absolute(&child, &[lib]);
    assert_eq!(
        parent, child,
        "a second pass through the same owner must not rewrite line 51 again"
    );
}

#[test]
fn map_chunk_line_to_absolute_keeps_original_digits_on_overflow() {
    // source_line + chunk_line - 1 must not wrap; on overflow the original
    // chunk-relative digits are preserved rather than a wrong absolute line.
    let msg = r#"[string "x"]:5: boom"#;
    let result = map_chunk_line_to_absolute(msg, &[("x", "section `x` prologue", NonZeroU32::MAX)]);
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
        &[(
            "line 10",
            "section `Main` prologue",
            NonZeroU32::new(10).expect("10 is non-zero"),
        )],
    );
    assert_eq!(result, msg);
}

#[test]
fn map_chunk_line_to_absolute_maps_each_owners_markers_once_under_one_tag() {
    let msg = r#"[string "line 50"]:2: boom
stack traceback:
        [string "line 50"]:2: in function 'boom'
        [string "line 30"]:3: in main chunk"#;
    let owners = [
        (
            "line 30",
            "caller",
            NonZeroU32::new(30).expect("30 is non-zero"),
        ),
        (
            "line 50",
            "lib",
            NonZeroU32::new(50).expect("50 is non-zero"),
        ),
    ];
    assert_eq!(
        map_chunk_line_to_absolute(msg, &owners),
        "lib:51: lib:51: boom
stack traceback:
        lib:51: in function 'boom'
        caller:32: in main chunk"
    );
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
fn a_location_longer_than_lua_keeps_in_a_chunk_name_still_maps() {
    // Lua keeps a string chunk name whole only under 45 bytes.
    let location = "section `Shared Research Helper Functions` (line 1200) prologue";
    assert!(location.len() >= 60, "the fixture must exceed Lua's cut");
    let program = LuaProgram::compile(
        "assert(false)",
        location,
        NonZeroU32::new(40).expect("40 is non-zero"),
        &null_emitter(),
        "Shared Research Helper Functions",
    )
    .expect("valid Lua must compile");
    let lua = Lua::new();
    let raw_error = program
        .load(&lua)
        .expect("bytecode must load")
        .call::<()>(())
        .expect_err("assert(false) must fail at runtime");
    let mapped = program.map_runtime_error(&raw_error).to_string();
    assert!(
        mapped.contains(&format!("{location}:40:")),
        "the long location must map to absolute line 40: {mapped}"
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
                message.contains(r#"[string "line 1"]"#),
                "the Lua diagnostic must name the chunk by its first prompt line: {message}"
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
fn lua_runtime_error_keeps_its_mlua_cause_out_of_the_source_chain() {
    // A Lua runtime failure is the cause-bearing `LuaRuntime` variant: its
    // message already holds the `mlua` error's text, so the originating
    // error is kept behind `cause()` and a chain printer shows it once.
    let err = run("error('boom')", "").expect_err("an explicit error() must raise");
    assert!(
        matches!(err, Error::LuaRuntime { .. }),
        "a Lua runtime failure must use the cause-bearing variant, got {err:?}"
    );
    assert!(
        matches!(err.cause(), Some(cause) if cause.is::<mlua::Error>()),
        "the originating mlua error must stay reachable as the cause"
    );
    assert!(
        std::error::Error::source(&err).is_none(),
        "the cause must not repeat as a chain segment"
    );
}
