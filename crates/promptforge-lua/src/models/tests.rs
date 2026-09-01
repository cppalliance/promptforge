use super::decode::{
    decode_lua_number, parse_bind_args, parse_opts_table, parse_single_alias, validate_alias,
    value_as_bool, value_as_nonzero_u32, value_as_temperature, value_as_u32,
};
use super::userdata::reject_infer_options;
use super::{ModelRuntime, record_default_binding};
use mlua::Value;
use mlua::{Lua, MultiValue};
use promptforge_model_client::model::{ModelBindOpts, ModelId, ModelInvocation, ModelSet};

#[test]
fn temperature_accepts_finite_in_domain_and_rejects_the_rest() {
    for good in [0.0, 0.7, 1.0, 2.0] {
        let got = value_as_temperature(&Value::Number(good), "models.bind")
            .expect("in-domain temperature")
            .get();
        assert!(
            (got - good).abs() <= f64::EPSILON,
            "temperature {good} must pass through unchanged, got {got}"
        );
    }
    for bad in [-0.1, 2.5, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            value_as_temperature(&Value::Number(bad), "models.bind").is_err(),
            "temperature {bad} must be rejected"
        );
    }
}

#[test]
fn integer_and_number_temperatures_share_one_decode_and_domain_check() {
    // The Lua integer form is decoded through the same path as the number
    // form (no separate i32 gate) and validated by the same domain check.
    let from_integer = value_as_temperature(&Value::Integer(1), "models.bind")
        .expect("integer 1 is in-domain")
        .get();
    let from_number = value_as_temperature(&Value::Number(1.0), "models.bind")
        .expect("number 1.0 is in-domain")
        .get();
    assert!((from_integer - from_number).abs() <= f64::EPSILON);
    assert!(
        value_as_temperature(&Value::Integer(5), "models.bind").is_err(),
        "an out-of-domain integer temperature must be rejected by the one domain check"
    );
}

#[test]
fn default_multi_arg_rolls_back_when_already_selected() {
    // PF-LM-003: a second multi-arg `models.default` must be rejected WITHOUT
    // leaving a half-recorded binding behind.
    let resolver = |_: &str, _: &ModelBindOpts| {
        Ok(promptforge_model_client::model::ResolvedModel {
            id: ModelId::from_validated("gateway", "m1"),
            invocation: ModelInvocation::from(&ModelBindOpts::default()),
            context: std::num::NonZeroU32::new(8192).expect("8192 is non-zero"),
        })
    };
    let mut set = ModelSet::default();
    let errors = std::sync::Mutex::new(None);
    record_default_binding(
        &mut set,
        &errors,
        &resolver,
        "a",
        "desc",
        &ModelBindOpts::default(),
    )
    .expect("the first models.default must succeed");
    assert_eq!(set.bindings.len(), 1);
    assert_eq!(set.default.as_deref(), Some("a"));

    let err = record_default_binding(
        &mut set,
        &errors,
        &resolver,
        "b",
        "desc",
        &ModelBindOpts::default(),
    )
    .expect_err("a second models.default must be rejected");
    assert!(
        err.to_string().contains("at most once"),
        "error must explain the at-most-once rule: {err}"
    );
    assert_eq!(
        set.bindings.len(),
        1,
        "a rejected second models.default must not record a binding (rollback)"
    );
    assert_eq!(set.default.as_deref(), Some("a"));
}

#[test]
fn model_runtime_select_enforces_at_most_once() {
    let mut runtime = ModelRuntime::new();
    assert!(runtime.used().is_none());
    runtime
        .select("writer".to_owned())
        .expect("first select ok");
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
    let lua = Lua::new();
    let table = Value::Table(lua.create_table().expect("create Lua table"));
    let boolean = Value::Boolean(true);
    let integer = Value::Integer(1);
    for value in [&table, &boolean, &integer] {
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
fn parse_bind_args_covers_each_branch() {
    let lua = Lua::new();
    // Missing description.
    let one: MultiValue = [lua_string(&lua, "writer")].into_iter().collect();
    assert!(
        parse_bind_args(one, "models.bind").is_err(),
        "one argument is rejected"
    );
    // Non-string alias.
    let bad_alias: MultiValue = [Value::Integer(1), lua_string(&lua, "desc")]
        .into_iter()
        .collect();
    assert!(
        parse_bind_args(bad_alias, "models.bind").is_err(),
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
    assert!(
        parse_bind_args(bad_opts, "models.bind").is_err(),
        "non-table opts fails"
    );
    // Too many arguments.
    let too_many: MultiValue = [
        lua_string(&lua, "writer"),
        lua_string(&lua, "desc"),
        Value::Nil,
        Value::Nil,
    ]
    .into_iter()
    .collect();
    assert!(
        parse_bind_args(too_many, "models.bind").is_err(),
        "four arguments fail"
    );
    // Valid two-argument form.
    let ok: MultiValue = [lua_string(&lua, "writer"), lua_string(&lua, "desc")]
        .into_iter()
        .collect();
    let (alias, description, opts) = parse_bind_args(ok, "models.bind").expect("valid bind args");
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
    let opts = parse_opts_table(&table, "models.bind").expect("all known keys parse");
    assert_eq!(opts.thinking, Some(true));
    assert_eq!(opts.context.map(std::num::NonZeroU32::get), Some(8192));
    assert_eq!(
        opts.temperature
            .map(promptforge_model_client::model::Temperature::get),
        Some(0.5)
    );
    assert_eq!(opts.max_tokens.map(std::num::NonZeroU32::get), Some(256));

    // MODEL-003: a zero count is rejected at the parse boundary, not stored.
    let zero_context = lua.create_table().expect("table");
    zero_context.set("context", 0).expect("set context");
    assert!(
        parse_opts_table(&zero_context, "models.bind").is_err(),
        "a zero context minimum must be rejected"
    );
    let zero_max = lua.create_table().expect("table");
    zero_max.set("max_tokens", 0).expect("set max_tokens");
    assert!(
        parse_opts_table(&zero_max, "models.bind").is_err(),
        "a zero max_tokens cap must be rejected"
    );

    let unknown = lua.create_table().expect("table");
    unknown.set("bogus", 1).expect("set bogus");
    assert!(
        parse_opts_table(&unknown, "models.bind").is_err(),
        "an unknown opts key must be rejected"
    );

    let non_string_key = lua.create_table().expect("table");
    non_string_key.set(1, "x").expect("set numeric key");
    assert!(
        parse_opts_table(&non_string_key, "models.bind").is_err(),
        "a non-string opts key must be rejected"
    );
}

#[test]
fn scalar_decoders_cover_valid_and_invalid_inputs() {
    assert!(value_as_bool(&Value::Boolean(false), "thinking", "models.bind").is_ok());
    assert!(value_as_bool(&Value::Integer(1), "thinking", "models.bind").is_err());

    assert_eq!(
        value_as_u32(&Value::Integer(7), "context", "models.bind").expect("ok"),
        7
    );
    assert_eq!(
        value_as_u32(&Value::Number(9.0), "context", "models.bind").expect("whole number ok"),
        9
    );
    assert!(value_as_u32(&Value::Integer(-1), "context", "models.bind").is_err());
    assert!(value_as_u32(&Value::Number(1.5), "context", "models.bind").is_err());
    assert!(value_as_u32(&Value::Boolean(true), "context", "models.bind").is_err());

    // MODEL-003: the non-zero decoder accepts positive counts and rejects zero.
    assert_eq!(
        value_as_nonzero_u32(&Value::Integer(7), "context", "models.bind")
            .expect("positive count")
            .get(),
        7
    );
    assert!(
        value_as_nonzero_u32(&Value::Integer(0), "context", "models.bind").is_err(),
        "a zero count must be rejected"
    );

    assert!(
        (decode_lua_number(&Value::Integer(2), "t", "models.bind").expect("int") - 2.0).abs()
            < f64::EPSILON
    );
    assert!(decode_lua_number(&Value::Boolean(true), "t", "models.bind").is_err());
}

#[test]
fn parse_single_alias_and_validate_alias_branches() {
    let lua = Lua::new();
    let ok: MultiValue = [lua_string(&lua, "writer")].into_iter().collect();
    assert_eq!(
        parse_single_alias(&ok, "models.default").expect("string alias"),
        "writer"
    );
    let bad: MultiValue = [Value::Integer(1)].into_iter().collect();
    assert!(
        parse_single_alias(&bad, "models.default").is_err(),
        "a non-string alias must be rejected"
    );

    assert!(validate_alias("Writer_1-x").is_ok());
    assert!(
        validate_alias(&format!("A{}", "2".repeat(63))).is_ok(),
        "a 64-character alias must be accepted"
    );
    assert!(
        validate_alias(&format!("A{}", "2".repeat(64))).is_err(),
        "a 65-character alias must be rejected"
    );
    assert!(validate_alias("").is_err(), "empty alias rejected");
    assert!(validate_alias("1abc").is_err(), "leading digit rejected");
    assert!(validate_alias("a b").is_err(), "space rejected");
}

#[test]
fn live_model_apis_label_nested_decoder_errors_by_entry_point() {
    let run = |source: &str| {
        let lua = Lua::new();
        let set = std::sync::Arc::new(std::sync::Mutex::new(ModelSet::default()));
        let errors = std::sync::Arc::new(std::sync::Mutex::new(None));
        let resolver = |_: &str, _: &ModelBindOpts| {
            Ok(promptforge_model_client::model::ResolvedModel {
                id: ModelId::from_validated("gateway", "m1"),
                invocation: ModelInvocation::from(&ModelBindOpts::default()),
                context: std::num::NonZeroU32::new(8192).expect("8192 is non-zero"),
            })
        };
        lua.scope(|scope| {
            super::install_live_models(&lua, scope, &resolver, &set, &errors)
                .map_err(mlua::Error::external)?;
            lua.load(source).exec()
        })
        .expect_err("the invalid nested scalar must be rejected")
        .to_string()
    };

    let bind = run("models.bind('writer', 'desc', { thinking = 1 })");
    assert!(
        bind.contains("models.bind opts.thinking must be a boolean"),
        "models.bind wording must remain exact: {bind}"
    );
    let default = run("models.default('writer', 'desc', { thinking = 1 })");
    assert!(
        default.contains("models.default opts.thinking must be a boolean"),
        "models.default must identify its own entry point: {default}"
    );
    assert!(
        !default.contains("models.bind"),
        "models.default errors must not be mislabelled: {default}"
    );
}

#[test]
fn model_runtime_starts_with_no_selection() {
    let runtime = ModelRuntime::new();
    assert!(runtime.used().is_none(), "fresh runtime has no selection");
}
