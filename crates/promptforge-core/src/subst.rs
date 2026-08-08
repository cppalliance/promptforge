//! `{{ }}` prose substitution.
//!
//! After a section's Lua preamble runs, the harness resolves `{{ path }}`
//! placeholders in the prose before the model sees it. Lua source in the
//! preamble and epilog is never substituted. Four namespaces are available:
//! `args` (the single raw input string), `reply` (the previous section's model
//! reply, nil in section 1), `var` (values the preamble wrote), and `sys`
//! (runtime-provided metadata). Resolution is a single pass with no recursion:
//! scalars render as strings, tables/arrays as JSON, and a missing path is a
//! hard error. `{{ reply }}` when nil is a hard error. Substitution does no
//! arithmetic - compute in Lua and reference the result.

use serde_json::Value;

use crate::{Error, Result};

/// Resolve every `{{ path }}` in `prose` against `args`, `reply`, `var`,
/// and `sys`.
///
/// `var` and `sys` are JSON objects (`var` read back from the Lua preamble,
/// `sys` built by the runtime). `reply` is the previous section's model
/// reply text, or `None` in the first section. This function receives prose
/// only and does not transform either compiled Lua phase.
///
/// # Errors
/// Returns [`Error::Substitution`] for an unclosed `{{`, an unknown namespace, a
/// missing key, a null value, or a `{{ reply }}` reference when `reply` is
/// `None` (section 1).
pub fn substitute(
    prose: &str,
    args: &str,
    reply: Option<&str>,
    var: &Value,
    sys: &Value,
) -> Result<String> {
    let mut out = String::with_capacity(prose.len());
    let mut rest = prose;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| Error::Substitution("unclosed '{{' in prose".to_string()))?;
        let path = after[..end].trim();
        out.push_str(&resolve(path, args, reply, var, sys)?);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Resolve a single `{{ }}` path to its rendered string.
fn resolve(
    path: &str,
    args: &str,
    reply: Option<&str>,
    var: &Value,
    sys: &Value,
) -> Result<String> {
    if path == "args" {
        return Ok(args.to_string());
    }
    if path == "reply" {
        return reply.map(String::from).ok_or_else(|| {
            Error::Substitution("{{ reply }} is nil (no prior section reply)".to_string())
        });
    }
    let Some((namespace, keys)) = path.split_once('.') else {
        return Err(Error::Substitution(format!("bad path: {{{{ {path} }}}}")));
    };
    let root = match namespace {
        "var" => var,
        "sys" => sys,
        "args" => {
            return Err(Error::Substitution(
                "args is a string, not a table".to_string(),
            ));
        }
        "reply" => {
            return Err(Error::Substitution(
                "reply is a string, not a table".to_string(),
            ));
        }
        other => {
            return Err(Error::Substitution(format!(
                "unknown namespace '{other}' in {{{{ {path} }}}}"
            )));
        }
    };
    let mut current = root;
    for key in keys.split('.') {
        current = current
            .get(key)
            .ok_or_else(|| Error::Substitution(format!("missing {{{{ {path} }}}}")))?;
    }
    render(current, path)
}

/// Render a resolved JSON value as its substituted string.
fn render(value: &Value, path: &str) -> Result<String> {
    match value {
        Value::Null => Err(Error::Substitution(format!("missing {{{{ {path} }}}}"))),
        Value::String(s) => Ok(s.clone()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Array(_) | Value::Object(_) => {
            serde_json::to_string(value).map_err(|e| Error::Substitution(e.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(prose: &str) -> Result<String> {
        let var = json!({ "kind": "library", "count": 3, "row": { "a": 1 } });
        let sys = json!({ "when": "2026-07-29T00:00:00Z", "id": 1 });
        substitute(prose, "Acme Corp", None, &var, &sys)
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
        assert!(run("open {{ args").is_err());
    }

    #[test]
    fn resolves_reply_when_present() {
        let var = json!({});
        let sys = json!({});
        let out = substitute("prev: {{ reply }}", "", Some("model output"), &var, &sys).unwrap();
        assert_eq!(out, "prev: model output");
    }

    #[test]
    fn reply_nil_is_error() {
        let var = json!({});
        let sys = json!({});
        let err = substitute("{{ reply }}", "", None, &var, &sys).expect_err("nil reply must fail");
        assert!(
            err.to_string().contains("nil"),
            "error must mention nil: {err}"
        );
    }

    #[test]
    fn reply_dot_path_is_error() {
        let var = json!({});
        let sys = json!({});
        let err = substitute("{{ reply.x }}", "", Some("text"), &var, &sys)
            .expect_err("reply is a string, not a table");
        assert!(
            err.to_string().contains("not a table"),
            "error must say not a table: {err}"
        );
    }
}
