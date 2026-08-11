//! `${VAR}` environment interpolation over parsed TOML.
//!
//! Interpolation runs *after* the TOML is parsed and only on string leaves, so
//! `${VAR}` inside comments or keys is never expanded and an interpolated value
//! containing a quote, backslash, or newline can never corrupt the document
//! structure on a re-parse. (CFG-007)

use crate::error::ConfigError;

/// Expand `${VAR}` from the environment; `$$` is a literal `$`.
///
/// # Errors
/// Returns [`ConfigError::Interpolation`] on an unclosed `${...}` and
/// [`ConfigError::UnresolvedVar`] when a referenced variable is unset.
pub(crate) fn interpolate(input: &str) -> Result<String, ConfigError> {
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
                    return Err(ConfigError::Interpolation(
                        "unclosed ${...} interpolation".to_string(),
                    ));
                }
                let value =
                    std::env::var(&name).map_err(|_| ConfigError::UnresolvedVar(name.clone()))?;
                out.push_str(&value);
            }
            _ => out.push('$'),
        }
    }
    Ok(out)
}

/// Recursively interpolate `${VAR}` in every string leaf of a TOML value,
/// leaving keys, comments (already stripped by the parser), and non-string
/// scalars untouched. (CFG-007)
pub(crate) fn interpolate_value(value: &mut toml::Value) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(text) => {
            *text = interpolate(text)?;
        }
        toml::Value::Array(items) => {
            for item in items {
                interpolate_value(item)?;
            }
        }
        toml::Value::Table(table) => {
            for (_, entry) in table.iter_mut() {
                interpolate_value(entry)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::interpolate;

    #[test]
    fn double_dollar_is_literal() {
        assert_eq!(interpolate("cost is $$5").unwrap(), "cost is $5");
    }

    #[test]
    fn unset_variable_is_an_error() {
        assert!(interpolate("${PFG_DEFINITELY_UNSET_VAR_XYZ}").is_err());
    }

    #[test]
    fn unclosed_interpolation_is_an_error() {
        assert!(interpolate("${UNCLOSED").is_err());
    }
}
