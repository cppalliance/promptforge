//! Tests for the `models` namespace: `use`, `default`, `get`, and the model runtime selection.

use super::{ModelRuntime, UseOptions, install_models};
use mlua::Lua;
use promptforge_model_client::model::ModelBinding;
use promptforge_model_client::model::{ModelInvocation, ModelSet, Temperature};
use promptforge_types::detail::model_id_from_validated;
use std::sync::{Arc, Mutex};

/// The runtime's selected label, if any.
fn used(runtime: &ModelRuntime) -> Option<&str> {
    runtime.selection().map(|(label, _)| label)
}

#[test]
fn model_runtime_select_allows_reselection() {
    // Selections are read at call time, so a later `models.use` replaces the
    // earlier one and steers the next model round.
    let mut runtime = ModelRuntime::new();
    assert!(used(&runtime).is_none());
    runtime.select("writer".to_owned(), UseOptions::default());
    assert_eq!(used(&runtime), Some("writer"));
    runtime.select("other".to_owned(), UseOptions::default());
    assert_eq!(
        used(&runtime),
        Some("other"),
        "a second select must replace the first"
    );
}

#[test]
fn model_runtime_starts_with_no_selection() {
    let runtime = ModelRuntime::new();
    assert!(used(&runtime).is_none(), "fresh runtime has no selection");
}

/// A bound role for the shared set: `label`, with the keyword set recorded.
fn bound_role(label: &str, capabilities: &[&str]) -> ModelBinding {
    ModelBinding::new(
        label,
        "A general model for tests",
        model_id_from_validated("gateway", "m1"),
        ModelInvocation {
            temperature: None,
            max_tokens: None,
            thinking: None,
        },
        std::num::NonZeroU32::new(8192).expect("8192 is non-zero"),
    )
    .with_capabilities(capabilities.iter().map(|word| (*word).to_owned()).collect())
}

/// A fresh VM with the `models` table installed over a shared set holding
/// the `writer` and `critic` roles.
fn models_vm() -> (Lua, Arc<Mutex<ModelSet>>, Arc<Mutex<ModelRuntime>>) {
    let lua = Lua::new();
    let set = Arc::new(Mutex::new(ModelSet::from_parts(
        vec![
            bound_role("writer", &["frontier", "thinking"]),
            bound_role("critic", &["fast"]),
        ],
        None,
    )));
    let runtime = Arc::new(Mutex::new(ModelRuntime::new()));
    install_models(&lua, &lua.globals(), &set, &runtime, false)
        .expect("the models install cannot fail on a fresh VM");
    (lua, set, runtime)
}

#[test]
fn the_models_namespace_has_no_bind() {
    let (lua, _, _) = models_vm();
    let (bind_is_nil, has_use, has_default, has_get, has_infer): (bool, bool, bool, bool, bool) =
        lua.load(
            "return models.bind == nil, \
                    type(models.use) == 'function', \
                    type(models.default) == 'function', \
                    type(models.get) == 'function', \
                    type(models.infer) == 'function'",
        )
        .eval()
        .expect("the namespace probe evaluates");
    assert!(bind_is_nil && has_use && has_default && has_get && has_infer);
}

#[test]
fn models_use_selects_a_bound_role_by_label() {
    let (lua, _, runtime) = models_vm();
    let handle: String = lua
        .load("local h = models.use('writer'); return h.label .. '|' .. h.name")
        .eval()
        .expect("a bound label selects");
    assert_eq!(handle, "writer|writer");
    assert_eq!(used(&runtime.lock().expect("runtime lock")), Some("writer"));
}

#[test]
fn models_use_rejects_an_unbound_label() {
    let (lua, _, _) = models_vm();
    let error = lua
        .load("models.use('ghost')")
        .exec()
        .expect_err("an unbound label is a hard error");
    assert!(
        error
            .to_string()
            .contains("models.use label \"ghost\" is not a bound model role"),
        "the rejection names the label: {error}"
    );
}

#[test]
fn models_use_options_reach_the_returned_handle() {
    let (lua, _, runtime) = models_vm();
    let (temperature, max_tokens): (f64, u32) = lua
        .load(
            "local h = models.use('writer', { temperature = 0.3, max_tokens = 256 }); \
             return h.temperature, h.max_tokens",
        )
        .eval()
        .expect("a valid options table is accepted");
    assert!((temperature - 0.3).abs() < f64::EPSILON);
    assert_eq!(max_tokens, 256);
    assert_eq!(used(&runtime.lock().expect("runtime lock")), Some("writer"));
}

#[test]
fn models_use_accepts_an_integer_temperature_and_leaves_omitted_fields_nil() {
    let (lua, _, _) = models_vm();
    let (temperature, max_tokens_is_nil): (f64, bool) = lua
        .load("local h = models.use('writer', { temperature = 0 }); return h.temperature, h.max_tokens == nil")
        .eval()
        .expect("an integer temperature is accepted");
    assert!(temperature.abs() < f64::EPSILON);
    assert!(
        max_tokens_is_nil,
        "an omitted field keeps the role's default"
    );
}

#[test]
fn models_use_rejects_invalid_options_without_selecting() {
    let cases = [
        (
            "models.use('writer', { temperature = 2.5 })",
            "models.use option temperature 2.5 is outside the supported range [0.0, 2.0]",
        ),
        (
            "models.use('writer', { temperature = -0.1 })",
            "models.use option temperature -0.1 is outside the supported range [0.0, 2.0]",
        ),
        (
            "models.use('writer', { temperature = 0/0 })",
            "models.use option temperature must be finite, got NaN",
        ),
        (
            "models.use('writer', { temperature = 'hot' })",
            "models.use option temperature must be a number, got string",
        ),
        (
            "models.use('writer', { max_tokens = 0 })",
            "models.use option max_tokens must be an integer in [1, 4294967295], got 0",
        ),
        (
            "models.use('writer', { max_tokens = -1 })",
            "models.use option max_tokens must be an integer in [1, 4294967295], got -1",
        ),
        (
            "models.use('writer', { max_tokens = 1.5 })",
            "models.use option max_tokens must be an integer in [1, 4294967295], got 1.5",
        ),
        (
            "models.use('writer', { max_tokens = 'many' })",
            "models.use option max_tokens must be an integer in [1, 4294967295], got string",
        ),
        (
            "models.use('writer', { top_p = 0.9 })",
            "models.use option \"top_p\" is unknown: expected temperature or max_tokens",
        ),
        (
            "models.use('writer', { 0.5 })",
            "models.use option names must be strings, got integer",
        ),
        (
            "models.use('writer', 'fast')",
            "models.use options must be a table, got string",
        ),
        (
            "models.use('writer', {}, 1)",
            "models.use takes at most 2 arguments, got 3",
        ),
    ];
    for (chunk, expected) in cases {
        let (lua, _, runtime) = models_vm();
        let error = lua
            .load(chunk)
            .exec()
            .expect_err("an invalid options argument is a hard error");
        assert!(
            error.to_string().contains(expected),
            "{chunk}: expected {expected:?} in {error}"
        );
        assert!(
            used(&runtime.lock().expect("runtime lock")).is_none(),
            "{chunk}: a rejected call must not select"
        );
    }
}

#[test]
fn models_use_reports_the_first_bad_option_in_key_order_on_every_state() {
    // Each fresh state walks `pairs` under its own hash seed; the rejection
    // must not follow that walk.
    let cases = [
        (
            "models.use('writer', { top_p = 1, max_tokens = 0, temperature = 3 })",
            "models.use option max_tokens must be an integer in [1, 4294967295], got 0",
        ),
        (
            "models.use('writer', { temperature = 3, top_p = 1 })",
            "models.use option temperature 3 is outside the supported range [0.0, 2.0]",
        ),
        (
            "models.use('writer', { 'x', [true] = 1, temperature = 3 })",
            "models.use option names must be strings, got boolean",
        ),
    ];
    for (chunk, expected) in cases {
        for _ in 0..32 {
            let (lua, _, _) = models_vm();
            let error = lua
                .load(chunk)
                .exec()
                .expect_err("an invalid options table is a hard error");
            assert!(
                error.to_string().contains(expected),
                "{chunk}: expected {expected:?} in {error}"
            );
        }
    }
}

/// The section's effective binding's `(temperature, max_tokens)`, read the
/// way the engine's Chat-effect sites read it.
fn effective_sampling(
    set: &std::sync::Mutex<ModelSet>,
    runtime: &std::sync::Mutex<ModelRuntime>,
) -> (Option<f64>, Option<u32>) {
    let binding = crate::resolve_model_binding(set, runtime)
        .expect("the model state is readable")
        .expect("a selection resolves");
    let invocation = binding.invocation();
    (
        invocation.temperature.map(Temperature::get),
        invocation.max_tokens.map(std::num::NonZeroU32::get),
    )
}

#[test]
fn the_selection_carries_its_options_until_a_later_models_use_replaces_them() {
    let (lua, set, runtime) = models_vm();
    lua.load("models.use('writer', { temperature = 0.5, max_tokens = 64 })")
        .exec()
        .expect("a valid options table is accepted");
    assert_eq!(effective_sampling(&set, &runtime), (Some(0.5), Some(64)));

    let (temperature_is_nil, max_tokens_is_nil): (bool, bool) = lua
        .load("local h = models.use('writer'); return h.temperature == nil, h.max_tokens == nil")
        .eval()
        .expect("a plain models.use selects");
    assert!(temperature_is_nil && max_tokens_is_nil);
    assert_eq!(
        effective_sampling(&set, &runtime),
        (None, None),
        "a plain models.use clears the earlier options"
    );
}

#[test]
fn a_models_get_handle_for_the_selected_label_keeps_the_role_defaults() {
    let (lua, _, _) = models_vm();
    let (temperature_is_nil, max_tokens_is_nil): (bool, bool) = lua
        .load(
            "models.use('writer', { temperature = 0.5, max_tokens = 64 }); \
             local h = models.get('writer'); \
             return h.temperature == nil, h.max_tokens == nil",
        )
        .eval()
        .expect("models.get inspects the bound role");
    assert!(temperature_is_nil && max_tokens_is_nil);
}

#[test]
fn models_default_takes_a_label_and_parks_the_prompt_wide_default() {
    let (lua, set, _) = models_vm();
    lua.load("models.default('writer')")
        .exec()
        .expect("a bound label becomes the default");
    assert_eq!(
        set.lock().expect("set lock").default.as_deref(),
        Some("writer")
    );
}

#[test]
fn models_default_is_idempotent_for_the_same_label_and_refuses_a_change() {
    let (lua, set, _) = models_vm();
    // The shared library replays into every section, so re-naming the same
    // default must be a no-op.
    lua.load("models.default('writer'); models.default('writer')")
        .exec()
        .expect("re-naming the same default is a no-op");
    assert_eq!(
        set.lock().expect("set lock").default.as_deref(),
        Some("writer")
    );
    let error = lua
        .load("models.default('critic')")
        .exec()
        .expect_err("the prompt-wide default cannot change mid-run");
    assert!(
        error
            .to_string()
            .contains("models.default is already \"writer\""),
        "the refusal names the parked default: {error}"
    );
}

#[test]
fn models_default_rejects_an_unbound_label() {
    let (lua, _, _) = models_vm();
    let error = lua
        .load("models.default('ghost')")
        .exec()
        .expect_err("an unbound label is a hard error");
    assert!(
        error
            .to_string()
            .contains("models.default label \"ghost\" is not a bound model role"),
        "the rejection names the label: {error}"
    );
}

#[test]
fn the_handle_exposes_label_and_the_full_keyword_set() {
    let (lua, _, _) = models_vm();
    let inspected: String = lua
        .load(
            "local h = models.get('writer'); \
             return h.label .. '|' .. h.model_id .. '|' .. table.concat(h.capabilities, ',')",
        )
        .eval()
        .expect("the handle inspects");
    assert_eq!(inspected, "writer|m1|frontier,thinking");
}

/// Builds a section VM with the Agent-window raw-id opt-in as `raw_ids`,
/// host values injected (which installs the `models` table).
fn h2_vm(raw_ids: bool) -> crate::SectionVm {
    let emitter = crate::tests::recording::null_emitter();
    let mut vm = crate::SectionVm::new(
        &promptforge_types::untrusted::GuardNonce::from_seed(1),
        &emitter,
        "S",
    )
    .expect("the VM builds");
    if raw_ids {
        vm.allow_raw_model_ids();
    }
    vm.inject_host(
        "",
        &serde_json::json!({}),
        &std::sync::Arc::new(
            promptforge_vfs::empty()
                .acquire(promptforge_vfs::Origin::new("models test fixture"))
                .expect("the stock backend acquires"),
        ),
    )
    .expect("host injection installs the models table");
    vm
}

#[test]
fn models_get_resolves_an_undeclared_alias_as_a_raw_gateway_id_only_when_permitted() {
    let vm = h2_vm(true);
    let resolved: String = vm
        .lua()
        .load("local h = models.get('qwen/qwen3-8b'); return h.name .. '|' .. h.model_id .. '|' .. h.context")
        .eval()
        .expect("the raw-id opt-in resolves the undeclared alias");
    assert_eq!(
        resolved, "qwen/qwen3-8b|qwen/qwen3-8b|8192",
        "the handle freezes the raw gateway id under the fallback context window"
    );

    let vm = h2_vm(false);
    let error = vm
        .lua()
        .load("models.get('ghost')")
        .exec()
        .expect_err("without the opt-in an undeclared alias is an error");
    assert!(
        error.to_string().contains("is not a bound model role"),
        "the strict path names the bound-role rule: {error}"
    );
}

#[test]
fn the_raw_id_fallback_still_validates_the_gateway_id() {
    let vm = h2_vm(true);
    let error = vm
        .lua()
        .load("models.get('bad\\nid')")
        .exec()
        .expect_err("a control character fails the id's own validation");
    assert!(
        error.to_string().contains("is invalid"),
        "the raw path validates as a gateway id, not as an alias: {error}"
    );
}
