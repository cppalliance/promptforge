//! The harness's one spawn site.
//!
//! Every tokio task the harness starts passes through [`spawn_tagged`] or
//! [`spawn_blocking_tagged`]. Each opens a `tracing` span carrying the
//! caller's tag (an `EffectId` and `Provenance` once the effect loop lands)
//! so a run's tasks trace as a group, and each is the single permitted
//! caller of the raw tokio method it wraps.

use std::fmt::Display;

use tokio::task::JoinHandle;
use tracing::Instrument;

/// Spawn `fut` on the tokio runtime inside a span tagged `tag`.
///
/// The span is named `spawn` and carries the tag under the `tag` field.
/// The future runs to completion or until its [`JoinHandle`] is aborted,
/// exactly as with `tokio::spawn`.
///
/// # Panics
///
/// Panics when called outside a tokio runtime, as `tokio::spawn` does.
#[allow(clippy::disallowed_methods)]
pub fn spawn_tagged<T, F>(tag: T, fut: F) -> JoinHandle<F::Output>
where
    T: Display,
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let span = tracing::info_span!("spawn", tag = %tag);
    tokio::spawn(fut.instrument(span))
}

/// Run `f` on tokio's blocking pool inside a span tagged `tag`.
///
/// The span is named `spawn_blocking` and carries the tag under the `tag`
/// field; it is entered for the whole of `f`. The closure runs to
/// completion even if its [`JoinHandle`] is dropped, exactly as with
/// `tokio::task::spawn_blocking`.
///
/// # Panics
///
/// Panics when called outside a tokio runtime, as
/// `tokio::task::spawn_blocking` does.
#[allow(clippy::disallowed_methods)]
pub fn spawn_blocking_tagged<T, F, R>(tag: T, f: F) -> JoinHandle<R>
where
    T: Display,
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let span = tracing::info_span!("spawn_blocking", tag = %tag);
    tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        f()
    })
}
