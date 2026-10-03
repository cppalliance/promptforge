//! What every suite Host hands a Harness beside its recorder: an offline
//! broker and a tokio-backed timer.

use std::time::Duration;

use harness::{BoxFuture, InferenceBroker, Timer};
use promptforge::effect::Round;
use promptforge::model::{
    Completion, CompletionError, CompletionErrorKind, CompletionOptions, Message, ModelBinding,
    ModelCatalog, ToolSchema,
};

/// Lists no model and refuses every round as `Unavailable`.
pub(crate) struct Offline;

impl InferenceBroker for Offline {
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

/// Sleeps on tokio's timer, as a Host on tokio does.
pub(crate) struct Clock;

impl Timer for Clock {
    fn sleep(&self, seconds: f64) -> BoxFuture<()> {
        let duration = Duration::try_from_secs_f64(seconds).unwrap_or(Duration::ZERO);
        Box::pin(tokio::time::sleep(duration))
    }
}
