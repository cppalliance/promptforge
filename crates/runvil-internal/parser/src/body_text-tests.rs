//! Tests for reading prompt text in place from the one shared copy.

use std::sync::Arc;

use super::BodyText;

#[test]
fn a_body_text_reads_back_the_part_it_was_made_from() {
    let text: Arc<str> = Arc::from("# Tété\n\nFirst prose.\n\n## S\n");
    let start = text.find("First").unwrap();
    let part = &text[start..start + "First prose.".len()];
    let body = BodyText::within(&text, part).expect("a stretch of the shared text");
    assert_eq!(body.as_str(), "First prose.");
    assert_eq!(
        body.as_str().as_ptr(),
        part.as_ptr(),
        "the stretch must be read in place, not copied"
    );

    let tail = &text[text.len()..];
    let empty = BodyText::within(&text, tail).expect("an empty stretch at the end is still inside");
    assert_eq!(empty.as_str(), "");
}

#[test]
fn a_string_outside_the_shared_text_is_refused() {
    let text: Arc<str> = Arc::from("shared prompt body");
    let copy = String::from("shared prompt body");
    assert!(
        BodyText::within(&text, &copy).is_none(),
        "an equal string elsewhere is not a stretch of the shared text"
    );
    assert!(
        BodyText::within(&text, "shared").is_none(),
        "a literal is not a stretch of the shared text"
    );
    let other: Arc<str> = Arc::from("shared prompt body");
    assert!(
        BodyText::within(&text, &other[7..13]).is_none(),
        "another prompt's text is not a stretch of this one"
    );
}

#[test]
fn equal_text_from_different_stretches_compares_equal() {
    let text: Arc<str> = Arc::from("alpha\nalpha\nbeta");
    let first = BodyText::within(&text, &text[0..5]).unwrap();
    let second = BodyText::within(&text, &text[6..11]).unwrap();
    let third = BodyText::within(&text, &text[12..16]).unwrap();
    assert_eq!(
        first, second,
        "equality compares the text read, not where it sits"
    );
    assert_ne!(first, third);
    assert_eq!(first, BodyText::from_owned("alpha".to_owned()));
    assert_eq!(first, "alpha");
    assert_eq!(
        format!("{second:?}"),
        format!("{:?}", "alpha"),
        "debug output prints the text alone, as a String field did"
    );
}
