//! The Host's optional input broker at preparation, supplied among its
//! services under `INPUT_BROKER`: activation hands it to every declared
//! capability, or hands none when the Host has nobody to ask; and the
//! `promptforge/user-input` capability's `input.ask()` reaches it, is
//! refused when required on a Host without one, and degrades when
//! optional. A frontmatter alias named `input` collides with the
//! capability's prelude global and fails the run before any effect, while
//! an alias of another name runs beside it.

use super::*;

use std::sync::Mutex;

use harness_capabilities::{INPUT_BROKER, InputBroker, InputError, UserInput, activate};
use promptforge::Prompt;

/// A prompt declaring the probe capability, with nothing to run.
const DECLARES_PROBE: &str = "---\nname: declares-probe\ndescription: d\npromptforge: 0\n\
    capabilities:\n  - tests/probe\n---\n\n# Title\n\n## Only\n\nDone.\n";

/// A broker whose operator always types the same text.
struct Scripted(&'static str);

#[async_trait::async_trait]
impl InputBroker for Scripted {
    async fn wait(&self) -> Result<String, InputError> {
        Ok(self.0.to_owned())
    }
}

/// A broker whose every wait fails, with a hidden cause behind the
/// message.
struct Failing;

#[async_trait::async_trait]
impl InputBroker for Failing {
    async fn wait(&self) -> Result<String, InputError> {
        Err(InputError::with_source(
            "the operator's window closed",
            std::io::Error::other("socket reset"),
        ))
    }
}

/// A fixture capability contributing nothing, which records whether each
/// activation's services carried a broker.
struct Probe {
    id: CapabilityId,
    saw_broker: Arc<Mutex<Vec<bool>>>,
}

impl Capability for Probe {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "Records whether a broker reached activation."
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
        self.saw_broker
            .lock()
            .unwrap()
            .push(services.get(&INPUT_BROKER).is_some());
        Ok(Contribution::default())
    }
}

/// Host services holding `input` under `INPUT_BROKER`, or none when the
/// Host has nobody to ask.
fn with_input(input: Option<Arc<dyn InputBroker>>) -> HostServices {
    let mut host = HostServices::new();
    if let Some(input) = input {
        host.provide(&INPUT_BROKER, input).unwrap();
    }
    host
}

/// Prepares the probe-declaring prompt with `input` as the Host's broker,
/// and returns what each activation of the probe saw.
async fn probe_activations(input: Option<Arc<dyn InputBroker>>) -> Vec<bool> {
    let saw_broker = Arc::new(Mutex::new(Vec::new()));
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(Probe {
            id: CapabilityId::parse("tests/probe").unwrap(),
            saw_broker: Arc::clone(&saw_broker),
        }))
        .unwrap();
    let recorder = recorder();
    let mut services = services(&recorder, Some(Arc::new(registry)));
    services.services = with_input(input);
    prepare(DECLARES_PROBE, "", services)
        .await
        .expect("the prompt prepares");
    saw_broker.lock().unwrap().clone()
}

/// Prepares `source` under `services` and drives the run to its end.
async fn drive_prompt(source: &str, services: Services) -> RunOutcome {
    let recorder = Arc::clone(&services.recorder);
    let prepared = prepare(source, "", services)
        .await
        .expect("the prompt prepares");
    drive_run(
        prepared.run,
        prepared.performers,
        recorder,
        prepared.run_id,
        CancelHandle::new(),
    )
    .await
    .expect("the loop reaches an outcome")
}

#[tokio::test]
async fn activation_hands_the_hosts_broker_to_each_declared_capability() {
    let seen = probe_activations(Some(Arc::new(Scripted("unused")))).await;
    assert_eq!(
        seen,
        [true],
        "the probe's one activation received the host's broker"
    );
}

#[tokio::test]
async fn activation_hands_no_broker_to_a_capability_when_the_host_has_none() {
    let seen = probe_activations(None).await;
    assert_eq!(
        seen,
        [false],
        "the probe's one activation received no broker"
    );
}

/// The frontmatter line declaring `promptforge/user-input` as required,
/// the way `chat.md` declares it.
const REQUIRED: &str = "  - promptforge/user-input\n";

/// The frontmatter lines declaring `promptforge/user-input` as optional.
const OPTIONAL: &str = "  - ref: promptforge/user-input\n    optional: true\n";

/// Asks once and returns the run-fixed connection flag, the answer, and
/// the availability beside it.
const ASKS_ONCE: &str = "local text, available = input.ask()\n\
    return tostring(input.connected()) .. '|' .. text .. '|' .. tostring(available)";

/// A one-section prompt that declares the user-input capability with
/// `declaration` (none when empty) and runs `lua`.
fn user_input_prompt(declaration: &str, lua: &str) -> String {
    let capabilities = if declaration.is_empty() {
        String::new()
    } else {
        format!("capabilities:\n{declaration}")
    };
    format!(
        "---\nname: asks-input\ndescription: d\npromptforge: 0\n{capabilities}---\n\n\
         # Title\n\n## Only\n\n```lua\n{lua}\n```\n"
    )
}

/// A registry holding the first-party user-input capability.
fn user_input_registry() -> Arc<CapabilityRegistry> {
    let mut registry = CapabilityRegistry::new();
    registry.register(Arc::new(UserInput::new())).unwrap();
    Arc::new(registry)
}

/// The preparation services over `recorder` with the user-input registry and
/// `input` as the Host's broker.
fn user_input_services(
    recorder: &Arc<MemoryRecorder>,
    input: Option<Arc<dyn InputBroker>>,
) -> Services {
    let mut services = services(recorder, Some(user_input_registry()));
    services.services = with_input(input);
    services
}

#[tokio::test]
async fn a_required_user_input_declaration_on_a_host_without_a_broker_is_refused() {
    let recorder = recorder();
    let error = prepare(
        &user_input_prompt(REQUIRED, ASKS_ONCE),
        "",
        user_input_services(&recorder, None),
    )
    .await
    .expect_err("a required capability without its service refuses the run");
    let PrepareError::Refused { error, .. } = error else {
        panic!("the refusal is a requirements refusal: {error}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    assert!(
        error.to_string().contains(
            "- promptforge/user-input needs promptforge/input-broker, and this host provides none"
        ),
        "the notice names the capability and the missing service: {error}"
    );
}

#[tokio::test]
async fn a_required_user_input_tool_slot_without_a_broker_is_refused_for_the_broker_alone() {
    let recorder = recorder();
    let declaration = format!("{REQUIRED}tools:\n  ask: promptforge/user-input/ask\n");
    let error = prepare(
        &user_input_prompt(&declaration, ASKS_ONCE),
        "",
        user_input_services(&recorder, None),
    )
    .await
    .expect_err("a required capability without its service refuses the run");
    let PrepareError::Refused { error, .. } = error else {
        panic!("the refusal is a requirements refusal: {error}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    assert!(
        notice.contains(
            "- promptforge/user-input needs promptforge/input-broker, and this host provides none"
        ),
        "the notice names the capability and the missing service: {notice}"
    );
    assert!(
        !notice.contains("missing required capability: promptforge/user-input"),
        "the notice does not call the registered capability missing: {notice}"
    );
}

#[tokio::test]
async fn an_optional_user_input_declaration_without_a_broker_runs_on_the_fallback() {
    let recorder = recorder();
    let outcome = drive_prompt(
        &user_input_prompt(OPTIONAL, ASKS_ONCE),
        user_input_services(&recorder, None),
    )
    .await;
    assert_eq!(
        completed(outcome),
        "false|User input is unavailable in this host; continue without it.|false",
        "input.connected() is false and input.ask() answers the fallback, unavailable"
    );
}

#[test]
fn an_optional_user_input_declaration_without_a_broker_records_the_service_gap() {
    let source = user_input_prompt(OPTIONAL, ASKS_ONCE);
    let prompt = Prompt::parse(&source, "asks-input").0.unwrap();
    let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new());
    let activation = activate(Some(&user_input_registry()), &prompt, &services);
    assert!(activation.requirements.is_satisfied());
    assert_eq!(activation.service_gaps.len(), 1, "one gap is recorded");
    let gap = &activation.service_gaps[0];
    assert_eq!(gap.capability.to_string(), "promptforge/user-input");
    assert_eq!(gap.service, INPUT_BROKER.id());
}

#[tokio::test]
async fn an_operator_who_types_the_fallback_sentence_is_still_available() {
    let recorder = recorder();
    let fallback = "User input is unavailable in this host; continue without it.";
    let outcome = drive_prompt(
        &user_input_prompt(REQUIRED, ASKS_ONCE),
        user_input_services(&recorder, Some(Arc::new(Scripted(fallback)))),
    )
    .await;
    assert_eq!(
        completed(outcome),
        format!("true|{fallback}|true"),
        "the operator's text comes back byte-exact, marked available"
    );
}

#[tokio::test]
async fn a_failed_ask_raises_at_the_call_site_where_pcall_catches_it() {
    let recorder = recorder();
    let catches = "local ok, err = pcall(input.ask)\n\
        return tostring(ok) .. '|' .. err.kind .. '|' .. tostring(err)";
    let outcome = drive_prompt(
        &user_input_prompt(REQUIRED, catches),
        user_input_services(&recorder, Some(Arc::new(Failing))),
    )
    .await;
    let text = completed(outcome);
    assert!(text.starts_with("false|tool|"), "a tool failure: {text}");
    assert!(
        text.contains("the operator's window closed"),
        "the broker's message reaches the script: {text}"
    );
    assert!(
        !text.contains("socket reset"),
        "the broker's hidden cause never reaches the script: {text}"
    );
}

#[tokio::test]
async fn an_uncaught_failed_ask_ends_the_run_as_a_tool_failure() {
    let recorder = recorder();
    let outcome = drive_prompt(
        &user_input_prompt(REQUIRED, "return (input.ask())"),
        user_input_services(&recorder, Some(Arc::new(Failing))),
    )
    .await;
    let RunOutcome::Failed { kind, message } = outcome else {
        panic!("an uncaught failed ask fails the run: {outcome:?}");
    };
    assert_eq!(kind, "Tool");
    assert!(
        message.contains("the operator's window closed"),
        "the broker's message reaches the run's failure: {message}"
    );
}

#[tokio::test]
async fn input_ask_with_an_argument_raises() {
    let recorder = recorder();
    let passes_one = "local ok, err = pcall(input.ask, 1)\n\
        return tostring(ok) .. '|' .. tostring(err)";
    let outcome = drive_prompt(
        &user_input_prompt(REQUIRED, passes_one),
        user_input_services(&recorder, Some(Arc::new(Scripted("unused")))),
    )
    .await;
    let text = completed(outcome);
    assert!(text.starts_with("false|"), "the call raised: {text}");
    assert!(
        text.contains("input.ask takes no arguments"),
        "the error says why: {text}"
    );
}

#[tokio::test]
async fn an_alias_named_like_the_user_input_prelude_global_fails_the_run_before_any_effect() {
    let recorder = recorder();
    let declaration = format!("{REQUIRED}tools:\n  input: promptforge/user-input/ask\n");
    let prepared = prepare(
        &user_input_prompt(&declaration, ASKS_ONCE),
        "",
        user_input_services(&recorder, Some(Arc::new(Scripted("unused")))),
    )
    .await
    .expect("the prompt parses and its requirements are met");
    let run_id = prepared.run_id;
    let outcome = drive_run(
        prepared.run,
        prepared.performers,
        recorder.clone(),
        run_id,
        CancelHandle::new(),
    )
    .await
    .expect("the loop reaches an outcome");
    let RunOutcome::Failed { kind, message } = outcome else {
        panic!("the collision fails the run: {outcome:?}");
    };
    assert_eq!(kind, "Lua");
    assert!(
        message.contains(
            "capability `promptforge/user-input`: its prelude defines the global `input`, \
             which the prompt's frontmatter binds as a tool or model alias"
        ),
        "the failure names the capability, the global, and the alias: {message}"
    );
    let effects = recorder
        .records(run_id)
        .into_iter()
        .filter(|record| record.kind == RecordKind::Effect)
        .count();
    assert_eq!(effects, 0, "the run fails before it issues any effect");
}

#[tokio::test]
async fn a_normal_alias_for_the_ask_tool_runs_beside_the_untouched_engine_globals() {
    let recorder = recorder();
    let declaration = format!("{REQUIRED}tools:\n  ask: promptforge/user-input/ask\n");
    let outcome = drive_prompt(
        &user_input_prompt(
            &declaration,
            "return tools.call(ask) .. '|' .. ask.name .. '|' .. type(input.ask) .. '|' \
             .. type(store.read) .. '|' .. type(tools.call)",
        ),
        user_input_services(&recorder, Some(Arc::new(Scripted("hello")))),
    )
    .await;
    assert_eq!(
        completed(outcome),
        "hello|ask|function|function|function",
        "the alias global is the ask tool, and input, store, and tools are the Engine's"
    );
}

#[tokio::test]
async fn a_prompt_that_does_not_declare_user_input_has_no_input_global() {
    let recorder = recorder();
    let reaches = "local ok, err = pcall(function() return input.ask() end)\n\
        return tostring(ok) .. '|' .. tostring(err)";
    let outcome = drive_prompt(
        &user_input_prompt("", reaches),
        user_input_services(&recorder, Some(Arc::new(Scripted("unused")))),
    )
    .await;
    let text = completed(outcome);
    assert!(text.starts_with("false|"), "the call raised: {text}");
    assert!(
        text.contains("attempt to index a nil value (global 'input')"),
        "Lua's own error names the missing global: {text}"
    );
}
