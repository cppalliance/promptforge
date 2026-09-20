//! harness-log - the harness run log: an append-only Turso record of every
//! run and, per run, every effect, answer, and event in loop order.
//!
//! ## Invariants
//!
//! - Family: harness, private to `crates/harness/`; may depend on:
//!   `promptforge-api-runtime`, `promptforge-api-types`,
//!   `gateway-api-types`, `gateway-api-discovery`, `shared-*`, and its
//!   container siblings.
//!   Never on a `workshop-*` crate, a private `gateway-*` crate, or a
//!   `promptforge-*` crate behind the door. Read `AGENTS.md` before adding
//!   an import.
//! - `records` is append-only: a record's `seq` is the effect loop's
//!   order, not the clock's, assigned by the log in call order, and no
//!   record is updated or deleted once written. A `runs` row is written
//!   at `begin_run` and closed exactly once at `end_run`; a closed run
//!   accepts no more records.
//! - Every `u64` the engine hands over (`seed`, `effect_id`) is stored as
//!   its two's-complement `i64`, losslessly; a `task_id` is the engine's
//!   hierarchical task path stored as text; timestamps are UTC
//!   milliseconds since the Unix epoch. `started_at` is the caller's;
//!   `at` and `ended_at` are the log's wall clock.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Nothing in this crate spawns a tokio task directly; the harness
//!   spawns only through the instrumented wrapper in `harness-runner`
//!   (enforced by this crate's `clippy.toml`).

mod append;
mod error;
mod read;
mod record;
mod schema;

pub use append::RunLog;
pub use error::{DatabaseSource, LogError, PayloadSource};
pub use record::{
    Record, RecordFilter, RecordKind, RunId, RunMeta, RunOutcome, RunRow, Seq, StoredRecord,
};
