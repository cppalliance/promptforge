//! Workspace error tests: wire-code mapping, the JSON envelope, and debug-only source chains.

use super::*;

/// Collects a response body already buffered in memory.
pub(super) async fn body_bytes(response: Response) -> axum::body::Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is in memory already")
}

/// A distinctive injected cause for leak-boundary assertions.
fn injected_io() -> io::Error {
    io::Error::other("injected disk failure")
}

/// Every workspace failure keeps the status, code, and message it
/// answered with before the crate split.
#[test]
fn workspace_failures_keep_their_wire_mapping() {
    let cases: Vec<(WorkspaceError, StatusCode, &str)> = vec![
        (
            WorkspaceError::ResolveGrant {
                source: injected_io(),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            "resolve_grant",
        ),
        (
            WorkspaceError::ResolvePath {
                source: injected_io(),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            "resolve_path",
        ),
        (
            WorkspaceError::InspectPath {
                source: injected_io(),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            "inspect_path",
        ),
        (
            WorkspaceError::ListDirectory {
                source: injected_io(),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_directory",
        ),
        (
            WorkspaceError::ReadFile {
                source: injected_io(),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            "read_file",
        ),
        (
            WorkspaceError::WriteFile {
                source: injected_io(),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_file",
        ),
        (
            WorkspaceError::OutsideGrants,
            StatusCode::FORBIDDEN,
            "outside_grants",
        ),
        (
            WorkspaceError::ForbiddenComponent,
            StatusCode::FORBIDDEN,
            "forbidden_component",
        ),
        (WorkspaceError::NotFound, StatusCode::NOT_FOUND, "not_found"),
        (
            WorkspaceError::NotGranted,
            StatusCode::NOT_FOUND,
            "not_granted",
        ),
        (
            WorkspaceError::NotADirectory,
            StatusCode::BAD_REQUEST,
            "not_a_directory",
        ),
        (
            WorkspaceError::NotAFile,
            StatusCode::BAD_REQUEST,
            "not_a_file",
        ),
        (
            WorkspaceError::BinaryFile,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "binary_file",
        ),
        (
            WorkspaceError::NotUtf8 {
                source: String::from_utf8(vec![0xff]).unwrap_err(),
            },
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "not_utf8",
        ),
        (
            WorkspaceError::FileTooLarge { limit: 7 },
            StatusCode::PAYLOAD_TOO_LARGE,
            "file_too_large",
        ),
        (
            WorkspaceError::ModifiedConflict,
            StatusCode::CONFLICT,
            "modified_conflict",
        ),
    ];
    for (error, status, code) in cases {
        assert_eq!(error.status(), status, "status for {code}");
        assert_eq!(error.code(), code, "code for {code}");
    }
}

/// A refused ui-state put is the client's mistake: a foreign key or
/// non-JSON text is a bad request, an oversized value is too large, and
/// each message names what was required.
#[test]
fn ui_state_refusals_map_to_client_errors() {
    let key = WorkspaceError::UiStateKey("window".to_string());
    assert_eq!(key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(key.code(), "ui_state_key");
    let message = render_message(&key, false);
    assert_eq!(
        message,
        "ui-state key \"window\" is not allowed; one of layout, tree, closed_editors is required"
    );
    for allowed in crate::workspace_file::UI_STATE_KEYS {
        assert!(
            message.contains(allowed),
            "the message names every allow-listed key; missing {allowed}"
        );
    }

    let large = WorkspaceError::UiStateTooLarge { actual: 9, cap: 8 };
    assert_eq!(large.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(large.code(), "ui_state_too_large");
    assert_eq!(
        render_message(&large, false),
        "ui-state value is 9 bytes; at most 8 bytes are allowed"
    );

    let refusal = serde_json::from_str::<serde_json::Value>("{not json").unwrap_err();
    let not_json = WorkspaceError::UiStateNotJson {
        source: refusal.into(),
    };
    assert_eq!(not_json.status(), StatusCode::BAD_REQUEST);
    assert_eq!(not_json.code(), "ui_state_not_json");
    assert_eq!(
        render_message(&not_json, false),
        "ui-state value is not JSON"
    );
    assert!(
        render_message(&not_json, true).starts_with("ui-state value is not JSON: "),
        "the leaked chain names the parse refusal"
    );
}

/// The file-backed failures map to their own statuses and codes: a
/// refused file is the client's mistake, a taken path is a conflict,
/// and everything else is the server's problem.
#[test]
fn workspace_file_failures_map_to_their_wire_codes() {
    let refused = WorkspaceError::from(WorkspaceFileError::UnsupportedVersion {
        found: "2".to_string(),
        supported: "1",
    });
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(refused.code(), "workspace_file_refused");
    assert_eq!(
        render_message(&refused, false),
        "workspace file version 2 is unsupported; this build supports version 1",
        "the required-versus-actual text reaches production bodies"
    );

    let alien = WorkspaceError::from(WorkspaceFileError::NotAWorkspace {
        path: std::path::PathBuf::from("alien.pfwork"),
    });
    assert_eq!(alien.status(), StatusCode::BAD_REQUEST);
    assert_eq!(alien.code(), "workspace_file_refused");

    let taken = WorkspaceError::from(WorkspaceFileError::Io {
        source: io::Error::new(io::ErrorKind::AlreadyExists, "taken"),
    });
    assert_eq!(taken.status(), StatusCode::CONFLICT);
    assert_eq!(taken.code(), "workspace_file_taken");

    let missing = WorkspaceError::from(WorkspaceFileError::Io {
        source: io::Error::new(io::ErrorKind::NotFound, "missing"),
    });
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing.code(), "not_found");

    let closed = WorkspaceError::from(WorkspaceFileError::Closed);
    assert_eq!(closed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(closed.code(), "workspace_file_failed");
    assert_eq!(
        render_message(&closed, true),
        "workspace file operation failed: workspace file is closed",
        "debug bodies still chain the file failure"
    );
}

#[tokio::test]
async fn the_json_envelope_has_message_code_and_content_type() {
    let response = WorkspaceError::OutsideGrants.into_response();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("the envelope sets content-type");
    assert_eq!(content_type, "application/json");
    let body = body_bytes(response).await;
    let json: serde_json::Value = serde_json::from_slice(&body).expect("the envelope is JSON");
    assert_eq!(
        json["error"]["message"],
        "path is outside every granted root"
    );
    assert_eq!(json["error"]["code"], "outside_grants");
}

#[test]
fn production_messages_stay_at_the_variant_text() {
    let read = WorkspaceError::ReadFile {
        source: injected_io(),
    };
    assert_eq!(
        render_message(&read, false),
        "file cannot be read",
        "production bodies omit the source detail"
    );
}

#[test]
fn debug_messages_append_the_source_chain() {
    let read = WorkspaceError::ReadFile {
        source: injected_io(),
    };
    assert_eq!(
        render_message(&read, true),
        "file cannot be read: injected disk failure"
    );
}

/// Tests run under debug assertions, so the live envelope must include
/// the detail the debug side of the boundary promises.
#[cfg(debug_assertions)]
#[tokio::test]
async fn debug_builds_leak_detail_into_the_live_envelope() {
    let response = WorkspaceError::ReadFile {
        source: injected_io(),
    }
    .into_response();
    let body = body_bytes(response).await;
    let json: serde_json::Value = serde_json::from_slice(&body).expect("the envelope is JSON");
    assert_eq!(
        json["error"]["message"],
        "file cannot be read: injected disk failure"
    );
}

#[test]
fn the_ui_state_not_json_variant_reaches_the_serde_error_through_the_shared_wrapper() {
    let Err(json) = serde_json::from_str::<serde_json::Value>("nope") else {
        panic!("`nope` must not parse as JSON");
    };
    let error = WorkspaceError::UiStateNotJson {
        source: json.into(),
    };
    let Some(cause) = std::error::Error::source(&error) else {
        panic!("the ui-state not-json variant reports its serde cause as source()");
    };
    let Some(wrapper) = cause.downcast_ref::<JsonSource>() else {
        panic!("the serde cause is the shared JsonSource");
    };
    assert!(wrapper.as_inner().is_syntax());
}
