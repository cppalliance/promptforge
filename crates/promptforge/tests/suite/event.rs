//! Logging a run's events: every event round-trips through one JSON line
//! with a unique provenance, content events pair into a transcript, and
//! debug mode alone adds the raw model request and response.

use std::collections::HashSet;

use promptforge::event::{DebugMode, Event};
use serde_json::Value;

use super::support::{Scenario, TestResult, note, prompt, shout, text};

/// Asks the `writer` role to reply to the note, and shouts the reply.
const SHOUT: &str = "
    store.write('note.md', 'hello ' .. args)
    models.use('writer')
    return tools.call('example/text/shout', { text = models.infer(store.read('note.md')) })
";

/// The shouting greeter, and its scenario: the model replies `hi there`.
fn shouting() -> (String, Scenario) {
    let source = prompt("models: { writer: {} }", &[("Greet", SHOUT)]);
    (
        source,
        Scenario::tool(shout(), [text("hi there")]).args("world"),
    )
}

#[test]
fn every_event_round_trips_as_a_json_line_and_no_two_share_a_provenance() -> TestResult {
    let source = note("'hello ' .. args");
    let trace = Scenario::default().args("world").run(&source)?;
    assert_eq!(trace.text()?, "hello world");

    for event in &trace.events {
        let line = serde_json::to_string(event)?;
        assert_eq!(&serde_json::from_str::<Event>(&line)?, event);
        let record: Value = serde_json::from_str(&line)?;
        assert!(record["kind"].is_string(), "{line} names its kind");
    }

    let keys: HashSet<_> = trace.events.iter().map(Event::provenance).collect();
    assert_eq!(keys.len(), trace.events.len());
    let quiet = Scenario::default()
        .args("world")
        .context(|ctx| ctx.provenance_start(0))
        .run(&source)?;
    assert_eq!(quiet.text()?, "hello world");
    Ok(())
}

#[test]
fn content_events_pair_each_tool_result_with_its_caller_into_a_transcript() -> TestResult {
    let (source, scenario) = shouting();
    let trace = scenario.run(&source)?;
    assert_eq!(trace.text()?, "HI THERE");

    let (mut asked, mut transcript) = (HashSet::new(), Vec::new());
    for event in &trace.events {
        match event {
            Event::AssistantToolCalls { turn, calls, .. } => {
                asked.extend(calls.iter().map(|call| (*turn, call.id.clone())));
            }
            Event::AssistantReply { text, origin, .. } => {
                transcript.push(format!("{origin:?} reply: {text}"));
            }
            Event::ToolResult {
                turn,
                tool_call_id,
                alias,
                tool,
                content,
                ..
            } => {
                let caller = if asked.contains(&(*turn, tool_call_id.clone())) {
                    "model"
                } else {
                    "script"
                };
                let tool = tool
                    .as_ref()
                    .map_or("no tool".to_owned(), ToString::to_string);
                transcript.push(format!("{alias} ({tool}) for the {caller}: {content}"));
            }
            _ => {}
        }
    }
    assert_eq!(
        transcript,
        [
            "Infer reply: hi there",
            "example_text_shout (example/text/shout) for the script: HI THERE"
        ],
        "a tool's result names its wire name and the tool it resolved to"
    );
    Ok(())
}

#[test]
fn debug_mode_alone_adds_one_request_and_one_response_per_model_round() -> TestResult {
    let (source, scenario) = shouting();
    let captured = scenario
        .context(|ctx| ctx.report_debug(DebugMode::On))
        .run(&source)?;

    let (mut requests, mut responses) = (Vec::new(), Vec::new());
    for event in &captured.events {
        match event {
            Event::Request { turn, .. } => requests.push(*turn),
            Event::Response { turn, .. } => responses.push(*turn),
            _ => {}
        }
    }
    assert_eq!(requests.len(), 1);
    assert_eq!(requests, responses);

    let quiet = shouting().1.run(&source)?;
    assert!(
        !quiet
            .events
            .iter()
            .any(|event| matches!(event, Event::Request { .. } | Event::Response { .. }))
    );
    assert_eq!(captured.text()?, "HI THERE");
    assert_eq!(quiet.text()?, "HI THERE");
    Ok(())
}
