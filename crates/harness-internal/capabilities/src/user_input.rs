//! The `promptforge/user-input` capability: a prompt section pauses and
//! asks the operator for their next message.
//!
//! A prompt declares the capability in its frontmatter, required or
//! optional, and its sections then have an `input` table with two
//! functions:
//!
//! ````text
//! capabilities:
//!   - promptforge/user-input
//! ````
//!
//! ````lua
//! local text, available = input.ask()
//! ````
//!
//! A prompt that does not declare the capability has no `input` global.
//! By the end of this page you know what the Harness supplies for the
//! capability, what the script receives with and without an operator,
//! how a failed wait reaches the script, and how a prompt lets its model
//! ask.
//!
//! # Where this fits
//!
//! The capability contributes one tool, the ask tool, under the full id
//! [`USER_INPUT_ASK_TOOL`], and a prelude: the Lua source that defines
//! `input` in every section of the run. `input.ask()` calls the ask tool
//! by its full id, so every ask reaches the Harness as an ordinary
//! `ToolCall` effect that its tool performer runs. The ask tool waits
//! on the run's [`InputBroker`] in [`RunServices::input`], the part of
//! the Host that carries a question to a person. Each run gets a broker
//! bound to the session that launched it, so the question reaches the
//! right operator without naming the run or the section.
//!
//! Whether a broker is present is fixed when the capability activates: a
//! broker that is present stays present for the whole run. The prelude
//! records that fact, so `input.connected()` answers it without a tool
//! call, and `input.ask()` returns it as its second value, `available`.
//!
//! # Answering with operator text
//!
//! When the Host has an operator, the Harness puts a broker in the run's
//! services with [`RunServices::with_input`]. Each `input.ask()` waits on
//! [`InputBroker::wait`], and the script receives the operator's text
//! byte-exact with `available` set to `true`. Operator input is trusted,
//! so the ask tool answers with [`ToolOutput::trusted`] and the text is
//! never guard-wrapped.
//!
//! ```
//! use std::sync::Arc;
//!
//! use harness_capabilities::{
//!     Capability, InputBroker, InputError, RunServices, Tool, USER_INPUT_ASK_TOOL, UserInput,
//! };
//! use promptforge::cancel::CancelHandle;
//! use promptforge::tools::OutputTrust;
//!
//! /// Stands in for the Host's own way of reaching a person.
//! struct Operator;
//!
//! #[async_trait::async_trait]
//! impl InputBroker for Operator {
//!     async fn wait(&self) -> Result<String, InputError> {
//!         Ok("hello operator".to_owned())
//!     }
//! }
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new())
//!     .with_input(Arc::new(Operator));
//! let contribution = UserInput::new().create(&services)?;
//! let ask = &contribution.tools[0];
//! assert_eq!(ask.id().to_string(), USER_INPUT_ASK_TOOL);
//!
//! let answer = ask.call(serde_json::json!({})).await?;
//! assert_eq!(answer.text(), "hello operator");
//! assert_eq!(answer.trust(), OutputTrust::Trusted);
//! # Ok(())
//! # }
//! ```
//!
//! # Answering without an operator
//!
//! For a Host with nobody to ask, such as a batch or eval Host, the
//! Harness supplies no broker. What happens then depends on how the
//! prompt declared the capability.
//!
//! - A required declaration is refused before the run starts. The
//!   capability's [`needs`](Capability::needs) names
//!   [`Service::Input`], so activation never calls
//!   [`create`](Capability::create) and the refusal notice holds the line
//!   "- promptforge/user-input needs an input broker, and this host
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
//! - A prompt gets `input` only by declaring `promptforge/user-input`.
//!   Without the declaration, `input.ask()` fails with Lua's own
//!   "attempt to index a nil value (global 'input')".
//! - `input.ask()` takes no arguments. Calling it with any argument
//!   raises a Lua error with the message `input.ask takes no arguments`,
//!   so a prompt that passes a question fails loudly instead of losing
//!   it.
//! - The model can ask the operator only when the prompt opts in: it
//!   binds the ask tool under an alias in its `tools:` frontmatter, for
//!   example `ask: promptforge/user-input/ask`, and advertises that alias
//!   with `tools.add` or `tools.always`. Declaring the capability alone
//!   advertises nothing to the model.

use std::sync::Arc;

use promptforge::capabilities::CapabilityId;
use promptforge::tools::{ToolError, ToolErrorKind, ToolId, ToolOutput};

use crate::capability::{Capability, CapabilityError, Contribution, RunServices, Service};
use crate::input::{InputBroker, InputError};
use crate::tool::Tool;

#[cfg(test)]
#[path = "user_input-tests.rs"]
mod tests;

/// The ask tool's full id: what `input.ask()` calls, and what a prompt
/// binds under an alias of its own to let its model ask the operator.
pub const USER_INPUT_ASK_TOOL: &str = "promptforge/user-input/ask";

/// What the ask tool answers on a Host with nobody to ask.
const FALLBACK: &str = "User input is unavailable in this host; continue without it.";

/// The first-party `promptforge/user-input` capability.
///
/// Needs [`Service::Input`]. Contributes the ask tool,
/// [`USER_INPUT_ASK_TOOL`], and a prelude defining `input.ask()` and
/// `input.connected()`. The module page covers what a script receives.
///
/// # Examples
///
/// ```
/// use harness_capabilities::{Capability, CapabilityRegistry, Service, UserInput};
///
/// let mut registry = CapabilityRegistry::new();
/// registry.register(std::sync::Arc::new(UserInput::new()))?;
/// assert_eq!(UserInput::new().id().to_string(), "promptforge/user-input");
/// assert_eq!(UserInput::new().needs(), [Service::Input]);
/// # Ok::<(), harness_capabilities::RegistryError>(())
/// ```
#[derive(Debug, Clone)]
pub struct UserInput {
    /// The stable identity, `promptforge/user-input`.
    id: CapabilityId,
    /// The ask tool's identity, [`USER_INPUT_ASK_TOOL`].
    ask: ToolId,
}

impl UserInput {
    /// Builds the capability. It takes no configuration: everything it
    /// needs arrives per run in [`RunServices`].
    ///
    /// # Panics
    /// Panics only if the literal capability or tool id fails to parse, a
    /// defect in this crate rather than a caller error.
    #[must_use]
    pub fn new() -> UserInput {
        #[expect(
            clippy::expect_used,
            reason = "the id is a literal of the capability id grammar; a parse failure is a defect in this file, not a caller-actionable condition"
        )]
        let id = CapabilityId::parse("promptforge/user-input")
            .expect("the literal user-input capability id parses");
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

impl Capability for UserInput {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Ask the operator for their next message."
    }

    fn needs(&self) -> &[Service] {
        &[Service::Input]
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
        let broker = services.input.clone();
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

    async fn call(&self, _args: serde_json::Value) -> Result<ToolOutput, ToolError> {
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
