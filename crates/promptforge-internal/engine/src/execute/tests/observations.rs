//! Tests for the observation event sequence a run reports across its lifecycle.

use promptforge_parser::test_support::{one_section, prompt};
use promptforge_types::event::Event;

use super::models_loop::{LOOP_TO_TEXT, loop_events, loop_scenario};
use super::*;
use crate::test_support::scenario::Trace;

/// The prompt whose one section, `Only`, raises.
fn failing_prompt() -> String {
    one_section("error('expected failure')")
}

/// The prompt whose `## First` falls through to a `## Second` that raises.
fn second_section_errors() -> String {
    prompt(
        "",
        &[
            ("First", "local x = 1"),
            ("Second", "error('expected failure')"),
        ],
    )
}

/// Every offline fixture this suite pins an observation sequence for,
/// named so a diverging comparison says which one.
fn stream_fixtures() -> [(&'static str, String); 4] {
    [
        ("two sections", TWO_SECTIONS.to_owned()),
        ("store sections", STORE_SECTIONS.to_owned()),
        ("failing prompt", failing_prompt()),
        ("second section errors", second_section_errors()),
    ]
}

/// The `(section, detail)` sequence the run itself reported, as the suites'
/// observer sees it: the parse events ahead of it are left out.
fn sequence(trace: &Trace) -> Vec<(String, String)> {
    trace
        .run_records()
        .into_iter()
        .map(|record| (record.section, record.detail))
        .collect()
}

/// The `(section, kind)` trace of a raw event, read off its serialized
/// `kind` tag rather than through the recording adapter, so the comparison
/// never passes through the seam it checks.
fn event_trace(event: &Event) -> (String, String) {
    let value = serde_json::to_value(event).expect("an event serializes");
    let kind = value["kind"]
        .as_str()
        .expect("a serialized event includes its kind tag");
    (event.section().to_owned(), kind.to_owned())
}

/// An observer detail in the serialized `kind` spelling:
/// `Vfs read_numbered succeeded` is `vfs_read_numbered_succeeded`.
fn observer_kind(detail: &str) -> String {
    detail.to_ascii_lowercase().replace(' ', "_")
}

#[test]
fn the_returned_event_stream_matches_the_former_observer_sequence() {
    // The checkpoint's equivalence: for every fixture this suite pins, the
    // raw `Event` values a run returns spell, by their own `kind` tags,
    // the same `(section, detail)` sequence the recording observer sees
    // when the forwarding adapter replays them.
    for (name, md) in stream_fixtures() {
        let trace = Scenario::new(md).run();

        let streamed: Vec<(String, String)> = trace.run_events().iter().map(event_trace).collect();
        let observed: Vec<(String, String)> = sequence(&trace)
            .into_iter()
            .map(|(section, detail)| (section, observer_kind(&detail)))
            .collect();
        assert!(
            streamed
                .first()
                .is_some_and(|(_, kind)| kind == "run_started"),
            "{name}: the stream opens with the run boundary: {streamed:?}"
        );
        assert_eq!(
            streamed, observed,
            "{name}: the returned event stream and the former observer sequence agree"
        );
    }
}

#[test]
fn a_two_section_run_reports_the_exact_observation_sequence() {
    let trace = Scenario::new(TWO_SECTIONS).run();
    assert_eq!(trace.text(), "second");

    assert_eq!(
        sequence(&trace),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            ("First".to_string(), detail::SECTION_STARTED.to_string()),
            ("First".to_string(), detail::LUA_CHUNK_STARTED.to_string()),
            ("First".to_string(), detail::LUA_CHUNK_SUCCEEDED.to_string(),),
            (
                "First".to_string(),
                detail::LUA_TEARDOWN_STARTED.to_string()
            ),
            (
                "First".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("First".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Second".to_string(), detail::SECTION_STARTED.to_string()),
            ("Second".to_string(), detail::LUA_CHUNK_STARTED.to_string(),),
            (
                "Second".to_string(),
                detail::LUA_CHUNK_SUCCEEDED.to_string(),
            ),
            (
                "Second".to_string(),
                detail::LUA_TEARDOWN_STARTED.to_string(),
            ),
            (
                "Second".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Second".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Test prompt".to_string(), detail::RUN_SUCCEEDED.to_string()),
        ]
    );
}

#[test]
fn recording_and_null_observers_produce_the_same_result_and_store_state() {
    // The Engine takes no observer: a recording or null observer only
    // replays the events a run returns, after the run. What keeps the
    // choice from changing a run is that its result and store side effects
    // are fixed by its input, which two runs over fresh stores pin.
    let first_store = TestStore::new();
    let first = Scenario::new(STORE_SECTIONS).vfs(first_store.vfs()).run();
    let second_store = TestStore::new();
    let second = Scenario::new(STORE_SECTIONS).vfs(second_store.vfs()).run();

    assert_eq!(first.text(), second.text());
    assert_eq!(
        first_store.glob("**").unwrap(),
        second_store.glob("**").unwrap(),
        "two runs of one prompt must leave the same store paths"
    );
    assert_eq!(
        first_store.read("state.txt").unwrap(),
        second_store.read("state.txt").unwrap(),
        "two runs of one prompt must store the same contents"
    );

    let first_failure = Scenario::new(failing_prompt()).run();
    let second_failure = Scenario::new(failing_prompt()).run();
    assert_eq!(
        first_failure.error().to_string(),
        second_failure.error().to_string(),
        "two runs of one prompt must fail with the same error"
    );
}

#[test]
fn a_run_refused_by_the_version_gate_reports_nothing() {
    // The gate is not a run that failed; it is a run that never started, so
    // there is no RunStarted to pair a RunFinished with.
    let source = one_section("return \"ran\"").replacen("promptforge: 0", "promptforge: 2", 1);
    let trace = Scenario::new(source).run();
    assert!(matches!(trace.result(), RunResult::Failure(_)));
    let records = trace.run_records();
    assert!(
        records.is_empty(),
        "the gate must report nothing: {records:?}"
    );
}

#[test]
fn a_failing_run_still_reports_run_finished() {
    // The prologue fails, so the walk tears down its VM and the final
    // observation must report the run failure.
    let trace = Scenario::new(failing_prompt()).run();
    assert!(matches!(
        trace.error().inner(),
        Error::Lua(_) | Error::LuaRuntime { .. }
    ));

    assert_eq!(
        sequence(&trace),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            ("Only".to_string(), detail::SECTION_STARTED.to_string()),
            ("Only".to_string(), detail::LUA_CHUNK_STARTED.to_string()),
            ("Only".to_string(), detail::LUA_CHUNK_FAILED.to_string()),
            ("Only".to_string(), detail::LUA_TEARDOWN_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Test prompt".to_string(), detail::RUN_FAILED.to_string()),
        ],
        "a section that errors reports no SectionFinished"
    );
}

#[test]
fn an_erroring_section_reports_started_but_not_finished() {
    // The absence half of the section-boundary contract: a section that
    // errors mid-walk must emit SECTION_STARTED and never SECTION_FINISHED.
    let trace = Scenario::new(failing_prompt()).run();
    assert!(matches!(trace.result(), RunResult::Failure(_)));

    let observed = sequence(&trace);
    assert!(
        observed.contains(&("Only".to_string(), detail::SECTION_STARTED.to_string())),
        "the erroring section must report started: {observed:?}"
    );
    assert!(
        !observed
            .iter()
            .any(|(_, event)| event == &detail::SECTION_FINISHED.to_string()),
        "the erroring section must never report finished: {observed:?}"
    );
}

#[test]
fn an_erroring_section_tears_down_exactly_once_without_finishing() {
    // The RAII teardown contract on the error path: the frame's drop fires
    // the teardown pair exactly once, and the disarmed completion flag
    // keeps SECTION_FINISHED from firing for the erroring section.
    let trace = Scenario::new(second_section_errors()).run();
    assert!(matches!(trace.result(), RunResult::Failure(_)));

    let observed = sequence(&trace);
    for event in [
        detail::LUA_TEARDOWN_STARTED.to_string(),
        detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
    ] {
        let count = observed
            .iter()
            .filter(|(section, observed_event)| section == "Second" && observed_event == &event)
            .count();
        assert_eq!(
            count, 1,
            "the erroring section must tear down exactly once ({event}): {observed:?}"
        );
    }
    assert!(
        !observed.iter().any(|(section, event)| section == "Second"
            && event == &detail::SECTION_FINISHED.to_string()),
        "the erroring section must never report finished: {observed:?}"
    );
}

#[test]
fn a_one_byte_limit_fails_value_injection_with_teardown_observations() {
    // mlua accepts the one-byte ceiling itself, then the first Engine
    // allocation fails. Engine injection is inside the section's teardown
    // boundary, unlike the preceding bare apply_lua_limits call.
    let trace = Scenario::new(one_section("return \"ran\""))
        .context(|ctx| ctx.limits(RunLimits::new().lua_memory_bytes(std::num::NonZeroUsize::MIN)))
        .run();
    let error = trace.error();
    assert!(
        error.to_string().contains("memory"),
        "the failure must be the memory ceiling, got: {error}"
    );

    let observed = sequence(&trace);
    assert!(
        observed.contains(&("Only".to_owned(), detail::LUA_TEARDOWN_STARTED.to_string()))
            && observed.contains(&(
                "Only".to_owned(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string()
            )),
        "value injection failure must fire both teardown observations: {observed:?}"
    );
}

#[test]
fn one_execution_id_spans_parse_and_the_complete_runtime_lifecycle() {
    let gateway = ScriptedChat::new(vec![text("aliased final")]);
    let (tool, called) = scoped_tool("echo", "Echo a test value.");
    let source = "---\nname: lifecycle\ndescription: Correlated lifecycle fixture\npromptforge: 0\nplugins:\n  - tools\nmodels:\n  writer: {}\n---\n\n\
         # Lifecycle\n\n```lua\n\
         models.default('writer')\n```\n\n\
         ## Gather\n\n```lua\ntools.offer('tools/echo')\nstore.write('state.txt', 'before')\n```\n\n\
         Use the echo tool.\n\n\
         ```lua\n\
         local text = models.infer(prose)\n\
         local _ = tools.call('tools/echo', { value = 'hi' })\n\
         store.append('state.txt', '\\nafter')\n\
         return text\n\
         ```\n";
    let trace = Scenario::new(source)
        .bound()
        .tool(tool)
        .chat(&gateway)
        .run();

    assert_eq!(trace.text(), "aliased final");
    assert_eq!(trace.read("state.txt").unwrap(), "before\nafter");
    assert_eq!(called.load(Ordering::SeqCst), 1);
    let records = trace.records();
    assert!(!records.is_empty());
    assert!(
        records.iter().all(|record| record.execution == EXECUTION),
        "every lifecycle record must retain {EXECUTION}: {records:#?}"
    );
    let details = records
        .iter()
        .map(|record| record.detail.clone())
        .collect::<Vec<_>>();
    for expected in [
        detail::PARSE_STARTED,
        detail::RUN_STARTED,
        detail::SECTION_STARTED,
        detail::LUA_CHUNK_STARTED,
        detail::VFS_WRITE_SUCCEEDED,
        detail::MODEL_TURN_COMPLETED,
        detail::TOOL_CALL_SUCCEEDED,
        detail::LUA_CHUNK_STARTED,
        detail::VFS_APPEND_SUCCEEDED,
        detail::RUN_SUCCEEDED,
    ] {
        assert!(
            details.contains(&expected.to_string()),
            "the complete lifecycle must include {expected:?}: {records:#?}"
        );
    }
}

#[test]
fn the_tool_loop_reports_each_turn_and_each_tool_call() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "echo", "{\"value\":\"hi\"}")]),
        text("final answer"),
    ]);
    let source = one_section(&format!("tools.offer('tools/echo')\n{LOOP_TO_TEXT}"));
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "final answer");

    let recorder = Recorder::default();
    trace.replay(&recorder);
    assert_eq!(
        loop_events(&recorder),
        vec![
            detail::MODEL_TURN_COMPLETED.to_string(),
            detail::TOOL_CALL_SUCCEEDED.to_string(),
            detail::MODEL_TURN_COMPLETED.to_string(),
        ]
    );
    assert!(
        recorder
            .events()
            .iter()
            .filter(|(_, event)| loop_events(&recorder).contains(event))
            .all(|(section, _)| section == "Only"),
        "every loop event is reported under the section that ran the loop"
    );
}
