//! Small shared host-support primitives for the PromptForge runtime.
//!
//! [`untrusted`] wraps untrusted external data in a nonce-guarded envelope,
//! [`cancel`] is the cooperative cancellation handle and task-local scope a
//! run observes today, with [`cancel::sync`] holding the polled
//! `AtomicBool` tree the engine switches to, [`observe`] is the report-only vocabulary a run reports its
//! progress through, and [`events`] is the canonical metrics and
//! runtime-event vocabulary with the read-side
//! [`EventLog`](events::EventLog) a host may supply as a run input.
//! [`models`] is the host-facing model vocabulary (identity, catalog,
//! descriptor) and [`wire`] the streaming delta a host's `on_delta`
//! callback observes. [`tools`] is the runtime-agnostic tool vocabulary:
//! the implementation-free [`ToolDescriptor`](tools::ToolDescriptor), the
//! caller-provided [`ToolCatalog`](tools::ToolCatalog), trusted output, and
//! the model-safe tool error, and [`capabilities`] is the capability
//! identity vocabulary, the [`CapabilityId`](capabilities::CapabilityId) a
//! prompt declares and a tool id sits under. The implementation traits
//! behind them (`Tool`, `Capability`, `InputBroker`) are the harness's, in
//! `harness-capabilities`; the engine issues effects naming ids and never
//! holds an implementation. [`ids`] is the hierarchical, deterministic identity of a run's
//! chains and tasks and the [`Provenance`](ids::Provenance) replay key
//! stamped on every effect and event. [`event`] is the value form of a
//! run's reports, the [`Event`](event::Event) enum a host appends to its
//! log; [`timestamp`] is the UTC instant a run starts from, rendered over
//! std alone; and [`replay`] holds the behavior [`Flags`](replay::Flags) a
//! run records and the [`ReplayError`](replay::ReplayError) kinds. This
//! crate's only workspace dependency is the std-only `shared-vfs`, so every
//! promptforge crate may depend on it.

pub mod cancel;
pub mod capabilities;
pub mod event;
pub mod events;
pub mod ids;
pub mod models;
pub mod names;
pub mod observe;
pub mod replay;
pub mod timestamp;
pub mod tools;
pub mod untrusted;
pub mod wire;
