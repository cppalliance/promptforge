//! A sequence that holds at least one element by construction.

/// A vector holding at least one element.
///
/// The parse builds one only from a non-empty input, so a consumer never
/// meets the empty case: the awaited task set and a message's content
/// parts are both non-empty. It reads as a slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmpty<T>(Vec<T>);

impl<T> NonEmpty<T> {
    /// The sequence of `first` followed by `rest`.
    #[must_use]
    pub fn new(first: T, rest: Vec<T>) -> Self {
        let mut items = Vec::with_capacity(rest.len() + 1);
        items.push(first);
        items.extend(rest);
        Self(items)
    }

    /// `items` as a non-empty sequence, or `None` when it holds nothing.
    #[must_use]
    pub fn from_vec(items: Vec<T>) -> Option<Self> {
        if items.is_empty() {
            None
        } else {
            Some(Self(items))
        }
    }

    /// The elements, in order.
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        self.0
    }

    /// The sequence of `items` followed by this one's elements.
    #[must_use]
    pub fn preceded_by(mut self, mut items: Vec<T>) -> Self {
        items.append(&mut self.0);
        Self(items)
    }
}

impl<T> Extend<T> for NonEmpty<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, items: I) {
        self.0.extend(items);
    }
}

impl<T> std::ops::Deref for NonEmpty<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> IntoIterator for NonEmpty<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a NonEmpty<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
