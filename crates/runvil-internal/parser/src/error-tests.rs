//! Tests for the error classification and the cause-bearing error chains.

use runvil_lua::Error as LuaError;

use super::{Error, ParseError, ParseErrorKind};
use crate::test_support::{one_section, parse_err, prompt};

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

#[track_caller]
fn assert_no_segment_repeats(error: &(dyn std::error::Error + 'static)) {
    let segments = chain(error);
    for (index, segment) in segments.iter().enumerate() {
        for later in &segments[index + 1..] {
            assert!(
                !segment.contains(later.as_str()),
                "required each chain segment once, actual {later:?} repeated inside {segment:?}"
            );
        }
    }
}

#[test]
fn no_segment_of_a_cause_bearing_parse_error_chain_repeats() {
    for (src, kind) in [
        (prompt(": : :", &[]), ParseErrorKind::Frontmatter),
        (one_section("x ="), ParseErrorKind::Lua),
    ] {
        let error = parse_err(&src);
        assert_eq!(error.kind(), kind, "{error}");
        assert!(
            error.cause().is_some(),
            "the cause stays reachable: {error:?}"
        );
        assert!(
            std::error::Error::source(&error).is_none(),
            "required no source, since Display already holds the cause, actual a source on \
             {error:?}"
        );
        assert_no_segment_repeats(&error);

        let inner = error.into_inner();
        assert!(inner.cause().is_some(), "{inner:?}");
        assert!(std::error::Error::source(&inner).is_none(), "{inner:?}");
        assert_no_segment_repeats(&inner);
    }
}

#[test]
fn a_broken_parser_invariant_classifies_as_internal() {
    let error = ParseError::from_inner(Error::Internal("parser: a test invariant broke"));
    assert_eq!(error.kind(), ParseErrorKind::Internal);
    assert_eq!(
        error.to_string(),
        "internal invariant violated: parser: a test invariant broke"
    );
    assert_eq!(error.span(), None);
    assert_eq!(error.line(), None);
}

#[test]
fn a_lua_compiler_that_cannot_start_classifies_as_internal() {
    let vm_failure = mlua::Error::MemoryError("not enough memory".to_owned());
    let error = ParseError::from_inner(Error::Lua(LuaError::LuaRuntime {
        message: vm_failure.to_string(),
        cause: Box::new(vm_failure),
    }));
    assert_eq!(error.kind(), ParseErrorKind::Internal);
}
