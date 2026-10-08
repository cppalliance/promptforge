//! Token-level agreement helpers shared by take reconciliation and windowing.

#[path = "agreement-final-overlap.rs"]
mod final_overlap;
#[path = "agreement-projection.rs"]
mod projection;

pub(super) use final_overlap::{
    anchored_final_end, final_transcript_within_limit, range_guided_suffix_prefix_start,
};
pub(super) use projection::projected_prefix_end;

/// Bounded work that one range-guided alignment attempt did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct AlignmentMetrics {
    pub(super) input_bytes: usize,
    pub(super) tokens: usize,
    pub(super) normalization_work: usize,
    pub(super) dp_cells: usize,
}

/// Why range-guided alignment of two overlapping final windows located no
/// overlap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AlignmentReason {
    /// A transcript is over the final transcript byte limit.
    TranscriptBytes,
    /// A transcript has more tokens than alignment considers, or a token
    /// longer than it normalizes.
    TokenLimit,
    /// Normalizing the tokens to align went over the normalization bounds.
    NormalizationLimit,
    /// The overlap does not lie inside one window's decode range.
    OverlapOutsideRange,
    /// No pair of boundaries aligns within the edit and coverage bounds.
    NoCandidate,
    /// Several distinct boundary pairs score alike, so none is trusted.
    AmbiguousCandidates,
}

impl AlignmentReason {
    /// The stable code that logs carry for this reason.
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::TranscriptBytes => "transcript_bytes",
            Self::TokenLimit => "token_limit",
            Self::NormalizationLimit => "normalization_limit",
            Self::OverlapOutsideRange => "overlap_outside_range",
            Self::NoCandidate => "no_candidate",
            Self::AmbiguousCandidates => "ambiguous_candidates",
        }
    }
}

/// A failed range-guided alignment: the reason and the work it did first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AlignmentFailure {
    pub(super) reason: AlignmentReason,
    pub(super) metrics: AlignmentMetrics,
}

pub(super) fn matching_token_prefix_end(previous: &str, current: &str) -> usize {
    let previous = token_spans(previous);
    let current = token_spans(current);
    previous
        .iter()
        .zip(&current)
        .take_while(|((left, _, _), (right, _, _))| left == right || equivalent_token(left, right))
        .map(|(_, (_, _, end))| *end)
        .last()
        .unwrap_or(0)
}

/// Lowercase alphanumerics of a token that has any, else the token itself, so
/// two forms are equal exactly when `matching_token_prefix_end` matches the
/// tokens.
pub(super) fn normalized_token(token: &str) -> String {
    if token.chars().any(char::is_alphanumeric) {
        folded(token).collect()
    } else {
        token.to_owned()
    }
}

/// Whitespace-separated tokens of `text` with their byte start and end.
pub(crate) fn token_spans(text: &str) -> Vec<(&str, usize, usize)> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (index, character) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        match (start, character.is_whitespace()) {
            (None, false) => start = Some(index),
            (Some(begin), true) => {
                tokens.push((&text[begin..index], begin, index));
                start = None;
            }
            _ => {}
        }
    }
    tokens
}

pub(super) fn equivalent_token(left: &str, right: &str) -> bool {
    left.chars().any(char::is_alphanumeric) && folded(left).eq(folded(right))
}

/// Lowercase alphanumerics of `token`, the one fold that both
/// `equivalent_token` and `normalized_token` compare.
fn folded(token: &str) -> impl Iterator<Item = char> {
    token
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
}

#[cfg(test)]
mod tests {
    use super::{
        AlignmentReason, equivalent_token, matching_token_prefix_end, normalized_token,
        range_guided_suffix_prefix_start,
    };

    #[test]
    fn normalized_forms_are_equal_exactly_when_the_tokens_match() {
        for (left, right) in [
            ("Ask", "ask"),
            ("not,", "not"),
            ("now", "not"),
            ("...", "..."),
            ("...", "?!"),
            ("way?", "?"),
            ("?", "way?"),
        ] {
            assert_eq!(
                normalized_token(left) == normalized_token(right),
                matching_token_prefix_end(left, right) > 0,
                "{left:?} against {right:?}"
            );
        }
    }

    #[test]
    fn matching_token_prefix_end_ignores_punctuation_and_case_differences() {
        assert_eq!(
            matching_token_prefix_end("ask not, what", "ask not what"),
            "ask not what".len()
        );
        assert_eq!(
            matching_token_prefix_end("Ask not what", "ask not what"),
            "ask not what".len()
        );
        assert_eq!(
            matching_token_prefix_end("ask not what", "ask now what"),
            "ask".len()
        );
        assert_eq!(
            matching_token_prefix_end("so ... then", "so ... than"),
            "so ...".len(),
            "an identical punctuation-only token still matches"
        );
    }

    #[test]
    fn punctuation_only_tokens_never_establish_overlap() {
        assert!(!equivalent_token("...", "?!"));
        assert_eq!(
            range_guided_suffix_prefix_start(
                "canonical ...",
                0..100,
                "?! replacement",
                50..150,
                50..100,
            )
            .map_err(|failure| failure.reason),
            Err(AlignmentReason::NoCandidate)
        );
    }
}
