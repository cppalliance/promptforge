//! Library code issues effects as part of the calling section: `store`,
//! model, and tool calls while it loads, and prompt-wide state an H1
//! `require` sets for every later section.

use std::sync::Arc;

use promptforge_model_client::detail::tool_schema_name;

use crate::test_support::TestTool;

use super::super::super::{
    EchoTool, ScriptedChat, TestStore, bound_with_tools, gatewayed, resp_text, run,
};

/// The frontmatter and H1 opening of the effect cases: the fixture tools
/// Plugin declared, and the `writer` role the test helpers declare.
macro_rules! effect_prompt {
    ($body:literal) => {
        concat!(
            "---\nname: t\ndescription: d\npromptforge: 0\nplugins:\n  - tools\n---\n\n",
            "# Require\n\n",
            $body
        )
    };
}

#[tokio::test]
async fn library_code_makes_store_model_and_tool_calls_while_it_loads() {
    let md = effect_prompt!(
        "\
```lua\n\
models.default('writer')\n\
```\n\n\
## Main\n\n\
```lua\n\
return require '## loader'\n\
```\n\n\
## loader\n\n\
```lua\n\
store.write('lib.md', 'stored')\n\
local echoed = tools.call('tools/echo', { value = 'lib' })\n\
local reply = models.infer('ping')\n\
return table.concat({ store.read('lib.md'), tostring(echoed), tostring(reply) }, '|')\n\
```\n"
    );
    let gateway = ScriptedChat::new(vec![resp_text("pong")]);
    let store = TestStore::new();
    let out = run(
        &bound_with_tools(md),
        "",
        &[Arc::new(EchoTool) as Arc<dyn TestTool>],
        &store,
        gatewayed(&gateway),
    )
    .await
    .expect("the library's effects run for the caller");
    assert_eq!(out, "stored|echoed: lib|pong");
    assert_eq!(store.read("lib.md").expect("the write landed"), "stored");
    assert_eq!(gateway.requests().len(), 1, "one model round ran");
}

#[tokio::test]
async fn an_h1_library_sets_prompt_wide_state_every_later_section_sees() {
    let md = effect_prompt!(
        "\
```lua\n\
require '## setup'\n\
```\n\n\
## Main\n\n\
```lua\n\
local msgs = messages.new()\n\
msgs:user('ping')\n\
models.loop(msgs)\n\
return msgs[#msgs].content\n\
```\n\n\
## setup\n\n\
```lua\n\
models.default('writer')\n\
tools.always_offer('tools/echo')\n\
```\n"
    );
    let gateway = ScriptedChat::new(vec![resp_text("pong")]);
    let out = run(
        &bound_with_tools(md),
        "",
        &[Arc::new(EchoTool) as Arc<dyn TestTool>],
        &TestStore::new(),
        gatewayed(&gateway),
    )
    .await
    .expect("the section's loop runs under the H1 library's default");
    assert_eq!(out, "pong");
    let round = gateway
        .last_request()
        .expect("the round reaches the gateway");
    let offered: Vec<&str> = round.tools.iter().map(tool_schema_name).collect();
    assert_eq!(
        offered,
        ["tools_echo"],
        "the H1 library's prompt-wide offer reaches a later section's round"
    );
}
