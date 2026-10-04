//! Tests that inject filesystem failures into compaction and rotation and check recovery.

use super::super::compact::{compact_oversized_segment_with, recover_replacement};
use super::super::rotation::{
    cleanup_rotation_with, find_rotation_prepared, recover_rotation, rotation_committed_path,
    rotation_prepared_path, rotation_staged_path, rotation_targets,
};
use super::*;
use crate::config::LogConfig;

fn total_directory_file_bytes(directory: &Path) -> u64 {
    std::fs::read_dir(directory)
        .expect("read log directory")
        .map(|entry| {
            entry
                .expect("read log entry")
                .metadata()
                .expect("read log metadata")
                .len()
        })
        .sum()
}

fn observed_rotation_commit(current: &Path, fault: &FaultInjector) -> bool {
    rotation_committed_path(current).exists() || fault.commit_marker_written
}

fn interrupted_final_commit_cleanup(current: &Path, fault: &FaultInjector) -> bool {
    fault.failed_operation == Some("sync parent directory")
        && fault.commit_marker_written
        && !rotation_committed_path(current).exists()
        && find_rotation_prepared(current)
            .expect("inspect prepared rotation marker")
            .is_none()
}

#[test]
fn restart_compaction_recovers_every_injected_filesystem_failure() {
    let original = "old-prefix-".repeat(20) + "terminal diagnostic\n";
    let mut forced_replacement_gap = false;
    let mut completed = false;
    for (failures, fail_at) in (1..=32).enumerate() {
        let temp = TempStateDir::new("compaction-crash");
        let path = temp.0.join("gateway.log");
        std::fs::write(&path, &original).expect("seed oversized source");
        let mut fault = FaultInjector::crashing(fail_at);
        let result = compact_oversized_segment_with(
            &path,
            64,
            &mut fault,
            ReplacementMode::RemoveThenRename,
        );
        if result.is_ok() {
            completed = true;
            assert_eq!(
                failures, fault.calls,
                "the loop injected every staged replacement checkpoint independently"
            );
            break;
        }
        let operation = fault.assert_selected(fail_at, "replacement");
        if operation == "install replacement" {
            forced_replacement_gap = true;
            assert!(
                !path.exists() && artifact_path(&path, ".compact-backup").exists(),
                "the forced Windows replacement gap retains the original rollback copy"
            );
        }
        recover_replacement(&path).expect("restart recovers compaction");
        let recovered = std::fs::read_to_string(&path).expect("one complete copy survives");
        assert!(
            recovered == original
                || (recovered.starts_with(SEGMENT_TRUNCATION_MARKER)
                    && recovered.ends_with("terminal diagnostic\n")
                    && recovered.len() <= 64),
            "recovery keeps either the source or the complete durable replacement"
        );
    }
    assert!(
        forced_replacement_gap,
        "fault injection reaches the destructive Windows rename boundary"
    );
    assert!(
        completed,
        "the fault loop reaches the first non-failing run"
    );
}

#[test]
fn live_rotation_recovers_every_injected_filesystem_failure() {
    let mut forced_staging_gap = false;
    let mut forced_commit_cleanup_gap = false;
    let mut completed = false;
    for (failures, fail_at) in (1..=128).enumerate() {
        let temp = TempStateDir::new("rotation-crash");
        let logs = temp.0.join("logs");
        std::fs::create_dir_all(&logs).expect("create logs");
        let current = logs.join("gateway.log");
        let retained = LogConfig::new(&temp.0).retained_log_paths();
        std::fs::write(&current, "active\n").expect("seed active");
        for (index, path) in retained.iter().enumerate() {
            std::fs::write(path, format!("old-{}\n", index + 1)).expect("seed retained");
        }
        let old: Vec<Vec<u8>> = std::iter::once(&current)
            .chain(retained.iter())
            .map(|path| std::fs::read(path).expect("snapshot old chain"))
            .collect();
        let disk_budget = old.iter().map(Vec::len).sum::<usize>() as u64;
        let mut fault = FaultInjector::crashing(fail_at);
        let result = rotate_files(
            &current,
            &retained,
            &mut fault,
            ReplacementMode::RemoveThenRename,
        );
        if result.is_ok() {
            completed = true;
            assert_eq!(
                failures, fault.calls,
                "the loop injected every rotation checkpoint independently"
            );
            assert!(
                total_directory_file_bytes(&logs) <= disk_budget,
                "a completed rotation stays inside the original aggregate bytes"
            );
            assert_eq!(std::fs::read(&current).expect("new active"), b"");
            for (index, path) in retained.iter().enumerate() {
                assert_eq!(
                    std::fs::read(path).expect("new retained"),
                    old[index],
                    "the committed chain shifts each prior segment exactly once"
                );
            }
            break;
        }
        let operation = fault.assert_selected(fail_at, "rotation");
        assert!(
            total_directory_file_bytes(&logs) <= disk_budget,
            "transaction artifacts stay inside the aggregate budget at checkpoint {fail_at}"
        );
        // Cleanup removes the marker before its final parent sync. The
        // per-transaction state preserves that commit decision if that
        // exact sync is the injected crash boundary.
        let committed = observed_rotation_commit(&current, &fault);
        forced_commit_cleanup_gap |= interrupted_final_commit_cleanup(&current, &fault);
        if operation == "stage rotation source"
            && rotation_targets(&current, &retained)
                .iter()
                .any(|target| !target.exists() && artifact_path(target, ".rotation-old").exists())
        {
            forced_staging_gap = true;
        }
        recover_rotation(&current, &retained).expect("restart recovers rotation");
        assert!(
            total_directory_file_bytes(&logs) <= disk_budget,
            "recovery stays inside the same aggregate disk budget"
        );
        if committed {
            assert_eq!(std::fs::read(&current).expect("committed active"), b"");
            for (index, path) in retained.iter().enumerate() {
                assert_eq!(
                    std::fs::read(path).expect("committed retained"),
                    old[index],
                    "a durable commit marker keeps the complete new chain"
                );
            }
        } else {
            for (index, path) in std::iter::once(&current).chain(retained.iter()).enumerate() {
                assert_eq!(
                    std::fs::read(path).expect("rolled back chain"),
                    old[index],
                    "an uncommitted rotation restores every prior diagnostic name"
                );
            }
        }
    }
    assert!(
        forced_staging_gap,
        "fault injection reaches an in-place staging boundary with the source preserved"
    );
    assert!(
        forced_commit_cleanup_gap,
        "fault injection reaches the final sync after commit-marker cleanup"
    );
    assert!(
        completed,
        "the fault loop reaches the first non-failing run"
    );
}

#[test]
fn committed_sparse_rotation_survives_every_cleanup_crash_boundary() {
    let mut completed = false;
    for (failures, fail_at) in (1..=32).enumerate() {
        let temp = TempStateDir::new("sparse-cleanup-crash");
        let logs = temp.0.join("logs");
        std::fs::create_dir_all(&logs).expect("create logs");
        let config = LogConfig::new(&temp.0);
        let current = config.log_path();
        let retained = config.retained_log_paths();
        std::fs::write(&current, "").expect("seed fresh active");
        std::fs::write(&retained[0], "active\n").expect("seed shifted active");
        std::fs::write(&retained[2], "old-2\n").expect("seed sparse shifted segment");
        std::fs::write(&retained[4], "old-4\n").expect("seed sparse oldest destination");
        std::fs::write(artifact_path(&retained[4], ".rotation-old"), "old-5\n")
            .expect("seed pruned rollback segment");
        std::fs::write(artifact_path(&current, ".rotation-new"), "")
            .expect("seed stale empty stage");
        let old_mask = 1 | (1 << 2) | (1 << 4) | (1 << 5);
        std::fs::write(rotation_prepared_path(&current, old_mask), "")
            .expect("seed prepared marker");
        std::fs::write(rotation_staged_path(&current), "").expect("seed staged marker");
        std::fs::write(rotation_committed_path(&current), "").expect("seed commit marker");
        let disk_budget = total_directory_file_bytes(&logs);

        let mut fault = FaultInjector::crashing(fail_at);
        let result = cleanup_rotation_with(&current, &retained, &mut fault);
        if result.is_ok() {
            completed = true;
            assert_eq!(
                failures, fault.calls,
                "the loop injected every sparse cleanup checkpoint independently"
            );
        } else {
            fault.assert_selected(fail_at, "cleanup");
            assert!(
                total_directory_file_bytes(&logs) <= disk_budget,
                "interrupted cleanup never duplicates segment bytes"
            );
            recover_rotation(&current, &retained).expect("restart completes committed cleanup");
        }

        assert_eq!(std::fs::read(&current).expect("active survives"), b"");
        assert_eq!(
            std::fs::read(&retained[0]).expect("newest survives"),
            b"active\n"
        );
        assert!(!retained[1].exists(), "the sparse .2 remains absent");
        assert_eq!(
            std::fs::read(&retained[2]).expect("sparse .3 survives"),
            b"old-2\n"
        );
        assert!(!retained[3].exists(), "the sparse .4 remains absent");
        assert_eq!(
            std::fs::read(&retained[4]).expect("sparse .5 survives"),
            b"old-4\n"
        );
        assert!(
            total_directory_file_bytes(&logs) < disk_budget,
            "the committed oldest rollback segment is pruned after recovery"
        );
        if completed {
            break;
        }
    }
    assert!(
        completed,
        "the fault loop reaches the first non-failing sparse cleanup"
    );
}
