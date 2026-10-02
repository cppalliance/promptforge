//! workshop-run-log - the Workshop run log: an append-only Turso record of
//! every run and, per run, every effect, answer, and event in loop order,
//! and the recorder that lets the Harness write to it.
//!
//! ## Invariants
//!
//! - Tier: feature; may depend on: `workshop-protocol`,
//!   `workshop-registry`, `workshop-support`, and the service crates;
//!   today it names none of them. Outside the family it may name the
//!   Harness's public API `harness`, the Engine's public API
//!   `promptforge`, and `shared-*` crates. Never on `workshop-server`, a
//!   private Harness, Engine, or Gateway crate. Read the repository-root
//!   `AGENTS.md` before adding an import.
//! - The Harness holds no database and no storage path: this crate is
//!   where the Host's recorder keeps a run's history. It reaches the
//!   Harness only through `harness::record`.
//! - `records` is append-only: a record's `seq` is the effect loop's
//!   order, not the clock's, assigned by the log in call order, and no
//!   record is updated or deleted once written. A `runs` row is written
//!   at `begin_run` and closed exactly once at `end_run`; a closed run
//!   accepts no more records.
//! - Every `u64` the Engine hands over (`seed`, `effect_id`) is stored as
//!   its two's-complement `i64`, losslessly; a `task_id` is the Engine's
//!   hierarchical task path stored as text; timestamps are UTC
//!   milliseconds since the Unix epoch. `started_at` is the caller's;
//!   `at` and `ended_at` are the log's wall clock.
//! - A stored payload round-trips: `Record::payload` is written with
//!   `serde_json::to_string` and read back with `from_str`, and the parsed
//!   value equals the original with the same text. The workspace enables
//!   `serde_json`'s `float_roundtrip` feature, and `Value::Object` orders
//!   keys by `BTreeMap` rather than by insertion, so both hold. The
//!   fidelity test in `tests/it/fidelity.rs` pins it.
//! - `TursoRecorder` serializes every call behind its own lock, and calls
//!   from different runs may overlap. The database opens on the first
//!   call, and an open that fails leaves nothing behind, so the next call
//!   tries again. A write the log refuses fails the run it belongs to; the
//!   recorder never skips a record.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.

mod append;
mod error;
mod read;
mod record;
mod recorder;
mod schema;

pub use append::RunLog;
pub use error::LogError;
pub use record::{
    Record, RecordFilter, RecordKind, RunId, RunMeta, RunOutcome, RunRow, Seq, StoredRecord,
};
pub use recorder::TursoRecorder;
