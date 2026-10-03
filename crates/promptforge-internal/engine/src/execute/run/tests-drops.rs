//! A `Chat` or `ToolCall` effect the Harness drops while the run's cancel
//! flag is clear: the chain resumes with the cancelled error, which a
//! `pcall` catches so the run continues, and which ends the run
//! `Cancelled` when nothing catches it. A `Chat` drop is covered for both
//! kinds of round, a nested `models.infer` round and a `models.loop` round.

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
use crate::execute::run::{AnswerRecord, Effect, EffectAnswer, EffectRecord, Round, Run, Step};
use crate::model::{Completion, CompletionResult};
use crate::parser::Prompt;

/// A run over one section whose Lua is `body`, with the `writer` role
/// bound to `test-model` and the `echo` slot to `tests/tools/echo`.
fn bound_run(body: &str) -> Run {
    let source = format!(
        "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {{}}\n\
         tools:\n  echo: tests/tools/echo\n---\n\n# Run\n\n## Only\n\n\
         ```lua\nmodels.use('writer')\n{body}\n```\n"
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
