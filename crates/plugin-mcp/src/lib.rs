//! An MCP client Plugin: it connects a prompt to one remote MCP server
//! over Streamable HTTP and offers that server's tools as ordinary tools.
//!
//! A Host installs [`PACKAGE`] once per server, under the server's name
//! lowercased in ASCII, so the tools of a server named `wg21-papers` are
//! `wg21-papers/<tool>`. The Plugin has no prelude, so declaring it, as in
//! `plugins: [wg21-papers]`, only makes it required.
//!
//! The Host passes the server's entry from an `mcp.json` file as the
//! configuration. A remote entry has `url` and optional `headers`, and a
//! `type` of `"sse"`, the legacy transport, is refused. A local entry has
//! `command`, with optional `args`, `env`, and `cwd`. This Plugin
//! recognizes that shape and refuses it at install with `local MCP servers
//! (command) are not supported yet`, so the Host installs it as
//! unavailable. Other `type` values and unknown fields are ignored, because
//! the format is defined outside this repository.
//!
//! String values in an entry may use `${env:NAME}`, which an unset
//! variable refuses naming `NAME`, and `${userHome}`. Any other `${...}`
//! form passes through unchanged. `headers` may not set `accept`,
//! `Mcp-Session-Id`, or `Last-Event-Id`, which the client owns.
//!
//! Install does no I/O. It reads the Host's runtime from the Plugin
//! contract's [`promptforge_plugin::TOKIO_RUNTIME`] service and starts one
//! background task on it, which connects, runs the handshake, and lists
//! every page of tools within two minutes. The Plugin's `ready` waits for
//! that task, so a run sees the tools of a server that is still starting.
//! A server that fails to start is unavailable, with the reason. A server
//! that is ready stays ready, and a later failure shows as a transport
//! error on the call.
//!
//! A tool whose name, lowercased in ASCII, is not a valid tool id, or that
//! repeats an earlier tool's id after lowercasing, is dropped with a
//! warning. A tool's description is its `description`, else its `title`,
//! else its name, and its parameters are its `inputSchema`.
//!
//! A call sends `tools/call` with the tool's original name and waits up to
//! five minutes. Text blocks and embedded text resources come back joined
//! with blank lines. An image, audio, or binary resource becomes a
//! placeholder such as `[image omitted: image/png]`, a resource link
//! becomes `[resource: <uri>]`, and a result with no blocks but with
//! `structuredContent` gives that JSON. A result marked `isError` is a
//! backend error with the server's text, and a transport failure or the
//! deadline is a transport error. Dropping a call sends the server
//! `notifications/cancelled` for its request.
//!
//! The client declares no `capabilities`, because an optional extension
//! would make GitHub's remote server answer with interactive forms this
//! Plugin cannot show. Its HTTP client refuses redirects, which keeps the
//! configured headers off any other server.
//!
//! ## Invariants
//!
//! - May depend on: `promptforge-plugin`, `shared-*` crates,
//!   `workspace-hack`, and outside libraries. `cargo test -p build-xtask`
//!   enforces the Plugin family's allow-list. Only [`PACKAGE`] is public.
//! - Every output is untrusted.
//! - No reason, log line, `Debug` output, or error names an `env` or
//!   `headers` value. They may name keys.
//! - It runs on the Host's runtime, read through the contract's
//!   `TOKIO_RUNTIME` service, and `construct` does no I/O.
//! - It starts no process: a local entry is refused at `construct`.
//! - Dropping a call tells the server to drop the request.

mod connect;
mod entry;
mod result;
mod server;

use std::sync::Arc;

use promptforge_plugin::{
    HostServices, Package, Plugin, PluginId, ServiceKey, TOKIO_RUNTIME, ToolError,
};
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::entry::Vars;
use crate::server::{Server, State};

/// The Plugin's label, which a Host passes to its install.
///
/// Its name is `promptforge/mcp`. The Host installs it once per server,
/// passing the server's `mcp.json` entry as the configuration, and
/// provides the Plugin contract's [`promptforge_plugin::TOKIO_RUNTIME`]
/// service. It has no prelude and needs no per-run service.
pub const PACKAGE: Package = Package::new("promptforge/mcp", construct);

/// The key `construct` reads the Host's runtime through.
const RUNTIME: ServiceKey<Handle> = ServiceKey::new(TOKIO_RUNTIME);

/// Validates the entry, reads the runtime, and starts the connection task.
/// It does no I/O itself.
#[expect(
    clippy::needless_pass_by_value,
    reason = "the Package construct signature fixes the argument types"
)]
fn construct(
    name: &PluginId,
    config: Value,
    services: &HostServices,
) -> Result<Arc<dyn Plugin>, ToolError> {
    let env = |key: &str| std::env::var(key).ok();
    let home = std::env::home_dir().map(|dir| dir.to_string_lossy().into_owned());
    let entry = entry::parse(&config, &Vars { env: &env, home })?;
    let runtime = services.get(&RUNTIME).ok_or_else(|| {
        ToolError::message("MCP needs promptforge/tokio-runtime, and this host provides none")
    })?;
    let (state, watching) = watch::channel(State::Starting);
    let connection = runtime.spawn(connect::run(
        name.clone(),
        entry,
        connect::STARTUP_DEADLINE,
        state,
    ));
    Ok(Arc::new(Server::new(
        watching,
        connection,
        Handle::clone(&runtime),
    )))
}
