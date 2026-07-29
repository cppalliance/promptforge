//! Section execution.
//!
//! The entry section's Lua chunk runs first. If it returns a plain value, that
//! value is the run's result (the finish case of the exit rule, no model call).
//! Otherwise the harness resolves `{{ }}` substitutions in the prose - over
//! `args`, the `var` table the block populated, and the runtime `sys` table -
//! and sends the substituted prose to the gateway for one round trip.
//!
//! The remaining exit cases (nil = fall through, a descriptor = goto/task/fanout)
//! and the tool-call loop arrive in later commits.

use serde_json::{Value as Json, json};

use crate::Result;
use crate::client::{GatewayClient, Message};
use crate::lua;
use crate::parser::Prompt;
use crate::subst;

/// Execute a prompt's entry section and return the run's result.
///
/// `args` is the single raw input string, exposed to Lua and to `{{ args }}`.
///
/// # Errors
/// Returns [`crate::Error::Lua`] if the Lua block fails,
/// [`crate::Error::Substitution`] if a `{{ }}` path cannot be resolved,
/// [`crate::Error::MissingEnv`] if the gateway client cannot be built when a
/// model call is needed, or any transport/backend error from the model call.
pub async fn run(prompt: &Prompt, args: &str) -> Result<String> {
    let section = prompt.entry();
    let sys = build_sys();

    // Run the section's Lua block (if any): a returned value finishes the run;
    // otherwise the block has populated `var` for substitution.
    let var = if let Some(source) = &section.lua {
        let outcome = lua::run_chunk(source, args, &sys)?;
        if let Some(value) = outcome.returned {
            return Ok(value);
        }
        outcome.var
    } else {
        json!({})
    };

    // Resolve {{ args / var / sys }} in the prose, then one model round trip.
    let prose = subst::substitute(&section.prose, args, &var, &sys)?;
    let client = GatewayClient::from_env()?;
    let messages = [Message::user(prose)];
    client.complete(&messages).await
}

/// Build the runtime `sys` table for this run: launch timestamp, current time,
/// and the context id. For a single-section run `when` and `now` coincide and
/// `id` is 1; multi-section flow will differentiate them in a later commit.
fn build_sys() -> Json {
    let stamp = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    json!({ "when": stamp, "now": stamp, "id": 1 })
}
