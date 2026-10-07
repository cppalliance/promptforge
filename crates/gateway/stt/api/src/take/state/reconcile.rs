//! Aligned rewrite of a natural final over the interim text it replaces.

use super::FinalizedState;
use crate::take::agreement::{anchored_final_end, normalized_token, token_spans};
use crate::take::live_prefix::AnchoredSuffix;
use crate::take::text::append_transcript;
use crate::take::window::ShownHypotheses;

/// Displayed tokens before kept words that must equal the final's last tokens.
const SUFFIX_ANCHOR_TOKENS: usize = 2;
/// Displayed words after the final's last word that an anchored rewrite keeps.
const MAX_ANCHORED_SUFFIX_WORDS: usize = 5;

/// Settles `text` as the authoritative final through `range_end` and keeps
/// the displayed words after its last word live when they are anchored.
pub(super) fn rewrite_natural(
    state: &mut FinalizedState,
    text: &str,
    range_end: u64,
    shown: &ShownHypotheses,
) {
    let (displayed, agreed_end) = shown.after(state.samples);
    append_transcript(&mut state.text, text);
    append_transcript(&mut state.decoded_text, text);
    state.samples = range_end;
    state.transcribed_samples = range_end;
    state.anchored = anchored_final_end(text, &displayed, SUFFIX_ANCHOR_TOKENS)
        .and_then(|end| anchored_suffix(&displayed, end, agreed_end))
        .filter(|suffix| !repeats_final(text, suffix.text()));
}

/// Whether every word of `suffix`, in order, repeats a run of words in
/// `final_text`: the fast pass echoing audio the final covers, which it
/// often decodes from trailing silence, rather than speech after the final.
fn repeats_final(final_text: &str, suffix: &str) -> bool {
    let words = |text: &str| {
        token_spans(text)
            .into_iter()
            .map(|(token, _, _)| normalized_token(token))
            .collect::<Vec<_>>()
    };
    let echoed = words(suffix);
    !echoed.is_empty()
        && words(final_text)
            .windows(echoed.len())
            .any(|run| run == echoed.as_slice())
}

/// The first `MAX_ANCHORED_SUFFIX_WORDS` words of `displayed` after byte
/// `end`, of which those ending by byte `agreed_end` were shown agreed.
fn anchored_suffix(displayed: &str, end: usize, agreed_end: usize) -> Option<AnchoredSuffix> {
    let mut text = String::new();
    let mut agreed = 0;
    for (word, _, word_end) in token_spans(&displayed[end..])
        .into_iter()
        .take(MAX_ANCHORED_SUFFIX_WORDS)
    {
        append_transcript(&mut text, word);
        if end + word_end <= agreed_end {
            agreed = text.len();
        }
    }
    (!text.is_empty()).then(|| AnchoredSuffix::new(text, agreed))
}
