//! Tests for the send record `commit` keeps: the round a send extends and
//! how many leading messages it shares with that round's request.

use promptforge_types::ids::RoundId;

use super::{request, run, vm};

#[test]
fn the_first_commit_extends_nothing() {
    let (lua, list) = vm();
    run(&lua, "msgs:system('s'):user('u')");
    assert_eq!(list.commit(RoundId::new(0), &request(&list)), (None, 0));
}

#[test]
fn a_resend_with_no_changes_keeps_every_message() {
    let (lua, list) = vm();
    run(&lua, "msgs:system('s'):user('u')");
    let sent = request(&list);
    let _ = list.commit(RoundId::new(0), &sent);
    assert_eq!(
        list.commit(RoundId::new(1), &request(&list)),
        (Some(RoundId::new(0)), 2)
    );
}

#[test]
fn appends_keep_the_whole_previous_request() {
    let (lua, list) = vm();
    run(&lua, "msgs:system('s'):user('u1')");
    let _ = list.commit(RoundId::new(0), &request(&list));
    run(&lua, "msgs:assistant('a1'):user('u2')");
    let next = request(&list);
    assert_eq!(next.len(), 4);
    assert_eq!(
        list.commit(RoundId::new(1), &next),
        (Some(RoundId::new(0)), 2)
    );
}

#[test]
fn a_replace_in_the_middle_keeps_the_messages_before_the_first_change() {
    let (lua, list) = vm();
    run(
        &lua,
        "msgs:system('s'):user('u1'):assistant('a1'):user('u2'):assistant('a2'):user('u3')",
    );
    let _ = list.commit(RoundId::new(0), &request(&list));
    run(
        &lua,
        "msgs:replace(4, 4, { role = 'user', content = 'changed' })",
    );
    assert_eq!(
        list.commit(RoundId::new(1), &request(&list)),
        (Some(RoundId::new(0)), 3)
    );
}

#[test]
fn a_system_edit_keeps_nothing() {
    let (lua, list) = vm();
    run(
        &lua,
        "msgs:system('s'):user('u1'):assistant('a1'):user('u2')",
    );
    let _ = list.commit(RoundId::new(0), &request(&list));
    run(
        &lua,
        "msgs:replace(1, 1, { role = 'system', content = 'other' })",
    );
    assert_eq!(
        list.commit(RoundId::new(1), &request(&list)),
        (Some(RoundId::new(0)), 0)
    );
}

#[test]
fn removing_the_terminal_reply_keeps_all_but_the_last_message() {
    let (lua, list) = vm();
    run(&lua, "msgs:system('s'):user('u'):assistant('a')");
    let sent = request(&list);
    assert_eq!(sent.len(), 3);
    let _ = list.commit(RoundId::new(0), &sent);
    run(&lua, "msgs:replace(#msgs, #msgs)");
    assert_eq!(
        list.commit(RoundId::new(1), &request(&list)),
        (Some(RoundId::new(0)), 2)
    );
}

#[test]
fn each_commit_records_its_round_for_the_next_one() {
    let (lua, list) = vm();
    run(&lua, "msgs:user('u')");
    assert_eq!(list.commit(RoundId::new(3), &request(&list)), (None, 0));
    run(&lua, "msgs:assistant('a'):user('again')");
    assert_eq!(
        list.commit(RoundId::new(7), &request(&list)),
        (Some(RoundId::new(3)), 1)
    );
    assert_eq!(
        list.commit(RoundId::new(9), &request(&list)),
        (Some(RoundId::new(7)), 3)
    );
}
