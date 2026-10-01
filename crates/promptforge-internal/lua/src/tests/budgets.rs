//! Execution budgets: no instruction ceiling, the configured log budget
//! during the shared replay, and the memory ceiling.

use super::*;

#[test]
fn a_loop_exceeding_the_old_instruction_budget_completes() {
    // The instruction trip limit is gone: a block that runs far past the old
    // ~1e7-instruction ceiling (10_000 instructions per hook firing, 1_000
    // firings) completes instead of tripping a quota error. The hook still
    // fires throughout, polling the cancel flag.
    let out = run(
        "local n = 0\nfor i = 1, 8000000 do n = n + 1 end\nreturn n",
        "",
    )
    .expect("a loop past the old instruction budget must complete");
    assert_eq!(out.returned.as_deref(), Some("8000000"));
}

#[test]
fn shared_replay_consumes_the_configured_log_budget() {
    // `apply_lua_limits` lands before the replay, so the replay spends the
    // configured log budget rather than the construction defaults.
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Budget").expect("VM builds");
    vm.apply_lua_limits(DEFAULT_LUA_MEMORY_BYTES, 1)
        .expect("limits apply");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values inject");
    let observer = null_emitter();
    vm.install_engine_globals(&observer, "Budget")
        .expect("Engine globals must install");
    let error = vm
        .replay_shared(
            &program("log('one')\nlog('two')"),
            &null_emitter(),
            "Budget",
        )
        .expect_err("the second log must exhaust the configured budget");
    assert!(
        matches!(
            error,
            Error::LuaQuota {
                resource: "log event"
            }
        ),
        "log-budget exhaustion must surface as a typed LuaQuota: {error:?}"
    );
    vm.teardown(&null_emitter(), "Budget");
}

#[test]
fn the_memory_budget_error_stays_reachable() {
    // The instruction trip limit is gone, but the heap ceiling still refuses
    // a block that allocates past the memory budget `apply_lua_limits` set.
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Budget").expect("VM builds");
    vm.apply_lua_limits(4 * 1024 * 1024, DEFAULT_LUA_LOG_EVENTS)
        .expect("limits apply");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values inject");
    let observer = null_emitter();
    vm.install_engine_globals(&observer, "Budget")
        .expect("Engine globals must install");
    let error = run_scalar(
        &vm,
        &program(
            "local t = {}\nlocal i = 1\nwhile true do t[i] = string.rep('x', 16384) i = i + 1 end",
        ),
        &null_emitter(),
        "Budget",
    )
    .expect_err("allocation past the heap ceiling must fail");
    assert!(
        lua_error_message(&error).contains("memory"),
        "the memory ceiling must surface its refusal: {error:?}"
    );
    vm.teardown(&null_emitter(), "Budget");
}
