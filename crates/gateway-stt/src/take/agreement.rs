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

pub(super) fn matching_suffix_prefix_start(previous: &str, current: &str) -> Option<usize> {
    let previous = token_spans(previous);
    let current = token_spans(current);
    for overlap in (1..=previous.len().min(current.len())).rev() {
        let previous_start = previous.len() - overlap;
        if previous[previous_start..]
            .iter()
            .map(|(token, _, _)| *token)
            .zip(current[..overlap].iter().map(|(token, _, _)| *token))
            .all(|(previous, current)| equivalent_token(previous, current))
        {
            return Some(previous[previous_start].1);
        }
    }
    None
}

pub(super) fn equivalent_token(left: &str, right: &str) -> bool {
    left.chars().any(char::is_alphanumeric)
        && left
            .chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .eq(right
                .chars()
                .filter(|character| character.is_alphanumeric())
                .flat_map(char::to_lowercase))
}

#[cfg(test)]
mod tests {
    use super::{equivalent_token, matching_suffix_prefix_start};

    #[test]
    fn punctuation_only_tokens_never_establish_overlap() {
        assert!(!equivalent_token("...", "?!"));
        assert_eq!(
            matching_suffix_prefix_start("canonical ...", "?! replacement"),
            None
        );
    }
}
