use super::agreement::{matching_token_prefix_end, token_spans};
use super::interim::InterimSnapshot;
use super::text::append_transcript;

#[derive(Debug)]
struct PendingRegion {
    end: usize,
    text: String,
}

#[derive(Debug, Default)]
pub(super) struct WholeWindowState {
    segment_start: usize,
    window_start: Option<usize>,
    active: String,
    pending: Vec<PendingRegion>,
    last: Option<InterimSnapshot>,
}

impl WholeWindowState {
    pub(super) fn next(
        &mut self,
        finalized: &str,
        finalized_samples: usize,
        segment_start: usize,
        window_start: usize,
        _window_end: usize,
        hypothesis: &str,
    ) -> Option<InterimSnapshot> {
        self.pending.retain(|region| region.end > finalized_samples);
        if self.window_start.is_some() && self.segment_start != segment_start {
            if !self.active.is_empty() {
                self.pending.push(PendingRegion {
                    end: segment_start,
                    text: std::mem::take(&mut self.active),
                });
            }
            self.window_start = None;
        }
        self.segment_start = segment_start;

        let replacement = match self.window_start {
            Some(previous_start) if window_start > previous_start => {
                rebase_sliding_window(&self.active, hypothesis)
            }
            Some(_) | None => hypothesis.to_owned(),
        };
        let agreed_end = if self.active.is_empty() {
            0
        } else {
            matching_token_prefix_end(&self.active, &replacement)
        };
        self.active = replacement;
        self.window_start = Some(window_start);

        let mut agreed = String::new();
        for region in &self.pending {
            append_transcript(&mut agreed, &region.text);
        }
        append_transcript(&mut agreed, self.active[..agreed_end].trim());
        let agreed = owned_piece(!finalized.is_empty(), &agreed);
        let tentative = owned_piece(
            !finalized.is_empty() || !agreed.is_empty(),
            &self.active[agreed_end..],
        );
        let snapshot = InterimSnapshot::new(finalized.to_owned(), agreed, tentative);
        if self.last.as_ref() == Some(&snapshot) {
            return (!hypothesis.is_empty()).then_some(snapshot);
        }
        self.last = Some(snapshot.clone());
        Some(snapshot)
    }
}

fn rebase_sliding_window(previous: &str, current: &str) -> String {
    let previous_tokens = token_spans(previous);
    let current_tokens = token_spans(current);
    for overlap in (1..=previous_tokens.len().min(current_tokens.len())).rev() {
        let previous_start = previous_tokens.len() - overlap;
        if previous_tokens[previous_start..]
            .iter()
            .map(|(token, _, _)| *token)
            .zip(current_tokens[..overlap].iter().map(|(token, _, _)| *token))
            .all(|(previous, current)| equivalent_token(previous, current))
        {
            let mut rebased = previous[..previous_tokens[previous_start].1]
                .trim_end()
                .to_owned();
            append_transcript(&mut rebased, current);
            return rebased;
        }
    }
    current.to_owned()
}

fn equivalent_token(left: &str, right: &str) -> bool {
    left.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .eq(right
            .chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase))
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
    use super::WholeWindowState;

    #[test]
    fn whole_window_revision_replaces_a_promoted_leading_phrase() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 8_000, "Why is it");
        state.next("", 0, 0, 0, 9_600, "Why is it");
        let snapshot = state
            .next("", 0, 0, 0, 11_200, "Why is this")
            .expect("a revised whole-window hypothesis emits");

        assert_eq!(
            snapshot.into_parts(),
            (
                "Why is this".to_owned(),
                String::new(),
                "Why is".to_owned(),
                " this".to_owned(),
            )
        );
    }

    #[test]
    fn consumed_boundary_starts_a_region_before_finalization_arrives() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 16_000, "first segment");
        state.next("", 0, 0, 0, 16_000, "first segment");
        let pending = state
            .next("", 0, 16_000, 16_000, 24_000, "second start")
            .expect("the new segment starts without waiting for final text");
        assert_eq!(pending.into_parts().0, "first segment second start");

        let authoritative = state
            .next(
                "revised first",
                16_000,
                16_000,
                16_000,
                25_600,
                "second start now",
            )
            .expect("authoritative text replaces the pending segment");
        assert_eq!(
            authoritative.into_parts().0,
            "revised first second start now"
        );
    }

    #[test]
    fn advancing_window_rebases_through_overlap_without_repeating_it() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 16_000, "ask not what your country can do");
        let snapshot = state
            .next("", 0, 0, 8_000, 24_000, "your country can do for you")
            .expect("the sliding window emits a rebased hypothesis");

        assert_eq!(
            snapshot.into_parts().0,
            "ask not what your country can do for you"
        );
    }

    #[test]
    fn sliding_overlap_tolerates_native_punctuation_revision() {
        let mut state = WholeWindowState::default();
        state.next("", 0, 0, 0, 64_000, "And so my fellow Americans, ask");
        let snapshot = state
            .next("", 0, 0, 16_000, 80_000, "my fellow Americans ask not")
            .expect("the punctuated native overlap rebases");

        assert_eq!(
            snapshot.into_parts().0,
            "And so my fellow Americans ask not"
        );
    }
}
