//! What a `require` reaches: a nested fanout worker reaches a top-level
//! library its prompt reads cannot see, a library's own `require` during
//! its load resolves from the library's position, and a library function
//! called later resolves `call`, `prompt.section`, and `require` from the
//! calling section.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_fanout_worker_requires_a_top_level_library() {
    let md = require_prompt!(
        "\
## Research\n\n\
```lua\n\
local results = tasks.fanout('### Worker', { 'alpha', 'beta' })\n\
return table.concat(results, ',')\n\
```\n\n\
### Worker\n\n\
```lua\n\
return require('## helpers').label(item)\n\
```\n\n\
## helpers\n\n\
for {{ item }}\n\n\
```lua\n\
local seen = prose\n\
return { label = function(value) return value .. ' ' .. seen end }\n\
```\n"
    );
    let run = run_fixture(md, "require", EXECUTION, "", None).await;
    assert_eq!(
        run.result.expect("each arm loads the library"),
        "alpha for alpha,beta for beta",
        "each arm renders the library prose with its own item"
    );
    let loads = library_loads(&run.recorder.records());
    assert_eq!(
        loads
            .iter()
            .filter(|(section, detail)| section == "Worker"
                && detail == "Lua library load succeeded: ## helpers")
            .count(),
        2,
        "each arm's VM loads the library itself: {loads:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_worker_requires_a_library_its_prompt_section_read_cannot_see() {
    let md = require_prompt!(
        "\
## Research\n\n\
```lua\n\
return tostring(tasks.fanout('### Worker', { 'alpha' })[1])\n\
```\n\n\
### Worker\n\n\
```lua\n\
local read = prompt.section('## house_style')\n\
return tostring(read) .. '|' .. require '## house_style'\n\
```\n\n\
## house_style\n\n\
Plain words for {{ item }}.\n\n\
```lua\n\
return prose\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("the worker reaches its parent's sibling");
    assert_eq!(
        out, "nil|Plain words for alpha.",
        "a parent's sibling is outside the visible set but within the reach"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_load_resolves_from_the_library_and_a_later_call_from_the_caller() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
local lib = require '## lib'\n\
return lib.helped .. '|' .. lib.later()\n\
```\n\n\
### mine\n\n\
Mine text.\n\n\
```lua\n\
return 'mine'\n\
```\n\n\
## lib\n\n\
```lua\n\
local helped = require '### helper'\n\
return {\n\
  helped = helped,\n\
  later = function()\n\
    local text = prompt.section('### mine')\n\
    local found = text ~= nil and text:find('Mine text.', 1, true) ~= nil\n\
    local helper_ok = pcall(require, '### helper')\n\
    return table.concat({ require '### mine', call('### mine'), tostring(found), \
tostring(helper_ok) }, ',')\n\
  end,\n\
}\n\
```\n\n\
### helper\n\n\
```lua\n\
return 'helped'\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("each lookup resolves from its own position");
    assert_eq!(
        out, "helped|mine,mine,true,false",
        "the load reaches the library's child; the later call reaches the caller's \
         child and no longer the library's"
    );
}
