//! The host's optional input broker at preparation: activation hands it
//! to every declared capability, or hands none when the host has nobody
//! to ask; and a `user_input()` call is answered through it, with the
//! unavailable fallback when there is none.

use super::*;

use std::sync::Mutex;

use harness_capabilities::{InputBroker, InputError};

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
