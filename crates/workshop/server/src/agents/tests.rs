//! The agent sessions' Harness setup: a prompt declaring `promptforge/web`
//! prepares over the server's capabilities and services, a Harness without
//! the search provider refuses it naming that service, and outside a
//! runtime the services leave the runtime out.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use harness::capability::HostServices;
use harness::record::{MemoryRecorder, RunOutcome};
use harness::{
    BoxFuture, CatalogBinding, Harness, HarnessConfig, InferenceBroker, LaunchRequest, OnDelta,
    SessionState,
};
use harness_gateway_client::{CompletionError, CompletionErrorKind};
use harness_web::{SEARCH_PROVIDER, TOKIO_RUNTIME};
use promptforge::model::{
    Completion, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
};
use workshop_registry::Registry;

use super::{capabilities, services};

/// A prompt that requires the web capability and returns a fixed text.
const BROWSES: &str = "---\nname: browses\ndescription: needs web\npromptforge: 0\n\
    capabilities:\n  - promptforge/web\n---\n\n\
    # Browses\n\n## Only\n\n```lua\nreturn 'browsed'\n```\n";

/// Lists no model and refuses every round as `Unavailable`, so a launch
/// binds no model and nothing is fetched.
struct OfflineBroker;

impl InferenceBroker for OfflineBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        Box::pin(async { Ok(ModelCatalog::empty()) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let kind = CompletionErrorKind::Unavailable;
        Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
    }
}

/// A Harness over the server's capabilities and `services`, with `browses`
/// discoverable, the offline broker, and a usable catalog.
fn harness_over(dir: &Path, services: HostServices, recorder: Arc<MemoryRecorder>) -> Harness {
    let agents = dir.join("agents");
    std::fs::create_dir_all(&agents).expect("the agents directory creates");
    std::fs::write(agents.join("browses.md"), BROWSES).expect("the agent writes");
    let harness = Harness::new(
        HarnessConfig {
            agents_path: agents,
        },
        recorder,
        Arc::new(OfflineBroker),
        capabilities(),
        services,
    );
    harness.set_catalog(CatalogBinding {
        generation: 1,
        models: vec![serde_json::json!({ "kind": "chat" })],
    });
    harness
}

/// Launches `browses`, waits for its session to close, and returns the
/// outcome of its one run.
async fn browse(harness: &Harness, recorder: &MemoryRecorder) -> Option<RunOutcome> {
    let session = harness
        .launch(LaunchRequest {
            agent: "browses".to_owned(),
            args: String::new(),
            input_text: None,
        })
        .await
        .expect("the discovered agent launches");
    let mut state = session.subscribe_state();
    tokio::time::timeout(
        Duration::from_secs(10),
        state.wait_for(|current| *current == SessionState::Closed),
    )
    .await
    .expect("the session closes in time")
    .expect("the session's state watch stays open");
    let runs = session.run_ids();
    assert_eq!(runs.len(), 1, "one run ends the session");
    recorder.outcome(runs[0])
}

#[tokio::test]
async fn a_prompt_declaring_web_prepares_on_the_servers_capabilities_and_services() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = harness_over(
        dir.path(),
        services(&Registry::new()),
        Arc::clone(&recorder),
    );

    assert_eq!(
        browse(&harness, &recorder).await,
        Some(RunOutcome::Completed {
            final_text: "browsed".to_owned(),
        }),
        "the server registers web and provides both services it reads"
    );
}

#[tokio::test]
async fn a_harness_without_the_search_provider_refuses_a_prompt_requiring_web() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let recorder = Arc::new(MemoryRecorder::new());
    let mut runtime_only = HostServices::new();
    runtime_only
        .provide(&TOKIO_RUNTIME, Arc::new(tokio::runtime::Handle::current()))
        .expect("an empty map takes the runtime");
    let harness = harness_over(dir.path(), runtime_only, Arc::clone(&recorder));

    let outcome = browse(&harness, &recorder).await;
    let Some(RunOutcome::Failed { kind, message }) = outcome else {
        panic!("the run is refused as it prepares: {outcome:?}");
    };
    assert_eq!(kind, "RequirementsUnmet");
    assert!(
        message.contains("promptforge/web") && message.contains("promptforge/search-provider"),
        "the refusal names the capability and the missing service: {message}"
    );
}

#[test]
fn outside_a_runtime_the_services_leave_the_runtime_out() {
    let services = services(&Registry::new());
    assert!(services.provides(&SEARCH_PROVIDER.id()));
    assert!(
        !services.provides(&TOKIO_RUNTIME.id()),
        "a synchronous caller has no runtime to provide"
    );
}
