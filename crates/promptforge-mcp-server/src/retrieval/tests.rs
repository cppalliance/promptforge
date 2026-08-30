//! What `need_prompt` retrieves, and what the tool call around it reports.
//!
//! Every test here shares one loaded model (see [`fixture`](super::fixture)), so
//! the suite pays for the weights once. The ranking tests assert positions in a
//! shortlist rather than scores: a score is a property of the model, while "the
//! right prompt came first" is the contract this tool has with its caller.

use std::sync::Arc;

use promptforge_tool_picker::{Config as PickerConfig, ToolPicker};
use rmcp::model::{CallToolRequestParams, ErrorCode, JsonObject};
use serde_json::{Value, json};

use super::fixture::{self, PROMPTS};
use super::{Candidate, Retrieval, Shortlist};
use crate::CatalogHandle;
use crate::catalog::{Catalog, Entry};
use crate::server::{PreparedTools, PromptForgeServer};
use crate::tools::NEED_PROMPT;

/// The candidate names a capability retrieves, best first.
fn names(shortlist: &Shortlist) -> Vec<String> {
    match shortlist {
        Shortlist::Candidates(candidates) => candidates.iter().map(|c| c.name.clone()).collect(),
        other => panic!("expected candidates, got {other:?}"),
    }
}

/// One `tools/call` request over an object of arguments.
fn call(arguments: Value) -> CallToolRequestParams {
    let arguments: JsonObject = match arguments {
        Value::Object(map) => map,
        other => panic!("arguments must be an object, got {other}"),
    };
    CallToolRequestParams::new(NEED_PROMPT).with_arguments(arguments)
}

/// A server over the six fixture prompts, with the given retrieval behind it.
fn server(retrieval: Retrieval) -> PromptForgeServer {
    let prompts = fixture::catalog(PROMPTS);
    PromptForgeServer::new(
        Arc::clone(&prompts.config),
        Arc::new(CatalogHandle::with_retrieval(
            prompts.catalog.clone(),
            retrieval,
        )),
        Arc::new(
            PreparedTools::new(
                &prompts.config.gateway,
                &prompts.config.tools,
                promptforge_core::model::ModelCatalog::empty(),
                fixture::model(),
                None,
            )
            .expect("prepare fixture live tools"),
        ),
    )
}

#[test]
fn boot_indexes_both_consumers_over_one_loaded_encoder() {
    // Boot's contract: the model is loaded once and lent to both the retrieval
    // index and the execution picker. A regression that made either consumer
    // load its own copy would double the weights parse at every boot.
    let prompts = fixture::catalog(PROMPTS);
    let model = fixture::model();
    let retrieval = Retrieval::start(model, &prompts.catalog, None);
    let prepared = PreparedTools::new(
        &prompts.config.gateway,
        &prompts.config.tools,
        promptforge_core::model::ModelCatalog::empty(),
        model,
        None,
    )
    .expect("prepare fixture live tools");

    assert!(
        retrieval.shares_model_with(prepared.picker()),
        "retrieval and the execution picker must rank over the same loaded encoder"
    );
}

#[test]
fn descriptors_carry_the_name_the_description_and_the_args_schema() {
    let prompts = fixture::catalog(&[("alpha", "Do the alpha thing.")]);
    let descriptors = super::index::descriptors(&prompts.catalog);

    let all: Vec<_> = descriptors.iter().collect();
    let [descriptor] = all.as_slice() else {
        panic!("one prompt is one descriptor");
    };
    assert_eq!(descriptor.server(), "promptforge");
    assert_eq!(descriptor.name(), "alpha");
    assert_eq!(descriptor.description(), "Do the alpha thing.");
    assert_eq!(
        descriptor.input_schema(),
        &json!({"type": "object", "properties": {"args": {"type": "string"}}}),
        "the descriptor declares what a run of the prompt takes"
    );
}

#[test]
fn a_broken_prompt_is_not_a_descriptor_and_so_is_never_recommended() {
    let prompts = fixture::catalog(&[("alpha", "Do the alpha thing.")]);
    let mut entries = prompts.catalog.entries().to_vec();
    entries.push(Entry::broken(
        "beta".to_owned(),
        prompts.catalog.entries()[0].path().to_path_buf(),
        "frontmatter is missing a name",
    ));
    let catalog = Catalog::new(entries);
    assert_eq!(catalog.len(), 2, "the broken entry is in the catalog");

    let descriptors = super::index::descriptors(&catalog);

    assert_eq!(descriptors.len(), 1, "and out of what retrieval ranks");
    assert_eq!(
        descriptors.iter().next().expect("one descriptor").name(),
        "alpha"
    );
}

#[test]
fn an_author_register_capability_returns_the_right_prompt_first() {
    let prompts = fixture::catalog(PROMPTS);
    let retrieval = fixture::retrieval(&prompts.catalog);

    let shortlist = retrieval.shortlist("Build a stakeholder position report for one entity.");

    let retrieved = names(&shortlist);
    assert_eq!(
        retrieved.len(),
        3,
        "three of six, best first: {retrieved:?}"
    );
    assert_eq!(retrieved[0], "stakeholder_position", "{retrieved:?}");
}

#[test]
fn a_conversational_capability_still_returns_it_inside_the_shortlist() {
    let prompts = fixture::catalog(PROMPTS);
    let retrieval = fixture::retrieval(&prompts.catalog);

    // The register the tool's description asks a caller to avoid: a goal about
    // named people rather than a documented capability. The shortlist is the
    // backstop that keeps it from being a wrong answer.
    let shortlist =
        retrieval.shortlist("I need to know where Herb Sutter stands on ABI stability.");

    let retrieved = names(&shortlist);
    assert!(
        retrieved.iter().any(|n| n == "stakeholder_position"),
        "the shortlist absorbs a conversational phrasing: {retrieved:?}"
    );
}

#[test]
fn a_capability_the_engine_default_would_abstain_on_still_returns_candidates() {
    let prompts = fixture::catalog(PROMPTS);
    let descriptors = super::index::descriptors(&prompts.catalog);
    let capability = "I need to know where Herb Sutter stands on ABI stability.";

    // The engine's own default floor, which was tuned for author-register prose.
    let strict =
        ToolPicker::build_with_model(fixture::model(), descriptors, PickerConfig::default(), None)
            .expect("the strict engine indexes the fixture");
    assert!(
        strict
            .shortlist(capability, 3)
            .expect("the strict engine ranks")
            .is_empty(),
        "the default floor abstains on this phrasing, which is what zero is for"
    );

    let retrieval = fixture::retrieval(&prompts.catalog);
    assert!(
        !names(&retrieval.shortlist(capability)).is_empty(),
        "nothing here binds unattended, so an abstention would only be an empty answer"
    );
}

#[test]
fn an_empty_catalog_returns_no_candidates_rather_than_an_error() {
    let retrieval = fixture::retrieval(&Catalog::new(Vec::new()));

    assert_eq!(
        retrieval.shortlist("Build a stakeholder position report for one entity."),
        Shortlist::Candidates(Vec::new())
    );
}

#[test]
fn an_idle_index_reports_that_retrieval_is_unavailable() {
    let retrieval = Retrieval::idle();

    assert!(!retrieval.is_available());
    assert_eq!(retrieval.shortlist("anything"), Shortlist::Unavailable);
}

#[test]
fn a_reindex_ranks_the_new_catalog_and_the_old_names_are_gone() {
    let before = fixture::catalog(&[
        (
            "stakeholder_position",
            "Build a stakeholder position report for one entity.",
        ),
        (
            "translate_text",
            "Translate a document into another language.",
        ),
    ]);
    let retrieval = fixture::retrieval(&before.catalog);
    let capability = "Build a stakeholder position report for one entity.";
    assert_eq!(
        names(&retrieval.shortlist(capability))[0],
        "stakeholder_position"
    );

    let after = fixture::catalog(&[
        (
            "position_report",
            "Build a stakeholder position report for one entity.",
        ),
        (
            "translate_text",
            "Translate a document into another language.",
        ),
    ]);
    let reindexed = retrieval.reindexed(&after.catalog);

    assert!(
        !retrieval.same_index(&reindexed),
        "a reindex is a new immutable index, not a mutation of the old one"
    );
    let retrieved = names(&reindexed.shortlist(capability));
    assert_eq!(retrieved[0], "position_report", "{retrieved:?}");
    assert!(
        !retrieved.iter().any(|n| n == "stakeholder_position"),
        "a name the catalog no longer has must not be recommended: {retrieved:?}"
    );
    assert_eq!(
        names(&retrieval.shortlist(capability))[0],
        "stakeholder_position",
        "the original index is immutable and still ranks its own catalog"
    );
}

#[test]
fn a_reindex_of_an_idle_index_stays_idle() {
    let prompts = fixture::catalog(PROMPTS);
    let retrieval = Retrieval::idle();

    let reindexed = retrieval.reindexed(&prompts.catalog);

    assert!(
        !reindexed.is_available(),
        "there is no model behind an idle index to reuse, and a save must not load one"
    );
}

#[tokio::test]
async fn the_tool_call_reports_each_candidate_as_a_name_and_a_description() {
    let prompts = fixture::catalog(PROMPTS);
    let retrieval = fixture::retrieval(&prompts.catalog);
    let expected = match retrieval.shortlist("Draft release notes from a range of commits.") {
        Shortlist::Candidates(candidates) => candidates,
        other => panic!("expected candidates, got {other:?}"),
    };
    let server = server(fixture::retrieval(&prompts.catalog));

    let result = server
        .dispatch(call(
            json!({ "capability": "Draft release notes from a range of commits." }),
        ))
        .await
        .expect("need_prompt answers");

    assert_ne!(result.is_error, Some(true), "candidates are not an error");
    let structured = result
        .structured_content
        .clone()
        .expect("the candidates are structured");
    assert_eq!(structured, json!({ "prompts": expected }));
    assert_eq!(
        structured["prompts"][0]["name"], "release_notes",
        "{structured:#}"
    );
    let [block] = result.content.as_slice() else {
        panic!("expected one content block");
    };
    let text = &block.as_text().expect("the block is text").text;
    assert_eq!(
        serde_json::from_str::<Value>(text).expect("the text block is the same JSON"),
        structured
    );
}

#[tokio::test]
async fn a_missing_capability_is_the_client_s_bug() {
    let prompts = fixture::catalog(PROMPTS);
    let server = server(fixture::retrieval(&prompts.catalog));

    let error = server
        .dispatch(call(json!({})))
        .await
        .expect_err("the schema declared a required string");
    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn a_mistyped_capability_is_the_client_s_bug() {
    let prompts = fixture::catalog(PROMPTS);
    let server = server(fixture::retrieval(&prompts.catalog));

    let error = server
        .dispatch(call(json!({ "capability": 7 })))
        .await
        .expect_err("the schema declared the required argument a string");
    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
}

#[test]
fn a_failed_shortlist_is_a_server_fault_the_caller_cannot_act_on() {
    // The production arm behind `need_prompt`: a ranking that could not embed
    // the capability is nothing the caller did or can correct, so it is a
    // protocol fault rather than a result carrying `isError`.
    let error =
        crate::server::need_prompt_result(&Shortlist::Failed("embedding failed".to_owned()))
            .expect_err("a failed ranking is a fault, not a result");
    assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
    assert!(
        error.message.contains("rank prompts for the capability"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn a_published_but_unloadable_index_says_so_rather_than_faulting() {
    let server = server(Retrieval::idle());

    let result = server
        .dispatch(call(json!({ "capability": "Draft release notes." })))
        .await
        .expect("an advertised tool that cannot answer is a result, not a fault");

    assert_eq!(result.is_error, Some(true));
    let [block] = result.content.as_slice() else {
        panic!("expected one content block");
    };
    let text = &block.as_text().expect("the block is text").text;
    assert!(
        text.contains("list_prompts"),
        "the caller is sent somewhere it can still get an answer: {text}"
    );
}

#[test]
fn a_candidate_serializes_as_a_name_and_a_description() {
    let candidate = Candidate {
        name: "alpha".to_owned(),
        description: "Do the alpha thing.".to_owned(),
    };

    assert_eq!(
        serde_json::to_value(&candidate).expect("a candidate serializes"),
        json!({ "name": "alpha", "description": "Do the alpha thing." })
    );
}
