//! Lazy `prose` behavior and the `reply` register's removal: the pending
//! Markdown buffer installs as a fresh read-only lazy `prose` template
//! before each Lua coroutine starts, every `{{ }}` substitution evaluates
//! once on the first runtime read and memoizes, unconsumed buffers discard
//! at section end, and no `reply` global or automatic result handoff
//! crosses a section boundary.

use super::*;

/// The frontmatter every lazy-prose test shares, fused into the prompt
/// literal at compile time so a test states only its body.
macro_rules! prose_prompt {
    ($body:literal) => {
        concat!(
            "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n",
            $body
        )
    };
}

#[tokio::test]
async fn prose_snapshots_section_state_at_the_first_read() {
    // The template's `{{ var.word }}` resolves when Lua first reads
    // `prose`, not when the buffer installs: the mutation before the read
    // is visible in the rendered string.
    let md = prose_prompt!(
        "## Only\n\n\
The word is {{ var.word }}.\n\n\
```lua\nvar.word = 'mutated'\nreturn prose\n```\n"
    );
    let out = run_offline(md).await.expect("the run succeeds");
    assert_eq!(out, "The word is mutated.");
}

#[tokio::test]
async fn prose_memoizes_after_the_first_read() {
    // One evaluation per prose-Lua pair: a mutation after the first read
    // never reaches the memoized string.
    let md = prose_prompt!(
        "## Only\n\n\
The word is {{ var.word }}.\n\n\
```lua\n\
var.word = 'one'\n\
local first = prose\n\
var.word = 'two'\n\
assert(prose == first, 'a later read returns the memoized string')\n\
return prose\n\
```\n"
    );
    let out = run_offline(md).await.expect("the run succeeds");
    assert_eq!(out, "The word is one.");
}

#[tokio::test]
async fn unread_prose_never_evaluates_and_never_errors() {
    // The template carries an unclosed `{{` and a missing key; a block
    // that never reads `prose` runs clean because neither is evaluated.
    let md = prose_prompt!(
        "## Only\n\n\
An unclosed {{ placeholder and a {{ var.missing }} key.\n\n\
```lua\nreturn 'ok'\n```\n"
    );
    let out = run_offline(md)
        .await
        .expect("unread prose must not evaluate");
    assert_eq!(out, "ok");
}

#[tokio::test]
async fn a_second_prose_lua_pair_evaluates_fresh() {
    // Every prose-Lua pair owns a separate lazy value: the second pair's
    // read renders its own buffer against the state at its first read,
    // while the first pair's memoized string survives in `var`.
    let md = prose_prompt!(
        "## Only\n\n\
First: {{ var.word }}.\n\n\
```lua\n\
var.word = 'one'\n\
var.first = prose\n\
```\n\n\
Second: {{ var.word }}.\n\n\
```lua\n\
var.word = 'two'\n\
assert(var.first == 'First: one.', 'the first pair keeps its memo')\n\
assert(prose == 'Second: two.', 'the second pair renders fresh')\n\
return prose\n\
```\n"
    );
    let out = run_offline(md).await.expect("the run succeeds");
    assert_eq!(out, "Second: two.");
}

#[tokio::test]
async fn prose_assignment_is_rejected() {
    let md = prose_prompt!(
        "## Only\n\n\
Some template.\n\n\
```lua\n\
local ok, err = pcall(function() prose = 'x' end)\n\
assert(not ok, 'assigning prose must fail')\n\
assert(tostring(err):match('read%-only'), 'the error names the read-only contract: ' .. tostring(err))\n\
return 'ok'\n\
```\n"
    );
    let out = run_offline(md).await.expect("the run succeeds");
    assert_eq!(out, "ok");
}

#[tokio::test]
async fn recursive_prose_reference_is_rejected_at_the_read_site() {
    let md = prose_prompt!(
        "## Only\n\n\
Recursion: {{ prose }}.\n\n\
```lua\n\
local ok, err = pcall(function() return prose end)\n\
assert(not ok, 'a recursive {{ prose }} must fail')\n\
assert(tostring(err):match('prose'), 'the error names prose: ' .. tostring(err))\n\
return 'ok'\n\
```\n"
    );
    let out = run_offline(md).await.expect("the run succeeds");
    assert_eq!(out, "ok");
}

#[tokio::test]
async fn pcall_catches_a_read_site_substitution_error() {
    let md = prose_prompt!(
        "## Only\n\n\
Missing: {{ var.missing }}.\n\n\
```lua\n\
local ok, err = pcall(function() return prose end)\n\
assert(not ok, 'the read must fail')\n\
assert(tostring(err):match('missing'), 'the substitution error surfaces: ' .. tostring(err))\n\
return 'caught'\n\
```\n"
    );
    let out = run_offline(md).await.expect("the run succeeds");
    assert_eq!(out, "caught");
}

#[tokio::test]
async fn an_uncaught_read_error_fails_the_run() {
    let md = prose_prompt!(
        "## Only\n\n\
Missing: {{ var.missing }}.\n\n\
```lua\nreturn prose\n```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("an uncaught read error must fail the run");
    let rendered = error.to_string();
    assert!(
        rendered.contains("missing {{ var.missing }}"),
        "the substitution error reaches the run failure: {rendered}"
    );
}

#[tokio::test]
async fn unconsumed_prose_is_discarded_at_section_end() {
    // The first section's buffer is never followed by a Lua fence; it
    // discards without evaluation, so its bad placeholder cannot fail.
    let md = prose_prompt!(
        "## First\n\n\
{{ var.missing }} is never read here.\n\n\
## Second\n\n\
```lua\nreturn 'ok'\n```\n"
    );
    let out = run_offline(md)
        .await
        .expect("an unconsumed buffer discards at section end");
    assert_eq!(out, "ok");
}

#[tokio::test]
async fn trailing_prose_after_the_last_fence_is_inert() {
    let md = prose_prompt!(
        "## Only\n\n\
```lua\nreturn 'ok'\n```\n\n\
Trailing {{ var.missing }} commentary.\n"
    );
    let out = run_offline(md).await.expect("trailing prose is inert");
    assert_eq!(out, "ok");
}

#[tokio::test]
async fn prose_with_no_pending_markdown_renders_empty() {
    let md = prose_prompt!(
        "## Only\n\n\
```lua\n\
assert(prose == '', 'no pending markdown renders as empty')\n\
return 'ok'\n\
```\n"
    );
    let out = run_offline(md).await.expect("the run succeeds");
    assert_eq!(out, "ok");
}

#[tokio::test]
async fn the_reply_register_is_gone_and_nothing_hands_off_at_fall_through() {
    // `reply` is an ordinary global now: a write dies with the section VM,
    // and the next section sees nil - no register, no roll-forward.
    let md = prose_prompt!(
        "## First\n\n\
```lua\nreply = 'custom'\n```\n\n\
## Second\n\n\
```lua\n\
assert(reply == nil, 'no reply register crosses a section boundary')\n\
return 'ok'\n\
```\n"
    );
    let out = run_offline(md).await.expect("the run succeeds");
    assert_eq!(out, "ok");
}

#[tokio::test]
async fn lazy_prose_composes_with_a_shared_library_metatable() {
    // A shared library's `_G` metatable keeps working after the prose
    // guard installs over it: its `__newindex` still captures ordinary
    // globals, and `prose` stays read-only and lazy.
    let md = prose_prompt!(
        "# T\n\n\
```lua shared\n\
captured = {}\n\
setmetatable(_G, { __newindex = function(_, k, v) captured[k] = v end })\n\
```\n\n\
## Only\n\n\
Hello {{ var.word }}.\n\n\
```lua\n\
var.word = 'world'\n\
plain = 'x'\n\
assert(captured.plain == 'x', 'the shared __newindex still captures')\n\
local ok = pcall(function() prose = 'y' end)\n\
assert(not ok, 'prose stays read-only over a shared metatable')\n\
return prose\n\
```\n"
    );
    let out = run_offline(md).await.expect("the run succeeds");
    assert_eq!(out, "Hello world.");
}
