use super::{GlobalName, GlobalNameErrorKind};

fn kind_of(input: &str) -> GlobalNameErrorKind {
    GlobalName::parse(input)
        .expect_err("the input must be rejected")
        .kind()
}

#[test]
fn a_two_segment_name_parses_as_a_capability_name() {
    let name = GlobalName::parse("promptforge/web").expect("a valid capability name");
    assert_eq!(name.namespace(), "promptforge");
    assert_eq!(name.pack(), "web");
}

#[test]
fn a_three_segment_name_parses_as_a_tool_name() {
    let name = GlobalName::parse("promptforge/web/fetch").expect("a valid tool name");
    assert_eq!(name.namespace(), "promptforge");
    assert_eq!(name.pack(), "web");
}

#[test]
fn a_reverse_dns_namespace_parses() {
    let name = GlobalName::parse("org.rustalliance/core").expect("a valid capability name");
    assert_eq!(name.namespace(), "org.rustalliance");
    assert_eq!(name.pack(), "core");
}

#[test]
fn display_round_trips_a_two_segment_name() {
    let name = GlobalName::parse("promptforge/web").expect("a valid capability name");
    assert_eq!(name.to_string(), "promptforge/web");
    assert_eq!(
        GlobalName::parse(&name.to_string()).expect("the display form re-parses"),
        name
    );
}

#[test]
fn display_round_trips_a_three_segment_name() {
    let name = GlobalName::parse("promptforge/web/fetch").expect("a valid tool name");
    assert_eq!(name.to_string(), "promptforge/web/fetch");
    assert_eq!(
        GlobalName::parse(&name.to_string()).expect("the display form re-parses"),
        name
    );
}

#[test]
fn dashes_underscores_and_dots_are_legal_segment_characters() {
    GlobalName::parse("org.rustalliance/my-pack/v1_2.tool").expect("the charset allows - _ .");
}

#[test]
fn a_single_segment_is_rejected_as_a_segment_count_error() {
    assert_eq!(kind_of("promptforge"), GlobalNameErrorKind::SegmentCount);
}

#[test]
fn four_segments_are_rejected_as_a_segment_count_error() {
    assert_eq!(
        kind_of("promptforge/web/fetch/extra"),
        GlobalNameErrorKind::SegmentCount
    );
}

#[test]
fn an_empty_string_is_rejected_as_a_segment_count_error() {
    assert_eq!(kind_of(""), GlobalNameErrorKind::SegmentCount);
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
    assert_eq!(kind_of("promptforge/"), GlobalNameErrorKind::Empty);
}

#[test]
fn uppercase_is_rejected_because_comparison_is_case_sensitive() {
    assert_eq!(kind_of("Promptforge/web"), GlobalNameErrorKind::Control);
    assert_eq!(kind_of("promptforge/Web"), GlobalNameErrorKind::Control);
}

#[test]
fn an_at_sign_is_rejected_because_v1_is_unversioned() {
    assert_eq!(kind_of("promptforge/web@2"), GlobalNameErrorKind::Control);
}

#[test]
fn a_control_character_is_rejected_as_a_control_error() {
    assert_eq!(kind_of("promptforge/we\tb"), GlobalNameErrorKind::Control);
}

#[test]
fn non_ascii_is_rejected_as_a_control_error() {
    assert_eq!(
        kind_of("promptforge/w\u{e9}b"),
        GlobalNameErrorKind::Control
    );
}
