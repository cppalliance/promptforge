//! How store failures reach the author and the run: caught error tables,
//! uncaught run errors, the store view's confinement, and write conflicts
//! from the shared library.

use super::*;

use crate::{RunErrorKind, RunResult};

/// A store failure the author caught with `pcall` is an error table of
/// kind `store`: its `reason`, `path`, and `anchor` fields and its
/// integer `count` read from Lua, and `tostring` returns the
/// model-facing message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caught_store_error_is_a_store_table_with_reason_fields_and_message() {
    let md = flow_prompt!(
        "\
## Only\n\n\
```lua\n\
local ok, err = pcall(function() return store.read('missing.txt') end)\n\
assert(ok == false, 'the missing read fails')\n\
assert(err.kind == 'store', 'kind is store, got ' .. tostring(err.kind))\n\
assert(err.reason == 'not_found', 'reason is not_found, got ' .. tostring(err.reason))\n\
assert(err.path == 'missing.txt', 'the path field names the file')\n\
assert(tostring(err) == 'file not found in store: missing.txt', 'tostring is the message')\n\
store.write('a.txt', 'na na na')\n\
local ok2, err2 = pcall(function() store.str_replace('a.txt', 'na', 'la') end)\n\
assert(ok2 == false, 'the ambiguous anchor fails')\n\
assert(err2.reason == 'anchor', 'reason is anchor, got ' .. tostring(err2.reason))\n\
assert(err2.anchor == 'na', 'the anchor field names the anchor')\n\
assert(type(err2.count) == 'number' and err2.count % 1 == 0, 'count is an integer')\n\
assert(err2.count == 3, 'count is the match count')\n\
assert(tostring(err2):find('include more surrounding text', 1, true) ~= nil, 'the message carries the hint')\n\
return 'ok'\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("the caught store errors carry the documented shape");
    assert_eq!(out, "ok");
}

/// A store argument type error stays kind `lua`, like every other Engine
/// function's argument errors: only what the VFS reports is kind `store`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_argument_type_error_is_kind_lua() {
    let md = flow_prompt!(
        "\
## Only\n\n\
```lua\n\
local ok, err = pcall(function() store.write(123, 'x') end)\n\
assert(ok == false, 'the non-string path fails')\n\
assert(err.kind == 'lua', 'an argument error is kind lua, got ' .. tostring(err.kind))\n\
local ok2, err2 = pcall(function() store.glob('a') end)\n\
return 'ok'\n\
```\n"
    );
    let out = run_offline(md)
        .await
        .expect("the argument type error surfaces as a lua error");
    assert_eq!(out, "ok");
}

/// An uncaught store failure ends the run as a `Store` run error whose
/// message is the model-facing rendering.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_uncaught_store_failure_ends_the_run_as_a_store_error() {
    let md = flow_prompt!(
        "\
## Only\n\n\
```lua\n\
return store.read('missing.txt')\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("an uncaught store failure must fail the run");
    assert_eq!(
        error.kind(),
        RunErrorKind::Store,
        "the run fails as a store error: {error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("file not found in store: missing.txt"),
        "the message is the store's rendering: {error}"
    );
}

/// An uncaught store failure in the H1 pass ends the run as a `Store`
/// run error: the H1 blocks run through the same coroutine machinery as
/// any section.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_uncaught_store_failure_in_h1_ends_the_run_as_a_store_error() {
    let md = flow_prompt!(
        "\
```lua\n\
return store.read('missing.txt')\n\
```\n\n\
## Only\n\n\
Done.\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("an uncaught store failure in H1 must fail the run");
    assert_eq!(
        error.kind(),
        RunErrorKind::Store,
        "the H1 failure is a store error: {error:?}"
    );
}

/// A caught store error raised again, after another suspending call,
/// keeps its classification: the run ends as a `Store` run error whose
/// message is the original rendering.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caught_store_error_raised_again_after_a_suspending_call_keeps_store() {
    let md = flow_prompt!(
        "\
## First\n\n\
```lua\n\
local ok, err = pcall(function() return store.read('missing.txt') end)\n\
assert(ok == false, 'the missing read fails')\n\
call('### Sibling', '')\n\
error(err)\n\
```\n\n\
### Sibling\n\n\
```lua\n\
return 'sibling'\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("the re-raised store error must fail the run");
    assert_eq!(
        error.kind(),
        RunErrorKind::Store,
        "the re-raised error keeps its store classification: {error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("file not found in store: missing.txt"),
        "the message is the original rendering: {error}"
    );
}

/// A store declared at `/` covers only its own mount: a real directory
/// mounted beneath it lies outside the store view, so a file that
/// directory holds reads as absent from the store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_at_the_root_cannot_reach_a_host_mount_beneath_it() {
    let md = flow_prompt!(
        "\
## Only\n\n\
```lua\n\
local ok, err = pcall(function() return store.read('host/secret.txt') end)\n\
assert(ok == false, 'the host path is not in the store')\n\
assert(err.reason == 'not_found', 'the host file is simply absent: ' .. tostring(err))\n\
return 'ok'\n\
```\n"
    );
    let vfs = promptforge_vfs::VfsRef::builder()
        .store("/", promptforge_vfs::MemoryBackend::new())
        .mount("/host", promptforge_vfs::MemoryBackend::new())
        .build();
    // The real mount holds the file; the store view reads only its own
    // mount.
    vfs.acquire(promptforge_vfs::Origin::new("host seeding"))
        .expect("the handle acquires")
        .write("/host/secret.txt", b"secret")
        .expect("the host mount seeds");
    let out = run_fixture(md, "exec-flow", EXECUTION, "", Some(vfs))
        .await
        .result
        .expect("the store view is confined to its own mount");
    assert_eq!(out, "ok");
}

/// The shared library replays in every section VM through the direct
/// store closures, so its top-level write runs under each arm's own
/// identity: the second arm's load meets the first's standing write
/// claim and the run ends with a determinism violation, even though the
/// write never yields through the scheduler.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shared_library_write_across_two_arms_ends_the_run_with_determinism() {
    let md = flow_prompt!(
        "\
```lua shared\n\
store.write('shared.txt', 'seed')\n\
```\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {'alpha', 'beta'})\n\
return r[1].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
return item\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("the shared library's cross-arm write must end the run");
    assert_eq!(
        error.kind(),
        RunErrorKind::Determinism,
        "the load-time conflict is a determinism violation: {error:?}"
    );
}

/// A conflict the shared library caught with its own `pcall` still ends
/// the run with a determinism violation: the direct closures record the
/// conflict, and the load's setup reads it when the load returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shared_library_conflict_caught_with_pcall_still_ends_the_run_with_determinism() {
    let md = flow_prompt!(
        "\
```lua shared\n\
local ok = pcall(function() store.write('shared.txt', 'seed') end)\n\
```\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {'alpha', 'beta'})\n\
return r[1].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
return item\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("the pcall-caught load conflict must still end the run");
    assert_eq!(
        error.kind(),
        RunErrorKind::Determinism,
        "a caught load-time conflict is still fatal: {error:?}"
    );
}

/// The shared library the captured-store cases load: `store.write`
/// captured in a local at load time, reached later only through a helper.
const CAPTURED_STORE_WRITE: &str = "\
```lua shared\n\
local write = store.write\n\
function save_note(path, text)\n\
  write(path, text)\n\
end\n\
```\n\n";

/// A store function the shared library captured at load time follows the
/// load phase: called after load it yields like `store.*`, so two arms
/// racing on one path through it end the run with a determinism violation
/// even though each arm caught its own failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_captured_store_function_conflict_caught_with_pcall_still_ends_the_run_with_determinism()
{
    let md = [
        flow_prompt!(""),
        CAPTURED_STORE_WRITE,
        "## Parent\n\n\
```lua\n\
local r = fanout('### Worker', {'alpha', 'beta'})\n\
return r[1].text\n\
```\n\n\
### Worker\n\n\
```lua\n\
pcall(save_note, 'note.txt', item)\n\
return item\n\
```\n",
    ]
    .concat();
    let error = run_offline(&md)
        .await
        .expect_err("the captured function's caught conflict must still end the run");
    assert_eq!(
        error.kind(),
        RunErrorKind::Determinism,
        "a conflict through a captured store function is fatal: {error:?}"
    );
}

/// A store function the shared library captured at load time reaches the
/// Harness as an `Effect::Store` when called after load, like `store.*` does.
#[test]
fn a_captured_store_function_called_after_load_reaches_the_host_as_a_store_effect() {
    use super::super::super::context::{parse, test_context};
    use super::super::super::serial_driver::perform_locally;
    use crate::{Effect, EffectRecord, Run};
    use promptforge_lua::StoreOp;

    let md = [
        flow_prompt!(""),
        CAPTURED_STORE_WRITE,
        "## Only\n\n\
```lua\n\
save_note('note.txt', 'kept')\n\
return 'saved'\n\
```\n",
    ]
    .concat();
    let run = Run::new(Arc::new(parse(&md)), "", test_context(EXECUTION));
    let mut records = Vec::new();
    let (result, _) = crate::test_support::drive(run, |_, effect: &Effect| {
        records.push(effect.record());
        perform_locally(effect, &mut |effect| {
            panic!("the fixture issues no model round: {effect:?}")
        })
    });
    assert!(
        matches!(&result, RunResult::Ok(text) if text == "saved"),
        "the run succeeds: {result:?}"
    );
    assert!(
        records.iter().any(|record| matches!(
            record,
            EffectRecord::Store { op: StoreOp::Write { path, .. } } if path == "note.txt"
        )),
        "the captured write is performed by the host: {records:?}"
    );
}
