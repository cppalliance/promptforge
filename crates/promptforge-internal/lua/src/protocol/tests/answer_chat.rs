//! Answer-to-envelope rendering for the `chat` answer's typed error. A
//! successful `chat` answer resumes as an opaque userdata, which the
//! `models.loop` adapter's tests read.

use super::*;

#[test]
fn an_err_chat_answer_round_trips_and_retains_the_typed_error() {
    let lua = Lua::new();
    let (envelope, retained) = Answer::Chat(Err(Error::Interrupted))
        .into_envelope(&lua)
        .expect("the envelope renders");
    match retained {
        Some(Error::Interrupted) => {}
        other => panic!("expected the retained Interrupted error, got {other:?}"),
    }
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(!ok);
    let (kind, message) = failure_parts(&lua, result);
    assert_eq!(kind, "cancelled");
    assert_eq!(
        message,
        "interrupted: the run was cancelled or this call was stopped"
    );
}
