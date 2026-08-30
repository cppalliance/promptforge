//! Per-feature route constructors, one child module per domain, composed
//! into the full router by [`crate::app::router`].

pub(crate) mod assets;
pub(crate) mod chat;
pub(crate) mod gateway_config;
pub(crate) mod health;
pub(crate) mod voice;
pub(crate) mod workspace;
