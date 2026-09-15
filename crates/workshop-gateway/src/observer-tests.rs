use std::sync::Arc;

use serde_json::json;
use shared_promptforge_api::events::{ClientTiming, LlamaTimings, Usage, VllmMetrics};

use super::*;

fn full_metrics() -> CallMetrics {
    CallMetrics {
        usage: Some(Usage {
            prompt_tokens: 7,
            completion_tokens: 3,
            total_tokens: 10,
            cached_tokens: Some(2),
            reasoning_tokens: Some(1),
        }),
        llama: Some(LlamaTimings {
            prompt_n: 7,
            prompt_ms: 12.5,
            prompt_per_second: 560.0,
            predicted_n: 3,
            predicted_ms: 30.5,
            predicted_per_second: 98.5,
            draft_n: 4,
            draft_n_accepted: 2,
        }),
        vllm: Some(VllmMetrics {
            time_to_first_token_ms: Some(8.5),
            generation_time_ms: Some(22.5),
            queue_time_ms: Some(1.5),
            mean_itl_ms: Some(7.5),
            tokens_per_second: Some(133.5),
        }),
        client: Some(ClientTiming {
            ttft_ms: Some(9.5),
            mean_itl_ms: Some(8.25),
            e2e_ms: 41.5,
        }),
    }
}

/// Emits one event of every kind through the Observer hooks.
fn emit_one_of_each(log: &WorkshopObserver) {
    log.on_user_input("run", "chat", "hi");
    log.on_thinking("run", "chat", 0, 0, 1, "llama-3", "pondering");
    log.on_assistant_tool_calls(
        "run",
        "chat",
        0,
        0,
        1,
        "llama-3",
        &[ToolCallEvent {
            id: "call_1".to_owned(),
            name: "read_file".to_owned(),
            arguments: json!({ "path": "notes.txt" }),
        }],
    );
    log.on_tool_result(
        "run",
        "chat",
        0,
        0,
        1,
        "call_1",
        "read_file",
        "file contents",
        false,
    );
    log.on_assistant_reply(
        "run",
        "chat",
        1,
        0,
        2,
        "hello",
        Some("stop"),
        "llama-3",
        Some(&full_metrics()),
    );
}

fn collect(log: &WorkshopObserver) -> Vec<RuntimeEvent> {
    (0..log.len())
        .map(|index| log.get(index).expect("every index below len() reads"))
        .collect()
}

#[test]
fn concurrent_appends_lose_nothing_and_preserve_per_producer_order() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    let log = Arc::new(WorkshopObserver::new(Some(&path)).expect("open a fresh log"));

    let mut producers = Vec::new();
    for producer in 0..4 {
        let log = Arc::clone(&log);
        producers.push(std::thread::spawn(move || {
            let section = format!("producer-{producer}");
            for sequence in 0..25 {
                log.on_user_input("run", &section, &sequence.to_string());
            }
        }));
    }
    for producer in producers {
        producer.join().expect("producer threads finish");
    }

    assert_eq!(log.len(), 100, "no append may be lost");
    let events = collect(&log);
    let expected: Vec<String> = (0..25).map(|sequence| sequence.to_string()).collect();
    for producer in 0..4 {
        let section = format!("producer-{producer}");
        let sequence: Vec<&str> = events
            .iter()
            .filter(|event| event.section == section)
            .map(|event| event.content.as_str())
            .collect();
        assert_eq!(
            sequence, expected,
            "{section} must keep its own append order through the interleaving"
        );
    }

    // The file's order is the in-memory order: the two advance under
    // one guard, and the replay proves it.
    let replayed = WorkshopObserver::load_from(&path).expect("replay the concurrent log");
    assert_eq!(collect(&replayed), events);
}

#[test]
fn event_log_reads_see_a_consistent_prefix() {
    let log = Arc::new(WorkshopObserver::new(None).expect("open a memory log"));
    let writer = Arc::clone(&log);
    let producer = std::thread::spawn(move || {
        for sequence in 0..200 {
            writer.on_user_input("run", "chat", &sequence.to_string());
        }
    });

    // Every observed length is a fully readable prefix, and an entry
    // once appended never changes.
    loop {
        let len = log.len();
        for index in 0..len {
            let event = log
                .get(index)
                .expect("every index below an observed len() must read");
            assert_eq!(
                event.content,
                index.to_string(),
                "entry {index} must be the entry that was appended there"
            );
        }
        if len == 200 {
            break;
        }
        std::thread::yield_now();
    }
    producer.join().expect("the producer thread finishes");
}

#[test]
fn subscribe_receives_every_entry_in_log_order() {
    let log = WorkshopObserver::new(None).expect("open a memory log");
    let mut entries = log.subscribe();
    emit_one_of_each(&log);

    let expected = [
        (RuntimeEventKind::UserInput, "hi".to_owned()),
        (RuntimeEventKind::Thinking, "pondering".to_owned()),
        (
            RuntimeEventKind::AssistantToolCalls,
            r#"[{"id":"call_1","name":"read_file","arguments":{"path":"notes.txt"}}]"#.to_owned(),
        ),
        (RuntimeEventKind::ToolResult, "file contents".to_owned()),
        (RuntimeEventKind::AssistantReply, "hello".to_owned()),
    ];
    for (kind, content) in expected {
        let received = entries.try_recv().expect("every appended entry broadcasts");
        assert_eq!((received.kind, received.content), (kind, content));
    }
    assert!(
        matches!(
            entries.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ),
        "no entry may broadcast that was not appended"
    );
}

#[test]
fn append_and_load_round_trip_byte_for_byte() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    let log = WorkshopObserver::new(Some(&path)).expect("open a fresh log");
    emit_one_of_each(&log);
    let in_memory = collect(&log);
    drop(log);

    let original = std::fs::read_to_string(&path).expect("read the persisted log");
    let restored = WorkshopObserver::load_from(&path).expect("replay the log");
    assert_eq!(collect(&restored), in_memory, "replay restores every entry");

    // Re-serializing the replayed log reproduces the file byte for
    // byte: nothing was lost, reordered, or reshaped in either
    // direction.
    let mut rebuilt = header_line().expect("the header line renders");
    for event in collect(&restored) {
        rebuilt.push_str(&serde_json::to_string(&event).expect("events serialize"));
        rebuilt.push('\n');
    }
    assert_eq!(rebuilt, original);

    // A loaded log keeps appending to the same file, behind the same
    // header.
    restored.on_user_input("run", "chat", "again");
    drop(restored);
    let reloaded = WorkshopObserver::load_from(&path).expect("replay the appended log");
    assert_eq!(reloaded.len(), 6);
    assert_eq!(
        reloaded.get(5).map(|event| event.content),
        Some("again".to_owned())
    );
}

#[test]
fn load_from_tolerates_crlf_line_endings() {
    // An autocrlf checkout of the committed canary, or a log touched
    // by a CRLF editor, materializes \r\n endings; replay must keep
    // reading such a file.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    let log = WorkshopObserver::new(Some(&path)).expect("open a fresh log");
    emit_one_of_each(&log);
    let events = collect(&log);
    drop(log);

    let text = std::fs::read_to_string(&path).expect("read the persisted log");
    std::fs::write(&path, text.replace('\n', "\r\n")).expect("rewrite with CRLF endings");

    let replayed = WorkshopObserver::load_from(&path).expect("a CRLF log must still load");
    assert_eq!(collect(&replayed), events);
}

#[test]
fn new_truncates_to_a_fresh_headed_log() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    std::fs::write(&path, "stale junk from an earlier life\n").expect("seed stale bytes");

    let log = WorkshopObserver::new(Some(&path)).expect("open over the stale file");
    drop(log);
    assert_eq!(
        std::fs::read_to_string(&path).expect("read the fresh log"),
        header_line().expect("the header line renders"),
        "new() must truncate to a bare versioned header"
    );
    let empty = WorkshopObserver::load_from(&path).expect("replay the fresh log");
    assert_eq!(empty.len(), 0);
}

#[test]
fn load_from_rejects_missing_and_alien_headers_and_torn_lines() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let header = header_line().expect("the header line renders");

    let empty = dir.path().join("empty.jsonl");
    std::fs::write(&empty, "").expect("write the empty file");
    let error = WorkshopObserver::load_from(&empty).expect_err("an empty file must not load");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("missing event log header"));

    let alien = dir.path().join("alien.jsonl");
    std::fs::write(
        &alien,
        "{\"format\":\"workshop-event-log\",\"version\":999}\n",
    )
    .expect("write the alien file");
    let error = WorkshopObserver::load_from(&alien).expect_err("an alien version must not load");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("unsupported event log"));

    let torn = dir.path().join("torn.jsonl");
    std::fs::write(&torn, format!("{header}{{\"kind\":\"user_message\""))
        .expect("write the torn file");
    let error = WorkshopObserver::load_from(&torn).expect_err("a torn line must not load");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error.to_string().contains("line 2"),
        "the error must name the offending line: {error}"
    );

    // The version discipline leans on this: a kind outside the
    // version-1 vocabulary (the reserved `plan`, for one) must refuse
    // to load rather than replay as something else.
    let unknown = dir.path().join("unknown.jsonl");
    std::fs::write(
        &unknown,
        format!(
            "{header}{{\"kind\":\"plan\",\"section\":\"chat\",\"chain_id\":0,\"depth\":0,\"turn\":0,\"content\":\"\"}}\n"
        ),
    )
    .expect("write the unknown-kind file");
    let error = WorkshopObserver::load_from(&unknown)
        .expect_err("a kind this version does not speak must not load");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error.to_string().contains("malformed event on line 2"),
        "the error must name the offending line: {error}"
    );

    let missing = dir.path().join("missing.jsonl");
    let error = WorkshopObserver::load_from(&missing).expect_err("a missing file must not load");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[test]
fn a_poisoned_lock_recovers_for_appends_and_reads() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    let log = Arc::new(WorkshopObserver::new(Some(&path)).expect("open a fresh log"));

    let poisoner = Arc::clone(&log);
    let panicked = std::thread::spawn(move || {
        let _guard = poisoner
            .inner
            .write()
            .expect("the lock is not yet poisoned");
        panic!("poisoning the event log lock on purpose");
    })
    .join();
    assert!(panicked.is_err(), "the poisoning thread must panic");
    assert!(log.inner.is_poisoned(), "the lock must be poisoned");

    // Zone two: the poison is recovered, not propagated - appends,
    // reads, broadcast, and persistence all keep working.
    let mut entries = log.subscribe();
    log.on_user_input("run", "chat", "after the poison");
    assert_eq!(log.len(), 1);
    assert_eq!(
        log.get(0).map(|event| event.content),
        Some("after the poison".to_owned())
    );
    assert_eq!(
        entries
            .try_recv()
            .expect("the broadcast survives the poison")
            .content,
        "after the poison"
    );
    drop(entries);
    drop(log);
    let replayed = WorkshopObserver::load_from(&path).expect("replay the poisoned-era log");
    assert_eq!(replayed.len(), 1, "persistence survives the poison");
}

#[test]
fn a_failing_writer_degrades_to_the_in_memory_log() {
    struct FailingWriter;
    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("injected append failure"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let log = WorkshopObserver::with_writer_for_test(FailingWriter);
    let mut entries = log.subscribe();
    emit_one_of_each(&log);
    assert_eq!(
        log.len(),
        5,
        "a failed file append never loses the in-memory entry"
    );
    assert_eq!(
        entries
            .try_recv()
            .expect("the broadcast survives the failing writer")
            .content,
        "hi"
    );
}
