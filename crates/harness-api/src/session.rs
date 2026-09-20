//! The session vocabulary clients speak and render - ids, launch
//! requests, durable events, and ephemeral deltas - and the live
//! [`Session`] handle a client launches, sends input to, cancels, closes,
//! and subscribes to events and deltas through. A session's transcript is
//! the harness run log: subscribe first, then read
//! [`Session::transcript`] past the last seen index.
//!
//! The wait frames a session announces its input waits with, and the
//! error a refused answer returns, are the wait registry's own.

pub use harness_sessions::input::{WaitError, WaitFrame};
pub use harness_sessions::protocol::{Delta, DeltaKind, LaunchRequest, SessionEvent, SessionId};
pub use harness_sessions::session::Session;
pub use harness_sessions::transition::SessionState;
