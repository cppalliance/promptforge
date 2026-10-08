//! The Plugin one server is installed as: its state, its tool list, and
//! its calls.

use std::collections::HashSet;
use std::time::Duration;

use promptforge_plugin::{
    Plugin, PluginFuture, PluginId, ToolContext, ToolDescriptor, ToolError, ToolErrorKind, ToolId,
    ToolOutput,
};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CancelledNotificationParam, ClientRequest, JsonObject,
    RequestId, Tool,
};
use rmcp::service::{Peer, PeerRequestOptions, RoleClient};
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::result;

/// How long a call may take before it fails as a transport error.
pub(crate) const CALL_DEADLINE: Duration = Duration::from_mins(5);

/// Where a server is. It moves from `Starting` to `Ready` or `Failed` once
/// and never back to `Starting`.
#[derive(Debug)]
pub(crate) enum State {
    Starting,
    Ready(Ready),
    Failed(String),
}

/// A connected server: the peer calls go through and the tools it listed.
#[derive(Debug)]
pub(crate) struct Ready {
    pub(crate) peer: Peer<RoleClient>,
    pub(crate) tools: Vec<Listed>,
}

/// One tool the server listed and the Plugin kept.
#[derive(Debug)]
pub(crate) struct Listed {
    pub(crate) descriptor: ToolDescriptor,
    /// The name the server knows the tool by, which a call sends.
    pub(crate) mcp_name: String,
}

/// Keeps the tools whose lowercased names form a tool id under `plugin`,
/// and drops, with a warning, any that do not or that repeat an earlier
/// tool's id.
pub(crate) fn catalog(plugin: &PluginId, tools: Vec<Tool>) -> Vec<Listed> {
    let mut seen = HashSet::new();
    let mut kept = Vec::new();
    for tool in tools {
        let mcp_name = tool.name.to_string();
        let lowered = mcp_name.to_ascii_lowercase();
        let Ok(id) = ToolId::parse(&format!("{plugin}/{lowered}")) else {
            tracing::warn!(
                %plugin, tool = %mcp_name,
                "dropping an MCP tool whose lowercased name is not a tool id of lowercase letters, digits, '-', '_', and '.'"
            );
            continue;
        };
        if !seen.insert(id.clone()) {
            tracing::warn!(
                %plugin, tool = %mcp_name,
                "dropping an MCP tool whose lowercased name matches an earlier tool's"
            );
            continue;
        }
        let description = tool
            .description
            .as_deref()
            .or(tool.title.as_deref())
            .unwrap_or(&mcp_name)
            .to_owned();
        let schema = Value::Object((*tool.input_schema).clone());
        kept.push(Listed {
            descriptor: ToolDescriptor::new(id, description, schema),
            mcp_name,
        });
    }
    kept
}

/// The Plugin: a watch on the connection task's state, the task, and the
/// Host's runtime.
#[derive(Debug)]
pub(crate) struct Server {
    state: watch::Receiver<State>,
    connection: JoinHandle<()>,
    runtime: Handle,
}

impl Server {
    pub(crate) fn new(
        state: watch::Receiver<State>,
        connection: JoinHandle<()>,
        runtime: Handle,
    ) -> Server {
        Server {
            state,
            connection,
            runtime,
        }
    }

    /// The peer and server-side name for `tool`, or the reason a call
    /// cannot go out.
    fn target(&self, tool: &ToolId) -> Result<(Peer<RoleClient>, String), ToolError> {
        match &*self.state.borrow() {
            State::Ready(ready) => ready
                .tools
                .iter()
                .find(|listed| listed.descriptor.id == *tool)
                .map(|listed| (ready.peer.clone(), listed.mcp_name.clone()))
                .ok_or_else(|| ToolError::message(format!("MCP server has no tool {tool}"))),
            State::Starting => Err(ToolError::message("MCP server is still starting")),
            State::Failed(reason) => Err(ToolError::message(reason.clone())),
        }
    }
}

impl Drop for Server {
    /// Aborts the connection task, which drops the running service and so
    /// ends the HTTP session.
    fn drop(&mut self) {
        self.connection.abort();
    }
}

/// A spawned call, aborted when the call awaiting it is dropped.
struct AbortOnDrop(JoinHandle<Result<ToolOutput, ToolError>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Tells the server to drop a request when the task holding it is aborted.
/// rmcp sends `notifications/cancelled` itself on a timeout, but dropping a
/// request's handle or response sends nothing.
struct CancelOnDrop {
    armed: Option<(Peer<RoleClient>, RequestId)>,
    runtime: Handle,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some((peer, id)) = self.armed.take() {
            self.runtime.spawn(async move {
                // The connection may be gone already, which leaves nothing to tell.
                let _ = peer
                    .notify_cancelled(CancelledNotificationParam::new(Some(id), None))
                    .await;
            });
        }
    }
}

/// Sends one `tools/call` and renders the answer. It runs on the Host's
/// runtime, so dropping the awaiting call aborts it and fires the guard.
async fn send(
    peer: Peer<RoleClient>,
    name: String,
    arguments: JsonObject,
    runtime: Handle,
) -> Result<ToolOutput, ToolError> {
    let request = ClientRequest::CallToolRequest(CallToolRequest::new(
        CallToolRequestParams::new(name).with_arguments(arguments),
    ));
    let handle = peer
        .send_cancellable_request(request, PeerRequestOptions::with_timeout(CALL_DEADLINE))
        .await
        .map_err(|e| result::service_error(&e))?;
    let mut guard = CancelOnDrop {
        armed: Some((handle.peer.clone(), handle.id.clone())),
        runtime,
    };
    let response = handle.await_response().await;
    guard.armed = None;
    result::output(response)
}

impl Plugin for Server {
    fn tools(&self) -> Vec<ToolDescriptor> {
        match &*self.state.borrow() {
            State::Ready(ready) => ready
                .tools
                .iter()
                .map(|listed| listed.descriptor.clone())
                .collect(),
            State::Starting | State::Failed(_) => Vec::new(),
        }
    }

    fn ready(&self) -> PluginFuture<'_, Result<(), ToolError>> {
        let mut state = self.state.clone();
        Box::pin(async move {
            let settled = state
                .wait_for(|now| !matches!(now, State::Starting))
                .await
                .map_err(|_| {
                    ToolError::message("MCP connection task ended before it finished starting")
                })?;
            match &*settled {
                State::Failed(reason) => Err(ToolError::message(reason.clone())),
                State::Starting | State::Ready(_) => Ok(()),
            }
        })
    }

    fn call<'a>(
        &'a self,
        cx: ToolContext<'a>,
        args: Value,
    ) -> PluginFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let (peer, name) = self.target(cx.tool())?;
            let arguments = match args {
                Value::Object(map) => map,
                Value::Null => JsonObject::new(),
                _ => {
                    return Err(
                        ToolError::message("MCP tool arguments must be a JSON object")
                            .with_kind(ToolErrorKind::InvalidArguments),
                    );
                }
            };
            let mut task =
                AbortOnDrop(
                    self.runtime
                        .spawn(send(peer, name, arguments, self.runtime.clone())),
                );
            match (&mut task.0).await {
                Ok(outcome) => outcome,
                Err(join) => Err(
                    ToolError::with_source("the MCP call task did not finish", join)
                        .with_kind(ToolErrorKind::Transport),
                ),
            }
        })
    }
}

#[cfg(test)]
#[path = "server-tests.rs"]
mod tests;
