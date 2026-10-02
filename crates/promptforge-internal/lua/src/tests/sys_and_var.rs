//! Compatibility-chunk returns, the read-only `sys` table, and the guarded
//! `var` table.

use super::*;

#[test]
fn returns_args_verbatim() {
    assert_eq!(
        run("return args", "hello").unwrap().returned.as_deref(),
        Some("hello")
    );
}

#[test]
fn expression_only_compatibility_chunk_returns_its_value() {
    assert_eq!(run("42", "").unwrap().returned.as_deref(), Some("42"));
}

#[test]
fn no_return_is_none() {
    assert_eq!(run("local x = 1", "hello").unwrap().returned, None);
}

#[test]
fn reads_sys() {
    assert_eq!(
        run("return sys.id", "").unwrap().returned.as_deref(),
        Some("1")
    );
    assert_eq!(
        run("return sys.when", "").unwrap().returned.as_deref(),
        Some("t")
    );
}

#[test]
fn unknown_sys_field_is_a_lua_error() {
    let error = run("return sys.bogus", "").expect_err("missing sys field must fail");
    assert!(
        error.to_string().contains("unknown sys field 'bogus'"),
        "error was {error}"
    );
}

#[test]
fn writing_sys_field_is_a_lua_error() {
    let existing = run("sys.when = 'x'", "").expect_err("writing an existing sys field must fail");
    assert!(
        existing
            .to_string()
            .contains("sys is read-only; cannot set 'when'"),
        "error was {existing}"
    );

    let created = run("sys.extra = 1", "").expect_err("creating a sys field must fail");
    assert!(
        created
            .to_string()
            .contains("sys is read-only; cannot set 'extra'"),
        "error was {created}"
    );
}

#[test]
fn a_sys_write_raises_the_whole_refusal_for_a_string_or_integer_key() {
    for (target, refusal) in [
        ("sys.extra", "sys is read-only; cannot set 'extra'"),
        ("sys[1]", "sys is read-only; cannot set 'Integer(1)'"),
    ] {
        let source =
            format!("local ok, err = pcall(function() {target} = 1 end)\nreturn tostring(err)");
        let out = run(&source, "").expect("the pcall catches the refusal");
        let caught = out.returned.expect("the chunk returns the caught error");
        assert_eq!(refusal_line(&caught), Some(refusal), "writing {target}");
    }
    let label = run("return getmetatable(sys)", "").unwrap();
    assert_eq!(label.returned.as_deref(), Some("sys is sealed"));
}

#[test]
fn var_is_read_back() {
    let out = run("var.greeting = 'hi ' .. args", "bob").unwrap();
    assert_eq!(
        out.var.get("greeting").and_then(|v| v.as_str()),
        Some("hi bob")
    );
}

#[test]
fn var_guard_allows_json_data_and_reads_back() {
    let out = run(
        "var.n = 1\nvar.s = 'x'\nvar.t = { a = {1, 2} }\nvar.b = true",
        "",
    )
    .expect("JSON data writes must pass the guard");
    assert_eq!(
        out.var,
        json!({ "n": 1, "s": "x", "t": { "a": [1, 2] }, "b": true })
    );
}

#[test]
fn var_rejects_a_function_at_the_assigning_line() {
    let error = run("var.f = function() end", "")
        .expect_err("a function assigned into var must fail at the assigning line");
    assert!(
        error
            .to_string()
            .contains("var.f must be JSON data, got function"),
        "error was {error}"
    );
}

#[test]
fn var_rejects_a_nested_function_at_the_assigning_line() {
    let error = run("var.t = { f = function() end }", "")
        .expect_err("a nested function must fail the deep check at the assigning line");
    assert!(
        error.to_string().contains("function"),
        "the bridge error must name the offending type: {error}"
    );
}

#[test]
fn var_guard_error_is_catchable_at_the_assigning_line() {
    // A pcall around the write catches the guard's error, proving the failure
    // is raised by that statement rather than later at serialization. The
    // caught value is mlua's error userdata, so stringify before matching.
    let out = run(
        "local ok, err = pcall(function() var.f = function() end end)\n\
         assert(not ok, 'the write must fail')\n\
         assert(tostring(err):match('must be JSON data'), tostring(err))\n\
         var.kept = 'yes'\n\
         return var.kept",
        "",
    )
    .expect("the caught guard error must not fail the chunk");
    assert_eq!(out.returned.as_deref(), Some("yes"));
    assert_eq!(out.var.get("kept").and_then(|v| v.as_str()), Some("yes"));
}

#[test]
fn var_guard_rejects_incremental_nested_function_writes_at_the_assigning_line() {
    let out = run(
        "var.t = {}\n\
         local ok, err = pcall(function() var.t.f = function() end end)\n\
         assert(not ok, 'the nested write must fail')\n\
         assert(tostring(err):match('var.t.f must be JSON data'), tostring(err))\n\
         var.t.kept = 'yes'\n\
         return var.t.kept",
        "",
    )
    .expect("the nested guard error must remain catchable");
    assert_eq!(out.returned.as_deref(), Some("yes"));
    assert_eq!(out.var, json!({ "t": { "kept": "yes" } }));
}

#[test]
fn assigning_nil_to_a_var_field_removes_it_from_var_and_its_snapshot() {
    let lua = Lua::new();
    let var = guarded_var(&lua, Some(&json!({ "k": 1, "kept": "yes" })))
        .expect("the guarded var builds from a JSON object");
    lua.globals().raw_set("var", var).expect("var installs");
    let reads_nil: bool = lua
        .load("var.k = nil\nreturn var.k == nil")
        .eval()
        .expect("assigning nil passes the guard");
    assert!(reads_nil, "var.k must read as nil after `var.k = nil`");
    let snapshot = var_snapshot_table(&lua).expect("the snapshot reads back");
    let keys = snapshot
        .pairs::<String, Value>()
        .map(|pair| pair.map(|(key, _)| key))
        .collect::<mlua::Result<Vec<_>>>()
        .expect("the snapshot iterates");
    assert_eq!(keys, ["kept"], "the snapshot must drop the removed key");
    assert_eq!(
        var_to_json(&lua).expect("var reads back"),
        json!({ "kept": "yes" })
    );
}

#[test]
fn reassigning_the_var_global_fails_read_back() {
    // `var = 5` drops the guarded proxy from reach; read-back must reject it
    // rather than silently roll the pre-reassignment data forward.
    let error = run("var = 5", "").expect_err("reassigning the var global must fail at read-back");
    assert!(
        error.to_string().contains("`var` global was reassigned"),
        "the error must name the reassignment: {error}"
    );
}
