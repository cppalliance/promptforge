//! A required section stays an ordinary section: when the walk reaches a
//! library it runs under the section rules, and the two placements that
//! keep a returning library away from the walk.

use crate::RunErrorKind;

use super::*;

#[tokio::test]
async fn the_walk_runs_a_globals_only_library_harmlessly() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
require '## helpers'\n\
var.greeting = greet('main')\n\
```\n\n\
## helpers\n\n\
```lua\n\
function greet(name) return 'hello ' .. name end\n\
```\n\n\
## Last\n\n\
```lua\n\
return var.greeting .. '|' .. tostring(greet)\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("the walk runs the library as a section");
    assert_eq!(
        out, "hello main|nil",
        "the library's globals stay in each VM that ran it"
    );
}

#[tokio::test]
async fn the_walk_fails_the_run_on_a_library_that_returns_a_table() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
var.name = (require '## lib').name\n\
```\n\n\
## lib\n\n\
```lua\n\
return { name = 'lib' }\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("a section block cannot return a table");
    assert_eq!(error.kind(), RunErrorKind::Lua, "{error}");
    assert!(
        error
            .to_string()
            .contains("cannot return a table as a result"),
        "the walk applies the section return rule to the library: {error}"
    );
}

#[tokio::test]
async fn the_walk_ends_the_chain_with_a_library_that_returns_a_scalar() {
    let md = require_prompt!(
        "\
## Main\n\n\
```lua\n\
var.from_main = require '## lib'\n\
```\n\n\
## lib\n\n\
```lua\n\
return 'from lib'\n\
```\n\n\
## Last\n\n\
```lua\n\
error('the chain ends at the library')\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("the library's scalar ends the chain");
    assert_eq!(out, "from lib");
}

#[tokio::test]
async fn a_returning_library_after_a_return_or_under_its_user_stays_off_the_walk() {
    let after = require_prompt!(
        "\
## Main\n\n\
```lua\n\
return (require '## lib').name\n\
```\n\n\
## lib\n\n\
```lua\n\
return { name = 'after' }\n\
```\n"
    );
    let nested = require_prompt!(
        "\
## Main\n\n\
```lua\n\
var.name = (require '### lib').name\n\
```\n\n\
### lib\n\n\
```lua\n\
return { name = 'nested' }\n\
```\n\n\
## Last\n\n\
```lua\n\
return var.name\n\
```\n"
    );
    for (md, expected) in [(after, "after"), (nested, "nested")] {
        let out = run_offline(md)
            .await
            .expect("the walk never reaches the library");
        assert_eq!(out, expected);
    }
}
