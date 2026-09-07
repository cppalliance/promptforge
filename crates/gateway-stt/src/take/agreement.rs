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

    pub(super) fn retain_tentative(&mut self, tentative: &str) {
        self.previous.clear();
        self.previous.push_str(tentative);
    }
}

pub(super) fn matching_token_prefix_end(previous: &str, current: &str) -> usize {
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

pub(super) fn token_spans(text: &str) -> Vec<(&str, usize, usize)> {
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
