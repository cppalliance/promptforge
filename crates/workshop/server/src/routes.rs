//! Per-feature route constructors, one child module per domain, composed
//! into the full router by [`crate::app::router`]. The extracted feature
//! subsystems (`/ws`, `/agents/ws`, `/v1/models`, `/workspace/*`)
//! self-register their routes through the registry instead of appearing
//! here.

pub(crate) mod assets;
pub(crate) mod gateway_config;
pub(crate) mod health;
pub(crate) mod realtime;
