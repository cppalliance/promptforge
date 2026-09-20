//! `{{ }}` prose substitution.
//!
//! When a section's Lua first reads the lazy `prose` value, the harness
//! resolves `{{ path }}` placeholders in the pending Markdown template. Lua
//! source is never substituted. Five sources are available:
//! `args` (the single raw input string), `argv` (its parsed JSON form, nil
//! when the args did not parse), `item` (the current fanout arm's
//! item value, nil outside arms), `var` (values the section's Lua wrote),
//! and `sys`
//! (runtime-provided metadata). An unknown first segment resolves as a bare
//! global: a section-local Lua global (`x = 42` without `local`) read through
//! a host-supplied lookup, with dotted paths indexing into its JSON form.
//! Resolution is a single pass with no recursion:
//! scalars render as strings, tables/arrays as JSON, and a missing path is a
//! hard error. `{{ item }}` outside a fanout arm is a hard error. A missing
//! bare global, or one holding
//! a function or userdata, is a hard error. Substitution does no arithmetic -
//! compute in Lua and reference the result.
//!
//! # Escape grammar
//!
//! A backslash escapes the following character when it is `{`, `}`, or `\\`:
//! the backslash is consumed and the next character is emitted literally.
//! Everywhere else a backslash is an ordinary literal. This lets prose carry a
//! literal opening delimiter (`\{{` emits `{{`), a literal closing delimiter
//! (`\}}` emits `}}`), and a literal backslash (`\\` emits `\`). Escapes
//! compose, so adjacent escaped delimiters resolve independently.
//!
//! Substitution is a single left-to-right pass over the *input* prose only:
//! resolved output is appended to a separate buffer and never rescanned, so a
//! replacement that itself contains `{{ ... }}` is emitted verbatim and never
//! triggers a second round of substitution.

use serde_json::Value;

use crate::Result;

/// A stable classification of a [`SubstitutionError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubstErrorKind {
    /// A `{{` was opened but never closed with `}}`.
    Unclosed,
    /// A bare namespace with no `.key` suffix where one is required.
    BadPath,
    /// A path segment was empty or whitespace-padded (`var.`, `var..x`).
    EmptySegment,
    /// The leading namespace is not one of the known roots and names no bare
    /// global.
    UnknownNamespace,
    /// A scalar namespace (`args`/`item`) was indexed like a table.
    NotATable,
    /// A `var`/`sys` lookup found no value at the requested key.
    MissingKey,
    /// The resolved value was JSON null.
    NullValue,
    /// `{{ item }}` was used outside a fanout arm.
    NilItem,
    /// `{{ argv }}` was used when the args string did not parse (or H1 left
    /// `argv` nil).
    NilArgv,
    /// A table/array value failed to serialize to JSON, or a bare global was
    /// not JSON data.
    Serialize,
}

/// A typed substitution failure.
///
/// Carries a stable [`kind`](SubstitutionError::kind), the byte
/// [`offset`](SubstitutionError::offset) of the offending placeholder within
/// the prose, a `message` that embeds a bounded, control-escaped preview of the
/// placeholder path, and - for the serialization case - the preserved
/// underlying error as its [`source`](std::error::Error::source).
#[derive(Debug)]
pub(crate) struct SubstitutionError {
    kind: SubstErrorKind,
    offset: usize,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl SubstitutionError {
    fn new(kind: SubstErrorKind, offset: usize, message: String) -> Self {
        SubstitutionError {
            kind,
            offset,
            message,
            source: None,
        }
    }

    fn with_source(
        kind: SubstErrorKind,
        offset: usize,
        message: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        SubstitutionError {
            kind,
            offset,
            message,
            source: Some(source),
        }
    }
}

impl std::fmt::Display for SubstitutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} [{:?} at byte {}]",
            self.message, self.kind, self.offset
        )
    }
}

impl std::error::Error for SubstitutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|s| s as &(dyn std::error::Error + 'static))
    }
}

type SubstResult<T> = std::result::Result<T, SubstitutionError>;

/// Renders the JSON scalar forms shared by item and placeholder rendering.
fn render_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// Renders a spawned chain's `item` for prose substitution: strings
/// verbatim, numbers and booleans in their natural string form, arrays and
/// objects as compact JSON. The one rule is the Lua crate's, shared with
/// the `fanout` shim's exhausted-arm stub so `{{ item }}` and the stub's
/// heading render a member identically.
#[must_use]
pub(crate) fn render_item(item: &Value) -> String {
    crate::lua::render_item(item)
}

/// The value sources `{{ }}` placeholders resolve against.
///
/// `var` and `sys` are JSON objects (`var` read back from the section's
/// Lua, `sys` built by the runtime). `argv` is the parsed form of the args
/// string - the run's frozen value on the walk - or `None` when the args
/// did not parse; `{{ argv }}` renders the whole value and `{{ argv.key }}`
/// indexes it, with a nil `argv` a hard error. `item` is the current fanout
/// arm's item value, or `None` outside arms; it renders per
/// [`render_item`]. `globals` resolves a bare global by name: `Ok(None)`
/// when unset, `Ok(Some(_))` with its JSON form when set.
pub(crate) struct Sources<'a> {
    /// The raw args string: `{{ args }}`.
    pub(crate) args: &'a str,
    /// The parsed form of the args string: `{{ argv }}` and dotted paths.
    pub(crate) argv: Option<&'a Value>,
    /// The current fanout arm's item value: `{{ item }}`.
    pub(crate) item: Option<&'a Value>,
    /// The section's `var` clipboard, read back from its Lua.
    pub(crate) var: &'a Value,
    /// The runtime-provided metadata: `{{ sys.key }}`.
    pub(crate) sys: &'a Value,
    /// The bare-global lookup for `{{ name }}` resolution.
    pub(crate) globals: &'a dyn Fn(&str) -> Result<Option<Value>>,
}

/// Resolves every `{{ path }}` in `prose` against the [`Sources`].
///
/// This function receives prose only and does not transform either compiled
/// Lua phase.
///
/// # Errors
/// Returns [`Error::Substitution`](crate::Error::Substitution) for an unclosed
/// `{{`, an unknown namespace or missing bare global, an empty or whitespace
/// path segment, a missing key, a null value, a non-JSON bare global,
/// `{{ item }}` when `item` is `None`, or `{{ argv }}` when `argv` is `None`.
pub(crate) fn substitute(prose: &str, sources: &Sources<'_>) -> Result<String> {
    Ok(substitute_inner(prose, sources)?)
}

fn substitute_inner(prose: &str, sources: &Sources<'_>) -> SubstResult<String> {
    let mut out = String::with_capacity(prose.len());
    let bytes = prose.as_bytes();
    let mut i = 0;
    while i < prose.len() {
        // Escape grammar: a backslash consumes itself and emits a literal `{`,
        // `}`, or `\` when one immediately follows.
        if bytes[i] == b'\\' && i + 1 < prose.len() {
            let next = bytes[i + 1];
            if matches!(next, b'{' | b'}' | b'\\') {
                out.push(next as char);
                i += 2;
                continue;
            }
        }
        if bytes[i] == b'{' && i + 1 < prose.len() && bytes[i + 1] == b'{' {
            let start = i;
            let after = &prose[i + 2..];
            let end = after.find("}}").ok_or_else(|| {
                SubstitutionError::new(
                    SubstErrorKind::Unclosed,
                    start,
                    "unclosed '{{' in prose".to_string(),
                )
            })?;
            let path = after[..end].trim();
            out.push_str(&resolve(path, start, sources)?);
            i += 2 + end + 2;
            continue;
        }
        let Some(ch) = prose[i..].chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    Ok(out)
}

/// The nil-`argv` failure shared by the bare and dotted `{{ argv }}` paths.
fn nil_argv<'a>(argv: Option<&'a Value>, path: &str, offset: usize) -> SubstResult<&'a Value> {
    argv.ok_or_else(|| {
        SubstitutionError::new(
            SubstErrorKind::NilArgv,
            offset,
            format!(
                "{{{{ {} }}}} is nil (the args string is not JSON)",
                path_preview(path)
            ),
        )
    })
}

/// Resolves an unknown first segment as a bare global: the host lookup
/// reads the section-local Lua global and converts it to JSON.
fn bare_global_root(
    name: &str,
    path: &str,
    offset: usize,
    globals: &dyn Fn(&str) -> Result<Option<Value>>,
) -> SubstResult<Value> {
    globals(name)
        .map_err(|error| {
            SubstitutionError::with_source(
                SubstErrorKind::Serialize,
                offset,
                format!(
                    "global '{}' in {{{{ {} }}}} is not JSON data",
                    path_preview(name),
                    path_preview(path)
                ),
                Box::new(error),
            )
        })?
        .ok_or_else(|| {
            SubstitutionError::new(
                SubstErrorKind::UnknownNamespace,
                offset,
                format!(
                    "unknown namespace or global '{}' in {{{{ {} }}}}",
                    path_preview(name),
                    path_preview(path)
                ),
            )
        })
}

/// Resolves a single `{{ }}` path to its rendered string.
fn resolve(path: &str, offset: usize, sources: &Sources<'_>) -> SubstResult<String> {
    if path == "args" {
        return Ok(sources.args.to_string());
    }
    if path == "item" {
        return sources.item.map(render_item).ok_or_else(|| {
            SubstitutionError::new(
                SubstErrorKind::NilItem,
                offset,
                "{{ item }} is nil (not inside a fanout arm)".to_string(),
            )
        });
    }
    if path == "argv" {
        return render(nil_argv(sources.argv, path, offset)?, path, offset);
    }

    // Validate the complete segment grammar before any lookup: every segment
    // (namespace included) must be nonempty and free of leading or trailing
    // whitespace, so `var.`, `var..x`, and `var. .x` are rejected up front even
    // when a matching JSON key happens to exist.
    for segment in path.split('.') {
        if segment.is_empty() || segment.trim() != segment {
            return Err(SubstitutionError::new(
                SubstErrorKind::EmptySegment,
                offset,
                format!(
                    "empty or padded path segment in {{{{ {} }}}}",
                    path_preview(path)
                ),
            ));
        }
    }

    let (namespace, keys) = match path.split_once('.') {
        Some((namespace, keys)) => (namespace, Some(keys)),
        None => (path, None),
    };

    // An unknown first segment resolves as a bare global: the host lookup
    // reads the section-local Lua global and converts it to JSON.
    let global_value;
    let root = match namespace {
        "var" => sources.var,
        "sys" => sources.sys,
        "argv" => nil_argv(sources.argv, path, offset)?,
        "args" | "item" => {
            return Err(SubstitutionError::new(
                SubstErrorKind::NotATable,
                offset,
                format!("{namespace} is a string, not a table"),
            ));
        }
        other => {
            global_value = bare_global_root(other, path, offset, sources.globals)?;
            &global_value
        }
    };

    let mut current = root;
    match keys {
        Some(keys) => {
            for key in keys.split('.') {
                current = current.get(key).ok_or_else(|| {
                    SubstitutionError::new(
                        SubstErrorKind::MissingKey,
                        offset,
                        format!("missing {{{{ {} }}}}", path_preview(path)),
                    )
                })?;
            }
        }
        // A bare `var`/`sys` with no key stays an error; a bare global with
        // no keys renders its whole JSON value.
        None if matches!(namespace, "var" | "sys") => {
            return Err(SubstitutionError::new(
                SubstErrorKind::BadPath,
                offset,
                format!("bad path: {{{{ {} }}}}", path_preview(path)),
            ));
        }
        None => {}
    }
    render(current, path, offset)
}

/// Renders a prompt-controlled placeholder path for a diagnostic.
///
/// Control characters are escaped and the text is truncated to a bounded length
/// so a hostile or malformed placeholder cannot forge multiline log records,
/// leak an oversized span, or smuggle control characters through `Display`.
fn path_preview(path: &str) -> String {
    use std::fmt::Write as _;
    const MAX_PREVIEW_CHARS: usize = 80;
    let mut out = String::with_capacity(path.len().min(MAX_PREVIEW_CHARS));
    for ch in path.chars().take(MAX_PREVIEW_CHARS) {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:04x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    if path.chars().count() > MAX_PREVIEW_CHARS {
        out.push_str("...");
    }
    out
}

/// Renders a resolved JSON value as its substituted string.
fn render(value: &Value, path: &str, offset: usize) -> SubstResult<String> {
    if let Some(rendered) = render_scalar(value) {
        return Ok(rendered);
    }
    if value.is_null() {
        Err(SubstitutionError::new(
            SubstErrorKind::NullValue,
            offset,
            format!("missing {{{{ {} }}}}", path_preview(path)),
        ))
    } else {
        serde_json::to_string(value).map_err(|error| {
            SubstitutionError::with_source(
                SubstErrorKind::Serialize,
                offset,
                format!("could not serialize {{{{ {} }}}}", path_preview(path)),
                Box::new(error),
            )
        })
    }
}

#[cfg(test)]
mod tests {
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
    fn error_carries_kind_and_offset() {
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
        let err =
            substitute("{{ item }}", &test_sources(&var, &sys)).expect_err("nil item must fail");
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
}
