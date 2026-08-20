//! `{{ }}` prose substitution.
//!
//! After a section's Lua prologue runs, the harness resolves `{{ path }}`
//! placeholders in the prose before the model sees it. Lua source in the
//! prologue and epilog is never substituted. Five sources are available:
//! `args` (the single raw input string), `reply` (the previous section's model
//! reply, nil in section 1), `item` (the current fanout arm's item value, nil
//! outside arms), `var` (values the prologue wrote), and `sys`
//! (runtime-provided metadata). An unknown first segment resolves as a bare
//! global: a section-local Lua global (`x = 42` without `local`) read through
//! a host-supplied lookup, with dotted paths indexing into its JSON form.
//! Resolution is a single pass with no recursion:
//! scalars render as strings, tables/arrays as JSON, and a missing path is a
//! hard error. `{{ reply }}` when nil is a hard error. `{{ item }}` outside a
//! fanout arm is a hard error. A missing bare global, or one holding a
//! function or userdata, is a hard error. Substitution does no arithmetic -
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
    /// A scalar namespace (`args`/`reply`/`item`) was indexed like a table.
    NotATable,
    /// A `var`/`sys` lookup found no value at the requested key.
    MissingKey,
    /// The resolved value was JSON null.
    NullValue,
    /// `{{ reply }}` was used before any prior section reply existed.
    NilReply,
    /// `{{ item }}` was used outside a fanout arm.
    NilItem,
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

/// Renders a fanout arm's item for prose substitution and stub text:
/// strings verbatim, numbers and booleans in their natural string form,
/// arrays and objects as compact JSON.
#[must_use]
pub(crate) fn render_item(item: &Value) -> String {
    if let Some(rendered) = render_scalar(item) {
        return rendered;
    }
    // Serializing a `Value` cannot fail; the default is unreachable.
    serde_json::to_string(item).unwrap_or_default()
}

/// Resolve every `{{ path }}` in `prose` against `args`, `reply`, `item`,
/// `var`, `sys`, and the section's bare globals.
///
/// `var` and `sys` are JSON objects (`var` read back from the Lua prologue,
/// `sys` built by the runtime). `reply` is the previous section's model
/// reply text, or `None` in the first section. `item` is the current fanout
/// arm's item value, or `None` outside arms; it renders per
/// [`render_item`]. `globals` resolves a bare global by name: `Ok(None)`
/// when unset, `Ok(Some(_))` with its JSON form when set. This function
/// receives prose only and does not transform either compiled Lua phase.
///
/// # Errors
/// Returns [`Error::Substitution`](crate::Error::Substitution) for an unclosed
/// `{{`, an unknown namespace or missing bare global, an empty or whitespace
/// path segment, a missing key, a null value, a non-JSON bare global,
/// `{{ reply }}` when `reply` is `None`, or `{{ item }}` when `item` is
/// `None`.
pub(crate) fn substitute(
    prose: &str,
    args: &str,
    reply: Option<&str>,
    item: Option<&Value>,
    var: &Value,
    sys: &Value,
    globals: &dyn Fn(&str) -> Result<Option<Value>>,
) -> Result<String> {
    Ok(substitute_inner(
        prose, args, reply, item, var, sys, globals,
    )?)
}

fn substitute_inner(
    prose: &str,
    args: &str,
    reply: Option<&str>,
    item: Option<&Value>,
    var: &Value,
    sys: &Value,
    globals: &dyn Fn(&str) -> Result<Option<Value>>,
) -> SubstResult<String> {
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
            out.push_str(&resolve(path, start, args, reply, item, var, sys, globals)?);
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

/// Resolve a single `{{ }}` path to its rendered string.
#[expect(
    clippy::too_many_arguments,
    reason = "the resolver keeps the path, its offset, and the five substitution sources explicit and linear"
)]
fn resolve(
    path: &str,
    offset: usize,
    args: &str,
    reply: Option<&str>,
    item: Option<&Value>,
    var: &Value,
    sys: &Value,
    globals: &dyn Fn(&str) -> Result<Option<Value>>,
) -> SubstResult<String> {
    if path == "args" {
        return Ok(args.to_string());
    }
    if path == "reply" {
        return reply.map(String::from).ok_or_else(|| {
            SubstitutionError::new(
                SubstErrorKind::NilReply,
                offset,
                "{{ reply }} is nil (no prior section reply)".to_string(),
            )
        });
    }
    if path == "item" {
        return item.map(render_item).ok_or_else(|| {
            SubstitutionError::new(
                SubstErrorKind::NilItem,
                offset,
                "{{ item }} is nil (not inside a fanout arm)".to_string(),
            )
        });
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
        "var" => var,
        "sys" => sys,
        "args" | "reply" | "item" => {
            return Err(SubstitutionError::new(
                SubstErrorKind::NotATable,
                offset,
                format!("{namespace} is a string, not a table"),
            ));
        }
        other => {
            global_value = globals(other)
                .map_err(|error| {
                    SubstitutionError::with_source(
                        SubstErrorKind::Serialize,
                        offset,
                        format!(
                            "global '{}' in {{{{ {} }}}} is not JSON data",
                            path_preview(other),
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
                            path_preview(other),
                            path_preview(path)
                        ),
                    )
                })?;
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

/// Render a resolved JSON value as its substituted string.
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

    fn run(prose: &str) -> Result<String> {
        let var = json!({ "kind": "library", "count": 3, "row": { "a": 1 } });
        let sys = json!({ "when": "2026-07-29T00:00:00Z", "id": 1 });
        substitute(prose, "Acme Corp", None, None, &var, &sys, &no_globals)
    }

    fn err_of(prose: &str) -> SubstitutionError {
        let var = json!({ "kind": "library", "row": { "a": 1 }, "arr": [1, 2] });
        let sys = json!({ "id": 1 });
        substitute_inner(
            prose,
            "Acme Corp",
            Some("r"),
            Some(&json!("i")),
            &var,
            &sys,
            &no_globals,
        )
        .expect_err("expected substitution failure")
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
        let out = substitute(
            "value: {{ var.payload }}",
            "SECRET",
            None,
            None,
            &var,
            &sys,
            &no_globals,
        )
        .unwrap();
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
        let out = substitute(
            "the answer is {{ answer }}",
            "",
            None,
            None,
            &var,
            &sys,
            &globals,
        )
        .unwrap();
        assert_eq!(out, "the answer is 42");
    }

    #[test]
    fn bare_global_dotted_path_indexes_the_json() {
        let var = json!({});
        let sys = json!({});
        let globals = |name: &str| Ok((name == "row").then(|| json!({ "a": { "b": 2 } })));
        let out = substitute("cell {{ row.a.b }}", "", None, None, &var, &sys, &globals).unwrap();
        assert_eq!(out, "cell 2");
    }

    #[test]
    fn bare_global_table_renders_as_json() {
        let var = json!({});
        let sys = json!({});
        let globals = |name: &str| Ok((name == "row").then(|| json!({ "a": 1 })));
        let out = substitute("{{ row }}", "", None, None, &var, &sys, &globals).unwrap();
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
        let e = substitute_inner("{{ f }}", "", None, None, &var, &sys, &globals).unwrap_err();
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
    fn null_value_and_reply_item_kinds() {
        let var = json!({ "n": Value::Null });
        let sys = json!({});
        let e =
            substitute_inner("{{ var.n }}", "", None, None, &var, &sys, &no_globals).unwrap_err();
        assert_eq!(e.kind, SubstErrorKind::NullValue);

        let e =
            substitute_inner("{{ reply }}", "", None, None, &var, &sys, &no_globals).unwrap_err();
        assert_eq!(e.kind, SubstErrorKind::NilReply);
        let e =
            substitute_inner("{{ item }}", "", None, None, &var, &sys, &no_globals).unwrap_err();
        assert_eq!(e.kind, SubstErrorKind::NilItem);
    }

    #[test]
    fn not_a_table_kind() {
        let e = err_of("{{ reply.x }}");
        assert_eq!(e.kind, SubstErrorKind::NotATable);
        assert!(e.to_string().contains("not a table"));
    }

    // --- SUBST-006: null, arrays, trust-neutral passthrough ------------------

    #[test]
    fn array_renders_as_json() {
        let var = json!({ "arr": [1, 2, 3] });
        let sys = json!({});
        let out = substitute("{{ var.arr }}", "", None, None, &var, &sys, &no_globals).unwrap();
        assert_eq!(out, "[1,2,3]");
    }

    #[test]
    fn resolves_reply_when_present() {
        let var = json!({});
        let sys = json!({});
        let out = substitute(
            "prev: {{ reply }}",
            "",
            Some("model output"),
            None,
            &var,
            &sys,
            &no_globals,
        )
        .unwrap();
        assert_eq!(out, "prev: model output");
    }

    #[test]
    fn reply_nil_is_error() {
        let var = json!({});
        let sys = json!({});
        let err = substitute("{{ reply }}", "", None, None, &var, &sys, &no_globals)
            .expect_err("nil reply must fail");
        assert!(
            err.to_string().contains("nil"),
            "error must mention nil: {err}"
        );
    }

    #[test]
    fn reply_dot_path_is_error() {
        let var = json!({});
        let sys = json!({});
        let err = substitute(
            "{{ reply.x }}",
            "",
            Some("text"),
            None,
            &var,
            &sys,
            &no_globals,
        )
        .expect_err("reply is a string, not a table");
        assert!(
            err.to_string().contains("not a table"),
            "error must say not a table: {err}"
        );
    }

    #[test]
    fn resolves_item_when_present() {
        let var = json!({});
        let sys = json!({});
        let out = substitute(
            "topic: {{ item }}",
            "",
            None,
            Some(&json!("the angle")),
            &var,
            &sys,
            &no_globals,
        )
        .unwrap();
        assert_eq!(out, "topic: the angle");
    }

    #[test]
    fn item_nil_is_error() {
        let var = json!({});
        let sys = json!({});
        let err = substitute("{{ item }}", "", None, None, &var, &sys, &no_globals)
            .expect_err("nil item must fail");
        assert!(
            err.to_string().contains("nil"),
            "error must mention nil: {err}"
        );
    }

    #[test]
    fn item_dot_path_is_error() {
        let var = json!({});
        let sys = json!({});
        let err = substitute(
            "{{ item.x }}",
            "",
            None,
            Some(&json!("text")),
            &var,
            &sys,
            &no_globals,
        )
        .expect_err("item is a string, not a table");
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
