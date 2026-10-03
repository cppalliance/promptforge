//! The OpenAI chat-completions wire code: the request body builder, the
//! read loop over a caller's chunk source and the live pieces it hands
//! out, the SSE reassembly and the strict turn parse it finishes through,
//! and the failure classifier.
//!
//! The scanner and accumulator in [`stream`] and the body walk in [`parse`]
//! stay crate-private, reached only through the read loop, so every
//! completion this crate returns passed the one byte cap, the one
//! `[DONE]` rule, and the one turn rule set.

pub(crate) mod classify;
pub(crate) mod delta;
pub(crate) mod parse;
pub(crate) mod read;
pub(crate) mod request;
pub(crate) mod stream;
