//! Tests for a capability prelude's restricted environment: the globals
//! it sees, the read-only `var` view at every depth, and its tool calls
//! yielding from a block.

use super::*;

#[test]
fn a_prelude_sees_only_its_restricted_environment() {
    let vm = section_vm_with_var(Some(&json!({ "mode": "fast" })));
    install_ui(vm.lua(), Arc::new(json!({}))).expect("the ui global installs");
    let probe = "\
function probe_hidden()
  return type(ui) .. ',' .. type(argv) .. ',' .. type(sys) .. ',' .. type(models)
    .. ',' .. type(first_global)
end
function probe_visible()
  return type(tools.call) .. ',' .. type(store) .. ',' .. type(untrusted) .. ','
    .. type(string.format)
end
function probe_var()
  local ok, err = pcall(function() var.mode = 'slow' end)
  return var.mode, ok, type(err), tostring(err)
end
function probe_pcall()
  return pcall
end";
    install_preludes(
        vm.lua(),
        &[
            prelude("acme/first", "first_global = 1"),
            prelude("acme/probe", probe),
        ],
        &[],
    )
    .expect("the preludes install");

    let author_view: String = eval(
        &vm,
        "return type(ui) .. ',' .. type(argv) .. ',' .. type(sys) .. ',' .. type(models) \
         .. ',' .. type(first_global)",
    );
    assert!(
        !author_view.contains("nil"),
        "author code sees every probed global: {author_view}"
    );
    let hidden: String = eval(&vm, "return probe_hidden()");
    assert_eq!(hidden, "nil,nil,nil,nil,nil");
    let visible: String = eval(&vm, "return probe_visible()");
    assert_eq!(visible, "function,table,function,function");
    let (mode, ok, kind, message): (String, bool, String, String) = eval(&vm, "return probe_var()");
    assert_eq!(mode, "fast", "var reads through the view");
    assert!(!ok, "a write to var raises");
    assert_eq!(
        kind, "table",
        "the normalized pcall turns the refusal into an error table"
    );
    assert!(
        message.contains("var is read-only") && message.contains("'mode'"),
        "the refusal names var and the field: {message}"
    );
    let author_mode: String = eval(&vm, "return var.mode");
    assert_eq!(author_mode, "fast", "the refused write left var unchanged");
    let same_pcall: bool = eval(&vm, "return probe_pcall() == pcall");
    assert!(same_pcall, "the prelude's pcall is the normalized global");
}

#[test]
fn the_var_view_is_read_only_at_every_depth() {
    let vm = section_vm_with_var(Some(&json!({
        "cfg": { "mode": "fast", "list": [{ "n": 1 }] }
    })));
    let probe = "\
function probe_meta()
  return getmetatable(var), getmetatable(var.cfg)
end
function probe_write(path)
  local ok, err = pcall(function()
    if path == 'cfg' then var.cfg.mode = 'slow' else var.cfg.list[1].n = 2 end
  end)
  return ok, tostring(err)
end";
    install_preludes(vm.lua(), &[prelude("acme/probe", probe)], &[]).expect("the prelude installs");

    let (root, nested): (String, String) = eval(&vm, "return probe_meta()");
    assert_eq!(
        (root.as_str(), nested.as_str()),
        ("var is read-only", "var is read-only"),
        "getmetatable returns only the label, never the guarded var"
    );
    for (path, refusal, field) in [
        ("cfg", "var.cfg is read-only", "'mode'"),
        ("deep", "var.cfg.list[1] is read-only", "'n'"),
    ] {
        let (ok, message): (bool, String) = eval(&vm, &format!("return probe_write('{path}')"));
        assert!(!ok, "a nested write through the view raises");
        assert!(
            message.contains(refusal) && message.contains(field),
            "the refusal names the nested path and the field: {message}"
        );
    }
    let (mode, n): (String, i64) = eval(&vm, "return var.cfg.mode, var.cfg.list[1].n");
    assert_eq!(
        (mode.as_str(), n),
        ("fast", 1),
        "the refused writes left var unchanged"
    );
}

#[test]
fn the_var_view_raises_the_whole_refusal_for_a_string_or_integer_key() {
    let vm = section_vm_with_var(Some(&json!({ "mode": "fast" })));
    let probe = "\
function probe_set(key)
  local ok, err = pcall(function() var[key] = 1 end)
  return tostring(err)
end";
    install_preludes(vm.lua(), &[prelude("acme/probe", probe)], &[]).expect("the prelude installs");
    for (key, refusal) in [
        (
            "'mode'",
            "var is read-only inside a capability prelude; cannot set 'mode'",
        ),
        (
            "1",
            "var is read-only inside a capability prelude; cannot set 'Integer(1)'",
        ),
    ] {
        let message: String = eval(&vm, &format!("return probe_set({key})"));
        assert_eq!(refusal_line(&message), Some(refusal), "writing var[{key}]");
    }
}

#[test]
fn a_prelude_function_called_from_a_block_yields_its_tool_call() {
    let vm = section_vm();
    install_preludes(
        vm.lua(),
        &[prelude(
            "acme/kit",
            "kit = {}\nfunction kit.run(script)\n  return tools.call('acme/kit/run', { script = script })\nend",
        )],
        &[],
    )
    .expect("the prelude installs");
    let block = LuaProgram::compile(
        "return kit.run('ls')",
        "prelude test block",
        NonZeroU32::new(1).expect("a non-zero line"),
        &null_emitter(),
        SECTION,
    )
    .expect("the block compiles");

    let CoroStep::Yielded(_, values) = vm.start_block_coro(&block).expect("the block starts")
    else {
        panic!("the block must suspend on the tool call");
    };
    match vm.request_from_yield(&values) {
        YieldParse::Request(Request::ToolCall {
            alias,
            args,
            call_id,
            ..
        }) => {
            assert_eq!(alias, "acme/kit/run");
            assert_eq!(args, json!({ "script": "ls" }));
            assert_eq!(call_id, None, "a prelude's call is a script call");
        }
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}
