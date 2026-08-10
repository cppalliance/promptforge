//! Scoring every indexed tool against a need and keeping the best few.
//!
//! This module ranks and draws no conclusion. It also owns the validated vector
//! layout: an [`Index`] cannot be constructed with a malformed row buffer, so
//! query and policy code never has to recover from an impossible internal shape.
//!
//! Every stored row and every query vector reaching this module is already
//! L2-normalized, so cosine similarity is the plain dot product. Ordering is a
//! total order - score descending, then catalog position - so a ranking is
//! reproducible.

use std::cmp::Ordering;

/// One scored tool: where it sits in the catalog and how well it matched.
///
/// A candidate is a finding, not a decision, and it is produced only by ranking
/// over a picker's own [`Index`], so its position always addresses a real row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Candidate {
    /// The tool's position in the catalog the index was built from.
    index: usize,
    /// The cosine similarity between the query and this tool's vector.
    score: f32,
}

impl Candidate {
    /// Builds a candidate at `index` scoring `score`.
    pub(crate) fn new(index: usize, score: f32) -> Self {
        Self { index, score }
    }

    /// Returns the catalog position this candidate names.
    pub(crate) fn index(self) -> usize {
        self.index
    }

    /// Returns the cosine similarity, reported exactly as computed.
    pub(crate) fn score(self) -> f32 {
        self.score
    }
}

/// The stored embedding rows, addressed by catalog position.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Vectors<'a> {
    /// The rows, end to end.
    rows: &'a [f32],
    /// How many floats one row occupies.
    stride: usize,
}

impl<'a> Vectors<'a> {
    /// A view of `rows` read as records `stride` floats wide.
    #[must_use]
    pub(crate) fn new(rows: &'a [f32], stride: usize) -> Self {
        Self { rows, stride }
    }

    /// The row for the tool at catalog position `index`, if wholly present.
    #[must_use]
    pub(crate) fn row(self, index: usize) -> Option<&'a [f32]> {
        if self.stride == 0 {
            return None;
        }
        let start = index.checked_mul(self.stride)?;
        let end = start.checked_add(self.stride)?;
        self.rows.get(start..end)
    }

    /// The cosine similarity between two stored rows, if both are present.
    #[must_use]
    pub(crate) fn similarity(self, a: usize, b: usize) -> Option<f32> {
        Some(dot(self.row(a)?, self.row(b)?))
    }
}

/// A validated flat vector index: exactly one row per catalog entry.
///
/// Construction rejects a zero stride, a row buffer whose length is not the
/// catalog count times the stride, and any partial row. A malformed crate-owned
/// layout is a bug that fails here rather than degrading into a missing row.
#[derive(Debug)]
pub(crate) struct Index {
    /// Every tool's vector, end to end, `stride` floats apart.
    rows: Vec<f32>,
    /// How many floats one row occupies.
    stride: usize,
}

impl Index {
    /// Builds a validated index from a flat `rows` buffer of `count` rows.
    ///
    /// # Errors
    /// Returns a description of the broken invariant when `stride` is zero or
    /// `rows` is not exactly `stride * count` long.
    pub(crate) fn new(rows: Vec<f32>, stride: usize, count: usize) -> Result<Self, String> {
        if stride == 0 {
            return Err("the model reported a zero embedding dimension".to_owned());
        }
        let expected = stride
            .checked_mul(count)
            .ok_or_else(|| "the vector buffer length overflowed".to_owned())?;
        if rows.len() != expected {
            return Err(format!(
                "the vector buffer holds {} floats, expected {expected} for {count} rows of {stride}",
                rows.len()
            ));
        }
        Ok(Self { rows, stride })
    }

    /// A borrowed view of the stored rows.
    #[must_use]
    pub(crate) fn vectors(&self) -> Vectors<'_> {
        Vectors::new(&self.rows, self.stride)
    }

    /// The best `k` rows for `query`, best first, under the total order.
    #[must_use]
    pub(crate) fn top_k(&self, query: &[f32], k: usize) -> Vec<Candidate> {
        top_k(query, &self.rows, k)
    }
}

/// The best `k` rows of `vectors` for `query`, in descending score order.
#[must_use]
fn top_k(query: &[f32], vectors: &[f32], k: usize) -> Vec<Candidate> {
    if query.is_empty() || k == 0 {
        return Vec::new();
    }

    let mut candidates: Vec<Candidate> = vectors
        .chunks_exact(query.len())
        .enumerate()
        .map(|(index, row)| Candidate::new(index, dot(query, row)))
        .collect();

    candidates.sort_unstable_by(by_score_then_position);
    candidates.truncate(k);
    candidates
}

/// The dot product of two equally long vectors.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The total order [`Index::top_k`] sorts by: score descending, position ascending.
fn by_score_then_position(a: &Candidate, b: &Candidate) -> Ordering {
    comparable(b.score)
        .total_cmp(&comparable(a.score))
        .then(a.index.cmp(&b.index))
}

/// A score as it is ordered: anything not finite ranks below everything else.
#[must_use]
pub(crate) fn comparable(score: f32) -> f32 {
    if score.is_finite() {
        score
    } else {
        f32::NEG_INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::{Candidate, Index, Vectors};

    fn positions(candidates: &[Candidate]) -> Vec<usize> {
        candidates
            .iter()
            .map(|candidate| candidate.index())
            .collect()
    }

    const ROWS: [f32; 8] = [
        0.0, 1.0, // 0: orthogonal to the query below
        1.0, 0.0, // 1: identical to it
        -1.0, 0.0, // 2: opposed to it
        0.6, 0.8, // 3: 0.6 against it
    ];

    fn index() -> Index {
        Index::new(ROWS.to_vec(), 2, 4).expect("a well-formed layout is accepted")
    }

    #[test]
    fn candidates_come_back_in_descending_score_order() {
        let ranked = index().top_k(&[1.0, 0.0], 4);
        assert_eq!(positions(&ranked), vec![1, 3, 0, 2]);
        for pair in ranked.windows(2) {
            assert!(pair[0].score() >= pair[1].score());
        }
    }

    #[test]
    fn a_k_larger_than_the_index_returns_the_index_unpadded() {
        let ranked = index().top_k(&[1.0, 0.0], 99);
        assert_eq!(ranked.len(), 4);
    }

    #[test]
    fn a_k_smaller_than_the_index_truncates_the_same_ranking() {
        let index = index();
        let full = index.top_k(&[1.0, 0.0], 4);
        for k in 0..=4 {
            assert_eq!(index.top_k(&[1.0, 0.0], k).as_slice(), &full[..k]);
        }
    }

    #[test]
    fn an_empty_query_yields_no_candidates() {
        assert!(index().top_k(&[], 3).is_empty());
    }

    #[test]
    fn exactly_tied_scores_are_ordered_by_catalog_position() {
        let query = [0.6_f32, 0.8];
        let tied: Vec<f32> = query.iter().copied().cycle().take(8).collect();
        let index = Index::new(tied, 2, 4).expect("layout");
        let ranked = index.top_k(&query, 4);
        assert_eq!(positions(&ranked), vec![0, 1, 2, 3]);
        for _ in 0..8 {
            assert_eq!(index.top_k(&query, 4), ranked);
        }
    }

    #[test]
    fn a_non_finite_score_never_outranks_a_real_one() {
        let rows = vec![f32::NAN, 0.0, 0.5, 0.0, f32::INFINITY, 0.0, 1.0, 0.0];
        let ranked = Index::new(rows, 2, 4)
            .expect("layout")
            .top_k(&[1.0, 0.0], 4);
        assert_eq!(positions(&ranked), vec![3, 1, 0, 2]);
    }

    #[test]
    fn similarities_read_two_stored_rows() {
        let vectors = Vectors::new(&ROWS, 2);
        assert_eq!(vectors.similarity(1, 1), Some(1.0));
        assert_eq!(vectors.similarity(1, 2), Some(-1.0));
        assert_eq!(vectors.similarity(0, 1), Some(0.0));
        assert_eq!(vectors.similarity(1, 3), Some(0.6));
        assert_eq!(vectors.similarity(0, 9), None);
    }

    #[test]
    fn construction_rejects_malformed_layouts() {
        assert!(Index::new(vec![1.0, 0.0], 0, 1).is_err(), "zero stride");
        assert!(
            Index::new(vec![1.0, 0.0, 0.5], 2, 2).is_err(),
            "partial row"
        );
        assert!(Index::new(vec![1.0, 0.0], 2, 2).is_err(), "count mismatch");
        assert!(
            Index::new(Vec::new(), 2, 0).is_ok(),
            "an empty index is valid"
        );
    }
}
