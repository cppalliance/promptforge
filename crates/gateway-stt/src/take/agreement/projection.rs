use std::ops::Range;

use super::final_overlap::MAX_FINAL_TRANSCRIPT_BYTES;

const PUNCTUATION_TOKEN_BAND: usize = 3;
const MAX_PUNCTUATION_TOKENS: usize = PUNCTUATION_TOKEN_BAND * 2 + 1;
const MAX_PROJECTION_TOKENS: usize = MAX_FINAL_TRANSCRIPT_BYTES.div_ceil(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::take) struct ProjectionMetrics {
    pub(in crate::take) input_bytes: usize,
    pub(in crate::take) tokens: usize,
    pub(in crate::take) audio_before_overlap: u64,
    pub(in crate::take) audio_total: u64,
    pub(in crate::take) rounded_tokens: usize,
    pub(in crate::take) selected_tokens: usize,
    pub(in crate::take) punctuation_examined: usize,
    pub(in crate::take) punctuation_candidates: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::take) struct ProjectedPrefix {
    pub(in crate::take) byte_end: usize,
    pub(in crate::take) metrics: ProjectionMetrics,
}

pub(in crate::take) fn projected_prefix_end(
    previous: &str,
    previous_range: Range<u64>,
    overlap_start: u64,
) -> Option<ProjectedPrefix> {
    if previous.len() > MAX_FINAL_TRANSCRIPT_BYTES {
        return None;
    }
    let audio_total = previous_range.end.checked_sub(previous_range.start)?;
    let audio_before_overlap = overlap_start.checked_sub(previous_range.start)?;
    if audio_total == 0 || audio_before_overlap > audio_total {
        return None;
    }
    let tokens = count_tokens(previous)?;
    let rounded_tokens = project_tokens(audio_before_overlap, audio_total, tokens)?.min(tokens);
    let cut = locate_cut(previous, tokens, rounded_tokens);
    Some(ProjectedPrefix {
        byte_end: cut.byte_end,
        metrics: ProjectionMetrics {
            input_bytes: previous.len(),
            tokens,
            audio_before_overlap,
            audio_total,
            rounded_tokens,
            selected_tokens: cut.selected_tokens,
            punctuation_examined: cut.punctuation_examined,
            punctuation_candidates: cut.punctuation_candidates,
        },
    })
}

fn count_tokens(text: &str) -> Option<usize> {
    let mut tokens = 0_usize;
    let mut inside = false;
    for character in text.chars() {
        if character.is_whitespace() {
            if inside {
                tokens = tokens.checked_add(1)?;
                if tokens > MAX_PROJECTION_TOKENS {
                    return None;
                }
                inside = false;
            }
        } else {
            inside = true;
        }
    }
    if inside {
        tokens = tokens.checked_add(1)?;
    }
    (tokens <= MAX_PROJECTION_TOKENS).then_some(tokens)
}

fn project_tokens(before: u64, total: u64, tokens: usize) -> Option<usize> {
    let numerator = u128::from(before).checked_mul(tokens as u128)?;
    let denominator = u128::from(total);
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    // Exact half-token ties stay on the prior side so estimation never claims
    // an extra overlap token merely because the transcript density is unknown.
    let rounded = quotient.checked_add(u128::from(remainder.checked_mul(2)? > denominator))?;
    usize::try_from(rounded).ok()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LocatedCut {
    byte_end: usize,
    selected_tokens: usize,
    punctuation_examined: usize,
    punctuation_candidates: usize,
}

fn locate_cut(text: &str, tokens: usize, projected: usize) -> LocatedCut {
    let prefer_punctuation = projected > 0 && projected < tokens;
    let candidate_start = projected.saturating_sub(PUNCTUATION_TOKEN_BAND).max(1);
    let candidate_end = projected.saturating_add(PUNCTUATION_TOKEN_BAND).min(tokens);
    let mut token_start = None;
    let mut token_index = 0;
    let mut projected_end = 0;
    let mut punctuation_end = 0;
    let mut punctuation_index = 0;
    let mut punctuation_examined = 0;
    let mut punctuation_candidates = 0;
    for (byte_index, character) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        match (token_start, character.is_whitespace()) {
            (None, false) => token_start = Some(byte_index),
            (Some(start), true) => {
                token_index += 1;
                if token_index == projected {
                    projected_end = byte_index;
                }
                if prefer_punctuation && (candidate_start..=candidate_end).contains(&token_index) {
                    punctuation_examined += 1;
                    if is_punctuation_boundary(&text[start..byte_index]) {
                        punctuation_candidates += 1;
                        punctuation_end = byte_index;
                        punctuation_index = token_index;
                    }
                }
                token_start = None;
            }
            _ => {}
        }
    }
    debug_assert!(punctuation_examined <= MAX_PUNCTUATION_TOKENS);
    if punctuation_candidates == 1 {
        LocatedCut {
            byte_end: punctuation_end,
            selected_tokens: punctuation_index,
            punctuation_examined,
            punctuation_candidates,
        }
    } else {
        LocatedCut {
            byte_end: projected_end,
            selected_tokens: projected,
            punctuation_examined,
            punctuation_candidates,
        }
    }
}

fn is_punctuation_boundary(token: &str) -> bool {
    token.chars().next_back().is_some_and(|character| {
        matches!(
            character,
            '.' | '!' | '?' | ',' | ';' | ':' | '。' | '！' | '？' | '，' | '；' | '：'
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_uses_u128_nearest_rounding_with_ties_earlier() {
        assert_eq!(project_tokens(1, 4, 2), Some(0));
        assert_eq!(project_tokens(3, 4, 2), Some(1));
        assert_eq!(project_tokens(u64::MAX - 1, u64::MAX, 2), Some(2));

        let origin = u64::MAX - 160;
        let projected = projected_prefix_end(
            "zero one two three four five six seven eight nine",
            origin..u64::MAX,
            origin + 32,
        )
        .expect("near-u64-limit projection remains bounded");
        assert_eq!(projected.metrics.rounded_tokens, 2);
        assert_eq!(
            &"zero one two three four five six seven eight nine"[..projected.byte_end],
            "zero one"
        );
    }

    #[test]
    fn one_nearby_sentence_or_clause_boundary_wins_uniquely() {
        let projected = projected_prefix_end("one two three, four five six seven", 0..70, 20)
            .expect("bounded projection succeeds");
        assert_eq!(projected.metrics.rounded_tokens, 2);
        assert_eq!(projected.metrics.selected_tokens, 3);
        assert_eq!(projected.metrics.punctuation_candidates, 1);
        assert_eq!(
            &"one two three, four five six seven"[..projected.byte_end],
            "one two three,"
        );
    }

    #[test]
    fn ambiguous_or_distant_punctuation_keeps_the_projected_boundary() {
        let ambiguous = projected_prefix_end("one, two three, four five", 0..50, 20)
            .expect("ambiguous projection succeeds");
        assert_eq!(ambiguous.metrics.rounded_tokens, 2);
        assert_eq!(ambiguous.metrics.selected_tokens, 2);
        assert_eq!(ambiguous.metrics.punctuation_candidates, 2);

        let distant = projected_prefix_end("one two three four five six, seven", 0..70, 20)
            .expect("distant projection succeeds");
        assert_eq!(distant.metrics.selected_tokens, 2);
        assert_eq!(distant.metrics.punctuation_candidates, 0);
        assert!(distant.metrics.punctuation_examined <= MAX_PUNCTUATION_TOKENS);
    }

    #[test]
    fn byte_token_and_punctuation_bounds_cover_worst_case_input() {
        let maximum = std::iter::repeat_n("x", MAX_PROJECTION_TOKENS)
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(maximum.len(), MAX_FINAL_TRANSCRIPT_BYTES - 1);
        let projected = projected_prefix_end(&maximum, 0..u64::MAX, u64::MAX / 2)
            .expect("maximum token input remains bounded");
        assert_eq!(projected.metrics.tokens, MAX_PROJECTION_TOKENS);
        assert!(projected.metrics.punctuation_examined <= MAX_PUNCTUATION_TOKENS);
        assert!(
            projected_prefix_end(&format!("{maximum} xx"), 0..u64::MAX, u64::MAX / 2,).is_none()
        );
    }
}
