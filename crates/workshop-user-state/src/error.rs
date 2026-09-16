//! The user-state operation failure type.
//!
//! [`UserStateError`] is the boundary between the store's zone-two
//! refusals and its caller: a put names the key it refused or the size
//! it exceeded, and a failed write surfaces the I/O cause.

use std::io;

use crate::store::USER_STATE_KEYS;

/// A user-state operation failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UserStateError {
    /// A put named a key outside the allow-list. The message lists the
    /// allow-list itself so it cannot drift from the keys.
    #[error(
        "user-state key {0:?} is not allowed; one of {allowed} is required",
        allowed = USER_STATE_KEYS.join(", ")
    )]
    Key(String),

    /// A value's JSON text exceeds the size cap.
    #[non_exhaustive]
    #[error("user-state value is {actual} bytes; at most {cap} bytes are allowed")]
    TooLarge {
        /// The size of the refused value.
        actual: usize,
        /// The cap it exceeded.
        cap: usize,
    },

    /// The state file could not be written.
    #[error("user-state file cannot be written")]
    Io(#[source] io::Error),
}
