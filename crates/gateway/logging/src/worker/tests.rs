//! Tests for the stderr fallback, the latency budget, and run retention.

use super::*;
use crate::queue::{LogPriority, LogQueue};
use std::sync::Arc;

#[path = "tests-recovery.rs"]
mod recovery;
#[path = "tests-segments.rs"]
mod segments;

/// A file whose handle rejects writes, standing in for a disk
/// failure: opened read-only, every write and flush errors.
fn rejected_file(dir: &Path) -> File {
    let path = dir.join("rejected.log");
    std::fs::write(&path, "").expect("seed the file");
    std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open read-only")
}

#[test]
fn a_failed_file_write_falls_back_to_synchronous_stderr() {
    let temp = TempStateDir::new("sink-write-fallback");
    let mut sink = Sink::File(BufWriter::new(rejected_file(&temp.0)));
    assert!(!sink.is_stderr(), "the sink starts on the file");

    // A record larger than the buffer bypasses it and reaches the
    // rejecting handle immediately.
    let big = "x".repeat(16 * 1024);
    sink.write_line(&big);
    assert!(
        sink.is_stderr(),
        "a rejected write switches the sink to stderr"
    );
    sink.write_line("after the fallback\n");
    sink.flush();
    assert!(
        sink.is_stderr(),
        "the fallback keeps accepting records instead of failing"
    );
}

#[test]
fn a_failed_file_flush_falls_back_to_synchronous_stderr() {
    let temp = TempStateDir::new("sink-flush-fallback");
    let mut sink = Sink::File(BufWriter::new(rejected_file(&temp.0)));

    // A small record sits in the buffer, so the write succeeds and
    // the flush is what the handle rejects.
    sink.write_line("buffered record\n");
    assert!(!sink.is_stderr(), "a buffered write has not failed yet");
    sink.flush();
    assert!(
        sink.is_stderr(),
        "a rejected flush switches the sink to stderr"
    );
}

/// Enqueues `records` lines and drains them through `sink` with the
/// real worker's batch loop, returning the p95 enqueue-to-write
/// latency. The enqueue instant is stamped before the record enters
/// the queue, so the queue's sequence number indexes the stamps.
fn measure_p95_enqueue_to_write(sink: Sink, records: usize) -> std::time::Duration {
    use std::sync::Mutex;
    use std::time::Instant;

    let queue = Arc::new(LogQueue::new());
    let stamps = Arc::new(Mutex::new(Vec::<Instant>::with_capacity(records)));
    let worker = {
        let queue = Arc::clone(&queue);
        let stamps = Arc::clone(&stamps);
        std::thread::spawn(move || {
            let mut sink = sink;
            let mut latencies = Vec::with_capacity(records);
            loop {
                let batch = queue.take_batch();
                for record in &batch.records {
                    sink.write_line(&record.line);
                    let written = Instant::now();
                    let index = usize::try_from(record.sequence).expect("sequence fits");
                    let enqueued = stamps.lock().expect("stamps mutex")[index];
                    latencies.push(written - enqueued);
                }
                if let Some(summary) = &batch.summary {
                    sink.write_line(summary);
                }
                sink.flush();
                if batch.done {
                    break;
                }
            }
            latencies
        })
    };
    for index in 0..records {
        stamps.lock().expect("stamps mutex").push(Instant::now());
        queue.enqueue(
            LogPriority::Info,
            Box::from(format!(
                "latency probe {index}: a record of roughly the size a formatted event has\n"
            )),
        );
    }
    queue.close();
    let mut latencies = worker.join().expect("the worker joins");
    assert_eq!(
        latencies.len(),
        records,
        "every enqueued record was written"
    );
    let p95 = records * 95 / 100;
    latencies.select_nth_unstable(p95);
    latencies[p95]
}

#[test]
#[ignore = "release-mode latency budget: run `cargo test -p gateway-logging --release -- --ignored`"]
fn production_logging_stays_within_latency_budget() {
    use std::time::Duration;

    const RECORDS: usize = 20_000;

    let baseline = measure_p95_enqueue_to_write(Sink::Null, RECORDS);
    let temp = TempStateDir::new("latency");
    let (_path, file) = open_log_file(&temp.0).expect("open the production segmented sink");
    let file_sink = measure_p95_enqueue_to_write(Sink::Segmented(file), RECORDS);

    // The budget: less than 2% over the null-sink baseline, or 1 ms,
    // whichever is larger.
    let budget = (baseline / 50).max(Duration::from_millis(1));
    println!("p95 enqueue-to-write: null sink {baseline:?}, file sink {file_sink:?}");
    println!("budget: {budget:?} (2% of baseline or 1 ms, whichever is larger)");
    assert!(
        file_sink <= baseline + budget,
        "the file sink's p95 {file_sink:?} exceeds the baseline {baseline:?} by more than {budget:?}"
    );
}

struct TempStateDir(PathBuf);

impl TempStateDir {
    fn new(test: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gateway-logging-{test}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create the temp state dir");
        Self(dir)
    }
}

impl Drop for TempStateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_log_rotation_retains_five_previous_runs() {
    let temp = TempStateDir::new("rotation");
    std::fs::create_dir_all(temp.0.join("logs")).expect("logs dir");
    std::fs::write(temp.0.join("logs/gateway.log"), "first run").expect("seed log");

    let (path, file) = open_log_file(&temp.0).expect("first rotation opens");
    drop(file);
    assert_eq!(path, temp.0.join("logs/gateway.log"));
    assert_eq!(
        std::fs::read_to_string(temp.0.join("logs/gateway.log.1")).expect("rotated log"),
        "first run",
        "the previous run's log rotates to .1"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("fresh log"),
        "",
        "the new run starts on a fresh file"
    );

    // Five more runs fill the retained chain: after six rotations the
    // first run has shifted to .5 and every slot holds its run.
    for run in 2..=6u32 {
        std::fs::write(&path, format!("run {run}")).expect("write the run's log");
        let (_path, file) = open_log_file(&temp.0).expect("rotation opens");
        drop(file);
    }
    for run in 1..=5u32 {
        assert_eq!(
            std::fs::read_to_string(temp.0.join(format!("logs/gateway.log.{run}")))
                .expect("retained log"),
            format!("run {}", 7 - run),
            ".{run} holds the run {n} log",
            n = 7 - run
        );
    }
    assert!(
        !temp.0.join("logs/gateway.log.6").exists(),
        "retention stops at five previous runs"
    );
}

#[test]
fn the_sixth_previous_run_drops_off_the_retained_chain() {
    let temp = TempStateDir::new("rotation-drop");
    std::fs::create_dir_all(temp.0.join("logs")).expect("logs dir");

    // Seven runs: the two oldest must leave the chain entirely once
    // more than five previous runs exist.
    for run in 1..=7u32 {
        std::fs::write(temp.0.join("logs/gateway.log"), format!("run {run}"))
            .expect("write the run's log");
        let (_path, file) = open_log_file(&temp.0).expect("rotation opens");
        drop(file);
    }
    let retained: Vec<String> = (1..=5u32)
        .map(|run| {
            std::fs::read_to_string(temp.0.join(format!("logs/gateway.log.{run}")))
                .expect("retained log")
        })
        .collect();
    assert_eq!(
        retained,
        vec!["run 7", "run 6", "run 5", "run 4", "run 3"],
        "the chain holds exactly the five newest previous runs"
    );
    assert!(
        !retained.iter().any(|contents| contents == "run 1"),
        "the sixth previous run is deleted, not retained"
    );
}
