//! Shared loopback request checks for PromptForge servers.
//!
//! Two middlewares form the wall. [`require_loopback`] refuses any request
//! whose peer address is not loopback. [`require_loopback_host`] refuses
//! any request whose authority is not the bound loopback socket, closing
//! DNS rebinding. The config-ui crate wraps its SPA asset routes with the
//! peer check (re-exporting it as its own public surface), and the gateway
//! applies the peer check to its admin config endpoints and the host check
//! to its whole loopback-bound surface, so each check exists in exactly one
//! place. The crate is deliberately tiny -
//! axum is its only dependency - because the gateway needs the wall in
//! every build, including headless builds that never compile the
//! config-ui crate and its embedded-asset machinery.
//!
//! WebSocket Origin policy stays explicit and product-specific:
//! [`gateway_loopback_origin_allowed`] admits native clients or HTTP loopback
//! origins, while [`workshop_same_origin_authority_allowed`] requires browser
//! origins to match the Workshop request authority.

mod host;
mod origin;
mod peer;

pub use host::require_loopback_host;
pub use origin::{gateway_loopback_origin_allowed, workshop_same_origin_authority_allowed};
pub use peer::{is_loopback_peer, require_loopback};
