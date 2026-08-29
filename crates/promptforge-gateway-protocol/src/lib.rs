//! The PromptForge gateway's OpenAI wire protocol and upstream abstraction.
//!
//! This crate is the shared protocol contract between the gateway, its local
//! inference subsystem, and external clients: the OpenAI-shaped request and
//! response bodies with their trust-boundary validation ([`wire`]), the
//! [`upstream::Upstream`] trait with its OpenAI passthrough
//! ([`upstream::OpenAiUpstream`]) and SSE chunk parser, and the bounded HTTP
//! client policy every outbound call shares ([`http_util`]).
//!
//! Failures at the upstream seam are reported as [`ProtocolError`]; an
//! explicit teardown failure is reported as [`ShutdownError`]. The crate
//! contains no local inference, no routing, and no HTTP server handlers.

mod error;
pub mod http_util;
pub mod upstream;
pub mod wire;

pub use crate::error::{ProtocolError, ShutdownError};
