use super::*;

fn ctx(window: u32) -> NonZeroU32 {
    NonZeroU32::new(window).expect("test context window is non-zero")
}

fn gateway_id(name: &str) -> ModelId {
    ModelId::gateway(name).expect("test model alias is valid")
}

fn catalog() -> ModelCatalog {
    ModelCatalog::new([
        ModelDescriptor::new(
            gateway_id("small"),
            "A tiny model",
            ctx(8_192),
            ThinkingMode::Never,
        ),
        ModelDescriptor::new(
            gateway_id("analyst"),
            "A careful analysis model",
            ctx(131_072),
            ThinkingMode::Switchable,
        ),
        ModelDescriptor::new(
            gateway_id("always-think"),
            "Always thinks aloud",
            ctx(64_000),
            ThinkingMode::Always,
        ),
    ])
    .expect("test catalog has unique model ids")
}

#[test]
fn context_filter_drops_small_windows() {
    let catalog = catalog();
    let matches = catalog.filtered(&ModelBindOpts {
        context: Some(ctx(40_000)),
        ..ModelBindOpts::default()
    });
    let names: Vec<_> = matches.iter().map(|m| m.id().name()).collect();
    assert_eq!(names, ["analyst", "always-think"]);
}

#[test]
fn thinking_false_keeps_never_and_switchable() {
    let catalog = catalog();
    let matches = catalog.filtered(&ModelBindOpts {
        thinking: Some(false),
        ..ModelBindOpts::default()
    });
    let names: Vec<_> = matches.iter().map(|m| m.id().name()).collect();
    assert_eq!(names, ["small", "analyst"]);
}

#[test]
fn thinking_true_keeps_switchable_and_always() {
    let catalog = catalog();
    let matches = catalog.filtered(&ModelBindOpts {
        thinking: Some(true),
        ..ModelBindOpts::default()
    });
    let names: Vec<_> = matches.iter().map(|m| m.id().name()).collect();
    assert_eq!(names, ["analyst", "always-think"]);
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
fn model_picker_ids_round_trip_through_the_global_name_grammar() {
    for name in ["small", "analyst", "always-think"] {
        let id = gateway_id(name);
        let picker_id = model_to_picker_id(&id);
        // The neutral name segment keeps vendor ids out of `enriched_text`.
        assert_eq!(picker_id.name(), PICKER_MODEL_LABEL);
        assert_eq!(model_from_picker_id(&picker_id), id);
    }
}

#[test]
fn model_picker_ids_escape_characters_outside_the_grammar() {
    // Model id components are nearly arbitrary text: uppercase, `/`, and
    // spaces are all legal there but illegal in a global-name segment.
    let id = ModelId::new("Gateway.Local", "Qwen/Qwen3 32B").expect("a valid model id");
    let picker_id = model_to_picker_id(&id);
    let text = picker_id.to_string();
    assert!(
        text.bytes().all(|b| b.is_ascii_lowercase()
            || b.is_ascii_digit()
            || matches!(b, b'/' | b'-' | b'_' | b'.')),
        "the encoded id must satisfy the global-name charset: {text}"
    );
    assert_eq!(model_from_picker_id(&picker_id), id);
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
