//! workshop-protocol - the wire protocol of the workshop sockets: every
//! JSON frame the server exchanges with the UI, typed in one place, with
//! zero I/O.
//!
//! Frames are grouped by direction - inbound (client to server) first,
//! outbound (server to client) second. Every wire shape is pinned by the
//! plain tests in `tests/it`. The TypeScript half of this contract is
//! `crates/workshop/ui/src/services/protocol.ts`; the two files
//! cross-cite each other so a shape change touches both or neither. The
//! agent-session frame family is additionally pinned by the shared
//! fixture `tests/fixtures/agent-frames.json`, asserted as the same JSON
//! by the fixture test here and by the SPA suite's
//! `crates/workshop/ui/test/agent-wire-fixtures.mjs`, so drift on either
//! side fails that side's tests. The workshop-socket frame family is
//! pinned the same way by `tests/fixtures/workshop-frames.json`, asserted
//! by the `workshop_frames` test here and by the SPA suite's
//! `crates/workshop/ui/test/workshop-wire-fixtures.mjs`. The wire shapes
//! are additionally frozen end to end by the characterization tests in
//! `workshop-server`'s `tests/it`.
//!
//! ## Invariants
//!
//! - Tier: vocabulary; may depend on: no internal `workshop-*` crates.
//!   Read the repository-root `AGENTS.md` before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Zero I/O: no sockets, tasks, or clocks, so every wire shape is
//!   pinned by a plain test.
//!
//! # Inbound workshop-socket frames
//!
//! `{"type":"select_model","model":"..."}`
//! ([`SelectModelFrame`]) selects the chat model: the menu validates the
//! id against the retained catalog and publishes a fresh
//! [`WorkbenchFrame`] on success; an unknown model is refused with an
//! `error` frame. `{"type":"switch_profile","name":"..."}`
//! ([`SwitchProfileFrame`]) selects a gateway profile, `null` selecting
//! no profile: the pending snapshot publishes immediately, the steps of
//! the selection arrive as [`StatusFrame`]s, and the settled menu
//! publishes a final [`WorkbenchFrame`] and a [`CatalogFrame`]; a switch
//! requested while one runs is refused with an `error` frame. Both
//! events may include an optional `id`, echoed on the `error` frame that
//! refuses them.
//!
//! No inbound frame is pushed by the server, so none takes a delivery
//! classification; the reply frames they trigger are classified below.
//!
//! # Agent-session frames
//!
//! The `/agents/ws` socket speaks its own frame family. On connect the
//! server pushes [`AgentsFrame`], the discovered agent list. The client
//! opens a session with `{"type":"launch","agent":"..."}` or reattaches
//! with `{"type":"attach","session":"..."}`; either is answered with
//! [`AgentSessionFrame`] naming the session id. A running session streams
//! [`AgentEventFrame`]s - the durable event log, each frame holding its
//! log index, replayed from the top on attach - and [`AgentDeltaFrame`]s,
//! the ephemeral live chunks, each stamped with the `reply` id of the
//! durable event that will supersede it. `{"type":"cancel"}` fires the
//! session's turn-cancel: cancellation is a stop reason, never an error -
//! no error frame follows, pending waits die as `input_cancelled`, and
//! the relaunched agent returns to waiting. Frames already in flight
//! from the cancelled run may still arrive between the cancel and the
//! relaunch: a defined grace window, absorbed by the reply-id
//! coalescing (the cancelled round never settles, so its deltas fall to
//! the round that eventually does), never a protocol violation.
//!
//! A session-level failure is pushed as an id-less [`ErrorFrame`]: a
//! model round that failed while the program survived it (the built-in
//! chat `pcall`s `models.chat` and returns to waiting), or a run that
//! ended in error. Delivery on this socket: ephemeral - the reports are
//! sent on a bounded broadcast beside the deltas and may drop under lag; the
//! durable transcript already shows the failed turn as one without a
//! reply, and terminal failures also land on the status bus.
//!
//! # Agent-session input frames
//!
//! An agent session asks its operator for input through the Workshop's
//! `user_input` tool. Three frames make up that conversation: the server
//! pushes [`InputFrame::Required`] when a wait opens and
//! [`InputFrame::Cancelled`] when one dies unresolved, and the client
//! answers with an `input_response` frame parsed as [`InputResponse`].
//! Both pushed frames are durable: the wait registry retains every
//! unresolved wait and the session resends it on reconnect, so a push
//! lost to a dead socket is repaired by the resent set - a live wait
//! reappears, and a stale prompt is dropped because its token is absent.
//! Cancellation is an explicit outcome: every path out of an unresolved
//! wait pushes `input_cancelled` for its token. The session loops that
//! route these frames arrive with agent sessions; the shapes and
//! classification are pinned here first.
//!
//! # Delivery contract
//!
//! Every frame the server pushes has exactly one of two delivery
//! semantics. The session loops are built on this classification, so no
//! pushed frame type ships unclassified.
//!
//! **Durable** frames are always delivered, and may coalesce. Where the
//! data is shared fan-out state, the producer records it and wakes each
//! connection loop through a `Notify`; the loop compares the shared
//! revision against its own per-client cursor and sends everything past
//! the cursor, so a missed wakeup is harmless. A durable frame that answers
//! the connection's own request (a `launch` acknowledgment) is sent
//! directly by the loop that owns the socket, without a cursor - no shared
//! state exists for a cursor to index.
//!
//! **Ephemeral** frames may drop under lag. They are sent on bounded
//! channels (a broadcast where the state fans out); a client too slow
//! to drain its channel lags out and its connection may drop. The drop is
//! harmless because every ephemeral frame has a self-contained repair
//! path: status and catalog are complete snapshots resent on reconnect.
//!
//! ## Classification
//!
//! Workshop socket (`/ws`):
//!
//! - [`ErrorFrame`] - durable on this socket. The direct reply refusing
//!   a malformed frame or a menu event, sent by the loop that owns the
//!   socket - the contract's no-cursor case.
//! - [`StatusFrame`] - ephemeral. Every update is a complete snapshot of
//!   the bar, so a lagging client loses nothing by skipping
//!   intermediates, and the current status is resent on reconnect.
//! - [`CatalogFrame`] - ephemeral. Each push holds the whole catalog
//!   verbatim; the newest push supersedes every older one and the
//!   catalog is resent on reconnect.
//! - [`WorkbenchFrame`] - ephemeral. Every push is a complete snapshot
//!   of the server-owned Model-menu state, retained and resent on
//!   reconnect, like the catalog frame. The connect-time send -
//!   the retained snapshot follows the status and catalog snapshots on
//!   every new session, so the UI boots with zero HTTP state fetches -
//!   is that resend promise, not a third delivery class.

mod agent;
mod catalog;
mod error;
mod input;
mod menu;
mod status;
mod workbench;

pub use agent::{
    AgentDeltaFrame, AgentDeltaKind, AgentEvent, AgentEventFrame, AgentEventKind,
    AgentSessionFrame, AgentsFrame,
};
pub use catalog::{CatalogFrame, CatalogPush, is_chat_capable};
pub use error::{ErrorEnvelope, ErrorFrame};
pub use input::{InputFrame, InputResponse};
pub use menu::{SelectModelFrame, SwitchProfileFrame};
pub use status::{Activity, Severity, StatusBarUpdate, StatusFrame};
pub use workbench::{WorkbenchFrame, WorkbenchSnapshot};
