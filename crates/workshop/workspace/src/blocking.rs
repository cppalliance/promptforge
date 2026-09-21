//! The blocking-pool seam for the workspace's filesystem work.
//!
//! Every `async fn` in this crate that touches the disk directly - a
//! stat, a canonicalize, a read, a copy, a pointer write - runs that
//! work through one of the two helpers here so a tokio executor thread
//! never waits on I/O. Turso's own database I/O is async-native and does
//! not come through here. A worker that panics, or is cancelled at
//! runtime shutdown, surfaces as an `io::Error` holding the join
//! failure, the same shape `workshop-user-state` gives its atomic write.

use std::io;

/// Runs `work`, synchronous filesystem work, on tokio's blocking pool
/// and hands back what it returned; a join failure is the `Err`.
pub(crate) async fn blocking<T>(work: impl FnOnce() -> T + Send + 'static) -> io::Result<T>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(io::Error::other)
}

/// [`blocking`] for work that itself fails with `E`: the worker's own
/// failure passes through, and a join failure is folded into `E` by
/// `join_failed`, so the caller sees one error type.
pub(crate) async fn try_blocking<T, E>(
    work: impl FnOnce() -> Result<T, E> + Send + 'static,
    join_failed: impl FnOnce(io::Error) -> E,
) -> Result<T, E>
where
    T: Send + 'static,
    E: Send + 'static,
{
    match blocking(work).await {
        Ok(result) => result,
        Err(source) => Err(join_failed(source)),
    }
}
