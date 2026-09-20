//! The effect record: every effect kind projects onto a record that
//! round-trips through serde, and the projection drops exactly the live
//! handles. Then the run's host boundary: `Done` waits on outstanding
//! effects, a drop is an answer, and the run is `Send`.

use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge_api_types::event::Event;
use promptforge_api_types::tools::ToolId;
use promptforge_model_client::model::{ModelInvocation, Temperature};
use serde_json::json;

use super::*;
use crate::execute::protocol::StoreOp;
use crate::input::{InputError, InputOutcome};
use crate::model::{Message, ToolSchema};
use crate::model::{ModelBinding, ModelId};
use crate::observe::NullObserver;
use crate::test_support::TestBroker;

/// A context for the run `run-test` under fixed host inputs; nothing here
/// reads the seed or `sys.when`.
fn run_context() -> RunContext {
    RunContext::new(
        "run-test",
        1,
        promptforge_api_types::timestamp::Timestamp::UNIX_EPOCH,
    )
}

/// Serializes the record and reads it back: the round trip a run log and
/// a replay depend on.
fn round_trip(record: &EffectRecord) -> EffectRecord {
    let text = serde_json::to_string(record).expect("a record serializes");
    serde_json::from_str(&text).expect("a serialized record deserializes")
}

fn binding() -> ModelBinding {
    ModelBinding::new(
        "writer",
        "A general model for tests",
        ModelId::from_validated("gateway", "test-model"),
        ModelInvocation {
            temperature: Some(Temperature::new(0.2).expect("0.2 is in range")),
            max_tokens: NonZeroU32::new(256),
            thinking: Some(false),
        },
        NonZeroU32::new(4096).expect("4096 is non-zero"),
    )
}

#[test]
fn a_chat_effect_records_its_model_messages_tools_and_invocation() {
    let binding = binding();
    let effect = Effect::Chat {
        options: binding.completion_options(),
        binding,
        messages: vec![Message::user("ask")],
        tools: vec![
            ToolSchema::new("grab", "Grab a value", json!({ "type": "object" }))
                .expect("a valid schema"),
        ],
        stream: true,
    };
    let record = effect.record();
    assert_eq!(
        record,
        EffectRecord::Chat {
            model: "test-model".to_owned(),
            alias: "writer".to_owned(),
            messages: vec![json!({ "role": "user", "content": "ask" })],
            tools: vec!["grab".to_owned()],
            temperature: Some(0.2),
            max_tokens: Some(256),
            thinking: Some(false),
        }
    );
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_tool_call_effect_records_its_identity_alias_and_args() {
    let effect = Effect::ToolCall {
        tool: ToolId::parse("tests/tools/echo").expect("a valid id"),
        alias: "echo".to_owned(),
        args: json!({ "value": "hi" }),
    };
    let record = effect.record();
    assert_eq!(
        record,
        EffectRecord::ToolCall {
            tool: ToolId::parse("tests/tools/echo").expect("a valid id"),
            alias: "echo".to_owned(),
            args: json!({ "value": "hi" }),
        }
    );
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_user_input_effect_records_its_execution_and_section() {
    let effect = Effect::UserInput {
        execution: "run-1".to_owned(),
        section: "Only".to_owned(),
    };
    let record = effect.record();
    assert_eq!(
        record,
        EffectRecord::UserInput {
            execution: "run-1".to_owned(),
            section: "Only".to_owned(),
        }
    );
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_store_effect_records_its_operation_and_drops_the_access_handle() {
    let access = Arc::new(
        promptforge_vfs::empty()
            .acquire(shared_vfs::Origin::new("run test fixture"))
            .expect("the stock backend acquires"),
    );
    let effect = Effect::Store {
        access,
        op: StoreOp::Read {
            path: "notes.md".to_owned(),
            start: Some(1),
            end: None,
        },
    };
    let record = effect.record();
    assert_eq!(
        record,
        EffectRecord::Store {
            op: StoreOp::Read {
                path: "notes.md".to_owned(),
                start: Some(1),
                end: None,
            },
        }
    );
    // The record is the operation alone: nothing of the handle survives.
    let text = serde_json::to_string(&record).expect("a record serializes");
    assert!(
        !text.contains("access"),
        "the store record carries no handle: {text}"
    );
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_timer_effect_records_its_seconds() {
    let effect = Effect::Timer { seconds: 0.25 };
    let record = effect.record();
    assert_eq!(record, EffectRecord::Timer { seconds: 0.25 });
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_task_events_effect_records_its_task_and_last_bound() {
    let task: promptforge_api_types::ids::TaskId = "0.2".parse().expect("a task id parses");
    let effect = Effect::TaskEvents {
        task: task.clone(),
        last: Some(4),
    };
    let record = effect.record();
    assert_eq!(
        record,
        EffectRecord::TaskEvents {
            task,
            last: Some(4)
        }
    );
    assert_eq!(round_trip(&record), record);
}

/// A run over one section whose only Lua block is `body`, capability-free.
fn run_of(body: &str, ctx: RunContext) -> Run {
    let source = format!(
        "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n# Run\n\n## Only\n\n```lua\n{body}\n```\n"
    );
    let prompt = Prompt::parse(&source, "run-test", &NullObserver::default())
        .expect("the run test prompt parses");
    Run::new(Arc::new(prompt), "", ctx)
}

/// A broker that never answers, so only a dropped effect ends the wait.
struct PendingBroker;

#[async_trait::async_trait]
impl TestBroker for PendingBroker {
    async fn user_input(
        &self,
        _execution: &str,
        _section: &str,
    ) -> std::result::Result<InputOutcome, InputError> {
        std::future::pending().await
    }
}

/// The one effect a pending step issued.
fn only_effect(step: Step) -> (EffectId, Effect) {
    let Step::Pending { mut effects, .. } = step else {
        panic!("the step is pending, got {step:?}");
    };
    assert_eq!(
        effects.len(),
        1,
        "exactly one effect is issued: {effects:?}"
    );
    let (id, _, effect) = effects.remove(0);
    (id, effect)
}

/// Whether `events` carry the run's end boundary.
fn ended(events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::RunSucceeded { .. } | Event::RunFailed { .. }))
}

const fn assert_send<T: Send>() {}

#[test]
fn a_run_is_send() {
    // The host boundary: one caller at a time, and the thread may change
    // between calls, so the run (its Lua VMs included) must cross threads.
    assert_send::<Run>();
}

#[test]
fn done_is_withheld_while_a_store_effect_is_outstanding_and_delivered_after_dropped() {
    let mut run = run_of(
        "store.write('notes.md', 'kept')\nreturn 'unreachable'",
        run_context(),
    );
    let (id, effect) = only_effect(run.step());
    assert!(
        matches!(effect, Effect::Store { .. }),
        "the store call is one store effect: {effect:?}"
    );
    // The host cancels while the store operation is out. The next step
    // tears the run down and reports its end, but the effect still owes
    // its answer, so `Done` waits.
    run.cancel();
    let step = run.step();
    let Step::Pending { effects, events } = step else {
        panic!("Done is withheld while the store effect is unanswered, got {step:?}");
    };
    assert!(effects.is_empty(), "a torn-down run issues nothing");
    assert!(
        ended(&events),
        "the run's end boundary is reported: {events:?}"
    );
    run.resume(id, EffectAnswer::Dropped);
    let step = run.step();
    let Step::Done { result, .. } = step else {
        panic!("Done follows the last answer, got {step:?}");
    };
    assert!(
        matches!(result, RunResult::Cancelled),
        "the cancelled run reports as cancelled: {result:?}"
    );
}

#[test]
fn a_dropped_answer_resumes_a_waiting_chain_with_the_cancelled_error() {
    let mut run = run_of(
        "return user_input()",
        run_context().input_broker(Arc::new(PendingBroker)),
    );
    let (id, effect) = only_effect(run.step());
    assert!(matches!(effect, Effect::UserInput { .. }));
    run.resume(id, EffectAnswer::Dropped);
    let step = run.step();
    let Step::Done { result, .. } = step else {
        panic!("the dropped wait ends the run, got {step:?}");
    };
    assert!(
        matches!(result, RunResult::Cancelled),
        "the chain resumed with the cancelled error and the run reports it: {result:?}"
    );
}

#[test]
fn an_orphaned_effects_real_answer_is_discarded_and_still_counts_as_the_answer() {
    let mut run = run_of(
        "store.write('notes.md', 'kept')\nreturn 'unreachable'",
        run_context(),
    );
    let (id, _) = only_effect(run.step());
    run.cancel();
    assert!(matches!(run.step(), Step::Pending { .. }));
    // The host performed the operation before it learned of the cancel:
    // its answer is the effect's one answer, discarded rather than applied.
    run.resume(
        id,
        EffectAnswer::Store(Ok(crate::execute::protocol::StoreOutcome::Unit)),
    );
    assert!(
        matches!(
            run.step(),
            Step::Done {
                result: RunResult::Cancelled,
                ..
            }
        ),
        "Done follows the orphan's answer"
    );
}

#[test]
fn an_answer_for_an_unissued_effect_is_an_internal_error() {
    let mut run = run_of(
        "return user_input()",
        run_context().input_broker(Arc::new(PendingBroker)),
    );
    let (id, _) = only_effect(run.step());
    run.resume(EffectId(id.0 + 99), EffectAnswer::Timer);
    // The unknown id ended the run; the real effect is now an orphan whose
    // answer the host still owes.
    let Step::Pending { events, .. } = run.step() else {
        panic!("the run waits for the orphan's answer");
    };
    assert!(ended(&events));
    run.resume(id, EffectAnswer::Dropped);
    let Step::Done { result, .. } = run.step() else {
        panic!("Done follows the orphan's answer");
    };
    let RunResult::Failure(error) = result else {
        panic!("an unknown id fails the run, got {result:?}");
    };
    assert_eq!(error.kind(), crate::execute::RunErrorKind::Internal);
    assert!(
        error.to_string().contains("did not issue"),
        "the failure names the unknown id: {error}"
    );
}

#[test]
fn an_answer_of_the_wrong_kind_for_a_pending_effect_fails_loudly() {
    let mut run = run_of(
        "return user_input()",
        run_context().input_broker(Arc::new(PendingBroker)),
    );
    let (id, _) = only_effect(run.step());
    run.resume(id, EffectAnswer::Timer);
    let Step::Done { result, .. } = run.step() else {
        panic!("the mismatch ends the run with nothing outstanding");
    };
    let RunResult::Failure(error) = result else {
        panic!("a wrong-kind answer fails the run, got {result:?}");
    };
    assert!(
        error.to_string().contains("effect's own kind"),
        "the mismatch is a loud invariant failure: {error}"
    );
}

#[test]
fn a_child_cancel_handles_cancel_is_observed_by_the_instruction_hook() {
    let parent = CancelHandle::new();
    let child = parent.child();
    let mut run = run_of(
        "local n = 0\nwhile true do n = n + 1 end",
        run_context().cancel(child),
    );
    // The loop never yields, so only the hook can end it: the parent's
    // cancel reaches the child the run holds, from another thread.
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        parent.cancel();
    });
    let step = run.step();
    canceller.join().expect("the canceller thread finishes");
    let Step::Done { result, events } = step else {
        panic!("the hook aborts the loop and nothing is outstanding, got {step:?}");
    };
    assert!(matches!(result, RunResult::Cancelled), "got {result:?}");
    assert!(ended(&events), "the end boundary rides the final step");
}

#[test]
fn a_context_without_a_host_handle_shares_its_one_flag_with_prepare_and_the_run() {
    let source = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n# Run\n\n## Only\n\n```lua\nreturn 'x'\n```\n";
    let prompt = Prompt::parse(source, "run-test", &NullObserver::default())
        .expect("the run test prompt parses");
    // The flag `prepare` hands the capabilities is the context's own.
    let (ctx, _) = crate::execute::Environment::new().prepare(&prompt, run_context());
    let capabilities_flag = ctx.cancel.clone();
    let mut run = Run::new(Arc::new(prompt), "", ctx);
    assert!(!capabilities_flag.is_cancelled());
    assert!(!run.cancel_handle().is_cancelled());
    run.cancel();
    assert!(
        capabilities_flag.is_cancelled(),
        "the run's cancel sets the flag the capabilities hold"
    );
    assert!(
        run.cancel_handle().is_cancelled(),
        "the run's own handle is the same flag"
    );
}

#[test]
fn a_run_is_decided_once_its_end_is_reported_while_done_is_withheld() {
    let mut run = run_of(
        "store.write('notes.md', 'kept')\nreturn 'unreachable'",
        run_context(),
    );
    assert!(!run.decided(), "a fresh run is undecided");
    let (id, _) = only_effect(run.step());
    assert!(!run.decided(), "a run waiting on an answer is undecided");
    run.cancel();
    assert!(
        matches!(run.step(), Step::Pending { .. }),
        "Done waits on the store effect"
    );
    assert!(
        run.decided(),
        "the run is decided before Done, so the host can drop what it holds"
    );
    run.resume(id, EffectAnswer::Dropped);
    assert!(matches!(run.step(), Step::Done { .. }));
    assert!(run.decided(), "a finished run stays decided");
}

#[test]
fn a_stillborn_run_reports_its_failure_on_the_first_step() {
    let source = "---\nname: t\ndescription: d\npromptforge: 7\n---\n\n# Run\n\n## Only\n\ndone\n";
    let prompt = Prompt::parse(source, "run-test", &NullObserver::default())
        .expect("the prompt parses whatever version it declares");
    let mut run = Run::new(Arc::new(prompt), "", run_context());
    let Step::Done { result, events } = run.step() else {
        panic!("a run that cannot start is done at once");
    };
    assert!(events.is_empty(), "nothing ran, nothing reported");
    let RunResult::Failure(error) = result else {
        panic!("an unsupported version fails, got {result:?}");
    };
    assert_eq!(error.kind(), crate::execute::RunErrorKind::Version);
}
