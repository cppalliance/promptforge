//! A stop and a cancel reaching the effects in flight: `stop_round` drops
//! a chat round, a tool call, and a timer and leaves the run's cancel
//! flag clear, so a `pcall` keeps the run alive while an uncaught drop
//! ends it `Cancelled`; a prompt shaped like `chat` returns to its
//! question after a stop during a tool call; and a stop leaves a question
//! to the operator, or any call whose descriptor survives stops, open,
//! while a cancel drops it.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use harness_plugins::{HostServices, USER_INPUT_ASK_TOOL, UserInput};
use harness_runner::Harness;
use harness_runner::recorder::MemoryRecorder;
use promptforge::model::{Completion, CompletionResult, ToolCall};
use serde_json::json;

use super::{
    PendingTimer, RunOutcome, answers_to, answers_where, calls, request, run_beside, until,
};
use crate::scripted::{
    Held, HeldTimer, MODEL, Operator, ScriptedBroker, held_broker, hold_registry, reply,
    surviving_hold_registry,
};

/// A main section parked under a `pcall` on a 30-second timed wait over
/// two children: one parked on a model round, the other on a held tool
/// call, each under a `pcall` that reports how the call ended. The run
/// then joins both children and returns how its own wait ended beside
/// both children's reports.
const STOPS_ALL: &str = "---\nname: stops\ndescription: d\npromptforge: 0\n\
    plugins:\n  - harness\nmodels:\n  writer: {}\n---\n\n\
    # Stops\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Main\n\n```lua\n\
    local chat = tasks.spawn('## Chat')\n\
    local tool = tasks.spawn('## Tool')\n\
    local ok, err = pcall(tasks.join_any, { chat, tool }, { timeout = 30 })\n\
    local results = tasks.join({ chat, tool })\n\
    return tostring(ok) .. ':' .. err.kind .. '|' .. results[1].result .. '|' .. results[2].result\n```\n\n\
    ## Chat\n\n```lua\n\
    local ok, err = pcall(models.infer, 'held')\n\
    return 'chat:' .. tostring(ok) .. ':' .. err.kind\n```\n\n\
    ## Tool\n\n```lua\n\
    local ok, err = pcall(tools.call, 'harness/hold')\n\
    return 'tool:' .. tostring(ok) .. ':' .. err.kind\n```\n";

/// A prompt whose one round sits under nothing that catches.
const STOPS_UNCAUGHT: &str = "---\nname: uncaught\ndescription: d\npromptforge: 0\n\
    models:\n  writer: {}\n---\n\n\
    # Uncaught\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Only\n\n```lua\nreturn models.infer('held')\n```\n";

/// The built-in chat's shape: ask the operator, run the model loop under
/// a `pcall`, and ask again, with the held tool advertised to the model.
const CHATS: &str = "---\nname: chats\ndescription: d\npromptforge: 0\n\
    plugins:\n  - harness\n  - user-input\n\
    tools:\n  hold: harness/hold\nmodels:\n  writer: {}\n---\n\n\
    # Chats\n\n```lua\nmodels.default('writer')\ntools.always('hold')\n```\n\n\
    ## Conversation\n\n```lua\n\
    local history = messages.new()\n\
    while true do\n\
    \x20   local text = input.ask()\n\
    \x20   history:user(text)\n\
    \x20   pcall(function() return models.loop(history) end)\n\
    end\n```\n";

/// A prompt that returns the operator's answer to one question.
const ASKS: &str = "---\nname: asks\ndescription: d\npromptforge: 0\n\
    plugins:\n  - user-input\n---\n\n\
    # Asks\n\n## Only\n\n```lua\nreturn (input.ask())\n```\n";

/// A prompt that returns what the held tool answers.
const HOLDS: &str = "---\nname: holds\ndescription: d\npromptforge: 0\n\
    plugins:\n  - harness\n---\n\n\
    # Holds\n\n## Only\n\n```lua\nreturn tools.call('harness/hold')\n```\n";

#[tokio::test]
async fn a_stop_drops_a_chat_round_a_tool_call_and_a_timer_and_a_pcall_keeps_the_run_alive() {
    let chat = Arc::new(Held::default());
    let tool = Arc::new(Held::default());
    let timer = Arc::new(Held::default());
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = Harness::new(
        recorder.clone(),
        Arc::new(held_broker(&chat)),
        Arc::new(HeldTimer(Arc::clone(&timer))),
        hold_registry(&tool),
        HostServices::new(),
    );
    let in_flight = [Arc::clone(&chat), Arc::clone(&tool), Arc::clone(&timer)];

    let report = run_beside(harness, request(STOPS_ALL), |control| async move {
        until(
            "a chat round, a tool call, and a timer are all in flight",
            || in_flight.iter().all(|held| held.started() == 1),
        )
        .await;
        control.stop_round();
    })
    .await
    .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "false:cancelled|chat:false:cancelled|tool:false:cancelled".to_owned()
        },
        "the main's pcall caught its interrupted wait, each child's pcall caught its dropped \
         call, and the run went on to its end"
    );
    for (held, what) in [(&chat, "round"), (&tool, "tool call"), (&timer, "timer")] {
        assert_eq!(held.started(), 1, "the {what} started once");
        assert_eq!(held.dropped(), 1, "the stopped {what} was torn down");
    }
    let records = recorder.records(report.run_id.expect("the run began"));
    for kind in ["Chat", "ToolCall", "Timer"] {
        assert_eq!(
            answers_to(&records, kind),
            [json!("Dropped")],
            "the stop answered the {kind} effect Dropped"
        );
    }
}

#[tokio::test]
async fn an_uncaught_stopped_round_ends_the_run_cancelled() {
    let chat = Arc::new(Held::default());
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = Harness::new(
        recorder.clone(),
        Arc::new(held_broker(&chat)),
        Arc::new(PendingTimer::default()),
        hold_registry(&Arc::default()),
        HostServices::new(),
    );
    let watched = Arc::clone(&chat);

    let report = run_beside(harness, request(STOPS_UNCAUGHT), |control| async move {
        until("the round is in flight", || watched.started() == 1).await;
        control.stop_round();
    })
    .await
    .expect("the run reaches an outcome");

    assert_eq!(report.outcome, RunOutcome::Cancelled);
    let run_id = report.run_id.expect("the run began");
    assert_eq!(recorder.outcome(run_id), Some(RunOutcome::Cancelled));
    assert_eq!(
        answers_to(&recorder.records(run_id), "Chat"),
        [json!("Dropped")]
    );
}

/// The round that asks for the held tool: the model's first round calls
/// `hold`, and every later round replies with text.
fn calls_hold_once() -> ScriptedBroker {
    ScriptedBroker::new(|round, _messages| {
        Box::pin(async move {
            if round.id.get() > 0 {
                return reply("done");
            }
            let call = ToolCall::from_parts("call-1", "hold", json!({}))?;
            Completion::from_result(CompletionResult::ToolCalls(vec![call]), MODEL).map(Box::new)
        })
    })
}

#[tokio::test]
async fn a_chat_shaped_prompt_returns_to_its_question_after_a_stop_during_a_tool_call() {
    let tool = Arc::new(Held::default());
    let (operator, answers) = Operator::new();
    let asked = Arc::clone(&operator.asked);
    let mut plugins = hold_registry(&tool);
    plugins.register(Arc::new(UserInput::new())).unwrap();
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = Harness::new(
        recorder.clone(),
        Arc::new(calls_hold_once()),
        Arc::new(PendingTimer::default()),
        plugins,
        operator.services(),
    );
    let watched = Arc::clone(&tool);

    let report = run_beside(harness, request(CHATS), |control| async move {
        until("the prompt asks its first question", || {
            asked.load(Ordering::SeqCst) == 1
        })
        .await;
        answers.send("hello".to_owned()).unwrap();
        until("the model's tool call is in flight", || {
            watched.started() == 1
        })
        .await;
        control.stop_round();
        until("the prompt returns to its question", || {
            asked.load(Ordering::SeqCst) == 2
        })
        .await;
        control.cancel();
    })
    .await
    .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Cancelled,
        "the cancel ended the run waiting on its second question"
    );
    assert_eq!(tool.dropped(), 1, "the stopped tool call was torn down");
    assert_eq!(
        operator.abandoned.load(Ordering::SeqCst),
        1,
        "the cancel tore down the second question"
    );
    let records = recorder.records(report.run_id.expect("the run began"));
    assert_eq!(
        answers_where(&records, "ToolCall", calls("harness/hold")),
        [json!("Dropped")],
        "the stop answered the model's tool call Dropped"
    );
    let asks = answers_where(&records, "ToolCall", calls(USER_INPUT_ASK_TOOL));
    assert_eq!(asks.len(), 2, "two questions: {asks:?}");
    assert_eq!(asks[0]["ToolCall"]["Ok"]["text"], "hello");
    assert_eq!(
        asks[1],
        json!("Dropped"),
        "the cancel answered the second question Dropped"
    );
}

/// A Harness on `operator` with the user-input Plugin and a broker no
/// round reaches.
fn asking_harness(recorder: &Arc<MemoryRecorder>, operator: &Arc<Operator>) -> Harness {
    let mut plugins = hold_registry(&Arc::default());
    plugins.register(Arc::new(UserInput::new())).unwrap();
    Harness::new(
        recorder.clone(),
        Arc::new(ScriptedBroker::replying()),
        Arc::new(PendingTimer::default()),
        plugins,
        operator.services(),
    )
}

#[tokio::test]
async fn a_stop_leaves_a_question_to_the_operator_open() {
    let (operator, answers) = Operator::new();
    let asked = Arc::clone(&operator.asked);
    let abandoned = Arc::clone(&operator.abandoned);
    let recorder = Arc::new(MemoryRecorder::new());

    let report = run_beside(
        asking_harness(&recorder, &operator),
        request(ASKS),
        |control| async move {
            until("the prompt asks its question", || {
                asked.load(Ordering::SeqCst) == 1
            })
            .await;
            control.stop_round();
            for _ in 0..16 {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                abandoned.load(Ordering::SeqCst),
                0,
                "the stop left the question open"
            );
            answers.send("kept".to_owned()).unwrap();
        },
    )
    .await
    .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "kept".to_owned()
        },
        "the question survived the stop and its answer ended the run"
    );
    let records = recorder.records(report.run_id.expect("the run began"));
    assert_eq!(answers_to(&records, "ToolCall").len(), 1);
    assert_ne!(answers_to(&records, "ToolCall")[0], json!("Dropped"));
}

#[tokio::test]
async fn a_stop_leaves_any_tool_call_whose_descriptor_survives_stops_in_flight() {
    let tool = Arc::new(Held::default());
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = Harness::new(
        recorder.clone(),
        Arc::new(ScriptedBroker::replying()),
        Arc::new(PendingTimer::default()),
        surviving_hold_registry(&tool),
        HostServices::new(),
    );
    let watched = Arc::clone(&tool);

    let report = run_beside(harness, request(HOLDS), |control| async move {
        until("the held call is in flight", || watched.started() == 1).await;
        control.stop_round();
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            watched.dropped(),
            0,
            "the stop left the surviving call in flight"
        );
        control.cancel();
    })
    .await
    .expect("the run reaches an outcome");

    assert_eq!(report.outcome, RunOutcome::Cancelled);
    assert_eq!(tool.dropped(), 1, "the cancel tore the surviving call down");
}

#[tokio::test]
async fn a_cancel_drops_a_question_to_the_operator() {
    let (operator, _answers) = Operator::new();
    let asked = Arc::clone(&operator.asked);
    let recorder = Arc::new(MemoryRecorder::new());

    let report = run_beside(
        asking_harness(&recorder, &operator),
        request(ASKS),
        |control| async move {
            until("the prompt asks its question", || {
                asked.load(Ordering::SeqCst) == 1
            })
            .await;
            control.cancel();
        },
    )
    .await
    .expect("the run reaches an outcome");

    assert_eq!(report.outcome, RunOutcome::Cancelled);
    assert_eq!(
        operator.abandoned.load(Ordering::SeqCst),
        1,
        "the cancel tore the question down"
    );
    let records = recorder.records(report.run_id.expect("the run began"));
    assert_eq!(answers_to(&records, "ToolCall"), [json!("Dropped")]);
}
