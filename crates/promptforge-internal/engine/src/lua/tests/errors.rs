//! The failure contract at the coroutine boundary: every failure that
//! reaches Lua is a `{ kind, message, ... }` table whose `tostring` is the
//! message, a Rust-raised error answered through the envelope arrives in
//! the same shape, an author's own raise passes through untouched, a
//! typed error substituted at the boundary keeps its kind, a kept table
//! maps back onto the executor's typed variants, and tracebacks through
//! the shim render the impl frames verbatim.

use std::num::NonZeroU32;

use mlua::MultiValue;

use promptforge_lua::{Error, ErrorKind};

use crate::execute::protocol::Answer;
use crate::lua::{CoroStep, LuaBlockResult, LuaProgram, OverflowReason};
use crate::model::ModelSet;
use crate::test_support::recording::null_emitter;

use super::{compile_block, scheduler_vm, start, test_models};

#[test]
fn models_infer_rejects_a_third_argument() {
    // `models.infer(handle?, prompt)` is the whole signature; a third
    // argument (per-call options, or anything else) raises at the call
    // site rather than being silently dropped.
    let vm = scheduler_vm(&test_models(), None);
    let program = compile_block(
        r#"local ok, err = pcall(models.infer, models.get("fast"), "yo", { temperature = 0 })
           assert(not ok, "a third argument must fail")
           assert(tostring(err) == "models.infer takes (handle?, prompt)", tostring(err))
           return "rejected""#,
    );
    match vm.start_block_coro(&program).expect("the block runs") {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => assert_eq!(text, "rejected"),
        other => panic!("expected the rejection return, got {other:?}"),
    }
}

#[test]
fn a_shim_argument_error_is_a_table_whose_tostring_is_the_message() {
    // Every failure that reaches Lua is a `{ kind, message, ... }` table:
    // `tostring` (and `..`) gives exactly the message an author saw before,
    // and a caller that branches reads `kind`. A shim's own argument error
    // is an authoring error, so its kind is `lua`.
    let vm = scheduler_vm(&test_models(), None);
    let program = compile_block(
        r#"local ok, err = pcall(models.infer, models.get("fast"), "yo", { temperature = 0 })
           assert(not ok, "a third argument must fail")
           return type(err) .. "|" .. tostring(err.kind) .. "|" .. err"#,
    );
    match vm.start_block_coro(&program).expect("the block runs") {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => {
            assert_eq!(text, "table|lua|models.infer takes (handle?, prompt)");
        }
        other => panic!("expected the rejection return, got {other:?}"),
    }
}

#[test]
fn a_failure_envelope_raises_a_table_holding_the_kind_and_fields() {
    // A Rust-raised error answered through the envelope reaches the
    // author's `pcall` in the same shape as a shim raise: `kind` names the
    // failure, the kind's fields appear beside it, and `tostring` is the
    // typed error's display text unchanged.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let program = compile_block(
        r#"local ok, err = pcall(models.infer, "hi")
           assert(not ok, "the failure envelope must raise")
           return type(err) .. "|" .. tostring(err.kind) .. "|" .. tostring(err.reason) .. "|" .. tostring(err)"#,
    );
    let CoroStep::Yielded(thread, _values) =
        vm.start_block_coro(&program).expect("the block suspends")
    else {
        panic!("the shim yield must suspend the block");
    };
    let error = Error::ContextExhausted {
        reason: OverflowReason::Provider,
    };
    let display = error.to_string();
    match vm
        .resume_block_coro_answer::<Error>(&program, &thread, Answer::Infer(Err(error)))
        .expect("the pcall'd block resumes")
    {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => {
            assert_eq!(text, format!("table|context_exhausted|provider|{display}"));
        }
        other => panic!("expected the caught failure's rendering, got {other:?}"),
    }
}

#[test]
fn a_host_callback_failure_caught_by_pcall_is_the_same_error_table() {
    // A host callback that fails directly from Rust - no envelope, no shim
    // raise - reaches the author's `pcall` as the same `{ kind, message }`
    // table: `kind` is readable at every call site, and `tostring` is the
    // message text (mlua's appended traceback is not part of it). The
    // `sys` guard fails from a metamethod rather than a call, and
    // `xpcall`'s handler sees the same normalized value.
    let vm = scheduler_vm(&test_models(), None);
    let program = compile_block(
        r#"local ok, err = pcall(models.get, "missing")
           assert(not ok, "an unbound alias must fail")
           local first = type(err) .. "|" .. tostring(err.kind) .. "|" .. tostring(err)
           local ok2, err2 = pcall(function() return sys.nothing end)
           assert(not ok2, "an unknown sys field must fail")
           local second = type(err2) .. "|" .. tostring(err2.kind) .. "|" .. tostring(err2)
           local ok3, third = xpcall(models.get, function(e)
             return type(e) .. "|" .. tostring(e.kind)
           end, "missing")
           assert(not ok3, "the handler runs for the callback failure")
           return first .. "\n" .. second .. "\n" .. third"#,
    );
    match vm.start_block_coro(&program).expect("the block runs") {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => {
            assert_eq!(
                text,
                "table|lua|models.get alias \"missing\" is not a bound model role\n\
                 table|lua|runtime error: unknown sys field 'nothing'\n\
                 table|lua"
            );
        }
        other => panic!("expected the caught failures' rendering, got {other:?}"),
    }
}

#[test]
fn the_normalizing_pcall_leaves_lua_values_and_returns_unchanged() {
    // Only a Rust-raised failure is rewritten: an author's string error and
    // an author's own table come back exactly as raised, and a successful
    // call keeps every return value, nils included.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let program = compile_block(
        r##"local ok, err = pcall(error, "plain", 0)
            assert(not ok and err == "plain", "a string error passes through")
            local own = { kind = "custom" }
            local ok2, err2 = pcall(error, own)
            assert(not ok2 and err2 == own, "an author's table passes through")
            local n = select("#", pcall(function() return 1, nil, 3 end))
            assert(n == 4, "pcall keeps the return count")
            local ok3, x, y, z = pcall(function() return 1, nil, 3 end)
            assert(ok3 and x == 1 and y == nil and z == 3, "pcall keeps the returns")
            return "unchanged""##,
    );
    match vm.start_block_coro(&program).expect("the block runs") {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => assert_eq!(text, "unchanged"),
        other => panic!("expected the pass-through return, got {other:?}"),
    }
}

#[test]
fn an_authors_plain_table_with_a_kind_is_not_read_back_as_a_raise() {
    // The read-back recognizes an error table by the shared metatable, not
    // by shape: an author's own `error({ kind = ..., message = ... })` is
    // never mistaken for a shim raise and mapped onto the executor's typed
    // variant. It fails as an ordinary Lua runtime error.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let program = compile_block(r#"error({ kind = "tool_loop_exhausted", message = "x" }, 0)"#);
    match vm.start_block_coro(&program) {
        Err(Error::LuaRuntime { .. }) => {}
        other => panic!("an author's table is an ordinary runtime failure, got {other:?}"),
    }
}

#[test]
fn an_uncaught_lua_kind_shim_raise_keeps_the_mapped_runtime_error() {
    // A `lua`-kind table that propagates out of the block is not kept as a
    // `Raised`: the mapped runtime error already holds the same message
    // with its source and the prompt line the traceback maps to, which is
    // what an authoring error needs. The block is compiled at prompt line
    // 40 and fails at chunk line 2, so the mapped author frame is line 41.
    let vm = scheduler_vm(&test_models(), None);
    let program = LuaProgram::compile(
        "local h = models.get(\"fast\")\nmodels.infer(h, \"yo\", { temperature = 0 })",
        "section `Test` prologue",
        NonZeroU32::new(40).expect("40 is non-zero"),
        &null_emitter(),
        "Test",
    )
    .expect("the driver block compiles");
    match vm.start_block_coro(&program) {
        Err(Error::LuaRuntime { message, .. }) => {
            assert!(
                message.contains("models.infer takes (handle?, prompt)"),
                "the mapped error keeps the shim's message: {message}"
            );
            assert!(
                message.contains("[string \"section `Test` prologue\"]:41:"),
                "the author frame maps to the absolute prompt line: {message}"
            );
        }
        other => panic!("a lua-kind raise must surface as the mapped runtime error, got {other:?}"),
    }
}

#[test]
fn a_typed_error_substituted_at_the_coroutine_boundary_keeps_its_kind() {
    // The shim raises the envelope's table; when that raise surfaces as the
    // coroutine's failure, the driver receives the typed error it answered
    // with, not a string and not a generic Lua runtime error. This holds
    // whether the block let the raise propagate or caught and re-raised
    // the same table.
    for source in [
        "models.infer(\"hi\")",
        "local ok, err = pcall(models.infer, \"hi\")\nerror(err, 0)",
    ] {
        let vm = scheduler_vm(&ModelSet::default(), None);
        let program = compile_block(source);
        let CoroStep::Yielded(thread, _values) =
            vm.start_block_coro(&program).expect("the block suspends")
        else {
            panic!("the shim yield must suspend the block");
        };
        let answer = Answer::Infer(Err(Error::LuaQuota {
            resource: "instruction",
        }));
        match vm.resume_block_coro_answer::<Error>(&program, &thread, answer) {
            Err(Error::LuaQuota {
                resource: "instruction",
            }) => {}
            other => panic!("block {source:?} must surface the typed quota error, got {other:?}"),
        }
    }
}

#[test]
fn a_structured_raise_surfacing_as_the_coroutine_failure_keeps_its_table() {
    // Without a retained typed error to substitute (the envelope was
    // rendered ahead of time, as a Lua-side raise would be), the failure
    // still arrives typed: the table the shim raised is kept as a
    // `Raised` value holding its kind and fields, never flattened to the
    // message string.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let program = compile_block("models.infer(\"hi\")");
    let CoroStep::Yielded(thread, _values) =
        vm.start_block_coro(&program).expect("the block suspends")
    else {
        panic!("the shim yield must suspend the block");
    };
    let (envelope, _retained) = Answer::<Error>::Infer(Err(Error::ContextExhausted {
        reason: OverflowReason::Precheck,
    }))
    .into_envelope(vm.lua())
    .expect("the envelope renders");
    match vm.resume_block_coro(&program, &thread, envelope) {
        Err(Error::Raised(raised)) => {
            assert_eq!(raised.kind, ErrorKind::ContextExhausted);
            assert!(
                raised.message.starts_with("context exhausted: "),
                "the table keeps the display message: {}",
                raised.message
            );
            assert_eq!(
                raised.fields.get("reason").map(String::as_str),
                Some("precheck")
            );
        }
        other => panic!("expected the kept table as a Raised failure, got {other:?}"),
    }
}

#[test]
fn a_raised_table_maps_onto_the_executor_error_type_by_kind() {
    // The executor's `From<promptforge_lua::Error>` turns a kept table back
    // into the variant its kind names, so a Lua-side raise classifies the
    // same way as the Rust-raised error it replaces.
    let exhausted = promptforge_lua::Raised {
        kind: ErrorKind::ContextExhausted,
        message: "context exhausted: provider".to_owned(),
        fields: [("reason".to_owned(), "provider".to_owned())]
            .into_iter()
            .collect(),
    };
    assert!(matches!(
        crate::Error::from(Error::Raised(exhausted)),
        crate::Error::ContextExhausted {
            reason: OverflowReason::Provider
        }
    ));
    let loop_exhausted = promptforge_lua::Raised {
        kind: ErrorKind::ToolLoopExhausted,
        message: "tool-call loop did not converge".to_owned(),
        fields: std::collections::BTreeMap::new(),
    };
    assert!(matches!(
        crate::Error::from(Error::Raised(loop_exhausted)),
        crate::Error::ToolLoopExhausted
    ));
    let cancelled = promptforge_lua::Raised {
        kind: ErrorKind::Cancelled,
        message: "interrupted by Ctrl-C".to_owned(),
        fields: std::collections::BTreeMap::new(),
    };
    assert!(matches!(
        crate::Error::from(Error::Raised(cancelled)),
        crate::Error::Interrupted
    ));
    // The empty-reply arm keeps the message the author saw as `detail`
    // and copies the `finish_reason` field across.
    let empty = promptforge_lua::Raised {
        kind: ErrorKind::EmptyModelReply,
        message: "the model returned an empty turn".to_owned(),
        fields: [("finish_reason".to_owned(), "length".to_owned())]
            .into_iter()
            .collect(),
    };
    match crate::Error::from(Error::Raised(empty)) {
        crate::Error::EmptyModelReply {
            detail,
            finish_reason,
        } => {
            assert_eq!(detail, "the model returned an empty turn");
            assert_eq!(finish_reason.as_deref(), Some("length"));
        }
        other => panic!("expected the empty-reply variant, got {other:?}"),
    }
}

#[test]
fn an_error_envelope_raises_at_the_call_site_without_a_position_prefix() {
    let vm = scheduler_vm(&ModelSet::default(), None);
    let (thread, _yielded) = start(&vm, r#"return models.infer("hi")"#);
    let error = thread
        .resume::<MultiValue>((false, "model is down"))
        .expect_err("the shim raises the envelope's message");
    // The raised error's message line is exactly the envelope string:
    // `error(result, 0)` suppresses the position prefix. (mlua appends
    // the traceback to the payload; that is its own rendering, not a
    // prefix on the message.)
    let mlua::Error::RuntimeError(message) = &error else {
        panic!("expected a runtime error, got {error:?}");
    };
    let first_line = message.lines().next().expect("a message line");
    assert_eq!(first_line, "model is down");
}

#[test]
fn a_traceback_through_a_shim_shows_unmapped_impl_frames() {
    let vm = scheduler_vm(&ModelSet::default(), None);
    // The var_snapshot capture fails on a reassigned `var` global: an
    // unexpected shim error, whose frames must render verbatim.
    let program = LuaProgram::compile(
        "var = 5\ncall(\"## Child\")",
        "section `Test` prologue",
        NonZeroU32::new(40).expect("40 is non-zero"),
        &null_emitter(),
        "Test",
    )
    .expect("the driver program compiles");
    let function = program.load(vm.lua()).expect("the driver program loads");
    let thread = vm
        .lua()
        .create_thread(function)
        .expect("the driver thread creates");
    let error = thread
        .resume::<MultiValue>(())
        .expect_err("the reassigned var fails the snapshot");
    let raw = error.to_string();
    assert!(
        raw.contains("crates/promptforge-internal/lua/src/__impl_coro.lua:"),
        "the shim frame renders as a verbatim file:line: {raw}"
    );
    assert!(
        !raw.contains("[string \"@crates") && !raw.contains("[string \"crates"),
        "the shim frame renders bare, outside any [string \"...\"] wrapper: {raw}"
    );
    assert!(
        raw.contains("[string \"section `Test` prologue\"]:2:"),
        "the author frame is present at chunk line 2: {raw}"
    );
    let mapped = program.map_runtime_error(&error).to_string();
    assert!(
        mapped.contains("crates/promptforge-internal/lua/src/__impl_coro.lua:"),
        "the line mapper leaves the shim frame unmapped: {mapped}"
    );
    assert!(
        mapped.contains("[string \"section `Test` prologue\"]:41:"),
        "the author frame maps to the absolute prompt line: {mapped}"
    );
}
