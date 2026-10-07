//! Per-token agreement evidence for the active whole-window region.
//!
//! A token is agreed once its normalized form holds at the same position in
//! enough hypotheses spread over enough audio. Agreed text never shrinks: a
//! hypothesis that disputes it replaces only the tentative text after it. A
//! hypothesis's last word that ends in punctuation waits for a later
//! hypothesis to continue past it, because the window edge is where whisper
//! attaches punctuation that the normalized form ignores but the display shows.

use std::collections::VecDeque;

use gateway_stt_engine::EnginePolicy;

use super::Spoken;
use crate::take::agreement::{matching_token_prefix_end, normalized_token, token_spans};

/// Hypotheses that must carry a token at the same position before it is agreed.
const MIN_AGREEMENT_OBSERVATIONS: usize = 2;
/// Audio-end distance, half a second in samples, that must separate a token's
/// first and latest observation before it is agreed.
const MIN_AGREEMENT_SPREAD_SAMPLES: u64 = (EnginePolicy::SAMPLE_RATE / 2) as u64;
/// Token similarity that a hypothesis disputing agreed text must reach with a
/// recent hypothesis to avoid being held.
const OUTLIER_SIMILARITY: f64 = 0.35;
/// Recent hypotheses a disputing hypothesis is compared with.
const OUTLIER_HISTORY: usize = 5;
/// Leading tokens of each hypothesis that similarity reads.
const MAX_SIMILARITY_TOKENS: usize = 128;
/// Prefix lengths around the agreed token count searched for the end of the
/// hypothesis prefix that disputed agreed text covers.
const CUT_TOKEN_BAND: usize = 8;
const CUT_BAND_WIDTH: usize = CUT_TOKEN_BAND * 2 + 1;

#[derive(Debug)]
struct TokenEvidence {
    normalized: String,
    observations: usize,
    first_end: u64,
    latest_end: u64,
    awaits_continuation: bool,
}

impl TokenEvidence {
    const fn new(normalized: String, audio_end: u64, awaits_continuation: bool) -> Self {
        Self {
            normalized,
            observations: 1,
            first_end: audio_end,
            latest_end: audio_end,
            awaits_continuation,
        }
    }

    fn settled(&self) -> bool {
        !self.awaits_continuation
            && self.observations >= MIN_AGREEMENT_OBSERVATIONS
            && self.latest_end.saturating_sub(self.first_end) >= MIN_AGREEMENT_SPREAD_SAMPLES
    }
}

/// Evidence for each token position of the active text, of which the first
/// `agreed` are agreed and end at byte `agreed_end`.
#[derive(Debug, Default)]
pub(super) struct Agreement {
    tokens: Vec<TokenEvidence>,
    agreed: usize,
    agreed_end: usize,
    recent: VecDeque<Vec<String>>,
    held: Option<Vec<String>>,
}

impl Agreement {
    /// Agreement over `active` whose first `agreed_end` bytes stay agreed, as
    /// words carried across a natural final that were agreed before it.
    pub(super) fn seeded(active: &str, agreed_end: usize) -> Self {
        let tokens = token_spans(&active[..agreed_end])
            .into_iter()
            .map(|(token, _, _)| TokenEvidence::new(normalized_token(token), 0, false))
            .collect::<Vec<_>>();
        Self {
            agreed: tokens.len(),
            tokens,
            agreed_end,
            ..Self::default()
        }
    }

    pub(super) const fn agreed_end(&self) -> usize {
        self.agreed_end
    }

    /// For each leading word of `words` that earlier hypotheses decoded at
    /// the same position, the window end of the first one that did.
    pub(super) fn first_seen(&self, words: &[String]) -> Vec<u64> {
        self.tokens
            .iter()
            .zip(words)
            .take_while(|(token, word)| token.normalized == **word)
            .map(|(token, _)| token.first_end)
            .collect()
    }

    /// Revises `active`, whose agreed prefix this agreement tracks, with
    /// `replacement`, the hypothesis rebased onto `active` and decoded through
    /// sample `audio_end`. Returns the new active text, or `None` when the
    /// hypothesis is held as an outlier.
    ///
    /// Evidence starts after the agreed prefix, or after the aligned cut when
    /// the replacement disputes it, so a position always names the same place
    /// after agreed text. A hypothesis with a cut tail heard past its last
    /// word, so that word does not wait for a later hypothesis.
    pub(super) fn revise(
        &mut self,
        active: &str,
        hypothesis: Spoken<'_>,
        replacement: &str,
        audio_end: u64,
    ) -> Option<String> {
        let agreed_text = &active[..self.agreed_end];
        let spans = token_spans(replacement);
        let prefix_end = matching_token_prefix_end(agreed_text, replacement);
        let extends = spans
            .iter()
            .take_while(|(_, _, end)| *end <= prefix_end)
            .count()
            == self.agreed;
        let similarity = similarity_tokens(hypothesis.text());
        match self.held.take() {
            Some(held) if similar(&similarity, &held) => self.remember(held),
            _ if !extends
                && !self
                    .recent
                    .iter()
                    .any(|recent| similar(&similarity, recent)) =>
            {
                self.held = Some(similarity);
                return None;
            }
            _ => {}
        }
        self.remember(similarity);

        let normalized = spans
            .iter()
            .map(|(token, _, _)| normalized_token(token))
            .collect::<Vec<_>>();
        let cut = if extends {
            self.agreed
        } else {
            aligned_cut(&self.tokens[..self.agreed], &normalized)
        };
        let rest_start = cut.checked_sub(1).map_or(0, |last| spans[last].2);
        let rest = &replacement[rest_start..];
        let mut text = agreed_text.to_owned();
        if !text.is_empty() && !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            text.push(' ');
        }
        let offset = text.len();
        text.push_str(rest);
        let punctuated_end = spans.last().is_some_and(|(token, _, _)| {
            token.ends_with(|character: char| !character.is_alphanumeric())
        });
        self.observe(
            &normalized[cut..],
            punctuated_end,
            hypothesis.cut(),
            audio_end,
        );
        let first = self.agreed;
        while self
            .tokens
            .get(self.agreed)
            .is_some_and(TokenEvidence::settled)
        {
            self.agreed += 1;
        }
        if self.agreed > first {
            self.agreed_end = offset + spans[cut + self.agreed - first - 1].2 - rest_start;
        }
        Some(text)
    }

    /// Records `tentative`, the hypothesis tokens after the cut, at the
    /// positions after agreed text; `punctuated_end` says whether the last of
    /// them ends in punctuation, and `continued` whether the decode heard
    /// past it.
    fn observe(
        &mut self,
        tentative: &[String],
        punctuated_end: bool,
        continued: bool,
        audio_end: u64,
    ) {
        let agreed = self.agreed;
        let end = agreed + tentative.len();
        self.tokens.truncate(end);
        for (position, normalized) in (agreed..).zip(tentative) {
            let waits = position + 1 == end && !continued;
            match self.tokens.get_mut(position) {
                Some(token) if token.normalized == *normalized => {
                    token.observations = token.observations.saturating_add(1);
                    token.latest_end = token.latest_end.max(audio_end);
                    token.awaits_continuation =
                        waits && (punctuated_end || token.awaits_continuation);
                }
                Some(token) => {
                    *token =
                        TokenEvidence::new(normalized.clone(), audio_end, waits && punctuated_end);
                }
                None => self.tokens.push(TokenEvidence::new(
                    normalized.clone(),
                    audio_end,
                    waits && punctuated_end,
                )),
            }
        }
    }

    fn remember(&mut self, tokens: Vec<String>) {
        if self.recent.len() == OUTLIER_HISTORY {
            self.recent.pop_front();
        }
        self.recent.push_back(tokens);
    }
}

fn similarity_tokens(hypothesis: &str) -> Vec<String> {
    token_spans(hypothesis)
        .into_iter()
        .take(MAX_SIMILARITY_TOKENS)
        .map(|(token, _, _)| normalized_token(token))
        .collect()
}

/// Whether twice the longest common token subsequence reaches
/// `OUTLIER_SIMILARITY` of both hypotheses' combined token count.
#[expect(
    clippy::cast_precision_loss,
    reason = "similarity reads at most MAX_SIMILARITY_TOKENS tokens per hypothesis"
)]
fn similar(left: &[String], right: &[String]) -> bool {
    let total = left.len() + right.len();
    total == 0 || (2 * common_subsequence(left, right)) as f64 >= OUTLIER_SIMILARITY * total as f64
}

fn common_subsequence(left: &[String], right: &[String]) -> usize {
    let mut previous = vec![0; right.len() + 1];
    let mut current = vec![0; right.len() + 1];
    for left_token in left {
        for (index, right_token) in right.iter().enumerate() {
            current[index + 1] = if left_token == right_token {
                previous[index] + 1
            } else {
                previous[index + 1].max(current[index])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// Length of the hypothesis prefix that disputed agreed text covers, or the
/// whole hypothesis when it ends before the band.
fn aligned_cut(agreed: &[TokenEvidence], hypothesis: &[String]) -> usize {
    let agreed = agreed
        .iter()
        .map(|token| token.normalized.as_str())
        .collect::<Vec<_>>();
    covering_prefix(&agreed, hypothesis).map_or(hypothesis.len(), |(_, length)| length)
}

/// Edit distance and length of the hypothesis prefix that covers `reference`:
/// the one within `CUT_TOKEN_BAND` of the reference token count with the
/// least banded edit distance to it, the longer on a tie, or `None` when the
/// hypothesis ends before the band.
pub(super) fn covering_prefix(reference: &[&str], hypothesis: &[String]) -> Option<(usize, usize)> {
    // Column `offset` of the row for `row` reference tokens holds the distance
    // to the hypothesis prefix of `row + offset - CUT_TOKEN_BAND` tokens.
    let prefix = |row: usize, offset: usize| {
        (row + offset)
            .checked_sub(CUT_TOKEN_BAND)
            .filter(|length| *length <= hypothesis.len())
    };
    let mut previous = [usize::MAX; CUT_BAND_WIDTH];
    for (offset, cell) in previous.iter_mut().enumerate() {
        if let Some(length) = prefix(0, offset) {
            *cell = length;
        }
    }
    for (row, token) in (1..).zip(reference) {
        let mut current = [usize::MAX; CUT_BAND_WIDTH];
        for offset in 0..CUT_BAND_WIDTH {
            let Some(length) = prefix(row, offset) else {
                continue;
            };
            let deletion = previous
                .get(offset + 1)
                .map_or(usize::MAX, |cell| cell.saturating_add(1));
            let insertion = offset
                .checked_sub(1)
                .map_or(usize::MAX, |left| current[left].saturating_add(1));
            let substitution = length.checked_sub(1).map_or(usize::MAX, |last| {
                previous[offset].saturating_add(usize::from(*token != hypothesis[last]))
            });
            current[offset] = deletion.min(insertion).min(substitution);
        }
        previous = current;
    }
    (0..CUT_BAND_WIDTH)
        .filter_map(|offset| {
            prefix(reference.len(), offset).map(|length| (previous[offset], length))
        })
        .min_by(|left, right| left.0.cmp(&right.0).then(right.1.cmp(&left.1)))
}
