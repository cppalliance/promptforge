//! Run preparation: a prompt whose requirements the environment cannot
//! meet is refused with the Engine's own notice and its run ended as
//! failed; a prompt that does not parse fails the same way under the
//! `Parse` kind, and each failure leaves its parse events at the recorder;
//! each preparation draws a fresh seed and start, both handed to the
//! recorder when the run begins under the run's name; and the prepared tool
//! performer sends a `ToolCall` effect to the Plugin its id names. The
//! run's optional input broker - one of its own services, lent to every
//! tool call and behind the fixture ask Plugin - sits in the `input`
//! child module, the run's services meeting a Plugin's needs sit in the
//! `host_services` child module, a Plugin's prelude reaching the prepared
//! run sits in the `prelude` child module, the prompt's declared input
//! and output files sit in the `files` child module, and the wait for
//! each Plugin to be ready sits in the `ready` child module.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use harness_runner::effect_loop::drive_run;
use harness_runner::prepare::{PrepareError, Prepared, Services, prepare};
use harness_runner::recorder::{MemoryRecorder, RecordKind, RunId, RunOutcome};
use harness_runner::{HostContext, display_chain};
use promptforge::RunErrorKind;
use promptforge::cancel::CancelHandle;
use promptforge::event::Event;
use promptforge_plugin::{
    HostServices, Package, Plugin, PluginFuture, PluginId, ToolContext, ToolDescriptor, ToolError,
    ToolId, ToolOutput,
};
use serde_json::{Value, json};

use crate::support::Unused;

#[path = "prepare-files.rs"]
mod files;
#[path = "prepare-host-services.rs"]
mod host_services;
#[path = "prepare-input.rs"]
mod input;
#[path = "prepare-prelude.rs"]
mod prelude;
#[path = "prepare-ready.rs"]
mod ready;

/// A prompt declaring `web` as a required Plugin that no
/// Host here installs.
const NEEDS_WEB: &str = "---\nname: needs-web\ndescription: d\npromptforge: 0\n\
    plugins:\n  - web\n---\n\n# Title\n\n## Only\n\nDone.\n";

/// A prompt with unclosed frontmatter, so it does not parse.
const UNCLOSED: &str = "---\nname: unclosed\ndescription: d\npromptforge: 0\n\n# Title\n";

/// A Plugin-free prompt whose one section returns a constant.
const PLAIN: &str = "---\nname: plain\ndescription: d\npromptforge: 0\n---\n\n\
    # Title\n\n## Only\n\n```lua\nreturn 'plain'\n```\n";

/// A prompt binding `echo` to the fixture tool and calling it once.
const CALLS_ECHO: &str = "---\nname: calls-echo\ndescription: d\npromptforge: 0\n\
    plugins:\n  - tools\ntools:\n  echo: tools/echo\n---\n\n\
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

/// A Host with no Plugins.
pub(crate) fn bare() -> HostContext {
    HostContext::new(HostServices::new())
}

/// A Host with `package` installed under its default name.
pub(crate) fn installing(package: Package) -> HostContext {
    let mut host = bare();
    host.install(package, None, Value::Null).unwrap();
    host
}

/// The preparation services over `recorder` and `host`, with no run
/// services and the performers no test here reaches.
pub(crate) fn services(recorder: &Arc<MemoryRecorder>, host: HostContext) -> Services {
    Services {
        host: Arc::new(host),
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

/// The fixture Plugin `tools`: one tool, `tools/echo`, echoing its
/// `value` argument as trusted text.
const TOOLS: Package = echo_package("tests/tools", None);

/// A package whose Plugin offers `<name>/echo`, with `prelude`.
pub(crate) const fn echo_package(name: &'static str, prelude: Option<&'static str>) -> Package {
    let package = Package::new(name, construct_echo);
    match prelude {
        Some(prelude) => package.prelude(prelude),
        None => package,
    }
}

/// A Plugin offering the one echo tool.
struct Echo {
    tools: Vec<ToolDescriptor>,
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the Package construct signature fixes the return type"
)]
fn construct_echo(
    name: &PluginId,
    _config: Value,
    _services: &HostServices,
) -> Result<Arc<dyn Plugin>, ToolError> {
    let echo = ToolDescriptor::new(
        ToolId::parse(&format!("{name}/echo")).unwrap(),
        "Echo the value argument.",
        json!({"type": "object", "properties": {"value": {"type": "string"}}}),
    );
    Ok(Arc::new(Echo { tools: vec![echo] }))
}

impl Plugin for Echo {
    fn tools(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    fn call<'a>(
        &'a self,
        _cx: ToolContext<'a>,
        args: Value,
    ) -> PluginFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let value = args
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::message("echo: missing `value`"))?;
            Ok(ToolOutput::trusted(value.to_owned()))
        })
    }
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
    let error = prepare(NEEDS_WEB, "", services(&recorder, bare()))
        .await
        .expect_err("a missing required Plugin refuses the run");

    let PrepareError::Refused { run_id, error, .. } = error else {
        panic!("the refusal is a requirements refusal: {error}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = "the environment cannot satisfy this prompt:\n\
        - missing required Plugin: web";
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
    let error = prepare(UNCLOSED, "", services(&recorder, bare()))
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
    let first = prepare(PLAIN, "", services(&recorder, bare()))
        .await
        .unwrap();
    let second = prepare(PLAIN, "", services(&recorder, bare()))
        .await
        .unwrap();
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
    } = prepare(PLAIN, "", services(&recorder, bare()))
        .await
        .unwrap();
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

/// Prepares `source` over `host` and drives the run to its end.
pub(crate) async fn drive_over(source: &str, host: HostContext) -> RunOutcome {
    let recorder = recorder();
    let prepared = prepare(source, "", services(&recorder, host))
        .await
        .unwrap();
    drive_run(
        prepared.run,
        prepared.performers,
        recorder.clone(),
        prepared.run_id,
        CancelHandle::new(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn the_tool_performer_sends_the_effect_to_the_plugin_its_id_names() {
    assert_eq!(
        completed(drive_over(CALLS_ECHO, installing(TOOLS)).await),
        "hi",
        "the ToolCall effect reached the echo tool"
    );
}

#[tokio::test]
async fn a_script_reaches_an_undeclared_plugins_tool_by_full_id_without_its_prelude() {
    let undeclared = "---\nname: undeclared\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n## Only\n\n```lua\n\
        return tools.call('extra/echo', { value = 'hi' }) .. '|' .. type(extra)\n```\n";
    let extra = echo_package("tests/extra", Some("extra = {}\n"));
    assert_eq!(
        completed(drive_over(undeclared, installing(extra)).await),
        "hi|nil",
        "the undeclared Plugin's tool is in the catalog, and its prelude is not installed"
    );
}

#[tokio::test]
async fn the_offering_lists_an_undeclared_plugins_tool_and_a_script_calls_it_by_its_record() {
    let offering = "---\nname: offering\ndescription: d\npromptforge: 0\n\
        plugins:\n  - tools\n---\n\n# Title\n\n## Only\n\n```lua\n\
        local offered = tools.offered()\n\
        local record = offered[1]\n\
        return #offered .. '|' .. record.name .. '|' .. record.plugin .. '|' .. \
          tools.call(record, { value = 'hi' })\n```\n";
    let mut host = installing(TOOLS);
    host.install(echo_package("tests/extra", None), None, Value::Null)
        .unwrap();
    assert_eq!(
        completed(drive_over(offering, host).await),
        "1|extra_echo|extra|hi",
        "the declared Plugin's tool stays out, and the undeclared one's record reaches its Plugin"
    );
}
