//! The payload-free lifecycle boundaries as named constructors.
//!
//! An emit site names a boundary (`lifecycle::RUN_STARTED`) and hands it
//! to [`Emitter::report`](crate::emitter::Emitter::report), which stamps
//! the run's coordinates on it. Each constant is the constructor
//! of the matching [`Event`] variant, declared once from one list so a
//! boundary cannot gain a constant without gaining a variant.
//!
//! An emit-site vocabulary for the engine crates; the facade does not
//! re-export it. A host reads the events themselves.

use super::Event;
use crate::ids::Provenance;

/// A payload-free lifecycle boundary: the constructor of one [`Event`]
/// variant that takes only the three coordinates.
pub type Lifecycle = fn(String, String, Provenance) -> Event;

/// Declares one constant per payload-free lifecycle variant, each the
/// variant's constructor over the three coordinates.
macro_rules! lifecycle_constants {
    ($($name:ident => $variant:ident),* $(,)?) => {
        $(
            #[doc = concat!("The [`Event::", stringify!($variant), "`] boundary.")]
            pub const $name: Lifecycle = |execution, section, provenance| Event::$variant {
                execution,
                section,
                provenance,
            };
        )*

        /// Every payload-free lifecycle constructor beside its variant
        /// name, for a test that pins the list to the enum.
        #[cfg(test)]
        pub(crate) const ALL: &[(&str, Lifecycle)] = &[
            $((stringify!($variant), $name),)*
        ];
    };
}

lifecycle_constants! {
    PARSE_STARTED => ParseStarted,
    PARSE_SUCCEEDED => ParseSucceeded,
    PARSE_FAILED => ParseFailed,
    RUN_STARTED => RunStarted,
    RUN_SUCCEEDED => RunSucceeded,
    RUN_FAILED => RunFailed,
    SECTION_STARTED => SectionStarted,
    SECTION_FINISHED => SectionFinished,
    MODEL_TURN_COMPLETED => ModelTurnCompleted,
    MODEL_TURN_FAILED => ModelTurnFailed,
    MODEL_TURN_TRUNCATED => ModelTurnTruncated,
    TOOL_CALL_SUCCEEDED => ToolCallSucceeded,
    TOOL_CALL_FAILED => ToolCallFailed,
    LUA_COMPILATION_STARTED => LuaCompilationStarted,
    LUA_COMPILATION_SUCCEEDED => LuaCompilationSucceeded,
    LUA_COMPILATION_FAILED => LuaCompilationFailed,
    LUA_SHARED_LOAD_STARTED => LuaSharedLoadStarted,
    LUA_SHARED_LOAD_SUCCEEDED => LuaSharedLoadSucceeded,
    LUA_SHARED_LOAD_FAILED => LuaSharedLoadFailed,
    LUA_CHUNK_STARTED => LuaChunkStarted,
    LUA_CHUNK_SUCCEEDED => LuaChunkSucceeded,
    LUA_CHUNK_FAILED => LuaChunkFailed,
    LUA_REPLY_BINDING_STARTED => LuaReplyBindingStarted,
    LUA_REPLY_BINDING_SUCCEEDED => LuaReplyBindingSucceeded,
    LUA_REPLY_BINDING_FAILED => LuaReplyBindingFailed,
    LUA_TEARDOWN_STARTED => LuaTeardownStarted,
    LUA_TEARDOWN_SUCCEEDED => LuaTeardownSucceeded,
    TOOL_SCOPE_VALIDATION_STARTED => ToolScopeValidationStarted,
    TOOL_SCOPE_VALIDATION_SUCCEEDED => ToolScopeValidationSucceeded,
    TOOL_SCOPE_VALIDATION_FAILED => ToolScopeValidationFailed,
    MODEL_CATALOG_VALIDATION_STARTED => ModelCatalogValidationStarted,
    MODEL_CATALOG_VALIDATION_SUCCEEDED => ModelCatalogValidationSucceeded,
    MODEL_CATALOG_VALIDATION_FAILED => ModelCatalogValidationFailed,
    STORE_WRITE_SUCCEEDED => StoreWriteSucceeded,
    STORE_WRITE_FAILED => StoreWriteFailed,
    STORE_APPEND_SUCCEEDED => StoreAppendSucceeded,
    STORE_APPEND_FAILED => StoreAppendFailed,
    STORE_READ_SUCCEEDED => StoreReadSucceeded,
    STORE_READ_FAILED => StoreReadFailed,
    STORE_READ_NUMBERED_SUCCEEDED => StoreReadNumberedSucceeded,
    STORE_READ_NUMBERED_FAILED => StoreReadNumberedFailed,
    STORE_REPLACE_SUCCEEDED => StoreReplaceSucceeded,
    STORE_REPLACE_FAILED => StoreReplaceFailed,
    STORE_DELETE_SUCCEEDED => StoreDeleteSucceeded,
    STORE_DELETE_FAILED => StoreDeleteFailed,
    STORE_GLOB_SUCCEEDED => StoreGlobSucceeded,
    STORE_GLOB_FAILED => StoreGlobFailed,
    USER_INPUT_WAIT_STARTED => UserInputWaitStarted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_constant_builds_the_variant_it_is_named_for() {
        let provenance = Provenance {
            task: "0".parse().expect("a task id parses"),
            seq: 0,
        };
        for (variant, build) in ALL {
            let event = build("run".to_owned(), "S".to_owned(), provenance.clone());
            let json = serde_json::to_value(&event).expect("an event serializes");
            let mut expected = String::new();
            for (index, ch) in variant.chars().enumerate() {
                if ch.is_ascii_uppercase() {
                    if index > 0 {
                        expected.push('_');
                    }
                    expected.push(ch.to_ascii_lowercase());
                } else {
                    expected.push(ch);
                }
            }
            assert_eq!(json["kind"], expected, "{variant} builds its own kind");
            assert_eq!(event.execution(), "run");
            assert_eq!(event.section(), "S");
            assert_eq!(event.provenance(), &provenance);
        }
    }
}
