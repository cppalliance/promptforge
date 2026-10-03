//! harness-runner - the per-run Harness and the effect loop it drives: a
//! [`Harness`] resolves the launch model through the Host's broker,
//! prepares an Engine `Run` from the prompt's source (drawing the inputs
//! the Engine refuses to draw itself, putting the declared input file in
//! place, activating capabilities, beginning the run at its recorder),
//! steps it, performs each chat, tool-call, and timer effect through its
//! performer and answers each Vfs effect inline, feeds the answers back,
//! hands every event, effect, and answer to the run's recorder, and reads
//! the declared output file, all inside the one future [`Harness::run`]
//! returns.
//!
//! ## Invariants
//!
//! - Family: Harness, private to `crates/harness-internal/`; may depend
//!   on: `promptforge` and container siblings only.
//!   Never on a `workshop-*`, `gateway-*`, or `shared-*` crate, or a
//!   private `promptforge-*` crate. `cargo test -p build-xtask` enforces the
//!   product and container boundaries.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - The Harness polls every effect inside the run's own future and starts
//!   no task, so any executor can drive a run. Performers must not block
//!   while polled: one that blocks stalls every other effect of the run,
//!   and a performer with blocking or CPU-heavy work hands it to the
//!   Host's own runtime. The crate's async needs come from `futures-util`;
//!   `cancel` is the one module that names tokio.
//! - The recorder is written in loop order: a step's events before the
//!   step's effects are issued, each effect before its performer starts,
//!   each answer before the run resumes with it. Every effect record has
//!   exactly one answer record; a dropped effect's answer is `Dropped`.
//! - The loop never reads an event to decide anything; control comes
//!   from the run's own word (`Step`, `Run::decided`), the cancel flag,
//!   and the Host's stop.

pub mod cancel;
mod display_chain;
pub mod effect_loop;
pub mod environment;
pub mod files;
mod harness;
pub mod performers;
pub mod prepare;
pub mod recorder;

pub use display_chain::display_chain;
pub use harness::{Harness, HarnessError, RunControl, RunReport, RunRequest};
pub use recorder::{
    MemoryRecorder, Record, RecordKind, RecorderError, RecorderFuture, RunId, RunMeta, RunOutcome,
    RunRecorder,
};
