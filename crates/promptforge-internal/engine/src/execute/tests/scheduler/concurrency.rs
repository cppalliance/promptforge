//! The admission limit: `tasks.concurrency` clamps to the parent chain's
//! limit and reads the effective limit back, a queued task reads
//! `blocked == 'queued'` until its start event fires at admission, a
//! resumed task is admitted ahead of fresh starts, and a nested fanout
//! does not deadlock under a ceiling of one.

use std::num::NonZeroUsize;
use std::time::Duration;

use super::*;
use crate::test_support::tokio_driver::TokioDriver;

/// Builds the run context and its observing host under the given limits.
fn limited_context(
    prompt: &Prompt,
    store: &TestStore,
    limits: RunLimits,
    observer: Arc<dyn Observer>,
) -> (RunState, RunHost) {
    scheduler_context_from(
        prompt,
        store,
        &test_context(EXECUTION).limits(limits),
        RunHost::new().observer(observer),
    )
}

/// The run's concurrency ceiling narrowed to `n` admitted tasks.
fn ceiling(n: usize) -> RunLimits {
    RunLimits::new().max_concurrency(NonZeroUsize::new(n).expect("the ceiling is non-zero"))
}

#[tokio::test(flavor = "current_thread")]
async fn tasks_concurrency_clamps_to_the_host_ceiling_and_reads_the_effective_limit_back() {
    // The main walk's parent is the host ceiling of 4: asking for 16
    // clamps to 4, a later 2 lowers it, the no-argument form reads the
    // current limit back, and a later 4 climbs back to the parent's
    // limit - the setter is `min(n, parent)`, never an error.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Limits\n\n\
        ## Main\n\n\
        ```lua\n\
        local a = tasks.concurrency(16)\n\
        local b = tasks.concurrency(2)\n\
        local c = tasks.concurrency()\n\
        local d = tasks.concurrency(4)\n\
        return a .. '|' .. b .. '|' .. c .. '|' .. d\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = limited_context(
        &prompt,
        &TestStore::new(),
        ceiling(4),
        Arc::new(NullObserver::default()),
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the run completes");

    assert_eq!(out, "4|2|2|4");
}

#[tokio::test(flavor = "current_thread")]
async fn tasks_concurrency_rejects_an_argument_that_is_not_a_positive_whole_number() {
    // The shim checks the argument at the call site, so every refused
    // shape raises the same `lua` error value an author `pcall` catches:
    // zero, a negative, a fraction, a string, a table, and a whole-number
    // float too large for the limit's 64-bit range - the shim's
    // whole-number check lets the last through and the parser refuses it
    // as the call's error, so it raises at the call site like the rest.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Limits\n\n\
        ## Main\n\n\
        ```lua\n\
        local function attempt(value)\n\
          local ok, err = pcall(tasks.concurrency, value)\n\
          assert(not ok, 'concurrency(' .. tostring(value) .. ') must be refused')\n\
          assert(err.kind == 'lua', tostring(err))\n\
          assert(tostring(err):find('positive whole number', 1, true), tostring(err))\n\
          return 'refused'\n\
        end\n\
        local a = attempt(0)\n\
        local b = attempt(-3)\n\
        local c = attempt(2.5)\n\
        local d = attempt('two')\n\
        local e = attempt({})\n\
        local f = attempt(1e20)\n\
        assert(a == b and b == c and c == d and d == e and e == f, a .. ' / ' .. b .. ' / ' .. c)\n\
        return a\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = limited_context(
        &prompt,
        &TestStore::new(),
        ceiling(4),
        Arc::new(NullObserver::default()),
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the caught refusals end the run normally");

    assert_eq!(out, "refused");
}

#[tokio::test(flavor = "current_thread")]
async fn tasks_concurrency_accepts_a_whole_number_float_as_a_limit() {
    // `2.0` is a positive whole number: the shim's check accepts it and
    // the parser converts the float, so the call sets the limit and
    // reads the effective value back exactly like an integer argument.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Limits\n\n\
        ## Main\n\n\
        ```lua\n\
        local set = tasks.concurrency(2.0)\n\
        assert(set == 2, 'the float limit clamps to 2, got ' .. set)\n\
        local read = tasks.concurrency()\n\
        assert(read == 2, 'the effective limit reads back 2, got ' .. read)\n\
        return 'ok'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = limited_context(
        &prompt,
        &TestStore::new(),
        ceiling(4),
        Arc::new(NullObserver::default()),
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the run completes");

    assert_eq!(out, "ok");
}

#[tokio::test(flavor = "current_thread")]
async fn a_queued_task_reads_blocked_queued_and_its_start_event_fires_at_admission() {
    // Ceiling 1: the first child is admitted and parks on a slow model
    // round; the second stays queued - `tasks.status` reads `running`
    // with `blocked == 'queued'` and no section - and its start event
    // fires only once the first child ends and frees the slot.
    let gateway = ScriptedGateway::start(vec![
        resp_delayed_text("slow", Duration::from_millis(200)),
        resp_text("fast"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Limits\n\n\
        ## Main\n\n\
        ```lua\n\
        local a = tasks.spawn('## Child')\n\
        local b = tasks.spawn('## Child')\n\
        local s = tasks.status(b)\n\
        assert(s.state == 'running', s.state)\n\
        assert(s.blocked == 'queued', tostring(s.blocked))\n\
        assert(s.section == nil, tostring(s.section))\n\
        local _, ok = tasks.join_any({ a })\n\
        assert(ok)\n\
        local _, ok2 = tasks.join_any({ b })\n\
        assert(ok2)\n\
        return 'done'\n\
        ```\n\n\
        ## Child\n\n\
        ```lua\n\
        return models.infer('slow please')\n\
        ```\n";
    let prompt = parse(md);
    let recorder = Arc::new(Recorder::default());
    let (ctx, host) = limited_context(
        &prompt,
        &TestStore::new(),
        ceiling(1),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the run completes");

    assert_eq!(out, "done");
    let events = recorder.events();
    assert_eq!(
        events
            .iter()
            .filter(|(_, detail)| *detail == "Task started")
            .count(),
        2,
        "both children start exactly once: {events:?}"
    );
    let a_succeeded = events
        .iter()
        .position(|(_, detail)| detail == "Task succeeded")
        .expect("the first child succeeded");
    let b_started = events
        .iter()
        .rposition(|(_, detail)| detail == "Task started")
        .expect("the second child started");
    assert!(
        a_succeeded < b_started,
        "the second child's start event fires at its admission, after the first child ends: \
         {events:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_start_event_reports_the_spawning_section_not_the_admission_time_one() {
    // Ceiling 1: the walk spawns its child and jumps to the next section
    // while the child is still queued; the child is admitted only when
    // the walk parks on its join, by which time the walk's current
    // section is Next. The start event must still report the spawning
    // section, Main - the spawn-time capture, not an admission-time read
    // of the spawner's position.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Sections\n\n\
        ## Main\n\n\
        ```lua\n\
        var.t = tasks.spawn('### Worker')\n\
        jump('## Next')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\nreturn 'work'\n```\n\n\
        ## Next\n\n\
        ```lua\n\
        local _, ok = tasks.join_any({ var.t })\n\
        assert(ok)\n\
        return 'done'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = limited_context(
        &prompt,
        &TestStore::new(),
        ceiling(1),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the run completes");

    assert_eq!(out, "done");
    let events = recorder.events();
    let started = events
        .iter()
        .filter(|(_, detail)| *detail == "Task started")
        .collect::<Vec<_>>();
    assert_eq!(started.len(), 1, "exactly one start event: {events:?}");
    assert_eq!(
        started[0].0, "Main",
        "the start event reports the spawning section, not the admission-time one: {events:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_resumed_task_is_admitted_ahead_of_fresh_starts() {
    // Ceiling 1. The walker task spawns three sub-tasks and joins them
    // one at a time, inferring between rounds; each sub-task infers once.
    // When a sub-task ends it wakes the walker, and the walker's
    // re-acquire must beat the next sub-task's first admission - a
    // scheduler that admitted fresh starts first would let S2's request
    // reach the gateway before the walker's W1.
    let gateway = ScriptedGateway::start(vec![
        resp_text("S1"),
        resp_text("W1"),
        resp_text("S2"),
        resp_text("W2"),
        resp_text("S3"),
        resp_text("W3"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Resume\n\n\
        ## Main\n\n\
        ```lua\n\
        local w = tasks.spawn('### Walker')\n\
        local _, ok = tasks.join_any({ w })\n\
        assert(ok)\n\
        return 'done'\n\
        ```\n\n\
        ### Walker\n\n\
        ```lua\n\
        local set = {}\n\
        for i = 1, 3 do set[i] = tasks.spawn('#### Sub', { index = i }) end\n\
        for i = 1, 3 do\n\
          tasks.join_any({ set[i] })\n\
          models.infer('W' .. i)\n\
        end\n\
        return 'walker done'\n\
        ```\n\n\
        #### Sub\n\n\
        ```lua\n\
        return models.infer('S' .. sys.index)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = limited_context(
        &prompt,
        &TestStore::new(),
        ceiling(1),
        Arc::new(NullObserver::default()),
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the run completes");

    assert_eq!(out, "done");
    assert_eq!(
        request_prompts(&gateway),
        vec!["S1", "W1", "S2", "W2", "S3", "W3"],
        "each sub-task's end resumes the walker before the next sub-task starts"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_arm_that_lowers_its_limit_runs_its_fanout_two_arms_at_a_time() {
    // The arm calls `tasks.concurrency(2)` and fans out over four
    // sub-tasks, each making two model rounds. Under a deterministic
    // driver the request order proves the limit: the first two arms
    // interleave their rounds (x:a, y:a, x:b, y:b), and the next two are
    // admitted only once both of the first have ended - a limit that
    // admitted a third arm early would interleave x:a, y:a, z:a, ...
    let gateway = ScriptedGateway::start(vec![
        resp_text("r1"),
        resp_text("r2"),
        resp_text("r3"),
        resp_text("r4"),
        resp_text("r5"),
        resp_text("r6"),
        resp_text("r7"),
        resp_text("r8"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Limits\n\n\
        ## Main\n\n\
        ```lua\n\
        local arm = tasks.spawn('### Arm')\n\
        local _, ok = tasks.join_any({ arm })\n\
        assert(ok)\n\
        return 'done'\n\
        ```\n\n\
        ### Arm\n\n\
        ```lua\n\
        local n = tasks.concurrency(2)\n\
        assert(n == 2, 'the effective limit is 2, got ' .. n)\n\
        local r = fanout('#### Sub', {'x', 'y', 'z', 'w'})\n\
        return table.concat(r, ',')\n\
        ```\n\n\
        #### Sub\n\n\
        ```lua\n\
        local a = models.infer(item .. ':a')\n\
        local b = models.infer(item .. ':b')\n\
        return a .. b\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = limited_context(
        &prompt,
        &TestStore::new(),
        ceiling(8),
        Arc::new(NullObserver::default()),
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the limited fanout completes");

    assert_eq!(out, "done");
    assert_eq!(
        request_prompts(&gateway),
        vec!["x:a", "y:a", "x:b", "y:b", "z:a", "w:a", "z:b", "w:b"],
        "the arm's limit admits two sub-tasks at a time"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_nested_fanout_does_not_deadlock_under_a_ceiling_of_one() {
    // Ceiling 1 over a three-arm fanout whose every arm runs its own
    // three-arm fanout: each arm is admitted one at a time, and while an
    // arm is parked on its inner join it gives its slot back so its
    // inner arms can run - otherwise the run would deadlock with the
    // outer arm holding the only slot while its inner arms wait for it.
    // Results place by collection index at both levels.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Arm', {'a', 'b', 'c'})\n\
        return table.concat(r, ',')\n\
        ```\n\n\
        ### Arm\n\n\
        ```lua\n\
        local inner = fanout('#### Sub', {'x', 'y', 'z'})\n\
        local parts = {}\n\
        for i = 1, #inner do parts[i] = inner[i].text end\n\
        return item .. ':' .. table.concat(parts, '+')\n\
        ```\n\n\
        #### Sub\n\n\
        ```lua\n\
        return item .. sys.index\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = limited_context(
        &prompt,
        &TestStore::new(),
        ceiling(1),
        Arc::new(NullObserver::default()),
    );
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        TokioDriver::new(&ctx, host, None).drive(),
    )
    .await
    .expect("the nested fanout must not deadlock");

    assert_eq!(
        out.expect("the nested fanout completes"),
        "a:x1+y2+z3,b:x1+y2+z3,c:x1+y2+z3"
    );
}
