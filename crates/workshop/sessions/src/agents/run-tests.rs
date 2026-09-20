use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use harness_api::bridge::InputBroker;
use promptforge_api_runtime::execute::{RunError, RunErrorKind};
use promptforge_api_runtime::input::{InputError, InputOutcome};
use promptforge_api_types::models::{ModelDescriptor, ModelId, ThinkingMode};
use tokio::sync::{broadcast, mpsc};

use workshop_gateway::WorkshopObserver;
use workshop_registry::Registry;
use workshop_support::ReconnectBackoff;

use super::*;
use crate::agents::lifecycle::{CANCELLATION_CAPACITY, RunLifecycle};
use crate::agents::{BUILTIN_CHAT_SOURCE, ERROR_CAPACITY, agent_client, session_registry};

/// The descriptor the chat unit runs bind the declared `chat` role to; its
/// window clears the role's declared minimum.
fn test_model() -> ModelDescriptor {
    ModelDescriptor::new(
        ModelId::gateway("test-model").expect("the test model id is valid"),
        "test model",
        NonZeroU32::new(200_000).expect("200000 is non-zero"),
        ThinkingMode::Never,
    )
}

/// The unavailable-fallback policy as a broker: every wait resumes
/// unavailable, so the built-in chat returns without a model call.
struct UnavailableBroker;

#[async_trait::async_trait]
impl InputBroker for UnavailableBroker {
    async fn user_input(
        &self,
        _execution: &str,
        _section: &str,
    ) -> Result<InputOutcome, InputError> {
        Ok(InputOutcome::Unavailable)
    }
}

/// A broker whose every wait fails: the failure policy.
struct FailingBroker;

#[async_trait::async_trait]
impl InputBroker for FailingBroker {
    async fn user_input(
        &self,
        _execution: &str,
        _section: &str,
    ) -> Result<InputOutcome, InputError> {
        Err(InputError::message("the input device is gone"))
    }
}

/// A sink over fresh session plumbing with no listeners: the run's events
/// land in the returned log.
fn silent_sink() -> (SessionSink, Arc<WorkshopObserver>) {
    let registry = Registry::new();
    let (errors, _) = broadcast::channel(ERROR_CAPACITY);
    let (supervisor_events, _events) = mpsc::unbounded_channel();
    let (cancellations, _cancellation_events) = mpsc::channel(CANCELLATION_CAPACITY);
    let log = Arc::new(WorkshopObserver::new());
    let sink = SessionSink {
        log: Arc::clone(&log),
        rounds: Arc::new(AtomicU64::new(0)),
        push: registry.push(),
        backoff: ReconnectBackoff::new(),
        errors,
        lifecycle: Arc::new(RunLifecycle::new(supervisor_events, cancellations)),
    };
    (sink, log)
}

/// Runs the embedded chat prompt through the session's own run path with
/// the given broker and model, against a client no model call can survive.
/// The registry carries the first-party capabilities exactly as the
/// session wiring builds them, so the prompt's declared contract is
/// activated, prepared, and checked the production way.
async fn run_builtin_chat(
    broker: Arc<dyn InputBroker>,
    model: ModelDescriptor,
) -> (Result<(), AgentRunError>, Arc<WorkshopObserver>) {
    let (sink, log) = silent_sink();
    let registry = session_registry("http://127.0.0.1:9", "k")
        .expect("a well-shaped gateway root builds the session registry");
    let client = agent_client("http://127.0.0.1:9", "k").expect("the test client builds");
    let parts = RunParts {
        sink,
        broker,
        ui: serde_json::json!({ "selected_model": null, "workspace_root": null }),
        seed: 1,
        started_at: Timestamp::UNIX_EPOCH,
        on_delta: Arc::new(|_| {}),
        model: Some(model),
        execution: "chat-unit".to_owned(),
        cancel: harness_api::cancel::CancelHandle::new(),
    };
    let result = run_markdown_agent(BUILTIN_CHAT_SOURCE, parts, client, Arc::new(registry)).await;
    (result, log)
}

/// The engine error behind a failed run, when the failure carried one.
fn run_error(error: &AgentRunError) -> Option<&RunError> {
    match error {
        AgentRunError::Failed {
            source: Some(source),
            ..
        } => source.downcast_ref::<RunError>(),
        _ => None,
    }
}

#[test]
fn the_builtin_chat_declares_its_contract_in_frontmatter() {
    let prompt = Prompt::parse(BUILTIN_CHAT_SOURCE, "chat-unit")
        .0
        .expect("the embedded chat prompt parses");
    let frontmatter = prompt.frontmatter();
    let capabilities = frontmatter.capabilities();
    assert_eq!(
        capabilities.len(),
        1,
        "chat declares exactly one capability"
    );
    assert_eq!(capabilities[0].id().to_string(), "promptforge/web");
    assert!(
        !capabilities[0].is_optional(),
        "the built-in host always installs its own web capability"
    );
    let tools = frontmatter.tools();
    assert_eq!(tools.len(), 2, "both web tools get exact slots");
    assert!(tools.get("fetch").is_some(), "the fetch slot is declared");
    assert!(tools.get("search").is_some(), "the search slot is declared");
    let chat = frontmatter
        .models()
        .get("chat")
        .expect("the chat role is declared for the host's current model");
    assert_eq!(
        chat.min_context(),
        NonZeroU32::new(32768),
        "the role declares the window its web tools need, so an undersized \
         binding is refused at prepare instead of failing mid-conversation"
    );
}

#[tokio::test]
async fn an_undersized_model_is_refused_before_the_first_turn() {
    // The scenario this pins: a launch that bound a small descriptor (the
    // catalog fetch's fallback window) used to run, then fail the first
    // conversation that outgrew it. The declared minimum turns that into a
    // refusal at prepare naming the role.
    let small = ModelDescriptor::new(
        ModelId::gateway("small-model").expect("the test model id is valid"),
        "small model",
        NonZeroU32::new(8192).expect("8192 is non-zero"),
        ThinkingMode::Never,
    );
    let (result, log) = run_builtin_chat(Arc::new(UnavailableBroker), small).await;
    let error = result.expect_err("an 8192-token model cannot satisfy the chat role");
    assert_eq!(
        run_error(&error).map(RunError::kind),
        Some(RunErrorKind::RequirementsUnmet),
        "the refusal is the engine's typed requirements error: {error}"
    );
    let notice = error.to_string();
    assert!(
        notice.contains("chat") && notice.contains("32768") && notice.contains("8192"),
        "the notice names the role, the minimum, and the actual window: {notice}"
    );
    assert!(log.is_empty(), "a refused run reports nothing");
}

#[tokio::test]
async fn the_builtin_chat_returns_without_input_beneath_it() {
    // The unavailable fallback: user_input() resumes unavailable, the
    // prompt returns, and no model call is ever attempted (the client
    // points at a closed port, so an attempt would fail the run).
    let (result, _log) = run_builtin_chat(Arc::new(UnavailableBroker), test_model()).await;
    assert!(
        result.is_ok(),
        "the unavailable fallback ends the run cleanly: {result:?}"
    );
}

#[tokio::test]
async fn a_failing_broker_fails_the_builtin_chat_as_typed_input() {
    let (result, _log) = run_builtin_chat(Arc::new(FailingBroker), test_model()).await;
    let error = result.expect_err("the broker failure fails the run");
    assert_eq!(
        run_error(&error).map(RunError::kind),
        Some(RunErrorKind::Input),
        "a broker failure is the typed input failure: {error}"
    );
}

#[tokio::test]
async fn a_cancelled_token_interrupts_the_run() {
    // A broker that never answers parks the run on its input wait; firing
    // the session's token must end it as an interruption, never a failure.
    struct ParkedBroker;

    #[async_trait::async_trait]
    impl InputBroker for ParkedBroker {
        async fn user_input(
            &self,
            _execution: &str,
            _section: &str,
        ) -> Result<InputOutcome, InputError> {
            std::future::pending().await
        }
    }

    let (sink, _log) = silent_sink();
    let registry = session_registry("http://127.0.0.1:9", "k")
        .expect("a well-shaped gateway root builds the session registry");
    let client = agent_client("http://127.0.0.1:9", "k").expect("the test client builds");
    let token = harness_api::cancel::CancelHandle::new();
    let parts = RunParts {
        sink,
        broker: Arc::new(ParkedBroker),
        ui: serde_json::json!({ "selected_model": null, "workspace_root": null }),
        seed: 1,
        started_at: Timestamp::UNIX_EPOCH,
        on_delta: Arc::new(|_| {}),
        model: Some(test_model()),
        execution: "chat-unit".to_owned(),
        cancel: token.clone(),
    };
    let run = tokio::spawn(run_markdown_agent(
        BUILTIN_CHAT_SOURCE,
        parts,
        client,
        Arc::new(registry),
    ));
    tokio::task::yield_now().await;
    token.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), run)
        .await
        .expect("the cancelled run ends promptly")
        .expect("the run task joins");
    assert!(
        matches!(result, Err(AgentRunError::Interrupted)),
        "cancellation is a stop reason, never a failure: {result:?}"
    );
}
