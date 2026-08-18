use super::run;
use super::*;

#[tokio::test]
async fn falls_through_to_next_section() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn \"second\"\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "second");
}

#[tokio::test]
async fn explicit_return_stops_fall_through() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## First\n\n```lua\nreturn \"first\"\n```\n\n\
## Second\n\n```lua\nreturn \"unreached\"\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "first");
}

#[tokio::test]
async fn generic_result_when_nothing_produced() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\nlocal x = 1\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "done");
}

#[tokio::test]
async fn sys_id_increments_per_section() {
    // First section files nothing and falls through; second returns its id.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn tostring(sys.id)\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "2");
}

// --- H1-only prompts (no ## sections) ---

#[tokio::test]
async fn h1_only_lua_return() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Title\n\n```lua\nreturn \"hello\"\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "hello");
}

#[tokio::test]
async fn h1_only_lua_no_return() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Title\n\n```lua\nlocal x = 1\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "done");
}

// --- Version gate at the top of `run` ---

#[tokio::test]
async fn supported_major_one_proceeds() {
    // A `promptforge: 1` prompt clears the gate and runs to completion.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "ran");
}

#[tokio::test]
async fn unsupported_major_is_refused() {
    // A future major is refused, never silently degraded to major 1.
    let md = "---\nname: t\ndescription: d\npromptforge: 2\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let err = run_offline(md)
        .await
        .expect_err("an unsupported major must be refused");
    assert!(matches!(err, Error::UnsupportedVersion(2)));
}

#[tokio::test]
async fn missing_version_is_not_a_promptforge_prompt() {
    // No `promptforge:` key: not our prompt, so `run` declines with a Parse
    // error rather than executing it.
    let md = "---\nname: t\ndescription: d\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let err = run_offline(md)
        .await
        .expect_err("a prompt with no promptforge version must be declined");
    match err {
        Error::Parse(msg) => assert!(
            msg.contains("not a promptforge prompt"),
            "the Parse message must name the missing version, got: {msg}"
        ),
        other => panic!("expected Parse, got {other:?}"),
    }
}

// --- Cross-section store persistence ---

#[tokio::test]
async fn store_persists_across_sections() {
    // One store is created for the run and threaded to every section. The
    // first section's Lua writes a file; the second, in a fresh context,
    // reads it back - proving the store outlives the context-clearing
    // transition. The read lands in `var`, so it round-trips the value.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Writer\n\n```lua\nstore.write('note.txt', 'carried across')\n```\n\n\
## Reader\n\n```lua\nvar.seen = store.read('note.txt')\nreturn var.seen\n```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent()).await.unwrap();
    assert_eq!(
        out, "carried across",
        "the second section must read what the first wrote"
    );
    // The very same handle still holds the file after the run, confirming
    // both sections shared one store rather than each getting a fresh one.
    assert_eq!(
        store.read("note.txt").expect("read"),
        "carried across",
        "the run's store must retain the written file"
    );
}

// --- Tool-call loop test (exercises the model round trip via a mock) ---
