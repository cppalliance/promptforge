//! `list_from_section`: the items it returns, the sections it resolves,
//! and the sections it refuses.

use super::*;

/// `list_from_section` returns a sibling list section's pre-parsed bullet
/// items as a Lua array of strings, addressed by heading string.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_returns_bullet_items() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
local items = list_from_section('## List')\n\
assert(type(items) == 'table')\n\
assert(#items == 2)\n\
assert(items[1] == 'alpha')\n\
assert(items[2] == 'beta')\n\
return 'ok'\n\
```\n\n\
## List\n\n\
- alpha\n\
- beta\n"
    );
    let out = run_offline(md)
        .await
        .expect("a sibling list section's bullet items must be returned");
    assert_eq!(out, "ok");
}

/// A numbered list section's items come back in order, markers stripped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_returns_numbered_items() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
local items = list_from_section('## Nums')\n\
assert(#items == 3)\n\
assert(items[1] == 'one' and items[2] == 'two' and items[3] == 'three')\n\
return 'ok'\n\
```\n\n\
## Nums\n\n\
1. one\n\
2. two\n\
3. three\n"
    );
    let out = run_offline(md)
        .await
        .expect("a numbered list section's items must be returned");
    assert_eq!(out, "ok");
}

/// The caller's direct children are in the visible set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_resolves_a_direct_child() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
local items = list_from_section('### Sub')\n\
assert(#items == 2 and items[1] == 'x' and items[2] == 'y')\n\
return 'ok'\n\
```\n\n\
### Sub\n\n\
- x\n\
- y\n"
    );
    let out = run_offline(md)
        .await
        .expect("a direct child list section must be visible");
    assert_eq!(out, "ok");
}

/// A sibling's child (niece), a child's child (grandchild), and the caller
/// itself are all outside the visible set: each resolves as not-found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_hides_nieces_grandchildren_and_the_caller() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
local ok_niece = pcall(list_from_section, '### Niece')\n\
assert(not ok_niece, 'a niece is not visible')\n\
local ok_grand = pcall(list_from_section, '#### Grand')\n\
assert(not ok_grand, 'a grandchild is not visible')\n\
local ok_self = pcall(list_from_section, '## Main')\n\
assert(not ok_self, 'the caller itself is not visible')\n\
return 'ok'\n\
```\n\n\
### Kid\n\n\
- kid-item\n\n\
#### Grand\n\n\
- grand-item\n\n\
## Other\n\n\
```lua\nlocal x = 1\n```\n\n\
### Niece\n\n\
- niece-item\n"
    );
    let out = run_offline(md)
        .await
        .expect("nieces, grandchildren, and the caller must all be invisible");
    assert_eq!(out, "ok");
}

/// The not-found error lists exactly the visible sections - siblings plus
/// direct children - so the error channel cannot leak the rest of the
/// document's structure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_not_found_lists_only_the_visible_sections() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
list_from_section('## Missing')\n\
```\n\n\
### Kid\n\n\
- kid-item\n\n\
#### Grand\n\n\
- grand-item\n\n\
## Sibling\n\n\
```lua\nlocal x = 1\n```\n\n\
### Niece\n\n\
- niece-item\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("an unknown heading must fail");
    let rendered = error.to_string();
    assert!(rendered.contains("not found"), "error was: {rendered}");
    assert!(
        rendered.contains("## Sibling"),
        "a sibling is visible: {rendered}"
    );
    assert!(
        rendered.contains("### Kid"),
        "a direct child is visible: {rendered}"
    );
    let (_, available) = rendered
        .split_once("available sections:")
        .expect("the not-found error must list the visible sections");
    assert!(
        !available.contains("## Main"),
        "the caller is not listed: {rendered}"
    );
    assert!(
        !available.contains("Niece"),
        "a niece is not listed: {rendered}"
    );
    assert!(
        !available.contains("Grand"),
        "a grandchild is not listed: {rendered}"
    );
}

/// Naming a prose section (no pre-parsed items) is the mistake the no-items
/// error exists to catch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_rejects_a_prose_section() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
list_from_section('## Prose')\n\
```\n\n\
## Prose\n\n\
Just prose, no list items here.\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("a prose section has no pre-parsed items");
    let rendered = error.to_string();
    assert!(
        rendered.contains("section `Prose` has no pre-parsed items"),
        "error was: {rendered}"
    );
}
