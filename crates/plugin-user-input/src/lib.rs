//! `plugin-user-input` - the Plugin that lets a prompt section pause and
//! ask the operator for their next message.
//!
//! A Host installs [`PACKAGE`], by default under the name `user-input`. A
//! prompt declares the Plugin in its frontmatter, and its sections then
//! have an `input` table:
//!
//! ````text
//! plugins:
//!   - user-input
//! ````
//!
//! ````lua
//! local text = input.ask()
//! ````
//!
//! A prompt that does not declare the Plugin has no `input` global.
//!
//! # Where this fits
//!
//! The Plugin offers one tool, the ask tool, under `<name>/ask`, where
//! `<name>` is the name the Host installed it under and the last segment
//! is [`ASK`]. Its prelude defines `input.ask()`, which calls the ask tool
//! by that full id, so every ask reaches the Harness as an ordinary tool
//! call. The ask tool waits on the run's [`InputBroker`], provided among
//! the run's own services under [`INPUT_BROKER`]. A Host binds each run's
//! broker to whoever launched the run, so the question reaches the right
//! operator without naming the run or the section.
//!
//! The operator's answer is trusted, so the ask tool answers with
//! [`ToolOutput::trusted`](promptforge_plugin::ToolOutput::trusted) and
//! the text is never guard-wrapped. The ask tool's descriptor survives
//! stops: a stop leaves the question open, and only a cancel drops it.
//!
//! # A run without an operator
//!
//! [`PACKAGE`] names [`INPUT_BROKER`] among its needs, so a run whose Host
//! supplies no broker cannot use the Plugin, and a prompt that declares it
//! is refused before the run starts with a line naming the Plugin and
//! `promptforge/input-broker`.
//!
//! # Failed and cancelled waits
//!
//! A broker failure is a [`ToolError`], and it becomes the ask call's
//! error unchanged. It raises at the script's `input.ask()` call as a Lua
//! error of kind `"tool"` whose message is the broker's message, so
//! `pcall` catches it; uncaught, it ends the run as a tool failure. The
//! message reaches the prompt, where the model can read it too, so a
//! broker writes it for that audience. Cancelling the run drops the ask
//! tool's pending wait.
//!
//! # Prompt-side rules
//!
//! - `input.ask()` takes no arguments. Calling it with any argument
//!   raises a Lua error with the message `input.ask takes no arguments`,
//!   so a prompt that passes a question fails loudly instead of losing
//!   it.
//! - The model can ask the operator only when the prompt's Lua opts in.
//!   A prompt offers the ask tool by its id, with
//!   `tools.offer("user-input/ask")` or `tools.always_offer`, and the
//!   model sees it as `user-input_ask`. Neither installing nor declaring
//!   the Plugin advertises anything to the model.
//!
//! ## Invariants
//!
//! - May depend on: `promptforge-plugin`, `shared-*` crates,
//!   `workspace-hack`, and outside libraries. `cargo test -p build-xtask`
//!   enforces the Plugin family's allow-list.
//! - The crate names no async runtime. The Harness polls the ask call
//!   inside the run's own future, so an [`InputBroker`] must not block
//!   while polled.
//! - The ask tool's descriptor sets `survives_stop`, so a stop leaves the
//!   operator's question open, and only a cancel drops it.

mod ask;

use promptforge_plugin::{Package, ServiceId, ServiceKey, ToolError};

/// The Plugin's label, which a Host passes to its install.
///
/// Its name is `promptforge/user-input`, so a Host that picks no name
/// installs it as `user-input`. It has a prelude and needs the run's
/// [`INPUT_BROKER`].
pub const PACKAGE: Package = Package::new("promptforge/user-input", ask::construct)
    .prelude(PRELUDE)
    .needs(NEEDS);

/// The ask tool's last segment: its full id is `<name>/ask`, under the
/// name the Host installed the Plugin under.
pub const ASK: &str = "ask";

/// The service key for the input broker, the per-run service that carries
/// a question to the operator.
///
/// The key's id is `promptforge/input-broker`. A Host with an operator
/// provides its broker among each run's services under this key.
pub const INPUT_BROKER: ServiceKey<dyn InputBroker> = ServiceKey::new("promptforge/input-broker");

/// The package's needs: a run without a broker cannot use the Plugin.
const NEEDS: &[ServiceId] = &[INPUT_BROKER.id()];

/// The Lua that defines `input.ask()` for a prompt that declares the
/// Plugin. `...` is the name the Host installed the Plugin under.
const PRELUDE: &str = r#"local plugin = ...
input = {}
function input.ask(...)
  if select('#', ...) > 0 then
    error("input.ask takes no arguments", 2)
  end
  return tools.call(plugin .. "/ask")
end
"#;

/// Waits for the operator's next message on the ask tool's behalf.
///
/// # Invariants
///
/// - [`wait`](InputBroker::wait) returns the operator's text byte-exact,
///   as the operator sent it.
/// - A wait whose future is dropped, as happens when the run is
///   cancelled, must close any prompt it opened for the operator. It must
///   not panic.
/// - [`wait`](InputBroker::wait) must not block while it is polled. The
///   Harness polls it inside the run's own future, beside every other
///   effect of the run, so a broker hands any blocking work to the Host's
///   own runtime.
#[async_trait::async_trait]
pub trait InputBroker: Send + Sync {
    /// Waits for the operator's next message and returns it byte-exact.
    ///
    /// # Errors
    /// Returns a [`ToolError`], whose message is safe to show the model,
    /// when the broker fails to produce the operator's message, for
    /// example because the Host withdrew the wait. It is the ask call's
    /// error unchanged.
    async fn wait(&self) -> Result<String, ToolError>;
}
