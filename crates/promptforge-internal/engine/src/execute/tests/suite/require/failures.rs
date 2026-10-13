//! What raises: a cycle, a heading out of reach or misspelled, a `jump`
//! during a load, and a Plugin prelude that defines or calls `require`.
//! A library that raises while loading runs again on the next `require`,
//! and a raise in library code reports the library's location and
//! prompt-file line.

use promptforge_types::plugins::{PluginId, Prelude};

use crate::parser::Prompt;
use crate::test_support::{RunFixture, run_with_fixture};
use crate::{Environment, RunErrorKind, RunResult};

use super::super::support::context;
use super::*;

/// Runs `md` under the default environment with `preludes` installed in
/// every section VM.
async fn run_with_preludes(md: &str, preludes: Vec<Prelude>) -> Result<String, RunError> {
    let prompt = Prompt::parse(md, EXECUTION)
        .0
        .expect("the fixture prompt parses");
    let env = Environment::new().preludes(preludes);
    match run_with_fixture(&env, &prompt, "", context(EXECUTION), RunFixture::new()).await {
        RunResult::Ok(text) => Ok(text),
        RunResult::Failure(error) => Err(error),
        RunResult::Cancelled => panic!("offline fixture runs are never cancelled"),
    }
}

/// A prelude contributed by the Plugin `id`.
fn prelude(id: &str, source: &str) -> Prelude {
    Prelude::new(PluginId::parse(id).expect("a valid Plugin id"), source)
}

/// The text of a run that fails as a Lua failure.
fn lua_failure(result: Result<String, RunError>) -> String {
    let error = result.expect_err("the run fails");
    assert_eq!(error.kind(), RunErrorKind::Lua, "{error}");
    error.to_string()
}

#[tokio::test]
async fn a_cycle_raises_naming_its_sections_in_order() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
require '## a'\n\
```\n\n\
## a\n\n\
```lua\n\
require '## b'\n\
```\n\n\
## b\n\n\
```lua\n\
require '## a'\n\
```\n"
    );
    let message = lua_failure(run_offline(md).await);
    assert!(
        message.contains("require cycle: ## a -> ## b -> ## a"),
        "the cycle names both libraries: {message}"
    );
}

#[tokio::test]
async fn an_out_of_reach_or_misspelled_heading_parses_and_raises_when_the_call_runs() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
local _, far = pcall(require, '### deep')\n\
local _, typo = pcall(require, '## helpres')\n\
return tostring(far) .. '\\n' .. tostring(typo)\n\
```\n\n\
## other\n\n\
### deep\n\n\
```lua\n\
return 'deep'\n\
```\n\n\
## helpers\n\n\
```lua\n\
return 'helpers'\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("the caller catches both raises");
    let (far, typo) = out.split_once('\n').expect("two messages");
    assert!(
        far.contains(
            "section heading `### deep` not found; available sections: ## other, ## helpers"
        ),
        "a sibling's child is out of reach: {far}"
    );
    assert!(
        typo.contains(
            "section heading `## helpres` not found; available sections: ## other, ## helpers"
        ),
        "a misspelled heading names the reach: {typo}"
    );
}

#[tokio::test]
async fn a_jump_during_a_load_raises() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
require '## jumper'\n\
```\n\n\
## jumper\n\n\
```lua\n\
jump('## Last')\n\
```\n\n\
## Last\n\n\
```lua\n\
return 'jumped'\n\
```\n"
    );
    let message = lua_failure(run_offline(md).await);
    assert!(
        message.contains("jump is unavailable while a library loads"),
        "the load refuses the jump: {message}"
    );
}

#[tokio::test]
async fn a_library_that_raises_while_loading_runs_again_on_the_next_require() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
var.attempts = 0\n\
local first_ok = pcall(require, '## flaky')\n\
local second = require '## flaky'\n\
return tostring(first_ok) .. '|' .. second .. '|' .. var.attempts\n\
```\n\n\
## flaky\n\n\
```lua\n\
var.attempts = var.attempts + 1\n\
if var.attempts == 1 then error('the first load fails') end\n\
return 'loaded'\n\
```\n"
    );
    let out = run_offline(md).await.expect("the second load succeeds");
    assert_eq!(out, "false|loaded|2");
}

#[tokio::test]
async fn a_prelude_cannot_define_require_or_call_it_while_it_installs() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
return 'unreachable'\n\
```\n\n\
## lib\n\n\
```lua\n\
return 'lib'\n\
```\n"
    );
    let defines = lua_failure(
        run_with_preludes(
            md,
            vec![prelude("shadow", "function require() return 'mine' end")],
        )
        .await,
    );
    assert!(
        defines.contains("Plugin `shadow`: its prelude defines the global `require`")
            && defines.contains("which is reserved as an Engine global"),
        "require is reserved: {defines}"
    );
    let calls =
        lua_failure(run_with_preludes(md, vec![prelude("eager", "require '## lib'")]).await);
    assert!(
        calls.contains("Plugin `eager`: its prelude failed to load")
            && calls.contains("attempt to call a nil value (global 'require')"),
        "the prelude phase has no require: {calls}"
    );
}

#[tokio::test]
async fn a_raise_during_the_load_reports_the_library_line() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
require '## lib'\n\
```\n\n\
## lib\n\n\
```lua\n\
local ready = true\n\
error('load fault')\n\
```\n"
    );
    let message = lua_failure(run_offline(md).await);
    assert!(
        message.contains("section `lib` prologue:19: load fault"),
        "the raise names the library block and its prompt line 19: {message}"
    );
}

#[tokio::test]
async fn a_raise_while_a_long_titled_library_loads_reports_the_library_line() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
require '## Shared Research Helper Functions'\n\
```\n\n\
## Shared Research Helper Functions\n\n\
```lua\n\
local ready = true\n\
error('load fault')\n\
```\n"
    );
    let message = lua_failure(run_offline(md).await);
    assert!(
        message.contains("section `Shared Research Helper Functions` prologue:19: load fault"),
        "the raise names the library block and its prompt line 19: {message}"
    );
}

#[tokio::test]
async fn a_raise_from_a_library_function_called_later_reports_the_library_line() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
require '## lib'\n\
boom()\n\
```\n\n\
## lib\n\n\
```lua\n\
function boom()\n\
  error('later fault')\n\
end\n\
```\n"
    );
    let message = lua_failure(run_offline(md).await);
    assert!(
        message.contains("section `lib` prologue:20: later fault"),
        "the raise names the library block and its prompt line 20: {message}"
    );
    assert!(
        message.contains("section `Main` prologue:13:"),
        "the caller's frame maps to its own prompt line 13: {message}"
    );
}
