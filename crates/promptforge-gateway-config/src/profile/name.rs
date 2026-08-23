//! The [`ProfileName`] confinement type and its parse error.

use std::path::Path;

/// A validated profile name: exactly one normal path component with a non-empty
/// UTF-8 stem.
///
/// Rejects path separators, `.`, `..`, and the empty string, so a profile
/// selection can never escape the configured profiles directory. This is the
/// profile-switch confinement type.
///
/// # Examples
/// ```
/// use promptforge_gateway_config::ProfileName;
///
/// assert!(ProfileName::parse("dev").is_ok());
/// assert!(ProfileName::parse("../secrets").is_err());
/// assert!(ProfileName::parse("a/b").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileName(String);

impl ProfileName {
    /// Parse a profile name, confining it to a single normal path component.
    ///
    /// # Errors
    /// Returns [`ProfileNameError`] when `name` is empty, is `.` or `..`,
    /// contains a path separator or NUL, or is not exactly one normal path
    /// component.
    pub fn parse(name: &str) -> Result<ProfileName, ProfileNameError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ProfileNameError::new("profile name must not be empty"));
        }
        if name == "." || name == ".." {
            return Err(ProfileNameError::new(
                "profile name must not be `.` or `..`",
            ));
        }
        if name.contains(['/', '\\']) || name.contains('\0') {
            return Err(ProfileNameError::new(
                "profile name must not contain path separators",
            ));
        }
        let mut components = Path::new(name).components();
        match (components.next(), components.next()) {
            (Some(std::path::Component::Normal(part)), None)
                if part == std::ffi::OsStr::new(name) => {}
            _ => {
                return Err(ProfileNameError::new(
                    "profile name must be a single path component",
                ));
            }
        }
        Ok(ProfileName(name.to_owned()))
    }

    /// The validated name as a string slice.
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProfileName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The reason a string was rejected as a [`ProfileName`].
#[non_exhaustive]
pub struct ProfileNameError {
    reason: &'static str,
}

impl ProfileNameError {
    fn new(reason: &'static str) -> ProfileNameError {
        ProfileNameError { reason }
    }
}

impl std::fmt::Debug for ProfileNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileNameError")
            .field("reason", &self.reason)
            .finish()
    }
}

impl std::fmt::Display for ProfileNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason)
    }
}

impl std::error::Error for ProfileNameError {}

#[cfg(test)]
mod tests {
    use super::ProfileName;

    #[test]
    fn accepts_simple_names() {
        assert_eq!(ProfileName::parse("dev").unwrap().as_str(), "dev");
        assert_eq!(ProfileName::parse("  prod  ").unwrap().as_str(), "prod");
    }

    #[test]
    fn rejects_traversal_and_separators() {
        for bad in ["", ".", "..", "a/b", "a\\b", "../x", "x\0y"] {
            assert!(ProfileName::parse(bad).is_err(), "should reject {bad:?}");
        }
    }
}
