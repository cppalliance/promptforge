//! The `mcp.json` entry a server is installed from: its shape, its
//! variables, and its headers.

use std::collections::HashMap;
use std::fmt;

use promptforge_plugin::ToolError;
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{Map, Value};

/// The reason every local entry is refused with, until local servers
/// arrive.
pub(crate) const LOCAL_NOT_SUPPORTED: &str = "local MCP servers (command) are not supported yet";

/// Headers rmcp owns, in lowercase. rmcp refuses one of them only on the
/// first request, so the entry is refused here instead.
const RESERVED_HEADERS: [&str; 3] = ["accept", "mcp-session-id", "last-event-id"];

/// A remote server: where it is and the headers every request carries.
pub(crate) struct RemoteEntry {
    pub(crate) url: String,
    pub(crate) headers: HashMap<HeaderName, HeaderValue>,
}

impl fmt::Debug for RemoteEntry {
    /// Names the header keys only, because a header value may be a secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut keys: Vec<&str> = self.headers.keys().map(HeaderName::as_str).collect();
        keys.sort_unstable();
        f.debug_struct("RemoteEntry")
            .field("header_keys", &keys)
            .finish_non_exhaustive()
    }
}

/// Reads variables for [`expand`]: the environment, then the home
/// directory. Both are parameters so a test sets them without touching the
/// process.
pub(crate) struct Vars<'a> {
    pub(crate) env: &'a dyn Fn(&str) -> Option<String>,
    pub(crate) home: Option<String>,
}

fn refuse(text: impl Into<String>) -> ToolError {
    ToolError::message(text)
}

/// Parses `config` into a remote entry.
///
/// # Errors
/// Refuses a local entry with [`LOCAL_NOT_SUPPORTED`], an entry that is
/// both shapes or neither, a wrong value type, the legacy `sse` type, an
/// unset `${env:NAME}`, and a header rmcp owns or that is not a valid
/// header. No message holds an entry value.
pub(crate) fn parse(config: &Value, vars: &Vars<'_>) -> Result<RemoteEntry, ToolError> {
    let entry = config.as_object().ok_or_else(|| {
        refuse("an MCP entry must be a JSON object with `url`, or with `command`")
    })?;
    match (entry.contains_key("url"), entry.contains_key("command")) {
        (true, true) => Err(refuse(
            "an MCP entry has `url` or `command`, and this one has both",
        )),
        (false, false) => Err(refuse(
            "an MCP entry needs `url` for a remote server or `command` for a local one, and this one has neither",
        )),
        (false, true) => {
            check_local_shape(entry)?;
            Err(refuse(LOCAL_NOT_SUPPORTED))
        }
        (true, false) => parse_remote(entry, vars),
    }
}

/// Checks the value types of a local entry's fields, so a malformed one is
/// named as malformed. No variable is resolved.
fn check_local_shape(entry: &Map<String, Value>) -> Result<(), ToolError> {
    string_field(entry, "command")?;
    if let Some(args) = entry.get("args") {
        let all_strings = args
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string));
        if !all_strings {
            return Err(wrong_type("args", "an array of strings"));
        }
    }
    if let Some(env) = entry.get("env") {
        string_map(env, "env")?;
    }
    string_field(entry, "cwd")?;
    Ok(())
}

fn parse_remote(entry: &Map<String, Value>, vars: &Vars<'_>) -> Result<RemoteEntry, ToolError> {
    if entry.get("type").and_then(Value::as_str) == Some("sse") {
        return Err(refuse(
            "type `sse` is the legacy MCP transport, and only Streamable HTTP is supported",
        ));
    }
    let url = string_field(entry, "url")?.unwrap_or_default();
    let url = expand(url, vars).map_err(|why| refuse(format!("`url` {why}")))?;
    let scheme_ok = ["http://", "https://"].iter().any(|scheme| {
        url.get(..scheme.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(scheme))
    });
    if !scheme_ok {
        return Err(refuse("`url` must start with http:// or https://"));
    }
    let mut headers = HashMap::new();
    if let Some(value) = entry.get("headers") {
        for (key, raw) in string_map(value, "headers")? {
            let (name, value) = header(key, raw, vars)?;
            headers.insert(name, value);
        }
    }
    Ok(RemoteEntry { url, headers })
}

/// One header: a valid name that rmcp does not own, and a valid value
/// with its variables resolved. The value is marked sensitive, so no
/// `Debug` output shows it.
fn header(key: &str, raw: &str, vars: &Vars<'_>) -> Result<(HeaderName, HeaderValue), ToolError> {
    let name = HeaderName::from_bytes(key.as_bytes())
        .map_err(|_| refuse(format!("header `{key}` is not a valid header name")))?;
    if RESERVED_HEADERS.contains(&name.as_str()) {
        return Err(refuse(format!(
            "header `{key}` is set by the MCP client, so `headers` may not name it"
        )));
    }
    let text = expand(raw, vars).map_err(|why| refuse(format!("header `{key}` {why}")))?;
    let mut value = HeaderValue::from_str(&text).map_err(|_| {
        refuse(format!(
            "header `{key}` has a value that is not a valid header value"
        ))
    })?;
    value.set_sensitive(true);
    Ok((name, value))
}

fn wrong_type(field: &str, wanted: &str) -> ToolError {
    refuse(format!("`{field}` must be {wanted}"))
}

/// The string at `field`, or `None` when it is absent.
fn string_field<'a>(
    entry: &'a Map<String, Value>,
    field: &str,
) -> Result<Option<&'a str>, ToolError> {
    match entry.get(field) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(wrong_type(field, "a string")),
    }
}

/// The key and string value of each member of the object at `value`.
fn string_map<'a>(value: &'a Value, field: &str) -> Result<Vec<(&'a str, &'a str)>, ToolError> {
    let members = value
        .as_object()
        .ok_or_else(|| wrong_type(field, "an object of strings"))?;
    members
        .iter()
        .map(|(key, member)| {
            member
                .as_str()
                .map(|text| (key.as_str(), text))
                .ok_or_else(|| refuse(format!("`{field}.{key}` must be a string")))
        })
        .collect()
}

/// Expands `${env:NAME}` and `${userHome}` in `text`, and passes any other
/// `${...}` form through verbatim, as Cursor does. The error reads as the
/// end of a sentence about the value, and names the variable, never a
/// value.
pub(crate) fn expand(text: &str, vars: &Vars<'_>) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let inner = &after[..end];
        if let Some(name) = inner.strip_prefix("env:") {
            let value = (vars.env)(name)
                .ok_or_else(|| format!("uses ${{env:{name}}}, and {name} is not set"))?;
            out.push_str(&value);
        } else if inner == "userHome" {
            let home = vars
                .home
                .as_deref()
                .ok_or("uses ${userHome}, and this machine has no home directory")?;
            out.push_str(home);
        } else {
            out.push_str("${");
            out.push_str(inner);
            out.push('}');
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
#[path = "entry-tests.rs"]
mod tests;
