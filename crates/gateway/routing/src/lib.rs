//! Routing vocabulary for the PromptForge gateway: the [`Model`] and
//! [`Endpoint`] table entries and the per-dominion admission control
//! ([`queue`]) that both the gateway's routing table and the local inference
//! crate build on.
//!
//! This crate holds only the shared data plane. The gateway's routing
//! table (`Routing`) and its error envelopes sit in the gateway crate;
//! local provisioning and the `llama-server` lifecycle sit in
//! `gateway-local`.
//!
//! The `test-helpers` feature exposes the `DominionQueue` observation seams
//! (`waiter_count`, `distinct_clients`) for downstream crates' test suites;
//! in-crate tests always see them.

mod model;
pub mod queue;

pub use crate::model::{Endpoint, GEMMA3_TOOL_CODE, Model};
pub use crate::queue::dominion_queues;
