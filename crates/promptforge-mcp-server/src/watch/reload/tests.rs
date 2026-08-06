//! What one settled window does, driven by calling the reload rather than by
//! provoking a filesystem.
//!
//! The last test in this file is the only one that goes through the handler. The
//! rest assert the catalog a reload leaves behind, which is a narrower claim than
//! the contract makes: a client calls a prompt, so the thing worth asserting is
//! that a call after a reload runs the new body and that a call to a prompt
//! broken by a save answers with the error rather than the last good copy.

use std::fs;
use std::sync::Arc;

use rmcp::model::{CallToolRequestParams, CallToolResult, JsonObject};
use serde_json::{Value, json};

use super::{Reload, ignored_changes};
use crate::config::Config;
use crate::retrieval::Retrieval;
use crate::server::{PreparedTools, PromptForgeServer};
use crate::watch::fixture::{Fixture, config_source};

/// One `tools/call` request over an object of arguments.
fn call(name: &'static str, arguments: Value) -> CallToolRequestParams {
    let arguments: JsonObject = match arguments {
        Value::Object(map) => map,
        other => panic!("arguments must be an object, got {other}"),
    };
    CallToolRequestParams::new(name).with_arguments(arguments)
}

/// The text of a result's single content block.
fn text_of(result: &CallToolResult) -> String {
    let [block] = result.content.as_slice() else {
        panic!("expected exactly one content block")
    };
    block.as_text().expect("the block is text").text.clone()
}

#[test]
fn an_edited_prompt_replaces_the_live_one() {
    let fixture = Fixture::new();
    fixture.rewrite("alpha", "Do the alpha thing, better", "alpha v2");

    let reload = fixture.reload();

    assert!(!reload.refused);
    assert!(
        reload.ranking_changed,
        "retrieval ranks on that description"
    );
    assert_eq!(fixture.description("alpha"), "Do the alpha thing, better");
}

#[test]
fn a_body_only_edit_leaves_the_ranking_alone() {
    let fixture = Fixture::new();
    fixture.rewrite("alpha", "Do the alpha thing", "alpha v2");

    let reload = fixture.reload();

    assert_eq!(
        reload,
        Reload {
            ranking_changed: false,
            refused: false,
        }
    );
}

#[test]
fn a_new_file_joins_the_catalog() {
    let fixture = Fixture::new();
    Fixture::write_prompt(fixture.root(), "gamma", "Do the gamma thing", "gamma v1");

    let reload = fixture.reload();

    assert!(!reload.refused);
    assert_eq!(fixture.catalog.load().len(), 3);
    assert_eq!(fixture.description("gamma"), "Do the gamma thing");
}

#[test]
fn a_deleted_file_leaves_the_catalog() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.root().join("prompts").join("beta.md"))
        .expect("remove the fixture prompt");

    let reload = fixture.reload();

    assert!(!reload.refused);
    let catalog = fixture.catalog.load();
    assert_eq!(catalog.len(), 1);
    assert!(catalog.find("beta").is_none());
}

#[test]
fn a_broken_prompt_is_retained_with_its_error_and_the_rest_keep_serving() {
    let fixture = Fixture::new();
    fixture.break_prompt("alpha");

    let reload = fixture.reload();

    assert!(!reload.refused, "one bad file must not freeze the catalog");
    let catalog = fixture.catalog.load();
    assert_eq!(catalog.len(), 2);
    let broken = catalog
        .find("alpha")
        .expect("the broken entry keeps its place");
    assert!(
        broken.problem().is_some(),
        "a broken entry carries the error that a call to it answers with"
    );
    assert!(broken.prompt().is_none());
    let healthy = catalog
        .find("beta")
        .expect("every other prompt keeps serving");
    assert!(healthy.problem().is_none());
    assert!(healthy.prompt().is_some());
}

#[test]
fn a_catalog_level_fault_keeps_the_previous_catalog() {
    let fixture = Fixture::new();
    for name in ["alpha", "beta"] {
        fs::remove_file(fixture.root().join("prompts").join(format!("{name}.md")))
            .expect("remove the fixture prompt");
    }

    let reload = fixture.reload();

    assert!(reload.refused, "an empty resolved catalog is not an answer");
    assert_eq!(fixture.catalog.load().len(), 2);
}

#[test]
fn an_unparsable_configuration_keeps_the_previous_catalog() {
    let fixture = Fixture::new();
    fs::write(fixture.root().join("prompts.toml"), "this is not toml = [")
        .expect("break the configuration");

    let reload = fixture.reload();

    assert!(reload.refused);
    assert_eq!(fixture.catalog.load().len(), 2);
}

#[test]
fn a_run_in_flight_finishes_under_the_snapshot_it_started_with() {
    let fixture = Fixture::new();
    // What a run holds: an `Arc<Catalog>` taken before the save.
    let in_flight = fixture.catalog.load();
    fixture.rewrite("alpha", "Something else entirely", "alpha v2");

    let reload = fixture.reload();

    assert!(!reload.refused);
    assert_eq!(
        in_flight
            .find("alpha")
            .expect("the snapshot still holds the entry")
            .description(),
        "Do the alpha thing",
        "the snapshot a run started with is unaffected by the swap"
    );
    assert_eq!(fixture.description("alpha"), "Something else entirely");
}

#[test]
fn a_bind_change_is_ignored_rather_than_applied() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join("prompts.toml"),
        config_source(fixture.root(), "bind = \"127.0.0.1:9999\"\n"),
    )
    .expect("rewrite the bind address");

    let reload = fixture.reload();

    assert!(
        !reload.refused,
        "a change the reload ignores is not a refusal"
    );
    assert_eq!(
        fixture.catalog.load().len(),
        2,
        "the catalog reloads even though the socket does not"
    );
}

#[test]
fn every_setting_a_reload_cannot_apply_is_named() {
    let dir = tempfile::tempdir().expect("create a temporary root");
    let root = dir.path();
    let boot = Config::from_toml_str(&config_source(root, "")).expect("the boot configuration");

    assert!(
        ignored_changes(&boot, &boot).is_empty(),
        "an unchanged file changes nothing"
    );

    let moved = Config::from_toml_str(&config_source(
        root,
        "bind = \"127.0.0.1:9999\"\nwatch_debounce = \"2s\"\n",
    ))
    .expect("the candidate configuration");
    assert_eq!(
        ignored_changes(&boot, &moved),
        vec!["[server].bind", "[server].watch_debounce"]
    );

    let elsewhere = Config::from_toml_str(
        "[server]\ntoken = \"shared\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:9999/v1\"\ntoken = \"gw\"\nmodel = \"m\"\n",
    )
    .expect("the candidate configuration");
    let named = ignored_changes(&boot, &elsewhere);
    assert!(named.contains(&"[gateway].url"), "{named:?}");
    assert!(named.contains(&"[gateway].model"), "{named:?}");
    assert!(named.contains(&"[paths].prompts"), "{named:?}");
}

/// What a reload does to the retrieval index behind `need_prompt`.
///
/// These share the test binary's one loaded model, so a rebuild here costs one
/// forward pass per prompt and no weights - which is the whole point of the
/// rebuild path this rides.
#[cfg(feature = "picker")]
mod retrieval {
    use super::{Arc, Fixture, fs};
    use crate::retrieval::{Retrieval, Shortlist, fixture as retrieval_fixture};

    /// The names a capability retrieves, best first.
    fn names(retrieval: &Retrieval, capability: &str) -> Vec<String> {
        match retrieval.shortlist(capability) {
            Shortlist::Candidates(candidates) => candidates.into_iter().map(|c| c.name).collect(),
            other => panic!("expected candidates, got {other:?}"),
        }
    }

    /// A fixture whose retrieval index is loaded over its own catalog.
    fn indexed() -> (Fixture, Arc<Retrieval>) {
        let retrieval = Arc::new(Retrieval::idle());
        let fixture = Fixture::with_retrieval(Arc::clone(&retrieval));
        retrieval.install_with(retrieval_fixture::embedder(), &fixture.catalog.load());
        (fixture, retrieval)
    }

    #[test]
    fn a_renamed_prompt_is_retrievable_under_its_new_name() {
        let (fixture, retrieval) = indexed();
        let capability = "Do the alpha thing";
        assert!(names(&retrieval, capability).contains(&"alpha".to_owned()));

        fs::remove_file(fixture.root().join("prompts").join("alpha.md"))
            .expect("remove the old file");
        Fixture::write_prompt(
            fixture.root(),
            "alpha_two",
            "Do the alpha thing",
            "alpha v2",
        );
        let reload = fixture.reload();

        assert!(reload.ranking_changed, "a name is part of what is ranked");
        assert_eq!(retrieval.rebuilds(), 1);
        let retrieved = names(&retrieval, capability);
        assert!(retrieved.contains(&"alpha_two".to_owned()), "{retrieved:?}");
        assert!(
            !retrieved.contains(&"alpha".to_owned()),
            "a name the catalog no longer has must not be recommended: {retrieved:?}"
        );
    }

    #[test]
    fn a_body_only_edit_does_not_rebuild_the_index() {
        let (fixture, retrieval) = indexed();

        fixture.rewrite("alpha", "Do the alpha thing", "alpha v2");
        let reload = fixture.reload();

        assert!(!reload.ranking_changed);
        assert_eq!(
            retrieval.rebuilds(),
            0,
            "a body nothing ranks on changed, so the embedding cost is not paid"
        );
    }
}

#[tokio::test]
async fn a_call_after_a_reload_runs_the_new_body_and_a_broken_prompt_answers_with_its_error() {
    let fixture = Fixture::new();
    let server = PromptForgeServer::new(
        Arc::clone(&fixture.config),
        Arc::clone(&fixture.catalog),
        Arc::new(Retrieval::idle()),
        Arc::new(PreparedTools::new(&fixture.config.gateway).expect("prepare fixture live tools")),
    );

    let before = server
        .dispatch(call("run_prompt", json!({ "prompt": "alpha" })))
        .await
        .expect("the runner answers");
    assert_eq!(text_of(&before), "alpha v1");

    fixture.rewrite("alpha", "Do the alpha thing", "alpha v2");
    fixture.break_prompt("beta");
    assert!(!fixture.reload().refused);

    let after = server
        .dispatch(call("run_prompt", json!({ "prompt": "alpha" })))
        .await
        .expect("the runner answers");
    assert_eq!(
        text_of(&after),
        "alpha v2",
        "the second call runs the body the save wrote"
    );

    let broken = server
        .dispatch(call("run_prompt", json!({ "prompt": "beta" })))
        .await
        .expect("a broken prompt is a result, not a protocol error");
    assert_eq!(broken.is_error, Some(true));
    assert!(
        !text_of(&broken).contains("beta v1"),
        "a prompt broken by a save must not run the last good copy: {}",
        text_of(&broken)
    );
}

#[tokio::test]
async fn a_prompt_added_mid_session_is_callable_on_the_same_handler() {
    // The whole of what replaced the announcement: nothing is told to anyone,
    // and the one handler a session holds runs a prompt written after it
    // connected, because every call reads the catalog fresh.
    let fixture = Fixture::new();
    let server = PromptForgeServer::new(
        Arc::clone(&fixture.config),
        Arc::clone(&fixture.catalog),
        Arc::new(Retrieval::idle()),
        Arc::new(PreparedTools::new(&fixture.config.gateway).expect("prepare fixture live tools")),
    );

    let missing = server
        .dispatch(call("run_prompt", json!({ "prompt": "gamma" })))
        .await
        .expect("an unresolvable name is a result, not a protocol error");
    assert_eq!(missing.is_error, Some(true));

    Fixture::write_prompt(fixture.root(), "gamma", "Do the gamma thing", "gamma v1");
    assert!(!fixture.reload().refused);

    let listed = server
        .dispatch(call("list_prompts", json!({})))
        .await
        .expect("the listing answers");
    assert!(text_of(&listed).contains("gamma"));

    let ran = server
        .dispatch(call("run_prompt", json!({ "prompt": "gamma" })))
        .await
        .expect("the runner answers");
    assert_eq!(text_of(&ran), "gamma v1");
}
