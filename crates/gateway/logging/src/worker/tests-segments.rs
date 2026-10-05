//! Tests for segment byte budgets, the terminal-record reserve, and restart normalization.

use super::super::rotation::open_log_file_with_limits;
use super::*;
use crate::config::LogConfig;

fn total_log_bytes(state_dir: &Path) -> u64 {
    let config = LogConfig::new(state_dir);
    std::iter::once(config.log_path())
        .chain(config.retained_log_paths())
        .map(|path| path.metadata().map_or(0, |metadata| metadata.len()))
        .sum()
}

#[test]
fn rotation_reserves_the_marker_and_preserves_the_terminal_record() {
    let temp = TempStateDir::new("segment-terminal");
    let limits = RotationLimits {
        segment: 64,
        aggregate: 128,
        terminal_record: 24,
    };
    let (path, sink) = open_log_file_with_limits(&temp.0, limits).expect("open segmented log");
    let queue = Arc::new(LogQueue::new());
    let worker = LogWorker::spawn(Arc::clone(&queue), sink).expect("spawn the production worker");
    queue.enqueue(LogPriority::Info, Box::from("ordinary-record-000\n"));
    queue.enqueue(LogPriority::Info, Box::from("ordinary-record-001\n"));
    queue.enqueue(LogPriority::Info, Box::from("gateway exiting\n"));
    queue.close();
    worker.join().expect("worker drains segmented sink");

    let retained =
        std::fs::read_to_string(temp.0.join("logs/gateway.log.1")).expect("retained segment");
    assert!(
        retained.ends_with(SEGMENT_TRUNCATION_MARKER),
        "the full segment ends with the reserved marker"
    );
    assert!(
        retained.len() as u64 <= limits.segment,
        "the retained segment obeys its fixed-size budget"
    );
    assert_eq!(
        std::fs::read_to_string(path).expect("active segment"),
        "gateway exiting\n",
        "the terminal record moves whole to the active segment"
    );
    assert!(
        total_log_bytes(&temp.0) <= limits.aggregate,
        "active and retained bytes stay inside one aggregate budget"
    );
}

#[test]
fn byte_boundaries_rotate_a_full_numbered_chain_without_splitting_utf8() {
    let temp = TempStateDir::new("segment-byte-boundaries");
    let logs = temp.0.join("logs");
    std::fs::create_dir_all(&logs).expect("create logs");
    let config = LogConfig::new(&temp.0);
    let current = config.log_path();
    let retained = config.retained_log_paths();
    File::create(&current).expect("create active");
    for (index, path) in retained.iter().enumerate() {
        std::fs::write(path, format!("old-{}\n", index + 1)).expect("seed full chain");
    }
    let retained_bytes = retained
        .iter()
        .map(|path| path.metadata().expect("retained metadata").len())
        .sum();
    let limits = RotationLimits {
        segment: 48,
        aggregate: 48 * 6,
        terminal_record: 16,
    };
    let mut sink = SegmentedFile {
        current: current.clone(),
        retained: retained.clone(),
        file: Some(BufWriter::new(
            OpenOptions::new()
                .append(true)
                .open(&current)
                .expect("open active"),
        )),
        current_bytes: 0,
        retained_bytes,
        limits,
    };
    let multibyte = "😀aaaaaaaaaaaaaa\n";
    let exact_boundary = "bbbbbbbbbbbbbbb\n";
    let maximum_terminal = "ccccccccccccccc\n";
    assert_eq!(multibyte.len(), 19);
    assert_eq!(exact_boundary.len(), 16);
    assert_eq!(
        u64::try_from(maximum_terminal.len()).expect("record length fits u64"),
        limits.terminal_record
    );

    sink.write_line(multibyte).expect("write multibyte record");
    sink.write_line(exact_boundary)
        .expect("exact byte boundary stays in the active segment");
    sink.flush().expect("flush exact boundary");
    assert_eq!(
        std::fs::read_to_string(&current).expect("read exact active"),
        format!("{multibyte}{exact_boundary}"),
        "equality with the reserved marker does not rotate"
    );

    sink.write_line(maximum_terminal)
        .expect("one byte over rotates before the maximum terminal record");
    sink.flush().expect("flush terminal");
    let newest =
        std::fs::read_to_string(&retained[0]).expect("newest retained remains valid UTF-8");
    assert_eq!(
        newest,
        format!("{multibyte}{exact_boundary}{SEGMENT_TRUNCATION_MARKER}")
    );
    assert_eq!(newest.len() as u64, limits.segment);
    assert_eq!(
        std::fs::read_to_string(&current).expect("active terminal"),
        maximum_terminal
    );
    for (index, path) in retained.iter().enumerate().skip(1) {
        assert_eq!(
            std::fs::read_to_string(path).expect("shifted retained"),
            format!("old-{index}\n"),
            "the complete numbered chain shifts oldest-first"
        );
    }
    assert!(
        !std::fs::read_to_string(&retained[retained.len() - 1])
            .expect("oldest retained")
            .contains("old-5"),
        "the prior oldest segment is pruned only after its replacement is durable"
    );
    assert!(
        total_log_bytes(&temp.0) <= limits.aggregate,
        "all named segments remain within the aggregate byte budget"
    );
}

#[test]
fn restart_caps_legacy_segments_and_prunes_oldest_before_admission() {
    let temp = TempStateDir::new("segment-restart");
    let logs = temp.0.join("logs");
    std::fs::create_dir_all(&logs).expect("logs dir");
    let terminal = "gateway exiting after a fatal error\n";
    std::fs::write(
        logs.join("gateway.log"),
        format!("{}{}", "😀".repeat(40), terminal),
    )
    .expect("seed oversized active log");
    std::fs::write(logs.join("gateway.log.1"), "newer-retained".repeat(4))
        .expect("seed newer retained log");
    std::fs::write(logs.join("gateway.log.2"), "oldest-retained".repeat(4))
        .expect("seed oldest retained log");
    let limits = RotationLimits {
        segment: 64,
        aggregate: 80,
        terminal_record: 40,
    };

    let (_path, mut first) =
        open_log_file_with_limits(&temp.0, limits).expect("normalize first restart");
    first
        .write_line("first restart\n")
        .expect("write after first restart");
    first.flush().expect("flush first restart");
    let normalized =
        std::fs::read_to_string(logs.join("gateway.log.1")).expect("normalized legacy segment");
    assert!(
        normalized.starts_with(SEGMENT_TRUNCATION_MARKER),
        "an oversized legacy segment records the omitted prefix"
    );
    assert!(
        normalized.ends_with(terminal),
        "tail compaction reserves enough room for the prior terminal record"
    );
    drop(first);
    let (_path, mut second) =
        open_log_file_with_limits(&temp.0, limits).expect("normalize second restart");
    second
        .write_line("second restart\n")
        .expect("write after second restart");
    second.flush().expect("flush second restart");

    let config = LogConfig::new(&temp.0);
    for path in std::iter::once(config.log_path()).chain(config.retained_log_paths()) {
        let bytes = path.metadata().map_or(0, |metadata| metadata.len());
        assert!(
            bytes <= limits.segment,
            "{} exceeded the segment budget with {bytes} bytes",
            path.display()
        );
    }
    assert!(
        total_log_bytes(&temp.0) <= limits.aggregate,
        "restart normalization and later writes preserve the aggregate budget"
    );
    assert!(
        !logs.join("gateway.log.2").exists(),
        "oldest segments are pruned before newer bytes are admitted"
    );
    let retained =
        std::fs::read_to_string(logs.join("gateway.log.1")).expect("newest retained segment");
    assert!(
        retained.contains("first restart"),
        "the current numbered diagnostic name retains the newest prior segment"
    );
}
