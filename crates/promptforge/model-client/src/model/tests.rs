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
