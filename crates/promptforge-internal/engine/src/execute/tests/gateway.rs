//! The model that fails every round with a context overflow, as a
//! scenario's counting closure and as the tokio driver's client.

use super::*;

/// The broker-reported context overflow every round of the overflowing
/// model fails with.
fn context_overflow() -> crate::model::CompletionError {
    crate::model::CompletionError::context_overflow(
        Some(5000),
        Some(4096),
        "the request is larger than the model's context window",
    )
}

/// A scenario's model whose every round fails with a broker-reported
/// context overflow, so a test drives the provider overflow path. It
/// counts the rounds it answers in `rounds`.
pub(super) fn overflowing(rounds: &Arc<AtomicUsize>) -> impl Fn(&Effect) -> EffectAnswer + 'static {
    let rounds = Arc::clone(rounds);
    move |_| {
        rounds.fetch_add(1, Ordering::SeqCst);
        EffectAnswer::Chat(Err(context_overflow()))
    }
}

/// [`overflowing`] as a client of the tokio driver, for the suites it
/// still drives. It counts the rounds it was asked to perform.
#[derive(Clone, Default)]
pub(super) struct OverflowClient {
    pub(super) calls: Arc<AtomicUsize>,
}

impl crate::test_support::ChatClient for OverflowClient {
    fn complete(
        &self,
        _messages: Vec<crate::model::Message>,
        _tools: Vec<crate::model::ToolSchema>,
        _options: crate::model::CompletionOptions,
        _limits: RunLimits,
    ) -> crate::test_support::BoxFuture<
        std::result::Result<crate::model::Completion, crate::model::CompletionError>,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(context_overflow()) })
    }
}
