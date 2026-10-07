//! The agent conversations' Harness setup: a prompt declaring
//! `web` prepares over the server's Plugins and
//! services, a Harness without the search provider refuses it naming
//! that service, and outside a runtime the services leave the runtime
//! out.

use std::sync::Arc;
use std::time::Duration;

use harness::plugin::HostServices;
use harness::record::{MemoryRecorder, RunOutcome};
use harness::vfs::VfsRef;
use harness::{BoxFuture, Harness, HostSnapshot, InferenceBroker, RunRequest};
use harness_gateway_client::{CompletionError, CompletionErrorKind};
use harness_web::{SEARCH_PROVIDER, TOKIO_RUNTIME};
use promptforge::effect::Round;
use promptforge::model::{
    Completion, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
};
use workshop_agents::{Conversations, SessionState, TokioTimer};
use workshop_registry::Registry;

use super::{plugins, services};

/// A prompt that requires the web Plugin and returns a fixed text.
const BROWSES: &str = "---\nname: browses\ndescription: needs web\npromptforge: 0\n\
    plugins:\n  - web\n---\n\n\
    # Browses\n\n## Only\n\n```lua\nreturn 'browsed'\n```\n";

/// Lists no model and refuses every round as `Unavailable`, so a run
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
        _round: Round,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let kind = CompletionErrorKind::Unavailable;
        Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
    }
}

/// Runs `browses` as one conversation over the server's Plugins and
/// `services`, then reads the conversation's state and the outcome of its
/// one run.
async fn browse(services: &HostServices) -> Option<RunOutcome> {
    let recorder = Arc::new(MemoryRecorder::new());
    let conversation = Conversations::new().open("browses");
    let harness = Harness::new(
        conversation.recorder(recorder.clone()),
        Arc::new(OfflineBroker),
        Arc::new(TokioTimer),
        plugins(),
        conversation.services(services),
    );
    let request = RunRequest {
        name: conversation.id().to_string(),
        source: BROWSES.to_owned(),
        args: String::new(),
        input_text: None,
        vfs: VfsRef::default(),
        host: HostSnapshot::default(),
    };
    tokio::time::timeout(Duration::from_secs(10), conversation.run(harness, request))
        .await
        .expect("the run ends in time");
    assert_eq!(
        conversation.state(),
        SessionState::Closed,
        "the conversation ends with its run"
    );
    recorder.outcome(conversation.run_id().expect("the recorder began the run"))
}

#[tokio::test]
async fn a_prompt_declaring_web_prepares_on_the_servers_plugins_and_services() {
    assert_eq!(
        browse(&services(&Registry::new())).await,
        Some(RunOutcome::Completed {
            final_text: "browsed".to_owned(),
        }),
        "the server registers web and provides both services it reads"
    );
}

#[tokio::test]
async fn a_harness_without_the_search_provider_refuses_a_prompt_requiring_web() {
    let mut runtime_only = HostServices::new();
    runtime_only
        .provide(&TOKIO_RUNTIME, Arc::new(tokio::runtime::Handle::current()))
        .expect("an empty map takes the runtime");

    let outcome = browse(&runtime_only).await;
    let Some(RunOutcome::Failed { kind, message }) = outcome else {
        panic!("the run is refused as it prepares: {outcome:?}");
    };
    assert_eq!(kind, "RequirementsUnmet");
    assert!(
        message.contains("web") && message.contains("promptforge/search-provider"),
        "the refusal names the Plugin and the missing service: {message}"
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
