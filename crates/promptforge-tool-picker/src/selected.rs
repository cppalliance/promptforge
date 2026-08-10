//! Selected-scope near-duplicate analysis over a picker's stored vectors.
//!
//! Only the picker owns the vectors needed to answer whether two selected tools
//! are near-verbatim copies, so this analysis lives beside it. Results borrow
//! the picker's descriptors, so a dense selected set never deep-clones a schema.

use std::collections::HashSet;

use crate::catalog::{ToolDescriptor, ToolId};
use crate::error::SelectionError;
use crate::rank::Vectors;

/// A selected pair whose stored tool vectors meet the duplicate threshold.
///
/// Has no public constructor and always refers to two distinct catalog entries,
/// in catalog order, whose stored-vector similarity met the configured
/// inclusive threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct NearDuplicate<'a> {
    /// The earlier tool in catalog order.
    first: &'a ToolDescriptor,
    /// The later tool in catalog order.
    second: &'a ToolDescriptor,
    /// The cosine similarity between the tools' stored vectors.
    similarity: f32,
}

impl<'a> NearDuplicate<'a> {
    /// Builds a pair; the caller guarantees two distinct catalog entries.
    fn new(first: &'a ToolDescriptor, second: &'a ToolDescriptor, similarity: f32) -> Self {
        Self {
            first,
            second,
            similarity,
        }
    }

    /// Returns the earlier tool in catalog order.
    #[must_use]
    pub fn first(&self) -> &'a ToolDescriptor {
        self.first
    }

    /// Returns the later tool in catalog order.
    #[must_use]
    pub fn second(&self) -> &'a ToolDescriptor {
        self.second
    }

    /// Returns the cosine similarity between the tools' stored vectors.
    #[must_use]
    pub fn similarity(&self) -> f32 {
        self.similarity
    }
}

/// The near-duplicate pairs among a selected scope, in catalog pair order.
#[derive(Debug, Clone, PartialEq)]
pub struct NearDuplicates<'a> {
    /// The qualifying pairs, in catalog pair order.
    pairs: Vec<NearDuplicate<'a>>,
}

impl<'a> NearDuplicates<'a> {
    /// The number of qualifying pairs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    /// Whether no pair qualified.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// The pair at `index`, if any.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<NearDuplicate<'a>> {
        self.pairs.get(index).copied()
    }

    /// Iterates the qualifying pairs, in catalog pair order.
    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter(&self) -> NearDuplicateIter<'a, '_> {
        NearDuplicateIter {
            inner: self.pairs.iter(),
        }
    }
}

impl<'a, 'pairs> IntoIterator for &'pairs NearDuplicates<'a> {
    type Item = NearDuplicate<'a>;
    type IntoIter = NearDuplicateIter<'a, 'pairs>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A borrowing iterator over the pairs of a [`NearDuplicates`].
#[derive(Debug, Clone)]
pub struct NearDuplicateIter<'a, 'pairs> {
    /// The backing slice iterator over the pairs.
    inner: std::slice::Iter<'pairs, NearDuplicate<'a>>,
}

impl<'a> Iterator for NearDuplicateIter<'a, '_> {
    type Item = NearDuplicate<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().copied()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl DoubleEndedIterator for NearDuplicateIter<'_, '_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back().copied()
    }
}

impl ExactSizeIterator for NearDuplicateIter<'_, '_> {}

impl std::iter::FusedIterator for NearDuplicateIter<'_, '_> {}

/// Finds selected pairs meeting `threshold`, preserving catalog pair order.
///
/// Every requested identity is validated against one hash set before any pair
/// is compared; the first missing identity is reported. Repeated identities are
/// idempotent set membership.
pub(crate) fn near_duplicates<'a>(
    tools: &'a [ToolDescriptor],
    vectors: Vectors<'_>,
    threshold: f32,
    ids: &[ToolId],
) -> Result<NearDuplicates<'a>, SelectionError> {
    let catalog: HashSet<&ToolId> = tools.iter().map(ToolDescriptor::id).collect();
    let mut requested: HashSet<&ToolId> = HashSet::with_capacity(ids.len());
    for id in ids {
        if !catalog.contains(id) {
            return Err(SelectionError::new(id.clone()));
        }
        requested.insert(id);
    }

    let mut pairs = Vec::new();
    for (first_index, first) in tools.iter().enumerate() {
        if !requested.contains(first.id()) {
            continue;
        }
        for (second_index, second) in tools.iter().enumerate().skip(first_index + 1) {
            if !requested.contains(second.id()) {
                continue;
            }
            if let Some(similarity) = vectors.similarity(first_index, second_index)
                && similarity >= threshold
            {
                pairs.push(NearDuplicate::new(first, second, similarity));
            }
        }
    }
    Ok(NearDuplicates { pairs })
}

#[cfg(test)]
mod tests {
    use super::near_duplicates;
    use crate::catalog::{ToolDescriptor, ToolId};
    use crate::rank::Vectors;
    use serde_json::json;

    fn tool(server: &str, name: &str) -> ToolDescriptor {
        ToolDescriptor::new(ToolId::new(server, name), "does a thing", json!({}))
    }

    #[test]
    fn an_absent_id_rejects_the_whole_selected_set_naming_the_first_missing() {
        let tools = vec![tool("files", "read"), tool("files", "write")];
        let missing = ToolId::new("missing", "tool");
        let ids = vec![
            tools[0].id().clone(),
            missing.clone(),
            ToolId::new("also", "missing"),
        ];
        let error = near_duplicates(&tools, Vectors::new(&[1.0, 0.0, 1.0, 0.0], 2), 0.9, &ids)
            .expect_err("an absent identity rejects the set");
        assert_eq!(error.missing_id(), &missing);
    }

    #[test]
    fn repeated_ids_are_idempotent_and_cross_server_pairs_follow_catalog_order() {
        let tools = vec![
            tool("first", "read"),
            tool("second", "read"),
            tool("third", "read"),
        ];
        let vectors = [1.0, 0.0, 1.0, 0.0, 1.0, 0.0];
        let ids = vec![
            tools[2].id().clone(),
            tools[0].id().clone(),
            tools[1].id().clone(),
            tools[2].id().clone(),
        ];
        let pairs =
            near_duplicates(&tools, Vectors::new(&vectors, 2), 1.0, &ids).expect("all present");
        assert_eq!(pairs.len(), 3);
        assert_eq!(
            pairs
                .iter()
                .map(|pair| (pair.first().id().clone(), pair.second().id().clone()))
                .collect::<Vec<_>>(),
            vec![
                (tools[0].id().clone(), tools[1].id().clone()),
                (tools[0].id().clone(), tools[2].id().clone()),
                (tools[1].id().clone(), tools[2].id().clone()),
            ]
        );
    }

    #[test]
    fn a_pair_must_reach_the_threshold_inclusively() {
        let tools = vec![tool("files", "read"), tool("blobs", "read")];
        let ids = [tools[0].id().clone(), tools[1].id().clone()];
        let threshold = 0.75_f32;
        let at = [1.0, 0.0, threshold, (1.0 - threshold * threshold).sqrt()];
        let pairs = near_duplicates(&tools, Vectors::new(&at, 2), threshold, &ids).expect("ok");
        assert_eq!(pairs.len(), 1);
        assert!((pairs.get(0).unwrap().similarity() - threshold).abs() < 1e-6);

        let below = f32::from_bits(threshold.to_bits() - 1);
        let below_rows = [1.0, 0.0, below, (1.0 - below * below).sqrt()];
        assert!(
            near_duplicates(&tools, Vectors::new(&below_rows, 2), threshold, &ids)
                .expect("ok")
                .is_empty()
        );
    }
}
