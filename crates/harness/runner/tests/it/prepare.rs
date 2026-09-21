//! Run preparation: a prompt whose requirements the environment cannot
//! meet is refused with the engine's own notice and its row closed as
//! failed; a prompt that does not parse fails the same way under the
//! `Parse` kind; each preparation draws a fresh seed and start, both
//! written to `runs`; and the prepared tool performer resolves a
//! `ToolCall` effect's id in the activated table.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use harness_capabilities::{
    Capability, CapabilityError, CapabilityId, CapabilityRegistry, Contribution, RunServices, Tool,
    ToolTable,
};
use harness_log::{RunLog, RunOutcome};
use harness_runner::display_chain;
use harness_runner::effect_loop::{SharedLog, drive_run};
use harness_runner::performers::{ActivatedTools, ToolPerformer};
use harness_runner::prepare::{PrepareError, Prepared, Services, prepare_run};
use promptforge_api_runtime::execute::RunErrorKind;
use promptforge_api_types::cancel::CancelHandle;
use promptforge_api_types::tools::{ToolError, ToolId, ToolOutput};

use crate::support::Unused;

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

/// An in-memory log behind the loop's mutex.
async fn log() -> SharedLog {
    Arc::new(tokio::sync::Mutex::new(RunLog::in_memory().await.unwrap()))
}

/// The preparation services over `log` and `registry`, with the performers
/// no test here reaches.
fn services(log: &SharedLog, registry: Option<Arc<CapabilityRegistry>>) -> Services {
    Services {
        registry,
        vfs: shared_vfs::VfsRef::builder().build(),
        cancel: CancelHandle::new(),
        log: Arc::clone(log),
        chat: Arc::new(Unused),
        input: Arc::new(Unused),
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
async fn an_unmet_requirement_is_refused_with_the_engines_notice_and_its_row_closed_as_failed() {
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let error = prepare_run(
        &prompt_file(dir.path(), NEEDS_WEB),
        "",
        services(&log, None),
    )
    .await
    .expect_err("a missing required capability refuses the run");

    let PrepareError::Refused { run_id, error } = error else {
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

    let row = log.lock().await.run(run_id).await.unwrap();
    assert!(row.ended_at.is_some(), "the refused run's row is closed");
    assert_eq!(
        row.outcome,
        Some(RunOutcome::Failed {
            kind: "RequirementsUnmet".to_owned(),
            message: notice.to_owned(),
        }),
        "the row records the refusal as the run's failure"
    );
}

#[tokio::test]
async fn a_prompt_that_does_not_parse_fails_preparation_and_its_row_closes_as_a_parse_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = prompt_file(dir.path(), UNCLOSED);
    let log = log().await;
    let error = prepare_run(&path, "", services(&log, None))
        .await
        .expect_err("a prompt without a closed frontmatter does not parse");

    let PrepareError::Parse {
        path: reported,
        run_id,
        source,
    } = error
    else {
        panic!("the failure is a parse failure: {error}");
    };
    assert_eq!(reported, path, "the failure names the prompt it read");

    let row = log.lock().await.run(run_id).await.unwrap();
    assert!(row.ended_at.is_some(), "the unparsed run's row is closed");
    assert_eq!(
        row.outcome,
        Some(RunOutcome::Failed {
            kind: "Parse".to_owned(),
            message: display_chain(&source),
        }),
        "the row records the parse failure and its cause chain under the Parse kind"
    );
}

#[tokio::test]
async fn two_prepared_runs_draw_different_seeds_and_both_appear_in_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = prompt_file(dir.path(), PLAIN);
    let log = log().await;
    let first = prepare_run(&path, "", services(&log, None)).await.unwrap();
    let second = prepare_run(&path, "", services(&log, None)).await.unwrap();

    assert_ne!(
        first.seed, second.seed,
        "each preparation draws its own seed"
    );
    assert_ne!(
        first.run_id, second.run_id,
        "each preparation opens its own row"
    );
    for prepared in [&first, &second] {
        let row = log.lock().await.run(prepared.run_id).await.unwrap();
        assert_eq!(
            row.meta.seed, prepared.seed,
            "the row carries the seed the run was given"
        );
        assert_eq!(
            row.meta.started_at,
            prepared.started_at.unix_millis(),
            "the row carries the start the run was given"
        );
        assert_eq!(row.meta.session_id, "session-1");
        assert_eq!(row.meta.agent, "prepare-test");
        assert!(
            row.meta.prompt_hash.starts_with("sha256:"),
            "the prompt hash names its algorithm: {}",
            row.meta.prompt_hash
        );
        assert!(
            row.ended_at.is_none(),
            "a prepared run's row stays open for the loop"
        );
    }
    let first_hash = log
        .lock()
        .await
        .run(first.run_id)
        .await
        .unwrap()
        .meta
        .prompt_hash;
    let second_hash = log
        .lock()
        .await
        .run(second.run_id)
        .await
        .unwrap()
        .meta
        .prompt_hash;
    assert_eq!(
        first_hash, second_hash,
        "the same prompt text hashes the same"
    );
}

#[tokio::test]
async fn a_prepared_run_drives_to_its_end_under_its_own_performers() {
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let Prepared {
        run,
        run_id,
        performers,
        ..
    } = prepare_run(&prompt_file(dir.path(), PLAIN), "", services(&log, None))
        .await
        .unwrap();
    let outcome = drive_run(
        run,
        performers,
        Arc::clone(&log),
        run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .unwrap();
    assert_eq!(completed(outcome), "plain");
    let row = log.lock().await.run(run_id).await.unwrap();
    assert!(
        row.ended_at.is_some(),
        "the loop closes the row preparation opened"
    );
}

#[tokio::test]
async fn the_tool_performer_resolves_the_effects_id_in_the_activated_table() {
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let prepared = prepare_run(
        &prompt_file(dir.path(), CALLS_ECHO),
        "",
        services(&log, Some(fixture_registry())),
    )
    .await
    .unwrap();
    let outcome = drive_run(
        prepared.run,
        prepared.performers,
        Arc::clone(&log),
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
