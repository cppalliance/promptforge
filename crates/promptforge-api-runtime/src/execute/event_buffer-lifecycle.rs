//! The vocabulary bridge between today's [`Observation`] and the [`Event`]
//! values the engine emits: the payload-free pairs declared once and
//! mapped both ways, and the payload-carrying variants crossed field for
//! field. The `Emitter` builds events through [`lifecycle_event`]; the
//! `events_to_observer` adapter reads them back through
//! [`unit_observation`].

use promptforge_api_types::event::Event;
use promptforge_api_types::ids::Provenance;

use crate::observe::Observation;

/// Declares the payload-free lifecycle pairs once and derives both
/// directions of the mapping from the one list, so a variant can never be
/// mapped one way and forgotten the other.
macro_rules! lifecycle_pairs {
    ($($variant:ident),* $(,)?) => {
        /// The constructor for the payload-free [`Event`] matching a
        /// payload-free [`Observation`], or `None` for a payload-carrying
        /// variant.
        fn unit_constructor(
            observation: &Observation,
        ) -> Option<fn(String, String, Provenance) -> Event> {
            let constructor: fn(String, String, Provenance) -> Event = match observation {
                $(
                    Observation::$variant => |execution, section, provenance| Event::$variant {
                        execution,
                        section,
                        provenance,
                    },
                )*
                _ => return None,
            };
            Some(constructor)
        }

        /// The payload-free [`Observation`] matching a payload-free
        /// [`Event`], or `None` for any other variant.
        #[cfg_attr(
            not(any(test, feature = "test-support")),
            allow(dead_code, reason = "read by the test drivers' observer adapter alone")
        )]
        pub(crate) fn unit_observation(event: &Event) -> Option<Observation> {
            Some(match event {
                $(Event::$variant { .. } => Observation::$variant,)*
                _ => return None,
            })
        }

        /// The payload-free lifecycle variants as one or-pattern, so an
        /// exhaustive `match` over [`Event`] elsewhere names the group
        /// from this list rather than repeating it.
        #[cfg_attr(
            not(any(test, feature = "test-support")),
            allow(unused_macros, reason = "read by the test drivers' observer adapter alone")
        )]
        macro_rules! unit_lifecycle_variants {
            () => {
                $(promptforge_api_types::event::Event::$variant { .. })|*
            };
        }
        #[cfg_attr(
            not(any(test, feature = "test-support")),
            allow(unused_imports, reason = "read by the test drivers' observer adapter alone")
        )]
        pub(crate) use unit_lifecycle_variants;
    };
}

lifecycle_pairs! {
    ParseStarted,
    ParseSucceeded,
    ParseFailed,
    RunStarted,
    RunSucceeded,
    RunFailed,
    SectionStarted,
    SectionFinished,
    ModelTurnCompleted,
    ModelTurnFailed,
    ModelTurnTruncated,
    ToolCallSucceeded,
    ToolCallFailed,
    LuaCompilationStarted,
    LuaCompilationSucceeded,
    LuaCompilationFailed,
    LuaSharedLoadStarted,
    LuaSharedLoadSucceeded,
    LuaSharedLoadFailed,
    LuaChunkStarted,
    LuaChunkSucceeded,
    LuaChunkFailed,
    LuaReplyBindingStarted,
    LuaReplyBindingSucceeded,
    LuaReplyBindingFailed,
    LuaTeardownStarted,
    LuaTeardownSucceeded,
    ToolScopeValidationStarted,
    ToolScopeValidationSucceeded,
    ToolScopeValidationFailed,
    ModelCatalogValidationStarted,
    ModelCatalogValidationSucceeded,
    ModelCatalogValidationFailed,
    StoreWriteSucceeded,
    StoreWriteFailed,
    StoreAppendSucceeded,
    StoreAppendFailed,
    StoreReadSucceeded,
    StoreReadFailed,
    StoreReadNumberedSucceeded,
    StoreReadNumberedFailed,
    StoreReplaceSucceeded,
    StoreReplaceFailed,
    StoreDeleteSucceeded,
    StoreDeleteFailed,
    StoreGlobSucceeded,
    StoreGlobFailed,
    UserInputWaitStarted,
}

/// The [`Event`] form of one [`Observation`] under the given coordinates.
/// The payload-free variants map through the shared pair list; the
/// message and task variants carry their payloads across field for field.
/// `Observation` is `#[non_exhaustive]` across the crate seam, so a variant
/// this build does not know lands as [`Event::Other`] with its trace line.
pub(super) fn lifecycle_event(
    observation: Observation,
    execution: String,
    section: String,
    provenance: Provenance,
) -> Event {
    if let Some(constructor) = unit_constructor(&observation) {
        return constructor(execution, section, provenance);
    }
    match observation {
        Observation::Lua(message) => Event::Lua {
            execution,
            section,
            provenance,
            message,
        },
        Observation::Other(message) => Event::Other {
            execution,
            section,
            provenance,
            message,
        },
        Observation::TaskStarted {
            task,
            target,
            origin,
            input,
            item,
            index,
            var,
        } => Event::TaskStarted {
            execution,
            section,
            provenance,
            task,
            target,
            origin,
            input,
            item,
            index,
            var,
        },
        Observation::TaskSucceeded { task } => Event::TaskSucceeded {
            execution,
            section,
            provenance,
            task,
        },
        Observation::TaskFailed { task } => Event::TaskFailed {
            execution,
            section,
            provenance,
            task,
        },
        Observation::TaskCancelled { task } => Event::TaskCancelled {
            execution,
            section,
            provenance,
            task,
        },
        Observation::TaskAbandoned { task, reason } => Event::TaskAbandoned {
            execution,
            section,
            provenance,
            task,
            reason,
        },
        other => Event::Other {
            execution,
            section,
            provenance,
            message: other.to_string(),
        },
    }
}
