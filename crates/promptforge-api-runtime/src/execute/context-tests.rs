use super::*;
use crate::observe::NullObserver;

fn test_prompt() -> Prompt {
    let source = concat!(
        "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n",
        "# Title\n\n## Only\n\ndone\n",
    );
    Prompt::parse(source, "run-context-test", &NullObserver::default())
        .expect("the test prompt parses")
}

fn test_context(prompt: &Prompt) -> RunState {
    RunState::new(
        Arc::new(prompt.clone()),
        "",
        &promptforge_vfs::empty(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &RunContext::new(
            "run-context-test",
            1,
            promptforge_api_types::timestamp::Timestamp::UNIX_EPOCH,
        ),
    )
}

#[test]
fn new_builds_a_context_over_the_prompt() {
    let prompt = test_prompt();
    let ctx = test_context(&prompt);
    assert_eq!(ctx.prompt().title(), prompt.title());
}

#[test]
fn accessor_returns_the_run_prompt() {
    let prompt = test_prompt();
    let ctx = test_context(&prompt);
    assert_eq!(ctx.prompt(), &prompt);
}

#[test]
fn clones_share_the_prompt_allocation() {
    let ctx = test_context(&test_prompt());
    let clone = ctx.clone();
    assert!(Arc::ptr_eq(&ctx.prompt, &clone.prompt));
}

#[test]
fn derived_values_come_from_the_prompt_and_limits() {
    let prompt = test_prompt();
    let ctx = test_context(&prompt);
    assert_eq!(ctx.section_count(), prompt.sections().len());
    assert_eq!(ctx.max_tool_iterations(), 24);
}

#[test]
fn forks_swap_only_their_own_fields() {
    let ctx = test_context(&test_prompt());
    let chain = ctx.with_args("chain-args");
    assert_eq!(chain.args(), "chain-args");
    assert!(Arc::ptr_eq(&ctx.prompt, &chain.prompt));
    assert_eq!(ctx.args(), "");

    let turns = Arc::new(AtomicU32::new(7));
    let task: TaskId = "0.4".parse().expect("a task id parses");
    let arm = ctx.with_task(task.clone(), Arc::clone(&turns));
    assert!(Arc::ptr_eq(arm.turns(), &turns));
    assert!(Arc::ptr_eq(&ctx.prompt, &arm.prompt));
    assert_eq!(arm.emitter().task(), &task);
    assert_eq!(ctx.emitter().task(), &TaskId::from(ChainId::root()));
}

#[test]
fn a_task_fork_reports_into_the_shared_buffer_under_its_own_task() {
    let ctx = test_context(&test_prompt());
    let task: TaskId = "0.1".parse().expect("a task id parses");
    let arm = ctx.with_task(task.clone(), Arc::new(AtomicU32::new(0)));
    ctx.emitter()
        .report("Only", crate::observe::detail::SECTION_STARTED);
    arm.emitter()
        .report("Only", crate::observe::detail::SECTION_STARTED);
    // Both emitters share one buffer, drained through either context.
    let events = arm.events.take();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].provenance().task, TaskId::from(ChainId::root()));
    assert_eq!(events[1].provenance().task, task);
    assert_eq!(events[1].provenance().seq, 0);
}
