//! Tests for the error value's kind vocabulary: its tags parse back, and a
//! rejected tag's error names every accepted one.

use super::{ErrorKind, ParseErrorKindError};

#[test]
fn every_error_kind_tag_parses_back_to_its_kind() {
    for kind in ErrorKind::ALL {
        assert_eq!(kind.tag().parse::<ErrorKind>(), Ok(*kind));
    }
}

#[test]
fn an_unknown_error_kind_names_the_text_and_every_accepted_tag() {
    let error: ParseErrorKindError = "stoer"
        .parse::<ErrorKind>()
        .expect_err("a misspelled tag is not a kind");
    assert_eq!(error.input(), "stoer");
    let message = error.to_string();
    assert!(message.contains("`stoer`"), "the rejected text: {message}");
    for kind in ErrorKind::ALL {
        assert!(
            message.contains(&format!("`{}`", kind.tag())),
            "required every accepted tag, actual {message:?} without `{}`",
            kind.tag()
        );
    }
}
