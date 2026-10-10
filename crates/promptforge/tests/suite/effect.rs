//! Answering every kind of effect: the order a caller answers one batch
//! in does not change the result, and every effect and answer record
//! logs as a JSON line that parses back into its record type.

use std::error::Error;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use promptforge::effect::{
    AnswerRecord, Effect, EffectAnswer, EffectRecord, ToolCallOrigin, ToolCaller,
};
use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput};
use promptforge::vfs::perform_vfs_op;
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
use serde_json::{Value, json};

/// Asks a model and a tool at once from two tasks, writes a note while
/// they are out, and waits on a third task with a short timeout.
const GREETER: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Asks a model and a tool at once, and waits.\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer: {}\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "local ask = tasks.spawn('## Ask')\n",
    "local shout = tasks.spawn('## Shout')\n",
    "store.write('note.md', 'hello')\n",
    "local results = tasks.join({ ask, shout })\n",
    "tasks.join_any({ tasks.spawn('## Quick') }, { timeout = 0.01 })\n",
    "return results[1].result .. ' / ' .. results[2].result\n",
    "```\n\n",
    "## Ask\n\n",
    "```lua\n",
    "models.use('writer')\n",
    "return models.infer('hello')\n",
    "```\n\n",
    "## Shout\n\n",
    "```lua\n",
    "return tools.call('example/text/shout', { text = 'hello' })\n",
    "```\n\n",
    "## Quick\n\n",
    "```lua\n",
    "return 'quick'\n",
    "```\n",
);

/// The greeter, prepared with one canned model and the shout tool.
fn prepared_run() -> Result<Run, Box<dyn Error>> {
    let (parsed, _parse_events) = Prompt::parse(GREETER, "greeter");
    let prompt = parsed?;
    let model = ModelDescriptor::new(
        ModelId::gateway("canned")?,
        "Always replies hi there",
        NonZeroU32::new(8_192).ok_or("a context window is never zero")?,
        ThinkingMode::Never,
    );
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);
    let shout = ToolDescriptor::new(
        ToolId::parse("example/text/shout")?,
        "Returns the text in capital letters.",
        serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
    );
    let environment = Environment::new().tools(ToolCatalog::new(&[shout])?);
    let (ctx, requirements) = environment.prepare(&prompt, ctx);
    if let Some(refusal) = requirements.refusal() {
        return Err(refusal.into());
    }
    Ok(Run::new(Arc::new(prompt), "", ctx))
}

/// Answers each effect with the answer of its own kind, sleeping out a
/// timer on this thread.
fn answer(effect: Effect) -> EffectAnswer {
    match effect {
        Effect::Chat { .. } => {
            let reply = CompletionResult::Text("hi there".to_owned());
            EffectAnswer::Chat(Completion::from_result(reply, "canned").map(Box::new))
        }
        Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("HI THERE"))),
        Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
        Effect::Timer { seconds } => {
            std::thread::sleep(Duration::try_from_secs_f64(seconds).unwrap_or_default());
            EffectAnswer::Timer
        }
    }
}

/// Drives one run to its text, answering each batch in issue order or
/// reversed.
fn drive(mut run: Run, reverse: bool) -> Result<String, Box<dyn Error>> {
    loop {
        match run.step() {
            Step::Pending { mut effects, .. } => {
                if reverse {
                    effects.reverse();
                }
                for (id, _provenance, effect) in effects {
                    run.resume(id, answer(effect));
                }
            }
            Step::Done {
                result: RunResult::Ok(text),
                ..
            } => return Ok(text),
            Step::Done { result, .. } => {
                return Err(format!("the greeter did not succeed: {result:?}").into());
            }
        }
    }
}

#[test]
fn answering_a_batch_in_reverse_gives_the_same_result_as_issue_order() -> Result<(), Box<dyn Error>>
{
    let text = drive(prepared_run()?, true)?;
    assert_eq!(text, drive(prepared_run()?, false)?);
    assert_eq!(text, "hi there / HI THERE");
    Ok(())
}

#[test]
fn a_tool_calls_origin_is_named_at_promptforge_effect() -> Result<(), Box<dyn Error>> {
    let mut run = prepared_run()?;
    let origin = loop {
        let Step::Pending { effects, .. } = run.step() else {
            return Err("the greeter calls its shout tool before it ends".into());
        };
        let mut called = None;
        for (id, _provenance, effect) in effects {
            if let Effect::ToolCall { origin, .. } = &effect {
                called = Some(origin.clone());
            }
            run.resume(id, answer(effect));
        }
        if let Some(origin) = called {
            break origin;
        }
    };
    let expected = ToolCallOrigin {
        execution: "greeter".to_owned(),
        section: "Shout".to_owned(),
        caller: ToolCaller::Script,
    };
    assert_eq!(origin, expected);
    Ok(())
}

#[test]
fn each_effect_and_its_answer_log_as_one_json_line_that_parses_back() -> Result<(), Box<dyn Error>>
{
    let mut run = prepared_run()?;
    let mut log: Vec<String> = Vec::new();
    let mut issued = 0;
    let result = loop {
        match run.step() {
            Step::Pending { effects, .. } => {
                for (id, provenance, effect) in effects {
                    issued += 1;
                    let effect_record = effect.record();
                    let reply = answer(effect);
                    let answer_record = reply.record();
                    run.resume(id, reply);
                    let line = json!({ "provenance": provenance, "effect": effect_record, "answer": answer_record });
                    log.push(line.to_string());
                }
            }
            Step::Done { result, .. } => break result,
        }
    };
    assert!(matches!(result, RunResult::Ok(_)));

    let lines: Vec<Value> = log
        .iter()
        .map(|line| serde_json::from_str(line))
        .collect::<Result<_, _>>()?;
    assert_eq!(lines.len(), issued);
    for line in &lines {
        let _: EffectRecord = serde_json::from_value(line["effect"].clone())?;
        let _: AnswerRecord = serde_json::from_value(line["answer"].clone())?;
    }

    let write = lines
        .iter()
        .find(|line| line["effect"]["Vfs"]["op"].get("Write").is_some())
        .ok_or("the greeter writes its note")?;
    assert_eq!(
        write["effect"],
        json!({ "Vfs": { "op": { "Write": { "path": "note.md", "contents": "hello" } } } })
    );
    assert_eq!(write["answer"], json!({ "Vfs": { "Ok": "Unit" } }));
    Ok(())
}
