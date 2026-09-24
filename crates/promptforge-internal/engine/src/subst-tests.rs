//! Regression coverage for `{{ }}` prose substitution: the resolution
//! of the `args`, `argv`, `item`, `var`, `sys`, and bare-global sources, the
//! escape grammar, path segment validation, and the typed error kinds.

use super::*;
use serde_json::json;

/// The lookup for prose with no section VM behind it: every bare-global
/// name comes back unset.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature must match the globals-lookup callback type"
)]
fn no_globals(_: &str) -> Result<Option<Value>> {
    Ok(None)
}

/// Sources with the stock args string and no argv/item, over the given
/// `var`/`sys`; tests override fields with struct-update syntax.
fn test_sources<'a>(var: &'a Value, sys: &'a Value) -> Sources<'a> {
    Sources {
        args: "Acme Corp",
        argv: None,
        item: None,
        var,
        sys,
        globals: &no_globals,
    }
}

fn run(prose: &str) -> Result<String> {
    let var = json!({ "kind": "library", "count": 3, "row": { "a": 1 } });
    let sys = json!({ "when": "2026-07-29T00:00:00Z", "id": 1 });
    substitute(prose, &test_sources(&var, &sys))
}

fn err_of(prose: &str) -> SubstitutionError {
    let var = json!({ "kind": "library", "row": { "a": 1 }, "arr": [1, 2] });
    let sys = json!({ "id": 1 });
    let sources = Sources {
        item: Some(&json!("i")),
        ..test_sources(&var, &sys)
    };
    substitute_inner(prose, &sources).expect_err("expected substitution failure")
}

#[test]
fn substitution_diagnostics_escape_and_bound_the_placeholder() {
    let hostile = format!("var.{}", "x".repeat(500));
    let preview = path_preview(&hostile);
    assert!(
        preview.chars().count() <= 83,
        "preview must be bounded, got {} chars",
        preview.chars().count()
    );
    assert!(preview.ends_with("..."), "over-long preview must be elided");

    let with_controls = path_preview("var.a\nb\tc");
    assert!(
        !with_controls.contains('\n') && !with_controls.contains('\t'),
        "control characters must be escaped, got: {with_controls}"
    );
    assert!(with_controls.contains("\\n") && with_controls.contains("\\t"));
}

#[test]
fn resolves_args() {
    assert_eq!(run("hi {{ args }}!").unwrap(), "hi Acme Corp!");
}

// --- argv: the parsed-args namespace ------------------------------------

#[test]
fn resolves_argv_whole_value() {
    let var = json!({});
    let sys = json!({});
    let argv = json!({ "query": "papers", "n": 2 });
    let sources = Sources {
        argv: Some(&argv),
        ..test_sources(&var, &sys)
    };
    let out = substitute("got {{ argv }}", &sources).unwrap();
    assert_eq!(out, "got {\"n\":2,\"query\":\"papers\"}");
}

#[test]
fn resolves_argv_dotted_path() {
    let var = json!({});
    let sys = json!({});
    let argv = json!({ "query": "papers", "row": { "a": 1 } });
    let sources = Sources {
        argv: Some(&argv),
        ..test_sources(&var, &sys)
    };
    let out = substitute("q={{ argv.query }} cell={{ argv.row.a }}", &sources).unwrap();
    assert_eq!(out, "q=papers cell=1");
}

#[test]
fn scalar_argv_renders_whole() {
    let var = json!({});
    let sys = json!({});
    let argv = json!(42);
    let sources = Sources {
        argv: Some(&argv),
        ..test_sources(&var, &sys)
    };
    let out = substitute("{{ argv }}", &sources).unwrap();
    assert_eq!(out, "42");
}

#[test]
fn nil_argv_is_an_error() {
    let var = json!({});
    let sys = json!({});
    for prose in ["{{ argv }}", "{{ argv.x }}"] {
        let e = substitute_inner(prose, &test_sources(&var, &sys)).unwrap_err();
        assert_eq!(e.kind, SubstErrorKind::NilArgv, "path {prose:?}");
        assert!(e.to_string().contains("argv"), "names argv: {e}");
    }
}

#[test]
fn dotted_index_into_a_scalar_argv_is_an_error() {
    // Never a silent empty string: the existing missing-key failure.
    let var = json!({});
    let sys = json!({});
    let argv = json!("scalar");
    let sources = Sources {
        argv: Some(&argv),
        ..test_sources(&var, &sys)
    };
    let e = substitute_inner("{{ argv.x }}", &sources).unwrap_err();
    assert_eq!(e.kind, SubstErrorKind::MissingKey);
}

#[test]
fn resolves_var_scalar() {
    assert_eq!(run("a {{ var.kind }} paper").unwrap(), "a library paper");
    assert_eq!(run("{{ var.count }}").unwrap(), "3");
}

#[test]
fn resolves_sys() {
    assert_eq!(run("id {{ sys.id }}").unwrap(), "id 1");
    assert_eq!(run("at {{ sys.when }}").unwrap(), "at 2026-07-29T00:00:00Z");
}

#[test]
fn table_renders_as_json() {
    assert_eq!(run("{{ var.row }}").unwrap(), "{\"a\":1}");
}

#[test]
fn missing_key_is_error() {
    assert!(run("{{ var.nope }}").is_err());
    assert!(run("{{ ghost.x }}").is_err());
    let sys_error = run("{{ sys.bogus }}").expect_err("unknown sys field must fail");
    assert!(
        sys_error.to_string().contains("missing {{ sys.bogus }}"),
        "error was {sys_error}"
    );
}

#[test]
fn no_placeholders_passthrough() {
    assert_eq!(run("plain text").unwrap(), "plain text");
}

#[test]
fn unclosed_is_error() {
    assert_eq!(err_of("open {{ args").kind, SubstErrorKind::Unclosed);
}

// --- SUBST-003: escape grammar -------------------------------------------

#[test]
fn escaped_delimiters_are_literal() {
    assert_eq!(
        run(r"literal \{{ args }} here").unwrap(),
        "literal {{ args }} here"
    );
    assert_eq!(run(r"close \}} brace").unwrap(), "close }} brace");
    assert_eq!(run(r"back \\ slash").unwrap(), r"back \ slash");
}

#[test]
fn escape_then_real_placeholder_adjacent() {
    // First delimiter escaped, second one live and resolved.
    assert_eq!(run(r"\{{x}}{{ args }}").unwrap(), "{{x}}Acme Corp");
}

#[test]
fn lone_backslash_is_literal() {
    assert_eq!(run(r"a\b").unwrap(), r"a\b");
    assert_eq!(run("trailing\\").unwrap(), "trailing\\");
}

#[test]
fn replacement_produced_delimiters_are_not_resubstituted() {
    let var = json!({ "payload": "{{ args }}" });
    let sys = json!({});
    // `var.payload` renders text that looks like a placeholder; it must be
    // emitted verbatim, never resolved against args.
    let sources = Sources {
        args: "SECRET",
        ..test_sources(&var, &sys)
    };
    let out = substitute("value: {{ var.payload }}", &sources).unwrap();
    assert_eq!(out, "value: {{ args }}");
}

// --- SUBST-004: path segment grammar -------------------------------------

#[test]
fn empty_or_padded_segments_are_rejected() {
    for bad in ["var.", "var..x", "var. .x", "var.x.", "var. .x .y"] {
        let prose = format!("{{{{ {bad} }}}}");
        let e = err_of(&prose);
        assert_eq!(
            e.kind,
            SubstErrorKind::EmptySegment,
            "path {bad:?} must be an empty-segment error, got {:?}",
            e.kind
        );
    }
}

#[test]
fn valid_nested_segment_still_resolves() {
    assert_eq!(run("{{ var.row.a }}").unwrap(), "1");
}

// --- SUBST-005: typed error kind/offset/source ---------------------------

#[test]
fn error_has_kind_and_offset() {
    let e = err_of("prefix {{ ghost.x }}");
    assert_eq!(e.kind, SubstErrorKind::UnknownNamespace);
    assert_eq!(e.offset, 7, "offset must point at the '{{{{'");
    assert!(e.to_string().contains("ghost.x"));
}

// --- bare globals (section-local Lua globals in prose) -------------------

#[test]
fn bare_global_resolves_a_scalar() {
    let var = json!({});
    let sys = json!({});
    let globals = |name: &str| Ok((name == "answer").then(|| json!(42)));
    let sources = Sources {
        globals: &globals,
        ..test_sources(&var, &sys)
    };
    let out = substitute("the answer is {{ answer }}", &sources).unwrap();
    assert_eq!(out, "the answer is 42");
}

#[test]
fn bare_global_dotted_path_indexes_the_json() {
    let var = json!({});
    let sys = json!({});
    let globals = |name: &str| Ok((name == "row").then(|| json!({ "a": { "b": 2 } })));
    let sources = Sources {
        globals: &globals,
        ..test_sources(&var, &sys)
    };
    let out = substitute("cell {{ row.a.b }}", &sources).unwrap();
    assert_eq!(out, "cell 2");
}

#[test]
fn bare_global_table_renders_as_json() {
    let var = json!({});
    let sys = json!({});
    let globals = |name: &str| Ok((name == "row").then(|| json!({ "a": 1 })));
    let sources = Sources {
        globals: &globals,
        ..test_sources(&var, &sys)
    };
    let out = substitute("{{ row }}", &sources).unwrap();
    assert_eq!(out, "{\"a\":1}");
}

#[test]
fn missing_bare_global_is_unknown_namespace() {
    let e = err_of("{{ ghost }}");
    assert_eq!(e.kind, SubstErrorKind::UnknownNamespace);
    assert!(e.to_string().contains("global 'ghost'"));
}

#[test]
fn non_json_bare_global_is_an_error() {
    let var = json!({});
    let sys = json!({});
    let globals = |name: &str| {
        assert_eq!(name, "f");
        Err(crate::Error::Lua("global `f` is a function".to_owned()))
    };
    let sources = Sources {
        globals: &globals,
        ..test_sources(&var, &sys)
    };
    let e = substitute_inner("{{ f }}", &sources).unwrap_err();
    assert_eq!(e.kind, SubstErrorKind::Serialize);
    assert!(e.to_string().contains("not JSON data"));
    assert!(
        std::error::Error::source(&e).is_some(),
        "the lookup failure must be preserved as the source"
    );
}

#[test]
fn bare_namespaces_still_require_a_key() {
    // `{{ var }}` and `{{ sys }}` stay BadPath; only bare globals render
    // whole values.
    for prose in ["{{ var }}", "{{ sys }}"] {
        let e = err_of(prose);
        assert_eq!(e.kind, SubstErrorKind::BadPath, "path {prose:?}");
    }
}

#[test]
fn reply_is_no_longer_a_namespace() {
    // The reply register is removed: `{{ reply }}` resolves as a bare
    // global like any other name, and is unset here.
    let e = err_of("{{ reply }}");
    assert_eq!(e.kind, SubstErrorKind::UnknownNamespace);
}

#[test]
fn null_value_and_item_kinds() {
    let var = json!({ "n": Value::Null });
    let sys = json!({});
    let e = substitute_inner("{{ var.n }}", &test_sources(&var, &sys)).unwrap_err();
    assert_eq!(e.kind, SubstErrorKind::NullValue);

    let e = substitute_inner("{{ item }}", &test_sources(&var, &sys)).unwrap_err();
    assert_eq!(e.kind, SubstErrorKind::NilItem);
}

#[test]
fn not_a_table_kind() {
    let e = err_of("{{ args.x }}");
    assert_eq!(e.kind, SubstErrorKind::NotATable);
    assert!(e.to_string().contains("not a table"));
}

// --- SUBST-006: null, arrays, trust-neutral passthrough ------------------

#[test]
fn array_renders_as_json() {
    let var = json!({ "arr": [1, 2, 3] });
    let sys = json!({});
    let out = substitute("{{ var.arr }}", &test_sources(&var, &sys)).unwrap();
    assert_eq!(out, "[1,2,3]");
}

#[test]
fn resolves_item_when_present() {
    let var = json!({});
    let sys = json!({});
    let sources = Sources {
        item: Some(&json!("the angle")),
        ..test_sources(&var, &sys)
    };
    let out = substitute("topic: {{ item }}", &sources).unwrap();
    assert_eq!(out, "topic: the angle");
}

#[test]
fn item_nil_is_error() {
    let var = json!({});
    let sys = json!({});
    let err = substitute("{{ item }}", &test_sources(&var, &sys)).expect_err("nil item must fail");
    assert!(
        err.to_string().contains("nil"),
        "error must mention nil: {err}"
    );
}

#[test]
fn item_dot_path_is_error() {
    let var = json!({});
    let sys = json!({});
    let sources = Sources {
        item: Some(&json!("text")),
        ..test_sources(&var, &sys)
    };
    let err = substitute("{{ item.x }}", &sources).expect_err("item is a string, not a table");
    assert!(
        err.to_string().contains("not a table"),
        "error must say not a table: {err}"
    );
}

#[test]
fn render_item_renders_by_type() {
    // The item rendering rule: strings verbatim, numbers and booleans in
    // their natural string form, arrays and objects as compact JSON.
    assert_eq!(render_item(&json!("abc")), "abc");
    assert_eq!(render_item(&json!(3)), "3");
    assert_eq!(render_item(&json!(1.5)), "1.5");
    assert_eq!(render_item(&json!(true)), "true");
    assert_eq!(render_item(&json!(["a", 1])), "[\"a\",1]");
    assert_eq!(render_item(&json!({ "k": 1 })), "{\"k\":1}");
    assert_eq!(render_item(&Value::Null), "null");
}
