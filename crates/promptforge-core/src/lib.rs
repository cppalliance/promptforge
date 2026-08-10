//! PromptForge's validated parse-and-run facade.
//!
//! Hosts parse immutable [`Prompt`] values, assemble validated run inputs, and
//! call [`run`]. Tool output trust is mandatory, cancellation is explicit, and
//! ordinary [`Event`] values never carry model, tool, store, or credential
//! payloads. [`SensitiveCapture`] is the separate opt-in secret-bearing seam.

mod api;
mod cancel;
mod client;
mod debug;
mod dialects;
mod error;
mod execute;
pub(crate) mod fanout;
mod lua;
mod lua_models;
mod model;
mod normalize;
mod observe;
mod parser;
mod resolve;
mod store;
mod subst;
mod tools;
mod untrusted;

pub use crate::api::*;
pub(crate) use crate::error::{Error, NearDuplicateDiagnostic, Result};
