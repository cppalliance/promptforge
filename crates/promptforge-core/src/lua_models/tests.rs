use super::decode::{
    decode_lua_number, parse_need_args, parse_opts_table, parse_single_alias, validate_alias,
    value_as_bool, value_as_nonzero_u32, value_as_temperature, value_as_u32,
};
use super::userdata::{LuaModelHandle, reject_infer_options};
use super::{ModelBindingState, ModelRuntime, record_only_binding};
use crate::dialects::ToolDialectId;
use crate::model::{ModelId, ModelInvocation, ModelNeedOpts};
use mlua::Value;
use mlua::{Lua, MultiValue};

fn test_binding(alias: &str, dialect: ToolDialectId) -> ModelBinding {
    ModelBinding::new(
        alias,
        "a test capability",
        ModelId::from_validated("gateway", "m1"),
        ModelInvocation::from(&ModelNeedOpts::default()),
        dialect,
        std::num::NonZeroU32::new(8192).expect("8192 is non-zero"),
    )
}

#[test]
fn temperature_accepts_finite_in_domain_and_rejects_the_rest() {
    for good in [0.0, 0.7, 1.0, 2.0] {
        let got = value_as_temperature(&Value::Number(good))
            .expect("in-domain temperature")
            .get();
        assert!(
            (got - good).abs() <= f64::EPSILON,
            "temperature {good} must pass through unchanged, got {got}"
        );
    }
    for bad in [-0.1, 2.5, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            value_as_temperature(&Value::Number(bad)).is_err(),
            "temperature {bad} must be rejected"
        );
    }
}

#[test]
fn integer_and_number_temperatures_share_one_decode_and_domain_check() {
    // The Lua integer form is decoded through the same path as the number
    // form (no separate i32 gate) and validated by the same domain check.
    let from_integer = value_as_temperature(&Value::Integer(1))
        .expect("integer 1 is in-domain")
        .get();
    let from_number = value_as_temperature(&Value::Number(1.0))
        .expect("number 1.0 is in-domain")
        .get();
    assert!((from_integer - from_number).abs() <= f64::EPSILON);
    assert!(
        value_as_temperature(&Value::Integer(5)).is_err(),
        "an out-of-domain integer temperature must be rejected by the one domain check"
    );
}

#[test]
fn dialect_getter_returns_the_closed_dialect_id() {
    // PF-LM-011: the Rust getter returns the closed identity type, not a
    // freshly allocated String.
    let handle = LuaModelHandle::from_binding(&test_binding("a", ToolDialectId::OpenAi));
    assert_eq!(handle.dialect(), ToolDialectId::OpenAi);
}

#[test]
fn only_multi_arg_rolls_back_when_already_selected() {
    // PF-LM-003: a second multi-arg `models.only` must be rejected WITHOUT
    // leaving a half-recorded binding behind.
    let resolver = |_: &str, _: &ModelNeedOpts| {
        Ok(crate::model::ResolvedModel {
            id: ModelId::from_validated("gateway", "m1"),
            invocation: ModelInvocation::from(&ModelNeedOpts::default()),
            tool_dialect: ToolDialectId::OpenAi,
            context: std::num::NonZeroU32::new(8192).expect("8192 is non-zero"),
        })
    };
    let mut state = ModelBindingState::default();
    record_only_binding(
        &mut state,
        &resolver,
        "a",
        "desc",
        &ModelNeedOpts::default(),
    )
    .expect("the first models.only must succeed");
    assert_eq!(state.bindings.len(), 1);
    assert_eq!(state.only.as_deref(), Some("a"));

    let err = record_only_binding(
        &mut state,
        &resolver,
        "b",
        "desc",
        &ModelNeedOpts::default(),
    )
    .expect_err("a second models.only must be rejected");
    assert!(
        err.to_string().contains("at most once"),
        "error must explain the at-most-once rule: {err}"
    );
    assert_eq!(
        state.bindings.len(),
        1,
        "a rejected second models.only must not record a binding (rollback)"
    );
    assert_eq!(state.only.as_deref(), Some("a"));
}

#[test]
fn model_runtime_select_enforces_at_most_once() {
    let mut runtime = ModelRuntime::new();
    assert!(runtime.used().is_none());
    runtime.select("writer".to_owned()).expect("first select ok");
    assert_eq!(runtime.used(), Some("writer"));
    assert!(
        runtime.select("other".to_owned()).is_err(),
        "a second select must be rejected"
    );
}

#[test]
fn infer_options_absent_or_nil_are_accepted() {
    assert!(reject_infer_options(None).is_ok());
    assert!(reject_infer_options(Some(&Value::Nil)).is_ok());
}

#[test]
fn infer_options_reject_a_table_or_any_non_nil_value() {
    let boolean = Value::Boolean(true);
    let integer = Value::Integer(1);
    for value in [&boolean, &integer] {
        let error = reject_infer_options(Some(value))
            .expect_err("a non-nil infer options argument must be rejected");
        assert!(
            error
                .to_string()
                .contains("does not accept a second argument"),
            "error must explain the rejection, got: {error}"
        );
    }
}

// PF-LM-014: direct coverage of every parser branch and state transition.

fn lua_string(lua: &Lua, value: &str) -> Value {
    Value::String(lua.create_string(value).expect("create Lua string"))
}

#[test]
fn parse_need_args_covers_each_branch() {
    let lua = Lua::new();
    // Missing description.
    let one: MultiValue = [lua_string(&lua, "writer")].into_iter().collect();
    assert!(parse_need_args(one).is_err(), "one argument is rejected");
    // Non-string alias.
    let bad_alias: MultiValue = [Value::Integer(1), lua_string(&lua, "desc")]
        .into_iter()
        .collect();
    assert!(
        parse_need_args(bad_alias).is_err(),
        "non-string alias fails"
    );
    // opts not a table.
    let bad_opts: MultiValue = [
        lua_string(&lua, "writer"),
        lua_string(&lua, "desc"),
        Value::Integer(3),
    ]
    .into_iter()
    .collect();
    assert!(parse_need_args(bad_opts).is_err(), "non-table opts fails");
    // Too many arguments.
    let too_many: MultiValue = [
        lua_string(&lua, "writer"),
        lua_string(&lua, "desc"),
        Value::Nil,
        Value::Nil,
    ]
    .into_iter()
    .collect();
    assert!(parse_need_args(too_many).is_err(), "four arguments fail");
    // Valid two-argument form.
    let ok: MultiValue = [lua_string(&lua, "writer"), lua_string(&lua, "desc")]
        .into_iter()
        .collect();
    let (alias, description, opts) = parse_need_args(ok).expect("valid need args");
    assert_eq!(alias, "writer");
    assert_eq!(description, "desc");
    assert_eq!(opts.temperature, None);
}

#[test]
fn parse_opts_table_covers_each_key_and_rejects_unknown() {
    let lua = Lua::new();
    let table = lua.create_table().expect("table");
    table.set("thinking", true).expect("set thinking");
    table.set("context", 8192).expect("set context");
    table.set("temperature", 0.5).expect("set temperature");
    table.set("max_tokens", 256).expect("set max_tokens");
    let opts = parse_opts_table(&table).expect("all known keys parse");
    assert_eq!(opts.thinking, Some(true));
    assert_eq!(opts.context.map(std::num::NonZeroU32::get), Some(8192));
    assert_eq!(
        opts.temperature.map(crate::model::Temperature::get),
        Some(0.5)
    );
    assert_eq!(opts.max_tokens.map(std::num::NonZeroU32::get), Some(256));

    // MODEL-003: a zero count is rejected at the parse boundary, not stored.
    let zero_context = lua.create_table().expect("table");
    zero_context.set("context", 0).expect("set context");
    assert!(
        parse_opts_table(&zero_context).is_err(),
        "a zero context minimum must be rejected"
    );
    let zero_max = lua.create_table().expect("table");
    zero_max.set("max_tokens", 0).expect("set max_tokens");
    assert!(
        parse_opts_table(&zero_max).is_err(),
        "a zero max_tokens cap must be rejected"
    );

    let unknown = lua.create_table().expect("table");
    unknown.set("bogus", 1).expect("set bogus");
    assert!(
        parse_opts_table(&unknown).is_err(),
        "an unknown opts key must be rejected"
    );

    let non_string_key = lua.create_table().expect("table");
    non_string_key.set(1, "x").expect("set numeric key");
    assert!(
        parse_opts_table(&non_string_key).is_err(),
        "a non-string opts key must be rejected"
    );
}

#[test]
fn scalar_decoders_cover_valid_and_invalid_inputs() {
    assert!(value_as_bool(&Value::Boolean(false), "thinking").is_ok());
    assert!(value_as_bool(&Value::Integer(1), "thinking").is_err());

    assert_eq!(value_as_u32(&Value::Integer(7), "context").expect("ok"), 7);
    assert_eq!(
        value_as_u32(&Value::Number(9.0), "context").expect("whole number ok"),
        9
    );
    assert!(value_as_u32(&Value::Integer(-1), "context").is_err());
    assert!(value_as_u32(&Value::Number(1.5), "context").is_err());
    assert!(value_as_u32(&Value::Boolean(true), "context").is_err());

    // MODEL-003: the non-zero decoder accepts positive counts and rejects zero.
    assert_eq!(
        value_as_nonzero_u32(&Value::Integer(7), "context")
            .expect("positive count")
            .get(),
        7
    );
    assert!(
        value_as_nonzero_u32(&Value::Integer(0), "context").is_err(),
        "a zero count must be rejected"
    );

    assert!((decode_lua_number(&Value::Integer(2), "t").expect("int") - 2.0).abs() < f64::EPSILON);
    assert!(decode_lua_number(&Value::Boolean(true), "t").is_err());
}

#[test]
fn parse_single_alias_and_validate_alias_branches() {
    let lua = Lua::new();
    let ok: MultiValue = [lua_string(&lua, "writer")].into_iter().collect();
    assert_eq!(
        parse_single_alias(&ok, "models.only").expect("string alias"),
        "writer"
    );
    let bad: MultiValue = [Value::Integer(1)].into_iter().collect();
    assert!(
        parse_single_alias(&bad, "models.only").is_err(),
        "a non-string alias must be rejected"
    );

    assert!(validate_alias("Writer_1-x").is_ok());
    assert!(validate_alias("").is_err(), "empty alias rejected");
    assert!(validate_alias("1abc").is_err(), "leading digit rejected");
    assert!(validate_alias("a b").is_err(), "space rejected");
}

#[test]
fn model_runtime_starts_with_no_selection() {
    let runtime = ModelRuntime::new();
    assert!(runtime.used().is_none(), "fresh runtime has no selection");
}
