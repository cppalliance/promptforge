//! Cooperative cancellation: the instruction hook aborts a cancelled block,
//! and the coroutine shim's `pcall` and `xpcall` neither swallow
//! cancellation nor change an uncancelled failure.

use super::*;

#[test]
fn long_running_lua_block_cancels_cooperatively() {
    use promptforge_types::cancel::CancelHandle;

    // An unbounded loop that, without cooperative cancellation, would run
    // forever: no instruction ceiling ends it. With the cancel flag set, the
    // very first instruction-hook firing aborts it before counting itself;
    // the hook's error is the raw cancellation message, which the VM
    // classifies as `Error::Interrupted` once the flag is observed set.
    let program = LuaProgram::compile(
        "local n = 0\nwhile true do n = n + 1 end",
        "cancel loop",
        NonZeroU32::MIN,
        &null_emitter(),
        "Loop",
    )
    .expect("an infinite loop still compiles");

    let handle = CancelHandle::new();
    handle.cancel();

    let lua = Lua::new();
    let budget = install_instruction_budget(&lua).expect("hook installs on a fresh VM");
    budget.set_cancel(handle);
    let func = program
        .load(&lua)
        .expect("compiled bytecode loads into a fresh VM");
    let fired = budget.fired();
    let outcome = func.call::<()>(());

    assert_eq!(
        budget.fired(),
        fired,
        "a cancelled Lua block must abort at the first hook firing, required no counted firing"
    );
    assert!(
        budget.is_cancelled(),
        "the budget reports the installed flag as set"
    );
    let raw = outcome
        .expect_err("a cancelled loop cannot finish")
        .to_string();
    assert!(
        raw.contains("lua execution cancelled"),
        "the hook aborts the chunk under the cancel flag, got {raw}"
    );
}

#[test]
fn a_pre_cancelled_run_aborts_a_tight_loop_promptly() {
    use promptforge_types::cancel::CancelHandle;

    // No instruction ceiling aborts a runaway block anymore; the cancel flag,
    // polled by the instruction hook, is the kill switch. With the flag set
    // before the chunk starts, the first hook firing inside a tight
    // `while true do end` aborts it before counting itself.
    let handle = CancelHandle::new();
    handle.cancel();

    let vm = TestVm::new().build();
    vm.set_cancel(handle);
    let fired = vm.hook_firings();
    let outcome = run_scalar(&vm, &program("while true do end"), &null_emitter(), "Loop");

    assert_eq!(
        vm.hook_firings(),
        fired,
        "a cancelled tight loop must abort at the first hook firing, required no counted firing"
    );
    vm.teardown();
    assert!(
        matches!(outcome, Err(Error::Interrupted)),
        "expected Interrupted, got {outcome:?}"
    );
}

/// Starts `source` as a block coroutine on a shim VM whose run is already
/// cancelled.
#[track_caller]
fn start_cancelled_block(source: &str) -> Result<CoroStep> {
    let handle = promptforge_types::cancel::CancelHandle::new();
    handle.cancel();
    let vm = TestVm::new().shims().build();
    vm.set_cancel(handle);
    vm.start_block_coro(&program(source))
}

#[test]
fn a_cancelled_run_unwinds_through_an_author_pcall_loop() {
    let outcome =
        start_cancelled_block("while true do pcall(function() while true do end end) end");
    assert!(
        matches!(outcome, Err(Error::Interrupted)),
        "an author pcall must not swallow cancellation, got {outcome:?}"
    );
}

#[test]
fn a_cancelled_run_unwinds_through_an_author_xpcall_loop() {
    let outcome = start_cancelled_block(
        "while true do xpcall(function() while true do end end, function(e) return e end) end",
    );
    assert!(
        matches!(outcome, Err(Error::Interrupted)),
        "an author xpcall with a message handler must not swallow cancellation, got {outcome:?}"
    );
}

#[test]
fn a_pcall_failure_without_a_cancel_flag_still_returns_false_and_the_error() {
    let step = TestVm::new()
        .shims()
        .build()
        .start_block_coro(&program(
            "local ok, err = pcall(error, 'boom')\nreturn tostring(ok) .. '|' .. tostring(err)",
        ))
        .expect("a caught failure must not fail the block");
    let CoroStep::Done(LuaBlockResult::Returned(returned)) = step else {
        panic!("the block must return, got {step:?}");
    };
    assert_eq!(returned.as_deref(), Some("false|boom"));
}

#[test]
fn a_cancelled_run_skips_a_looping_xpcall_message_handler() {
    let outcome = start_cancelled_block(
        "xpcall(function() while true do end end, function() while true do end end)",
    );
    assert!(
        matches!(outcome, Err(Error::Interrupted)),
        "a message handler must not run under cancellation, got {outcome:?}"
    );
}

#[test]
fn an_xpcall_message_handler_without_a_cancel_flag_still_receives_the_failure() {
    let step = TestVm::new()
        .shims()
        .build()
        .start_block_coro(&program(
            "local ok, handled = xpcall(error, function(e) return 'handled ' .. tostring(e) end, 'boom')\n\
             return tostring(ok) .. '|' .. tostring(handled)",
        ))
        .expect("a handled failure must not fail the block");
    let CoroStep::Done(LuaBlockResult::Returned(returned)) = step else {
        panic!("the block must return, got {step:?}");
    };
    assert_eq!(returned.as_deref(), Some("false|handled boom"));
}
