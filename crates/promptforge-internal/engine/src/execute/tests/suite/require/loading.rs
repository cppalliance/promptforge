//! Loading a library: the table of functions it hands back, the saved
//! value a second `require` returns, the load reports, one load in each
//! requiring section, and a child section that starts without its
//! parent's libraries.

use super::*;

#[tokio::test]
async fn a_section_uses_the_table_of_functions_a_sibling_library_returns() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
local shared = require '## shared_stuff'\n\
return shared.search_all(argv.prose)\n\
```\n\n\
## shared_stuff\n\n\
```lua\n\
local M = {}\n\
function M.search_all(topic)\n\
  return 'searched ' .. topic .. ' everywhere'\n\
end\n\
return M\n\
```\n"
    );
    let out = run_with_args(md, "rust")
        .await
        .expect("the library's functions run for the caller");
    assert_eq!(out, "searched rust everywhere");
}

#[tokio::test]
async fn a_second_require_returns_the_saved_value_without_rerunning_it() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
local first = require '## counted'\n\
local second = require '## counted'\n\
local no_first = require '## falsy'\n\
local no_second = require '## falsy'\n\
return table.concat({ tostring(first == second), var.counted, tostring(no_first), \
tostring(no_second), var.falsy }, '|')\n\
```\n\n\
## counted\n\n\
```lua\n\
var.counted = (var.counted or 0) + 1\n\
return {}\n\
```\n\n\
## falsy\n\n\
```lua\n\
var.falsy = (var.falsy or 0) + 1\n\
return false\n\
```\n"
    );
    let out = run_offline(md).await.expect("both libraries load once");
    assert_eq!(out, "true|1|false|false|1");
}

#[tokio::test]
async fn a_first_load_reports_its_start_and_end_naming_the_library() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
require '## lib'\n\
require '## lib'\n\
assert(not pcall(require, '## broken'), 'the broken library raises')\n\
return 'done'\n\
```\n\n\
## lib\n\n\
```lua\n\
loaded = true\n\
```\n\n\
## broken\n\n\
```lua\n\
error('broken library')\n\
```\n"
    );
    let run = run_fixture(md, "require", EXECUTION, "", None).await;
    assert_eq!(run.result.expect("the caller catches the failure"), "done");
    assert_eq!(
        library_loads(&run.recorder.records()),
        [
            load("Main", "Lua library load started: ## lib"),
            load("Main", "Lua library load succeeded: ## lib"),
            load("Main", "Lua library load started: ## broken"),
            load("Main", "Lua library load failed: ## broken"),
        ],
        "a saved library reports nothing on its second require"
    );
}

#[tokio::test]
async fn load_time_code_runs_once_in_each_requiring_section() {
    let md = require_prompt!(
        "\
## First\n\n\
```lua\n\
require '## lib'\n\
require '## lib'\n\
```\n\n\
## Second\n\n\
```lua\n\
require '## lib'\n\
require '## lib'\n\
return tostring(var.loads)\n\
```\n\n\
## lib\n\n\
```lua\n\
var.loads = (var.loads or 0) + 1\n\
```\n"
    );
    let run = run_fixture(md, "require", EXECUTION, "", None).await;
    assert_eq!(
        run.result.expect("both sections load the library"),
        "2",
        "each section's VM loads the library once"
    );
    assert_eq!(
        library_loads(&run.recorder.records()),
        [
            load("First", "Lua library load started: ## lib"),
            load("First", "Lua library load succeeded: ## lib"),
            load("Second", "Lua library load started: ## lib"),
            load("Second", "Lua library load succeeded: ## lib"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_section_starts_without_its_parents_libraries_and_loads_its_own() {
    let md = require_prompt!(
        "\
## Parent\n\n\
```lua\n\
require '## lib'\n\
assert(greet, 'the parent sees the library global')\n\
return call('### Child')\n\
```\n\n\
### Child\n\n\
```lua\n\
local before = tostring(greet)\n\
require '## lib'\n\
return before .. '|' .. greet('child')\n\
```\n\n\
## lib\n\n\
```lua\n\
function greet(name) return 'hello ' .. name end\n\
```\n"
    );
    let run = run_fixture(md, "require", EXECUTION, "", None).await;
    assert_eq!(
        run.result.expect("the child loads the library itself"),
        "nil|hello child"
    );
    assert_eq!(
        library_loads(&run.recorder.records()),
        [
            load("Parent", "Lua library load started: ## lib"),
            load("Parent", "Lua library load succeeded: ## lib"),
            load("Child", "Lua library load started: ## lib"),
            load("Child", "Lua library load succeeded: ## lib"),
        ]
    );
}
