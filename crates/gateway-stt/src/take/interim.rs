use super::agreement::{LocalAgreement, matching_token_prefix_end, token_spans};
use super::text::append_transcript;
use super::{Take, TakeState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InterimSnapshot {
    transcript: String,
    finalized: String,
    agreed: String,
    tentative: String,
}

impl InterimSnapshot {
    fn new(finalized: String, agreed: String, tentative: String) -> Self {
        let transcript = format!("{finalized}{agreed}{tentative}");
        Self {
            transcript,
            finalized,
            agreed,
            tentative,
        }
    }

    pub(crate) fn committed(&self) -> &str {
        &self.transcript[..self.finalized.len() + self.agreed.len()]
    }

    pub(crate) fn into_parts(self) -> (String, String, String, String) {
        (self.transcript, self.finalized, self.agreed, self.tentative)
    }

    pub(crate) fn into_legacy_parts(self) -> (String, String) {
        (
            self.committed().to_owned(),
            self.tentative.trim_start().to_owned(),
        )
    }
}

#[derive(Debug, Default)]
pub(super) struct InterimState {
    agreement: LocalAgreement,
    promoted: String,
    agreement_finalized: String,
    finalized: String,
    last: Option<InterimSnapshot>,
    finalized_at_last_speech: String,
}

impl InterimState {
    pub(super) fn next(&mut self, finalized: &str, hypothesis: &str) -> Option<InterimSnapshot> {
        if self.agreement_finalized != finalized {
            let finalized_delta = finalized
                .strip_prefix(&self.agreement_finalized)
                .unwrap_or_default();
            let unpromoted = after_token_prefix(finalized_delta, token_spans(&self.promoted).len());
            append_transcript(&mut self.finalized, &self.promoted);
            append_transcript(&mut self.finalized, unpromoted.trim());
            self.agreement = LocalAgreement::default();
            self.promoted.clear();
            self.agreement_finalized.clear();
            self.agreement_finalized.push_str(finalized);
        }
        let suffix_start = matching_token_prefix_end(&self.promoted, hypothesis);
        let suffix = &hypothesis[suffix_start..];
        let agreement = self.agreement.observe(suffix);
        let mut tentative = agreement.tentative;
        self.agreement.retain_tentative(&tentative);
        append_transcript(&mut self.promoted, agreement.agreed.trim());
        if !hypothesis.is_empty() {
            self.finalized_at_last_speech.clear();
            self.finalized_at_last_speech.push_str(finalized);
        } else if finalized.len() <= self.finalized_at_last_speech.len() {
            return None;
        }
        let agreed = owned_piece(!self.finalized.is_empty(), &self.promoted);
        if suffix_start == 0 && !self.finalized.is_empty() && self.promoted.is_empty() {
            tentative = owned_piece(true, &tentative);
        }
        let snapshot = InterimSnapshot::new(self.finalized.clone(), agreed, tentative);
        if self.last.as_ref() == Some(&snapshot) {
            return (!hypothesis.is_empty()).then_some(snapshot);
        }
        self.last = Some(snapshot.clone());
        Some(snapshot)
    }
}

impl Take {
    pub(crate) fn next_interim(&self, hypothesis: &str) -> Option<(String, String)> {
        self.next_interim_snapshot(hypothesis)
            .map(InterimSnapshot::into_legacy_parts)
    }

    pub(crate) fn next_interim_snapshot(&self, hypothesis: &str) -> Option<InterimSnapshot> {
        let finalized = self.finalized();
        TakeState::lock(&self.state.interim).next(&finalized, hypothesis)
    }
}

fn after_token_prefix(text: &str, tokens: usize) -> &str {
    if tokens == 0 {
        return text;
    }
    token_spans(text)
        .get(tokens - 1)
        .map_or("", |(_, _, end)| &text[*end..])
}

fn owned_piece(has_prefix: bool, piece: &str) -> String {
    if !has_prefix || piece.is_empty() || piece.starts_with(char::is_whitespace) {
        piece.to_owned()
    } else {
        format!(" {piece}")
    }
}

#[cfg(test)]
mod tests {
    use gateway_stt_engine::TranscribeError;

    use super::Take;

    #[test]
    fn snapshot_fields_own_disjoint_exact_text_after_divergent_finalization() {
        let take = Take::without_final(Vec::new());
        take.next_interim("ask not your country");
        take.next_interim("ask not your country");
        take.record_finalized(Ok::<_, TranscribeError>("ask not your kingdom".to_owned()));
        take.next_interim("new tail first");
        let snapshot = take
            .next_interim_snapshot("new tail second")
            .expect("new speech emits a partitioned snapshot");

        assert_eq!(
            snapshot.into_parts(),
            (
                "ask not your country new tail second".to_owned(),
                "ask not your country".to_owned(),
                " new tail".to_owned(),
                " second".to_owned()
            )
        );
    }
}
