//! The field shape a store failure's error value carries: its `reason`
//! tag and the [`VfsError`](runvil_vfs::VfsError) variant's fields.

use super::ErrorField;

/// Returns the variant's own fields a store error value carries beside
/// `kind`, `message`, and `reason`: the paths the failure names, plus
/// `anchor` and its integer `count` for an anchor error, `rule` for an
/// invalid path, or the integer `valid_up_to` for text that is not UTF-8.
/// The rule is the [`PathReason`](runvil_vfs::PathReason) tag, which
/// `PathReason::from_tag` parses back.
fn store_error_fields(error: &runvil_vfs::VfsError) -> Vec<(String, ErrorField)> {
    use runvil_vfs::VfsError;
    let integer = |value: usize| ErrorField::Integer(i64::try_from(value).unwrap_or(i64::MAX));
    match error {
        VfsError::NotFound { path }
        | VfsError::AlreadyExists { path }
        | VfsError::NotADirectory { path }
        | VfsError::IsADirectory { path }
        | VfsError::DirectoryNotEmpty { path }
        | VfsError::InvalidRange { path, .. }
        | VfsError::PermissionDenied { path, .. }
        | VfsError::ApprovalRequired { path, .. }
        | VfsError::ScopeEnded { path }
        | VfsError::ReadOnly { path }
        | VfsError::OutsideRoot { path, .. }
        | VfsError::RootProtected { path }
        | VfsError::CrossMount { path, .. }
        | VfsError::Io { path, .. }
        | VfsError::Unsupported { path, .. }
        | VfsError::Conflict { path, .. } => {
            vec![("path".to_owned(), ErrorField::String(path.clone()))]
        }
        VfsError::NotUtf8 { path, valid_up_to } => vec![
            ("path".to_owned(), ErrorField::String(path.clone())),
            ("valid_up_to".to_owned(), integer(*valid_up_to)),
        ],
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
            ("count".to_owned(), integer(*count)),
        ],
        // `NoStore` and `Backend` name no path, and `VfsError` is
        // `#[non_exhaustive]`: a variant added later reports no fields
        // until it is named here.
        _ => Vec::new(),
    }
}

/// Returns the full field set a store error value carries: the `reason`
/// tag, as [`VfsError::tag`](runvil_vfs::VfsError::tag) names it,
/// first, then the variant's own fields: the paths the failure names, plus
/// the anchor, count, rule, or UTF-8 offset its variant holds.
///
/// One source for the shape, so a failure the direct closures raise
/// while a Plugin prelude installs and one answered through the
/// executor's effect path are indistinguishable to Lua code.
#[must_use]
pub fn store_error_value_fields(error: &runvil_vfs::VfsError) -> Vec<(String, ErrorField)> {
    let mut fields = vec![(
        "reason".to_owned(),
        ErrorField::String(error.tag().to_owned()),
    )];
    fields.extend(store_error_fields(error));
    fields
}
