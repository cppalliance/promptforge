//! Tests for the `models` namespace: `use`, `default`, `get`, and the model runtime selection.

use super::{ModelRuntime, UseOptions};
use crate::alias::Alias;
use crate::test_vm::{BuiltVm, TestVm};
use promptforge_model_client::model::ModelBinding;
use promptforge_model_client::model::{ModelId, ModelInvocation, ModelSet, Temperature};
use std::sync::Arc;

/// The validated alias `text` names.
#[track_caller]
fn alias(text: &str) -> Alias {
    Alias::parse(text.to_owned()).expect("a valid alias")
}

/// The runtime's selected label, if any.
fn used(runtime: &ModelRuntime) -> Option<&str> {
    runtime.selection().map(|(label, _)| label.as_str())
}

#[test]
fn model_runtime_select_allows_reselection() {
    // Selections are read at call time, so a later `models.use` replaces the
    // earlier one and steers the next model round.
    let mut runtime = ModelRuntime::new();
    assert!(used(&runtime).is_none());
    runtime.select(alias("writer"), UseOptions::default());
    assert_eq!(used(&runtime), Some("writer"));
    runtime.select(alias("other"), UseOptions::default());
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
        ModelId::gateway("m1").expect("a static model id is valid"),
        ModelInvocation::new(),
        std::num::NonZeroU32::new(8192).expect("8192 is non-zero"),
    )
    .with_capabilities(capabilities.iter().map(|word| (*word).to_owned()).collect())
}

/// A fresh VM with the `models` table installed over a shared set holding
/// the `writer` and `critic` roles.
#[track_caller]
fn models_vm() -> BuiltVm {
    TestVm::new()
        .models(ModelSet::from_parts(
            vec![
                bound_role("writer", &["frontier", "thinking"]),
                bound_role("critic", &["fast"]),
            ],
            None,
        ))
        .build()
}

#[test]
fn the_models_namespace_has_no_bind() {
    // `models.infer` suspends, so only the coroutine shim layer defines it.
    let vm = models_vm();
    let (bind_is_nil, has_use, has_default, has_get, infer_is_nil): (bool, bool, bool, bool, bool) =
        vm.eval(
            "return models.bind == nil, \
                    type(models.use) == 'function', \
                    type(models.default) == 'function', \
                    type(models.get) == 'function', \
                    models.infer == nil",
        );
    assert!(bind_is_nil && has_use && has_default && has_get);
    assert!(
        infer_is_nil,
        "install_models alone must not define models.infer"
    );
}

#[test]
fn a_handle_reads_nil_methods_where_the_shim_prelude_never_ran() {
    let vm = models_vm();
    let (infer_is_nil, loop_is_nil): (bool, bool) =
        vm.eval("local h = models.get('writer'); return h.infer == nil, h.loop == nil");
    assert!(
        infer_is_nil && loop_is_nil,
        "with no shim prelude a handle's infer and loop must read nil"
    );
}

#[test]
fn models_use_selects_a_bound_role_by_label() {
    let vm = models_vm();
    let handle: String = vm.eval("local h = models.use('writer'); return h.label .. '|' .. h.name");
    assert_eq!(handle, "writer|writer");
    assert_eq!(
        used(&vm.model_runtime.lock().expect("runtime lock")),
        Some("writer")
    );
}

#[test]
fn models_use_rejects_an_unbound_label() {
    let error = models_vm().eval_err("models.use('ghost')");
    assert!(
        error
            .to_string()
            .contains("models.use label \"ghost\" is not a bound model role"),
        "the rejection names the label: {error}"
    );
}

#[test]
fn models_use_options_reach_the_returned_handle() {
    let vm = models_vm();
    let (temperature, max_tokens): (f64, u32) = vm.eval(
        "local h = models.use('writer', { temperature = 0.3, max_tokens = 256 }); \
         return h.temperature, h.max_tokens",
    );
    assert!((temperature - 0.3).abs() < f64::EPSILON);
    assert_eq!(max_tokens, 256);
    assert_eq!(
        used(&vm.model_runtime.lock().expect("runtime lock")),
        Some("writer")
    );
}

#[test]
fn models_use_accepts_an_integer_temperature_and_leaves_omitted_fields_nil() {
    let (temperature, max_tokens_is_nil): (f64, bool) = models_vm().eval(
        "local h = models.use('writer', { temperature = 0 }); return h.temperature, h.max_tokens == nil",
    );
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
        let vm = models_vm();
        let error = vm.eval_err(chunk);
        assert!(
            error.to_string().contains(expected),
            "{chunk}: expected {expected:?} in {error}"
        );
        assert!(
            used(&vm.model_runtime.lock().expect("runtime lock")).is_none(),
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
            let error = models_vm().eval_err(chunk);
            assert!(
                error.to_string().contains(expected),
                "{chunk}: expected {expected:?} in {error}"
            );
        }
    }
}

/// The section's effective binding's `(temperature, max_tokens)`, read the
/// way the Engine's Chat-effect sites read it.
#[track_caller]
fn effective_sampling(vm: &BuiltVm) -> (Option<f64>, Option<u32>) {
    let binding = crate::resolve_model_binding(vm.models.as_ref(), &vm.model_runtime)
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
    let vm = models_vm();
    vm.eval::<()>("models.use('writer', { temperature = 0.5, max_tokens = 64 })");
    assert_eq!(effective_sampling(&vm), (Some(0.5), Some(64)));

    let (temperature_is_nil, max_tokens_is_nil): (bool, bool) =
        vm.eval("local h = models.use('writer'); return h.temperature == nil, h.max_tokens == nil");
    assert!(temperature_is_nil && max_tokens_is_nil);
    assert_eq!(
        effective_sampling(&vm),
        (None, None),
        "a plain models.use clears the earlier options"
    );
}

#[test]
fn a_models_get_handle_for_the_selected_label_keeps_the_role_defaults() {
    let (temperature_is_nil, max_tokens_is_nil): (bool, bool) = models_vm().eval(
        "models.use('writer', { temperature = 0.5, max_tokens = 64 }); \
         local h = models.get('writer'); \
         return h.temperature == nil, h.max_tokens == nil",
    );
    assert!(temperature_is_nil && max_tokens_is_nil);
}

#[test]
fn models_default_takes_a_label_and_parks_the_prompt_wide_default() {
    let vm = models_vm();
    vm.eval::<()>("models.default('writer')");
    assert_eq!(
        vm.models.lock().expect("set lock").default_alias(),
        Some("writer")
    );
}

#[test]
fn models_default_is_idempotent_for_the_same_label_and_refuses_a_change() {
    let vm = models_vm();
    // A library every section requires re-runs in each, so re-naming the
    // same default must be a no-op.
    vm.eval::<()>("models.default('writer'); models.default('writer')");
    assert_eq!(
        vm.models.lock().expect("set lock").default_alias(),
        Some("writer")
    );
    let error = vm.eval_err("models.default('critic')");
    assert!(
        error
            .to_string()
            .contains("models.default is already \"writer\""),
        "the refusal names the parked default: {error}"
    );
}

#[test]
fn models_default_rejects_an_unbound_label() {
    let error = models_vm().eval_err("models.default('ghost')");
    assert!(
        error
            .to_string()
            .contains("models.default label \"ghost\" is not a bound model role"),
        "the rejection names the label: {error}"
    );
}

#[test]
fn the_handle_exposes_label_and_the_full_keyword_set() {
    let inspected: String = models_vm().eval(
        "local h = models.get('writer'); \
         return h.label .. '|' .. h.model_id .. '|' .. table.concat(h.capabilities, ',')",
    );
    assert_eq!(inspected, "writer|m1|frontier,thinking");
}

#[test]
fn models_get_resolves_an_undeclared_alias_as_a_raw_gateway_id_only_when_permitted() {
    let resolved: String = TestVm::new().raw_model_ids().build().eval(
        "local h = models.get('qwen/qwen3-8b'); return h.name .. '|' .. h.model_id .. '|' .. h.context",
    );
    assert_eq!(
        resolved, "qwen/qwen3-8b|qwen/qwen3-8b|8192",
        "the handle freezes the raw gateway id under the fallback context window"
    );

    let error = TestVm::new().build().eval_err("models.get('ghost')");
    assert!(
        error.to_string().contains("is not a bound model role"),
        "the strict path names the bound-role rule: {error}"
    );
}

#[test]
fn the_raw_id_fallback_still_validates_the_gateway_id() {
    let error = TestVm::new()
        .raw_model_ids()
        .build()
        .eval_err("models.get('bad\\nid')");
    assert!(
        error.to_string().contains("is invalid"),
        "the raw path validates as a gateway id, not as an alias: {error}"
    );
}

#[test]
fn a_poisoned_model_set_reaches_the_caller_as_an_internal_error() {
    let vm = models_vm();
    vm.model_runtime
        .lock()
        .expect("runtime lock")
        .select(alias("writer"), UseOptions::default());
    let poisoner = Arc::clone(&vm.models);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock();
        panic!("poison the model set");
    })
    .join();

    let error = crate::resolve_model_binding(vm.models.as_ref(), &vm.model_runtime)
        .expect_err("a poisoned model set cannot resolve a binding");
    assert!(
        matches!(&error, crate::Error::Internal(message) if message == "model set mutex was poisoned"),
        "the lock failure is Engine misuse, neither an authoring nor a model failure: {error:?}"
    );
}
