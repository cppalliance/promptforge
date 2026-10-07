//! Speech-sandbox metrics over one take's interim snapshots and final transcript.

use std::collections::BTreeMap;

use gateway_stt::test_fixtures::{ReplayFinal, ReplaySnapshot};
use serde::{Deserialize, Serialize};

/// One take's metrics plus the sums and counts that aggregate them across takes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Metrics {
    pub(super) upwr: f64,
    pub(super) upsr: f64,
    pub(super) partial_latency_ms: f64,
    pub(super) commit_lag_ms: f64,
    pub(super) agreed_shrink_events: usize,
    pub(super) changed_words: usize,
    pub(super) completed_words: usize,
    pub(super) changed_pairs: usize,
    pub(super) pairs: usize,
    pub(super) latency_sum_ms: f64,
    pub(super) latency_words: usize,
    pub(super) lag_sum_ms: f64,
    pub(super) lag_words: usize,
}

const SAMPLES_PER_MS: f64 = 16.0;

#[derive(Default)]
struct Mean {
    sum: f64,
    count: usize,
}

impl Mean {
    fn add(&mut self, value: f64) {
        self.sum += value;
        self.count += 1;
    }
}

/// Computes the metrics of one take whose sequence is its snapshots'
/// transcripts followed by `completed`, with `finals` in take order.
pub(super) fn compute(
    snapshots: &[ReplaySnapshot],
    completed: &str,
    finals: &[ReplayFinal],
) -> Metrics {
    let sequence = snapshots
        .iter()
        .map(|snapshot| snapshot.transcript.as_str())
        .chain([completed])
        .collect::<Vec<_>>();
    let counts = sequence
        .windows(2)
        .map(|pair| changed_words(pair[0], pair[1]))
        .collect::<Vec<_>>();
    let changed = counts.iter().sum();
    let changed_pairs = counts.iter().filter(|count| **count > 0).count();
    let completed_words = words(completed).len();

    let mut latency = Mean::default();
    let mut lag = Mean::default();
    for position in 0..completed_words {
        let first = snapshots
            .iter()
            .find(|snapshot| words(&snapshot.transcript).len() > position)
            .map(|snapshot| millis(snapshot.at_ms));
        let stable = snapshots
            .iter()
            .find(|snapshot| stable_words(snapshot) > position)
            .map(|snapshot| millis(snapshot.at_ms));
        if let (Some(first), Some(end)) = (first, audio_end_ms(finals, position)) {
            latency.add(first - end);
        }
        if let (Some(first), Some(stable)) = (first, stable) {
            lag.add(stable - first);
        }
    }

    Metrics {
        upwr: ratio(changed, completed_words),
        upsr: ratio(changed_pairs, counts.len()),
        partial_latency_ms: mean(latency.sum, latency.count),
        commit_lag_ms: mean(lag.sum, lag.count),
        agreed_shrink_events: agreed_shrink_events(snapshots),
        changed_words: changed,
        completed_words,
        changed_pairs,
        pairs: counts.len(),
        latency_sum_ms: latency.sum,
        latency_words: latency.count,
        lag_sum_ms: lag.sum,
        lag_words: lag.count,
    }
}

/// Lists every metric in `current` that breaks its threshold against `baseline`.
pub(super) fn threshold_violations(
    current: &BTreeMap<String, Metrics>,
    baseline: &BTreeMap<String, Metrics>,
) -> Vec<String> {
    let mut violations = Vec::new();
    let mut compared = Vec::new();
    for (name, metrics) in current {
        let Some(base) = baseline.get(name) else {
            violations.push(format!("{name} has no baseline section"));
            continue;
        };
        compared.push((metrics, base));
        if metrics.changed_words * base.completed_words
            > base.changed_words * metrics.completed_words
        {
            violations.push(format!(
                "{name} UPWR {} is above its baseline {}",
                metrics.upwr, base.upwr
            ));
        }
        if metrics.agreed_shrink_events > base.agreed_shrink_events {
            violations.push(format!(
                "{name} has {} agreed-shrink events, above its baseline {}",
                metrics.agreed_shrink_events, base.agreed_shrink_events
            ));
        }
    }
    violations.extend(aggregate_violation(
        "partial latency",
        &compared,
        |metrics| (metrics.latency_sum_ms, metrics.latency_words),
    ));
    violations.extend(aggregate_violation("commit lag", &compared, |metrics| {
        (metrics.lag_sum_ms, metrics.lag_words)
    }));
    violations
}

/// Compares the mean over all words of all compared takes with 110 percent of
/// the same mean over their baselines.
fn aggregate_violation(
    label: &str,
    compared: &[(&Metrics, &Metrics)],
    total: impl Fn(&Metrics) -> (f64, usize),
) -> Option<String> {
    let (current_sum, current_count) = sum(compared.iter().map(|(metrics, _)| total(metrics)));
    let (base_sum, base_count) = sum(compared.iter().map(|(_, base)| total(base)));
    if base_count == 0 {
        return None;
    }
    let base = mean(base_sum, base_count);
    let actual = mean(current_sum, current_count);
    (actual > base + base.abs() / 10.0).then(|| {
        format!("aggregate {label} {actual} ms is above 110 percent of its baseline {base} ms")
    })
}

fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

fn changed_words(earlier: &str, later: &str) -> usize {
    let earlier = words(earlier);
    let later = words(later);
    let common = earlier
        .iter()
        .zip(&later)
        .take_while(|(left, right)| left == right)
        .count();
    earlier.len() - common
}

fn stable_words(snapshot: &ReplaySnapshot) -> usize {
    words(&format!("{}{}", snapshot.finalized, snapshot.agreed)).len()
}

fn agreed_shrink_events(snapshots: &[ReplaySnapshot]) -> usize {
    snapshots
        .windows(2)
        .filter(|pair| {
            pair[1].finalized == pair[0].finalized
                && !words(&pair[1].agreed).starts_with(&words(&pair[0].agreed))
        })
        .count()
}

/// Interpolates the audio end of `position` within the final that covers it:
/// the j-th of n words of a final over [s, e] ends at s + (e - s) * j / n.
#[expect(
    clippy::cast_precision_loss,
    reason = "fixture sample positions and word counts are far below 2^53"
)]
fn audio_end_ms(finals: &[ReplayFinal], position: usize) -> Option<f64> {
    let mut covered = 0;
    for step in finals {
        let count = words(&step.text).len();
        if position < covered + count {
            let start = step.sample_start as f64 / SAMPLES_PER_MS;
            let end = step.sample_end as f64 / SAMPLES_PER_MS;
            let ordinal = (position - covered + 1) as f64;
            return Some(start + (end - start) * ordinal / count as f64);
        }
        covered += count;
    }
    None
}

fn sum(totals: impl Iterator<Item = (f64, usize)>) -> (f64, usize) {
    totals.fold((0.0, 0), |(sum, count), (more, words)| {
        (sum + more, count + words)
    })
}

#[expect(
    clippy::cast_precision_loss,
    reason = "fixture times are far below 2^53 milliseconds"
)]
fn millis(at_ms: u64) -> f64 {
    at_ms as f64
}

#[expect(
    clippy::cast_precision_loss,
    reason = "fixture word and pair counts are far below 2^53"
)]
fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "fixture word counts are far below 2^53"
)]
fn mean(sum: f64, count: usize) -> f64 {
    if count == 0 { 0.0 } else { sum / count as f64 }
}

fn snapshot(at_ms: u64, finalized: &str, agreed: &str, tentative: &str) -> ReplaySnapshot {
    ReplaySnapshot {
        at_ms,
        revision: 0,
        transcript: format!("{finalized}{agreed}{tentative}"),
        finalized: finalized.to_owned(),
        agreed: agreed.to_owned(),
        tentative: tentative.to_owned(),
        audio_start_ms: 0,
        audio_end_ms: 0,
    }
}

fn final_text(sample_start: u64, sample_end: u64, text: &str) -> ReplayFinal {
    ReplayFinal {
        at_ms: 0,
        sample_start,
        sample_end,
        text: text.to_owned(),
    }
}

fn assert_exact(actual: f64, expected: f64, metric: &str) {
    assert_eq!(
        actual.to_bits(),
        expected.to_bits(),
        "{metric} is {actual}, expected {expected}"
    );
}

#[test]
fn upwr_and_upsr_count_earlier_words_after_the_exact_common_prefix() {
    let snapshots = [
        snapshot(1_000, "", "", "ask not"),
        snapshot(1_500, "", "", "Ask not what"),
        snapshot(2_000, "", "Ask", " not, what your"),
    ];
    let metrics = compute(&snapshots, "Ask not, what your country", &[]);

    assert_eq!(
        metrics.changed_words, 4,
        "a case flip and a comma flip count"
    );
    assert_eq!(metrics.completed_words, 5);
    assert_eq!(metrics.changed_pairs, 2);
    assert_eq!(metrics.pairs, 3, "completed is the sequence's last element");
    assert_exact(metrics.upwr, 4.0 / 5.0, "UPWR");
    assert_exact(metrics.upsr, 2.0 / 3.0, "UPSR");
}

#[test]
fn partial_latency_and_commit_lag_follow_word_positions() {
    let snapshots = [
        snapshot(2_200, "", "", "one two"),
        snapshot(3_100, "", "one two", " three"),
        snapshot(4_600, "", "one two three", " four"),
    ];
    let finals = [final_text(0, 64_000, "one two three four")];
    let metrics = compute(&snapshots, "one two three four", &finals);

    assert_eq!(metrics.latency_words, 4);
    assert_exact(
        metrics.latency_sum_ms,
        1_200.0 + 200.0 + 100.0 + 600.0,
        "latency sum",
    );
    assert_exact(metrics.partial_latency_ms, 525.0, "partial latency");
    assert_eq!(
        metrics.lag_words, 3,
        "the fourth word never becomes stable before completed"
    );
    assert_exact(metrics.lag_sum_ms, 900.0 + 900.0 + 1_500.0, "lag sum");
    assert_exact(metrics.commit_lag_ms, 1_100.0, "commit lag");
}

#[test]
fn audio_end_interpolates_within_the_final_that_covers_each_position() {
    let snapshots = [snapshot(
        7_000,
        "",
        "",
        "alpha beta gamma delta epsilon zeta",
    )];
    let finals = [
        final_text(0, 32_000, "alpha beta"),
        final_text(48_000, 96_000, "gamma delta epsilon"),
    ];
    let metrics = compute(&snapshots, "alpha beta gamma delta epsilon zeta", &finals);

    assert_eq!(
        metrics.latency_words, 5,
        "a word no final covers has no audio end"
    );
    assert_exact(
        metrics.latency_sum_ms,
        6_000.0 + 5_000.0 + 3_000.0 + 2_000.0 + 1_000.0,
        "latency sum",
    );
    assert_exact(metrics.partial_latency_ms, 3_400.0, "partial latency");
}

#[test]
fn agreed_shrink_counts_only_snapshots_whose_finalized_text_is_unchanged() {
    let snapshots = [
        snapshot(1_000, "", "Why is it", ""),
        snapshot(1_500, "", "Why is", " this"),
        snapshot(2_000, "", "Why is this", ""),
        snapshot(2_500, "Why is this", "", " the"),
        snapshot(3_000, "Why is this", " the", ""),
        snapshot(3_500, "Why is this", " a", " way"),
    ];
    let metrics = compute(&snapshots, "Why is this a way", &[]);

    assert_eq!(metrics.agreed_shrink_events, 2);
}

fn counted(changed: usize, completed: usize, latency: f64, lag: f64, shrink: usize) -> Metrics {
    Metrics {
        changed_words: changed,
        completed_words: completed,
        latency_sum_ms: latency,
        latency_words: 10,
        lag_sum_ms: lag,
        lag_words: 10,
        agreed_shrink_events: shrink,
        ..Metrics::default()
    }
}

#[test]
fn thresholds_flag_upwr_and_shrink_above_baseline_and_latency_beyond_a_tenth() {
    let baseline = BTreeMap::from([("scripted-a".to_owned(), counted(2, 10, 1_000.0, 500.0, 1))]);
    let check = |metrics: Metrics| {
        threshold_violations(
            &BTreeMap::from([("scripted-a".to_owned(), metrics)]),
            &baseline,
        )
    };

    assert!(check(counted(2, 10, 1_000.0, 500.0, 1)).is_empty());
    assert!(
        check(counted(1, 10, 1_100.0, 550.0, 0)).is_empty(),
        "ten percent above the latency and lag baselines still passes"
    );
    assert_eq!(
        check(counted(3, 10, 1_000.0, 500.0, 1)).len(),
        1,
        "UPWR rises"
    );
    assert_eq!(check(counted(2, 10, 1_101.0, 500.0, 1)).len(), 1, "latency");
    assert_eq!(
        check(counted(2, 10, 1_000.0, 551.0, 1)).len(),
        1,
        "commit lag"
    );
    assert_eq!(
        check(counted(2, 10, 1_000.0, 500.0, 2)).len(),
        1,
        "shrink rises"
    );
    assert_eq!(
        threshold_violations(
            &BTreeMap::from([("scripted-b".to_owned(), counted(2, 10, 1_000.0, 500.0, 1))]),
            &baseline,
        )
        .len(),
        1,
        "a fixture without a baseline section fails"
    );
}
