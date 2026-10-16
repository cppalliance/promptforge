//! Tests for `RunContext` construction, forking, and task sequence seeding.

use super::*;
use crate::test_support::scenario::TEST_SEED;

fn test_prompt() -> Prompt {
    let source = concat!(
        "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n",
        "# Title\n\n## Only\n\ndone\n",
    );
    Prompt::parse(source, "run-context-test")
        .0
        .expect("the test prompt parses")
}

fn test_state(prompt: &Prompt) -> RunState {
    state_over(
        prompt,
        RunContext::new(
            "run-context-test",
            TEST_SEED,
            runvil_types::timestamp::Timestamp::UNIX_EPOCH,
        ),
    )
}

/// The state a run of `prompt` assembles from `ctx`, its owned inputs
/// moved out as `Run::new` moves them.
fn state_over(prompt: &Prompt, mut ctx: RunContext) -> RunState {
    let owned = OwnedInputs::take(&mut ctx);
    RunState::new(Arc::new(prompt.clone()), "", &ctx, owned)
}

#[test]
fn new_builds_a_context_over_the_prompt() {
    let prompt = test_prompt();
    let ctx = test_state(&prompt);
    assert_eq!(ctx.prompt().title(), prompt.title());
}

#[test]
fn accessor_returns_the_run_prompt() {
    let prompt = test_prompt();
    let ctx = test_state(&prompt);
    assert_eq!(ctx.prompt(), &prompt);
}

#[test]
fn clones_share_the_prompt_allocation() {
    let ctx = test_state(&test_prompt());
    let clone = ctx.clone();
    assert!(Arc::ptr_eq(&ctx.prompt, &clone.prompt));
}

#[test]
fn derived_values_come_from_the_prompt_and_limits() {
    let prompt = test_prompt();
    let ctx = test_state(&prompt);
    assert_eq!(
        ctx.section_count(),
        runvil_parser::detail::sections(&prompt).len()
    );
    assert_eq!(ctx.max_tool_iterations(), 24);
}

#[test]
fn forks_swap_only_their_own_fields() {
    let ctx = test_state(&test_prompt());
    let chain = ctx.with_args("chain-args");
    assert_eq!(&*chain.args_shared(), "chain-args");
    assert!(Arc::ptr_eq(&ctx.prompt, &chain.prompt));
    assert_eq!(&*ctx.args_shared(), "");

    let turns = Arc::new(AtomicU32::new(7));
    let task: TaskId = "0.4".parse().expect("a task id parses");
    let arm = ctx.with_task(task.clone(), Arc::clone(&turns));
    assert!(Arc::ptr_eq(arm.turns(), &turns));
    assert!(Arc::ptr_eq(&ctx.prompt, &arm.prompt));
    assert_eq!(arm.emitter().task(), &task);
    assert_eq!(ctx.emitter().task(), &TaskId::from(ChainId::root()));
}

#[test]
fn the_root_task_sequence_starts_where_the_context_says() {
    let ctx = state_over(
        &test_prompt(),
        RunContext::new(
            "run-context-test",
            TEST_SEED,
            runvil_types::timestamp::Timestamp::UNIX_EPOCH,
        )
        .provenance_start(5),
    );
    ctx.emitter()
        .report("Only", runvil_types::event::lifecycle::SECTION_STARTED);
    let events = ctx.take_events();
    assert_eq!(events[0].provenance().task, TaskId::from(ChainId::root()));
    assert_eq!(
        events[0].provenance().seq,
        5,
        "the root task's first stamp continues past the caller's parse events"
    );
}

#[test]
fn a_task_fork_reports_into_the_shared_buffer_under_its_own_task() {
    let ctx = test_state(&test_prompt());
    let task: TaskId = "0.1".parse().expect("a task id parses");
    let arm = ctx.with_task(task.clone(), Arc::new(AtomicU32::new(0)));
    ctx.emitter()
        .report("Only", runvil_types::event::lifecycle::SECTION_STARTED);
    arm.emitter()
        .report("Only", runvil_types::event::lifecycle::SECTION_STARTED);
    // Both emitters share one buffer, drained through either context.
    let events = arm.events.take();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].provenance().task, TaskId::from(ChainId::root()));
    assert_eq!(events[1].provenance().task, task);
    assert_eq!(events[1].provenance().seq, 0);
}
