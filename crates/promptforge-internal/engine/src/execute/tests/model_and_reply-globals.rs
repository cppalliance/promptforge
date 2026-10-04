//! What a section reads through `sys.model`, `item`, and `reply`:
//! `sys.model` is unknown before the one-time scope install and reads
//! the catalog id after it, in prose, the epilog, and a fanout arm; a
//! table `item` member substitutes as compact JSON; and `reply` is not
//! a global: it reads nil in Lua and fails as an unknown global in
//! prose.

use super::*;

#[tokio::test]
async fn shared_function_sees_sys_model_unknown_before_scope_close() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
# Test prompt\n\n\
```lua\nmodels.default('writer')\n```\n\n\
```lua shared\nfunction read_sys_model()\n  return sys.model\nend\n```\n\n\
## Only\n\n```lua\nreturn read_sys_model()\n```\n\nprose\n";
    let error = run(&bound_for_model(md), "", &[], &TestStore::new(), silent())
        .await
        .expect_err("shared function must not read sys.model before scope close");
    assert!(
        error.to_string().contains("unknown sys field 'model'"),
        "error must name the missing field: {error}"
    );
}

#[tokio::test]
async fn prologue_sys_model_unknown_before_scope_close() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\n```lua\nreturn sys.model\n```\n\nprose\n";
    let error = run(&bound_for_model(md), "", &[], &TestStore::new(), silent())
        .await
        .expect_err("prologue must not read sys.model before scope close");
    assert!(
        error.to_string().contains("unknown sys field 'model'"),
        "error must name the missing field: {error}"
    );
}

#[tokio::test]
async fn prose_substitution_sees_sys_model_catalog_id() {
    // The first script dispatch runs the one-time scope install, which
    // enriches `sys.model` with the bound catalog id; a prose read after it
    // substitutes the catalog id, not the alias.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nplugins:\n  - tests/tools\ntools:\n  echo: tests/tools/echo\nmodels:\n  writer: {}\n---\n\n\
# Test prompt\n\n```lua shared\n\
models.default('writer')\n```\n\n\
## Only\n\n```lua\ntools.call('echo', { value = 'x' })\n```\n\nModel id is {{ sys.model }}.\n\n\
```lua\nreturn prose\n```\n";
    let prompt = bound_with_tools(md);
    let out = run(
        &prompt,
        "",
        &[Arc::new(EchoTool) as Arc<dyn TestTool>],
        &TestStore::new(),
        silent(),
    )
    .await
    .unwrap();
    assert_eq!(out, "Model id is claude-sonnet-4-6.");
}

#[tokio::test]
async fn epilog_sees_model_catalog_id_not_alias_after_the_scope_install() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nplugins:\n  - tests/tools\ntools:\n  echo: tests/tools/echo\nmodels:\n  writer: {}\n---\n\n\
# Test prompt\n\n```lua shared\n\
models.default('writer')\n```\n\n\
## Only\n\n```lua\ntools.call('echo', { value = 'x' })\n```\n\n```lua\nreturn sys.model\n```\n";
    let prompt = bound_with_tools(md);
    let out = run(
        &prompt,
        "",
        &[Arc::new(EchoTool) as Arc<dyn TestTool>],
        &TestStore::new(),
        silent(),
    )
    .await
    .unwrap();
    assert_eq!(out, "claude-sonnet-4-6");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_sees_sys_model_catalog_id_after_the_scope_install() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nplugins:\n  - tests/tools\ntools:\n  echo: tests/tools/echo\nmodels:\n  writer: {}\n---\n\n\
# Test prompt\n\n```lua shared\n\
models.default('writer')\n```\n\n\
## Parent\n\n```lua\nlocal r = fanout('### Worker', list_from_section('### Items'))\nreturn table.concat(r, ',')\n```\n\n\
### Worker\n\n```lua\ntools.call('echo', { value = item })\n```\n\n\
```lua\nreturn sys.model .. ':' .. item\n```\n\n\
### Items\n\n- a\n";
    let prompt = bound_with_tools(md);
    let out = run(
        &prompt,
        "",
        &[Arc::new(EchoTool) as Arc<dyn TestTool>],
        &TestStore::new(),
        silent(),
    )
    .await
    .unwrap();
    assert_eq!(out, "claude-sonnet-4-6:a");
}

/// `{{ item }}` renders a non-string member per its type: here a table
/// member reaches the model as compact JSON.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_item_substitution_renders_a_table_member_as_compact_json() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## Parent\n\n```lua\nlocal r = fanout('### Worker', {{7, 'x'}})\nreturn r[1].text\n```\n\n\
### Worker\n\n```lua\n-- prologue\n```\n\nItem: {{ item }}.\n\n```lua\nreturn models.infer(prose)\n```\n";
    let gateway = ScriptedChat::new(vec![resp_text("hello from the mock")]);
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &TestStore::new(),
        gatewayed(&gateway),
    )
    .await
    .unwrap();
    assert_eq!(out, "hello from the mock");

    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    let user_content = body
        .messages
        .first()
        .map(crate::model::Message::content)
        .expect("first message must hold substituted prose");
    assert!(
        user_content.contains("Item: [7,\"x\"]."),
        "a table member must render as compact JSON, got: {user_content}"
    );
}

#[tokio::test]
async fn reply_is_nil_in_first_section() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\n```lua\nreturn tostring(reply)\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "nil");
}

#[tokio::test]
async fn reply_substitution_is_an_unknown_global_error() {
    // The reply register is gone: `{{ reply }}` names no namespace and no
    // bare global, so reading the prose fails at the read site.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\n{{ reply }}\n\n```lua\nreturn prose\n```\n";
    let err = run_offline(md)
        .await
        .expect_err("{{ reply }} names no global and must error");
    assert!(
        err.to_string().contains("reply"),
        "error must mention reply, got: {err}"
    );
}
