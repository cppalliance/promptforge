//! The sessions layer's spawn sites: a session's supervisor task and a
//! launch's filesystem work on the blocking pool.
//!
//! [`spawn_session`] and [`spawn_blocking_launch`] are the only callers of
//! `tokio::spawn` and `tokio::task::spawn_blocking` in this crate. Each
//! opens a `tracing` span: a supervisor's records the session id, and a
//! launch's records the agent name. Neither performs an effect: a run's
//! effects are polled inside its own future by the Harness.

use tokio::task::JoinHandle;
use tracing::Instrument;

/// Spawns a session's supervisor `fut` inside a span named `session` that
/// records the session id under `session`.
///
/// A supervisor is the one long-lived task this crate starts per session,
/// and each run it drives is polled inside it.
///
/// # Panics
///
/// Panics when called outside a tokio runtime, as `tokio::spawn` does.
#[expect(
    clippy::disallowed_methods,
    reason = "this wrapper is the sessions layer's permitted caller of tokio::spawn"
)]
pub(crate) fn spawn_session<F>(session: &str, fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let span = tracing::info_span!("session", session = %session);
    tokio::spawn(fut.instrument(span))
}

/// Runs `f`, a launch's filesystem work, on tokio's blocking pool inside
/// a span named `launch` that records the agent name under `agent`.
///
/// A launch walks the agents directory and reads the agent's source
/// before any run or session exists. None of that performs an effect, so
/// the agent name is what ties it to the launch that asked. The closure
/// runs to completion even if its [`JoinHandle`] is aborted or dropped,
/// just as with `tokio::task::spawn_blocking`.
///
/// # Panics
///
/// Panics when called outside a tokio runtime, as
/// `tokio::task::spawn_blocking` does.
#[expect(
    clippy::disallowed_methods,
    reason = "this wrapper is the sessions layer's permitted caller of tokio::task::spawn_blocking"
)]
pub(crate) fn spawn_blocking_launch<F, R>(agent: &str, f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let span = tracing::info_span!("launch", agent = %agent);
    tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        f()
    })
}
