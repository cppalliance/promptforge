//! The error-chain convention: a cause-bearing failure's message holds the
//! cause's text, its source chain shows that text once, and its cause stays
//! reachable through `cause()`; a variant that still keeps a `source()`
//! shows each segment of its chain once; an internal failure built with a
//! cause appends it once; and the store probe names a missing store.

use promptforge_lua::VfsOp;
use promptforge_types::tools::{ToolError, ToolErrorKind};
use promptforge_vfs::VfsError;

use super::*;
use crate::parser::Prompt;

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

/// One failure of each variant that keeps its cause in a plain field.
#[track_caller]
fn cause_bearing() -> Vec<Error> {
    let frontmatter = Prompt::parse("---\nname: [unclosed\n---\n\n# T\n\n## S\n\np\n", "test")
        .0
        .expect_err("an unclosed YAML list is refused");
    let syntax = mlua::Error::SyntaxError {
        message: "unexpected symbol near <eof>".to_owned(),
        incomplete_input: true,
    };
    let tool = ToolError::new(ToolErrorKind::Backend, "the backend refused");
    vec![
        Error::from(frontmatter),
        Error::lua(mlua::Error::RuntimeError("boom".to_owned())),
        Error::LuaCompile {
            location: "section `S` prologue".to_owned(),
            source_line: 1,
            lua_source: "x =".to_owned(),
            message: syntax.to_string(),
            cause: Box::new(syntax),
        },
        Error::Tool {
            message: tool.to_string(),
            cause: Box::new(tool),
        },
        Error::store_op(
            &VfsOp::Read {
                path: "a.txt".to_owned(),
                start: None,
                end: None,
            },
            VfsError::NotFound {
                path: "a.txt".to_owned(),
            },
        ),
        Error::store(VfsError::Backend {
            message: "the disk is gone".to_owned(),
        }),
        Error::internal_with_cause("a test invariant", std::io::Error::other("the cause")),
        Error::RequirementsUnmet {
            notice: "the environment cannot satisfy this prompt:\n- the H1 gate failed".to_owned(),
            cause: Some(Box::new(std::io::Error::other("assertion failed"))),
        },
    ]
}

/// One failure of each variant that still keeps its cause as a `source()`,
/// so a chain printer walks more than one segment.
#[track_caller]
fn source_bearing() -> Vec<Error> {
    let schema =
        promptforge_model_client::detail::tool_schema_new("echo", "", serde_json::json!([]))
            .expect_err("an array is not an object schema");
    let transport = crate::model::CompletionErrorKind::Transport;
    vec![
        Error::BindSchema {
            alias: "echo".to_owned(),
            source: Box::new(schema),
        },
        Error::Completion(
            crate::model::CompletionError::new(transport, transport.phrase())
                .with_source(std::io::Error::other("connection reset by peer")),
        ),
    ]
}

/// Asserts that no segment of `error`'s source chain repeats inside an
/// earlier one, and returns how many segments the chain has.
#[track_caller]
fn assert_no_segment_repeats(error: &(dyn std::error::Error + 'static)) -> usize {
    let segments = chain(error);
    for (index, segment) in segments.iter().enumerate() {
        for later in &segments[index + 1..] {
            assert!(
                !segment.contains(later.as_str()),
                "required each chain segment once, actual {later:?} repeated inside {segment:?}"
            );
        }
    }
    segments.len()
}

#[test]
fn no_segment_of_an_error_chain_repeats() {
    for error in cause_bearing() {
        assert!(
            error.cause().is_some(),
            "the cause stays reachable: {error:?}"
        );
        assert!(
            std::error::Error::source(&error).is_none(),
            "required no source, since Display already holds the cause, actual a source on \
             {error:?}"
        );
        let run_error = crate::RunError::from(error);
        assert!(
            run_error.cause().is_some(),
            "the public wrapper reaches the cause: {run_error}"
        );
        assert_eq!(assert_no_segment_repeats(&run_error), 1);
    }
    for error in source_bearing() {
        let run_error = crate::RunError::from(error);
        assert!(
            assert_no_segment_repeats(&run_error) > 1,
            "required a chain of at least two segments to walk, actual one for {run_error:?}"
        );
    }
}

#[test]
fn an_internal_failure_built_with_a_cause_shows_its_text_once_and_keeps_it() {
    let expected_line = line!() + 1;
    let error = Error::internal_with_cause("a test invariant", std::io::Error::other("the cause"));
    assert_eq!(
        error.to_string(),
        "internal invariant violated: a test invariant: the cause"
    );
    assert_eq!(
        error.cause().map(ToString::to_string).as_deref(),
        Some("the cause")
    );
    let location = crate::RunError::from(error)
        .location()
        .expect("an internal fault reports a location");
    assert_eq!(
        location.path,
        file!(),
        "the path is the construction site's"
    );
    assert_eq!(location.line, Some(expected_line));

    let bare = Error::internal("a test invariant");
    assert!(bare.cause().is_none());
    assert_eq!(
        bare.to_string(),
        "internal invariant violated: a test invariant"
    );
}

#[test]
fn a_requirements_refusal_appends_its_cause_once_after_the_notice() {
    let notice = "the environment cannot satisfy this prompt:\n- the H1 gate failed";
    let error = Error::RequirementsUnmet {
        notice: notice.to_owned(),
        cause: Some(Box::new(std::io::Error::other("assertion failed"))),
    };
    assert_eq!(error.to_string(), format!("{notice}: assertion failed"));
    let bare = Error::RequirementsUnmet {
        notice: notice.to_owned(),
        cause: None,
    };
    assert_eq!(bare.to_string(), notice);
}

#[test]
fn a_store_probe_failure_names_a_missing_store_and_keeps_the_fixed_text_otherwise() {
    assert_eq!(
        Error::store(VfsError::NoStore).to_string(),
        "this run declares no store, so no store operation can run"
    );
    let backend = VfsError::Backend {
        message: "the disk is gone".to_owned(),
    };
    let expected = format!("store operation failed: {backend}");
    assert_eq!(Error::store(backend).to_string(), expected);
}
