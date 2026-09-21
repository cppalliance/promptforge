//! Workshop observer tests: concurrent appends, consistent reads, and poisoned-lock recovery.

use std::sync::Arc;

use promptforge_api_types::ids::{ChainId, Provenance, TaskId};

use super::*;

/// A user-input event under `section` containing `text`, stamped with the
/// root task's zeroth sequence: the payload is what these tests read back.
fn input(section: &str, text: &str) -> Event {
    Event::UserInput {
        execution: "run".to_owned(),
        section: section.to_owned(),
        provenance: Provenance {
            task: TaskId::from(ChainId::root()),
            seq: 0,
        },
        text: text.to_owned(),
    }
}

/// The text of a user-input event, the field the assertions compare.
fn text_of(event: &Event) -> &str {
    match event {
        Event::UserInput { text, .. } => text,
        other => panic!("these tests append user-input events only, got {other:?}"),
    }
}

fn collect(log: &WorkshopObserver) -> Vec<Event> {
    (0..log.len())
        .map(|index| log.get(index).expect("every index below len() reads"))
        .collect()
}

#[test]
fn concurrent_appends_lose_nothing_and_preserve_per_producer_order() {
    let log = Arc::new(WorkshopObserver::new());

    let mut producers = Vec::new();
    for producer in 0..4 {
        let log = Arc::clone(&log);
        producers.push(std::thread::spawn(move || {
            let section = format!("producer-{producer}");
            for sequence in 0..25 {
                log.append(input(&section, &sequence.to_string()));
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
            .filter(|event| event.section() == section)
            .map(text_of)
            .collect();
        assert_eq!(
            sequence, expected,
            "{section} must keep its own append order through the interleaving"
        );
    }
}

#[test]
fn event_log_reads_see_a_consistent_prefix() {
    let log = Arc::new(WorkshopObserver::new());
    let writer = Arc::clone(&log);
    let producer = std::thread::spawn(move || {
        for sequence in 0..200 {
            writer.append(input("chat", &sequence.to_string()));
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
                text_of(&event),
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
    let log = WorkshopObserver::new();
    let mut entries = log.subscribe();
    for text in ["hi", "pondering", "hello"] {
        log.append(input("chat", text));
    }

    for expected in ["hi", "pondering", "hello"] {
        let received = entries.try_recv().expect("every appended entry broadcasts");
        assert_eq!(text_of(&received), expected);
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
fn reads_past_the_end_are_none_and_an_empty_log_says_so() {
    let log = WorkshopObserver::new();
    assert!(log.is_empty());
    assert_eq!(log.get(0), None);
    log.append(input("chat", "only"));
    assert!(!log.is_empty());
    assert_eq!(log.get(1), None, "reads at or past len must return None");
}

#[test]
fn a_poisoned_lock_recovers_for_appends_and_reads() {
    let log = Arc::new(WorkshopObserver::new());

    let poisoner = Arc::clone(&log);
    let panicked = std::thread::spawn(move || {
        let _guard = poisoner
            .events
            .write()
            .expect("the lock is not yet poisoned");
        panic!("poisoning the event log lock on purpose");
    })
    .join();
    assert!(panicked.is_err(), "the poisoning thread must panic");
    assert!(log.events.is_poisoned(), "the lock must be poisoned");

    // The poison is recovered, not propagated - appends, reads, and
    // broadcast all keep working.
    let mut entries = log.subscribe();
    log.append(input("chat", "after the poison"));
    assert_eq!(log.len(), 1);
    assert_eq!(log.get(0).as_ref().map(text_of), Some("after the poison"));
    assert_eq!(
        text_of(
            &entries
                .try_recv()
                .expect("the broadcast survives the poison")
        ),
        "after the poison"
    );
}
