//! Tests for the internal error: its source chains through the public
//! wrappers, its bridges from other crates' errors, and the locations and
//! classifications it reports through `RunError`.

use promptforge_lua::Error as LuaError;

use super::*;
use crate::parser::Prompt;

fn assert_source_survives_run_error(error: Error) {
    assert!(
        std::error::Error::source(&error).is_some(),
        "the internal error must preserve its source"
    );
    assert!(
        std::error::Error::source(&crate::RunError::from(error)).is_some(),
        "the public RunError wrapper must keep the source reachable"
    );
}

#[test]
fn context_exhaustion_maps_from_lua_and_classifies() {
    // The compactor's typed exhaustion crosses the crate seam
    // variant-for-variant and classifies as its own run-error kind, so a
    // caller can distinguish context exhaustion from a transport failure.
    let lua_error = LuaError::ContextExhausted {
        reason: promptforge_lua::OverflowReason::Provider,
    };
    let error = Error::from(lua_error);
    assert!(
        matches!(
            error,
            Error::ContextExhausted {
                reason: crate::lua::OverflowReason::Provider
            }
        ),
        "the mapping preserves the reason, got {error:?}"
    );
    assert!(
        error.to_string().starts_with("context exhausted: "),
        "the diagnostic names the exhaustion: {error}"
    );
    let run_error = crate::RunError::from(error);
    assert_eq!(run_error.kind(), crate::RunErrorKind::ContextExhausted);
    assert!(
        !run_error.is_retryable(),
        "retrying an over-window request cannot succeed"
    );
}

#[test]
fn source_bearing_binding_errors_preserve_their_cause() {
    // The binding and tool-scope failures keep the originating typed
    // error as a private `source()` instead of flattening it to a string,
    // and the chain survives through the public `RunError` wrapper.
    let schema_error =
        promptforge_model_client::detail::tool_schema_new("echo", "", serde_json::json!([]))
            .expect_err("an array is not an object schema");
    let bind = Error::BindSchema {
        alias: "echo".to_owned(),
        source: Box::new(schema_error),
    };
    assert_eq!(
        bind.to_string(),
        "model-facing schema build failure for tool alias \"echo\""
    );
    assert_source_survives_run_error(bind);
}

#[test]
fn lua_compile_preserves_the_originating_compiler_error() {
    // A compile failure keeps the concrete `mlua` error as a private
    // `source()` instead of flattening it into `message` alone, and the
    // chain survives through the public `RunError` wrapper.
    let compile = Error::LuaCompile {
        location: "section `S` prologue".to_owned(),
        source_line: 7,
        lua_source: "x =".to_owned(),
        message: "syntax error near '='".to_owned(),
        source: Box::new(mlua::Error::SyntaxError {
            message: "syntax error near '='".to_owned(),
            incomplete_input: false,
        }),
    };
    assert_source_survives_run_error(compile);
}

#[test]
fn typed_error_survives_the_lua_external_boundary() {
    // Passing the typed error (not its `to_string()`) to
    // `mlua::Error::external` keeps the original error as a downcastable
    // source across the Lua boundary, rather than flattening it to text.
    let original = Error::OutOfScopeToolCall {
        name: "echo".to_owned(),
        global_exists: false,
        in_scope: vec!["other".to_owned()],
    };
    let display = original.to_string();
    let external = mlua::Error::external(original);
    match &external {
        mlua::Error::ExternalError(cause) => {
            let recovered = cause
                .downcast_ref::<Error>()
                .expect("the original typed Error is preserved, not stringified");
            assert_eq!(recovered.to_string(), display);
        }
        other => panic!("expected an ExternalError holding the typed error, got {other:?}"),
    }
    // Re-wrapping through the crate's Lua boundary keeps the chain reachable.
    let wrapped = Error::lua(external);
    assert!(std::error::Error::source(&wrapped).is_some());
}

#[test]
fn completion_errors_preserve_their_causes_across_the_error_type_bridge() {
    // A broker's failure arrives as a `CompletionError` with its concrete
    // cause attached; the cause survives both the public
    // `CompletionError::source` and the mapping onto this crate's error
    // type.
    use crate::model::CompletionError;
    use promptforge_model_client::model::CompletionErrorKind;

    let cause = std::io::Error::other("connection refused");
    let completion = CompletionError::new(
        CompletionErrorKind::Unavailable,
        "model access is turned off or not configured",
    )
    .with_source(cause);
    assert_eq!(completion.kind(), CompletionErrorKind::Unavailable);
    assert!(
        std::error::Error::source(&completion).is_some(),
        "the configuration cause must survive the public wrapper"
    );
    let bridged = Error::from(completion);
    assert!(
        matches!(&bridged, Error::Completion(error) if error.kind() == CompletionErrorKind::Unavailable),
        "the error type holds the completion error whole, got {bridged:?}"
    );
    assert!(
        std::error::Error::source(&bridged).is_some(),
        "the cause must survive the bridge"
    );
}

#[test]
fn frontmatter_locations_surface_through_the_run_error() {
    // The parser's surfaced YAML position crosses the error-type
    // bridge and lands on `RunError::location` for navigation. A
    // frontmatter failure predates the prompt's name, so the path is
    // the placeholder a caller replaces with its own label for the source.
    let source = concat!(
        "---\n",
        "name: x\n",
        "description: d\n",
        "plugins:\n",
        "  - not a Plugin id\n",
        "---\n",
        "\n# T\n\n## S\n\np\n",
    );
    let parse = Prompt::parse(source, "test")
        .0
        .expect_err("a Plugin id with spaces must be rejected");
    let run_error = crate::RunError::from(Error::from(parse));
    assert_eq!(run_error.kind(), crate::RunErrorKind::Parse);
    let location = run_error
        .location()
        .expect("a parse failure reports a location");
    assert_eq!(location.line, Some(5));
    assert_eq!(location.column, Some(5));
    assert_eq!(location.span, None);
}

#[test]
fn structured_locations_include_the_prompt_name_through_the_run_error() {
    // A post-frontmatter parse failure reports the prompt's
    // frontmatter name as the location's path, plus the offending
    // span's line and column.
    let source = "---\nname: dup\ndescription: d\n---\n\n# T\n\n## S\n\np\n\n## S\n\nq\n";
    let parse = Prompt::parse(source, "test")
        .0
        .expect_err("duplicate sibling sections must be rejected");
    let run_error = crate::RunError::from(Error::from(parse));
    let location = run_error
        .location()
        .expect("a structured parse failure reports a location");
    assert_eq!(location.path, "dup");
    assert_eq!(location.line, Some(12));
    assert_eq!(location.column, Some(1));
    assert!(location.span.is_some());
}

#[test]
fn internal_faults_report_the_rust_file_and_line() {
    // An internal invariant failure locates itself in the Rust
    // source, captured at the construction site.
    let expected_line = line!() + 1;
    let run_error = crate::RunError::from(Error::internal("a test invariant"));
    let location = run_error
        .location()
        .expect("an internal fault reports a location");
    assert_eq!(location.path, file!(), "the path is the Rust source file");
    assert_eq!(location.line, Some(expected_line));
    assert_eq!(location.column, None);
}

#[test]
fn errors_without_a_location_return_none() {
    // Cancellation and the other non-positional kinds have no source
    // position to navigate to.
    let run_error = crate::RunError::from(Error::Interrupted);
    assert!(run_error.location().is_none());
}

#[test]
fn requirements_unmet_classifies_and_reports_the_notice_as_its_message() {
    // The refusal notice is the whole Display, and the kind classifies
    // it for code. Retrying cannot help: the
    // environment, not the transport, is what falls short.
    let error = Error::RequirementsUnmet {
        notice: "the environment cannot satisfy this prompt:\n- role 'analyst': requires a context of at least 200000 tokens; the current model provides 32000".to_owned(),
    };
    let run_error = crate::RunError::from(error);
    assert_eq!(run_error.kind(), crate::RunErrorKind::RequirementsUnmet);
    assert!(!run_error.is_cancelled());
    assert!(!run_error.is_retryable());
    assert!(run_error.location().is_none());
    assert!(run_error.to_string().contains("analyst"));
}

#[test]
fn a_poisoned_model_set_maps_to_a_lua_error_and_never_a_completion_error() {
    // The run's own model-set mutex failing is not a model failure: it
    // keeps the Lua mapping it always had, and a caller never sees it as a
    // retryable completion kind.
    use crate::model::{ModelSet, ModelView};
    use std::sync::{Arc, Mutex};

    let set = Arc::new(Mutex::new(ModelSet::default()));
    let poisoner = Arc::clone(&set);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock();
        panic!("poison the model set");
    })
    .join();

    let failure = set.bindings().expect_err("a poisoned set cannot be read");
    let error = Error::from(failure);
    assert!(
        matches!(&error, Error::Lua(message) if message == "model set mutex was poisoned"),
        "the lock failure is a Lua error: {error:?}"
    );
    assert!(!crate::RunError::from(error).is_retryable());
}

#[test]
fn every_store_failure_rebuilds_from_its_raised_table_under_its_own_tag() {
    use promptforge_vfs::{PathReason, VfsError};
    let path = || "notes.md".to_owned();
    let failures = [
        VfsError::NotFound { path: path() },
        VfsError::AlreadyExists { path: path() },
        VfsError::NotADirectory { path: path() },
        VfsError::IsADirectory { path: path() },
        VfsError::DirectoryNotEmpty { path: path() },
        VfsError::NotUtf8 {
            path: path(),
            valid_up_to: 7,
        },
        VfsError::InvalidPath {
            path: path(),
            reason: PathReason::Traversal,
        },
        VfsError::InvalidRange {
            path: path(),
            reason: "the bounds are invalid",
        },
        VfsError::Anchor {
            path: path(),
            anchor: "TODO".to_owned(),
            count: 2,
        },
        VfsError::PermissionDenied {
            path: path(),
            reason: "denied".to_owned(),
        },
        VfsError::ApprovalRequired {
            path: path(),
            reason: "ask".to_owned(),
        },
        VfsError::ScopeEnded { path: path() },
        VfsError::ReadOnly { path: path() },
        VfsError::OutsideRoot {
            path: path(),
            detail: "escapes".to_owned(),
        },
        VfsError::RootProtected { path: path() },
        VfsError::NoStore,
        VfsError::CrossMount {
            path: path(),
            detail: "two mounts".to_owned(),
        },
        VfsError::Io {
            path: path(),
            kind: std::io::ErrorKind::PermissionDenied,
            raw_os_error: Some(13),
            message: "denied".to_owned(),
        },
        VfsError::Unsupported {
            path: path(),
            detail: "no".to_owned(),
        },
        VfsError::Conflict {
            path: path(),
            detail: "raced".to_owned(),
        },
        VfsError::Backend {
            message: "gone".to_owned(),
        },
    ];
    for failure in failures {
        let raised = promptforge_lua::Raised {
            kind: promptforge_lua::ErrorKind::Store,
            message: failure.to_string(),
            fields: promptforge_lua::store_error_value_fields(&failure)
                .into_iter()
                .collect(),
        };
        let Error::Store { source, .. } = Error::from(LuaError::Raised(raised)) else {
            panic!("a raised store table maps to a store error: {failure:?}");
        };
        assert_eq!(source.tag(), failure.tag(), "{failure:?}");
        match (&source, &failure) {
            (
                VfsError::NotUtf8 { valid_up_to, .. },
                VfsError::NotUtf8 {
                    valid_up_to: raised_offset,
                    ..
                },
            ) => assert_eq!(valid_up_to, raised_offset, "the offset survives"),
            (
                VfsError::Io {
                    kind, raw_os_error, ..
                },
                _,
            ) => assert_eq!(
                (*kind, *raw_os_error),
                (std::io::ErrorKind::Other, None),
                "a table holds no I/O kind or OS code"
            ),
            _ => {}
        }
    }
}
