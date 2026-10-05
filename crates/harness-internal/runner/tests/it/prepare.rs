//! Run preparation: a prompt whose requirements the environment cannot
//! meet is refused with the Engine's own notice and its run ended as
//! failed; a prompt that does not parse fails the same way under the
//! `Parse` kind, and each failure leaves its parse events at the recorder;
//! each preparation draws a fresh seed and start, both handed to the
//! recorder when the run begins under the run's name; and the prepared tool
//! performer resolves a `ToolCall` effect's id in the activated table. The
//! Host's optional input broker - one of its services, handed to every
//! activated Plugin and behind the `promptforge/user-input`
//! Plugin - sits in the `input` child module, the Host's services
//! reaching activation sit in the `host_services` child module, a
//! Plugin's prelude reaching the prepared run sits in the `prelude`
//! child module, and the prompt's declared input and output files sit in
//! the `files` child module.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use harness_plugins::{
    Contribution, HostServices, Plugin, PluginError, PluginId, PluginRegistry, RunServices, Tool,
    ToolContext, ToolTable,
};
use harness_runner::display_chain;
use harness_runner::effect_loop::drive_run;
use harness_runner::performers::{ActivatedTools, ToolPerformer};
use harness_runner::prepare::{PrepareError, Prepared, Services, prepare};
use harness_runner::recorder::{MemoryRecorder, RecordKind, RunId, RunOutcome};
use promptforge::RunErrorKind;
use promptforge::cancel::CancelHandle;
use promptforge::effect::{ToolCallOrigin, ToolCaller};
use promptforge::event::Event;
use promptforge::tools::{ToolError, ToolId, ToolOutput};
use promptforge::vfs::{Origin, VfsRef};

use crate::support::Unused;

#[path = "prepare-files.rs"]
mod files;
#[path = "prepare-host-services.rs"]
mod host_services;
#[path = "prepare-input.rs"]
mod input;
#[path = "prepare-prelude.rs"]
mod prelude;

/// A prompt declaring `promptforge/web` as a required Plugin that no
/// registry here provides.
const NEEDS_WEB: &str = "---\nname: needs-web\ndescription: d\npromptforge: 0\n\
    plugins:\n  - promptforge/web\n---\n\n# Title\n\n## Only\n\nDone.\n";

/// A prompt with unclosed frontmatter, so it does not parse.
const UNCLOSED: &str = "---\nname: unclosed\ndescription: d\npromptforge: 0\n\n# Title\n";

/// A Plugin-free prompt whose one section returns a constant.
const PLAIN: &str = "---\nname: plain\ndescription: d\npromptforge: 0\n---\n\n\
    # Title\n\n## Only\n\n```lua\nreturn 'plain'\n```\n";

/// A prompt binding `echo` to the fixture tool and calling it once.
const CALLS_ECHO: &str = "---\nname: calls-echo\ndescription: d\npromptforge: 0\n\
    plugins:\n  - tests/tools\ntools:\n  echo: tests/tools/echo\n---\n\n\
    # Title\n\n## Only\n\n```lua\nreturn tools.call('echo', { value = 'hi' })\n```\n";

/// An empty in-memory recorder.
fn recorder() -> Arc<MemoryRecorder> {
    Arc::new(MemoryRecorder::new())
}

/// The events the recorder holds for `run_id`, the run a failed
/// preparation ended, after asserting they are a parse that reported, in
/// order, and nothing else, because the loop never saw the run.
fn recorded_parse_events(recorder: &MemoryRecorder, run_id: RunId) -> Vec<Event> {
    let records = recorder.records(run_id);
    assert!(
        records
            .iter()
            .all(|record| record.kind == RecordKind::Event),
        "a failed preparation records only events"
    );
    let events: Vec<Event> = records
        .into_iter()
        .map(|record| serde_json::from_value(record.payload).unwrap())
        .collect();
    assert!(
        matches!(events.first(), Some(Event::ParseStarted { .. })),
        "the events open with the parse: {events:?}"
    );
    events
}

/// The preparation services over `recorder` and `registry`, with no Host
/// services and the performers no test here reaches.
pub(crate) fn services(
    recorder: &Arc<MemoryRecorder>,
    registry: Option<Arc<PluginRegistry>>,
) -> Services {
    Services {
        registry,
        services: HostServices::new(),
        vfs: promptforge::vfs::VfsRef::default(),
        input_text: None,
        cancel: CancelHandle::new(),
        recorder: recorder.clone(),
        broker: Arc::new(Unused),
        timer: Arc::new(Unused),
        name: "session-1".to_owned(),
        model: None,
        ui: None,
    }
}

/// A fixture tool echoing its `value` argument as trusted text.
struct Echo {
    id: ToolId,
}

#[async_trait::async_trait]
impl Tool for Echo {
    fn id(&self) -> ToolId {
        self.id.clone()
    }

    fn wire_name(&self) -> &str {
        self.id.name()
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "Echo the value argument."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"value": {"type": "string"}}})
    }

    async fn call(
        &self,
        _cx: ToolContext<'_>,
        args: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        let value = args
            .get("value")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ToolError::message("echo: missing `value`"))?;
        Ok(ToolOutput::trusted(value.to_owned()))
    }
}

/// A fixture Plugin contributing the echo tool.
struct Tools {
    id: PluginId,
}

impl Plugin for Tools {
    fn id(&self) -> &PluginId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Plugin trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "The test tools."
    }

    fn create(&self, _services: &RunServices) -> Result<Contribution, PluginError> {
        Ok(Contribution {
            tools: vec![Arc::new(Echo {
                id: ToolId::parse("tests/tools/echo").unwrap(),
            })],
            prelude: None,
        })
    }
}

/// A registry holding the fixture Plugin.
fn fixture_registry() -> Arc<PluginRegistry> {
    let mut registry = PluginRegistry::new();
    registry
        .register(Arc::new(Tools {
            id: PluginId::parse("tests/tools").unwrap(),
        }))
        .unwrap();
    Arc::new(registry)
}

/// The wall clock now, UTC milliseconds since the Unix epoch, the unit of
/// `RunMeta::started_at`.
fn unix_millis_now() -> i64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    i64::try_from(elapsed.as_millis()).unwrap()
}

/// The final text of a completed run.
fn completed(outcome: RunOutcome) -> String {
    match outcome {
        RunOutcome::Completed { final_text } => final_text,
        other => panic!("the run completes: {other:?}"),
    }
}

#[tokio::test]
async fn an_unmet_requirement_is_refused_with_the_engines_notice_and_its_run_ended_as_failed() {
    let recorder = recorder();
    let error = prepare(NEEDS_WEB, "", services(&recorder, None))
        .await
        .expect_err("a missing required Plugin refuses the run");

    let PrepareError::Refused { run_id, error, .. } = error else {
        panic!("the refusal is a requirements refusal: {error}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = "the environment cannot satisfy this prompt:\n\
        - missing required Plugin: promptforge/web";
    assert_eq!(
        error.to_string(),
        notice,
        "the refusal is the engine's notice, verbatim"
    );

    assert_eq!(
        recorder.outcome(run_id),
        Some(RunOutcome::Failed {
            kind: "RequirementsUnmet".to_owned(),
            message: notice.to_owned(),
        }),
        "the recorder holds the refusal as the run's failure"
    );
    let events = recorded_parse_events(&recorder, run_id);
    assert!(
        matches!(events.last(), Some(Event::ParseSucceeded { .. })),
        "the prompt parsed before the environment refused it: {events:?}"
    );
}

#[tokio::test]
async fn a_prompt_that_does_not_parse_fails_preparation_and_its_run_ends_as_a_parse_failure() {
    let recorder = recorder();
    let error = prepare(UNCLOSED, "", services(&recorder, None))
        .await
        .expect_err("a prompt without a closed frontmatter does not parse");

    let PrepareError::Parse { run_id, source, .. } = error else {
        panic!("the failure is a parse failure: {error}");
    };

    assert_eq!(
        recorder.outcome(run_id),
        Some(RunOutcome::Failed {
            kind: "Parse".to_owned(),
            message: display_chain(&source),
        }),
        "the recorder holds the parse failure and its cause chain under the Parse kind"
    );
    let events = recorded_parse_events(&recorder, run_id);
    assert!(
        matches!(events.last(), Some(Event::ParseFailed { .. })),
        "the events end with the failed parse: {events:?}"
    );
}

#[tokio::test]
async fn two_prepared_runs_draw_different_seeds_and_both_begin_at_the_recorder() {
    let recorder = recorder();
    let before = unix_millis_now();
    let first = prepare(PLAIN, "", services(&recorder, None)).await.unwrap();
    let second = prepare(PLAIN, "", services(&recorder, None)).await.unwrap();
    let after = unix_millis_now();

    assert_ne!(
        first.run_id, second.run_id,
        "each preparation begins its own run"
    );
    let first_meta = recorder
        .meta(first.run_id)
        .expect("the recorder began the run");
    let second_meta = recorder
        .meta(second.run_id)
        .expect("the recorder began the run");
    assert_ne!(
        first_meta.seed, second_meta.seed,
        "each preparation draws its own seed"
    );
    assert!(
        first_meta.started_at <= second_meta.started_at,
        "each preparation reads the clock as it begins its run: {first_meta:?} then {second_meta:?}"
    );
    for prepared in [&first, &second] {
        let meta = recorder
            .meta(prepared.run_id)
            .expect("the recorder began the run");
        assert!(
            (before..=after).contains(&meta.started_at),
            "the start is the wall clock at preparation: {} not in {before}..={after}",
            meta.started_at
        );
        assert_eq!(meta.name, "session-1", "the metadata holds the run's name");
        assert!(
            meta.prompt_hash.starts_with("sha256:"),
            "the prompt hash names its algorithm: {}",
            meta.prompt_hash
        );
        assert_eq!(
            recorder.outcome(prepared.run_id),
            None,
            "a prepared run stays open for the loop"
        );
    }
    assert_eq!(
        recorder.meta(first.run_id).unwrap().prompt_hash,
        recorder.meta(second.run_id).unwrap().prompt_hash,
        "the same prompt text hashes the same"
    );
}

#[tokio::test]
async fn a_prepared_run_drives_to_its_end_under_its_own_performers() {
    let recorder = recorder();
    let Prepared {
        run,
        run_id,
        performers,
        ..
    } = prepare(PLAIN, "", services(&recorder, None)).await.unwrap();
    let outcome = drive_run(
        run,
        performers,
        recorder.clone(),
        run_id,
        CancelHandle::new(),
    )
    .await
    .unwrap();
    assert_eq!(completed(outcome.clone()), "plain");
    assert_eq!(
        recorder.outcome(run_id),
        Some(outcome),
        "the loop ends the run preparation began"
    );
}

#[tokio::test]
async fn the_tool_performer_resolves_the_effects_id_in_the_activated_table() {
    let recorder = recorder();
    let prepared = prepare(
        CALLS_ECHO,
        "",
        services(&recorder, Some(fixture_registry())),
    )
    .await
    .unwrap();
    let outcome = drive_run(
        prepared.run,
        prepared.performers,
        recorder.clone(),
        prepared.run_id,
        CancelHandle::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        completed(outcome),
        "hi",
        "the ToolCall effect reached the activated echo tool"
    );
}
#[tokio::test]
async fn the_tool_performer_refuses_an_id_the_table_does_not_hold() {
    let performer = ActivatedTools::new(ToolTable::new());
    let access = VfsRef::default()
        .acquire(Origin::new("prepare test"))
        .unwrap();
    let origin = ToolCallOrigin {
        execution: "prepare-test".to_owned(),
        section: "Only".to_owned(),
        caller: ToolCaller::Script,
    };
    let error = performer
        .call(
            ToolId::parse("tests/tools/echo").unwrap(),
            "echo".to_owned(),
            Arc::new(access),
            origin,
            serde_json::json!({}),
        )
        .await
        .expect_err("an id outside the table is refused");
    assert!(
        error.to_string().contains("tests/tools/echo"),
        "the refusal names the id: {error}"
    );
}
