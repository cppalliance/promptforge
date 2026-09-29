//! `fanout`'s collection and worker arguments: the results it returns,
//! the members each arm receives, the workers it resolves, and the
//! arguments it refuses.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_returns_structured_results() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', list_from_section('### Items'))\n\
assert(r[1].text == 'alpha-1')\n\
assert(r[1].ok == true)\n\
assert(r[1].item == 'alpha')\n\
assert(r[1].exhausted == false)\n\
assert(r[2].text == 'beta-2')\n\
assert(r[2].ok == true)\n\
assert(r[2].item == 'beta')\n\
assert(r[2].exhausted == false)\n\
assert(tostring(r[1]) == r[1].text)\n\
assert(table.concat(r, ',') == 'alpha-1,beta-2')\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item .. '-' .. sys.index\n```\n\n\
Do work.\n\n\
### Items\n\n\
- alpha\n\
- beta\n"
    );
    let out = run(&fixture(md), "", &[], &TestStore::new(), silent())
        .await
        .expect("fanout must return structured results");
    assert_eq!(out, "ok");
}

/// An off-walk child section still runs as a fanout worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_walk_worker_runs_as_a_fanout_arm() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', list_from_section('### Items'))\n\
return r[1].text .. ',' .. r[2].text\n\
```\n\n\
### Worker\n\n\
---\n\n\
```lua\nreturn item .. '-done'\n```\n\n\
### Items\n\n\
- alpha\n\
- beta\n"
    );
    let out = run_offline(md)
        .await
        .expect("an off-walk worker must run as a fanout arm");
    assert_eq!(out, "alpha-done,beta-done");
}

/// A collection larger than the old default item cap (1024) runs through the
/// prompt-level path: the parent's Lua builds the table,
/// `collection_to_items` converts it, and `fanout` dispatches every member.
/// The worker is pure Lua with an immediate return, so the run works
/// without a client and stays fast.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_accepts_a_prompt_built_collection_over_the_old_default_cap() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
local members = {}\n\
for i = 1, 1025 do members[i] = 'm' .. i end\n\
local r = fanout('### Worker', members)\n\
assert(#r == 1025, 'every member ran')\n\
return r[1025].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
return item\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("a collection over the old default cap must succeed");
    assert_eq!(out, "m1025");
}

/// Array members arrive as themselves, in collection order: a string stays a
/// string, a number a number, a boolean a boolean, a nested table a table.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_collection_array_members_arrive_as_themselves_in_order() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {'b', 2, true, {nested='x'}})\n\
assert(#r == 4)\n\
assert(r[1].text == 'string:b')\n\
assert(r[2].text == 'number:2')\n\
assert(r[3].text == 'boolean:true')\n\
assert(r[4].text == 'table:x')\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\n\
if type(item) == 'table' then return 'table:' .. item.nested end\n\
return type(item) .. ':' .. tostring(item)\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("array members must arrive as themselves");
    assert_eq!(out, "ok");
}

/// Hash members arrive as pair tables (`item.key` / `item.value`), and
/// `.item` on each arm result returns the same pair table.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_collection_hash_members_arrive_as_pair_tables() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {alpha=1, beta='two'})\n\
assert(#r == 2)\n\
local seen = {}\n\
for i = 1, #r do\n\
  assert(type(r[i].item) == 'table')\n\
  assert(r[i].item.key ~= nil and r[i].item.value ~= nil)\n\
  seen[r[i].text] = true\n\
end\n\
assert(seen['alpha=1'] and seen['beta=two'])\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item.key .. '=' .. tostring(item.value)\n```\n"
    );
    let out = run_offline(md)
        .await
        .expect("hash members must arrive as pair tables");
    assert_eq!(out, "ok");
}

/// An empty collection is an error: no work is likely a bug (a list section
/// that parsed empty, a wrong variable), so the fanout is rejected before
/// any scheduling rather than returning zero results.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_collection_empty_errors() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
fanout('### Worker', {})\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item\n```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("an empty collection must error");
    let rendered = error.to_string();
    assert!(
        rendered.contains("empty collection"),
        "error was: {rendered}"
    );
}

/// `.item` on each arm result returns the member value as a Lua value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_collection_item_round_trips_members() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {1, 'two', {n=3}})\n\
assert(r[1].item == 1)\n\
assert(r[2].item == 'two')\n\
assert(type(r[3].item) == 'table' and r[3].item.n == 3)\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\nreturn 'done'\n```\n"
    );
    let out = run_offline(md)
        .await
        .expect(".item must round-trip each member");
    assert_eq!(out, "ok");
}

/// A worker resolves as a sibling: a top-level worker marked off-walk is
/// shared infrastructure the walk never visits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_worker_resolves_as_a_sibling() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
local r = fanout('## Worker', {'x'})\n\
return r[1].text\n\
```\n\n\
## Worker\n\n\
---\n\n\
```lua\nreturn item .. '-done'\n```\n"
    );
    let out = run_offline(md)
        .await
        .expect("a sibling worker must resolve");
    assert_eq!(out, "x-done");
}

/// One off-walk sibling worker is shared by two sibling callers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_worker_shared_by_two_sibling_callers() {
    let md = flow_prompt!(
        "\
## A\n\n\
```lua\n\
local r = fanout('## Worker', {'a'})\n\
store.write('a.txt', r[1].text)\n\
```\n\n\
## B\n\n\
```lua\n\
local r = fanout('## Worker', {'b'})\n\
return store.read('a.txt') .. ',' .. r[1].text\n\
```\n\n\
## Worker\n\n\
---\n\n\
```lua\nreturn item .. '-done'\n```\n"
    );
    let out = run_offline(md)
        .await
        .expect("one worker must serve two sibling callers");
    assert_eq!(out, "a-done,b-done");
}

/// A sibling's child (a niece) is outside the caller's visible set and
/// resolves as not-found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_niece_worker_is_not_visible() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
fanout('### Niece', {'x'})\n\
```\n\n\
## Other\n\n\
```lua\nlocal x = 1\n```\n\n\
### Niece\n\n\
```lua\nreturn item\n```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("a niece worker must not be visible");
    assert!(
        error.to_string().contains("not found"),
        "error was: {error}"
    );
}

/// The two-string form errors at the boundary, pointing at
/// `list_from_section`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_string_second_parameter_errors_pointing_at_list_from_section() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
fanout('### Worker', '### Items')\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item\n```\n\n\
### Items\n\n\
- alpha\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("the two-string form must be rejected");
    let rendered = error.to_string();
    assert!(
        rendered.contains("fanout's second parameter is a collection"),
        "error was: {rendered}"
    );
    assert!(
        rendered.contains("list_from_section"),
        "the error must point at list_from_section: {rendered}"
    );
}

/// A number or boolean second parameter is not a collection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_number_and_boolean_second_parameters_error() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
local ok_n, err_n = pcall(fanout, '### Worker', 5)\n\
assert(not ok_n and tostring(err_n):find('collection'), tostring(err_n))\n\
local ok_b, err_b = pcall(fanout, '### Worker', true)\n\
assert(not ok_b and tostring(err_b):find('collection'), tostring(err_b))\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item\n```\n"
    );
    let out = run_offline(md)
        .await
        .expect("number and boolean second parameters must error");
    assert_eq!(out, "ok");
}

/// A function member cannot cross into an arm; the error names its index.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_function_member_errors_naming_the_index() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
fanout('### Worker', {'a', function() end})\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item\n```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("a function member must error");
    let rendered = error.to_string();
    assert!(rendered.contains("index 2"), "error was: {rendered}");
    assert!(rendered.contains("function"), "error was: {rendered}");
}

/// A non-scalar (table) key cannot be represented; the error says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_table_keyed_member_errors() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
local t = {}\n\
t[{}] = 'x'\n\
fanout('### Worker', t)\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item\n```\n"
    );
    let error = run_offline(md).await.expect_err("a table key must error");
    assert!(
        error
            .to_string()
            .contains("key must be a string, number, or boolean"),
        "error was: {error}"
    );
}

/// A list section is not a worker template; naming one as the worker errors.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_worker_that_is_a_list_section_errors() {
    let md = flow_prompt!(
        "\
## Parent\n\n\
```lua\n\
fanout('### Items', {'x'})\n\
```\n\n\
### Items\n\n\
- a\n\
- b\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("a list section is not a worker template");
    assert!(
        error
            .to_string()
            .contains("is a list section, not a worker template"),
        "error was: {error}"
    );
}
