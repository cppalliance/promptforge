//! An author's own metatable on `_G`: it keeps working for every other
//! global (defaults, strict errors, write hooks) in the H1 pass and in
//! later sections, across the shared replay and across fences, while
//! `argv` and `prose` stay guarded whatever author code does to `_G`'s
//! metatable.

use crate::RunError;

use super::support::run_fixture;

const EXECUTION: &str = "execute-test";

/// Runs a fixture offline with the given args string.
async fn run_args(md: &str, args: &str) -> Result<String, RunError> {
    run_fixture(md, "global-metatable", EXECUTION, args, None)
        .await
        .result
}

/// The frontmatter every test shares: one declared `query` string, so
/// `argv` is the parsed JSON or nil.
macro_rules! args_prompt {
    ($body:literal) => {
        concat!(
            "---\nname: t\ndescription: d\npromptforge: 0\n",
            "args:\n  query:\n    type: string\n",
            "---\n\n",
            $body
        )
    };
}

#[tokio::test]
async fn a_shared_defaults_metatable_works_beside_frozen_argv_and_read_only_prose() {
    let md = args_prompt!(
        "# T\n\n\
```lua shared\n\
defaults = { tone = 'friendly' }\n\
setmetatable(_G, { __index = defaults })\n\
```\n\n\
```lua\n\
assert(tone == 'friendly', 'H1 reads the default')\n\
argv = { query = 'repaired' }\n\
assert(not pcall(function() prose = 'x' end), 'prose is read-only in H1')\n\
```\n\n\
## First\n\n\
Tone: {{ tone }}.\n\n\
```lua\n\
assert(prose == 'Tone: friendly.', 'a default renders in prose: ' .. prose)\n\
assert(missing == nil, 'a global with no default reads nil')\n\
local ok, err = pcall(function() argv = 'hijacked' end)\n\
assert(not ok and tostring(err):find('argv is frozen outside H1: assign it in H1 only', 1, true), 'argv stays frozen: ' .. tostring(err))\n\
var.n = 2\n\
```\n\n\
Second fence {{ var.n }}.\n\n\
```lua\n\
assert(prose == 'Second fence 2.', 'the second fence renders fresh: ' .. prose)\n\
local ok, err = pcall(function() prose = 'x' end)\n\
assert(not ok and tostring(err):find('prose is read-only: assign to `var` or a section global instead', 1, true), 'prose stays read-only: ' .. tostring(err))\n\
assert(tone == 'friendly', 'the default survives the second fence')\n\
```\n\n\
## Second\n\n\
```lua\n\
assert(argv.query == 'repaired', 'the H1 repair reaches a later section')\n\
assert(not pcall(function() argv = 'hijacked' end), 'argv stays frozen in a later section')\n\
return argv.query .. ' ' .. tone\n\
```\n"
    );
    let out = run_args(md, "broken json").await.expect("the run succeeds");
    assert_eq!(out, "repaired friendly");
}

#[tokio::test]
async fn a_strict_metatable_raises_its_own_error_while_the_guard_serves_argv_and_prose() {
    let md = args_prompt!(
        "# T\n\n\
```lua shared\n\
setmetatable(_G, {\n\
  __index = function(_, key) error('undefined global ' .. key, 2) end,\n\
})\n\
```\n\n\
```lua\n\
local ok, err = pcall(function() return undefined_in_h1 end)\n\
assert(not ok and tostring(err):find('undefined global undefined_in_h1', 1, true), 'H1 is strict: ' .. tostring(err))\n\
```\n\n\
## Only\n\n\
Hi.\n\n\
```lua\n\
local ok, err = pcall(function() return undefined_here end)\n\
assert(not ok, 'an undefined global raises')\n\
assert(type(err) == 'string', 'the author error comes back as raised, got ' .. type(err))\n\
assert(err:find(']:1: undefined global undefined_here', 1, true), 'the error names the reading line: ' .. err)\n\
assert(argv.query == 'x', 'the guard serves argv, not the strict handler')\n\
assert(prose == 'Hi.', 'the guard serves prose, not the strict handler')\n\
return 'strict'\n\
```\n"
    );
    let out = run_args(md, "{\"query\":\"x\"}")
        .await
        .expect("the run succeeds");
    assert_eq!(out, "strict");
}

#[tokio::test]
async fn an_h1_repair_lands_under_a_strict_index_and_a_write_hook() {
    let md = args_prompt!(
        "# T\n\n\
```lua shared\n\
seen = {}\n\
setmetatable(_G, {\n\
  __index = function(_, key) error('undefined global ' .. key, 2) end,\n\
  __newindex = function(_, key, value) seen[key] = value end,\n\
})\n\
```\n\n\
```lua\n\
if argv == nil then argv = { query = 'repaired' } end\n\
assert(seen.argv == nil, 'the write hook never sees argv')\n\
```\n\n\
## Only\n\n\
```lua\n\
assert(argv.query == 'repaired', 'the repair reaches the walk')\n\
return argv.query\n\
```\n"
    );
    let out = run_args(md, "broken json").await.expect("the run succeeds");
    assert_eq!(out, "repaired");
}

#[tokio::test]
async fn setmetatable_on_g_in_a_block_frees_neither_argv_nor_prose() {
    let md = args_prompt!(
        "## Only\n\n\
First.\n\n\
```lua\n\
setmetatable(_G, nil)\n\
local a = pcall(function() argv = 'x' end)\n\
setmetatable(_G, {})\n\
local b = pcall(function() prose = 'x' end)\n\
captured = {}\n\
setmetatable(_G, { __newindex = function(_, k, v) captured[k] = v end })\n\
local c = pcall(function() argv = 'x' end)\n\
local d = pcall(function() prose = 'x' end)\n\
assert(not (a or b or c or d), 'every assignment is refused')\n\
assert(captured.argv == nil and captured.prose == nil, 'the author hook never sees argv or prose')\n\
plain = 'y'\n\
assert(captured.plain == 'y', 'the author hook sees other globals')\n\
assert(argv.query == 'x' and prose == 'First.', 'both still read as the host set them')\n\
```\n\n\
Second.\n\n\
```lua\n\
assert(prose == 'Second.', 'the next fence installs its prose under the author metatable')\n\
assert(getmetatable(_G).__newindex ~= nil, 'the author metatable survives the next fence')\n\
assert(not pcall(function() argv = 'x' end), 'argv is still frozen')\n\
return 'guarded'\n\
```\n"
    );
    let out = run_args(md, "{\"query\":\"x\"}")
        .await
        .expect("the run succeeds");
    assert_eq!(out, "guarded");
}

#[tokio::test]
async fn the_shared_library_cannot_assign_prose_before_the_first_block() {
    let md = args_prompt!(
        "# T\n\n\
```lua shared\n\
shared_prose = type(prose)\n\
local ok, err = pcall(function() prose = 'shadowed' end)\n\
shared_refusal = not ok and tostring(err)\n\
```\n\n\
## Only\n\n\
The real prose.\n\n\
```lua\n\
assert(shared_prose == 'nil', 'prose reads nil while the library loads')\n\
assert(shared_refusal and shared_refusal:find('prose is read-only', 1, true), 'the library cannot assign prose: ' .. tostring(shared_refusal))\n\
return prose\n\
```\n"
    );
    let out = run_args(md, "{\"query\":\"x\"}")
        .await
        .expect("the run succeeds");
    assert_eq!(out, "The real prose.");
}

#[tokio::test]
async fn getmetatable_on_g_returns_the_shared_librarys_metatable_and_edits_apply_live() {
    let md = args_prompt!(
        "# T\n\n\
```lua shared\n\
author_mt = { __index = { tone = 'friendly' } }\n\
setmetatable(_G, author_mt)\n\
```\n\n\
## Only\n\n\
```lua\n\
assert(getmetatable(_G) == author_mt, 'getmetatable(_G) is the library metatable')\n\
getmetatable(_G).__index = function(_, key) return 'live ' .. key end\n\
assert(tone == 'live tone', 'an edit applies at once')\n\
getmetatable(_G).__newindex = function() end\n\
assert(not pcall(function() argv = 'x' end), 'an edit cannot free argv')\n\
return tone\n\
```\n"
    );
    let out = run_args(md, "{\"query\":\"x\"}")
        .await
        .expect("the run succeeds");
    assert_eq!(out, "live tone");
}
