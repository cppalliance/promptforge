//! The globals a section reads: the `tasks` namespace, `item`, `var` across
//! the walk, `call`, and fanout arms, bare globals in prose, and the `sys`
//! metadata.

use super::*;

/// The `tasks` global is the task namespace (`tasks.spawn` and the
/// waits), not a control-flow table: indexing it by a heading string reads
/// nil, and control flow takes heading strings only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tasks_global_is_the_task_namespace() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
assert(type(tasks) == 'table', 'the tasks namespace is installed')\n\
assert(type(tasks.spawn) == 'function', 'tasks.spawn is a namespace function')\n\
assert(tasks['## Main'] == nil, 'the namespace is not a heading table')\n\
return 'ok'\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("the tasks namespace installs as a plain table");
    assert_eq!(out, "ok");
}

/// A walked section never sees the fanout `item` global: the seed split
/// installs `item` only for an arm, so a regression seeding it on the walk
/// path must fail here.
#[tokio::test]
async fn item_global_is_absent_in_a_walked_section() {
    let md = flow_prompt!(
        "\
## Only\n\n\
```lua\n\
assert(item == nil, 'a walked section must not see the fanout item global')\n\
return 'ok'\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("a walked section runs with no item global");
    assert_eq!(out, "ok");
}

/// `{{ item }}` in walked (non-arm) prose is a substitution error at the
/// read site: the walk pins `item: None`, so only a fanout arm's prose may
/// reference it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn item_in_walked_prose_is_a_substitution_error() {
    let md = flow_prompt!(
        "\
## Only\n\n\
Ask about {{ item }}.\n\n\
```lua\nreturn prose\n```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("{{ item }} outside a fanout arm must fail substitution");
    let rendered = error.to_string();
    assert!(
        rendered.contains("{{ item }} is nil"),
        "the walked section's prose must reject {{ item }}: {rendered}"
    );
}

/// Each fanout arm seeds `var` from a fresh clone of the caller's `var` (the
/// walk's H1-seeded value), so an arm reads the caller's entries but sibling
/// and caller writes never cross arm boundaries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arms_get_fresh_var_clones() {
    let md = flow_prompt!(
        "\
# Test prompt\n\n\
```lua\n\
var.from_h1 = 'seeded'\n\
```\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {'a', 'b'})\n\
assert(var.a == nil and var.b == nil, 'arm writes must not reach the caller')\n\
return r[1].text .. r[2].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
assert(var.from_h1 == 'seeded', 'an arm var clones the caller var in')\n\
local sibling = item == 'a' and 'b' or 'a'\n\
assert(var[sibling] == nil, 'each arm gets a fresh clone')\n\
var[item] = true\n\
return item\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("each arm must get a fresh var clone");
    assert_eq!(out, "ab");
}

/// The walk's `var` table persists: H1's writes seed the top-level walk, and
/// one section's writes reach the next across both fall-through and a jump.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn var_persists_across_sections_fallthrough_and_jump() {
    let md = flow_prompt!(
        "\
# Test prompt\n\n\
```lua\n\
var.from_h1 = 'seed'\n\
```\n\n\
## A\n\n\
```lua\n\
assert(var.from_h1 == 'seed', 'H1 var writes must reach the first H2 section')\n\
var.from_a = 'a'\n\
jump('## C')\n\
```\n\n\
## B\n\n\
```lua\n\
error('the jump must skip B')\n\
```\n\n\
## C\n\n\
```lua\n\
assert(var.from_h1 == 'seed', 'the jump shares the walk var')\n\
assert(var.from_a == 'a', 'the jump keeps the jumper writes')\n\
var.from_c = 'c'\n\
```\n\n\
## D\n\n\
```lua\n\
assert(var.from_c == 'c', 'fall-through after the jumped target keeps var')\n\
return var.from_h1 .. var.from_a .. var.from_c\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("var must persist across the walk");
    assert_eq!(out, "seedac");
}

/// `call` clones the caller's `var` in: the contained chain reads the
/// clone, and its writes are discarded when the chain ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_clones_var_in_and_discards_child_writes() {
    let md = flow_prompt!(
        "\
## Main\n\n\
```lua\n\
var.shared = 'caller'\n\
local r = call('## Sub')\n\
assert(r == 'sub saw caller', 'the child reads the cloned var')\n\
assert(var.child_write == nil, 'child writes must not reach the caller')\n\
return 'ok'\n\
```\n\n\
## Sub\n\n\
```lua\n\
var.child_write = 'sub'\n\
return 'sub saw ' .. var.shared\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("call must clone var in and discard child writes");
    assert_eq!(out, "ok");
}

/// The `var` write guard turns a non-JSON assignment into a run failure at
/// the assigning line (the lua module tests pin the guard's messages).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn var_write_of_a_function_fails_the_run() {
    let md = flow_prompt!(
        "\
## Only\n\n\
```lua\n\
var.f = function() end\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("a function assigned into var must fail the run");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("var.f must be JSON data, got function"),
        "the guard error must name the field and the type: {rendered}"
    );
}

/// A bare global (`x = 42` without `local`) resolves in prose read through
/// the lazy `prose` value, with dotted paths indexing into a table global
/// and a whole table rendering as JSON.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bare_global_resolves_in_prose() {
    let md = flow_prompt!(
        "\
## Only\n\n\
```lua\n\
answer = 42\n\
data = { score = 9 }\n\
```\n\n\
The answer is {{ answer }}; score {{ data.score }}; raw {{ data }}.\n\n\
```lua\nreturn prose\n```\n"
    );
    let out = run_offline(md)
        .await
        .expect("a bare global must resolve in prose");
    assert_eq!(out, "The answer is 42; score 9; raw {\"score\":9}.");
}

/// A `{{ }}` path whose first segment names no known namespace and no bare
/// global is a hard substitution error at the read site.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_bare_global_in_prose_errors() {
    let md = flow_prompt!(
        "\
## Only\n\n\
{{ ghost }} here.\n\n\
```lua\nreturn prose\n```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("a missing bare global must fail substitution");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("unknown namespace or global 'ghost'"),
        "the error must name the missing global: {rendered}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sys_exposes_section_metadata() {
    let md = flow_prompt!(
        "\
## Alpha\n\n\
```lua\n\
assert(sys.section_name == 'Alpha')\n\
assert(sys.execution == 'execute-test')\n\
assert(sys.section_count == 2)\n\
```\n\n\
## Beta\n\n\
```lua\n\
assert(sys.section_name == 'Beta')\n\
assert(sys.section_count == 2)\n\
assert(sys.execution == 'execute-test')\n\
return 'done'\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("sys must expose section metadata");
    assert_eq!(out, "done");
}
