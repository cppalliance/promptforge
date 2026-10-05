//! Commit promotion and the item, result, and failure capacity it reserves.

use std::future::pending;

use super::{
    CANCEL_JOIN_CAPACITY, COMMITTED_ITEM_CAPACITY, RESULT_CAPACITY, append_committable, encoded,
    session, source_message,
};

#[test]
fn commit_promotes_the_provisional_id_and_preserves_durable_lineage() {
    let mut session = session();
    let first_provisional = append_committable(&mut session);
    let first = session.commit().expect("first item commits");
    assert_eq!(first.item_id(), first_provisional);
    assert_eq!(first.previous_item_id(), None);

    session
        .finalize_completed(first.item_id(), "first")
        .expect("first item finalizes");
    let second_provisional = append_committable(&mut session);
    let second = session.commit().expect("second item commits");
    assert_eq!(second.item_id(), second_provisional);
    assert_eq!(second.previous_item_id(), Some(first.item_id()));
}

#[test]
fn committed_capacity_is_reserved_before_input_detach_and_retryable() {
    let mut session = session();
    let mut committed = Vec::new();
    for _ in 0..COMMITTED_ITEM_CAPACITY {
        append_committable(&mut session);
        committed.push(session.commit().expect("item commits within capacity"));
    }
    assert_eq!(session.committed_count(), COMMITTED_ITEM_CAPACITY);

    let retry_id = append_committable(&mut session);
    let error = session.commit().expect_err("fifth item is rejected");
    assert_eq!(error.to_string(), "commit fixture input");
    assert_eq!(
        source_message(&error).as_deref(),
        Some("the committed realtime item limit is reached")
    );
    assert_eq!(
        session
            .input_snapshot()
            .expect("rejected commit preserves input")
            .item_id(),
        retry_id
    );

    session
        .finalize_completed(committed[0].item_id(), "done")
        .expect("one item releases capacity");
    session.drain_results();
    let retried = session.commit().expect("same input retries");
    assert_eq!(retried.item_id(), retry_id);
}

#[test]
fn result_capacity_hypothesis_replacement_and_terminal_reservation_are_independent() {
    let mut session = session();
    append_committable(&mut session);
    let item = session.commit().expect("item commits");
    for index in 0..RESULT_CAPACITY {
        session
            .push_delta(item.item_id(), &format!("delta-{index}"))
            .expect("result enters bounded capacity");
    }
    let error = session
        .push_delta(item.item_id(), "overflow")
        .expect_err("capacity-plus-one is rejected");
    assert_eq!(error.to_string(), "push fixture delta");
    assert_eq!(
        source_message(&error).as_deref(),
        Some("the realtime session result capacity is reached")
    );

    session
        .replace_hypothesis(item.item_id(), 1, "old")
        .expect("first hypothesis enters its slot");
    session
        .replace_hypothesis(item.item_id(), 2, "new")
        .expect("new hypothesis replaces old");
    session
        .finalize_completed(item.item_id(), "authoritative")
        .expect("terminal uses its reserved slot despite saturation");

    let results = session.drain_results();
    assert_eq!(
        results
            .iter()
            .filter(|event| event["type"] == "delta")
            .count(),
        RESULT_CAPACITY
    );
    let hypothesis = results
        .iter()
        .find(|event| event["type"] == "hypothesis")
        .expect("one replaceable hypothesis remains");
    assert_eq!(hypothesis["revision"], 2);
    assert_eq!(hypothesis["transcript"], "new");
    assert_eq!(
        results
            .iter()
            .filter(|event| event["type"] == "completed")
            .count(),
        1
    );
}

#[test]
fn pending_precommit_failure_blocks_append_but_commits_one_item_failure() {
    let mut session = session();
    let item_id = append_committable(&mut session);
    session
        .fail_precommit("accurate segment failed")
        .expect("failure is retained by the input");
    let error = session
        .append_base64(&encoded(&[0, 0]))
        .expect_err("failed input rejects later audio");
    assert_eq!(error.to_string(), "append fixture audio");
    assert_eq!(
        source_message(&error).as_deref(),
        Some("accurate segment failed")
    );

    let committed = session
        .commit()
        .expect("failed input still establishes item");
    assert_eq!(committed.item_id(), item_id);
    let error = session
        .finalize_failed(committed.item_id(), "duplicate")
        .expect_err("a second terminal is rejected");
    assert_eq!(error.to_string(), "finalize fixture item failed");
    assert_eq!(
        source_message(&error).as_deref(),
        Some("the committed item already reached a terminal outcome")
    );
    let results = session.drain_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "failed");
    assert_eq!(results[0]["message"], "accurate segment failed");
}

#[test]
fn clear_discards_pending_precommit_failure_without_creating_an_item() {
    let mut session = session();
    append_committable(&mut session);
    session
        .fail_precommit("discard me")
        .expect("failure is retained");
    session.clear().expect("failed uncommitted input clears");
    assert_eq!(session.committed_count(), 0);
    assert!(session.drain_results().is_empty());
}

#[tokio::test]
async fn commit_reserves_interim_join_capacity_before_detaching_input() {
    let mut session = session();
    for _ in 0..CANCEL_JOIN_CAPACITY {
        session
            .append_base64(&encoded(&[0, 0]))
            .expect("input appends");
        session
            .spawn_interim(pending())
            .expect("interim starts within capacity");
        session.clear().expect("task is retained");
    }

    let provisional = append_committable(&mut session);
    session
        .spawn_interim(pending())
        .expect("current interim starts");
    let error = session
        .commit()
        .expect_err("commit cannot detach an unowned task");
    assert_eq!(error.to_string(), "commit fixture input");
    assert_eq!(
        source_message(&error).as_deref(),
        Some("the canceled interim task join capacity is reached")
    );
    assert_eq!(
        session
            .input_snapshot()
            .expect("rejected commit preserves input")
            .item_id(),
        provisional
    );
    session.join_canceled().await.expect("retired tasks join");
    assert_eq!(
        session.commit().expect("retry commits").item_id(),
        provisional
    );
}
