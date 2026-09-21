//! Cooperative cancellation for the harness's async session paths: the
//! awaitable [`CancelHandle`] a client selects over, defined in
//! `harness-runner` and re-exported here so that clients name it through
//! this crate.

pub use harness_runner::cancel::{
    CancelHandle, current, is_cancelled, maybe_scope, scope, wait_cancelled,
};
