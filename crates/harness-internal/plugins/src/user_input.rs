//! The `user-input` Plugin: a prompt section pauses and asks the
//! operator for their next message.
//!
//! A prompt declares the Plugin in its frontmatter, required or
//! optional, and its sections then have an `input` table with two
//! functions:
//!
//! ````text
//! plugins:
//!   - user-input
//! ````
//!
//! ````lua
//! local text, available = input.ask()
//! ````
//!
//! A prompt that does not declare the Plugin has no `input` global.
//! By the end of this page you know what the Harness supplies for the
//! Plugin, what the script receives with and without an operator,
//! how a failed wait reaches the script, and how a prompt lets its model
//! ask.
//!
//! # Where this fits
//!
//! The Plugin contributes one tool, the ask tool, under the full id
//! [`USER_INPUT_ASK_TOOL`], and a prelude: the Lua source that defines
//! `input` in every section of the run. `input.ask()` calls the ask tool
//! by its full id, so every ask reaches the Harness as an ordinary
//! `ToolCall` effect that its tool performer runs. The ask tool waits
//! on the run's [`InputBroker`] under [`INPUT_BROKER`], the part of
//! the Host that carries a question to a person. Each run gets a broker
//! bound to the session that launched it, so the question reaches the
//! right operator without naming the run or the section.
//!
//! Whether a broker is present is fixed when the Plugin activates: a
//! broker that is present stays present for the whole run. The prelude
//! records that fact, so `input.connected()` answers it without a tool
//! call, and `input.ask()` returns it as its second value, `available`.
//!
//! # Answering with operator text
//!
//! When the Host has an operator, it supplies a broker among its services
//! with [`HostServices::provide`](crate::HostServices::provide) under
//! [`INPUT_BROKER`], and the Harness hands those services to the run. Each
//! `input.ask()` waits on [`InputBroker::wait`], and the script receives
//! the operator's text byte-exact with `available` set to `true`.
//! Operator input is trusted, so the ask tool answers with
//! [`ToolOutput::trusted`] and the text is never guard-wrapped.
//!
//! # Answering without an operator
//!
//! For a Host with nobody to ask, such as a batch or eval Host, the
//! Harness supplies no broker. What happens then depends on how the
//! prompt declared the Plugin.
//!
//! - A required declaration is refused before the run starts. The
//!   Plugin's [`needs`](Plugin::needs) names
//!   [`INPUT_BROKER`], so activation never calls
//!   [`create`](Plugin::create) and the refusal notice holds the line
//!   "- user-input needs promptforge/input-broker, and the environment
//!   provides none".
//! - An optional declaration activates anyway, and the activation
//!   records a [`ServiceGap`](crate::ServiceGap). `input.connected()`
//!   returns `false`. Each `input.ask()` still issues the tool call, so
//!   the Host sees every ask, and returns the fixed sentence "User input
//!   is unavailable in this host; continue without it." with `available`
//!   set to `false`.
//!
//! **Branch on the flag.** A prompt tells real input from the fallback
//! by `available` or `input.connected()`, not by the text. An operator
//! who types exactly the fallback sentence still reports `available` as
//! `true`, so an operator cannot fake the unavailable state.
//!
//! # Failed and cancelled waits
//!
//! A broker that returns an [`InputError`] fails the ask tool's call with
//! a [`ToolError`] that carries the broker's message and, when the broker
//! gave one, its hidden cause. The failure raises at the script's
//! `input.ask()` call as a Lua error of kind `"tool"` whose message is
//! the broker's message, so `pcall` catches it. Uncaught, it ends the run
//! with [`RunErrorKind::Tool`](promptforge::RunErrorKind::Tool). The
//! cause stays on the Rust side and never reaches the prompt, but the
//! message does, where the model can read it too, so a broker writes it
//! for that audience.
//!
//! Cancelling the run drops the ask tool's pending wait, and the
//! [`InputBroker`] contract says a dropped wait must not leave the
//! operator prompting against it.
//!
//! # Prompt-side rules
//!
//! - A prompt gets `input` only by declaring `user-input`.
//!   Without the declaration, `input.ask()` fails with Lua's own
//!   "attempt to index a nil value (global 'input')".
//! - `input.ask()` takes no arguments. Calling it with any argument
//!   raises a Lua error with the message `input.ask takes no arguments`,
//!   so a prompt that passes a question fails loudly instead of losing
//!   it.
//! - The model can ask the operator only when the prompt opts in: it
//!   binds the ask tool under an alias in its `tools:` frontmatter, for
//!   example `ask: user-input/ask`, and advertises that alias
//!   with `tools.add` or `tools.always`. Declaring the Plugin alone
//!   advertises nothing to the model.

use std::sync::Arc;

use promptforge::plugins::PluginId;
use promptforge::tools::{ToolError, ToolErrorKind, ToolId, ToolOutput};

use crate::input::{InputBroker, InputError};
use crate::plugin::{Contribution, Plugin, PluginError, RunServices};
use crate::service::{ServiceId, ServiceKey};
use crate::tool::{Tool, ToolContext};

#[cfg(test)]
#[path = "user_input-tests.rs"]
mod tests;

/// The full tool id of the ask tool, which asks the operator for their
/// next message.
///
/// `input.ask()` calls the ask tool by this id. To let its model ask the
/// operator, a prompt binds this id under an alias of its own.
pub const USER_INPUT_ASK_TOOL: &str = "user-input/ask";

/// The service key for the input broker, the Host service that carries a
/// question to the operator.
///
/// The key's id is `promptforge/input-broker`. A Host with an operator
/// provides its broker under this key. The ask tool waits on that broker.
pub const INPUT_BROKER: ServiceKey<dyn InputBroker> = ServiceKey::new("promptforge/input-broker");

/// What the ask tool answers on a Host with nobody to ask.
const FALLBACK: &str = "User input is unavailable in this host; continue without it.";

/// The first-party Plugin that lets a prompt section pause and ask the
/// operator for their next message.
///
/// Its id is `user-input`. It needs the input broker provided
/// under the key [`INPUT_BROKER`]. It gives each run the ask tool,
/// [`USER_INPUT_ASK_TOOL`], and a prelude that defines `input.ask()` and
/// `input.connected()` in every section of the run.
///
/// When the run has a broker, `input.ask()` returns the operator's text
/// and `true`. Otherwise it returns a fixed fallback sentence and `false`.
#[derive(Debug, Clone)]
pub struct UserInput {
    /// The stable identity, `user-input`.
    id: PluginId,
    /// The ask tool's identity, [`USER_INPUT_ASK_TOOL`].
    ask: ToolId,
}

impl UserInput {
    /// Builds the Plugin.
    ///
    /// The input broker it needs arrives with each run, in [`RunServices`].
    ///
    /// # Panics
    /// Panics only if the built-in Plugin id or tool id fails to parse.
    /// That would be a defect in the Harness.
    #[must_use]
    pub fn new() -> UserInput {
        #[expect(
            clippy::expect_used,
            reason = "the id is a literal of the Plugin id grammar; a parse failure is a defect in this file, not a caller-actionable condition"
        )]
        let id = PluginId::parse("user-input").expect("the literal user-input Plugin id parses");
        #[expect(
            clippy::expect_used,
            reason = "the id is a literal of the tool id grammar; a parse failure is a defect in this file, not a caller-actionable condition"
        )]
        let ask = ToolId::parse(USER_INPUT_ASK_TOOL).expect("the literal ask tool id parses");
        UserInput { id, ask }
    }
}

impl Default for UserInput {
    fn default() -> UserInput {
        UserInput::new()
    }
}

impl Plugin for UserInput {
    fn id(&self) -> &PluginId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Plugin trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Ask the operator for their next message."
    }

    fn needs(&self) -> &[ServiceId] {
        const NEEDS: &[ServiceId] = &[INPUT_BROKER.id()];
        NEEDS
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, PluginError> {
        let broker = services.get(&INPUT_BROKER);
        let prelude = prelude(broker.is_some());
        Ok(Contribution {
            tools: vec![Arc::new(Ask {
                id: self.ask.clone(),
                broker,
            })],
            prelude: Some(prelude),
        })
    }
}

/// The ask tool: waits on the run's broker, or answers the fallback when
/// the run has none.
struct Ask {
    id: ToolId,
    broker: Option<Arc<dyn InputBroker>>,
}

#[async_trait::async_trait]
impl Tool for Ask {
    fn id(&self) -> ToolId {
        self.id.clone()
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn wire_name(&self) -> &str {
        "ask"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "Wait for the operator's next message and return its text."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn call(
        &self,
        _cx: ToolContext<'_>,
        _args: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        let Some(broker) = &self.broker else {
            return Ok(ToolOutput::trusted(FALLBACK));
        };
        broker
            .wait()
            .await
            .map(ToolOutput::trusted)
            .map_err(tool_error)
    }
}

/// The broker's failure as the ask tool's: the same message, and the
/// broker's error as the cause only when it has a cause of its own, so a
/// message-only failure stays message-only and a rendered cause chain
/// does not repeat the message.
fn tool_error(error: InputError) -> ToolError {
    let message = error.to_string();
    if std::error::Error::source(&error).is_some() {
        ToolError::with_source(message, error)
    } else {
        ToolError::message(message).with_kind(ToolErrorKind::Backend)
    }
}

/// The prelude for one run, with whether the run has a broker written in
/// as `connected`.
fn prelude(connected: bool) -> String {
    format!(
        r#"input = {{}}
local connected = {connected}
function input.connected()
  return connected
end
function input.ask(...)
  if select('#', ...) > 0 then
    error("input.ask takes no arguments", 2)
  end
  return tools.call("{USER_INPUT_ASK_TOOL}"), connected
end
"#
    )
}
