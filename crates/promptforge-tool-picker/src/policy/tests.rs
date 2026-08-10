//! Decision-policy tests: precedence, boundaries, hints, and the solo rule.

use super::{CandidateGroup, Outcome, Shortlist, decide, shortlist};
use crate::catalog::{ToolAnnotations, ToolDescriptor, ToolId};
use crate::config::Config;
use crate::rank::{Candidate, Vectors};
use serde_json::json;

/// How wide one synthetic stored row is.
const STRIDE: usize = 2;

/// Unit rows a half-radian apart, the closest pair scoring about `0.88`.
const SPREAD: [f32; 8] = [
    1.0,
    0.0,
    0.877_582_6,
    0.479_425_54,
    0.540_302_3,
    0.841_471,
    0.070_737_2,
    0.997_495,
];

/// One unit row repeated: every pair is a copy, similarity exactly `1.0`.
const COPIES: [f32; 8] = [0.6, 0.8, 0.6, 0.8, 0.6, 0.8, 0.6, 0.8];

fn distinct(count: usize) -> &'static [f32] {
    &SPREAD[..count * STRIDE]
}

fn twinned(count: usize) -> &'static [f32] {
    &COPIES[..count * STRIDE]
}

fn pair_at(similarity: f32) -> [f32; 4] {
    [1.0, 0.0, similarity, (1.0 - similarity * similarity).sqrt()]
}

fn rows(data: &[f32]) -> Vectors<'_> {
    Vectors::new(data, STRIDE)
}

fn tool(server: &str, name: &str) -> ToolDescriptor {
    ToolDescriptor::new(ToolId::new(server, name), "does a thing", json!({}))
}

fn hinted(server: &str, name: &str, annotations: ToolAnnotations) -> ToolDescriptor {
    tool(server, name).with_annotations(annotations)
}

fn ranking(scores: &[f32]) -> Vec<Candidate> {
    scores
        .iter()
        .enumerate()
        .map(|(index, &score)| Candidate::new(index, score))
        .collect()
}

fn one_server() -> Vec<ToolDescriptor> {
    vec![tool("files", "read_file"), tool("files", "load_file")]
}

fn two_servers() -> Vec<ToolDescriptor> {
    vec![tool("files", "read_file"), tool("blobs", "read_file")]
}

fn just_below(value: f32) -> f32 {
    f32::from_bits(value.to_bits() - 1)
}

fn exact_config() -> Config {
    Config::default()
        .with_similarity_floor(0.5)
        .and_then(|config| config.with_margin(0.125))
        .and_then(|config| config.with_duplicate_threshold(0.9375))
        .expect("exact thresholds are in the supported domain")
}

/// Builds the group an outcome is expected to carry, borrowing `tools`.
fn group<'a>(tools: &'a [ToolDescriptor], indices: &[usize]) -> CandidateGroup<'a> {
    CandidateGroup::new(indices.iter().map(|&index| &tools[index]).collect())
}

#[test]
fn nothing_above_the_floor_is_an_abstention() {
    let tools = two_servers();
    let outcome = decide(
        &ranking(&[0.4, 0.3]),
        &tools,
        rows(distinct(2)),
        &Config::default(),
    );
    assert_eq!(outcome, Outcome::Absent);
}

#[test]
fn an_empty_ranking_and_out_of_range_candidates_abstain() {
    let tools = two_servers();
    assert_eq!(
        decide(&[], &tools, rows(distinct(2)), &Config::default()),
        Outcome::Absent
    );
    let outcome = decide(
        &ranking(&[0.0, 0.0, 0.99])[2..],
        &tools,
        rows(distinct(2)),
        &Config::default(),
    );
    assert_eq!(outcome, Outcome::Absent);
}

#[test]
fn a_clear_leader_binds() {
    let tools = two_servers();
    let outcome = decide(
        &ranking(&[0.95, 0.7]),
        &tools,
        rows(distinct(2)),
        &Config::default(),
    );
    assert_eq!(outcome, Outcome::Bind(&tools[0]));
}

#[test]
fn twin_tools_on_one_server_are_a_duplicate() {
    let tools = one_server();
    let outcome = decide(
        &ranking(&[0.99, 0.985]),
        &tools,
        rows(twinned(2)),
        &Config::default(),
    );
    assert_eq!(outcome, Outcome::Duplicate(group(&tools, &[0, 1])));
}

#[test]
fn twin_tools_across_servers_are_a_shortlist() {
    let tools = two_servers();
    let outcome = decide(
        &ranking(&[0.99, 0.985]),
        &tools,
        rows(twinned(2)),
        &Config::default(),
    );
    assert_eq!(outcome, Outcome::Ambiguous(group(&tools, &[0, 1])));
}

#[test]
fn twins_are_measured_between_the_tools_not_between_their_scores() {
    let config = Config::default();
    assert!(0.9 < config.duplicate_threshold());
    let tools = one_server();
    assert_eq!(
        decide(&ranking(&[0.9, 0.9]), &tools, rows(twinned(2)), &config),
        Outcome::Duplicate(group(&tools, &[0, 1]))
    );
}

#[test]
fn a_duplicate_is_reported_even_when_the_margin_would_separate_it() {
    let config = Config::default().with_margin(0.01).expect("valid margin");
    let tools = one_server();
    let outcome = decide(&ranking(&[0.995, 0.98]), &tools, rows(twinned(2)), &config);
    assert_eq!(outcome, Outcome::Duplicate(group(&tools, &[0, 1])));
}

#[test]
fn a_score_exactly_at_the_floor_is_considered() {
    let config = exact_config();
    let tools = two_servers();
    assert_eq!(
        decide(
            &ranking(&[config.similarity_floor()]),
            &tools,
            rows(distinct(2)),
            &config
        ),
        Outcome::Bind(&tools[0])
    );
    assert_eq!(
        decide(
            &ranking(&[just_below(config.similarity_floor())]),
            &tools,
            rows(distinct(2)),
            &config
        ),
        Outcome::Absent
    );
}

#[test]
fn a_gap_exactly_equal_to_the_margin_binds() {
    let config = exact_config();
    let tools = two_servers();
    assert_eq!(
        decide(&ranking(&[0.875, 0.75]), &tools, rows(distinct(2)), &config),
        Outcome::Bind(&tools[0])
    );
    assert_eq!(
        decide(
            &ranking(&[0.875, 0.78125]),
            &tools,
            rows(distinct(2)),
            &config
        ),
        Outcome::Ambiguous(group(&tools, &[0, 1]))
    );
}

#[test]
fn a_pair_exactly_at_the_duplicate_threshold_is_a_twin() {
    let config = exact_config();
    let tools = one_server();
    let threshold = config.duplicate_threshold();
    assert_eq!(
        decide(
            &ranking(&[0.9, 0.9]),
            &tools,
            rows(&pair_at(threshold)),
            &config
        ),
        Outcome::Duplicate(group(&tools, &[0, 1]))
    );
    assert_eq!(
        decide(
            &ranking(&[0.9, 0.9]),
            &tools,
            rows(&pair_at(just_below(threshold))),
            &config
        ),
        Outcome::Ambiguous(group(&tools, &[0, 1]))
    );
}

#[test]
fn a_shortlist_group_is_bounded_by_the_configured_length() {
    let tools = vec![
        tool("a", "one"),
        tool("b", "two"),
        tool("c", "three"),
        tool("d", "four"),
    ];
    let scores = ranking(&[0.9, 0.895, 0.89, 0.885]);

    let Outcome::Ambiguous(shortlist) =
        decide(&scores, &tools, rows(distinct(4)), &Config::default())
    else {
        panic!("four candidates within the margin are a shortlist");
    };
    assert_eq!(shortlist.len(), 3);

    let narrow = Config::default().with_top_k(1).expect("valid top_k");
    let Outcome::Ambiguous(shortlist) = decide(&scores, &tools, rows(distinct(4)), &narrow) else {
        panic!("a narrow shortlist is still a shortlist");
    };
    assert_eq!(shortlist.len(), 2, "never fewer than both sides of the tie");
}

/// A read-only tool and a plainly modifying one, tied exactly.
fn read_only_against_writer() -> Vec<ToolDescriptor> {
    vec![
        hinted(
            "files",
            "write_file",
            ToolAnnotations::new().with_read_only(false),
        ),
        hinted(
            "blobs",
            "read_file",
            ToolAnnotations::new().with_read_only(true),
        ),
    ]
}

#[test]
fn a_hint_leads_the_shortlist_when_the_scores_tie_exactly() {
    let tools = read_only_against_writer();
    let outcome = decide(
        &ranking(&[0.9, 0.9]),
        &tools,
        rows(distinct(2)),
        &Config::default(),
    );
    assert_eq!(outcome, Outcome::Ambiguous(group(&tools, &[1, 0])));
}

#[test]
fn a_hint_chooses_which_tool_binds() {
    let config = Config::default().with_margin(0.0).expect("valid margin");
    let tools = read_only_against_writer();
    assert_eq!(
        decide(&ranking(&[0.9, 0.9]), &tools, rows(distinct(2)), &config),
        Outcome::Bind(&tools[1])
    );
}

#[test]
fn read_only_outranks_the_other_two_hints() {
    let tools = vec![
        hinted(
            "files",
            "one",
            ToolAnnotations::new()
                .with_read_only(true)
                .with_destructive(true)
                .with_idempotent(false),
        ),
        hinted(
            "blobs",
            "two",
            ToolAnnotations::new()
                .with_read_only(false)
                .with_destructive(false)
                .with_idempotent(true),
        ),
    ];
    assert_eq!(
        decide(
            &ranking(&[0.9, 0.9]),
            &tools,
            rows(distinct(2)),
            &Config::default()
        ),
        Outcome::Ambiguous(group(&tools, &[0, 1]))
    );
}

#[test]
fn absent_or_equal_hints_leave_catalog_order() {
    let safe = ToolAnnotations::new()
        .with_read_only(true)
        .with_destructive(false)
        .with_idempotent(true);
    let unclaimed = ToolAnnotations::new();
    let negative = ToolAnnotations::new()
        .with_read_only(false)
        .with_destructive(true)
        .with_idempotent(false);

    for (first, second) in [
        (unclaimed, unclaimed),
        (safe, safe),
        (negative, negative),
        (unclaimed, negative),
        (negative, unclaimed),
    ] {
        let tools = vec![
            hinted("files", "one", first),
            hinted("blobs", "two", second),
        ];
        assert_eq!(
            decide(
                &ranking(&[0.9, 0.9]),
                &tools,
                rows(distinct(2)),
                &Config::default()
            ),
            Outcome::Ambiguous(group(&tools, &[0, 1]))
        );
    }
}

#[test]
fn the_solo_candidate_rule_holds_at_its_boundaries() {
    let config = Config::default()
        .with_similarity_floor(0.8)
        .and_then(|config| config.with_solo_floor(0.5))
        .expect("valid floors");
    let tools = two_servers();

    // One leader between the floors binds.
    assert_eq!(
        decide(&ranking(&[0.7]), &tools, rows(distinct(2)), &config),
        Outcome::Bind(&tools[0])
    );
    // Below the solo floor abstains.
    assert_eq!(
        decide(&ranking(&[0.4]), &tools, rows(distinct(2)), &config),
        Outcome::Absent
    );
    // Two peers between the floors abstain rather than bind either.
    assert_eq!(
        decide(&ranking(&[0.7, 0.6]), &tools, rows(distinct(2)), &config),
        Outcome::Absent
    );
    // Equal floors disable the rule.
    let disabled = Config::default()
        .with_similarity_floor(0.8)
        .and_then(|config| config.with_solo_floor(0.8))
        .expect("valid floors");
    assert_eq!(
        decide(&ranking(&[0.7]), &tools, rows(distinct(2)), &disabled),
        Outcome::Absent
    );
}

#[test]
fn shortlist_offers_only_above_floor_candidates() {
    let config = Config::default();
    let tools = two_servers();
    let listed = shortlist(&ranking(&[0.9, 0.4]), &tools, &config);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed.first(), Some(&tools[0]));
}

#[test]
fn shortlist_returns_the_lone_solo_candidate_and_empties_on_two_peers() {
    let config = Config::default()
        .with_similarity_floor(0.8)
        .and_then(|config| config.with_solo_floor(0.5))
        .expect("valid floors");
    let tools = two_servers();

    let solo = shortlist(&ranking(&[0.7]), &tools, &config);
    assert_eq!(solo.first(), Some(&tools[0]), "one leader between floors");

    let peers = shortlist(&ranking(&[0.7, 0.6]), &tools, &config);
    assert!(peers.is_empty(), "two peers between floors offer nothing");

    let below = shortlist(&ranking(&[0.4]), &tools, &config);
    assert!(below.is_empty(), "a below-solo-floor leader offers nothing");
}

#[test]
fn a_shortlist_iterates_and_indexes_borrowed_descriptors() {
    let tools = vec![tool("a", "one"), tool("b", "two"), tool("c", "three")];
    let config = Config::default()
        .with_similarity_floor(0.0)
        .expect("valid floor");
    let listed: Shortlist<'_> = shortlist(&ranking(&[0.9, 0.85, 0.8]), &tools, &config);
    assert_eq!(listed.iter().count(), 3);
    assert_eq!(listed.get(2), Some(&tools[2]));
    let expected: &ToolDescriptor = &tools[0];
    assert!(std::ptr::eq(listed.first().unwrap(), expected));
}
