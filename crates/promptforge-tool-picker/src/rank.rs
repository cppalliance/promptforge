//! Scoring every indexed tool against a need and keeping the best few.
//!
//! This module ranks and nothing else. It produces an ordered list of
//! [`Candidate`]s and draws no conclusion from it: whether the top score is
//! good enough, whether the first two are too close to separate, and whether
//! the pair straddles two servers are all questions decided elsewhere, from
//! the list this module hands over. Keeping the arithmetic apart from the
//! judgement means the thresholds can be re-tuned without touching the code
//! that computes a score, and a score can be tested without a policy to agree
//! with.
//!
//! # Cosine similarity is a dot product here
//!
//! Every stored row and every query vector reaching this module has already
//! been L2-normalized by [`Embedder::embed`](crate::embed::Embedder::embed).
//! The cosine of the angle between two unit vectors is their dot product, so
//! no magnitude is computed and nothing is divided. Scores therefore lie in
//! `-1.0..=1.0` up to floating-point rounding, which is the range the
//! configured thresholds are expressed in.
//!
//! # Ordering is total, so a ranking is reproducible
//!
//! Sorting on score alone is not enough to make a ranking reproducible.
//! Similarity scores are `f32`, two tools can score exactly equal, and a
//! comparison that calls such a pair equal leaves their relative order to the
//! sort implementation and to the order the elements happened to arrive in.
//! The comparison used here is a *total* order - score first, then position in
//! the catalog - and no two rows share a position, so there is exactly one
//! correct output for a given query and index. See [`top_k`] for the rule.
//!
//! # The stored rows outlive the ranking
//!
//! A ranking answers "how well did each tool match this need". Some questions
//! are about the tools themselves and not about any need - whether two tools
//! are near-verbatim copies of each other, for one - and those are answered
//! from the stored rows rather than from scores. [`Vectors`] is the view of
//! the flat buffer that lets a later stage ask them.

use std::cmp::Ordering;

/// One scored tool: where it sits in the catalog and how well it matched.
///
/// A candidate is a finding, not a decision. It says only that the tool at
/// `index` scored `score` against some query; what that is worth is for the
/// caller to say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Candidate {
    /// The tool's position in the catalog the index was built from.
    ///
    /// It indexes [`ToolPicker::tools`](crate::picker::ToolPicker::tools) and
    /// [`ToolPicker::vector`](crate::picker::ToolPicker::vector) alike, which
    /// is what lets a candidate be turned back into a descriptor without
    /// carrying one.
    pub(crate) index: usize,
    /// The cosine similarity between the query and this tool's vector.
    ///
    /// The value is reported exactly as it was computed, including a
    /// non-finite one. Ordering demotes such a score (see [`top_k`]) but does
    /// not rewrite it: a caller that receives a NaN should see the NaN rather
    /// than a plausible number standing in for it.
    pub(crate) score: f32,
}

/// The stored embedding rows, addressed by catalog position.
///
/// A borrowed view of the flat buffer an index keeps its vectors in: rows laid
/// end to end, `stride` floats apart, row `i` describing the tool at catalog
/// position `i`. It carries no vectors of its own, so passing one costs a
/// pointer and a length and never copies a row.
///
/// Every row an index stores is L2-normalized, so the dot product of two rows
/// is their cosine similarity with no magnitudes to divide out. That is what
/// [`Vectors::similarity`] returns.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Vectors<'a> {
    /// The rows, end to end.
    rows: &'a [f32],
    /// How many floats one row occupies.
    stride: usize,
}

impl<'a> Vectors<'a> {
    /// A view of `rows` read as records `stride` floats wide.
    pub(crate) fn new(rows: &'a [f32], stride: usize) -> Self {
        Self { rows, stride }
    }

    /// The row for the tool at catalog position `index`.
    ///
    /// `None` when the row is not wholly present - an index past the end, or a
    /// stride of zero, which describes no row at all.
    pub(crate) fn row(self, index: usize) -> Option<&'a [f32]> {
        if self.stride == 0 {
            return None;
        }
        let start = index.checked_mul(self.stride)?;
        let end = start.checked_add(self.stride)?;
        self.rows.get(start..end)
    }

    /// The cosine similarity between two stored rows.
    ///
    /// A property of the pair of tools alone: no query is involved, so the
    /// answer is the same whatever need is being resolved. `None` when either
    /// row is absent, which is how a caller holding a position the buffer does
    /// not cover learns so rather than being handed a number.
    pub(crate) fn similarity(self, a: usize, b: usize) -> Option<f32> {
        Some(dot(self.row(a)?, self.row(b)?))
    }
}

/// The best `k` rows of `vectors` for `query`, in descending score order.
///
/// `vectors` is a flat buffer of rows laid end to end, each row as long as
/// `query`; row `i` is scored against the query by their dot product, which is
/// their cosine similarity because both are unit length. The returned
/// candidates carry the row's position in that buffer, so they index the
/// catalog directly.
///
/// # The order
///
/// Descending by score. Two rows that score *exactly* equal are ordered by
/// ascending catalog position - the earlier tool in the catalog the caller
/// supplied comes first. The tie-break is the catalog position rather than the
/// tool's name or server because position is the one key guaranteed to be
/// unique across a catalog: identities may repeat, since a catalog is not
/// deduplicated and republishing one tool under two servers is exactly the
/// case this engine exists to report. A unique key makes the comparison a
/// total order, and a total order makes the ranking a function of its inputs -
/// the same catalog and the same query yield the same list, in the same order,
/// on every call and every run.
///
/// A non-finite score is ordered as though it were the worst possible score,
/// so it can never displace a real match, and ties among such scores fall
/// back to catalog position like any other tie. Nothing in this crate can
/// produce one - a vector that could not be normalized is rejected before it
/// is ever stored - but a comparison that returns "neither is greater" for a
/// NaN would silently forfeit the total order that every guarantee above
/// rests on.
///
/// # What comes back
///
/// At most `k` candidates, and fewer when the index holds fewer: a `k` larger
/// than the catalog returns the whole catalog ranked, never a padded list. A
/// `k` of zero, an empty index, and an empty query each return no candidates
/// at all. Zero is unreachable through the engine's own configuration, which
/// rejects a `top_k` of zero outright, but this function takes `k` as an
/// argument and answers the question asked rather than assuming it cannot be.
///
/// The truncation is under the order described above and no other. A later
/// stage may re-sort the candidates it receives on keys this module knows
/// nothing about, but it receives only these `k`: a row that such a re-sort
/// would have promoted is already discarded if it did not place in the top `k`
/// here. Asking for a larger `k` is the only way to keep it.
pub(crate) fn top_k(query: &[f32], vectors: &[f32], k: usize) -> Vec<Candidate> {
    if query.is_empty() || k == 0 {
        return Vec::new();
    }

    // A trailing remainder shorter than one row is skipped rather than scored
    // against a truncated query; a buffer built by this crate never has one.
    let mut candidates: Vec<Candidate> = vectors
        .chunks_exact(query.len())
        .enumerate()
        .map(|(index, row)| Candidate {
            index,
            score: dot(query, row),
        })
        .collect();

    candidates.sort_unstable_by(by_score_then_position);
    candidates.truncate(k);
    candidates
}

/// The dot product of two equally long vectors.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The total order [`top_k`] sorts by: score descending, position ascending.
///
/// [`f32::total_cmp`] rather than [`PartialOrd`] so the comparison is total
/// for every pair of bit patterns, and positions are compared only when the
/// scores are indistinguishable under it.
fn by_score_then_position(a: &Candidate, b: &Candidate) -> Ordering {
    comparable(b.score)
        .total_cmp(&comparable(a.score))
        .then(a.index.cmp(&b.index))
}

/// A score as it is ordered: anything not finite ranks below everything else.
///
/// Shared with the decision layer, which refines this module's order with a
/// further tie-break and must demote a non-finite score identically or the two
/// orders would disagree about which candidate leads.
pub(crate) fn comparable(score: f32) -> f32 {
    if score.is_finite() {
        score
    } else {
        f32::NEG_INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::{Candidate, Vectors, top_k};

    /// The positions of a ranking, which is what most assertions here are about.
    fn positions(candidates: &[Candidate]) -> Vec<usize> {
        candidates.iter().map(|candidate| candidate.index).collect()
    }

    /// Four two-dimensional unit vectors, laid out flat as the index lays them.
    ///
    /// Synthetic rather than embedded: two dimensions make every score exact
    /// and hand-checkable, and no model has to be loaded to run the test.
    const ROWS: [f32; 8] = [
        0.0, 1.0, // 0: orthogonal to the query below
        1.0, 0.0, // 1: identical to it
        -1.0, 0.0, // 2: opposed to it
        0.6, 0.8, // 3: 0.6 against it
    ];

    #[test]
    fn candidates_come_back_in_descending_score_order() {
        let ranked = top_k(&[1.0, 0.0], &ROWS, 4);
        assert_eq!(positions(&ranked), vec![1, 3, 0, 2]);
        for pair in ranked.windows(2) {
            assert!(
                pair[0].score >= pair[1].score,
                "scores {} then {} are not descending",
                pair[0].score,
                pair[1].score
            );
        }
    }

    #[test]
    fn scores_are_the_cosines_and_stay_inside_the_unit_range() {
        let ranked = top_k(&[1.0, 0.0], &ROWS, 4);
        let scores: Vec<f32> = ranked.iter().map(|candidate| candidate.score).collect();
        assert_eq!(scores, vec![1.0, 0.6, 0.0, -1.0]);
        for score in scores {
            assert!(
                (-1.0..=1.0).contains(&score),
                "{score} is outside the cosine range of unit vectors"
            );
        }
    }

    #[test]
    fn a_k_larger_than_the_index_returns_the_index_unpadded() {
        let ranked = top_k(&[1.0, 0.0], &ROWS, 99);
        assert_eq!(ranked.len(), 4);
        assert_eq!(positions(&ranked), vec![1, 3, 0, 2]);
    }

    #[test]
    fn a_k_smaller_than_the_index_truncates_the_same_ranking() {
        let full = top_k(&[1.0, 0.0], &ROWS, 4);
        for k in 0..=4 {
            let ranked = top_k(&[1.0, 0.0], &ROWS, k);
            assert_eq!(ranked.len(), k);
            assert_eq!(ranked.as_slice(), &full[..k]);
        }
    }

    #[test]
    fn an_empty_index_yields_no_candidates() {
        assert!(top_k(&[1.0, 0.0], &[], 3).is_empty());
    }

    #[test]
    fn an_empty_query_yields_no_candidates() {
        assert!(top_k(&[], &ROWS, 3).is_empty());
    }

    #[test]
    fn exactly_tied_scores_are_ordered_by_catalog_position() {
        // Four rows, all scoring exactly 1.0 against the query, so the order
        // is decided entirely by the tie-break. The scores are equal by
        // construction rather than by luck: every row is the query itself.
        let query = [0.6_f32, 0.8];
        let tied: Vec<f32> = query.iter().copied().cycle().take(8).collect();

        let ranked = top_k(&query, &tied, 4);
        assert_eq!(positions(&ranked), vec![0, 1, 2, 3]);
        let first = ranked[0].score;
        for candidate in &ranked {
            assert_eq!(
                candidate.score.to_bits(),
                first.to_bits(),
                "the rows must tie exactly for this to be testing the tie-break"
            );
        }

        // Repeating the call must not shuffle them, and neither must asking
        // for a prefix of the same ranking.
        for _ in 0..16 {
            assert_eq!(top_k(&query, &tied, 4), ranked);
            assert_eq!(top_k(&query, &tied, 2), ranked[..2].to_vec());
        }
    }

    #[test]
    fn a_tie_below_the_leader_is_broken_by_position_too() {
        // Row 1 wins outright; rows 0, 2 and 3 tie behind it at 0.0.
        let rows = [0.0, 1.0, 1.0, 0.0, 0.0, -1.0, 0.0, 1.0];
        let ranked = top_k(&[1.0, 0.0], &rows, 4);
        assert_eq!(positions(&ranked), vec![1, 0, 2, 3]);
    }

    #[test]
    fn a_non_finite_score_never_outranks_a_real_one() {
        // A NaN and an infinity cannot come out of the embedder, but the
        // ordering must stay total if one ever did.
        let rows = [f32::NAN, 0.0, 0.5, 0.0, f32::INFINITY, 0.0, 1.0, 0.0];
        let ranked = top_k(&[1.0, 0.0], &rows, 4);
        assert_eq!(positions(&ranked), vec![3, 1, 0, 2]);
        assert!(
            ranked[2].score.is_nan(),
            "the score is reported as computed"
        );
        assert!(ranked[3].score.is_infinite() && ranked[3].score.is_sign_positive());
    }

    #[test]
    fn a_stored_row_is_read_back_at_its_catalog_position() {
        let vectors = Vectors::new(&ROWS, 2);
        assert_eq!(vectors.row(0), Some(&[0.0, 1.0][..]));
        assert_eq!(vectors.row(3), Some(&[0.6, 0.8][..]));
        assert_eq!(vectors.row(4), None);
    }

    #[test]
    fn a_similarity_is_the_dot_product_of_two_stored_rows() {
        let vectors = Vectors::new(&ROWS, 2);
        assert_eq!(vectors.similarity(1, 1), Some(1.0));
        assert_eq!(vectors.similarity(1, 2), Some(-1.0));
        assert_eq!(vectors.similarity(0, 1), Some(0.0));
        assert_eq!(vectors.similarity(1, 3), Some(0.6));
    }

    #[test]
    fn a_similarity_against_a_row_that_is_not_there_is_unanswerable() {
        let vectors = Vectors::new(&ROWS, 2);
        assert_eq!(vectors.similarity(0, 9), None);
        assert_eq!(vectors.similarity(9, 0), None);
        assert_eq!(Vectors::new(&ROWS, 0).similarity(0, 0), None);
    }

    #[test]
    fn a_trailing_partial_row_is_ignored() {
        let rows = [1.0, 0.0, 0.0, 1.0, 0.5];
        let ranked = top_k(&[1.0, 0.0], &rows, 4);
        assert_eq!(positions(&ranked), vec![0, 1]);
    }
}
