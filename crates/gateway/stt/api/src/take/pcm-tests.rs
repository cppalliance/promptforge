//! Tests for the retained PCM budget and rolling buffer accounting.

use super::{RetainedPcm, RetainedPcmBudget, RollingPcm};
use crate::audio::AudioError;

#[test]
fn miri_resident_queue_and_decode_charge_each_live_allocation() {
    let budget = RetainedPcmBudget::with_limit(14);
    let mut resident = RollingPcm::new(budget.clone());
    resident
        .append(vec![0.0; 10])
        .expect("resident PCM reserves");
    assert_eq!(budget.retained_samples(), 10);

    let queued = resident
        .transfer_range(2..6)
        .expect("the queued range transfers before compaction");
    assert_eq!(resident.origin(), 6);
    assert_eq!(resident.len(), 4);
    assert_eq!(queued.len(), 4);
    assert_eq!(budget.retained_samples(), 14);

    let (samples, decoding) = queued.into_decode();
    assert_eq!(samples.len(), 4);
    assert_eq!(decoding.samples(), 10);
    assert_eq!(budget.retained_samples(), 14);
    drop(samples);
    drop(decoding);
    assert_eq!(budget.retained_samples(), 4);
}

#[test]
fn aggregate_limit_counts_resident_and_copied_interim_pcm() {
    let budget = RetainedPcmBudget::with_limit(8);
    let mut resident = RollingPcm::new(budget.clone());
    resident
        .append(vec![0.0; 6])
        .expect("resident PCM reserves");
    let interim = resident
        .copy_range(2..4)
        .expect("interim PCM reserves independently");
    assert_eq!(budget.retained_samples(), 8);
    assert!(resident.copy_range(0..1).is_err());
    drop(interim);
    assert_eq!(budget.retained_samples(), 6);
}

#[test]
fn multiple_compactions_keep_absolute_origins() {
    let budget = RetainedPcmBudget::with_limit(20);
    let mut resident = RollingPcm::new(budget);
    resident
        .append((0_u8..12).map(f32::from).collect())
        .expect("resident PCM reserves");

    let first = resident
        .transfer_range(2..4)
        .expect("first range transfers");
    assert_eq!(first.samples(), &[2.0, 3.0]);
    drop(first);
    resident.append(vec![12.0, 13.0]).expect("PCM appends");
    let second = resident
        .transfer_range(8..12)
        .expect("second absolute range transfers");
    assert_eq!(second.samples(), &[8.0, 9.0, 10.0, 11.0]);
    assert_eq!(resident.origin(), 12);
    assert_eq!(resident.samples(), &[12.0, 13.0]);
}

#[test]
fn a_tail_too_large_for_the_headroom_transfers_by_copying_the_range() {
    let budget = RetainedPcmBudget::with_limit(14);
    let mut resident = RollingPcm::new(budget.clone());
    resident
        .append((0_u8..10).map(f32::from).collect())
        .expect("resident PCM reserves");

    let front = resident
        .transfer_range(0..2)
        .expect("the range fits the headroom its tail does not");
    assert_eq!(front.samples(), &[0.0, 1.0]);
    assert_eq!(resident.origin(), 2);
    assert_eq!(
        resident.samples(),
        (2_u8..10).map(f32::from).collect::<Vec<_>>()
    );
    assert_eq!(budget.retained_samples(), 12);
    drop(front);
    assert_eq!(budget.retained_samples(), 10);
}

#[test]
fn miri_releasing_a_prefix_at_the_cap_frees_its_budget_in_place() {
    let budget = RetainedPcmBudget::with_limit(12);
    let mut resident = RollingPcm::new(budget.clone());
    resident
        .append((0_u8..12).map(f32::from).collect())
        .expect("resident PCM fills the cap");
    let (_, rejected) = resident
        .try_append(vec![12.0; 2])
        .expect_err("the full cap rejects new audio");
    assert_eq!(rejected, [12.0; 2], "the rejected samples come back");

    resident.release_prefix(8).expect("the prefix is resident");
    assert_eq!(resident.origin(), 8);
    assert_eq!(resident.samples(), &[8.0, 9.0, 10.0, 11.0]);
    assert_eq!(budget.retained_samples(), 4);
    resident
        .try_append(rejected)
        .expect("the freed budget admits the rejected samples");
}

#[test]
fn miri_eighteen_second_decode_and_ten_second_resident_count_allocation_capacity() {
    let budget = RetainedPcmBudget::with_limit(30);
    let mut resident = RollingPcm::new(budget.clone());
    let mut first_window = Vec::with_capacity(18);
    first_window.extend((0_u8..18).map(f32::from));
    resident
        .append(first_window)
        .expect("the 18-sample forced window reserves");
    let forced = resident
        .transfer_range(0..18)
        .expect("the forced window transfers before compaction");
    let (samples, owner) = forced.into_decode();
    let mut next_stride = Vec::with_capacity(10);
    next_stride.extend([18.0; 10]);
    resident
        .append(next_stride)
        .expect("the next ten samples fit while 18 decode");
    assert_eq!(samples.capacity(), 18);
    assert_eq!(budget.retained_samples(), 28);

    let overlap = RetainedPcm::retain_tail(samples, owner, 8);
    resident
        .restore_prefix(10..18, overlap)
        .expect("the retired decode returns its overlap");

    assert_eq!(resident.origin(), 10);
    assert_eq!(
        resident.samples(),
        [
            (10_u8..18).map(f32::from).collect::<Vec<_>>(),
            vec![18.0; 10]
        ]
        .concat()
    );
    assert_eq!(
        budget.retained_samples(),
        18,
        "returning overlap reuses the decode allocation and releases the resident allocation"
    );
}

#[test]
fn allocation_slack_is_charged_once_and_admits_samples_without_a_second_charge() {
    let budget = RetainedPcmBudget::with_limit(30);
    let mut resident = RollingPcm::new(budget.clone());
    let mut decode_allocation = Vec::with_capacity(19);
    decode_allocation.extend([0.5; 18]);
    resident
        .append(decode_allocation)
        .expect("the allocation capacity fits");
    let decode = resident
        .transfer_range(0..18)
        .expect("the allocation transfers");
    let (_samples, _owner) = decode.into_decode();

    let mut next_stride = Vec::with_capacity(11);
    next_stride.extend([0.5; 10]);
    resident
        .append(next_stride)
        .expect("the exact remaining allocation fits");
    assert_eq!(budget.retained_samples(), 30);
    resident
        .append(vec![0.5])
        .expect("the destination's spare capacity is already counted");
    assert_eq!(resident.len(), 11);
    assert_eq!(budget.retained_samples(), 30, "the append charges nothing");
    assert!(
        resident.append(vec![0.5]).is_err(),
        "growth past the spare capacity needs headroom the cap no longer has"
    );
}

#[test]
fn a_rejected_pcm_append_names_the_retained_and_requested_durations_and_the_limit() {
    const SECOND: usize = 16_000;
    let mut resident = RollingPcm::new(RetainedPcmBudget::default());
    resident
        .append(vec![0.0; 20 * SECOND])
        .expect("twenty seconds fit the cap");

    let (error, rejected) = resident
        .try_append(vec![0.0; 11 * SECOND])
        .expect_err("eleven more seconds pass the thirty-second cap");

    assert_eq!(
        error,
        AudioError::BufferTooLong {
            maximum_seconds: 30,
            retained_ms: 20_000,
            requested_ms: 11_000,
        }
    );
    assert_eq!(
        error.to_string(),
        "audio buffer exceeds 30 seconds: 20000 ms retained, 11000 ms requested"
    );
    assert_eq!(
        rejected.len(),
        11 * SECOND,
        "the rejected samples come back"
    );
    assert_eq!(resident.len(), 20 * SECOND);
}

#[test]
fn twenty_seconds_retained_admit_a_six_second_append() {
    const SECOND: usize = 16_000;
    let budget = RetainedPcmBudget::default();
    let mut resident = RollingPcm::new(budget.clone());
    resident
        .append(vec![0.0; 20 * SECOND])
        .expect("twenty seconds fit the cap");

    resident
        .append(vec![0.0; 6 * SECOND])
        .expect("the batch counts once, so six more seconds fit the thirty-second cap");
    assert_eq!(resident.len(), 26 * SECOND);
    assert_eq!(
        budget.retained_samples(),
        26 * SECOND,
        "the append leaves exactly the buffer's own allocation counted"
    );
}

#[test]
fn destination_growth_obeys_the_exact_headroom_budget() {
    fn append_with_limit(limit: usize) -> Result<usize, AudioError> {
        let budget = RetainedPcmBudget::with_limit(limit);
        let mut resident = RollingPcm::new(budget.clone());
        let mut first = Vec::with_capacity(2);
        first.extend([1.0, 2.0]);
        resident.append(first)?;
        let mut second = Vec::with_capacity(2);
        second.extend([3.0, 4.0]);
        resident.append(second)?;
        Ok(budget.retained_samples())
    }

    assert_eq!(
        append_with_limit(4).expect("old destination and growth fit exactly"),
        4
    );
    assert!(
        append_with_limit(3).is_err(),
        "one sample below the destination's growth rejects before growth and copy"
    );
}
