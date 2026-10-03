//! Tests for the Host snapshot and for model resolution with nothing to
//! resolve.

use harness_runner::performers::{BoxFuture, OnDelta};
use promptforge::model::{
    Completion, CompletionErrorKind, CompletionOptions, Message, ModelBinding, ModelCatalog,
    ToolSchema,
};

use super::*;

/// Lists no model; a round is never made.
struct EmptyBroker;

impl InferenceBroker for EmptyBroker {
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

#[test]
fn the_host_snapshot_serves_the_first_root_and_the_selection() {
    let host = HostSnapshot {
        selected_model: Some("gpt".to_owned()),
        workspace_roots: vec![PathBuf::from("/w/one"), PathBuf::from("/w/two")],
    };
    let ui = host.ui();
    assert_eq!(ui["selected_model"], "gpt");
    assert_eq!(
        ui["workspace_root"],
        PathBuf::from("/w/one").display().to_string()
    );
    let empty = HostSnapshot::default().ui();
    assert!(empty["selected_model"].is_null());
    assert!(empty["workspace_root"].is_null());
}

#[tokio::test]
async fn no_selection_and_an_empty_broker_catalog_bind_no_model() {
    // No selection and a broker that lists nothing: nothing to resolve,
    // so the roles stay unbound rather than the run failing.
    let (_, model) = current_model(&Bindings::new(), &EmptyBroker)
        .await
        .expect("an empty list is not a failure");
    assert!(model.is_none());
}
