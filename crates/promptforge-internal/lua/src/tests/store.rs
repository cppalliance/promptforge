//! The always-on `store` table: each operation's result, the line bounds
//! of `read` and `read_numbered`, the removed operations, and writes seen
//! through the shared handle.

use super::*;

#[test]
fn store_exists_returns_boolean() {
    let access = fresh_access();
    let store = store_view(&access);
    assert_eq!(
        run_with("return tostring(store.exists('missing.txt'))", &access)
            .unwrap()
            .returned
            .as_deref(),
        Some("false")
    );
    store.write("a.txt", b"hi").expect("write");
    assert_eq!(
        run_with("return tostring(store.exists('a.txt'))", &access)
            .unwrap()
            .returned
            .as_deref(),
        Some("true")
    );
    assert_eq!(
        run_with(
            "store.delete('a.txt')\nreturn tostring(store.exists('a.txt'))",
            &access,
        )
        .unwrap()
        .returned
        .as_deref(),
        Some("false")
    );
}

#[test]
fn store_write_then_read_numbered_returns_numbered_content() {
    let out = run(
        "store.write('a.txt', 'first\\nsecond')\nreturn store.read_numbered('a.txt')",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("1| first\n2| second"));
}

#[test]
fn store_append_extends_the_file() {
    let out = run(
            "store.append('log.txt', 'one\\n')\nstore.append('log.txt', 'two')\nreturn store.read_numbered('log.txt')",
            "",
        )
        .unwrap();
    assert_eq!(out.returned.as_deref(), Some("1| one\n2| two"));
}

#[test]
fn store_str_replace_edits_in_place() {
    let out = run(
            "store.write('a.txt', 'the quick brown fox')\nstore.str_replace('a.txt', 'quick', 'slow')\nreturn store.read_numbered('a.txt')",
            "",
        )
        .unwrap();
    assert_eq!(out.returned.as_deref(), Some("1| the slow brown fox"));
}

#[test]
fn store_delete_then_read_raises() {
    let err = run(
        "store.write('a.txt', 'gone soon')\nstore.delete('a.txt')\nreturn store.read('a.txt')",
        "",
    )
    .expect_err("reading a deleted file must raise");
    let msg = lua_error_message(&err);
    assert!(
        msg.contains("file not found"),
        "the Lua error must include the store message, got: {msg}"
    );
}

#[test]
fn store_inject_is_absent() {
    let out = run("return tostring(store.inject)", "").unwrap();
    assert_eq!(
        out.returned.as_deref(),
        Some("nil"),
        "store.inject was removed; indexing it must yield nil"
    );
    assert!(
        run("store.inject('a.txt')", "").is_err(),
        "calling the removed store.inject must raise"
    );
}

#[test]
fn store_read_lines_is_absent() {
    let out = run("return tostring(store.read_lines)", "").unwrap();
    assert_eq!(
        out.returned.as_deref(),
        Some("nil"),
        "store.read_lines was removed; indexing it must yield nil"
    );
    assert!(
        run("store.read_lines('a.txt')", "").is_err(),
        "calling the removed store.read_lines must raise"
    );
}

#[test]
fn store_read_with_start_only_reads_to_eof() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read('a.txt', 2)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("two\nthree"));
}

#[test]
fn store_read_with_start_and_end_slices_inclusively() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read('a.txt', 2, 2)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("two"));
}

#[test]
fn store_read_clamps_end_to_the_last_line() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read('a.txt', 2, 99)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("two\nthree"));
}

#[test]
fn store_read_beyond_eof_returns_empty() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read('a.txt', 99)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some(""));
}

#[test]
fn store_read_start_below_one_raises() {
    for source in [
        "store.write('a.txt', 'one')\nreturn store.read('a.txt', 0)",
        "store.write('a.txt', 'one')\nreturn store.read('a.txt', -1)",
    ] {
        let err = run(source, "").expect_err("a start below 1 must raise");
        let msg = lua_error_message(&err);
        assert!(
            msg.contains("invalid line range"),
            "the Lua error must include the range message, got: {msg}"
        );
    }
}

#[test]
fn store_read_end_before_start_raises() {
    let err = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read('a.txt', 3, 2)",
        "",
    )
    .expect_err("an end before start must raise");
    let msg = lua_error_message(&err);
    assert!(
        msg.contains("invalid line range"),
        "the Lua error must include the range message, got: {msg}"
    );
}

#[test]
fn store_read_end_without_start_raises() {
    let err = run(
        "store.write('a.txt', 'one')\nreturn store.read('a.txt', nil, 1)",
        "",
    )
    .expect_err("an end without a start must raise");
    let msg = lua_error_message(&err);
    assert!(
        msg.contains("invalid line range"),
        "the Lua error must include the range message, got: {msg}"
    );
}

#[test]
fn store_read_numbered_without_bounds_numbers_from_one() {
    let access = fresh_access();
    let out = run_with(
        "store.write('a.txt', 'first\\nsecond')\nreturn store.read_numbered('a.txt')",
        &access,
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("1| first\n2| second"));
}

#[test]
fn store_read_numbered_numbers_a_slice_absolutely() {
    let access = fresh_access();
    let store = store_view(&access);
    let mut body = String::new();
    for n in 1..=85 {
        use std::fmt::Write as _;
        let _ = writeln!(body, "line{n}");
    }
    store.write("a.txt", body.as_bytes()).expect("write");
    let out = run_with("return store.read_numbered('a.txt', 84, 85)", &access).unwrap();
    assert_eq!(out.returned.as_deref(), Some("84| line84\n85| line85"));
}

#[test]
fn store_read_numbered_pads_across_the_hundred_boundary() {
    let access = fresh_access();
    let store = store_view(&access);
    let mut body = String::new();
    for n in 1..=100 {
        use std::fmt::Write as _;
        let _ = writeln!(body, "line{n}");
    }
    store.write("a.txt", body.as_bytes()).expect("write");
    let out = run_with("return store.read_numbered('a.txt', 99, 100)", &access).unwrap();
    assert_eq!(out.returned.as_deref(), Some(" 99| line99\n100| line100"));
}

#[test]
fn store_read_numbered_clamps_end_to_the_last_line() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read_numbered('a.txt', 2, 99)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("2| two\n3| three"));
}

#[test]
fn store_read_numbered_beyond_eof_returns_empty() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read_numbered('a.txt', 99)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some(""));
}

#[test]
fn store_read_numbered_start_below_one_raises() {
    for source in [
        "store.write('a.txt', 'one')\nreturn store.read_numbered('a.txt', 0)",
        "store.write('a.txt', 'one')\nreturn store.read_numbered('a.txt', -1)",
    ] {
        let err = run(source, "").expect_err("a start below 1 must raise");
        let msg = lua_error_message(&err);
        assert!(
            msg.contains("invalid line range"),
            "the Lua error must include the range message, got: {msg}"
        );
    }
}

#[test]
fn store_read_numbered_end_before_start_raises() {
    let err = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read_numbered('a.txt', 3, 2)",
        "",
    )
    .expect_err("an end before start must raise");
    let msg = lua_error_message(&err);
    assert!(
        msg.contains("invalid line range"),
        "the Lua error must include the range message, got: {msg}"
    );
}

#[test]
fn store_read_numbered_end_without_start_raises() {
    let err = run(
        "store.write('a.txt', 'one')\nreturn store.read_numbered('a.txt', nil, 1)",
        "",
    )
    .expect_err("an end without a start must raise");
    let msg = lua_error_message(&err);
    assert!(
        msg.contains("invalid line range"),
        "the Lua error must include the range message, got: {msg}"
    );
}

#[test]
fn installed_store_read_honors_line_bounds() {
    let access = fresh_access();
    let store = store_view(&access);
    store
        .write("a.txt", b"one\ntwo\nthree\n")
        .expect("the memory store can prepare a file");
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_values("", &json!({}), &access)
        .expect("Engine values must inject");
    let observer = null_emitter();
    vm.install_engine_globals(&observer, "Test")
        .expect("Engine globals must install");

    let sliced = run_scalar(
        &vm,
        &program("return store.read('a.txt', 2, 2)"),
        &null_emitter(),
        "Test",
    )
    .expect("a bounded read must run");
    assert_eq!(sliced.as_deref(), Some("two"));

    let err = run_scalar(
        &vm,
        &program("return store.read('a.txt', 0)"),
        &null_emitter(),
        "Test",
    )
    .expect_err("a start below 1 must raise");
    assert!(
        err.to_string().contains("invalid line range"),
        "the error must include the range message, got: {err}"
    );
}

#[test]
fn installed_store_read_numbered_honors_line_bounds() {
    let access = fresh_access();
    let store = store_view(&access);
    store
        .write("a.txt", b"one\ntwo\nthree\n")
        .expect("the memory store can prepare a file");
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_values("", &json!({}), &access)
        .expect("Engine values must inject");
    let observer = null_emitter();
    vm.install_engine_globals(&observer, "Test")
        .expect("Engine globals must install");

    let numbered = run_scalar(
        &vm,
        &program("return store.read_numbered('a.txt', 2, 3)"),
        &null_emitter(),
        "Test",
    )
    .expect("a bounded numbered read must run");
    assert_eq!(numbered.as_deref(), Some("2| two\n3| three"));

    let whole = run_scalar(
        &vm,
        &program("return store.read_numbered('a.txt')"),
        &null_emitter(),
        "Test",
    )
    .expect("an unbounded numbered read must run");
    assert_eq!(whole.as_deref(), Some("1| one\n2| two\n3| three"));

    let err = run_scalar(
        &vm,
        &program("return store.read_numbered('a.txt', 0)"),
        &null_emitter(),
        "Test",
    )
    .expect_err("a start below 1 must raise");
    assert!(
        err.to_string().contains("invalid line range"),
        "the error must include the range message, got: {err}"
    );
}

#[test]
fn store_glob_returns_a_sorted_array() {
    let out = run(
            "store.write('src/b.rs', '')\nstore.write('src/a.rs', '')\nlocal g = store.glob('src/*.rs')\nreturn g[1] .. ',' .. g[2]",
            "",
        )
        .unwrap();
    assert_eq!(out.returned.as_deref(), Some("src/a.rs,src/b.rs"));
}

#[test]
fn store_error_surfaces_as_lua_error() {
    // An ambiguous `str_replace` anchor is a `VfsError::Anchor`, wrapped as
    // `Error::Store` (mapped through `mlua::Error::external`).
    let err = run(
        "store.write('a.txt', 'na na na')\nstore.str_replace('a.txt', 'na', 'la')",
        "",
    )
    .expect_err("an ambiguous anchor must raise");
    let msg = lua_error_message(&err);
    assert!(
        msg.contains("expected exactly one"),
        "the Lua error must include the ambiguity message, got: {msg}"
    );
}

#[test]
fn store_writes_are_visible_on_the_shared_handle() {
    // The table is backed by the caller's handle, so a write from Lua is
    // observable through a clone of that same handle after the chunk ends.
    let access = fresh_access();
    let store = store_view(&access);
    run_with("store.write('shared.txt', 'from lua')", &access).unwrap();
    assert_eq!(
        store.read_string("shared.txt").expect("read"),
        "from lua",
        "a Lua write must land in the shared store"
    );
}
