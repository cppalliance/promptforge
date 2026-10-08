//! The connection task: it opens the transport, runs the handshake, lists
//! the tools, and publishes the server's state. This is the one place a
//! transport is built, so a later local-server arm joins the HTTP one here.

use std::time::Duration;

use promptforge_plugin::PluginId;
use rmcp::ServiceExt;
use rmcp::model::{ClientCapabilities, ClientConfig, Implementation};
use rmcp::service::ClientInitializeError;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use tokio::sync::watch;

use crate::entry::RemoteEntry;
use crate::result::service_cause;
use crate::server::{Ready, State, catalog};

/// How long a server has to finish the handshake and list its tools.
pub(crate) const STARTUP_DEADLINE: Duration = Duration::from_mins(2);

/// What this client tells a server about itself. It declares no
/// `capabilities`, because an optional extension such as
/// `io.modelcontextprotocol/ui` would make GitHub's server answer with
/// interactive forms that this Plugin cannot show.
fn client_config() -> ClientConfig {
    ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("PromptForge", env!("CARGO_PKG_VERSION")),
    )
}

/// Says what went wrong in the handshake from the error's kind alone, for
/// the reason `service_cause` gives in `result.rs`: rmcp's `Display` text
/// for a transport failure carries the request URL, which may hold an
/// `${env:NAME}` value.
fn handshake_cause(error: &ClientInitializeError) -> String {
    match error {
        ClientInitializeError::TransportError { context, .. } => {
            format!("the transport failed while {context}")
        }
        ClientInitializeError::ConnectionClosed(_) => "the server closed the connection".to_owned(),
        ClientInitializeError::JsonRpcError(_) => {
            "the server answered the handshake with an error".to_owned()
        }
        ClientInitializeError::NoCompatibleProtocolVersion { .. } => {
            "the server and this client share no protocol version".to_owned()
        }
        ClientInitializeError::Cancelled => "the handshake was cancelled".to_owned(),
        _ => "the server's answer was not a valid initialize result".to_owned(),
    }
}

/// Connects to `entry`, publishes `Ready` or `Failed` on `state` within
/// `deadline`, and then keeps the connection open until the task is
/// aborted or the transport ends. Neither end changes the published
/// state.
///
/// rmcp sets no handshake timeout and does not guard `list_all_tools`
/// against a server that repeats a cursor, so `deadline` bounds both.
pub(crate) async fn run(
    plugin: PluginId,
    entry: RemoteEntry,
    deadline: Duration,
    state: watch::Sender<State>,
) {
    // The headers go through the config, never a caller-built client, and
    // never `auth_header`, which adds `Bearer` itself and would double the
    // one an `Authorization` value already holds.
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(entry.url).custom_headers(entry.headers),
    );
    let start = async {
        let service = client_config()
            .serve(transport)
            .await
            .map_err(|e| format!("the MCP handshake failed: {}", handshake_cause(&e)))?;
        let tools = service.list_all_tools().await.map_err(|e| {
            format!(
                "listing the MCP server's tools failed: {}",
                service_cause(&e)
            )
        })?;
        Ok::<_, String>((service, tools))
    };
    match tokio::time::timeout(deadline, start).await {
        Ok(Ok((service, tools))) => {
            let ready = Ready {
                peer: service.peer().clone(),
                tools: catalog(&plugin, tools),
            };
            state.send_replace(State::Ready(ready));
            // Holding the service keeps the session open; its end changes no state.
            let _ = service.waiting().await;
        }
        Ok(Err(reason)) => {
            state.send_replace(State::Failed(reason));
        }
        Err(_) => {
            state.send_replace(State::Failed(format!(
                "the MCP server did not finish starting within {} seconds",
                deadline.as_secs()
            )));
        }
    }
}

#[cfg(test)]
#[path = "connect-tests.rs"]
mod tests;
