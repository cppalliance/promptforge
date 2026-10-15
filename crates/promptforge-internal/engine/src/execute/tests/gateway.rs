//! The scripted client that fails every round, and the model-failure
//! assertion.

use super::*;

/// A chat client whose every round fails with a broker-reported context
/// overflow, so a test drives the provider overflow path. It counts the
/// rounds it was asked to perform.
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
        Box::pin(async {
            Err(crate::model::CompletionError::context_overflow(
                Some(5000),
                Some(4096),
                "the request is larger than the model's context window",
            ))
        })
    }
}

/// Asserts `error` is a model failure of `kind` whose message is `message`.
#[track_caller]
pub(super) fn assert_model_failure(
    error: &Error,
    kind: crate::model::CompletionErrorKind,
    message: &str,
) {
    let Error::Completion(failure) = error else {
        panic!("expected a model failure, got {error:?}");
    };
    assert_eq!(
        (failure.kind(), failure.to_string().as_str()),
        (kind, message)
    );
}
