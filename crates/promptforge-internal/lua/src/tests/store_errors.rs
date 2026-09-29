//! The store error value: each failure's reason tag, fields, and message,
//! as rendered through the Lua boundary and as caught during the
//! shared-library load.

use super::*;

#[test]
fn shared_load_store_failure_caught_by_pcall_carries_the_reason_tag() {
    // The shared library loads through the direct store closures, before
    // the yield shims install. A non-conflict failure caught by the shim's
    // `pcall` must render the same shape the effect path renders: kind
    // `store`, the `reason` tag, and the variant's fields.
    let shared = program(
        "local ok, err = pcall(function() return store.read('missing.txt') end)\n\
             shared_store_error = err",
    );
    let access = fresh_access();
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_host("", &json!({}), &access)
        .expect("host values must inject");
    vm.install_host_apis(&null_emitter(), "Test")
        .expect("host APIs must install");
    vm.install_coro_shims(1).expect("coro shims must install");
    vm.replay_shared(&shared, &null_emitter(), "Test")
        .expect("the shared library must load");
    let out = run_scalar(
        &vm,
        &program(
            "return shared_store_error.kind .. '|' .. shared_store_error.reason .. '|' \
                 .. shared_store_error.path .. '|' .. tostring(shared_store_error)",
        ),
        &null_emitter(),
        "Test",
    )
    .expect("the section chunk must run");
    assert_eq!(
        out.as_deref(),
        Some("store|not_found|missing.txt|file not found in store: missing.txt"),
        "the shared-load store failure must carry the documented shape"
    );
}

/// One store failure's rendering: the reason tag, the fields (with the
/// anchor's `count` checked as an integer), and the model-facing message.
#[expect(
    clippy::too_many_lines,
    reason = "one table holds every reason's rendering beside its fields and message"
)]
#[test]
fn store_error_values_carry_reason_fields_and_message_for_each_reason() {
    use promptforge_vfs::{PathReason, VfsError};

    struct Case {
        error: VfsError,
        reason: &'static str,
        fields: Vec<(String, crate::ErrorField)>,
        message: &'static str,
    }

    fn path_field(path: &str) -> Vec<(String, crate::ErrorField)> {
        vec![(
            "path".to_owned(),
            crate::ErrorField::String(path.to_owned()),
        )]
    }

    let cases = [
        Case {
            error: VfsError::NotFound {
                path: "notes.md".to_owned(),
            },
            reason: "not_found",
            fields: path_field("notes.md"),
            message: "file not found in store: notes.md",
        },
        Case {
            error: VfsError::Anchor {
                path: "notes.md".to_owned(),
                anchor: "TODO".to_owned(),
                count: 3,
            },
            reason: "anchor",
            fields: vec![
                (
                    "path".to_owned(),
                    crate::ErrorField::String("notes.md".to_owned()),
                ),
                (
                    "anchor".to_owned(),
                    crate::ErrorField::String("TODO".to_owned()),
                ),
                ("count".to_owned(), crate::ErrorField::Integer(3)),
            ],
            message: "anchor \"TODO\" occurs 3 times in notes.md, expected exactly one; \
                      include more surrounding text so it matches once",
        },
        Case {
            error: VfsError::InvalidPath {
                path: "../x".to_owned(),
                reason: PathReason::Traversal,
            },
            reason: "invalid_path",
            fields: vec![
                (
                    "path".to_owned(),
                    crate::ErrorField::String("../x".to_owned()),
                ),
                (
                    "rule".to_owned(),
                    crate::ErrorField::String("traversal".to_owned()),
                ),
            ],
            message: "invalid path \"../x\": path contains a traversal segment",
        },
        Case {
            error: VfsError::InvalidRange {
                path: "notes.md".to_owned(),
                reason: "start must be at least 1",
            },
            reason: "invalid_range",
            fields: path_field("notes.md"),
            message: "invalid line range for notes.md: start must be at least 1",
        },
        Case {
            error: VfsError::NotUtf8 {
                path: "a.bin".to_owned(),
            },
            reason: "not_utf8",
            fields: path_field("a.bin"),
            message: "file in store is not UTF-8: a.bin",
        },
        Case {
            error: VfsError::IsADirectory {
                path: "d".to_owned(),
            },
            reason: "is_a_directory",
            fields: path_field("d"),
            message: "is a directory in store: d",
        },
        Case {
            error: VfsError::NotADirectory {
                path: "d".to_owned(),
            },
            reason: "not_a_directory",
            fields: path_field("d"),
            message: "not a directory in store: d",
        },
        Case {
            error: VfsError::DirectoryNotEmpty {
                path: "d".to_owned(),
            },
            reason: "directory_not_empty",
            fields: path_field("d"),
            message: "directory not empty in store: d",
        },
        Case {
            error: VfsError::AlreadyExists {
                path: "d".to_owned(),
            },
            reason: "already_exists",
            fields: path_field("d"),
            message: "file already exists in store: d",
        },
        Case {
            error: VfsError::PermissionDenied {
                path: "d".to_owned(),
                reason: "read-only mount".to_owned(),
            },
            reason: "permission_denied",
            fields: path_field("d"),
            message: "permission denied for store path d: read-only mount",
        },
        Case {
            error: VfsError::Unsupported {
                path: "d".to_owned(),
                detail: "no rename".to_owned(),
            },
            reason: "unsupported",
            fields: path_field("d"),
            message: "unsupported store operation on d: no rename",
        },
        Case {
            error: VfsError::Backend {
                message: "disk gone".to_owned(),
            },
            reason: "backend",
            fields: Vec::new(),
            message: "store backend failure: disk gone",
        },
    ];

    for case in cases {
        let reason = crate::store_error_reason(&case.error);
        let fields = crate::store_error_fields(&case.error);
        let op = crate::StoreOp::Read {
            path: "x".to_owned(),
            start: None,
            end: None,
        };
        let message = crate::store_error_message(&op, &case.error);
        assert_eq!(reason, case.reason, "wrong reason for {:?}", case.error);
        assert_eq!(fields, case.fields, "wrong fields for {:?}", case.error);
        assert_eq!(message, case.message, "wrong message for {:?}", case.error);
    }
}

/// A glob failure renders as an invalid glob pattern - the call surface's
/// promised wording - while a path failure renders as an invalid path.
#[test]
fn store_error_messages_use_the_operations_wording() {
    use promptforge_vfs::{PathReason, VfsError};
    let error = VfsError::InvalidPath {
        path: "a**b".to_owned(),
        reason: PathReason::Wildcard,
    };
    let glob = crate::StoreOp::Glob {
        pattern: "a**b".to_owned(),
    };
    let read = crate::StoreOp::Read {
        path: "a**b".to_owned(),
        start: None,
        end: None,
    };
    assert_eq!(
        crate::store_error_message(&glob, &error),
        "invalid glob pattern \"a**b\": pattern contains invalid wildcard grammar"
    );
    assert_eq!(
        crate::store_error_message(&read, &error),
        "invalid path \"a**b\": pattern contains invalid wildcard grammar"
    );
}

/// A store failure raised through the Lua boundary renders as an error
/// table of kind `store` whose `count` field is a Lua integer, and
/// `tostring` returns the model-facing message.
#[test]
fn a_raised_store_error_renders_an_integer_count_and_the_message() {
    let anchor = promptforge_vfs::VfsError::Anchor {
        path: "notes.md".to_owned(),
        anchor: "TODO".to_owned(),
        count: 3,
    };
    let error = Error::store(
        &crate::StoreOp::StrReplace {
            path: "notes.md".to_owned(),
            old: "TODO".to_owned(),
            new: "DONE".to_owned(),
        },
        anchor,
    );
    assert_eq!(error.kind(), crate::ErrorKind::Store);
    let fields = error.fields();
    assert!(fields.contains(&("count".to_owned(), crate::ErrorField::Integer(3),)));
    let lua = mlua::Lua::new();
    let table = crate::error_table(&lua, &error).expect("the table builds");
    let raised = crate::error_value::raised_from(&lua, &Value::Table(table))
        .expect("the table reads back")
        .expect("the table is a structured error table");
    assert_eq!(raised.kind, crate::ErrorKind::Store);
    assert_eq!(
        raised.fields.get("count"),
        Some(&crate::ErrorField::Integer(3)),
        "the count field stays an integer through the boundary"
    );
    assert!(
        error.to_string().contains("include more surrounding text"),
        "the message carries the recovery hint: {error}"
    );
}
