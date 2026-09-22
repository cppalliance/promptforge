//! The engine's event vocabulary: everything a run reports, as values.
//!
//! An [`Event`] is one thing that happened during a run, returned to the
//! host from `Run::step` beside the effects the run wants performed. It is
//! the one report-only vocabulary: lifecycle boundaries, content the model,
//! tools, and user produced, and the opt-in debug capture, in one
//! serializable enum. The engine emits through an
//! [`Emitter`](crate::emitter::Emitter), the host appends to its log, and
//! nothing is ever read back into the engine by this path: recording every
//! event or dropping them all leaves a run's outputs, errors, and ordering
//! unchanged. The payload-free lifecycle variants have named constructors
//! in [`lifecycle`] for the engine's emit sites.
//!
//! Every variant includes three coordinates before its payload: `execution`
//! (the caller-chosen run identifier), `section` (the reporting H2 heading
//! or agent name), and `provenance` (the [`Provenance`] replay key: the
//! nearest enclosing task and the item's position within it). A host writes
//! `task_id` and `task_seq` for every record from `provenance` alone,
//! without inspecting the payload.
//!
//! # Sensitivity
//! Lifecycle variants have no payload beyond their coordinates, and the
//! coordinates themselves are author-controlled (`execution` is caller
//! chosen, `section` is prompt-authored heading text). Content variants
//! hold model-, tool-, or user-authored text; task variants hold the
//! author's spawn seeds; debug variants hold the verbatim request and
//! response bodies. A host that persists or forwards events owns treating
//! all of it as untrusted.
//!
//! # Serialized form
//! One event serializes to one JSON object tagged by `kind` (the variant
//! name in `snake_case`) with the three coordinates and then the payload
//! fields beside it:
//!
//! ```
//! use promptforge_api_types::event::Event;
//! use promptforge_api_types::ids::Provenance;
//!
//! let event = Event::SectionStarted {
//!     execution: "run-1".to_owned(),
//!     section: "Gather".to_owned(),
//!     provenance: Provenance { task: "0".parse()?, seq: 4 },
//! };
//! assert_eq!(
//!     serde_json::to_string(&event)?,
//!     r#"{"kind":"section_started","execution":"run-1","section":"Gather","provenance":{"task":"0","seq":4}}"#
//! );
//! assert_eq!(event.provenance().seq, 4);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use serde::{Deserialize, Serialize};

use crate::ids::{AbandonReason, Provenance, TaskId, TaskOrigin};
use crate::metrics::{CallMetrics, ToolCallEvent};

#[doc(hidden)]
#[path = "event-lifecycle.rs"]
pub mod lifecycle;

#[cfg(test)]
#[path = "event-tests.rs"]
mod tests;

/// Declares the event enum with the three coordinates stamped on every
/// variant ahead of its own fields, and the coordinate accessors over all
/// of them. The macro exists so the coordinates are written once and can
/// never be left off a variant.
macro_rules! events {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident {
                    $(
                        $(#[$field_meta:meta])*
                        $field:ident : $ty:ty
                    ),* $(,)?
                }
            ),* $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        #[non_exhaustive]
        pub enum $name {
            $(
                $(#[$variant_meta])*
                $variant {
                    /// The caller-chosen run identifier.
                    execution: String,
                    /// The reporting scope: the prompt's H2 heading text,
                    /// or an agent's name.
                    section: String,
                    /// The replay key: the nearest enclosing task and this
                    /// event's position within it.
                    provenance: Provenance,
                    $(
                        $(#[$field_meta])*
                        $field: $ty,
                    )*
                },
            )*
        }

        impl $name {
            /// The caller-chosen run identifier this event belongs to.
            #[must_use]
            pub fn execution(&self) -> &str {
                match self {
                    $( $name::$variant { execution, .. } )|* => execution,
                }
            }

            /// The reporting scope this event was reported under.
            #[must_use]
            pub fn section(&self) -> &str {
                match self {
                    $( $name::$variant { section, .. } )|* => section,
                }
            }

            /// The replay key: the nearest enclosing task and this event's
            /// position within it.
            #[must_use]
            pub fn provenance(&self) -> &Provenance {
                match self {
                    $( $name::$variant { provenance, .. } )|* => provenance,
                }
            }
        }
    };
}

/// Which path produced one [`Event::AssistantReply`]: a user-facing chat
/// turn ([`Chat`](Self::Chat)) or a programmatic inference round
/// ([`Infer`](Self::Infer)).
///
/// The default is `chat`, so an older log written before the field existed
/// reads back as a chat reply. The enum is `#[non_exhaustive]`, so a host
/// matches the two known origins and keeps a wildcard for a future one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReplyOrigin {
    /// A user-facing chat turn: the reply belongs in the conversation.
    #[default]
    Chat,
    /// A programmatic inference round (`models.infer`): the reply is a
    /// model result the host may treat apart from the conversation.
    Infer,
}

events! {
    /// One thing that happened during a run.
    ///
    /// Variants fall into four groups. Lifecycle variants (the first group,
    /// through [`Lua`](Self::Lua)) mark operational boundaries; the
    /// payload-free ones have constructors in [`lifecycle`]. Task
    /// variants report a task chain's start and end. Content variants
    /// hold what a model, tool, or user produced. Debug variants hold the
    /// raw model-turn bodies. Every variant has `execution`, `section`,
    /// and `provenance` ahead of its payload; see the module docs.
    pub enum Event {
        // Lifecycle: parse and run.
        /// Prompt parsing began.
        ParseStarted {},
        /// Prompt parsing and parse-time compilation completed successfully.
        ParseSucceeded {},
        /// Prompt parsing or parse-time compilation returned an error.
        ParseFailed {},
        /// A run passed its version gate and began.
        RunStarted {},
        /// A run returned a value.
        RunSucceeded {},
        /// A run returned an error.
        RunFailed {},
        /// A top-level section began.
        SectionStarted {},
        /// A top-level section completed successfully.
        SectionFinished {},
        // Lifecycle: model turns and tool calls.
        /// A model round trip completed successfully.
        ModelTurnCompleted {},
        /// A model round trip returned an error.
        ModelTurnFailed {},
        /// A successful parse ended because the model hit its length limit.
        ModelTurnTruncated {},
        /// A tool dispatch completed successfully.
        ToolCallSucceeded {},
        /// A tool dispatch returned an error.
        ToolCallFailed {},
        // Lifecycle: the section VM.
        /// Lua source compilation began.
        LuaCompilationStarted {},
        /// Lua source compilation completed successfully.
        LuaCompilationSucceeded {},
        /// Lua source compilation returned an error.
        LuaCompilationFailed {},
        /// A section VM began loading and executing its shared program.
        LuaSharedLoadStarted {},
        /// A section VM loaded and executed its shared program successfully.
        LuaSharedLoadSucceeded {},
        /// A section VM failed to load or execute its shared program.
        LuaSharedLoadFailed {},
        /// A section VM began executing a Lua chunk.
        LuaChunkStarted {},
        /// A section VM executed a Lua chunk successfully.
        LuaChunkSucceeded {},
        /// A section VM failed to execute a Lua chunk.
        LuaChunkFailed {},
        /// A section VM began binding a model reply.
        LuaReplyBindingStarted {},
        /// A section VM bound a model reply successfully.
        LuaReplyBindingSucceeded {},
        /// A section VM failed to bind a model reply.
        LuaReplyBindingFailed {},
        /// A section VM began teardown.
        LuaTeardownStarted {},
        /// A section VM completed teardown.
        LuaTeardownSucceeded {},
        // Lifecycle: validation.
        /// Semantic validation of a model-visible tool scope began.
        ToolScopeValidationStarted {},
        /// A model-visible tool scope passed semantic validation.
        ToolScopeValidationSucceeded {},
        /// A model-visible tool scope failed semantic validation.
        ToolScopeValidationFailed {},
        /// Live-catalog model binding validation began.
        ModelCatalogValidationStarted {},
        /// Live-catalog model binding validation succeeded.
        ModelCatalogValidationSucceeded {},
        /// Live-catalog model binding validation failed.
        ModelCatalogValidationFailed {},
        // Lifecycle: store operations.
        /// A harness-mediated store write succeeded.
        StoreWriteSucceeded {},
        /// A harness-mediated store write failed.
        StoreWriteFailed {},
        /// A harness-mediated store append succeeded.
        StoreAppendSucceeded {},
        /// A harness-mediated store append failed.
        StoreAppendFailed {},
        /// A harness-mediated store read (verbatim) succeeded.
        StoreReadSucceeded {},
        /// A harness-mediated store read (verbatim) failed.
        StoreReadFailed {},
        /// A harness-mediated store read_numbered succeeded.
        StoreReadNumberedSucceeded {},
        /// A harness-mediated store read_numbered failed.
        StoreReadNumberedFailed {},
        /// A harness-mediated store replacement succeeded.
        StoreReplaceSucceeded {},
        /// A harness-mediated store replacement failed.
        StoreReplaceFailed {},
        /// A harness-mediated store deletion succeeded.
        StoreDeleteSucceeded {},
        /// A harness-mediated store deletion failed.
        StoreDeleteFailed {},
        /// A harness-mediated store glob succeeded.
        StoreGlobSucceeded {},
        /// A harness-mediated store glob failed.
        StoreGlobFailed {},
        // Lifecycle: input and the author's checkpoints.
        /// A section began waiting on operator input.
        UserInputWaitStarted {},
        /// The one author-controlled checkpoint: a validated Lua
        /// `log(message)`. Prompt authors must never place arguments,
        /// replies, tool data, credentials, paths, or store contents in it.
        Lua {
            /// The author's checkpoint text, verbatim.
            message: String,
        },
        // Tasks.
        /// A task chain was started by `tasks.spawn`, by the `fanout` shim
        /// for each of its arms, or by the model's `task` tool. The payload
        /// is the task's spawn seeds: everything a host needs to start the
        /// same chain again under the same id. Reported under the spawning
        /// section.
        TaskStarted {
            /// The task's id: its chain's hierarchical id.
            task: TaskId,
            /// The name of the section the task's chain starts at.
            target: String,
            /// The principal that started the task.
            origin: TaskOrigin,
            /// The `opts.input` override of the chain's `args`, when given.
            input: Option<String>,
            /// The `opts.item` seed installed as the chain's `item` global,
            /// when given.
            item: Option<serde_json::Value>,
            /// The `opts.index` seed reported as the chain's `sys.index`,
            /// when given.
            index: Option<u64>,
            /// The spawner's `var` snapshot the chain seeds from.
            var: serde_json::Value,
        },
        /// Terminal: a task's chain ended with a result. Reported under the
        /// task's target section.
        TaskSucceeded {
            /// The task's id.
            task: TaskId,
        },
        /// Terminal: a task's chain ended with an error. Reported under the
        /// task's target section.
        TaskFailed {
            /// The task's id.
            task: TaskId,
        },
        /// Terminal: the task was cancelled on purpose by its owner. Reported
        /// once under the task's target section; a repeated cancel reports
        /// nothing.
        TaskCancelled {
            /// The task's id.
            task: TaskId,
        },
        /// Terminal: the task's owner chain ended while the task was live,
        /// so the engine ended the task. Distinct from a cancellation: the
        /// task lost its owner rather than being stopped on purpose.
        TaskAbandoned {
            /// The task's id.
            task: TaskId,
            /// How the owner ended.
            reason: AbandonReason,
        },
        /// Reserved: an existing task was revived from its record rather
        /// than started anew. No producer emits it until resume lands; it
        /// is declared now so the log schema has the kind from its first
        /// version.
        TaskResumed {
            /// The task's id.
            task: TaskId,
        },
        // Content.
        /// One completed block of model thinking.
        Thinking {
            /// The model-turn counter the block was produced under.
            turn: u32,
            /// The model that produced it.
            model: String,
            /// The thinking text: untrusted model output.
            text: String,
        },
        /// One completed assistant reply: a model round's text reply,
        /// carrying the [`ReplyOrigin`] of the round that produced it.
        AssistantReply {
            /// The model-turn counter the reply was produced under.
            turn: u32,
            /// The reply text: untrusted model output.
            text: String,
            /// The provider's stop label, when it sent one.
            finish_reason: Option<String>,
            /// The model that produced the reply.
            model: String,
            /// Everything the call measured, when anything reported.
            metrics: Option<CallMetrics>,
            /// The provenance a host inspects to distinguish an inference
            /// round (`infer`) from a user-facing chat turn (`chat`).
            /// Defaults to `chat` when an older log carries no `origin`.
            #[serde(default)]
            origin: ReplyOrigin,
        },
        /// One batch of tool calls the model requested, unexecuted.
        AssistantToolCalls {
            /// The model-turn counter the batch was requested under.
            turn: u32,
            /// The model that requested the calls.
            model: String,
            /// The calls: untrusted model-authored names and arguments.
            calls: Vec<ToolCallEvent>,
        },
        /// The result of one dispatched tool call.
        ToolResult {
            /// The model-turn counter the call was dispatched under.
            turn: u32,
            /// The provider-issued tool-call id the result answers;
            /// providers recycle ids across rounds, so scope it by turn.
            tool_call_id: String,
            /// The alias the call named.
            alias: String,
            /// The tool's output: untrusted unless `trusted`.
            content: String,
            /// Whether the dispatch treated the tool as trusted (its output
            /// not nonce-wrapped).
            trusted: bool,
        },
        /// Text the user supplied, byte-exact.
        UserInput {
            /// The user's text: untrusted input.
            text: String,
        },
        /// One model-task notice as it is queued for the task's owner: the
        /// engine's own sentence telling the model how a task it started
        /// ended. A completed task's final text is embedded nonce-wrapped
        /// as untrusted; the rest of the sentence is the engine's.
        /// Reported under the owner's section.
        TaskNotice {
            /// The owner's model-turn counter when the notice was queued.
            turn: u32,
            /// The task that ended.
            task: TaskId,
            /// The sentence the model reads.
            text: String,
        },
        /// A task set its own progress note through `tasks.note`, the text
        /// its owner reads through `task_status`. Reported under the task's
        /// target section.
        TaskNote {
            /// The task that set the note.
            task: TaskId,
            /// The note: untrusted, authored by the task's model or its
            /// Lua.
            text: String,
        },
        // Debug.
        /// The JSON body sent to the chat-completions endpoint for one
        /// model turn: raw, unredacted, and including the full prompt.
        Request {
            /// The 1-based model-turn number within the run.
            turn: u32,
            /// The serialized request body.
            body: serde_json::Value,
        },
        /// The JSON body returned for one model turn, with parsed metadata.
        Response {
            /// The 1-based model-turn number within the run.
            turn: u32,
            /// The raw response body.
            body: serde_json::Value,
            /// The choice's `finish_reason`, when the backend supplied one.
            finish_reason: Option<String>,
            /// The message's `reasoning_content`, when the backend supplied
            /// one.
            reasoning_content: Option<String>,
        },
    }
}
