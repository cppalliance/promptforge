//! workshop-agents - Workshop's agent conversations: the conversation
//! table keyed by a Workshop-minted id, each conversation's transcript,
//! state, and failure reports, the operator's input waits, discovery over
//! the agents folder with the built-in `chat` agent, the Host's timer, and
//! the recorder tee every run of a conversation writes through.
//!
//! A conversation is one launched agent and its one run. The server
//! builds that run's Harness from what the [`Conversation`] hands it - the
//! recorder tee and a clone of the Host's services with the
//! conversation's input broker - streams the run's own rounds into it
//! through [`Conversation::publish_delta`], and drives it with
//! [`Conversation::run`]. A stop drops the round in
//! flight and keeps the run going; a close cancels the run, and the
//! conversation ends when the run does.
//!
//! ## Invariants
//!
//! - Tier: feature; may depend on: the vocabulary crates
//!   (`workshop-protocol`, `workshop-registry`, `workshop-support`), the
//!   service crates (`workshop-gateway`, `workshop-menu`,
//!   `workshop-status`), the Harness's public API `harness`, the Engine's
//!   public API `promptforge`, and third-party crates; today it names
//!   none of the Workshop crates. Never on `workshop-server`, another
//!   feature crate, or a private Harness, Engine, or Gateway crate.
//!   `cargo test -p build-xtask` enforces the product and container
//!   boundaries.
//! - A conversation's transcript is held in memory. The recorder tee adds
//!   each event to it only after the inner recorder accepts the event, so
//!   the record, the live broadcast, and the transcript agree event for
//!   event, and each event takes its `reply` from its own round.
//! - A dying input wait is an outcome, never silence: every path out of
//!   an unresolved wait removes the entry and pushes a durable cancelled
//!   frame.
//! - A conversation's channels close when it ends, so whatever holds only
//!   its receivers ends with it.

mod conversation;
mod discovery;
mod input;
mod protocol;
mod state;
mod table;
mod tee;
mod timer;
mod transcript;

pub use conversation::Conversation;
pub use discovery::{BUILTIN_CHAT_SOURCE, LaunchError, discover_agents, load_agent};
pub use input::{SessionInputBroker, WaitError, WaitFrame, WaitRegistry, complete_input_response};
pub use protocol::{ConversationId, Delta, DeltaKind, SessionEvent};
pub use state::{FailureKind, SessionFailure, SessionState};
pub use table::Conversations;
pub use timer::TokioTimer;
