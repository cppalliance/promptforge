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
//! Everywhere else a backslash is an ordinary literal. This lets prose
//! include a literal opening delimiter (`\{{` emits `{{`), a literal closing
//! delimiter (`\}}` emits `}}`), and a literal backslash (`\\` emits `\`).
//! Escapes compose, so adjacent escaped delimiters resolve independently.
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
/// Holds a stable [`kind`](SubstitutionError::kind), the byte
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
    /// The section's `var` table, read back from its Lua.
    pub(crate) var: &'a Value,
    /// The runtime-provided metadata: `{{ sys.key }}`.
    pub(crate) sys: &'a Value,
    /// The bare-global lookup for `{{ name }}` resolution.
    pub(crate) globals: &'a dyn Fn(&str) -> Result<Option<Value>>,
}

/// Resolves every `{{ path }}` in `prose` against the [`Sources`].
///
/// This function receives prose only; the compiled Lua phases are
/// untouched.
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
#[path = "subst-tests.rs"]
mod tests;
