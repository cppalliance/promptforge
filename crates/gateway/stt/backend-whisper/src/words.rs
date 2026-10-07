//! Word end times from whisper's per-token timing.

use std::time::Duration;

use gateway_stt_engine::EnginePolicy;
use gateway_whisper_ffi::{WhisperError, WhisperState};

const MICROS_PER_SECOND: u128 = 1_000_000;

/// Where each word of `state`'s most recent pass ends, in samples from its
/// first sample and held within its `window` samples, or empty when the pass
/// timed no tokens.
pub(crate) fn pass_word_ends(
    state: &WhisperState,
    window: usize,
) -> Result<Vec<u64>, WhisperError> {
    let mut tokens = Vec::new();
    for segment in 0..state.segment_count() {
        for token in 0..state.token_count(segment)? {
            let Some(span) = state.token_span(segment, token)? else {
                return Ok(Vec::new());
            };
            tokens.push((state.token_text(segment, token)?, span.end));
        }
    }
    let window = u64::try_from(window).unwrap_or(u64::MAX);
    Ok(word_ends(
        tokens.iter().map(|(text, end)| (text.as_str(), *end)),
        window,
    ))
}

/// The end of each whitespace-delimited word `tokens` spell, in samples from
/// the first sample and held at `window`.
///
/// A word ends where the last token adding a character to it ends. Special
/// tokens, which whisper names `[_..._]`, spell nothing.
pub(crate) fn word_ends<'a>(
    tokens: impl IntoIterator<Item = (&'a str, Duration)>,
    window: u64,
) -> Vec<u64> {
    let mut ends = Vec::new();
    let mut in_word = false;
    for (text, end) in tokens {
        if text.starts_with("[_") && text.ends_with(']') {
            continue;
        }
        let end = samples(end).min(window);
        for character in text.chars() {
            if character.is_whitespace() {
                in_word = false;
            } else if in_word {
                if let Some(last) = ends.last_mut() {
                    *last = end;
                }
            } else {
                ends.push(end);
                in_word = true;
            }
        }
    }
    ends
}

fn samples(offset: Duration) -> u64 {
    let samples = offset.as_micros() * EnginePolicy::SAMPLE_RATE as u128 / MICROS_PER_SECOND;
    u64::try_from(samples).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const WINDOW: u64 = 160_000;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn a_token_opening_with_whitespace_begins_a_word_and_any_other_extends_it() {
        let tokens = [
            (" And", ms(300)),
            (" so", ms(500)),
            (" Americ", ms(1_000)),
            ("ans", ms(1_250)),
        ];
        assert_eq!(word_ends(tokens, WINDOW), [4_800, 8_000, 20_000]);
    }

    #[test]
    fn punctuation_extends_the_word_it_follows() {
        let tokens = [(" you", ms(500)), (",", ms(600)), (" ask", ms(900))];
        assert_eq!(word_ends(tokens, WINDOW), [9_600, 14_400]);
    }

    #[test]
    fn a_first_token_without_leading_whitespace_begins_the_first_word() {
        let tokens = [("Ask", ms(400)), (" not", ms(700))];
        assert_eq!(word_ends(tokens, WINDOW), [6_400, 11_200]);
    }

    #[test]
    fn special_tokens_neither_begin_nor_extend_a_word() {
        let tokens = [
            ("[_BEG_]", ms(0)),
            (" ask", ms(400)),
            ("[_TT_35]", ms(700)),
            ("[_EOT_]", ms(900)),
        ];
        assert_eq!(word_ends(tokens, WINDOW), [6_400]);
    }

    #[test]
    fn a_whitespace_only_token_ends_a_word_without_beginning_one() {
        let tokens = [(" ask", ms(400)), (" ", ms(500)), ("not", ms(700))];
        assert_eq!(word_ends(tokens, WINDOW), [6_400, 11_200]);
    }

    #[test]
    fn a_token_holding_several_words_ends_each_of_them() {
        let tokens = [(" ask not", ms(700)), (" what", ms(900))];
        assert_eq!(word_ends(tokens, WINDOW), [11_200, 11_200, 14_400]);
    }

    #[test]
    fn an_end_past_the_window_is_held_at_the_window_end() {
        let tokens = [(" ask", ms(400)), (" not", ms(1_200))];
        assert_eq!(word_ends(tokens, 16_000), [6_400, 16_000]);
    }

    #[test]
    fn ends_number_one_per_whitespace_delimited_word_of_the_spelled_text() {
        let tokens = [
            ("[_BEG_]", ms(0)),
            (" And", ms(300)),
            (" so", ms(500)),
            (" my", ms(700)),
            (" fellow", ms(900)),
            (" Americ", ms(1_100)),
            ("ans", ms(1_300)),
            (",", ms(1_350)),
            (" ask", ms(1_700)),
            (" not", ms(1_900)),
            (".", ms(2_000)),
            ("[_TT_100]", ms(2_000)),
        ];
        let spelled = tokens
            .iter()
            .map(|(text, _)| *text)
            .filter(|text| !text.starts_with("[_"))
            .collect::<String>();
        let ends = word_ends(tokens, WINDOW);
        assert_eq!(ends.len(), spelled.split_whitespace().count());
        assert_eq!(ends.len(), 7, "{spelled:?}");
    }

    #[test]
    fn no_tokens_yield_no_ends() {
        assert!(word_ends([], WINDOW).is_empty());
    }
}
