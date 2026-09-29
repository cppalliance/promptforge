//! The field shape a store failure's error value carries: its `reason`
//! tag and the [`VfsError`](promptforge_vfs::VfsError) variant's fields.

use super::ErrorField;

/// The `reason` tag a store error value carries for `error`: the tag
/// author code branches on to tell the failure modes apart.
#[must_use]
pub fn store_error_reason(error: &promptforge_vfs::VfsError) -> &'static str {
    match error {
        promptforge_vfs::VfsError::NotFound { .. } => "not_found",
        promptforge_vfs::VfsError::AlreadyExists { .. } => "already_exists",
        promptforge_vfs::VfsError::NotADirectory { .. } => "not_a_directory",
        promptforge_vfs::VfsError::IsADirectory { .. } => "is_a_directory",
        promptforge_vfs::VfsError::DirectoryNotEmpty { .. } => "directory_not_empty",
        promptforge_vfs::VfsError::NotUtf8 { .. } => "not_utf8",
        promptforge_vfs::VfsError::InvalidPath { .. } => "invalid_path",
        promptforge_vfs::VfsError::InvalidRange { .. } => "invalid_range",
        promptforge_vfs::VfsError::Anchor { .. } => "anchor",
        promptforge_vfs::VfsError::PermissionDenied { .. } => "permission_denied",
        promptforge_vfs::VfsError::Unsupported { .. } => "unsupported",
        promptforge_vfs::VfsError::Conflict { .. } => "conflict",
        // `VfsError` is `#[non_exhaustive]`: every failure without its
        // own reason, the backend's and any future variant's, is the
        // backend bucket.
        _ => "backend",
    }
}

/// The variant's own fields a store error value carries beside `kind`,
/// `message`, and `reason`: the paths the failure names, plus `anchor`
/// and its integer `count` for an anchor error, or `rule` for an invalid
/// path. The rule is the [`PathReason`](promptforge_vfs::PathReason) tag,
/// which `PathReason::from_tag` parses back.
#[must_use]
pub fn store_error_fields(error: &promptforge_vfs::VfsError) -> Vec<(String, ErrorField)> {
    use promptforge_vfs::VfsError;
    match error {
        VfsError::NotFound { path }
        | VfsError::AlreadyExists { path }
        | VfsError::NotADirectory { path }
        | VfsError::IsADirectory { path }
        | VfsError::DirectoryNotEmpty { path }
        | VfsError::NotUtf8 { path }
        | VfsError::InvalidRange { path, .. }
        | VfsError::PermissionDenied { path, .. }
        | VfsError::Unsupported { path, .. }
        | VfsError::Conflict { path, .. } => {
            vec![("path".to_owned(), ErrorField::String(path.clone()))]
        }
        VfsError::InvalidPath { path, reason } => vec![
            ("path".to_owned(), ErrorField::String(path.clone())),
            (
                "rule".to_owned(),
                ErrorField::String(reason.tag().to_owned()),
            ),
        ],
        VfsError::Anchor {
            path,
            anchor,
            count,
        } => vec![
            ("path".to_owned(), ErrorField::String(path.clone())),
            ("anchor".to_owned(), ErrorField::String(anchor.clone())),
            (
                "count".to_owned(),
                ErrorField::Integer(i64::try_from(*count).unwrap_or(i64::MAX)),
            ),
        ],
        // `VfsError` is `#[non_exhaustive]`: a variant without named
        // fields - the backend's and any future variant's - reports none.
        _ => Vec::new(),
    }
}

/// The full field set a store error value carries: the `reason` tag plus
/// the variant's own fields from [`store_error_fields`].
///
/// One source for the shape, so a failure the direct closures raise
/// during a shared library load and one answered through the executor's
/// effect path are indistinguishable to author code.
#[must_use]
pub fn store_error_value_fields(error: &promptforge_vfs::VfsError) -> Vec<(String, ErrorField)> {
    let mut fields = vec![(
        "reason".to_owned(),
        ErrorField::String(store_error_reason(error).to_owned()),
    )];
    fields.extend(store_error_fields(error));
    fields
}
