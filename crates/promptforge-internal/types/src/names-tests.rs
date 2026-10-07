//! Tests for `GlobalName` parsing and its rejection kinds.

use super::{GlobalName, GlobalNameErrorKind};

fn kind_of(input: &str) -> GlobalNameErrorKind {
    GlobalName::parse(input)
        .expect_err("the input must be rejected")
        .kind()
}

#[test]
fn a_one_segment_name_parses() {
    let name = GlobalName::parse("web").expect("a valid one-segment name");
    assert_eq!(name.segments(), ["web"]);
}

#[test]
fn any_number_of_segments_parses() {
    let name = GlobalName::parse("github/issues/create/v2").expect("a valid four-segment name");
    assert_eq!(name.segments(), ["github", "issues", "create", "v2"]);
}

#[test]
fn display_round_trips_a_one_segment_name() {
    let name = GlobalName::parse("web").expect("a valid name");
    assert_eq!(name.to_string(), "web");
    assert_eq!(
        GlobalName::parse(&name.to_string()).expect("the display form re-parses"),
        name
    );
}

#[test]
fn display_round_trips_a_multi_segment_name() {
    let name = GlobalName::parse("web/fetch").expect("a valid name");
    assert_eq!(name.to_string(), "web/fetch");
    assert_eq!(
        GlobalName::parse(&name.to_string()).expect("the display form re-parses"),
        name
    );
}

#[test]
fn the_first_segment_is_a_one_segment_name() {
    let name = GlobalName::parse("web/fetch/deep").expect("a valid name");
    assert_eq!(
        name.first(),
        GlobalName::parse("web").expect("a valid name")
    );
}

#[test]
fn dashes_underscores_and_dots_are_legal_segment_characters() {
    GlobalName::parse("org.rustalliance/my-pack/v1_2.tool").expect("the charset allows - _ .");
}

#[test]
fn an_empty_string_is_rejected_as_an_empty_error() {
    assert_eq!(kind_of(""), GlobalNameErrorKind::Empty);
}

#[test]
fn an_empty_middle_segment_is_rejected_as_an_empty_error() {
    assert_eq!(kind_of("promptforge//web"), GlobalNameErrorKind::Empty);
}

#[test]
fn a_leading_separator_is_rejected_as_an_empty_error() {
    assert_eq!(kind_of("/web"), GlobalNameErrorKind::Empty);
}

#[test]
fn a_trailing_separator_is_rejected_as_an_empty_error() {
    assert_eq!(kind_of("web/"), GlobalNameErrorKind::Empty);
}

#[test]
fn uppercase_is_rejected_because_comparison_is_case_sensitive() {
    assert_eq!(kind_of("Web"), GlobalNameErrorKind::Control);
    assert_eq!(kind_of("web/Fetch"), GlobalNameErrorKind::Control);
}

#[test]
fn an_at_sign_is_rejected_because_v1_is_unversioned() {
    assert_eq!(kind_of("web@2"), GlobalNameErrorKind::Control);
}

#[test]
fn a_control_character_is_rejected_as_a_control_error() {
    assert_eq!(kind_of("we\tb"), GlobalNameErrorKind::Control);
}

#[test]
fn non_ascii_is_rejected_as_a_control_error() {
    assert_eq!(kind_of("w\u{e9}b"), GlobalNameErrorKind::Control);
}
