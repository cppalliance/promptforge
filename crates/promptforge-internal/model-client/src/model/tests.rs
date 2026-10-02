//! Tests for model bindings and invocation identity.

use std::num::NonZeroU32;

use super::*;

fn ctx(window: u32) -> NonZeroU32 {
    NonZeroU32::new(window).expect("test context window is non-zero")
}

fn gateway_id(name: &str) -> ModelId {
    ModelId::gateway(name).expect("test model alias is valid")
}

#[test]
fn same_weights_different_invocation_compare_unequal() {
    let id = gateway_id("analyst");
    let a = ModelBinding::new(
        "cool",
        "careful analysis",
        id.clone(),
        ModelInvocation {
            temperature: Some(Temperature::new(0.0).expect("0.0 is valid")),
            max_tokens: None,
            thinking: Some(false),
        },
        ctx(131_072),
    );
    let b = ModelBinding::new(
        "warm",
        "careful analysis",
        id,
        ModelInvocation {
            temperature: Some(Temperature::new(0.7).expect("0.7 is valid")),
            max_tokens: None,
            thinking: Some(false),
        },
        ctx(131_072),
    );
    assert_eq!(a.id(), b.id());
    assert_ne!(a.invocation(), b.invocation());
}

#[test]
fn binding_construction_is_atomic_with_context() {
    let binding = ModelBinding::new(
        "remote",
        "a remote model",
        gateway_id("remote"),
        ModelInvocation {
            temperature: None,
            max_tokens: None,
            thinking: None,
        },
        ctx(64_000),
    );
    let opts = binding.completion_options();
    assert_eq!(opts.model, "remote");
    assert_eq!(binding.context().get(), 64_000);
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
