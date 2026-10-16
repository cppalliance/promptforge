//! Tests for model bindings and invocation identity.

use std::num::NonZeroU32;

use super::*;

fn ctx(window: u32) -> NonZeroU32 {
    NonZeroU32::new(window).expect("test context window is non-zero")
}

fn gateway_id(name: &str) -> ModelId {
    ModelId::gateway(name).expect("test model alias is valid")
}

fn binding(alias: &str) -> ModelBinding {
    ModelBinding::new(
        alias,
        "a model",
        gateway_id(alias),
        ModelInvocation::new(),
        ctx(8_192),
    )
}

#[test]
fn same_weights_different_invocation_compare_unequal() {
    let id = gateway_id("analyst");
    let a = ModelBinding::new(
        "cool",
        "careful analysis",
        id.clone(),
        ModelInvocation::new()
            .with_temperature(Some(Temperature::new(0.0).expect("0.0 is valid")))
            .with_thinking(Some(false)),
        ctx(131_072),
    );
    let b = ModelBinding::new(
        "warm",
        "careful analysis",
        id,
        ModelInvocation::new()
            .with_temperature(Some(Temperature::new(0.7).expect("0.7 is valid")))
            .with_thinking(Some(false)),
        ctx(131_072),
    );
    assert_eq!(a.id(), b.id());
    assert_ne!(a.invocation(), b.invocation());
}

#[test]
fn a_new_invocation_sets_nothing_and_each_setter_sets_its_field() {
    let bare = ModelInvocation::new();
    assert_eq!(bare, ModelInvocation::default());
    assert_eq!(
        (bare.temperature, bare.max_tokens, bare.thinking),
        (None, None, None)
    );
    let cap = NonZeroU32::new(256).expect("256 is non-zero");
    let set = ModelInvocation::new()
        .with_temperature(Some(Temperature::new(0.2).expect("0.2 is valid")))
        .with_max_tokens(Some(cap))
        .with_thinking(Some(true));
    assert_eq!(set.temperature.map(Temperature::get), Some(0.2));
    assert_eq!(set.max_tokens, Some(cap));
    assert_eq!(set.thinking, Some(true));
}

#[test]
fn binding_construction_is_atomic_with_context() {
    let binding = ModelBinding::new(
        "remote",
        "a remote model",
        gateway_id("remote"),
        ModelInvocation::new(),
        ctx(64_000),
    );
    let opts = binding.completion_options();
    assert_eq!(opts.model, "remote");
    assert_eq!(binding.context().get(), 64_000);
}

#[test]
fn a_model_set_grows_by_push_and_takes_its_default_alias_by_setter() {
    let mut set = ModelSet::default();
    assert!(set.bindings().is_empty());
    assert_eq!(set.default_alias(), None);
    set.push(binding("writer"));
    set.push(binding("critic"));
    set.set_default_alias("writer".to_owned());
    assert_eq!(
        set.bindings()
            .iter()
            .map(ModelBinding::alias)
            .collect::<Vec<_>>(),
        ["writer", "critic"],
        "push keeps declaration order"
    );
    assert_eq!(set.default_alias(), Some("writer"));
    assert_eq!(
        set,
        ModelSet::from_parts(
            vec![binding("writer"), binding("critic")],
            Some("writer".to_owned())
        )
    );
}

#[test]
fn completion_options_read_back_every_field_they_were_built_with() {
    let bare = CompletionOptions::new("analyst");
    assert_eq!(bare.model(), "analyst");
    assert_eq!(bare.temperature(), None);
    assert_eq!(bare.max_tokens(), None);
    assert_eq!(bare.thinking(), None);

    let set = CompletionOptions::new("analyst")
        .with_temperature(0.2)
        .expect("0.2 is valid")
        .with_max_tokens(NonZeroU32::new(256).expect("256 is non-zero"))
        .with_thinking(false);
    assert_eq!(set.model(), "analyst");
    assert_eq!(set.temperature().map(Temperature::get), Some(0.2));
    assert_eq!(set.max_tokens(), NonZeroU32::new(256));
    assert_eq!(set.thinking(), Some(false));
}

#[test]
fn with_model_replaces_the_model_name_and_keeps_every_other_field() {
    let picked = CompletionOptions::new("bound")
        .with_temperature(0.2)
        .expect("0.2 is valid")
        .with_max_tokens(NonZeroU32::new(256).expect("256 is non-zero"))
        .with_thinking(true)
        .with_model("picked");
    assert_eq!(picked.model(), "picked");
    assert_eq!(picked.temperature().map(Temperature::get), Some(0.2));
    assert_eq!(picked.max_tokens(), NonZeroU32::new(256));
    assert_eq!(picked.thinking(), Some(true));
}
