//! Pending-document secret restoration and variable-reference inspection.

use std::collections::BTreeMap;
use std::path::Path;

use toml::Value;

use super::{Repr, read_pending_or_real};

const REDACTED: &str = "***";

/// Collects `${VAR}` references from the shadow-preferred single config file.
///
/// Values never enter the result. Each variable maps only to labels of the
/// string fields that reference it.
///
/// # Errors
/// Returns [`ConfigError`](crate::ConfigError) when neither config nor shadow
/// can be read or parsed.
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::pending_var_references;
/// use std::path::Path;
///
/// let references = pending_var_references(Path::new("gateway.toml"))?;
/// assert!(references.keys().all(|name| !name.is_empty()));
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn pending_var_references(
    config_path: &Path,
) -> Result<BTreeMap<String, Vec<String>>, crate::ConfigError> {
    let Some((_path, value)) =
        read_pending_or_real(config_path).map_err(crate::ConfigError::from)?
    else {
        return Err(crate::ConfigError::from(Repr::Read {
            path: config_path.to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "config not found"),
        }));
    };
    let mut references = BTreeMap::new();
    collect_var_references(&value, &mut Vec::new(), &mut references);
    Ok(references)
}

fn keyed_entry_key(array: &str) -> Option<&'static str> {
    match array {
        "model" | "local_model" | "stt_model" | "profile" => Some("name"),
        "endpoint" | "dominion" => Some("id"),
        _ => None,
    }
}

fn collect_var_references(
    value: &Value,
    label: &mut Vec<String>,
    references: &mut BTreeMap<String, Vec<String>>,
) {
    match value {
        Value::String(text) => {
            for name in referenced_var_names(text) {
                let labels = references.entry(name).or_default();
                let joined = label.join(" ");
                if !labels.contains(&joined) {
                    labels.push(joined);
                }
            }
        }
        Value::Array(items) => {
            let identity_key = label.last().and_then(|array| keyed_entry_key(array));
            for item in items {
                if let (Some(identity_key), Value::Table(table)) = (identity_key, item) {
                    let identity = table
                        .get(identity_key)
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    label.push(identity.to_owned());
                    collect_var_references(item, label, references);
                    label.pop();
                } else {
                    collect_var_references(item, label, references);
                }
            }
        }
        Value::Table(table) => {
            for (key, child) in table {
                label.push(key.clone());
                collect_var_references(child, label, references);
                label.pop();
            }
        }
        _ => {}
    }
}

fn referenced_var_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '$' {
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
            }
            Some('{') => {
                chars.next();
                let mut name = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    name.push(next);
                }
                if closed && !name.is_empty() {
                    names.push(name);
                }
            }
            _ => {}
        }
    }
    names
}

pub(super) fn restore_secrets(candidate: &mut Value, current: Option<&Value>) -> Result<(), Repr> {
    if let (Value::Table(table), current) = (&mut *candidate, current) {
        for (key, child) in table {
            let counterpart = current.and_then(|value| value.get(key.as_str()));
            if let Some(identity_key) = keyed_entry_key(key) {
                restore_keyed_array(child, counterpart, key, identity_key)?;
            } else {
                restore_node(child, counterpart, key)?;
            }
        }
        return Ok(());
    }
    restore_node(candidate, current, "")
}

fn restore_keyed_array(
    candidate: &mut Value,
    current: Option<&Value>,
    array_name: &str,
    identity_key: &str,
) -> Result<(), Repr> {
    let Value::Array(items) = candidate else {
        return restore_node(candidate, current, array_name);
    };
    for (index, item) in items.iter_mut().enumerate() {
        let identity = item
            .get(identity_key)
            .and_then(Value::as_str)
            .map(str::to_owned);
        let counterpart = match (&identity, current) {
            (Some(identity), Some(Value::Array(existing))) => existing.iter().find(|entry| {
                entry.get(identity_key).and_then(Value::as_str) == Some(identity.as_str())
            }),
            _ => None,
        };
        let label = identity.map_or_else(
            || format!("{array_name}[{index}]"),
            |identity| format!("{array_name} {identity}"),
        );
        restore_node(item, counterpart, &label)?;
    }
    Ok(())
}

fn restore_node(candidate: &mut Value, current: Option<&Value>, path: &str) -> Result<(), Repr> {
    match candidate {
        Value::String(text) if text == REDACTED => match current {
            Some(Value::String(existing)) => {
                existing.clone_into(text);
                Ok(())
            }
            _ => Err(Repr::Validation(format!(
                "secret marker \"***\" at {path} has no existing value to preserve"
            ))),
        },
        Value::Table(table) => {
            for (key, child) in table {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                restore_node(
                    child,
                    current.and_then(|value| value.get(key.as_str())),
                    &child_path,
                )?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                restore_node(
                    item,
                    current.and_then(|value| value.get(index)),
                    &format!("{path}[{index}]"),
                )?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
