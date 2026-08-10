//! Shared value decoding for the `models.*` host tables.
//!
//! Parses Lua argument shapes (`models.need`/`models.always` args and the opts
//! table) and validates scalar option values at the Lua trust boundary.

use std::num::NonZeroU32;

use mlua::{MultiValue, Table, Value};

use crate::model::{ModelNeedOpts, Temperature};
use crate::{Error, Result};

/// Extracts a single string alias from a `MultiValue` (for the 1-arg form).
pub(crate) fn parse_single_alias(args: &MultiValue, label: &str) -> mlua::Result<String> {
    match args.iter().next() {
        Some(Value::String(value)) => value
            .to_str()
            .map_err(|_| mlua::Error::external(format!("{label} alias must be a UTF-8 string")))
            .map(|s| s.to_owned()),
        _ => Err(mlua::Error::external(format!(
            "{label} expects a string alias as first argument"
        ))),
    }
}

pub(crate) fn parse_need_args(args: MultiValue) -> mlua::Result<(String, String, ModelNeedOpts)> {
    let mut values = args.into_iter();
    let alias = match values.next() {
        Some(Value::String(value)) => value
            .to_str()
            .map_err(|_| mlua::Error::external("models.need alias must be a UTF-8 string"))?
            .to_owned(),
        _ => {
            return Err(mlua::Error::external(
                "models.need expects alias, description, and optional opts table",
            ));
        }
    };
    let description = match values.next() {
        Some(Value::String(value)) => value
            .to_str()
            .map_err(|_| mlua::Error::external("models.need description must be a UTF-8 string"))?
            .to_owned(),
        _ => {
            return Err(mlua::Error::external(
                "models.need expects alias, description, and optional opts table",
            ));
        }
    };
    let opts = match values.next() {
        None | Some(Value::Nil) => ModelNeedOpts::default(),
        Some(Value::Table(table)) => parse_opts_table(&table)?,
        Some(_) => {
            return Err(mlua::Error::external(
                "models.need opts must be a table when provided",
            ));
        }
    };
    if values.next().is_some() {
        return Err(mlua::Error::external(
            "models.need expects at most three arguments",
        ));
    }
    Ok((alias, description, opts))
}

pub(crate) fn parse_opts_table(table: &Table) -> mlua::Result<ModelNeedOpts> {
    let mut opts = ModelNeedOpts::default();
    for pair in table.pairs::<Value, Value>() {
        // Propagate the original `mlua::Error` unchanged (PF-LM-012): it already
        // carries its source chain, so re-wrapping its text would discard it.
        let (key, value) = pair?;
        let key = match key {
            Value::String(key) => key
                .to_str()
                .map_err(|_| mlua::Error::external("models.need opts key must be a UTF-8 string"))?
                .to_owned(),
            _ => {
                return Err(mlua::Error::external(
                    "models.need opts keys must be strings",
                ));
            }
        };
        match key.as_str() {
            "thinking" => {
                opts.thinking = Some(value_as_bool(&value, "thinking")?);
            }
            "context" => {
                opts.context = Some(value_as_nonzero_u32(&value, "context")?);
            }
            "temperature" => {
                opts.temperature = Some(value_as_temperature(&value)?);
            }
            "max_tokens" => {
                opts.max_tokens = Some(value_as_nonzero_u32(&value, "max_tokens")?);
            }
            other => {
                return Err(mlua::Error::external(format!(
                    "unknown models.need opts key {other:?}"
                )));
            }
        }
    }
    Ok(opts)
}

/// Parses and validates a sampling temperature at the Lua trust boundary.
///
/// Lua integer and number forms are decoded through ONE numeric path (no
/// arbitrary `i32` gate), then validated by the core-owned [`Temperature`]
/// newtype - the single source of truth for the finite `[0.0, 2.0]` domain
/// (PF-LM-004/PF-LM-005). A non-finite (`NaN`, infinity) or out-of-domain value
/// is rejected here rather than forwarded to the gateway, and the validated
/// value travels onward as a `Temperature`, not a raw `f64`.
pub(crate) fn value_as_temperature(value: &Value) -> mlua::Result<Temperature> {
    let temperature = decode_lua_number(value, "temperature")?;
    Temperature::new(temperature).map_err(|error| {
        mlua::Error::external(format!("models.need opts.temperature is invalid: {error}"))
    })
}

pub(crate) fn value_as_bool(value: &Value, field: &str) -> mlua::Result<bool> {
    match value {
        Value::Boolean(flag) => Ok(*flag),
        _ => Err(mlua::Error::external(format!(
            "models.need opts.{field} must be a boolean"
        ))),
    }
}

pub(crate) fn value_as_u32(value: &Value, field: &str) -> mlua::Result<u32> {
    match value {
        Value::Integer(number) => u32::try_from(*number).map_err(|_| {
            mlua::Error::external(format!(
                "models.need opts.{field} must be a non-negative integer"
            ))
        }),
        Value::Number(number) if number.fract() == 0.0 => {
            let truncated = number.trunc();
            if (0.0..=f64::from(u32::MAX)).contains(&truncated) {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "range checked against u32::MAX and non-negative"
                )]
                Ok(truncated as u32)
            } else {
                Err(mlua::Error::external(format!(
                    "models.need opts.{field} must be a non-negative integer"
                )))
            }
        }
        _ => Err(mlua::Error::external(format!(
            "models.need opts.{field} must be a non-negative integer"
        ))),
    }
}

/// Decodes a positive Lua count into a [`NonZeroU32`], rejecting zero.
///
/// Domain counts (`context`, `max_tokens`) must be non-zero (MODEL-003): a zero
/// context minimum is a nonsensical constraint and a zero generation cap would
/// forbid all output. Both are rejected here, at the Lua parse boundary, rather
/// than travelling as an ambiguous `0` toward the wire.
pub(crate) fn value_as_nonzero_u32(value: &Value, field: &str) -> mlua::Result<NonZeroU32> {
    let raw = value_as_u32(value, field)?;
    NonZeroU32::new(raw).ok_or_else(|| {
        mlua::Error::external(format!(
            "models.need opts.{field} must be greater than zero"
        ))
    })
}

/// Decodes a Lua integer or number into an `f64` through a single path.
///
/// Both Lua numeric forms are accepted and converted uniformly; the caller's
/// domain check (for example [`value_as_temperature`]) is the single place that
/// bounds the result, so there is no separate, arbitrary integer-range gate.
pub(crate) fn decode_lua_number(value: &Value, field: &str) -> mlua::Result<f64> {
    match value {
        Value::Number(number) => Ok(*number),
        Value::Integer(number) => {
            // Lua integers are i64. The caller bounds the domain (temperatures
            // live in [0.0, 2.0]); any magnitude that would lose precision here
            // is far outside that domain and rejected by the caller's check.
            #[expect(
                clippy::cast_precision_loss,
                reason = "domain is bounded by the caller; large magnitudes are rejected there"
            )]
            Ok(*number as f64)
        }
        _ => Err(mlua::Error::external(format!(
            "models.need opts.{field} must be a number"
        ))),
    }
}

pub(crate) fn validate_alias(alias: &str) -> Result<()> {
    let bytes = alias.as_bytes();
    let valid = (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphabetic()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(Error::Lua(format!(
            "invalid model alias {alias:?}: expected [A-Za-z][A-Za-z0-9_-]{{0,63}}"
        )))
    }
}
