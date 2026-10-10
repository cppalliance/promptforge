//! The frontmatter contract keys: `plugins`, `args`, `models`.
//!
//! The YAML is the whole contract: Plugins install, models declare, args
//! type. Parsing validates the static shape - each Plugin a one-segment
//! plain name declared once, the alias grammar on map keys, the closed
//! model-keyword vocabulary, arg name and type sanity - and exposes the
//! FULL declaration on the parsed [`Prompt`](crate::Prompt); satisfying
//! the declaration against the caller's environment is prepare's job,
//! never the parser's.
//!
//! `args` and `models` are defined in submodules; this root owns the
//! duplicate-Plugin check and the map deserializer the two map keys share.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use promptforge_types::plugins::PluginId;

mod args;
mod models;

#[cfg(test)]
mod tests;

pub use args::{ArgDecl, ArgType, ArgsDecl};
pub use models::{ModelKeyword, ModelRole, ModelRoles};

/// The prompt-local alias grammar: `[A-Za-z][A-Za-z0-9_-]{0,63}`.
///
/// Model labels and args field names are prompt-local and never global
/// names. (The same rule sits in `promptforge-lua`'s `alias` module, the
/// run-time counterpart to this parse-time check.)
fn is_valid_alias(alias: &str) -> bool {
    let bytes = alias.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphabetic()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// How one contract map's keys are described in its errors.
#[derive(Clone, Copy)]
struct ContractKeys {
    /// The key kind (`model role label`), for error messages.
    what: &'static str,
}

/// Deserializes a contract map (`models`, `args`): string keys
/// validated against the alias grammar and `keys`, values deserialized as
/// `T`, duplicates rejected.
fn deserialize_contract_map<'de, D, T>(
    deserializer: D,
    keys: ContractKeys,
) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    deserializer.deserialize_map(MapVisitor {
        keys,
        _marker: PhantomData,
    })
}

/// The visitor behind [`deserialize_contract_map`]: a streaming map walk so
/// rejections keep their source position.
struct MapVisitor<T> {
    /// The checks the map's keys get.
    keys: ContractKeys,
    /// The value type, without ownership or variance claims.
    _marker: PhantomData<fn() -> T>,
}

impl<'de, T> Visitor<'de> for MapVisitor<T>
where
    T: Deserialize<'de>,
{
    type Value = BTreeMap<String, T>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "a map of {} keys to declarations",
            self.keys.what
        )
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let ContractKeys { what } = self.keys;
        let mut entries: BTreeMap<String, T> = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            if !is_valid_alias(&key) {
                return Err(de::Error::custom(format!(
                    "invalid {what} `{key}`: expected [A-Za-z][A-Za-z0-9_-]{{0,63}}"
                )));
            }
            if entries.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "duplicate {what} `{key}`: contract map keys must be unique"
                )));
            }
            entries.insert(key, map.next_value::<T>()?);
        }
        Ok(entries)
    }
}

/// Refuses a `plugins:` list that names one Plugin twice. Returns the
/// refusal's message, naming the first Plugin declared again.
pub(crate) fn check_distinct_plugins(plugins: &[PluginId]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    match plugins.iter().find(|plugin| !seen.insert(*plugin)) {
        Some(plugin) => Err(format!(
            "invalid frontmatter: Plugin {plugin} is declared more than once under plugins"
        )),
        None => Ok(()),
    }
}
