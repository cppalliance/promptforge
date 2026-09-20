//! The harness handle, its configuration, and the bindings a client
//! pushes across the door: the gateway, the chat catalog, and the host
//! snapshot. Defined in `harness-sessions`, which owns the sessions the
//! harness serves, and named here so clients reach them through the door.
//!
//! The client calls [`Harness::set_gateway`] at startup and on every
//! gateway replacement; the harness rebuilds its capability registry and
//! model client when the generation changes. [`Harness::set_catalog`] and
//! [`Harness::set_host`] push the client's chat-capable model list and
//! its selection and workspace roots the same way: as data, never as a
//! handle into the client.

pub use harness_sessions::environment::{CatalogBinding, GatewayBinding, HostSnapshot};
pub use harness_sessions::runtime::{Harness, HarnessConfig, LaunchError};
