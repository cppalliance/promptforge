//! What a stop reaches depends on when the loop sees it: a stop
//! interrupts a timed `tasks.join_any` while the task it joins keeps
//! waiting; a stop seen before a step's effects start drops the round in
//! flight and the run steps its chain on; a stop with only a question to
//! the operator open spares the round after its answer; and a stop raised
//! with nothing in flight changes nothing.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use harness_runner::recorder::MemoryRecorder;
use harness_runner::{Harness, HostContext, RunRequest};
use promptforge::vfs::VfsRef;
use promptforge_plugin::HostServices;
use serde_json::json;

use super::{
    HookedBackend, PendingTimer, RunOutcome, answers_to, answers_where, calls, plain_harness,
    request, run_beside, until,
};
use crate::scripted::{Held, HeldTimer, Operator, ScriptedBroker, held_broker, with_asker};

/// A main section parked under a `pcall` on a 30-second timed
/// `tasks.join_any` over a child that asks the operator. Once the wait
/// ends, the main infers once, joins the child for the operator's answer,
/// and returns how the wait ended, the reply, and the answer.
const STOPS_TIMED_JOIN: &str = "---\nname: timed\ndescription: d\npromptforge: 0\n\
    plugins:\n  - user-input\nmodels:\n  writer: {}\n---\n\n\
    # Timed\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Main\n\n```lua\n\
    local asker = tasks.spawn('## Asker')\n\
    local ok, err = pcall(tasks.join_any, { asker }, { timeout = 30 })\n\
    local after = models.infer('after')\n\
    local _t, _ok, answer = tasks.join_any({ asker })\n\
    return tostring(ok) .. ':' .. err.kind .. '|' .. after .. '|' .. answer\n```\n\n\
    ## Asker\n\n```lua\nreturn (input.ask())\n```\n";

/// A main section that spawns a child parked under a `pcall` on a model
/// round, writes a store file, and then only waits on the child, returning
/// the child's report of how its round ended.
const STOPS_BEFORE_A_STEP: &str = "---\nname: before-step\ndescription: d\npromptforge: 0\n\
    models:\n  writer: {}\n---\n\n\
    # BeforeStep\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Main\n\n```lua\n\
    local child = tasks.spawn('## Child')\n\
    store.write('note.md', 'kept')\n\
    local _t, _ok, report = tasks.join_any({ child })\n\
    return report\n```\n\n\
    ## Child\n\n```lua\n\
    local ok, err = pcall(models.infer, 'held')\n\
    return 'chat:' .. tostring(ok) .. ':' .. err.kind\n```\n";

/// A prompt that hands the operator's answer to one model round.
const ASKS_THEN_INFERS: &str = "---\nname: asks-infers\ndescription: d\npromptforge: 0\n\
    plugins:\n  - user-input\nmodels:\n  writer: {}\n---\n\n\
    # AsksInfers\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Only\n\n```lua\nreturn models.infer((input.ask()))\n```\n";

/// A prompt that writes a store file, then runs one model round.
const WRITES_THEN_INFERS: &str = "---\nname: writes-infers\ndescription: d\npromptforge: 0\n\
    models:\n  writer: {}\n---\n\n\
    # WritesInfers\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Only\n\n```lua\nstore.write('note.md', 'kept')\nreturn models.infer('after')\n```\n";

/// A Host holding only the fixture ask Plugin, as `user-input`.
fn asker_host() -> Arc<HostContext> {
    Arc::new(with_asker(HostContext::new(HostServices::new())))
}

/// A store whose every write raises a stop through `harness`'s control.
fn stopping_store(harness: &Harness) -> VfsRef {
    let control = harness.control();
    VfsRef::builder()
        .store("/", HookedBackend::new(move || control.stop_round()))
        .build()
}

#[tokio::test]
async fn a_stop_interrupts_a_timed_join_any_and_its_pcall_resumes_while_the_joined_task_waits() {
    let (operator, answers) = Operator::new();
    let asked = Arc::clone(&operator.asked);
    let timer = Arc::new(Held::default());
    let broker = ScriptedBroker::replying();
    let rounds = broker.rounds();
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = Harness::new(
        recorder.clone(),
        Arc::new(broker),
        Arc::new(HeldTimer(Arc::clone(&timer))),
        asker_host(),
        operator.services(),
    );
    let watched = Arc::clone(&timer);

    let report = run_beside(harness, request(STOPS_TIMED_JOIN), |control| async move {
        until(
            "the child's question and the wait's timer are in flight",
            || asked.load(Ordering::SeqCst) == 1 && watched.started() == 1,
        )
        .await;
        control.stop_round();
        until(
            "the interrupted wait's pcall resumes and the main infers",
            || rounds.lock().unwrap().len() == 1,
        )
        .await;
        answers.send("kept".to_owned()).unwrap();
    })
    .await
    .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "false:cancelled|re: after|kept".to_owned()
        },
        "the wait raised the cancelled error, and the child's question outlived the stop"
    );
    assert_eq!(timer.dropped(), 1, "the stopped timer was torn down");
    assert_eq!(
        operator.abandoned.load(Ordering::SeqCst),
        0,
        "the stop left the child's question open"
    );
    let records = recorder.records(report.run_id.expect("the run began"));
    assert_eq!(answers_to(&records, "Timer"), [json!("Dropped")]);
    let asks = answers_where(&records, "ToolCall", calls("user-input/ask"));
    assert_eq!(asks.len(), 1, "one question: {asks:?}");
    assert_eq!(asks[0]["ToolCall"]["Ok"]["text"], "kept");
}

#[tokio::test]
async fn a_stop_seen_before_a_steps_effects_start_drops_the_round_in_flight_and_its_pcall_resumes()
{
    let chat = Arc::new(Held::default());
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = Harness::new(
        recorder.clone(),
        Arc::new(held_broker(&chat)),
        Arc::new(PendingTimer::default()),
        Arc::new(HostContext::new(HostServices::new())),
        HostServices::new(),
    );
    let vfs = stopping_store(&harness);

    // The write and the child's round are issued in one step, and the
    // write runs inline and raises the stop. The loop sees the stop before
    // the next step's effects start, and that step issues none: the main
    // only waits on the child, whose dropped round is all that can wake it.
    let report = run_beside(
        harness,
        RunRequest {
            vfs,
            ..request(STOPS_BEFORE_A_STEP)
        },
        |_control| async {},
    )
    .await
    .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "chat:false:cancelled".to_owned()
        },
        "the child's pcall caught its dropped round, and the main joined the child"
    );
    let records = recorder.records(report.run_id.expect("the run began"));
    assert_eq!(
        answers_to(&records, "Chat"),
        [json!("Dropped")],
        "the stop answered the round in flight Dropped"
    );
}

#[tokio::test]
async fn a_stop_with_only_a_question_open_spares_the_round_after_its_answer() {
    let (operator, answers) = Operator::new();
    let asked = Arc::clone(&operator.asked);
    let broker = ScriptedBroker::replying();
    let rounds = broker.rounds();
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = Harness::new(
        recorder.clone(),
        Arc::new(broker),
        Arc::new(PendingTimer::default()),
        asker_host(),
        operator.services(),
    );

    let report = run_beside(harness, request(ASKS_THEN_INFERS), |control| async move {
        until("the prompt asks its question", || {
            asked.load(Ordering::SeqCst) == 1
        })
        .await;
        control.stop_round();
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        answers.send("kept".to_owned()).unwrap();
    })
    .await
    .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "re: kept".to_owned()
        },
        "the round after the answer completed"
    );
    assert_eq!(
        rounds.lock().unwrap().len(),
        1,
        "the round reached the broker"
    );
    assert_eq!(operator.abandoned.load(Ordering::SeqCst), 0);
    let records = recorder.records(report.run_id.expect("the run began"));
    let chats = answers_to(&records, "Chat");
    assert_eq!(chats.len(), 1, "one round: {chats:?}");
    assert_ne!(
        chats[0],
        json!("Dropped"),
        "the stop never reached the round"
    );
}

#[tokio::test]
async fn a_stop_raised_with_nothing_in_flight_changes_nothing_for_the_next_effect() {
    let broker = ScriptedBroker::replying();
    let rounds = broker.rounds();
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = plain_harness(&recorder, broker);
    // The write runs inline, so nothing is in flight when it raises the stop.
    let vfs = stopping_store(&harness);

    let report = harness
        .run(RunRequest {
            vfs,
            ..request(WRITES_THEN_INFERS)
        })
        .await
        .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "re: after".to_owned()
        },
        "the round after the stop completed"
    );
    assert_eq!(
        rounds.lock().unwrap().len(),
        1,
        "the round reached the broker"
    );
    let records = recorder.records(report.run_id.expect("the run began"));
    let chats = answers_to(&records, "Chat");
    assert_eq!(chats.len(), 1, "one round: {chats:?}");
    assert_ne!(
        chats[0],
        json!("Dropped"),
        "the stop never reached the round"
    );
}
