//! Guards against interim hypotheses whisper hallucinates over silence and
//! noise.

/// A pass above this no-speech probability may have decoded silence.
const NO_SPEECH_THRESHOLD: f32 = 0.6;
/// A pass below this mean token log-probability is a low-confidence guess.
const LOG_PROBABILITY_THRESHOLD: f32 = -1.0;
/// Consecutive copies of one n-gram that mark a decoder loop; two copies are
/// common in real speech.
const LOOP_COPIES: usize = 3;
/// Whole hypotheses whisper emits for silence or noise, in compared form.
/// Short phrases people often say, such as "thank you", "you", and "bye",
/// stay out, so a spoken one shows during the take.
const SILENCE_HALLUCINATIONS: [&str; 3] = [
    "please subscribe",
    "thanks for watching",
    "thank you for watching",
];

/// Token evidence from one interim pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TokenStats {
    /// The pass's highest segment no-speech probability, or `None` when the
    /// runtime reported none.
    pub(crate) no_speech: Option<f32>,
    /// The mean natural log of the pass's token probabilities, or `None` for
    /// a pass without tokens.
    pub(crate) mean_log_probability: Option<f32>,
}

impl TokenStats {
    /// Evidence from a pass's segments, each given as its no-speech
    /// probability and its token probabilities: the highest no-speech
    /// probability, and the mean log-probability over every token of every
    /// segment.
    pub(crate) fn from_segments<T>(segments: impl IntoIterator<Item = (f32, T)>) -> Self
    where
        T: IntoIterator<Item = f32>,
    {
        let mut no_speech = None;
        let mut probabilities = Vec::new();
        for (segment_no_speech, tokens) in segments {
            no_speech = Some(no_speech.map_or(segment_no_speech, |highest: f32| {
                highest.max(segment_no_speech)
            }));
            probabilities.extend(tokens);
        }
        Self {
            no_speech,
            mean_log_probability: mean_log_probability(probabilities),
        }
    }

    fn vetoes(self) -> bool {
        let Some(mean) = self.mean_log_probability else {
            return false;
        };
        mean < LOG_PROBABILITY_THRESHOLD
            && self
                .no_speech
                .is_none_or(|no_speech| no_speech > NO_SPEECH_THRESHOLD)
    }
}

/// The mean natural log of `probabilities`, or `None` when there are none.
fn mean_log_probability(probabilities: impl IntoIterator<Item = f32>) -> Option<f32> {
    let (sum, count) = probabilities
        .into_iter()
        .fold((0.0_f32, 0.0_f32), |(sum, count), probability| {
            (sum + probability.ln(), count + 1.0)
        });
    (count > 0.0).then(|| sum / count)
}

/// `text` with repeated n-gram loops collapsed, or empty when `stats` or the
/// collapsed text marks it as a hallucination.
pub(crate) fn guard_interim(text: &str, stats: TokenStats) -> String {
    if stats.vetoes() {
        return String::new();
    }
    let collapsed = collapse_loops(text);
    if SILENCE_HALLUCINATIONS.contains(&normalized(&collapsed).as_str()) {
        return String::new();
    }
    collapsed
}

/// `text` with every run of at least [`LOOP_COPIES`] consecutive copies of an
/// n-gram cut to one copy, which ends with the run's last word so the run's
/// closing punctuation stays.
fn collapse_loops(text: &str) -> String {
    let words = text.split_whitespace().collect::<Vec<_>>();
    let keys = words
        .iter()
        .map(|word| normalized(word))
        .collect::<Vec<_>>();
    let mut kept = Vec::with_capacity(words.len());
    let mut index = 0;
    while index < words.len() {
        if let Some((width, copies)) = loop_at(&keys[index..]) {
            let end = index + width * copies;
            kept.extend_from_slice(&words[index..index + width - 1]);
            kept.push(words[end - 1]);
            index = end;
        } else {
            kept.push(words[index]);
            index += 1;
        }
    }
    if kept.len() == words.len() {
        text.to_owned()
    } else {
        kept.join(" ")
    }
}

/// The width and copy count of the narrowest n-gram that starts `keys` and
/// repeats at least [`LOOP_COPIES`] times in a row.
fn loop_at(keys: &[String]) -> Option<(usize, usize)> {
    (1..=keys.len() / LOOP_COPIES).find_map(|width| {
        let gram = &keys[..width];
        let copies = 1 + keys[width..]
            .chunks_exact(width)
            .take_while(|chunk| *chunk == gram)
            .count();
        (copies >= LOOP_COPIES).then_some((width, copies))
    })
}

/// `text` lowercased to its letters and digits, one space between words.
fn normalized(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|character| character.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIKELY_SILENT: f32 = 0.9;
    const LOW_CONFIDENCE: f32 = -2.0;
    const SPOKEN: &str = "ask not what your country can do for you";

    fn stats(no_speech: Option<f32>, mean_log_probability: f32) -> TokenStats {
        TokenStats {
            no_speech,
            mean_log_probability: Some(mean_log_probability),
        }
    }

    fn confident() -> TokenStats {
        stats(Some(0.01), -0.1)
    }

    #[test]
    fn likely_silence_with_low_confidence_vetoes_the_hypothesis() {
        assert_eq!(
            guard_interim(SPOKEN, stats(Some(LIKELY_SILENT), LOW_CONFIDENCE)),
            ""
        );
    }

    #[test]
    fn a_no_speech_probability_at_the_threshold_keeps_a_low_confidence_hypothesis() {
        assert_eq!(
            guard_interim(SPOKEN, stats(Some(0.6), LOW_CONFIDENCE)),
            SPOKEN
        );
        assert_eq!(
            guard_interim(SPOKEN, stats(Some(0.6_f32.next_up()), LOW_CONFIDENCE)),
            "",
            "the next probability above 0.6 vetoes"
        );
    }

    #[test]
    fn a_mean_log_probability_at_the_threshold_keeps_a_likely_silent_hypothesis() {
        assert_eq!(
            guard_interim(SPOKEN, stats(Some(LIKELY_SILENT), -1.0)),
            SPOKEN
        );
        assert_eq!(
            guard_interim(SPOKEN, stats(Some(LIKELY_SILENT), (-1.0_f32).next_down())),
            "",
            "the next mean below -1.0 vetoes"
        );
    }

    #[test]
    fn confident_speech_keeps_its_text_even_when_whisper_suspects_silence() {
        assert_eq!(
            guard_interim(SPOKEN, stats(Some(LIKELY_SILENT), -0.2)),
            SPOKEN
        );
        assert_eq!(
            guard_interim(SPOKEN, stats(Some(0.1), LOW_CONFIDENCE)),
            SPOKEN
        );
    }

    #[test]
    fn without_a_no_speech_probability_low_confidence_alone_vetoes() {
        assert_eq!(guard_interim(SPOKEN, stats(None, -1.0)), SPOKEN);
        assert_eq!(
            guard_interim(SPOKEN, stats(None, (-1.0_f32).next_down())),
            ""
        );
    }

    #[test]
    fn a_pass_without_tokens_is_never_vetoed_for_confidence() {
        let no_tokens = TokenStats {
            no_speech: Some(LIKELY_SILENT),
            mean_log_probability: None,
        };
        assert_eq!(guard_interim(SPOKEN, no_tokens), SPOKEN);
    }

    #[test]
    fn mean_log_probability_averages_natural_logs_and_is_none_without_tokens() {
        assert_eq!(mean_log_probability([]), None);
        assert_eq!(mean_log_probability([1.0, 1.0]), Some(0.0));
        let mean = mean_log_probability([1.0, (-2.0_f32).exp()]).expect("two tokens");
        assert!(
            (mean + 1.0).abs() < 1e-6,
            "mean of ln 1 and -2 is -1, got {mean}"
        );
    }

    #[test]
    fn token_stats_take_the_highest_segment_no_speech_probability() {
        let stats = TokenStats::from_segments([(0.2, [0.5]), (0.7, [0.5]), (0.4, [0.5])]);
        assert_eq!(stats.no_speech, Some(0.7));
    }

    #[test]
    fn token_stats_average_every_token_of_every_segment_together() {
        let unlikely = (-3.0_f32).exp();
        let stats = TokenStats::from_segments([
            (0.1, vec![1.0]),
            (0.1, vec![]),
            (0.1, vec![unlikely, unlikely]),
        ]);
        let mean = stats.mean_log_probability.expect("three tokens");
        assert!(
            (mean + 2.0).abs() < 1e-6,
            "mean of ln 1, -3, and -3 is -2, not a mean of segment means, got {mean}"
        );
    }

    #[test]
    fn token_stats_of_a_pass_without_segments_are_empty() {
        let stats = TokenStats::from_segments(Vec::<(f32, Vec<f32>)>::new());
        assert_eq!(
            stats,
            TokenStats {
                no_speech: None,
                mean_log_probability: None,
            }
        );
    }

    #[test]
    fn clean_text_is_returned_exactly() {
        let text = "We choose  to go, to the moon.";
        assert_eq!(guard_interim(text, confident()), text);
    }

    #[test]
    fn a_word_repeated_three_times_collapses_to_one() {
        assert_eq!(
            guard_interim("the the the the cat sat", confident()),
            "the cat sat"
        );
    }

    #[test]
    fn a_phrase_loop_collapses_to_one_copy_ending_with_the_last_copy() {
        assert_eq!(
            guard_interim("And I said, I said, I said. Hello", confident()),
            "And I said. Hello"
        );
        assert_eq!(
            guard_interim(
                "Then we go there, We go there, we go THERE. Done",
                confident()
            ),
            "Then we go THERE. Done",
            "comparison ignores case and the kept copy takes the run's last word"
        );
    }

    #[test]
    fn two_copies_are_not_a_loop() {
        let text = "it was very very good, good";
        assert_eq!(guard_interim(text, confident()), text);
    }

    #[test]
    fn a_hypothesis_of_only_a_listed_silence_phrase_is_vetoed() {
        for text in [
            "Thanks for watching!",
            "Thank you for watching.",
            "Please subscribe.",
        ] {
            assert_eq!(guard_interim(text, confident()), "", "{text} vetoes");
        }
    }

    #[test]
    fn short_phrases_people_often_say_are_kept_alone() {
        for text in ["Thank you.", "you", "Bye.", "Thank you. Thank you."] {
            assert_eq!(guard_interim(text, confident()), text, "{text} stays");
        }
    }

    #[test]
    fn a_listed_phrase_inside_longer_speech_is_kept() {
        for text in [
            "Thanks for watching the demo with me.",
            "please subscribe to the feed",
        ] {
            assert_eq!(guard_interim(text, confident()), text, "{text} stays");
        }
    }

    #[test]
    fn a_looped_silence_phrase_collapses_and_then_vetoes() {
        assert_eq!(
            guard_interim(
                "Thanks for watching. Thanks for watching. Thanks for watching.",
                confident()
            ),
            ""
        );
    }

    #[test]
    fn every_listed_phrase_is_written_in_its_compared_form() {
        for phrase in SILENCE_HALLUCINATIONS {
            assert_eq!(normalized(phrase), phrase);
        }
    }
}
