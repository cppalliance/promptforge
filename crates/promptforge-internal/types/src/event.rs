//! The Engine's event vocabulary: everything a run reports, as values.
//!
//! An [`Event`] is one thing that happened during a run, returned to the
//! caller from `Run::step` beside the effects the run wants performed. It is
//! the one report-only vocabulary: lifecycle boundaries, content the model,
//! tools, and user produced, and the opt-in debug capture, in one
//! serializable enum. The Engine emits through an
//! [`Emitter`](crate::emitter::Emitter), the caller receives it from `step`,
//! and nothing is ever read back into the Engine by this path: recording every
//! event or dropping them all leaves a run's outputs, errors, and ordering
//! unchanged. The payload-free lifecycle variants have named constructors
//! in [`lifecycle`] for the Engine's emit sites.
//!
//! Every variant includes three coordinates before its payload: `execution`
//! (the caller-chosen run identifier), `section` (the reporting H2 heading
//! or agent name), and `provenance` (the [`Provenance`] replay key: the
//! nearest enclosing task and the item's position within it). A caller can
//! key every event by `provenance` alone, without inspecting the payload.
//!
//! # Sensitivity
//! Lifecycle variants have no payload beyond their coordinates, and the
//! coordinates themselves are author-controlled (`execution` is caller
//! chosen, `section` is prompt-authored heading text); the exception is
//! `ModelMetadataDegraded`, whose message may quote values from a
//! backend's response. Content variants hold model-, tool-, or
//! user-authored text; task variants hold the author's spawn seeds; debug
//! variants hold the verbatim request and response bodies. A caller that
//! persists or forwards events must treat all of it as untrusted.
//!
//! # Serialized form
//! One event serializes to one JSON object tagged by `kind` (the variant
//! name in `snake_case`) with the three coordinates and then the payload
//! fields beside it.

use serde::{Deserialize, Serialize};

use crate::ids::{AbandonReason, Provenance, RoundId, TaskId, TaskOrigin};
use crate::metrics::{CallMetrics, ToolCallEvent};
use crate::tools::ToolId;

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

/// The kind of model round that produced an [`Event::AssistantReply`]: a
/// user-facing chat turn ([`Chat`](Self::Chat)) or a programmatic
/// inference round ([`Infer`](Self::Infer)).
///
/// The default is `chat`, so a serialized reply that omits `origin`
/// reads back as a chat reply. A `Chat` effect's round carries the same
/// value in its `origin` field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReplyOrigin {
    /// A user-facing chat turn: the reply belongs in the conversation.
    #[default]
    Chat,
    /// A programmatic inference round started by `models.infer`. The reply
    /// is a model result that the application may handle apart from the
    /// conversation.
    Infer,
}

events! {
    /// One thing that happened during a run.
    ///
    /// Variants fall into four groups:
    ///
    /// - Lifecycle variants, from the first through [`Lua`](Self::Lua),
    ///   mark operational boundaries. The payload-free ones carry only
    ///   the coordinates. The `Vfs` variants report the run's
    ///   `store.*` calls, which go through the caller.
    /// - Task variants report a task chain's start and end.
    /// - Content variants hold what a model, a tool, or a user produced.
    /// - Debug variants hold the raw model-turn bodies.
    ///
    /// Every variant has three coordinates ahead of its payload:
    /// `execution`, `section`, and `provenance`. An event serializes to one
    /// JSON object tagged by `kind`, the variant name in `snake_case`, with
    /// the coordinates and the payload fields beside it.
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
        /// A model turn returned a text reply that stopped at the model's
        /// length limit.
        ModelTurnTruncated {},
        /// One metadata section of a completed model turn's response was
        /// present but malformed and degraded to `None`, or the response
        /// omitted the model name. The turn itself succeeded; each degraded
        /// section reports once, after the turn's `model_turn_completed`.
        ModelMetadataDegraded {
            /// The model-turn counter the response was served under.
            turn: u32,
            /// The Engine's sentence naming the section and why it failed
            /// to parse; it may quote backend-supplied values.
            message: String,
        },
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
        /// Validation of model bindings against the live model catalog
        /// began.
        ModelCatalogValidationStarted {},
        /// Validation of model bindings against the live model catalog
        /// succeeded.
        ModelCatalogValidationSucceeded {},
        /// Validation of model bindings against the live model catalog
        /// failed.
        ModelCatalogValidationFailed {},
        // Lifecycle: operations on the run's store view (the `store.*` calls).
        /// A `store.write` call succeeded.
        VfsWriteSucceeded {},
        /// A `store.write` call failed.
        VfsWriteFailed {},
        /// A `store.append` call succeeded.
        VfsAppendSucceeded {},
        /// A `store.append` call failed.
        VfsAppendFailed {},
        /// A `store.read` call, which reads verbatim, succeeded.
        VfsReadSucceeded {},
        /// A `store.read` call, which reads verbatim, failed.
        VfsReadFailed {},
        /// A `store.read_numbered` call succeeded.
        VfsReadNumberedSucceeded {},
        /// A `store.read_numbered` call failed.
        VfsReadNumberedFailed {},
        /// A `store.str_replace` call succeeded.
        VfsReplaceSucceeded {},
        /// A `store.str_replace` call failed.
        VfsReplaceFailed {},
        /// A `store.delete` call succeeded.
        VfsDeleteSucceeded {},
        /// A `store.delete` call failed.
        VfsDeleteFailed {},
        /// A `store.glob` call succeeded.
        VfsGlobSucceeded {},
        /// A `store.glob` call failed.
        VfsGlobFailed {},
        /// A `store.exists` existence check succeeded.
        VfsExistsSucceeded {},
        /// A `store.exists` existence check failed.
        VfsExistsFailed {},
        // Lifecycle: the author's checkpoints.
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
        /// is the task's spawn seeds: everything the caller needs to start
        /// the same chain again under the same id. The event is reported
        /// under the spawning section.
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
        /// A task's chain ended with a result, which ends the task. The
        /// event is reported under the task's target section.
        TaskSucceeded {
            /// The task's id.
            task: TaskId,
        },
        /// A task's chain ended with an error, which ends the task. The
        /// event is reported under the task's target section.
        TaskFailed {
            /// The task's id.
            task: TaskId,
        },
        /// A task ended because its owner cancelled it on purpose. The event
        /// is reported once per task, under the task's target section.
        TaskCancelled {
            /// The task's id.
            task: TaskId,
        },
        /// A task ended because it lost its owner: its owner chain ended
        /// while the task was live, and the Engine ended the task.
        TaskAbandoned {
            /// The task's id.
            task: TaskId,
            /// How the owner ended.
            reason: AbandonReason,
        },
        /// Reports an existing task revived from its record. The Engine
        /// never emits it.
        TaskResumed {
            /// The task's id.
            task: TaskId,
        },
        // Content.
        /// One completed block of model thinking.
        Thinking {
            /// The model-turn counter the block was produced under.
            turn: u32,
            /// The round that produced it: the id its `Chat` effect held.
            round: RoundId,
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
            /// The round that produced it: the id its `Chat` effect held.
            round: RoundId,
            /// The reply text: untrusted model output.
            text: String,
            /// The provider's stop label, when it sent one.
            finish_reason: Option<String>,
            /// The model that produced the reply.
            model: String,
            /// The call's measurements, when any were reported.
            metrics: Option<CallMetrics>,
            /// The kind of round that produced the reply: a programmatic
            /// inference round (`infer`) or a user-facing chat turn
            /// (`chat`). Defaults to `chat` when a serialized reply omits
            /// `origin`.
            #[serde(default)]
            origin: ReplyOrigin,
        },
        /// One batch of tool calls the model requested, reported before
        /// dispatch.
        AssistantToolCalls {
            /// The model-turn counter the batch was requested under.
            turn: u32,
            /// The round that requested it: the id its `Chat` effect held.
            round: RoundId,
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
            /// The bound tool `alias` resolved to; `None` for a Lua-local
            /// tool, a task built-in, or an event logged before the field.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            tool: Option<ToolId>,
            /// The tool's output: untrusted unless `trusted`.
            content: String,
            /// Whether the dispatch treated the tool as trusted (its output
            /// not nonce-wrapped).
            trusted: bool,
        },
        /// A notice queued for a task's owner: the Engine's sentence telling
        /// the model how a task it started ended. A completed task's final
        /// text is embedded nonce-wrapped as untrusted; the rest of the
        /// sentence is the Engine's. The event is reported under the
        /// owner's section.
        TaskNotice {
            /// The owner's model-turn counter when the notice was queued.
            turn: u32,
            /// The task that ended.
            task: TaskId,
            /// The sentence the model reads.
            text: String,
        },
        /// Reports a task's own progress note, set through `tasks.note`.
        /// The Engine never emits it: it stores the note on the task's
        /// chain, and the task's owner reads it through `task_status`.
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
