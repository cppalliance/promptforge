//! The test host bundle: [`RunHost`], the resources the suites hand the
//! tokio test driver.
//!
//! A [`Run`](crate::execute::Run) issues effects and reports events as
//! values; it holds no client, no tool implementation, no broker, and no
//! sink. Those belong to whoever performs the effects. `RunHost` is that
//! bundle for the suites: the [`ChatClient`] a `Chat` effect is performed
//! with, the [`TestToolTable`] a `ToolCall` effect's id resolves in, the
//! [`TestBroker`] a `UserInput` effect waits on, the delta hook a
//! streaming round forwards to, and the observer and capture the run's
//! events are replayed onto. [`performers`](RunHost::performers) and
//! [`sink`](RunHost::sink) turn the bundle into what
//! [`drive_tokio`](super::drive_tokio) takes. None of it reaches the
//! engine; a production host builds its own [`Performers`] and sink, and
//! activates its own capabilities.

use std::fmt;
use std::sync::Arc;

use promptforge_api_types::event::Event;

use super::recording::{self, DebugCapture, NullObserver, Observer};
#[cfg(test)]
use super::tokio_driver::EventSink;
use super::tokio_driver::{BoxFuture, Performers, refuse_tool_call};
use super::tools::{TestBroker, TestToolTable};
use crate::execute::RunLimits;
use crate::execute::{Effect, EffectAnswer};
use crate::model::{
    Completion, CompletionError, CompletionOptions, Message, StreamDelta, ToolSchema,
};

/// The live streaming-delta callback a chat round forwards its chunks to.
pub type DeltaHook = Arc<dyn Fn(StreamDelta) + Send + Sync>;

/// What the test driver performs a `Chat` round on: a stand-in for the
/// harness's model client, which the engine never holds and this crate
/// never names. The suites' implementation speaks the wire vocabulary to
/// an axum mock gateway over a dev-only HTTP client; a scripted host can
/// answer from a table.
pub trait ChatClient: Send + Sync {
    /// Performs one round: sends `messages` (with `tools` advertised when
    /// non-empty) under `options`, bounded by `limits`' request timeout
    /// and response cap, forwarding each live delta to `on_delta` when
    /// the round streams, and returns the completion or its failure.
    fn complete(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        limits: RunLimits,
        on_delta: Option<DeltaHook>,
    ) -> BoxFuture<Result<Completion, CompletionError>>;
}

/// The suites' resources for one run driven by the tokio test driver.
#[derive(Clone)]
#[non_exhaustive]
pub struct RunHost {
    /// The progress observer every drained event is replayed onto.
    pub(crate) observer: Arc<dyn Observer>,
    /// The opt-in raw request/response capture.
    pub(crate) debug: Option<Arc<dyn DebugCapture>>,
    /// The chat client `Chat` effects are performed with; `None` answers
    /// every round with the disabled-gateway failure.
    pub(crate) client: Option<Arc<dyn ChatClient>>,
    /// The implementations `ToolCall` effects resolve their ids in.
    pub(crate) tools: TestToolTable,
    /// The broker `UserInput` effects wait on; `None` answers every wait
    /// with the unavailable fallback.
    pub(crate) input: Option<Arc<dyn TestBroker>>,
    /// The live streaming-delta callback a section's model rounds forward
    /// their chunks to; `None` drops deltas at the leaf.
    pub(crate) on_delta: Option<DeltaHook>,
}

impl RunHost {
    /// Builds the silent host: a null observer, no capture, no client, no
    /// tools, no broker, no delta hook.
    #[must_use]
    pub fn new() -> RunHost {
        RunHost {
            observer: Arc::new(NullObserver::default()),
            debug: None,
            client: None,
            tools: TestToolTable::new(),
            input: None,
            on_delta: None,
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
    /// ([`RunContext::report_debug`](crate::execute::RunContext::report_debug)).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn debug(mut self, debug: Arc<dyn DebugCapture>) -> RunHost {
        self.debug = Some(debug);
        self
    }

    /// Sets the chat client `Chat` effects are performed with; without one
    /// every round fails with the disabled-gateway error.
    #[must_use]
    pub fn client(mut self, client: impl ChatClient + 'static) -> RunHost {
        self.client = Some(Arc::new(client));
        self
    }

    /// Sets the implementations `ToolCall` effects resolve their ids in.
    /// The catalog the engine binds against is the caller's to install on
    /// the [`Environment`](crate::execute::Environment) (see
    /// [`TestToolTable::catalog`]); a production host assembles both from
    /// its activated capabilities.
    #[must_use]
    pub fn tools(mut self, tools: TestToolTable) -> RunHost {
        self.tools = tools;
        self
    }

    /// Sets the broker `UserInput` effects wait on. The default (`None`)
    /// is the unavailable-fallback policy: every wait resolves to
    /// `INPUT_UNAVAILABLE_FALLBACK` with `available` false.
    #[must_use]
    pub fn input_broker(mut self, broker: Arc<dyn TestBroker>) -> RunHost {
        self.input = Some(broker);
        self
    }

    /// Sets the live streaming-delta callback `models.loop` rounds forward
    /// their chunks to. The default (`None`) drops deltas at the leaf.
    #[must_use]
    pub fn on_delta(mut self, hook: DeltaHook) -> RunHost {
        self.on_delta = Some(hook);
        self
    }

    /// The host's performers for the tokio test driver, starting from
    /// [`Performers::refusing`] and overriding the slots this host
    /// supplies: with a client, a `Chat` runs on it under `limits`'
    /// request timeout and body cap; a `ToolCall` resolves its id in the
    /// tool table (a miss is the refusal); with a broker, a `UserInput`
    /// waits on it.
    #[must_use]
    pub fn performers(&self, limits: RunLimits) -> Performers {
        let mut performers = Performers::refusing();
        if let Some(client) = self.client.clone() {
            let on_delta = self.on_delta.clone();
            performers.chat = Box::new(move |effect| {
                let client = Arc::clone(&client);
                let on_delta = on_delta.clone();
                Box::pin(async move {
                    let Effect::Chat {
                        messages,
                        tools,
                        options,
                        stream,
                        ..
                    } = effect
                    else {
                        return EffectAnswer::Dropped;
                    };
                    // The host's delta callback is the live consumer of a
                    // streaming round; without one, or for a round the
                    // effect marks non-streaming (a nested infer), the
                    // chunks drop at the leaf and the completed reply is
                    // the repair.
                    let on_delta = stream.then_some(on_delta).flatten();
                    let result = client
                        .complete(messages, tools, options, limits, on_delta)
                        .await
                        .map(Box::new);
                    EffectAnswer::Chat(result)
                })
            });
        }
        let tools = self.tools.clone();
        performers.tool_call = Box::new(move |effect| {
            let tools = tools.clone();
            Box::pin(async move {
                let Effect::ToolCall { tool, args, .. } = effect else {
                    return EffectAnswer::Dropped;
                };
                // Resolved by the stable identity against the suites'
                // fixture table, as the harness resolves it against its
                // activated capabilities; the alias is the record's, not
                // the resolver's.
                let Some(tool) = tools.get(&tool) else {
                    return refuse_tool_call();
                };
                EffectAnswer::ToolCall(tool.call(args).await)
            })
        });
        if let Some(broker) = self.input.clone() {
            performers.user_input = Box::new(move |effect| {
                let broker = Arc::clone(&broker);
                Box::pin(async move {
                    let Effect::UserInput { execution, section } = effect else {
                        return EffectAnswer::Dropped;
                    };
                    EffectAnswer::UserInput(broker.user_input(&execution, &section).await)
                })
            });
        }
        performers
    }

    /// The host's event sink for the tokio test driver: every event is
    /// replayed onto the observer and, for the debug pair, the capture.
    pub fn sink(&self) -> impl FnMut(Event) + Send + use<> {
        let observer = Arc::clone(&self.observer);
        let debug = self.debug.clone();
        move |event| recording::forward_one(event, observer.as_ref(), debug.as_deref())
    }

    /// The sink, boxed for the driver.
    #[cfg(test)]
    pub(crate) fn boxed_sink(&self) -> EventSink<'static> {
        Box::new(self.sink())
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
            .field("client", &self.client.is_some())
            .field("tools", &self.tools)
            .field("input", &self.input.is_some())
            .field("on_delta", &self.on_delta.is_some())
            .finish()
    }
}
