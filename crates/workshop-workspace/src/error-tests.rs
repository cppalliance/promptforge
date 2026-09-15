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
            WorkspaceError::NotUtf8,
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

#[tokio::test]
async fn the_json_envelope_carries_message_code_and_content_type() {
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
        "production bodies carry no source detail"
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

/// Tests run under debug assertions, so the live envelope must carry
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
