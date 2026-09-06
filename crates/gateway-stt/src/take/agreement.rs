use super::text::append_transcript;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct AgreementSnapshot {
    pub(super) agreed: String,
    pub(super) tentative: String,
}

#[derive(Debug, Default)]
pub(super) struct LocalAgreement {
    previous: String,
}

impl LocalAgreement {
    pub(super) fn observe(&mut self, hypothesis: &str) -> AgreementSnapshot {
        let agreed_end = if self.previous.is_empty() {
            0
        } else {
            matching_token_prefix_end(&self.previous, hypothesis)
        };
        self.previous.clear();
        self.previous.push_str(hypothesis);
        AgreementSnapshot {
            agreed: hypothesis[..agreed_end].to_owned(),
            tentative: hypothesis[agreed_end..].to_owned(),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct InterimState {
    agreement: LocalAgreement,
    promoted: String,
    agreement_finalized: String,
    committed: String,
    last_committed: String,
    last_tentative: String,
    finalized_at_last_speech: String,
}

impl InterimState {
    pub(super) fn next(&mut self, finalized: &str, hypothesis: &str) -> Option<(String, String)> {
        if self.agreement_finalized != finalized {
            let finalized_delta = finalized
                .strip_prefix(&self.agreement_finalized)
                .unwrap_or_default();
            let unpromoted = after_token_prefix(finalized_delta, token_spans(&self.promoted).len());
            append_transcript(&mut self.committed, unpromoted.trim());
            self.agreement = LocalAgreement::default();
            self.promoted.clear();
            self.agreement_finalized.clear();
            self.agreement_finalized.push_str(finalized);
        }
        let suffix_start = matching_token_prefix_end(&self.promoted, hypothesis);
        let suffix = hypothesis[suffix_start..].trim_start();
        let agreement = self.agreement.observe(suffix);
        let tentative = agreement.tentative.trim_start().to_owned();
        self.agreement.previous.clone_from(&tentative);
        let promoted = agreement.agreed.trim();
        append_transcript(&mut self.promoted, promoted);
        append_transcript(&mut self.committed, promoted);
        if !hypothesis.is_empty() {
            self.finalized_at_last_speech.clear();
            self.finalized_at_last_speech.push_str(finalized);
        } else if finalized.len() <= self.finalized_at_last_speech.len() {
            return None;
        }
        let committed = self.committed.clone();
        if committed == self.last_committed && tentative == self.last_tentative {
            return None;
        }
        self.last_committed.clone_from(&committed);
        self.last_tentative.clone_from(&tentative);
        Some((committed, tentative))
    }
}

fn matching_token_prefix_end(previous: &str, current: &str) -> usize {
    let previous = token_spans(previous);
    let current = token_spans(current);
    previous
        .iter()
        .zip(&current)
        .take_while(|((left, _, _), (right, _, _))| left == right)
        .map(|(_, (_, _, end))| *end)
        .last()
        .unwrap_or(0)
}

fn token_spans(text: &str) -> Vec<(&str, usize, usize)> {
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

fn after_token_prefix(text: &str, tokens: usize) -> &str {
    if tokens == 0 {
        return text;
    }
    token_spans(text)
        .get(tokens - 1)
        .map_or("", |(_, _, end)| &text[*end..])
}
