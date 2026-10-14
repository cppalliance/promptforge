//! Answering every kind of effect: the order a caller answers one batch
//! in does not change the result, and every effect and answer record
//! logs as a JSON line that parses back into its record type.

use promptforge::RunResult;
use promptforge::effect::{AnswerRecord, EffectRecord, ToolCallOrigin, ToolCaller};
use serde_json::{Value, json};

use super::support::{Scenario, TestResult, prompt, shout, text};

/// Asks a model and a tool at once from two tasks, writes a note while
/// they are out, and waits on a third task with a short timeout.
const GREET: &str = "
    local ask = tasks.spawn('## Ask')
    local shout = tasks.spawn('## Shout')
    store.write('note.md', 'hello')
    local results = tasks.join({ ask, shout })
    tasks.join_any({ tasks.spawn('## Quick') }, { timeout = 0.01 })
    return results[1].result .. ' / ' .. results[2].result
";

/// The greeter's four sections, with the model role they ask.
fn greeter() -> String {
    let ask = "models.use('writer')\nreturn models.infer('hello')";
    let shout = "return tools.call('example/text/shout', { text = 'hi there' })";
    let sections = [
        ("Greet", GREET),
        ("Ask", ask),
        ("Shout", shout),
        ("Quick", "return 'quick'"),
    ];
    prompt("models: { writer: {} }", &sections)
}

/// The greeter's scenario: the model replies `hi there`, and the shout
/// tool is registered.
fn scenario() -> Scenario {
    Scenario::tool(shout(), [text("hi there")])
}

#[test]
fn answering_a_batch_in_reverse_gives_the_same_result_as_issue_order() -> TestResult {
    let reversed = scenario().reverse().run(&greeter())?;
    let in_order = scenario().run(&greeter())?;
    let answered_reversed: Vec<_> = reversed
        .effects
        .iter()
        .map(|logged| &logged.provenance)
        .collect();
    let answered_in_order: Vec<_> = in_order
        .effects
        .iter()
        .map(|logged| &logged.provenance)
        .collect();
    assert_ne!(
        answered_reversed, answered_in_order,
        "reversing a batch must change the order its effects are answered in"
    );
    let (RunResult::Ok(reversed), RunResult::Ok(in_order)) = (reversed.result, in_order.result)
    else {
        return Err("the greeter did not succeed".into());
    };
    assert_eq!(reversed, in_order);
    assert_eq!(reversed, "hi there / HI THERE");
    Ok(())
}

#[test]
fn a_tool_calls_origin_is_named_at_promptforge_effect() -> TestResult {
    let trace = scenario().run(&greeter())?;
    let origin = trace
        .effects
        .iter()
        .find_map(|logged| match &logged.effect {
            EffectRecord::ToolCall { origin, .. } => Some(origin.clone()),
            _ => None,
        })
        .ok_or("the greeter calls its shout tool before it ends")?;
    let expected = ToolCallOrigin {
        execution: "greeter".to_owned(),
        section: "Shout".to_owned(),
        caller: ToolCaller::Script,
    };
    assert_eq!(origin, expected);
    Ok(())
}

#[test]
fn each_effect_and_its_answer_log_as_one_json_line_that_parses_back() -> TestResult {
    let trace = scenario().run(&greeter())?;
    assert!(matches!(trace.result, RunResult::Ok(_)));

    let log: Vec<String> = trace
        .effects
        .iter()
        .map(|logged| {
            let line = json!({
                "provenance": logged.provenance,
                "effect": logged.effect,
                "answer": logged.answer,
            });
            line.to_string()
        })
        .collect();
    let lines: Vec<Value> = log
        .iter()
        .map(|line| serde_json::from_str(line))
        .collect::<Result<_, _>>()?;
    assert_eq!(lines.len(), trace.effects.len());
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
