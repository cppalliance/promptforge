//! A `Chat`, `ToolCall`, or `Timer` effect the caller drops while the
//! run's cancel flag is clear: the chain resumes with the cancelled error,
//! which a `pcall` catches so the run continues, and which ends the run
//! `Cancelled` when nothing catches it. A `Chat` drop is covered for both
//! kinds of round, a nested `models.infer` round and a `models.loop` round.
//! A `Timer` drop is covered for the timed `tasks.join_any` and
//! `tasks.join` waits and the model's `await_tasks`, and under a run cancel,
//! where the drop wakes no one.

use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge_types::event::{Event, ReplyOrigin};
use promptforge_types::ids::RoundId;
use promptforge_types::models::{ModelDescriptor, ModelId, ThinkingMode};
use promptforge_types::tools::{ToolCatalog, ToolDescriptor, ToolId};
use serde_json::json;

use super::{only_effect, run_context};
use crate::execute::Environment;
use crate::execute::RunResult;
use crate::execute::run::{
    AnswerRecord, Effect, EffectAnswer, EffectId, EffectRecord, Round, Run, Step,
};
use crate::model::{Completion, CompletionResult, ToolCall};
use crate::parser::Prompt;

/// A run over one section whose Lua is `body`, with the `writer` role
/// bound to `test-model` and the `echo` slot to `tests/tools/echo`.
fn bound_run(body: &str) -> Run {
    bound_sections(&format!(
        "## Only\n\n```lua\nmodels.use('writer')\n{body}\n```\n"
    ))
}

/// A run over `sections`, the prompt's H2 sections, with the `writer` role
/// bound to `test-model` and the `echo` slot to `tests/tools/echo`.
fn bound_sections(sections: &str) -> Run {
    let source = format!(
        "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {{}}\n\
         tools:\n  echo: tests/tools/echo\n---\n\n# Run\n\n{sections}"
    );
    let prompt = Prompt::parse(&source, "run-test")
        .0
        .expect("the drop test prompt parses");
    let model = ModelDescriptor::new(
        ModelId::gateway("test-model").expect("a valid model id"),
        "Plays the model",
        NonZeroU32::new(4096).expect("4096 is non-zero"),
        ThinkingMode::Never,
    );
    let echo = ToolDescriptor::new(
        ToolId::parse("tests/tools/echo").expect("a valid tool id"),
        "echo",
        "Echoes its value.",
        json!({ "type": "object" }),
    );
    let catalog = ToolCatalog::new(&[echo]).expect("one tool is a valid catalog");
    let (ctx, requirements) = Environment::new()
        .tools(catalog)
        .prepare(&prompt, run_context().model(model));
    assert!(
        requirements.refusal().is_none(),
        "the model and the tool fill the prompt's slots: {requirements:?}"
    );
    Run::new(Arc::new(prompt), "", ctx)
}

/// Steps `run` to its next effect and answers it `Dropped`, returning the
/// effect's record and the answer's.
fn drop_next(run: &mut Run) -> (EffectRecord, AnswerRecord) {
    let (id, effect) = only_effect(run.step());
    let answer = EffectAnswer::Dropped;
    let records = (effect.record(), answer.record());
    run.resume(id, answer);
    records
}

#[test]
fn a_dropped_chat_or_tool_call_resumes_a_pcall_with_the_cancelled_error_and_the_run_continues() {
    let mut run = bound_run(
        "local chat_ok, chat_err = pcall(models.infer, 'dropped')\n\
         local tool_ok, tool_err = pcall(tools.call, 'echo', { value = 'dropped' })\n\
         local kept = models.infer('kept')\n\
         return tostring(chat_ok) .. ':' .. chat_err.kind .. '|' \
         .. tostring(tool_ok) .. ':' .. tool_err.kind .. '|' .. kept",
    );
    let chat = drop_next(&mut run);
    assert!(
        matches!(
            &chat,
            (EffectRecord::Chat { round, .. }, AnswerRecord::Dropped) if *round == RoundId::new(0)
        ),
        "the record holds the round and its dropped answer: {chat:?}"
    );
    let tool = drop_next(&mut run);
    assert!(
        matches!(
            &tool,
            (EffectRecord::ToolCall { alias, .. }, AnswerRecord::Dropped) if alias == "echo"
        ),
        "the record holds the call and its dropped answer: {tool:?}"
    );
    assert!(
        !run.cancel_handle().is_cancelled(),
        "the drops left the run's cancel flag clear"
    );
    let (id, effect) = only_effect(run.step());
    assert!(
        matches!(&effect, Effect::Chat { round, .. } if round.id == RoundId::new(1)),
        "the run went on to its next round: {effect:?}"
    );
    let completion = Completion::from_result(CompletionResult::Text("kept".to_owned()), "m")
        .expect("a text result is accepted");
    run.resume(id, EffectAnswer::Chat(Ok(Box::new(completion))));
    let Step::Done { result, events } = run.step() else {
        panic!("the run ends once its last round is answered");
    };
    let RunResult::Ok(text) = result else {
        panic!("a caught drop leaves the run to succeed: {result:?}");
    };
    assert_eq!(text, "false:cancelled|false:cancelled|kept");
    let turns: Vec<u32> = events
        .iter()
        .filter_map(|event| match event {
            Event::AssistantReply { turn, .. } => Some(*turn),
            _ => None,
        })
        .collect();
    assert_eq!(
        turns,
        vec![1],
        "the dropped round took no turn, so the answered one is the first"
    );
}

#[test]
fn a_dropped_loop_round_resumes_a_pcall_with_the_cancelled_error_and_the_run_continues() {
    let mut run = bound_run(
        "local msgs = messages.new()\n\
         msgs:user('asked')\n\
         local loop_ok, loop_err = pcall(models.loop, msgs)\n\
         local after_drop = #msgs\n\
         models.loop(msgs)\n\
         return tostring(loop_ok) .. ':' .. loop_err.kind .. '|' .. after_drop \
         .. '|' .. #msgs .. ':' .. msgs[#msgs].content",
    );
    let (id, effect) = only_effect(run.step());
    assert!(
        matches!(
            &effect,
            Effect::Chat { round, .. }
                if *round == Round { id: RoundId::new(0), origin: ReplyOrigin::Chat }
        ),
        "the loop's first round is a chat round numbered 0: {effect:?}"
    );
    let records = (effect.record(), EffectAnswer::Dropped.record());
    run.resume(id, EffectAnswer::Dropped);
    assert!(
        matches!(
            &records,
            (EffectRecord::Chat { round, .. }, AnswerRecord::Dropped) if *round == RoundId::new(0)
        ),
        "the record holds the loop round and its dropped answer: {records:?}"
    );
    assert!(
        !run.cancel_handle().is_cancelled(),
        "the drop left the run's cancel flag clear"
    );
    let (id, effect) = only_effect(run.step());
    assert!(
        matches!(
            &effect,
            Effect::Chat { round, .. }
                if *round == Round { id: RoundId::new(1), origin: ReplyOrigin::Chat }
        ),
        "the run went on to the next loop's round: {effect:?}"
    );
    let completion = Completion::from_result(CompletionResult::Text("kept".to_owned()), "m")
        .expect("a text result is accepted");
    run.resume(id, EffectAnswer::Chat(Ok(Box::new(completion))));
    let Step::Done { result, events } = run.step() else {
        panic!("the run ends once its last round is answered");
    };
    let RunResult::Ok(text) = result else {
        panic!("a caught drop leaves the run to succeed: {result:?}");
    };
    assert_eq!(
        text, "false:cancelled|1|2:kept",
        "the dropped round appended nothing, and the answered one appended its reply"
    );
    let turns: Vec<u32> = events
        .iter()
        .filter_map(|event| match event {
            Event::AssistantReply { turn, .. } => Some(*turn),
            _ => None,
        })
        .collect();
    assert_eq!(
        turns,
        vec![1],
        "the dropped round took no turn, so the answered one is the first"
    );
}

#[test]
fn an_uncaught_dropped_chat_or_tool_call_ends_the_run_cancelled() {
    for body in [
        "return models.infer('dropped')",
        "local msgs = messages.new()\nmsgs:user('dropped')\nreturn models.loop(msgs)",
        "return tools.call('echo', { value = 'dropped' })",
    ] {
        let mut run = bound_run(body);
        let _ = drop_next(&mut run);
        let Step::Done { result, .. } = run.step() else {
            panic!("{body}: the uncaught drop ends the run with nothing outstanding");
        };
        assert!(
            matches!(result, RunResult::Cancelled),
            "{body}: the cancelled error ends the run cancelled: {result:?}"
        );
    }
}

/// A run whose `## Main` runs `main` beside the `## Child` it spawns, whose
/// one round stays out until the test answers it.
fn timed_run(main: &str) -> Run {
    bound_sections(&format!(
        "## Main\n\n```lua\nmodels.use('writer')\n{main}\n```\n\n\
         ## Child\n\n```lua\nmodels.use('writer')\nreturn models.infer('child')\n```\n"
    ))
}

/// A main that catches its timed `wait` over the child, then joins the
/// child untimed and reports both.
fn caught_wait(wait: &str) -> String {
    format!(
        "local t = tasks.spawn('## Child')\n\
         local ok, err = pcall({wait}, {{ t }}, {{ timeout = 30 }})\n\
         local _t, _ok, result = tasks.join_any({{ t }})\n\
         return tostring(ok) .. ':' .. err.kind .. '|' .. result"
    )
}

/// Steps `run` to the main's parked timed wait and returns the wait's
/// timer and the child's round.
fn parked_wait(run: &mut Run) -> (EffectId, EffectId) {
    let Step::Pending { effects, .. } = run.step() else {
        panic!("the main parks on its timed wait while the child's round is out");
    };
    let issued = |kind: fn(&Effect) -> bool| {
        let Some((id, _, _)) = effects.iter().find(|(_, _, effect)| kind(effect)) else {
            panic!("the step issued the timer and the round: {effects:?}");
        };
        *id
    };
    let timer = issued(|effect| matches!(effect, Effect::Timer { .. }));
    let chat = issued(|effect| matches!(effect, Effect::Chat { .. }));
    assert_eq!(
        effects.len(),
        2,
        "only the timer and the round: {effects:?}"
    );
    (timer, chat)
}

/// A text completion of `text`.
fn text(text: &str) -> EffectAnswer {
    let completion = Completion::from_result(CompletionResult::Text(text.to_owned()), "m")
        .expect("a text result is accepted");
    EffectAnswer::Chat(Ok(Box::new(completion)))
}

#[test]
fn a_dropped_timer_raises_the_cancelled_error_into_a_timed_waits_pcall_and_the_run_continues() {
    for wait in ["tasks.join_any", "tasks.join"] {
        let mut run = timed_run(&caught_wait(wait));
        let (timer, chat) = parked_wait(&mut run);
        run.resume(timer, EffectAnswer::Dropped);
        assert!(
            !run.cancel_handle().is_cancelled(),
            "{wait}: the drop left the run's cancel flag clear"
        );
        let Step::Pending { effects, .. } = run.step() else {
            panic!("{wait}: the main goes on to wait on its child untimed");
        };
        assert!(
            effects.is_empty(),
            "{wait}: the caught drop issued nothing new: {effects:?}"
        );
        run.resume(chat, text("child"));
        let Step::Done { result, .. } = run.step() else {
            panic!("{wait}: the run ends once the child's round is answered");
        };
        let RunResult::Ok(out) = result else {
            panic!("{wait}: a caught drop leaves the run to succeed: {result:?}");
        };
        assert_eq!(
            out, "false:cancelled|child",
            "{wait}: the wait raised the cancelled error and the child kept running"
        );
    }
}

#[test]
fn an_uncaught_dropped_timer_ends_the_run_cancelled() {
    for wait in ["tasks.join_any", "tasks.join"] {
        let mut run = timed_run(&format!(
            "local t = tasks.spawn('## Child')\n\
             {wait}({{ t }}, {{ timeout = 30 }})\n\
             return 'waited'"
        ));
        let (timer, chat) = parked_wait(&mut run);
        run.resume(timer, EffectAnswer::Dropped);
        let Step::Pending { effects, .. } = run.step() else {
            panic!("{wait}: the child's round is still out, so the run is not done");
        };
        assert!(effects.is_empty(), "{wait}: nothing new: {effects:?}");
        assert!(
            run.decided(),
            "{wait}: the uncaught error decided the run; the child's round is an orphan"
        );
        run.resume(chat, EffectAnswer::Dropped);
        let Step::Done { result, .. } = run.step() else {
            panic!("{wait}: the run ends once the orphan is answered");
        };
        assert!(
            matches!(result, RunResult::Cancelled),
            "{wait}: the cancelled error ends the run cancelled: {result:?}"
        );
    }
}

#[test]
fn under_a_run_cancel_a_dropped_timer_ends_the_run_cancelled_without_resuming_the_wait() {
    let mut run = timed_run(&caught_wait("tasks.join_any"));
    let (timer, chat) = parked_wait(&mut run);
    run.cancel();
    run.resume(timer, EffectAnswer::Dropped);
    run.resume(chat, EffectAnswer::Dropped);
    let Step::Done { result, .. } = run.step() else {
        panic!("the cancel ends the run once both effects are answered");
    };
    assert!(
        matches!(result, RunResult::Cancelled),
        "the wait's pcall never resumed, so the run did not go on: {result:?}"
    );
}

#[test]
fn a_dropped_await_tasks_timer_resumes_the_models_tool_call_with_the_cancelled_error() {
    let mut run = bound_sections(
        "## Only\n\n```lua\nmodels.use('writer')\ntools.allow_tasks()\n\
         local msgs = messages.new()\nmsgs:user('wait')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         return tostring(ok) .. ':' .. err.kind\n```\n",
    );
    let (round, effect) = only_effect(run.step());
    assert!(
        matches!(effect, Effect::Chat { .. }),
        "the loop's round: {effect:?}"
    );
    let call = ToolCall::from_parts("call-1", "await_tasks", json!({ "timeout": 30 }))
        .expect("a whole call");
    let completion = Completion::from_result(CompletionResult::ToolCalls(vec![call]), "m")
        .expect("a tool-call result is accepted");
    run.resume(round, EffectAnswer::Chat(Ok(Box::new(completion))));
    let (timer, effect) = only_effect(run.step());
    assert!(
        matches!(effect, Effect::Timer { .. }),
        "await_tasks parks on its timeout alone: {effect:?}"
    );
    run.resume(timer, EffectAnswer::Dropped);
    let Step::Done { result, .. } = run.step() else {
        panic!("the loop's caught drop ends the run");
    };
    let RunResult::Ok(out) = result else {
        panic!("a caught drop leaves the run to succeed: {result:?}");
    };
    assert_eq!(
        out, "false:cancelled",
        "the loop raised the cancelled error"
    );
}
