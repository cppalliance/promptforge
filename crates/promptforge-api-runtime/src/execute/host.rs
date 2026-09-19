//! The host's side of a run: [`RunHost`], the resources the in-crate tokio
//! loop's performers draw on.
//!
//! A [`Run`](super::Run) issues effects and reports events as values; it
//! holds no client, no tool implementation, no broker, and no sink. Those
//! belong to whoever performs the effects. `RunHost` is that bundle for
//! the loop behind [`run`](super::run): the gateway client a `Chat` effect
//! is performed with, the capability registry
//! [`Environment::run`](super::Environment::run) activates the prompt's
//! declarations against (filling the [`ToolTable`] a `ToolCall` effect's
//! id resolves in), the [`InputBroker`] a `UserInput` effect waits on, the
//! delta hook a streaming round forwards to, and the observer and capture
//! the run's events are replayed onto. None of it reaches the engine.

use std::fmt;
use std::sync::Arc;

use crate::capabilities::CapabilityRegistry;
use crate::client::{GatewayClient, StreamDelta};
use crate::debug::DebugCapture;
use crate::input::InputBroker;
use crate::observe::{NullObserver, Observer};

use super::activation::{Activation, ToolTable};
use super::requirements::Requirements;

/// The performers' resources for one run driven by the in-crate loop.
#[derive(Clone)]
#[non_exhaustive]
pub struct RunHost {
    /// The progress observer every drained event is replayed onto.
    pub(crate) observer: Arc<dyn Observer>,
    /// The opt-in raw request/response capture.
    pub(crate) debug: Option<Arc<dyn DebugCapture>>,
    /// The gateway client `Chat` effects are performed with; `None` builds
    /// one from the process environment on the first round.
    pub(crate) client: Option<GatewayClient>,
    /// The installed capabilities the prompt's declarations activate
    /// against; `None` activates nothing, and the environment's own
    /// catalog stands.
    pub(crate) registry: Option<Arc<CapabilityRegistry>>,
    /// The implementations `ToolCall` effects resolve their ids in.
    pub(crate) tools: ToolTable,
    /// The broker `UserInput` effects wait on; `None` answers every wait
    /// with the unavailable fallback.
    pub(crate) input: Option<Arc<dyn InputBroker>>,
    /// The live streaming-delta callback a section's model rounds forward
    /// their chunks to; `None` drops deltas at the leaf.
    pub(crate) on_delta: Option<Arc<dyn Fn(StreamDelta) + Send + Sync>>,
    /// What activation could not satisfy, folded into the prepare report
    /// by [`Environment::run`](super::Environment::run) so one refusal
    /// names every gap.
    pub(crate) requirements: Requirements,
}

impl RunHost {
    /// Builds the silent host: a null observer, no capture, no client, no
    /// registry, no tools, no broker, no delta hook.
    #[must_use]
    pub fn new() -> RunHost {
        RunHost {
            observer: Arc::new(NullObserver::default()),
            debug: None,
            client: None,
            registry: None,
            tools: ToolTable::new(),
            input: None,
            on_delta: None,
            requirements: Requirements::default(),
        }
    }

    /// Sets the progress observer the run's events are replayed onto.
    #[must_use]
    pub fn observer(mut self, observer: Arc<dyn Observer>) -> RunHost {
        self.observer = observer;
        self
    }

    /// Sets the opt-in raw request/response capture. The engine reports
    /// the raw pair only when the context asks for it
    /// ([`RunContext::report_debug`](super::RunContext::report_debug)).
    #[must_use]
    pub fn debug(mut self, debug: Arc<dyn DebugCapture>) -> RunHost {
        self.debug = Some(debug);
        self
    }

    /// Sets the gateway client `Chat` effects are performed with; the
    /// default builds one from the process environment on first use.
    #[must_use]
    pub fn client(mut self, client: GatewayClient) -> RunHost {
        self.client = Some(client);
        self
    }

    /// Sets the installed capabilities the prompt's declarations activate
    /// against. [`Environment::run`](super::Environment::run) performs
    /// the activation once, on the loop path: it builds the run's VFS,
    /// activates each declared capability with the run's services, installs
    /// the resulting catalog for prepare to fill slots against, keeps the
    /// implementations here for the tool performer, and folds what
    /// activation could not satisfy into the refusal. Without a registry
    /// nothing activates and the environment's own catalog stands.
    #[must_use]
    pub fn registry(mut self, registry: Arc<CapabilityRegistry>) -> RunHost {
        self.registry = Some(registry);
        self
    }

    /// Sets the implementations `ToolCall` effects resolve their ids in.
    #[must_use]
    pub fn tools(mut self, tools: ToolTable) -> RunHost {
        self.tools = tools;
        self
    }

    /// Takes an activation's implementations and its unsatisfied
    /// requirements: the table performs the calls, and the report is
    /// folded into prepare's so the refusal names every gap. The
    /// activation's catalog is the caller's to install on the
    /// [`Environment`](super::Environment).
    #[must_use]
    pub(crate) fn activated(mut self, activation: Activation) -> RunHost {
        self.tools = activation.tools;
        self.requirements.merge(activation.requirements);
        self
    }

    /// Sets the broker `UserInput` effects wait on. The default (`None`)
    /// is the unavailable-fallback policy: every wait resolves to
    /// [`INPUT_UNAVAILABLE_FALLBACK`](crate::input::INPUT_UNAVAILABLE_FALLBACK)
    /// with `available` false.
    #[must_use]
    pub fn input_broker(mut self, broker: Arc<dyn InputBroker>) -> RunHost {
        self.input = Some(broker);
        self
    }

    /// Sets the live streaming-delta callback `models.loop` rounds forward
    /// their chunks to. The default (`None`) drops deltas at the leaf.
    #[must_use]
    pub fn on_delta(mut self, hook: Arc<dyn Fn(StreamDelta) + Send + Sync>) -> RunHost {
        self.on_delta = Some(hook);
        self
    }
}

impl Default for RunHost {
    fn default() -> RunHost {
        RunHost::new()
    }
}

impl fmt::Debug for RunHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunHost")
            .field("observer", &"<dyn Observer>")
            .field("debug", &self.debug.is_some())
            .field("client", &self.client)
            .field("registry", &self.registry)
            .field("tools", &self.tools)
            .field("input", &self.input.is_some())
            .field("on_delta", &self.on_delta.is_some())
            .field("requirements", &self.requirements)
            .finish()
    }
}
