//! Repeats the fast pass decodes from the silence after speech.
//!
//! Whisper often fills trailing silence by repeating the words before it,
//! as in "create a plan. I want you to create". A hypothesis tail is cut
//! when it repeats the start of the run of words just before it and the
//! speech the energy gate heard after the words before the tail is too short
//! to have said the words decoded since. A phrase the speaker repeats keeps
//! its speech, so it stays.

use gateway_stt_engine::EnginePolicy;

use crate::segment::SpeechBefore;
use crate::take::agreement::{equivalent_token, token_spans};

/// Speech one spoken word takes at the least: 100 ms. Words decoded after a
/// point that less speech than this per word followed were not all said.
const MIN_WORD_SAMPLES: u64 = (EnginePolicy::SAMPLE_RATE / 10) as u64;
/// Words back from a tail at which the run it repeats may start.
pub(super) const MAX_REPEATED_RUN_WORDS: usize = 32;

/// A hypothesis less any tail cut from it. A cut shows the decode heard audio
/// without speech past the last kept word, which is the continuation a
/// punctuated last word otherwise waits for.
#[derive(Clone, Copy, Debug)]
pub(in crate::take) struct Spoken<'text> {
    text: &'text str,
    cut: bool,
}

impl<'text> Spoken<'text> {
    pub(super) fn new(hypothesis: &'text str, end: usize) -> Self {
        Self {
            text: &hypothesis[..end],
            cut: end < hypothesis.len(),
        }
    }

    pub(in crate::take) const fn text(self) -> &'text str {
        self.text
    }

    pub(super) const fn cut(self) -> bool {
        self.cut
    }
}

impl<'text> From<&'text str> for Spoken<'text> {
    fn from(text: &'text str) -> Self {
        Self { text, cut: false }
    }
}

/// Byte end of `hypothesis` once a tail that no speech produced is cut.
///
/// `before` holds the words shown before the hypothesis, its last
/// `MAX_REPEATED_RUN_WORDS` at the most. `seen[i]` is the end of the window in
/// which an earlier hypothesis first decoded the hypothesis's word `i` at the
/// same place, for its leading such words; words past them were decoded from
/// the window that starts at `window_start`. A tail starts past those words,
/// which an earlier pass already heard, and one word repeats only the word
/// right before it, since a lone word often recurs in real speech. Shorter
/// cuts are tried after longer ones, so a whole repeated run leaves together.
pub(super) fn spoken_end(
    hypothesis: &str,
    before: &[&str],
    seen: &[u64],
    window_start: u64,
    speech: SpeechBefore,
) -> usize {
    let spans = token_spans(hypothesis);
    let words = before
        .iter()
        .copied()
        .chain(spans.iter().map(|(token, _, _)| *token))
        .collect::<Vec<_>>();
    let since = seen.last().copied().unwrap_or(window_start);
    for split in seen.len()..spans.len() {
        let tail = spans.len() - split;
        let at = before.len() + split;
        let longest_run = if tail == 1 { 1 } else { MAX_REPEATED_RUN_WORDS };
        let repeats = (tail..=at.min(longest_run)).any(|run| {
            (0..tail).all(|index| equivalent_token(words[at - run + index], words[at + index]))
        });
        let unseen = u64::try_from(split - seen.len() + tail).unwrap_or(u64::MAX);
        if repeats && speech.after(since) < unseen.saturating_mul(MIN_WORD_SAMPLES) {
            return split.checked_sub(1).map_or(0, |last| spans[last].2);
        }
    }
    hypothesis.len()
}

#[cfg(test)]
mod tests {
    use super::{MIN_WORD_SAMPLES, spoken_end};
    use crate::segment::SpeechBefore;

    const WINDOW_START: u64 = 0;
    const SPEECH_END: u64 = 88_000;
    const HANGOVER: u64 = 1_600;

    fn spoken<'text>(
        hypothesis: &'text str,
        before: &[&str],
        seen: &[u64],
        speech_end: u64,
    ) -> &'text str {
        &hypothesis[..spoken_end(
            hypothesis,
            before,
            seen,
            WINDOW_START,
            SpeechBefore::for_test(speech_end),
        )]
    }

    #[test]
    fn a_restarted_sentence_after_silence_is_cut_at_the_sentence_end() {
        let seen = [87_000; 7];
        assert_eq!(
            spoken(
                "I want you to create a plan. I want you to create",
                &[],
                &seen,
                SPEECH_END
            ),
            "I want you to create a plan."
        );
    }

    #[test]
    fn a_repeat_the_speaker_says_after_the_words_it_repeats_stays() {
        let seen = [SPEECH_END - HANGOVER - 5 * MIN_WORD_SAMPLES; 7];
        let hypothesis = "I want you to create a plan. I want you to create";
        assert_eq!(spoken(hypothesis, &[], &seen, SPEECH_END), hypothesis);
    }

    #[test]
    fn a_word_repeated_after_silence_is_cut_and_one_said_twice_stays() {
        assert_eq!(
            spoken("create a plan. Plan.", &[], &[87_000; 3], SPEECH_END),
            "create a plan."
        );
        let said = SPEECH_END - HANGOVER - MIN_WORD_SAMPLES;
        assert_eq!(
            spoken("create a plan. Plan.", &[], &[said; 3], SPEECH_END),
            "create a plan. Plan."
        );
    }

    #[test]
    fn unseen_words_before_a_repeat_need_their_own_speech() {
        let seen = [80_000; 6];
        let hypothesis = "I want you to create a plan. I want you to create";
        assert_eq!(
            spoken(hypothesis, &[], &seen, 84_000),
            "I want you to create a plan."
        );
        let room = 80_000 + HANGOVER + 6 * MIN_WORD_SAMPLES;
        assert_eq!(spoken(hypothesis, &[], &seen, room), hypothesis);
    }

    #[test]
    fn a_hypothesis_that_only_repeats_the_shown_words_before_it_is_cut_whole() {
        let before = ["create", "a", "plan."];
        assert_eq!(spoken("a plan", &before, &[], HANGOVER), "");
        assert_eq!(spoken("a plan", &before, &[], SPEECH_END), "a plan");
    }

    #[test]
    fn a_word_an_earlier_pass_already_decoded_at_its_place_is_never_cut() {
        let before = ["Can", "you", "check", "whether", "it", "runs?"];
        let hypothesis = "Thank you.";
        assert_eq!(
            spoken(hypothesis, &before, &[87_000; 2], SPEECH_END),
            hypothesis,
            "both words were heard by the earlier pass, so neither came from the silence"
        );
    }

    #[test]
    fn one_word_repeats_only_the_word_right_before_it() {
        let before = ["Can", "you", "check", "whether", "it", "runs?"];
        assert_eq!(
            spoken("Thank you.", &before, &[87_000], SPEECH_END),
            "Thank you.",
            "an earlier you several words back is not this word's echo"
        );
        assert_eq!(
            spoken("Thank thank", &before, &[87_000], SPEECH_END),
            "Thank"
        );
    }

    #[test]
    fn a_tail_that_repeats_nothing_is_kept_after_silence() {
        let seen = [80_000; 4];
        assert_eq!(
            spoken("ask not what you", &[], &seen, 80_000),
            "ask not what you"
        );
    }
}
