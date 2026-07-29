//! Section execution and fall-through.
//!
//! The run walks the top-level sections in file order, each in a fresh context.
//! For each section: run its Lua block; if the chunk returns a plain value that
//! value is the run's result and the run ends immediately (this doubles as the
//! return fence - sections after it are not reached by fall-through). Otherwise
//! the section's prose is `{{ }}`-substituted (over `args`, the `var` the block
//! wrote, and the runtime `sys`) and, if non-empty, sent to the gateway for one
//! round trip; then control falls through to the next section.
//!
//! Running off the last section ends the run: the result is `default_return`
//! from the frontmatter, else the last model reply, else a generic completion.
//!
//! Still to come: the other exit cases (a descriptor = goto/task/fanout), the
//! tool-call loop, and durable state to carry a non-terminal section's work
//! forward (today an intermediate section's model reply is not retained).

use serde_json::json;

use crate::client::{CompletionResult, GatewayClient, Message};
use crate::lua;
use crate::parser::Prompt;
use crate::subst;
use crate::{Error, Result};

/// Execute a prompt and return the run's result.
///
/// `args` is the single raw input string, exposed to Lua and to `{{ args }}`.
///
/// # Errors
/// Returns [`crate::Error::Lua`] if a Lua block fails,
/// [`crate::Error::Substitution`] if a `{{ }}` path cannot be resolved,
/// [`crate::Error::MissingEnv`] if the gateway client cannot be built when a
/// model call is needed, or any transport/backend error from a model call.
pub async fn run(prompt: &Prompt, args: &str) -> Result<String> {
    let when = now_rfc3339();
    let mut client: Option<GatewayClient> = None;
    let mut last_reply: Option<String> = None;

    for (index, section) in prompt.sections.iter().enumerate() {
        let sys = json!({ "when": when, "now": now_rfc3339(), "id": index + 1 });

        // Run the section's Lua block. A returned value ends the whole run.
        let var = if let Some(source) = &section.lua {
            let outcome = lua::run_chunk(source, args, &sys)?;
            if let Some(value) = outcome.returned {
                return Ok(value);
            }
            outcome.var
        } else {
            json!({})
        };

        // Substitute the prose; if there is any, take one model round trip.
        let prose = subst::substitute(&section.prose, args, &var, &sys)?;
        if !prose.trim().is_empty() {
            if client.is_none() {
                client = Some(GatewayClient::from_env()?);
            }
            if let Some(client) = &client {
                let text = match client.complete(&[Message::user(prose)], None).await? {
                    CompletionResult::Text(text) => text,
                    CompletionResult::ToolCalls(_) => {
                        return Err(Error::MalformedResponse(
                            "tool calls not supported without tools".into(),
                        ));
                    }
                };
                last_reply = Some(text);
            }
        }
        // Fall through to the next section (context clears - nothing is carried).
    }

    // Ran off the end.
    Ok(prompt
        .frontmatter
        .default_return
        .clone()
        .or(last_reply)
        .unwrap_or_else(|| "done".to_string()))
}

/// The current UTC time as an RFC 3339 string, or empty on a formatting error.
fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lua-only prompts never build the gateway client, so these run offline.
    fn parse(md: &str) -> Prompt {
        Prompt::parse(md).unwrap()
    }

    #[tokio::test]
    async fn falls_through_to_next_section() {
        let md = "---\nname: t\ndescription: d\nversion: 1\n---\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn \"second\"\n```\n";
        let out = run(&parse(md), "").await.unwrap();
        assert_eq!(out, "second");
    }

    #[tokio::test]
    async fn explicit_return_stops_fall_through() {
        let md = "---\nname: t\ndescription: d\nversion: 1\n---\n\n\
## First\n\n```lua\nreturn \"first\"\n```\n\n\
## Second\n\n```lua\nreturn \"unreached\"\n```\n";
        let out = run(&parse(md), "").await.unwrap();
        assert_eq!(out, "first");
    }

    #[tokio::test]
    async fn runs_off_end_to_default_return() {
        let md = "---\nname: t\ndescription: d\nversion: 1\ndefault_return: \"fell off\"\n---\n\n\
## Only\n\n```lua\nlocal x = 1\n```\n";
        let out = run(&parse(md), "").await.unwrap();
        assert_eq!(out, "fell off");
    }

    #[tokio::test]
    async fn generic_result_when_nothing_produced() {
        let md = "---\nname: t\ndescription: d\nversion: 1\n---\n\n\
## Only\n\n```lua\nlocal x = 1\n```\n";
        let out = run(&parse(md), "").await.unwrap();
        assert_eq!(out, "done");
    }

    #[tokio::test]
    async fn sys_id_increments_per_section() {
        // First section files nothing and falls through; second returns its id.
        let md = "---\nname: t\ndescription: d\nversion: 1\n---\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn tostring(sys.id)\n```\n";
        let out = run(&parse(md), "").await.unwrap();
        assert_eq!(out, "2");
    }
}
