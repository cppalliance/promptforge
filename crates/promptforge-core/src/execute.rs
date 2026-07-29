//! Section execution.
//!
//! Commit 1 (echo) implements the *finish* case of the Lua exit rule: the entry
//! section's Lua chunk runs first, and if it returns a plain value that value is
//! the run's result (no model call). If there is no Lua block, or the chunk
//! returns nothing, the section falls back to the existing single round trip:
//! send the prose to the gateway and return the reply.
//!
//! The other exit cases (nil = fall through to the next section, a descriptor =
//! goto/task/fanout) and `{{ }}` substitution arrive in later commits.

use crate::Result;
use crate::client::{GatewayClient, Message};
use crate::lua;
use crate::parser::Prompt;

/// Execute a prompt's entry section and return the run's result.
///
/// `args` is the single raw input string, exposed to the section's Lua block.
///
/// # Errors
/// Returns [`crate::Error::Lua`] if the Lua block fails, [`crate::Error::MissingEnv`]
/// if the gateway client cannot be built when a model call is needed, or any
/// transport/backend error from the model call.
pub async fn run(prompt: &Prompt, args: &str) -> Result<String> {
    let section = prompt.entry();

    // Finish case of the exit rule: a Lua chunk that returns a value ends the run.
    if let Some(source) = &section.lua
        && let Some(value) = lua::run_chunk(source, args)?
    {
        return Ok(value);
    }

    // No Lua, or the chunk returned nothing: send the prose to the model.
    // The client is built lazily so a Lua-only run needs no gateway credentials.
    let client = GatewayClient::from_env()?;
    let messages = [Message::user(section.prose.clone())];
    client.complete(&messages).await
}
