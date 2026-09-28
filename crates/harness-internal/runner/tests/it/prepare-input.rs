//! The host's optional input broker at preparation: activation hands it
//! to every declared capability, or hands none when the host has nobody
//! to ask; a `user_input()` call is answered through it, with the
//! unavailable fallback when there is none; and the
//! `promptforge/user-input` capability's `input.ask()` reaches it, is
//! refused when required on a host without one, and degrades when
//! optional.

use super::*;

use std::sync::Mutex;

use harness_capabilities::{InputBroker, InputError, Service, UserInput, activate};
use promptforge::Prompt;

/// A capability-free prompt returning what `user_input()` answered, with
/// the availability flag beside the text.
const ASKS: &str = "---\nname: asks\ndescription: d\npromptforge: 0\n---\n\n\
    # Title\n\n## Only\n\n```lua\nlocal text, available = user_input()\n\
    return text .. '|' .. tostring(available)\n```\n";

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
            .push(services.input.is_some());
        Ok(Contribution::default())
    }
}

/// Prepares the probe-declaring prompt from a file in `dir` with `input`
/// as the host's broker, and returns what each activation of the probe
/// saw.
async fn probe_activations(dir: &Path, input: Option<Arc<dyn InputBroker>>) -> Vec<bool> {
    let saw_broker = Arc::new(Mutex::new(Vec::new()));
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(Probe {
            id: CapabilityId::parse("tests/probe").unwrap(),
            saw_broker: Arc::clone(&saw_broker),
        }))
        .unwrap();
    let log = log().await;
    let mut services = services(&log, Some(Arc::new(registry)));
    services.input = input;
    prepare_run(&prompt_file(dir, DECLARES_PROBE), "", services)
        .await
        .expect("the prompt prepares");
    saw_broker.lock().unwrap().clone()
}

/// Prepares `source` from a file in `dir` under `services` and drives
/// the run to its end.
async fn drive_prompt(dir: &Path, source: &str, services: Services) -> RunOutcome {
    let log = Arc::clone(&services.log);
    let prepared = prepare_run(&prompt_file(dir, source), "", services)
        .await
        .expect("the prompt prepares");
    drive_run(
        prepared.run,
        prepared.performers,
        log,
        prepared.run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .expect("the loop reaches an outcome")
}

#[tokio::test]
async fn activation_hands_the_hosts_broker_to_each_declared_capability() {
    let dir = tempfile::tempdir().unwrap();
    let seen = probe_activations(dir.path(), Some(Arc::new(Scripted("unused")))).await;
    assert_eq!(
        seen,
        [true],
        "the probe's one activation received the host's broker"
    );
}

#[tokio::test]
async fn activation_hands_no_broker_to_a_capability_when_the_host_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let seen = probe_activations(dir.path(), None).await;
    assert_eq!(
        seen,
        [false],
        "the probe's one activation received no broker"
    );
}

#[tokio::test]
async fn a_user_input_call_is_answered_with_the_brokers_text() {
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let mut services = services(&log, None);
    services.input = Some(Arc::new(Scripted("typed by the operator")));
    let outcome = drive_prompt(dir.path(), ASKS, services).await;
    assert_eq!(
        completed(outcome),
        "typed by the operator|true",
        "the broker's text reaches user_input() byte-exact, marked available"
    );
}

#[tokio::test]
async fn a_user_input_call_without_a_broker_gets_the_unavailable_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let outcome = drive_prompt(dir.path(), ASKS, services(&log, None)).await;
    assert_eq!(
        completed(outcome),
        "User input is unavailable in this host; continue without it.|false",
        "a host with no broker answers the fallback, marked unavailable"
    );
}

#[tokio::test]
async fn a_broker_failure_fails_the_run_with_the_brokers_message_and_cause() {
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let mut services = services(&log, None);
    services.input = Some(Arc::new(Failing));
    let outcome = drive_prompt(dir.path(), ASKS, services).await;
    let RunOutcome::Failed { kind, message } = outcome else {
        panic!("a failed wait fails the run: {outcome:?}");
    };
    assert_eq!(kind, "Input", "the failure is the engine's input failure");
    assert!(
        message.contains("the operator's window closed"),
        "the broker's message reaches the run's failure: {message}"
    );
    assert!(
        message.contains("socket reset"),
        "the broker's hidden cause stays in the chain: {message}"
    );
    assert_eq!(
        message.matches("the operator's window closed").count(),
        1,
        "the message is not repeated along the chain: {message}"
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

/// The preparation services over `log` with the user-input registry and
/// `input` as the host's broker.
fn user_input_services(log: &SharedLog, input: Option<Arc<dyn InputBroker>>) -> Services {
    let mut services = services(log, Some(user_input_registry()));
    services.input = input;
    services
}

#[tokio::test]
async fn a_required_user_input_declaration_on_a_host_without_a_broker_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let error = prepare_run(
        &prompt_file(dir.path(), &user_input_prompt(REQUIRED, ASKS_ONCE)),
        "",
        user_input_services(&log, None),
    )
    .await
    .expect_err("a required capability without its service refuses the run");
    let PrepareError::Refused { error, .. } = error else {
        panic!("the refusal is a requirements refusal: {error}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    assert!(
        error.to_string().contains(
            "- promptforge/user-input needs an input broker, and this host provides none"
        ),
        "the notice names the capability and the missing service: {error}"
    );
}

#[tokio::test]
async fn an_optional_user_input_declaration_without_a_broker_runs_on_the_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let outcome = drive_prompt(
        dir.path(),
        &user_input_prompt(OPTIONAL, ASKS_ONCE),
        user_input_services(&log, None),
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
    assert_eq!(gap.service, Service::Input);
}

#[tokio::test]
async fn an_operator_who_types_the_fallback_sentence_is_still_available() {
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let fallback = "User input is unavailable in this host; continue without it.";
    let outcome = drive_prompt(
        dir.path(),
        &user_input_prompt(REQUIRED, ASKS_ONCE),
        user_input_services(&log, Some(Arc::new(Scripted(fallback)))),
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
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let catches = "local ok, err = pcall(input.ask)\n\
        return tostring(ok) .. '|' .. err.kind .. '|' .. tostring(err)";
    let outcome = drive_prompt(
        dir.path(),
        &user_input_prompt(REQUIRED, catches),
        user_input_services(&log, Some(Arc::new(Failing))),
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
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let outcome = drive_prompt(
        dir.path(),
        &user_input_prompt(REQUIRED, "return (input.ask())"),
        user_input_services(&log, Some(Arc::new(Failing))),
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
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let passes_one = "local ok, err = pcall(input.ask, 1)\n\
        return tostring(ok) .. '|' .. tostring(err)";
    let outcome = drive_prompt(
        dir.path(),
        &user_input_prompt(REQUIRED, passes_one),
        user_input_services(&log, Some(Arc::new(Scripted("unused")))),
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
async fn a_prompt_that_does_not_declare_user_input_has_no_input_global() {
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let reaches = "local ok, err = pcall(function() return input.ask() end)\n\
        return tostring(ok) .. '|' .. tostring(err)";
    let outcome = drive_prompt(
        dir.path(),
        &user_input_prompt("", reaches),
        user_input_services(&log, Some(Arc::new(Scripted("unused")))),
    )
    .await;
    let text = completed(outcome);
    assert!(text.starts_with("false|"), "the call raised: {text}");
    assert!(
        text.contains("attempt to index a nil value (global 'input')"),
        "Lua's own error names the missing global: {text}"
    );
}
