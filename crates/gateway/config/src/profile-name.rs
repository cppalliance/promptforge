//! The [`ProfileName`] identifier type and its parse error.

use std::path::Path;

/// A validated profile name safe for URLs, state files, and operator labels.
///
/// Rejects surrounding whitespace, path separators, `.`, `..`, and the empty
/// string so one spelling remains safe in every profile-selection surface.
///
/// # Examples
/// ```
/// use gateway_config::ProfileName;
///
/// assert!(ProfileName::parse("dev").is_ok());
/// assert!(ProfileName::parse("../secrets").is_err());
/// assert!(ProfileName::parse("a/b").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProfileName(String);

impl ProfileName {
    /// Parses a profile name as one normal path component.
    ///
    /// # Errors
    /// Returns [`ProfileNameError`] when `name` has surrounding whitespace, is
    /// empty, is `.` or `..`, contains a path separator or NUL, or is not
    /// exactly one normal path component.
    pub fn parse(name: &str) -> Result<ProfileName, ProfileNameError> {
        if name != name.trim() {
            return Err(ProfileNameError::new(
                "profile name must not have surrounding whitespace",
            ));
        }
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
    pub fn as_str(&self) -> &str {
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
    }

    #[test]
    fn rejects_traversal_and_separators() {
        for bad in [
            "", " prod", "prod ", ".", "..", "a/b", "a\\b", "../x", "x\0y",
        ] {
            assert!(ProfileName::parse(bad).is_err(), "should reject {bad:?}");
        }
    }
}
