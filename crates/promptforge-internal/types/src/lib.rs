//! Small shared run-support primitives for the PromptForge runtime.
//!
//! [`untrusted`] wraps untrusted external data in a nonce-guarded envelope
//! and [`cancel`] is the polled `AtomicBool` cancellation tree the engine
//! observes. [`event`] is the value form of a run's reports, the
//! [`Event`](event::Event) enum the Harness appends to its log, with the
//! payload-free boundaries' constructors in [`event::lifecycle`];
//! [`emitter`] is the provenance-stamping [`Emitter`](emitter::Emitter)
//! every engine crate reports through and the [`EventSink`](emitter::EventSink)
//! a run drains; and [`metrics`] is the model-call metrics vocabulary those
//! events embed. [`models`] is the public model vocabulary (identity,
//! catalog, descriptor) and [`wire`] the streaming delta the Harness's `on_delta`
//! callback observes. [`tools`] is the runtime-agnostic tool vocabulary:
//! the implementation-free [`ToolDescriptor`](tools::ToolDescriptor), the
//! caller-provided [`ToolCatalog`](tools::ToolCatalog), trusted output, and
//! the model-safe tool error, and [`capabilities`] is the capability
//! identity vocabulary, the [`CapabilityId`](capabilities::CapabilityId) a
//! prompt declares and a tool id sits under. The implementation traits
//! behind them (`Tool`, `Capability`) are the harness's, in
//! `harness-capabilities`; the engine issues effects naming ids and never
//! holds an implementation. [`ids`] is the hierarchical, deterministic
//! identity of a run's chains and tasks and the [`Provenance`](ids::Provenance)
//! replay key stamped on every effect and event; [`timestamp`] is the UTC
//! instant a run starts from, rendered over std alone; and [`replay`] holds
//! the behavior [`Flags`](replay::Flags) a run records and the
//! [`ReplayError`](replay::ReplayError) kinds. This crate depends on no
//! other promptforge crate, so every promptforge crate may depend on it,
//! and it declares no async runtime.
//!
//! ## Invariants
//!
//! - May depend on: no workspace crate beyond `workspace-hack` and the
//!   doctest-only `promptforge` dev-dependency. Read the repository-root
//!   `AGENTS.md` before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.

pub mod cancel;
pub mod capabilities;
pub mod detail;
pub mod emitter;
pub mod event;
pub mod ids;
pub mod metrics;
pub mod models;
pub mod names;
pub mod replay;
pub mod timestamp;
pub mod tools;
pub mod untrusted;
pub mod wire;
