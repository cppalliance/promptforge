//! `${VAR}` interpolation over a parsed `prompts.toml` document.
//!
//! Interpolation runs after the TOML parse rather than over the raw text, so an
//! unset variable is attributed to the field that carried it. That is what lets
//! `[server].api_key` alone survive an unset variable while every other field
//! still fails the load.

use crate::error::{ConfigError, ConfigErrorKind};

/// Expands `${VAR}` in every string the parsed document carries.
///
/// Interpolating after the parse rather than over the raw text is what lets one
/// field be treated differently from the rest. `[server].api_key` is that
/// field: an unset variable there drops the key, because the HTTP transport
/// refuses to bind without one anyway and the stdio transport never reads it,
/// so failing the load would stop a local install over a credential it does not
/// use. Everywhere else an unset variable still fails the load, which is what
/// keeps the gateway from starting with a blank credential.
pub(super) fn interpolate_document(
    document: &mut toml::Table,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ConfigError> {
    if let Some(server) = document
        .get_mut("server")
        .and_then(toml::Value::as_table_mut)
        && let Some(toml::Value::String(api_key)) = server.get("api_key")
        && interpolate_with(api_key, lookup)
            .is_err_and(|e| e.kind() == ConfigErrorKind::UnresolvedVar)
    {
        server.remove("api_key");
    }
    interpolate_table(document, lookup)
}

/// Expands `${VAR}` in every string under one table.
fn interpolate_table(
    table: &mut toml::Table,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ConfigError> {
    for (_, value) in table.iter_mut() {
        interpolate_value(value, lookup)?;
    }
    Ok(())
}

/// Expands `${VAR}` in one value, reaching through arrays and tables. A number,
/// a boolean, and a datetime carry no text to expand.
fn interpolate_value(
    value: &mut toml::Value,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(text) => *text = interpolate_with(text, lookup)?,
        toml::Value::Array(items) => {
            for item in items {
                interpolate_value(item, lookup)?;
            }
        }
        toml::Value::Table(table) => interpolate_table(table, lookup)?,
        _ => {}
    }
    Ok(())
}

/// Expands `${VAR}` through `lookup`, which answers `None` for an unset name;
/// `$$` is a literal `$`.
pub(super) fn interpolate_with(
    input: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, ConfigError> {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some('{') => {
                chars.next();
                let mut name = String::new();
                let mut closed = false;
                for nc in chars.by_ref() {
                    if nc == '}' {
                        closed = true;
                        break;
                    }
                    name.push(nc);
                }
                if !closed {
                    return Err(ConfigError::interpolation("unclosed ${...} interpolation"));
                }
                let value = lookup(&name).ok_or_else(|| ConfigError::unresolved_var(name))?;
                out.push_str(&value);
            }
            _ => out.push('$'),
        }
    }
    Ok(out)
}
