//! Section execution.
//!
//! Tranche 1 runs exactly one round trip: take the entry section's prose, send
//! it to the model as a single user message, and return the reply. No tool-call
//! loop, no fall-through to later sections, no Lua. Those arrive in later
//! tranches.

use crate::Result;
use crate::client::{Client, Message};
use crate::parser::Prompt;

/// Execute a prompt's entry section and return the model's text reply.
///
/// # Errors
/// Propagates any [`crate::Error`] from the underlying model call (transport,
/// backend status, or malformed response).
pub async fn run(prompt: &Prompt, client: &Client) -> Result<String> {
    let section = prompt.entry();
    let messages = [Message::user(section.prose.clone())];
    client.complete(&messages).await
}
