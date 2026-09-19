//! The coroutine mechanics the shims rely on, each pinned by the spike
//! that confirmed it: the per-coroutine cancellation hook on every block
//! thread, a yield across `pcall`, `jump` through `Thread::resume`,
//! `@`-named chunk errors rendering verbatim, and scalar returns and VM
//! state rolling forward across block coroutines.

use promptforge_lua::Error;

use crate::cancel::CancelHandle;
use crate::execute::protocol::{Request, YieldParse};
use crate::lua::{CoroStep, LuaBlockResult, LuaProgram};
use crate::model::ModelSet;

use super::{compile_block, scheduler_vm};

#[test]
fn the_cancellation_hook_fires_inside_a_resumed_coroutine() {
    // Spike (a): instruction hooks are per-coroutine in PUC Lua, so the
    // main-state hook installed at construction cannot bite here. The
    // block coroutine carries the VM's hook via `Thread::set_hook`; no
    // instruction ceiling remains, so if that install regressed, this
    // pre-cancelled loop would hang the test instead of aborting.
    let handle = CancelHandle::new();
    handle.cancel();
    let vm = scheduler_vm(&ModelSet::default(), None);
    vm.set_cancel(handle);
    let program = compile_block("while true do end");
    match vm.start_block_coro(&program) {
        Err(error) => assert!(
            matches!(error, Error::Interrupted),
            "the per-coroutine hook must observe cancellation: {error:?}"
        ),
        other => panic!("a cancelled infinite loop can only fail, got {other:?}"),
    }
}

#[test]
fn every_block_coroutine_carries_the_cancellation_hook() {
    // One VM installs the hook on every block coroutine it starts, not
    // only the first: under a cancelled run, each block's first hook
    // firing aborts it. A thread that missed the install would let the
    // second block hang (the loop) or finish (the bounded for), so either
    // block escaping cancellation fails this test.
    let handle = CancelHandle::new();
    handle.cancel();
    let vm = scheduler_vm(&ModelSet::default(), None);
    vm.set_cancel(handle);
    for source in [
        "while true do end",
        "for i = 1, 100000 do end\nreturn \"done\"",
    ] {
        let program = compile_block(source);
        match vm.start_block_coro(&program) {
            Err(error) => assert!(
                matches!(error, Error::Interrupted),
                "block {source:?} must abort on the cancelled run: {error:?}"
            ),
            other => panic!("a cancelled block can only fail, got {other:?}"),
        }
    }
}

#[test]
fn a_shim_yield_suspends_and_resumes_across_pcall() {
    // Spike (b): yield across pcall (5.4+ semantics, re-confirmed on
    // 5.5). If yield could not cross the pcall boundary, the resume
    // would fail with "attempt to yield across a pcall boundary".
    let vm = scheduler_vm(&ModelSet::default(), None);
    let program = compile_block(
        "local ok, result = pcall(function() return models.infer(\"hi\") end)\n\
             assert(ok, result)\n\
             return \"pcall:\" .. result",
    );
    let CoroStep::Yielded(thread, values) =
        vm.start_block_coro(&program).expect("the block suspends")
    else {
        panic!("the shim yield must suspend the pcall'd block");
    };
    let value = values.into_iter().next().expect("one yielded value");
    let request = match Request::from_yield(vm.lua(), &value) {
        YieldParse::Request(request) => request,
        other => panic!("the shim yield is a well-formed request, got {other:?}"),
    };
    assert!(matches!(request, Request::Infer { .. }));
    match vm
        .resume_block_coro(&program, &thread, (true, "answer"))
        .expect("the suspended pcall resumes")
    {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => {
            assert_eq!(text, "pcall:answer");
        }
        other => panic!("expected the resumed return, got {other:?}"),
    }
}

#[test]
fn jump_transfers_through_thread_resume_unchanged() {
    // Spike (c): `jump` records the heading and raises its transfer
    // marker; through `Thread::resume` the slot still takes precedence
    // over the chunk's error, so the outcome matches the legacy path.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let program = compile_block("jump(\"## Target\")\nerror(\"unreachable\")");
    match vm
        .start_block_coro(&program)
        .expect("a jump is not a failure")
    {
        CoroStep::Done(LuaBlockResult::Jump(heading)) => assert_eq!(heading, "## Target"),
        other => panic!("expected the jump transfer, got {other:?}"),
    }
}

#[test]
fn at_named_chunk_errors_render_verbatim_through_resume() {
    // Spike (d): `set_name` passes an `@`-prefixed chunk name through to
    // lua_load untouched, so an error in a chunk resumed via `Thread`
    // renders as a verbatim file:line: reference with no wrapper.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let program = LuaProgram::compile_internal(
        "local x = nil\nreturn x.field",
        "@crates/promptforge-api-runtime/src/lua/__impl_probe.lua",
    )
    .expect("the probe compiles");
    let error = match vm.start_block_coro(&program) {
        Err(error) => error,
        other => panic!("the probe must fail, got {other:?}"),
    };
    let raw = error.to_string();
    assert!(
        raw.contains("crates/promptforge-api-runtime/src/lua/__impl_probe.lua:2:"),
        "the error renders as a verbatim file:line: {raw}"
    );
    assert!(
        !raw.contains("[string \"@"),
        "the chunk name carries no [string \"...\"] wrapper: {raw}"
    );
}

#[test]
fn scalar_return_and_vm_state_roll_forward_across_block_coroutines() {
    // Chunk-return semantics: a block's scalar return survives the
    // coroutine boundary, and the VM state (`var`, `reply`) written by
    // one block's coroutine is visible to the next block's coroutine.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let first = compile_block("var.count = 41\nreply = \"rolled\"\nreturn \"first-result\"");
    match vm.start_block_coro(&first).expect("block one runs") {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => {
            assert_eq!(text, "first-result");
        }
        other => panic!("expected block one's scalar return, got {other:?}"),
    }
    let second = compile_block("assert(var.count == 41)\nassert(reply == \"rolled\")\nreturn 42");
    match vm.start_block_coro(&second).expect("block two runs") {
        CoroStep::Done(LuaBlockResult::Returned(Some(text))) => assert_eq!(text, "42"),
        other => panic!("expected block two's scalar return, got {other:?}"),
    }
}
