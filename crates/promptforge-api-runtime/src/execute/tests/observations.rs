//! Tests for the observation event sequence a run reports across its lifecycle.

use promptforge_api_types::event::Event;

use super::run;
use super::serial_driver::perform_locally;
use super::*;
use crate::execute::run::Run;
use crate::test_support::drive;

const FAILING_PROMPT: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\n```lua\nerror('expected failure')\n```\n";

const SECOND_SECTION_ERRORS: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nerror('expected failure')\n```\n";

/// Every offline fixture this suite pins an observation sequence for,
/// named so a diverging comparison says which one.
const STREAM_FIXTURES: [(&str, &str); 4] = [
    ("two sections", TWO_SECTIONS),
    ("store sections", STORE_SECTIONS),
    ("failing prompt", FAILING_PROMPT),
    ("second section errors", SECOND_SECTION_ERRORS),
];

/// Drives `md` on the serial driver - no runtime, no observer, no
/// forwarding adapter - and returns the raw outcome and event stream.
fn drive_serially(md: &str) -> (RunResult, Vec<Event>) {
    let prompt = parse(md);
    let env = Environment::new();
    let ctx = test_context(EXECUTION).vfs(env.run_vfs());
    let (ctx, requirements) = env.prepare(&prompt, ctx);
    assert!(
        requirements.refusal().is_none(),
        "the fixtures declare nothing prepare could refuse"
    );
    drive(Run::new(Arc::new(prompt), "", ctx), |_, effect| {
        perform_locally(effect, &mut |effect| {
            panic!("the fixtures issue no model round: {effect:?}")
        })
    })
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
/// `Store read_numbered succeeded` is `store_read_numbered_succeeded`.
fn observer_kind(detail: &str) -> String {
    detail.to_ascii_lowercase().replace(' ', "_")
}

#[tokio::test]
async fn the_returned_event_stream_matches_the_former_observer_sequence() {
    // The checkpoint's equivalence: for every fixture this suite pins, the
    // raw `Event` values a serial drive returns spell, by their own `kind`
    // tags, the same `(section, detail)` sequence the recording observer
    // saw through the tokio driver and the forwarding adapter, and the two
    // drivers decide the run alike.
    for (name, md) in STREAM_FIXTURES {
        let (observed_result, records) = run_recorded(md).await;
        let (result, stream) = drive_serially(md);

        let streamed: Vec<(String, String)> = stream.iter().map(event_trace).collect();
        let observed: Vec<(String, String)> = events(&records)
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

        match (result, observed_result) {
            (RunResult::Ok(text), Ok(observed_text)) => {
                assert_eq!(
                    text, observed_text,
                    "{name}: both drivers return the same text"
                );
            }
            (RunResult::Failure(error), Err(observed_error)) => assert_eq!(
                Error::from(error).to_string(),
                observed_error.to_string(),
                "{name}: both drivers fail with the same error"
            ),
            (result, observed_result) => panic!(
                "{name}: the two drivers must decide the run alike: {result:?} vs {observed_result:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_two_section_run_reports_the_exact_observation_sequence() {
    let (result, records) = run_recorded(TWO_SECTIONS).await;
    assert_eq!(result.unwrap(), "second");

    assert_eq!(
        events(&records),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            ("First".to_string(), detail::SECTION_STARTED.to_string()),
            (
                "First".to_string(),
                detail::LUA_SHARED_LOAD_STARTED.to_string(),
            ),
            (
                "First".to_string(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string(),
            ),
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
            (
                "Second".to_string(),
                detail::LUA_SHARED_LOAD_STARTED.to_string(),
            ),
            (
                "Second".to_string(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string(),
            ),
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

#[tokio::test]
async fn recording_and_null_observers_produce_the_same_result_and_store_state() {
    let prompt = fixture(STORE_SECTIONS);
    let recorded_store = TestStore::new();
    let sink = Arc::new(Recorder::default());
    let observed_result = run(
        &prompt,
        "",
        &[],
        &recorded_store,
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&sink) as Arc<dyn Observer>,
            client: None,
            debug: None,
        },
    )
    .await;
    let null_store = TestStore::new();
    let null_result = run(&prompt, "", &[], &null_store, silent()).await;

    assert_eq!(observed_result.unwrap(), null_result.unwrap());
    assert_eq!(
        recorded_store.glob("**").unwrap(),
        null_store.glob("**").unwrap(),
        "observer choice must not change store side effects"
    );
    assert_eq!(
        recorded_store.read("state.txt").unwrap(),
        null_store.read("state.txt").unwrap(),
        "observer choice must not change stored contents"
    );

    let failing = fixture(FAILING_PROMPT);
    let sink = Arc::new(Recorder::default());
    let observed_error = run(
        &failing,
        "",
        &[],
        &TestStore::new(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&sink) as Arc<dyn Observer>,
            client: None,
            debug: None,
        },
    )
    .await
    .expect_err("the prologue fails");
    let null_error = run(&failing, "", &[], &TestStore::new(), silent())
        .await
        .expect_err("the prologue fails");
    assert_eq!(
        observed_error.to_string(),
        null_error.to_string(),
        "observer choice must not change errors"
    );
}

#[tokio::test]
async fn a_run_refused_by_the_version_gate_reports_nothing() {
    // The gate is not a run that failed; it is a run that never started, so
    // there is no RunStarted to pair a RunFinished with.
    let md = "---\nname: t\ndescription: d\npromptforge: 2\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let (result, records) = run_recorded(md).await;
    assert!(result.is_err());
    assert!(
        records.is_empty(),
        "the gate must report nothing: {records:?}"
    );
}

#[tokio::test]
async fn a_failing_run_still_reports_run_finished() {
    // The prologue fails, so the walk tears down its VM and the final
    // observation must report the run failure.
    let (result, records) = run_recorded(FAILING_PROMPT).await;
    assert!(matches!(
        result,
        Err(Error::Lua(_) | Error::LuaRuntime { .. })
    ));

    assert_eq!(
        events(&records),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            ("Only".to_string(), detail::SECTION_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_SHARED_LOAD_STARTED.to_string(),
            ),
            (
                "Only".to_string(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string(),
            ),
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

#[tokio::test]
async fn an_erroring_section_reports_started_but_not_finished() {
    // The absence half of the section-boundary contract: a section that
    // errors mid-walk must emit SECTION_STARTED and never SECTION_FINISHED.
    let (result, records) = run_recorded(FAILING_PROMPT).await;
    assert!(result.is_err());

    let observed = events(&records);
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

#[tokio::test]
async fn an_erroring_section_tears_down_exactly_once_without_finishing() {
    // The RAII teardown contract on the error path: the frame's drop fires
    // the teardown pair exactly once, and the disarmed completion flag
    // keeps SECTION_FINISHED from firing for the erroring section.
    let (result, records) = run_recorded(SECOND_SECTION_ERRORS).await;
    assert!(result.is_err());

    let observed = events(&records);
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

#[tokio::test]
async fn a_one_byte_limit_fails_host_injection_with_teardown_observations() {
    // mlua accepts the one-byte ceiling itself, then the first host allocation
    // fails. Host injection is inside the section's teardown boundary, unlike
    // the preceding bare apply_lua_limits call.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let recorder = Arc::new(Recorder::default());
    let sink = Arc::clone(&recorder) as Arc<dyn Observer>;
    let result = run_with_context(&fixture(md), move |ctx| {
        ctx.observer(sink)
            .limits(RunLimits::new().lua_memory_bytes(std::num::NonZeroUsize::MIN))
    })
    .await;
    let error = result.expect_err("a 1-byte Lua memory ceiling must fail the run");
    assert!(
        error.to_string().contains("memory"),
        "the failure must be the memory ceiling, got: {error}"
    );

    let observed = events(&recorder.records());
    assert!(
        observed.contains(&("Only".to_owned(), detail::LUA_TEARDOWN_STARTED.to_string()))
            && observed.contains(&(
                "Only".to_owned(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string()
            )),
        "host injection failure must fire both teardown observations: {observed:?}"
    );
}

#[tokio::test]
async fn one_execution_id_spans_parse_and_the_complete_runtime_lifecycle() {
    let gateway = ScriptedGateway::start(vec![resp_text("aliased final")]).await;
    let addr = gateway.addr();
    let tool = Arc::new(ScopedFixtureTool::new(
        "echo",
        "canonical_echo",
        "Echo a test value.",
    ));
    let source = "---\nname: lifecycle\ndescription: Correlated lifecycle fixture\npromptforge: 0\ncapabilities:\n  - tests/tools\ntools:\n  echo: tests/tools/echo\nmodels:\n  writer: {}\n---\n\n\
         # Lifecycle\n\n```lua\n\
         tools.always('echo')\n\
         models.default('writer')\n```\n\n\
         ## Gather\n\n```lua\nstore.write('state.txt', 'before')\n```\n\n\
         Use the echo tool.\n\n\
         ```lua\n\
         local text = models.infer(prose)\n\
         local _ = tools.call('echo', { value = 'hi' })\n\
         store.append('state.txt', '\\nafter')\n\
         return text\n\
         ```\n";
    let recorder = Arc::new(Recorder::default());
    let (prompt, parse_events) = Prompt::parse(source, EXECUTION);
    crate::test_support::forward(parse_events, recorder.as_ref());
    let prompt = prompt.expect("the lifecycle fixture must parse");
    let tools: [Arc<dyn TestTool>; 1] = [Arc::clone(&tool) as Arc<dyn TestTool>];
    let prompt = TestPrompt {
        prompt,
        models: test_model_catalog(),
    };
    let store = TestStore::new();

    let result = run(
        &prompt,
        "",
        &tools,
        &store,
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            client: Some(gateway_client(addr)),
            debug: None,
        },
    )
    .await
    .expect("the lifecycle fixture must run");

    assert_eq!(result, "aliased final");
    assert_eq!(store.read("state.txt").unwrap(), "before\nafter");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    let records = recorder.records();
    assert!(!records.is_empty());
    assert!(
        records
            .iter()
            .all(|(execution, _, _)| execution == EXECUTION),
        "every lifecycle record must retain {EXECUTION}: {records:#?}"
    );
    let details = records
        .iter()
        .map(|(_, _, detail)| detail.clone())
        .collect::<Vec<_>>();
    for expected in [
        detail::PARSE_STARTED,
        detail::RUN_STARTED,
        detail::SECTION_STARTED,
        detail::LUA_CHUNK_STARTED,
        detail::STORE_WRITE_SUCCEEDED,
        detail::MODEL_TURN_COMPLETED,
        detail::TOOL_CALL_SUCCEEDED,
        detail::LUA_CHUNK_STARTED,
        detail::STORE_APPEND_SUCCEEDED,
        detail::RUN_SUCCEEDED,
    ] {
        assert!(
            details.contains(&expected.to_string()),
            "the complete lifecycle must include {expected:?}: {records:#?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_tool_loop_reports_each_turn_and_each_tool_call() {
    use super::models_loop::{echo_tools, loop_context_observed, loop_events, loop_prompt};
    use crate::test_support::tokio_driver::TokioDriver;

    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('ask the model')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(Recorder::default());
    let ctx = loop_context_observed(
        &prompt,
        echo_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the loop converges");
    assert_eq!(out, "final answer");

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
