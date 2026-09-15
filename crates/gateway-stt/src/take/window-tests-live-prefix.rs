use super::super::WholeWindowState;
use crate::take::live_prefix::LivePrefixSnapshot;

fn prefix(
    finalized: &str,
    finalized_samples: u64,
    pending: Option<(&str, std::ops::Range<u64>)>,
) -> LivePrefixSnapshot {
    LivePrefixSnapshot::for_test(finalized, finalized_samples, pending)
}

#[test]
fn pending_forced_middle_replaces_covered_active_text_without_a_gap() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 64_000, "superseded tail");

    let first = state
        .try_next(
            &prefix("", 0, Some(("word0000 word0001", 0..160_000))),
            160_000,
            160_000,
            176_000,
            "word0010",
        )
        .expect("live ownership remains within capacity")
        .expect("the pending middle and active tail emit together");
    assert_eq!(
        first.into_parts(),
        (
            "word0000 word0001 word0010".to_owned(),
            String::new(),
            "word0000 word0001".to_owned(),
            " word0010".to_owned(),
        )
    );

    let revised = state
        .try_next(
            &prefix(
                "word0000",
                16_000,
                Some(("word0001 revised word0019", 16_000..320_000)),
            ),
            320_000,
            320_000,
            336_000,
            "word0020",
        )
        .expect("revised ownership remains within capacity")
        .expect("one snapshot replaces the prior pending middle");
    assert_eq!(
        revised.into_parts(),
        (
            "word0000 word0001 revised word0019 word0020".to_owned(),
            "word0000".to_owned(),
            " word0001 revised word0019".to_owned(),
            " word0020".to_owned(),
        )
    );
}

#[test]
fn repeated_pending_phrase_appears_once_per_absolute_range() {
    let mut state = WholeWindowState::default();
    let snapshot = state
        .try_next(
            &prefix(
                "opening",
                16_000,
                Some(("echo now echo now", 16_000..160_000)),
            ),
            160_000,
            160_000,
            176_000,
            "echo now",
        )
        .expect("repeated regions remain within capacity")
        .expect("repeated phrases at distinct ranges emit");

    assert_eq!(
        snapshot.into_parts().0,
        "opening echo now echo now echo now"
    );
}

#[test]
fn every_forced_stride_keeps_the_complete_visible_prefix() {
    let mut state = WholeWindowState::default();
    for stride in 0..4_u64 {
        let pending_end = stride * 160_000;
        let pending = (stride > 0).then(|| {
            (
                format!("middle through stride {stride}"),
                (pending_end - 160_000)..pending_end,
            )
        });
        let owned_pending = pending
            .as_ref()
            .map(|(text, range)| (text.as_str(), range.clone()));
        let expected = if stride == 0 {
            format!("settled tail stride {stride}")
        } else {
            format!("settled middle through stride {stride} tail stride {stride}")
        };
        let snapshot = state
            .try_next(
                &prefix("settled", 1, owned_pending),
                pending_end,
                pending_end,
                pending_end + 16_000,
                &format!("tail stride {stride}"),
            )
            .expect("stride remains within capacity")
            .expect("each stride emits one complete snapshot");

        assert_eq!(snapshot.into_parts().0, expected);
    }
}
