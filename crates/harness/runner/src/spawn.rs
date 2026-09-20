//! The harness's one spawn site.
//!
//! Every tokio task the harness starts passes through [`spawn_tagged`] or
//! [`spawn_blocking_tagged`]. Each opens a `tracing` span carrying the
//! effect the task performs - its [`EffectId`] and [`Provenance`] - so a
//! run's tasks trace as a group and slice by task, and each is the single
//! permitted caller of the raw tokio method it wraps.

use promptforge_api_runtime::EffectId;
use promptforge_api_types::ids::Provenance;
use tokio::task::JoinHandle;
use tracing::Instrument;

/// What a spawned task is tagged with: the effect it performs and the
/// provenance the engine stamped on that effect.
pub type Tag = (EffectId, Provenance);

/// Spawn `fut` on the tokio runtime inside a span tagged `tag`.
///
/// The span is named `spawn` and carries the effect id under `effect`,
/// the task path under `task`, and the task-local sequence under `seq`.
/// The future runs to completion or until its [`JoinHandle`] is aborted,
/// exactly as with `tokio::spawn`.
///
/// # Panics
///
/// Panics when called outside a tokio runtime, as `tokio::spawn` does.
#[allow(clippy::disallowed_methods)]
pub fn spawn_tagged<F>(tag: Tag, fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (effect, provenance) = tag;
    let span = tracing::info_span!(
        "spawn",
        effect = %effect,
        task = %provenance.task,
        seq = provenance.seq
    );
    tokio::spawn(fut.instrument(span))
}

/// Run `f` on tokio's blocking pool inside a span tagged `tag`.
///
/// The span is named `spawn_blocking` and carries the same fields as
/// [`spawn_tagged`]'s; it is entered for the whole of `f`. The closure
/// runs to completion even if its [`JoinHandle`] is aborted or dropped,
/// exactly as with `tokio::task::spawn_blocking`.
///
/// # Panics
///
/// Panics when called outside a tokio runtime, as
/// `tokio::task::spawn_blocking` does.
#[allow(clippy::disallowed_methods)]
pub fn spawn_blocking_tagged<F, R>(tag: Tag, f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (effect, provenance) = tag;
    let span = tracing::info_span!(
        "spawn_blocking",
        effect = %effect,
        task = %provenance.task,
        seq = provenance.seq
    );
    tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        f()
    })
}
