//! [`BodyText`]: a stretch of a prompt's text that sections and blocks read
//! in place. The parser makes one shared copy of the prompt body, and every
//! section's Markdown, list item, and prose block points into it.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

/// A stretch of a prompt's text, read in place from the one shared copy.
#[derive(Clone)]
pub struct BodyText {
    text: Arc<str>,
    range: Range<usize>,
}

impl BodyText {
    /// The text this stretch covers.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text[self.range.clone()]
    }

    /// `part` located inside `text` by its byte offset; `None` when `part` is
    /// not a subslice of `text`.
    #[must_use]
    pub(crate) fn within(text: &Arc<str>, part: &str) -> Option<BodyText> {
        let start = part.as_ptr().addr().checked_sub(text.as_ptr().addr())?;
        let end = start.checked_add(part.len())?;
        (end <= text.len()).then(|| BodyText {
            text: Arc::clone(text),
            range: start..end,
        })
    }

    /// A whole owned string, for synthetic sections in test support.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub(crate) fn from_owned(text: String) -> BodyText {
        let range = 0..text.len();
        BodyText {
            text: Arc::from(text),
            range,
        }
    }
}

impl PartialEq for BodyText {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for BodyText {}

impl PartialEq<str> for BodyText {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for BodyText {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl AsRef<str> for BodyText {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for BodyText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <str as fmt::Debug>::fmt(self.as_str(), f)
    }
}

#[cfg(test)]
#[path = "body_text-tests.rs"]
mod tests;
