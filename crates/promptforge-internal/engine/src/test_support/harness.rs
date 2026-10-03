//! The suites' Harness bundle: [`RunHarness`], the resources the suites hand the
//! tokio test driver.
//!
//! A [`Run`](crate::execute::Run) issues effects and reports events as
//! values; it holds no client, no tool implementation, and no sink. Those
//! belong to whoever performs the effects. `RunHarness` is that bundle for
//! the suites: the [`ChatClient`] a `Chat` effect is performed with, the
//! [`TestToolTable`] a `ToolCall` effect's id resolves in, and the
//! observer and capture the run's events are replayed onto.
//! [`performers`](RunHarness::performers) and
//! [`sink`](RunHarness::sink) turn the bundle into what
//! [`drive_tokio`](super::drive_tokio) takes. The Engine sees only its
//! effects and answers; the Harness builds its own [`Performers`] and
//! sink in production, and activates its own capabilities.

use std::fmt;
use std::sync::Arc;

use promptforge_types::event::Event;

use super::recording::{self, DebugCapture, NullObserver, Observer};
#[cfg(test)]
use super::tokio_driver::EventSink;
use super::tokio_driver::{BoxFuture, Performers, refuse_tool_call};
use super::tools::TestToolTable;
use crate::execute::RunLimits;
use crate::execute::{Effect, EffectAnswer};
use crate::model::{Completion, CompletionError, CompletionOptions, Message, ToolSchema};

/// What the test driver performs a `Chat` round on: a stand-in for the
/// Harness's model client, which the Engine never holds and this crate
/// never names. The suites' implementation answers each round from a
/// script, in process.
pub trait ChatClient: Send + Sync {
    /// Performs one round: sends `messages` (with `tools` advertised when
    /// non-empty) under `options`, bounded by `limits`' request timeout
    /// and response cap, and returns the completion or its failure.
    fn complete(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        limits: RunLimits,
    ) -> BoxFuture<Result<Completion, CompletionError>>;
}

/// The suites' resources for one run driven by the tokio test driver.
#[derive(Clone)]
#[non_exhaustive]
pub struct RunHarness {
    /// The progress observer every drained event is replayed onto.
    pub(crate) observer: Arc<dyn Observer>,
    /// The opt-in raw request/response capture.
    pub(crate) debug: Option<Arc<dyn DebugCapture>>,
    /// The chat client `Chat` effects are performed with; `None` answers
    /// every round with the disabled-gateway failure.
    pub(crate) client: Option<Arc<dyn ChatClient>>,
    /// The implementations `ToolCall` effects resolve their ids in.
    pub(crate) tools: TestToolTable,
}

impl RunHarness {
    /// Builds the silent bundle: a null observer, no capture, no client,
    /// and no tools.
    #[must_use]
    pub fn new() -> RunHarness {
        RunHarness {
            observer: Arc::new(NullObserver::default()),
            debug: None,
            client: None,
            tools: TestToolTable::new(),
        }
    }

    /// Sets the progress observer the run's events are replayed onto.
    #[must_use]
    pub fn observer(mut self, observer: Arc<dyn Observer>) -> RunHarness {
        self.observer = observer;
        self
    }

    /// Sets the opt-in raw request/response capture. The Engine reports
    /// the raw pair only when the context asks for it
    /// ([`RunContext::report_debug`](crate::execute::RunContext::report_debug)).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn debug(mut self, debug: Arc<dyn DebugCapture>) -> RunHarness {
        self.debug = Some(debug);
        self
    }

    /// Sets the chat client `Chat` effects are performed with; without one
    /// every round fails with the disabled-gateway error.
    #[must_use]
    pub fn client(mut self, client: impl ChatClient + 'static) -> RunHarness {
        self.client = Some(Arc::new(client));
        self
    }

    /// Sets the implementations `ToolCall` effects resolve their ids in.
    /// The catalog the Engine binds against is the caller's to install on
    /// the [`Environment`](crate::execute::Environment) (see
    /// [`TestToolTable::catalog`]); in production the Harness assembles
    /// both from its activated capabilities.
    #[must_use]
    pub fn tools(mut self, tools: TestToolTable) -> RunHarness {
        self.tools = tools;
        self
    }

    /// The bundle's performers for the tokio test driver, starting from
    /// [`Performers::refusing`] and overriding the slots this bundle
    /// supplies: with a client, a `Chat` runs on it under `limits`'
    /// request timeout and body cap; a `ToolCall` resolves its id in the
    /// tool table (a miss is the refusal).
    #[must_use]
    pub fn performers(&self, limits: RunLimits) -> Performers {
        let mut performers = Performers::refusing();
        if let Some(client) = self.client.clone() {
            performers.chat = Box::new(move |effect| {
                let client = Arc::clone(&client);
                Box::pin(async move {
                    let Effect::Chat {
                        messages,
                        tools,
                        options,
                        ..
                    } = effect
                    else {
                        return EffectAnswer::Dropped;
                    };
                    let result = client
                        .complete(messages, tools, options, limits)
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
                // fixture table, as the Harness resolves it against its
                // activated capabilities; the alias is the record's, not
                // the resolver's.
                let Some(tool) = tools.get(&tool) else {
                    return refuse_tool_call();
                };
                EffectAnswer::ToolCall(tool.call(args).await)
            })
        });
        performers
    }

    /// The bundle's event sink for the tokio test driver: every event is
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

impl Default for RunHarness {
    fn default() -> RunHarness {
        RunHarness::new()
    }
}

impl fmt::Debug for RunHarness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunHarness")
            .field("observer", &"<dyn Observer>")
            .field("debug", &self.debug.is_some())
            .field("client", &self.client.is_some())
            .field("tools", &self.tools)
            .finish()
    }
}
