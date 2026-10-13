//! Library prose: a library block reads its own `prose`, rendered from
//! the caller's environment, and the caller's `prose` comes back
//! unchanged, whether the caller read it first or not and whether the
//! library finished or raised.

use super::*;

#[tokio::test]
async fn a_library_hands_its_prose_to_the_caller_and_leaves_the_callers_prose_alone() {
    let md = require_prompt!(
        "\
## Main\n\n\
First {{ var.word }}.\n\n\
```lua\n\
var.word = 'one'\n\
local before = prose\n\
var.word = 'changed'\n\
style = 'terse'\n\
local text = require '## house_style'\n\
local ok = pcall(require, '## broken_style')\n\
var.summary = table.concat({ before, prose, text, tostring(ok) }, '|')\n\
```\n\n\
Second {{ var.word }}.\n\n\
```lua\n\
var.word = 'two'\n\
local ok = pcall(require, '## broken_style')\n\
var.word = 'three'\n\
return table.concat({ var.summary, tostring(ok), var.broken_saw, prose }, '|')\n\
```\n\n\
## house_style\n\n\
Write in a {{ style }} voice for {{ args }}.\n\n\
```lua\n\
return prose\n\
```\n\n\
## broken_style\n\n\
Broken {{ style }}.\n\n\
```lua\n\
var.broken_saw = prose\n\
error('the style broke')\n\
```\n"
    );
    let out = run_with_args(md, "readers")
        .await
        .expect("the caller reads its own prose around each require");
    assert_eq!(
        out,
        "First one.|First one.|Write in a terse voice for readers.|false|false|Broken terse.\
         |Second three.",
        "the memo the caller read survives both loads, and an unread prose renders \
         from the caller's own template after a failed load"
    );
}
