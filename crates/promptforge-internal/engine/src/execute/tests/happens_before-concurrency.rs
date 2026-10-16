//! The concurrency traces: 6, nested tasks that join transitively,
//! run within the limits, and do not deadlock at a ceiling of 1; and
//! 12, an arm's `tasks.concurrency` limit gating its fanout within the
//! caller's ceiling, and its clamping to that ceiling.

use super::*;
use crate::execute::tests::scheduler::ceiling;

/// Trace 6: nested tasks. An arm spawns sub-tasks and joins them, then
/// the caller joins the arm and reads the sub-tasks' files - the
/// deliveries join transitively, so the reads pass. While the arm is
/// parked on its join it gives its slot back, so the sub-tasks run
/// within the arm's limit and every ancestor's, and the run must not
/// deadlock even at a ceiling of 1.
const TRACE_6: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Nested\n\n\
    ## Main\n\n\
    ```lua\n\
    local arm = tasks.spawn('### Arm')\n\
    local _, ok, result = tasks.join_any({ arm })\n\
    assert(ok and result == 'arm done', tostring(result))\n\
    return store.read('sub-one.md') .. '/' .. store.read('sub-two.md')\n\
    ```\n\n\
    ### Arm\n\n\
    ```lua\n\
    local a = tasks.spawn('#### Sub', { item = 'one' })\n\
    local b = tasks.spawn('#### Sub', { item = 'two' })\n\
    tasks.join({ a, b })\n\
    return 'arm done'\n\
    ```\n\n\
    #### Sub\n\n\
    ```lua\n\
    store.write('sub-' .. item .. '.md', 'sub ' .. item)\n\
    return 'sub'\n\
    ```\n";

/// Trace 12: concurrency. An arm calls `tasks.concurrency(2)` and then
/// runs a fanout of 10; at most 2 of its sub-tasks run at once, and the
/// whole run never exceeds the caller's ceiling of 8. Each sub-task makes
/// one model round and returns its collection index, so the result is
/// the indexes in order in every interleaving - the arm's two-at-a-time
/// admission order is spawn order, while the model replies themselves
/// may land on either of a concurrently admitted pair.
const TRACE_12: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Concurrency\n\n\
    ## Main\n\n\
    ```lua\n\
    local arm = tasks.spawn('### Arm')\n\
    local _, ok, result = tasks.join_any({ arm })\n\
    assert(ok, tostring(result))\n\
    return result\n\
    ```\n\n\
    ### Arm\n\n\
    ```lua\n\
    local n = tasks.concurrency(2)\n\
    assert(n == 2, 'the effective limit is 2, got ' .. n)\n\
    local items = {}\n\
    for i = 1, 10 do items[i] = tostring(i) end\n\
    local r = tasks.fanout('#### Sub', items)\n\
    return table.concat(r, '|')\n\
    ```\n\n\
    #### Sub\n\n\
    ```lua\n\
    models.infer('sub ' .. item)\n\
    return tostring(sys.index)\n\
    ```\n";

/// Trace 12's clamp: `tasks.concurrency(16)` under the caller's ceiling of 4
/// returns 4 - the clamp keeps prompts portable wherever they run,
/// including where the ceiling is tighter.
const TRACE_12_CLAMP: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Clamp\n\n\
    ## Main\n\n\
    ```lua\n\
    local n = tasks.concurrency(16)\n\
    assert(n == 4, 'clamped to the run ceiling of 4, got ' .. n)\n\
    return 'clamped:' .. n\n\
    ```\n";

#[test]
fn nested_tasks_join_transitively_and_do_not_deadlock_at_a_ceiling_of_one() {
    // Trace 6: the arm's sub-tasks run one at a time under the arm's
    // share, the caller reads their files after joining the arm, and the
    // run completes in every interleaving - a scheduler that let the
    // parked arm keep its slot would deadlock here. The ceiling of 1
    // admits one task at a time, so the nested tasks only run because the
    // parked arm gives its slot back.
    assert_every_interleaving_reads(
        |store| trace_scenario(TRACE_6, store).limits(ceiling(1)),
        "sub one/sub two",
    );
}

#[test]
fn an_arms_concurrency_limit_gates_its_fanout_and_the_whole_run() {
    // Trace 12: the arm lowers its limit to 2 and fans out over 10
    // sub-tasks, each making one model round and returning its index.
    // The result is the indexes in collection order in every
    // interleaving, and at no point do more than the arm's 2 sub-tasks
    // run at once (the arm itself is the third live task, within the
    // caller's ceiling of 8, which the whole run must never exceed). The
    // exact two-at-a-time request order is pinned separately, under a
    // deterministic driver, in the concurrency suite.
    let expected = (1..=10)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join("|");
    let gateway = ScriptedChat::new((1..=10).map(|i| text(&format!("r{i}"))).collect::<Vec<_>>());
    assert_every_interleaving_reads(
        |store| {
            trace_scenario(TRACE_12, store)
                .limits(ceiling(8))
                .chat(&gateway)
        },
        &expected,
    );
    assert_eq!(
        gateway.call_count(),
        30,
        "every sub-task made its one round, once per seed"
    );
}

#[test]
fn tasks_concurrency_clamps_to_the_callers_ceiling() {
    // Trace 12's clamp: under the caller's ceiling of 4, asking for 16
    // yields the effective limit 4, so a prompt stays portable wherever it
    // runs, including where the ceiling is tighter.
    assert_every_interleaving_reads(
        |store| trace_scenario(TRACE_12_CLAMP, store).limits(ceiling(4)),
        "clamped:4",
    );
}
