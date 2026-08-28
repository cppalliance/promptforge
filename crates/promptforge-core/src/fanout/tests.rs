use std::sync::Arc;

use serde_json::json;
use tokio::sync::mpsc;

use super::arm::ArmFinalizer;
use super::*;
use crate::observe::{Observation, Observer, detail};
use crate::parser::Block;

#[test]
fn resolve_sibling_finds_exact_match() {
    let sections = vec![sibling("Worker", 3), sibling("Topics", 3)];
    let found = resolve_sibling("### Worker", &sections).expect("must resolve");
    assert_eq!(found.name, "Worker");
}

#[test]
fn resolve_sibling_missing_heading_lists_available() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("### Missing", &sections).expect_err("missing heading must error");
    assert!(err.to_string().contains("### Worker"), "error was: {err}");
}

#[test]
fn resolve_sibling_bare_name_errors() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("Worker", &sections).expect_err("bare name without ### must error");
    assert!(err.to_string().contains("### markers"), "error was: {err}");
}

fn sibling(name: &str, level: u8) -> Section {
    crate::test_support::synthetic_section(
        name,
        level,
        vec![Block::Prose {
            text: String::new(),
            loop_capable: true,
        }],
        Vec::new(),
    )
}

#[test]
fn resolve_sibling_requires_whitespace_after_markers() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("###Worker", &sections)
        .expect_err("no whitespace after markers must error");
    assert!(err.to_string().contains("whitespace"), "error was: {err}");
}

#[test]
fn resolve_sibling_marker_only_heading_errors_as_nameless() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("### ", &sections).expect_err("a marker-only heading must error");
    assert!(err.to_string().contains("has no name"), "error was: {err}");
}

#[test]
fn resolve_sibling_requires_exact_level() {
    let sections = vec![sibling("Worker", 3)];
    // Same name, wrong marker level, must not resolve.
    let err = resolve_sibling("## Worker", &sections)
        .expect_err("a level mismatch must not resolve by name alone");
    assert!(err.to_string().contains("not found"), "error was: {err}");
    // The exact address resolves.
    let ok = resolve_sibling("### Worker", &sections).expect("exact address resolves");
    assert_eq!(ok.name, "Worker");
}

#[test]
fn resolve_sibling_rejects_more_than_one_match() {
    let sections = vec![sibling("Worker", 3), sibling("Worker", 3)];
    let err = resolve_sibling("### Worker", &sections)
        .expect_err("two identical siblings must be rejected as ambiguous");
    assert!(err.to_string().contains("ambiguous"), "error was: {err}");
}

/// Forwards each observation over the channel, so the finalizer test asserts
/// on arrival order.
struct ChannelObserver {
    tx: mpsc::Sender<(String, Observation)>,
}

impl Observer for ChannelObserver {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        let _ = self.tx.try_send((section.to_owned(), event));
    }
}

#[test]
fn arm_finalizer_emits_cancelled_on_drop_unless_finished() {
    // FANOUT-004/006: the guard emits exactly one terminal event. Dropped
    // without finishing => cancelled; finished => only that event.
    let (tx, mut rx) = mpsc::channel::<(String, Observation)>(8);
    let observer: Arc<dyn Observer> = Arc::new(ChannelObserver { tx });

    drop(ArmFinalizer::new(
        Arc::clone(&observer),
        "exec".to_string(),
        "S".to_string(),
    ));
    let (_, event) = rx.try_recv().expect("a dropped finalizer emits an event");
    assert_eq!(event, detail::FANOUT_ARM_CANCELLED);
    assert!(rx.try_recv().is_err(), "exactly one terminal event on drop");

    let mut finalizer =
        ArmFinalizer::new(Arc::clone(&observer), "exec".to_string(), "S".to_string());
    finalizer.finish(detail::FANOUT_ARM_SUCCEEDED);
    drop(finalizer);
    let (_, event) = rx.try_recv().expect("finish emits its event");
    assert_eq!(event, detail::FANOUT_ARM_SUCCEEDED);
    assert!(
        rx.try_recv().is_err(),
        "a finished finalizer does not also emit cancelled on drop"
    );
}

// --- collection conversion -------------------------------------------------

fn eval(lua: &mlua::Lua, source: &str) -> Value {
    lua.load(source).eval::<Value>().expect("chunk evaluates")
}

#[test]
fn collection_to_items_rejects_a_non_table() {
    let lua = mlua::Lua::new();
    for source in ["return '### Items'", "return 5", "return true"] {
        let value = eval(&lua, source);
        let error = collection_to_items(&lua, &value).expect_err("a non-table is not a collection");
        assert!(
            error.to_string().contains("list_from_section"),
            "the error must point at list_from_section for {source}: {error}"
        );
    }
}

#[test]
fn collection_to_items_preserves_array_order_and_member_types() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {'b', 2, true, {nested='x'}}");
    let items = collection_to_items(&lua, &value).expect("a mixed array converts");
    assert_eq!(
        items,
        vec![json!("b"), json!(2), json!(true), json!({"nested": "x"})]
    );
}

#[test]
fn collection_to_items_wraps_hash_members_as_pair_tables() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {alpha=1, beta='two'}");
    let mut items = collection_to_items(&lua, &value).expect("a hash table converts");
    // The hash part's order is undefined; sort for the comparison.
    items.sort_by_key(ToString::to_string);
    assert_eq!(
        items,
        vec![
            json!({"key": "alpha", "value": 1}),
            json!({"key": "beta", "value": "two"})
        ]
    );
}

#[test]
fn collection_to_items_emits_the_array_part_before_the_hash_part() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {'a', 'b', extra='c'}");
    let items = collection_to_items(&lua, &value).expect("a mixed table converts");
    assert_eq!(
        items,
        vec![
            json!("a"),
            json!("b"),
            json!({"key": "extra", "value": "c"})
        ]
    );
}

#[test]
fn collection_to_items_keeps_integer_keys_outside_the_border_as_pairs() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {[5]='five'}");
    let items = collection_to_items(&lua, &value).expect("a sparse table converts");
    assert_eq!(items, vec![json!({"key": 5, "value": "five"})]);
}

#[test]
fn collection_to_items_returns_an_empty_vec_for_an_empty_table() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {}");
    let items = collection_to_items(&lua, &value).expect("an empty table converts");
    assert!(items.is_empty());
}

#[test]
fn collection_to_items_rejects_a_function_member_naming_its_index() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "return {'a', function() end}");
    let error = collection_to_items(&lua, &value).expect_err("a function member must error");
    let rendered = error.to_string();
    assert!(rendered.contains("index 2"), "error was: {rendered}");
    assert!(rendered.contains("function"), "error was: {rendered}");

    let value = eval(&lua, "return {cb=function() end}");
    let error =
        collection_to_items(&lua, &value).expect_err("a hash-position function member must error");
    let rendered = error.to_string();
    assert!(rendered.contains("index cb"), "error was: {rendered}");
    assert!(rendered.contains("function"), "error was: {rendered}");
}

struct Stub;
impl mlua::UserData for Stub {}

#[test]
fn collection_to_items_rejects_a_userdata_member_naming_its_index() {
    let lua = mlua::Lua::new();
    let userdata = lua.create_userdata(Stub).expect("userdata creates");
    let table = lua.create_table().expect("table creates");
    table.raw_set(1, userdata).expect("member installs");
    let error =
        collection_to_items(&lua, &Value::Table(table)).expect_err("a userdata member must error");
    let rendered = error.to_string();
    assert!(rendered.contains("index 1"), "error was: {rendered}");
    assert!(rendered.contains("userdata"), "error was: {rendered}");
}

#[test]
fn collection_to_items_rejects_a_non_scalar_key() {
    let lua = mlua::Lua::new();
    let value = eval(&lua, "local t = {}; t[{}] = 'x'; return t");
    let error = collection_to_items(&lua, &value).expect_err("a table key must error");
    assert!(
        error
            .to_string()
            .contains("key must be a string, number, or boolean"),
        "error was: {error}"
    );
}
