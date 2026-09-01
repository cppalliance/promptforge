//! The agent run's configuration: exactly what agents need.
//!
//! Core's `RunConfig` is untouched by the agent path; [`AgentConfig`] is the
//! slim agent counterpart, carried by value into `run_agent`.

use std::sync::Arc;

use promptforge_core_support::cancel::CancelHandle;
use promptforge_core_support::events::EventLog;
use promptforge_core_support::observe::Observer;
use promptforge_model_client::client::StreamDelta;

/// Everything one `run_agent` call carries beyond its catalogs and store.
///
/// How the caller learns things, by channel - `run_agent` itself signals
/// nothing beyond its return:
///
/// - content events ride [`observer`](Self::observer);
/// - live deltas ride [`on_delta`](Self::on_delta);
/// - cancellation is the caller firing the [`cancel`](Self::cancel) handle
///   it retained, after which `run_agent` returns
///   [`Interrupted`](crate::AgentError::Interrupted).
pub struct AgentConfig {
    /// The agent's name - its `.lua` file stem - passed as the `section`
    /// label on every observer call, since agents have no sections. The
    /// SPA and the event JSONL both key on it.
    pub name: String,
    /// The run's execution id, passed on every observer call.
    pub execution: String,
    /// The run's write-only reporting sink.
    pub observer: Arc<dyn Observer>,
    /// The run's cancel handle. `run_agent` installs it as the task's
    /// cancel scope, so every suspended host call (`models.infer`,
    /// `tool_call`) races cancellation and running Lua observes it through
    /// the instruction hook.
    pub cancel: CancelHandle,
    /// The read-side history the agent builds context from, when the host
    /// supplies one. Consumed by the agent-only `runtime.events()` host
    /// call: a read-only indexed view whose snapshot length bound refreshes
    /// at every host-call resume. Absent, `runtime.events()` returns an
    /// empty table.
    pub event_log: Option<Arc<dyn EventLog>>,
    /// The live streaming-delta callback, when the host supplies one.
    /// Forwarded by the agent-only `models.chat`, installed in a later
    /// step; deltas never ride the observer.
    pub on_delta: Option<Arc<dyn Fn(StreamDelta) + Send + Sync>>,
    /// The host-state snapshot provider behind the agent-only `ui()` host
    /// call, installed in a later step. Absent means `ui` is nil.
    pub ui: Option<Arc<dyn Fn() -> serde_json::Value + Send + Sync>>,
    /// Lua resource ceilings for the agent VM.
    pub limits: AgentLimits,
}

/// Shows the data fields; the trait objects and closures have no useful
/// rendering.
impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentConfig")
            .field("name", &self.name)
            .field("execution", &self.execution)
            .field("cancel", &self.cancel)
            .field("event_log", &self.event_log.is_some())
            .field("on_delta", &self.on_delta.is_some())
            .field("ui", &self.ui.is_some())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// Lua resource ceilings for one agent run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentLimits {
    /// The agent VM's Lua heap ceiling in bytes.
    pub lua_memory_bytes: usize,
    /// The agent VM's `log()` event budget.
    pub lua_log_events: u32,
}

impl Default for AgentLimits {
    /// Mirrors the section VM's own defaults: a 64 MiB Lua heap and 1024
    /// `log()` events.
    fn default() -> AgentLimits {
        AgentLimits {
            lua_memory_bytes: 64 * 1024 * 1024,
            lua_log_events: 1024,
        }
    }
}
