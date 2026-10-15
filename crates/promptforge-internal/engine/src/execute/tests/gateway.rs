//! The model that fails every round with a context overflow, as a
//! scenario's counting closure.

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
