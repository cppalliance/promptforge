//! Store operation reports: the pair each operation reports from a chunk
//! and from the shared library, their order against later Lua side
//! effects, and their freedom from payloads.

use super::*;

#[test]
fn store_exists_reports_its_pair_from_the_shared_library() {
    // The shared library loads through the direct store closures, before
    // the yield shims install; `store.exists` must report its pair there
    // exactly as it does from a chunk, for both a present and an absent
    // file, and for a failure the library catches with `pcall`.
    let shared = program("store.exists('seed.txt')\nstore.exists('missing.txt')");
    let access = fresh_access();
    store_view(&access)
        .write("seed.txt", b"planted")
        .expect("the memory store can prepare a file");
    let recorder = Arc::new(Recorder::default());
    let mut vm =
        SectionVm::new(&test_nonce(), recorder.emitter(), "Shared").expect("VM must build");
    vm.inject_values("", &json!({}), &access)
        .expect("Engine values must inject");
    let observer = recorder.emitter().clone();
    vm.install_engine_globals(&observer, "Shared")
        .expect("Engine globals must install");
    vm.replay_shared(&shared, recorder.emitter(), "Shared")
        .expect("the shared library must load");
    vm.teardown(recorder.emitter(), "Shared");
    assert_eq!(
        recorder.observations(),
        [
            detail::LUA_SHARED_LOAD_STARTED,
            detail::VFS_EXISTS_SUCCEEDED,
            detail::VFS_EXISTS_SUCCEEDED,
            detail::LUA_SHARED_LOAD_SUCCEEDED,
            detail::LUA_TEARDOWN_STARTED,
            detail::LUA_TEARDOWN_SUCCEEDED,
        ]
        .into_iter()
        .map(|detail| ("Shared".to_owned(), detail.clone()))
        .collect::<Vec<_>>()
    );

    let failing_shared = program(
        "local ok = pcall(function() return store.exists('a.txt') end)\n\
         assert(not ok, 'the failing backend must refuse the check')",
    );
    let recorder = Arc::new(Recorder::default());
    let mut vm =
        SectionVm::new(&test_nonce(), recorder.emitter(), "Shared").expect("VM must build");
    vm.inject_values("", &json!({}), &failing_access())
        .expect("Engine values must inject");
    let observer = recorder.emitter().clone();
    vm.install_engine_globals(&observer, "Shared")
        .expect("Engine globals must install");
    vm.replay_shared(&failing_shared, recorder.emitter(), "Shared")
        .expect("the pcall-caught failure must not abort the load");
    vm.teardown(recorder.emitter(), "Shared");
    assert_eq!(
        recorder.observations(),
        [
            detail::LUA_SHARED_LOAD_STARTED,
            detail::VFS_EXISTS_FAILED,
            detail::LUA_SHARED_LOAD_SUCCEEDED,
            detail::LUA_TEARDOWN_STARTED,
            detail::LUA_TEARDOWN_SUCCEEDED,
        ]
        .into_iter()
        .map(|detail| ("Shared".to_owned(), detail.clone()))
        .collect::<Vec<_>>()
    );
}

#[test]
fn store_reports_are_ordered_exact_and_payload_free_on_failure() {
    let recorder = Arc::new(Recorder::default());
    let observer = recorder.emitter().clone();
    let access = fresh_access();
    let source = "store.write('secret/path.txt', 'private contents')\n\
                      store.read('secret/path.txt')\n\
                      store.str_replace('secret/path.txt', 'missing secret', 'replacement')";
    let error = run_chunk(
        source,
        "private input",
        &json!({ "id": 1, "when": "t" }),
        &access,
        &observer,
        "Gather",
    )
    .expect_err("the missing anchor must fail");
    assert!(matches!(error, Error::Lua(_) | Error::LuaRuntime { .. }));

    let observations = recorder.observations();
    assert_eq!(
        observations,
        vec![
            ("Gather".to_string(), detail::VFS_WRITE_SUCCEEDED.clone()),
            ("Gather".to_string(), detail::VFS_READ_SUCCEEDED.clone()),
            ("Gather".to_string(), detail::VFS_REPLACE_FAILED.clone()),
        ]
    );
    let trace = format!("{observations:?}");
    for payload in [
        "secret/path.txt",
        "private contents",
        "missing secret",
        "replacement",
        "private input",
    ] {
        assert!(
            !trace.contains(payload),
            "observation leaked payload {payload:?}: {trace}"
        );
    }
}

/// One store operation's success and failure observation pair, driven
/// through the block-code closures against both a working and a failing
/// backend.
#[expect(
    clippy::too_many_lines,
    reason = "one table holds every operation's pair beside its prepare step"
)]
#[test]
fn every_store_operation_reports_its_exact_success_and_failure() {
    struct Case {
        source: &'static str,
        success: Observation,
        failure: Observation,
        prepare: fn(&Arc<Access>),
    }

    fn empty(_access: &Arc<Access>) {}

    fn existing(access: &Arc<Access>) {
        store_view(access)
            .write("a.txt", b"old")
            .expect("the memory store can prepare a file");
    }

    let cases = [
        Case {
            source: "store.write('a.txt', 'new')",
            success: detail::VFS_WRITE_SUCCEEDED,
            failure: detail::VFS_WRITE_FAILED,
            prepare: empty,
        },
        Case {
            source: "store.append('a.txt', 'new')",
            success: detail::VFS_APPEND_SUCCEEDED,
            failure: detail::VFS_APPEND_FAILED,
            prepare: empty,
        },
        Case {
            source: "store.read('a.txt')",
            success: detail::VFS_READ_SUCCEEDED,
            failure: detail::VFS_READ_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.read('a.txt', 1, 1)",
            success: detail::VFS_READ_SUCCEEDED,
            failure: detail::VFS_READ_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.read_numbered('a.txt')",
            success: detail::VFS_READ_NUMBERED_SUCCEEDED,
            failure: detail::VFS_READ_NUMBERED_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.read_numbered('a.txt', 1, 1)",
            success: detail::VFS_READ_NUMBERED_SUCCEEDED,
            failure: detail::VFS_READ_NUMBERED_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.str_replace('a.txt', 'old', 'new')",
            success: detail::VFS_REPLACE_SUCCEEDED,
            failure: detail::VFS_REPLACE_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.delete('a.txt')",
            success: detail::VFS_DELETE_SUCCEEDED,
            failure: detail::VFS_DELETE_FAILED,
            prepare: existing,
        },
        Case {
            source: "local matches = store.glob('*.txt')",
            success: detail::VFS_GLOB_SUCCEEDED,
            failure: detail::VFS_GLOB_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.exists('a.txt')",
            success: detail::VFS_EXISTS_SUCCEEDED,
            failure: detail::VFS_EXISTS_FAILED,
            prepare: existing,
        },
    ];

    for case in cases {
        let access = fresh_access();
        (case.prepare)(&access);
        let recorder = Arc::new(Recorder::default());
        let observer = recorder.emitter().clone();
        run_chunk(case.source, "", &json!({}), &access, &observer, "Store")
            .expect("the memory store operation succeeds");
        assert_eq!(
            recorder.observations(),
            vec![("Store".to_owned(), case.success.clone())],
            "wrong success observation for {}",
            case.source
        );

        let access = failing_access();
        let recorder = Arc::new(Recorder::default());
        let observer = recorder.emitter().clone();
        let error = run_chunk(case.source, "", &json!({}), &access, &observer, "Store")
            .expect_err("the failing backend rejects every operation");
        assert!(
            matches!(error, Error::Lua(_) | Error::LuaRuntime { .. }),
            "a store failure surfaces through the Lua boundary, got {error:?}"
        );
        assert_eq!(
            recorder.observations(),
            vec![("Store".to_owned(), case.failure.clone())],
            "wrong failure observation for {}",
            case.source
        );
    }
}

#[test]
fn store_observations_happen_before_later_lua_side_effects() {
    // Each store report is pushed once its operation has landed, before
    // the chunk's next statement runs: the author's `log` checkpoint
    // between the two writes must land between their two reports. Reports
    // buffered until chunk end would put the checkpoint first.
    let access = fresh_access();
    let recorder = Arc::new(Recorder::default());
    let observer = recorder.emitter().clone();

    run_chunk(
        "store.write('first.txt', '')\nlog('mark')\nstore.write('second.txt', '')",
        "",
        &json!({}),
        &access,
        &observer,
        "Store",
    )
    .expect("both writes succeed");

    assert_eq!(
        recorder.observations(),
        vec![
            ("Store".to_owned(), detail::VFS_WRITE_SUCCEEDED),
            ("Store".to_owned(), Observation::Lua("mark".to_owned())),
            ("Store".to_owned(), detail::VFS_WRITE_SUCCEEDED),
        ],
        "each write's report lands before the next Lua statement runs"
    );
    assert_eq!(
        store_view(&access)
            .glob("**")
            .expect("the memory store can glob"),
        vec!["first.txt".to_owned(), "second.txt".to_owned()],
        "both writes landed before the chunk returned"
    );
}
