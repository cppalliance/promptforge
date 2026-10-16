//! Tests for the internal error: the cause-bearing variants' chains and the
//! quota variant's message.

use runvil_types::tools::{ToolError, ToolErrorKind};
use runvil_vfs::VfsError;

use super::{Error, QuotaResource};
use crate::protocol::VfsOp;

/// The display text of each segment of `error`'s source chain, the error
/// itself first.
fn chain(error: &(dyn std::error::Error + 'static)) -> Vec<String> {
    let mut segments = Vec::new();
    let mut next = Some(error);
    while let Some(segment) = next {
        segments.push(segment.to_string());
        next = segment.source();
    }
    segments
}

#[test]
fn no_segment_of_a_cause_bearing_error_chain_repeats() {
    let syntax = mlua::Error::SyntaxError {
        message: "unexpected symbol near <eof>".to_owned(),
        incomplete_input: true,
    };
    let errors = [
        Error::lua(mlua::Error::RuntimeError("boom".to_owned())),
        Error::LuaCompile {
            location: "section `S` prologue".to_owned(),
            source_line: 1,
            lua_source: "x =".to_owned(),
            message: syntax.to_string(),
            cause: Box::new(syntax),
        },
        Error::tool(ToolError::new(
            ToolErrorKind::Backend,
            "the backend refused",
        )),
        Error::store(
            &VfsOp::Read {
                path: "a.txt".to_owned(),
                start: None,
                end: None,
            },
            VfsError::NotFound {
                path: "a.txt".to_owned(),
            },
        ),
    ];
    for error in &errors {
        assert!(
            error.cause().is_some(),
            "the cause stays reachable: {error:?}"
        );
        assert!(
            std::error::Error::source(error).is_none(),
            "required no source, since Display already holds the cause, actual a source on \
             {error:?}"
        );
        let segments = chain(error);
        for (index, segment) in segments.iter().enumerate() {
            for later in &segments[index + 1..] {
                assert!(
                    !segment.contains(later.as_str()),
                    "required each chain segment once, actual {later:?} repeated inside \
                     {segment:?}"
                );
            }
        }
    }
}

#[test]
fn a_quota_error_names_its_exhausted_resource() {
    for (resource, expected) in [
        (QuotaResource::LogEvent, "lua log event quota exceeded"),
        (QuotaResource::LogByte, "lua log byte quota exceeded"),
        (QuotaResource::Instruction, "lua instruction quota exceeded"),
    ] {
        assert_eq!(Error::LuaQuota { resource }.to_string(), expected);
    }
}
