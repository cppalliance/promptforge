//! Run preparation: a prompt whose requirements the environment cannot
//! meet is refused with the Engine's own notice and its run ended as
//! failed; a prompt that does not parse fails the same way under the
//! `Parse` kind, and each failure returns the events it recorded; each
//! preparation draws a fresh seed and start, both handed to the recorder
//! when the run begins; and the prepared tool performer resolves a
//! `ToolCall` effect's id in the activated table. The Host's optional
//! input broker - handed to every activated capability and behind the
//! `promptforge/user-input` capability - sits in the `input` child
//! module, the Host's services reaching activation sit in the
//! `host_services` child module, a capability's prelude reaching the
//! prepared run sits in the `prelude` child module, and the prompt's
//! declared input and output files sit in the `files` child module.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use harness_capabilities::{
    Capability, CapabilityError, CapabilityId, CapabilityRegistry, Contribution, HostServices,
    RunServices, Tool, ToolTable,
};
use harness_runner::display_chain;
use harness_runner::effect_loop::drive_run;
use harness_runner::performers::{ActivatedTools, ToolPerformer};
use harness_runner::prepare::{PrepareError, Prepared, Services, prepare_run};
use harness_runner::recorder::{MemoryRecorder, RecordKind, RunId, RunOutcome};
use promptforge::RunErrorKind;
use promptforge::cancel::CancelHandle;
use promptforge::event::Event;
use promptforge::tools::{ToolError, ToolId, ToolOutput};

use crate::support::{Unused, no_deltas};

#[path = "prepare-files.rs"]
mod files;
#[path = "prepare-host-services.rs"]
mod host_services;
#[path = "prepare-input.rs"]
mod input;
#[path = "prepare-prelude.rs"]
mod prelude;

/// A prompt declaring `promptforge/web` as a required capability that no
/// registry here provides.
const NEEDS_WEB: &str = "---\nname: needs-web\ndescription: d\npromptforge: 0\n\
    capabilities:\n  - promptforge/web\n---\n\n# Title\n\n## Only\n\nDone.\n";

/// A prompt with unclosed frontmatter, so it does not parse.
const UNCLOSED: &str = "---\nname: unclosed\ndescription: d\npromptforge: 0\n\n# Title\n";

/// A capability-free prompt whose one section returns a constant.
const PLAIN: &str = "---\nname: plain\ndescription: d\npromptforge: 0\n---\n\n\
    # Title\n\n## Only\n\n```lua\nreturn 'plain'\n```\n";

/// A prompt binding `echo` to the fixture tool and calling it once.
const CALLS_ECHO: &str = "---\nname: calls-echo\ndescription: d\npromptforge: 0\n\
    capabilities:\n  - tests/tools\ntools:\n  echo: tests/tools/echo\n---\n\n\
    # Title\n\n## Only\n\n```lua\nreturn tools.call('echo', { value = 'hi' })\n```\n";

/// Writes `source` as a prompt file in `dir` and returns its path.
fn prompt_file(dir: &Path, source: &str) -> PathBuf {
    let path = dir.join("agent.md");
    std::fs::write(&path, source).expect("the fixture prompt is written");
    path
}

/// An empty in-memory recorder.
fn recorder() -> Arc<MemoryRecorder> {
    Arc::new(MemoryRecorder::new())
}

/// Asserts `events`, the ones a failed preparation returned, are what the
/// recorder holds for `run_id`: a parse that reported, in order, and
/// nothing else, because the loop never saw the run.
fn assert_events_are_the_recorded_ones(recorder: &MemoryRecorder, run_id: RunId, events: &[Event]) {
    assert!(
        matches!(events.first(), Some(Event::ParseStarted { .. })),
        "the events open with the parse: {events:?}"
    );
    let records = recorder.records(run_id);
    assert!(
        records
            .iter()
            .all(|record| record.kind == RecordKind::Event),
        "a failed preparation records only events"
    );
    let stored: Vec<serde_json::Value> = records.into_iter().map(|record| record.payload).collect();
    let returned: Vec<serde_json::Value> = events
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();
    assert_eq!(stored, returned, "the error carries what was recorded");
}

/// The preparation services over `recorder` and `registry`, with no Host
/// services and the performers no test here reaches.
fn services(recorder: &Arc<MemoryRecorder>, registry: Option<Arc<CapabilityRegistry>>) -> Services {
    Services {
        registry,
        services: HostServices::new(),
        vfs: promptforge::vfs::VfsRef::default(),
        input_text: None,
        cancel: CancelHandle::new(),
        recorder: recorder.clone(),
        broker: Arc::new(Unused),
        on_delta: no_deltas(),
        input: None,
        session_id: "session-1".to_owned(),
        agent: "prepare-test".to_owned(),
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

    async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let value = args
            .get("value")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ToolError::message("echo: missing `value`"))?;
        Ok(ToolOutput::trusted(value.to_owned()))
    }
}

/// A fixture capability contributing the echo tool.
struct Tools {
    id: CapabilityId,
}

impl Capability for Tools {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "The test tools."
    }

    fn create(&self, _services: &RunServices) -> Result<Contribution, CapabilityError> {
        Ok(Contribution {
            tools: vec![Arc::new(Echo {
                id: ToolId::parse("tests/tools/echo").unwrap(),
            })],
            prelude: None,
        })
    }
}

/// A registry holding the fixture capability.
fn fixture_registry() -> Arc<CapabilityRegistry> {
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(Tools {
            id: CapabilityId::parse("tests/tools").unwrap(),
        }))
        .unwrap();
    Arc::new(registry)
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
    let dir = tempfile::tempdir().unwrap();
    let recorder = recorder();
    let error = prepare_run(
        &prompt_file(dir.path(), NEEDS_WEB),
        "",
        services(&recorder, None),
    )
    .await
    .expect_err("a missing required capability refuses the run");

    let PrepareError::Refused {
        run_id,
        events,
        error,
    } = error
    else {
        panic!("the refusal is a requirements refusal: {error}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = "the environment cannot satisfy this prompt:\n\
        - missing required capability: promptforge/web";
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
    assert!(
        matches!(events.last(), Some(Event::ParseSucceeded { .. })),
        "the prompt parsed before the environment refused it: {events:?}"
    );
    assert_events_are_the_recorded_ones(&recorder, run_id, &events);
}

#[tokio::test]
async fn a_prompt_that_does_not_parse_fails_preparation_and_its_run_ends_as_a_parse_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = prompt_file(dir.path(), UNCLOSED);
    let recorder = recorder();
    let error = prepare_run(&path, "", services(&recorder, None))
        .await
        .expect_err("a prompt without a closed frontmatter does not parse");

    let PrepareError::Parse {
        path: reported,
        run_id,
        events,
        source,
    } = error
    else {
        panic!("the failure is a parse failure: {error}");
    };
    assert_eq!(reported, path, "the failure names the prompt it read");

    assert_eq!(
        recorder.outcome(run_id),
        Some(RunOutcome::Failed {
            kind: "Parse".to_owned(),
            message: display_chain(&source),
        }),
        "the recorder holds the parse failure and its cause chain under the Parse kind"
    );
    assert!(
        matches!(events.last(), Some(Event::ParseFailed { .. })),
        "the events end with the failed parse: {events:?}"
    );
    assert_events_are_the_recorded_ones(&recorder, run_id, &events);
}

#[tokio::test]
async fn two_prepared_runs_draw_different_seeds_and_both_begin_at_the_recorder() {
    let dir = tempfile::tempdir().unwrap();
    let path = prompt_file(dir.path(), PLAIN);
    let recorder = recorder();
    let first = prepare_run(&path, "", services(&recorder, None))
        .await
        .unwrap();
    let second = prepare_run(&path, "", services(&recorder, None))
        .await
        .unwrap();

    assert_ne!(
        first.seed, second.seed,
        "each preparation draws its own seed"
    );
    assert_ne!(
        first.run_id, second.run_id,
        "each preparation begins its own run"
    );
    for prepared in [&first, &second] {
        let meta = recorder
            .meta(prepared.run_id)
            .expect("the recorder began the run");
        assert_eq!(
            meta.seed, prepared.seed,
            "the recorder holds the seed the run was given"
        );
        assert_eq!(
            meta.started_at,
            prepared.started_at.unix_millis(),
            "the recorder holds the start the run was given"
        );
        assert_eq!(meta.session_id, "session-1");
        assert_eq!(meta.agent, "prepare-test");
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
    let dir = tempfile::tempdir().unwrap();
    let recorder = recorder();
    let Prepared {
        run,
        run_id,
        performers,
        ..
    } = prepare_run(
        &prompt_file(dir.path(), PLAIN),
        "",
        services(&recorder, None),
    )
    .await
    .unwrap();
    let outcome = drive_run(
        run,
        performers,
        recorder.clone(),
        run_id,
        CancelHandle::new(),
        |_event| {},
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
    let dir = tempfile::tempdir().unwrap();
    let recorder = recorder();
    let prepared = prepare_run(
        &prompt_file(dir.path(), CALLS_ECHO),
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
        |_event| {},
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
    let error = performer
        .call(
            ToolId::parse("tests/tools/echo").unwrap(),
            "echo".to_owned(),
            serde_json::json!({}),
        )
        .await
        .expect_err("an id outside the table is refused");
    assert!(
        error.to_string().contains("tests/tools/echo"),
        "the refusal names the id: {error}"
    );
}
