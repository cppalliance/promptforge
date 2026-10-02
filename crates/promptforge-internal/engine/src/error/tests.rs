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
    // Host can distinguish context exhaustion from a transport failure.
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
    use promptforge_model_client::client::ToolSchemaError;

    let schema_error = ToolSchemaError::NonObjectSchema {
        name: "echo".to_owned(),
    };
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
fn config_errors_preserve_their_causes_across_the_error_type_bridge() {
    // A transport's configuration failure (an unusable credential, a
    // bad endpoint URL) arrives as the client error type's `Config`
    // variant with its concrete cause attached; the cause survives both
    // the public CompletionError::source and the mapping onto this
    // crate's error type, classified as Unavailable.
    use crate::model::CompletionError;
    use promptforge_model_client::Error as ClientError;
    use promptforge_model_client::model::CompletionErrorKind;

    let cause = std::io::Error::other("gateway URL is not a valid URL");
    let completion = CompletionError::from(ClientError::Config {
        message: "gateway endpoint is unusable".to_owned(),
        source: Box::new(cause),
    });
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
    // Step 6: the parser's surfaced YAML position crosses the error-type
    // bridge and lands on `RunError::location` for navigation. A
    // frontmatter failure predates the prompt's name, so the path is
    // the placeholder the Host replaces with its own label for the source.
    let source = concat!(
        "---\n",
        "name: x\n",
        "description: d\n",
        "capabilities:\n",
        "  - not a capability id\n",
        "---\n",
        "\n# T\n\n## S\n\np\n",
    );
    let parse = Prompt::parse(source, "test")
        .0
        .expect_err("a capability id with spaces must be rejected");
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
    // Step 6: a post-frontmatter parse failure reports the prompt's
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
    // Step 6: an internal invariant failure locates itself in the Rust
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
    // Step 10: the refusal notice is the whole Display - it may arrive
    // as tool output when the prompt runs as a sub-run tool - and the
    // kind classifies it for code. Retrying cannot help: the
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
