//! The frontmatter contract keys: `plugins`, `tools`, `args`, `models`.
//!
//! The YAML is the whole contract: Plugins install, tools bind, models
//! declare, args type. Parsing validates the static shape - Plugin id
//! arity, each Plugin declared once, the alias grammar on slot keys,
//! the reserved names no tool alias or model role label may take, no tool
//! slot backed by an optional Plugin, the closed model-keyword
//! vocabulary, arg name and type sanity - and exposes the FULL declaration
//! on the parsed [`Prompt`](crate::Prompt); satisfying the declaration
//! against the caller's environment is prepare's job, never the parser's.
//!
//! `args` and `models` are defined in submodules; this root owns the
//! Plugin and tool-slot shapes plus the map deserializer all four keys
//! share.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use promptforge_types::names::GlobalName;
use promptforge_types::plugins::PluginId;
use promptforge_types::tools::ToolId;

mod args;
mod models;

#[cfg(test)]
mod tests;

pub use args::{ArgDecl, ArgType, ArgsDecl};
pub use models::{ModelKeyword, ModelRole, ModelRoles};

/// The prompt-local alias grammar: `[A-Za-z][A-Za-z0-9_-]{0,63}`.
///
/// Aliases are the only names a model ever sees - tool slot aliases, model
/// labels, and args field names are all prompt-local and never global
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

/// How one contract map's keys are checked beyond the alias grammar.
#[derive(Clone, Copy)]
struct ContractKeys {
    /// The frontmatter key the map sits under (`tools`), for error
    /// messages.
    map: &'static str,
    /// The key kind (`tool alias`), for error messages.
    what: &'static str,
    /// A key that satisfies the grammar but is rejected as reserved
    /// (`open`).
    deferred: Option<&'static str>,
    /// Whether each key installs as a section VM global of its own name,
    /// so a reserved name ([`promptforge_lua::RESERVED_NAMES`]) is refused.
    installs_global: bool,
}

/// Deserializes a contract map (`tools`, `models`, `args`): string keys
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
        let ContractKeys {
            map: map_key,
            what,
            deferred,
            installs_global,
        } = self.keys;
        let mut entries: BTreeMap<String, T> = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            if deferred == Some(key.as_str()) {
                return Err(de::Error::custom(format!(
                    "the `{key}` key is reserved for the deferred open toolset posture; it is not a usable {what}"
                )));
            }
            if !is_valid_alias(&key) {
                return Err(de::Error::custom(format!(
                    "invalid {what} `{key}`: expected [A-Za-z][A-Za-z0-9_-]{{0,63}}"
                )));
            }
            if installs_global && let Some(kind) = promptforge_lua::reserved_name(&key) {
                return Err(de::Error::custom(format!(
                    "{what} `{key}` in `{map_key}` is reserved ({kind}): tool aliases and model \
                     role labels install as section VM globals, so none may take a reserved name"
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

/// Parses a Plugin id: a [`GlobalName`] of exactly two segments
/// (`namespace/plugin`).
///
/// The grammar accepts two or three segments, so the arity check counts
/// separators: exactly one `/` is two segments. A `@` never gets that
/// far - the charset rejects it.
fn parse_plugin_id(text: &str) -> Result<GlobalName, String> {
    let name =
        GlobalName::parse(text).map_err(|error| format!("invalid Plugin id `{text}`: {error}"))?;
    if text.matches('/').count() != 1 {
        return Err(format!(
            "invalid Plugin id `{text}`: a Plugin id has exactly 2 segments (namespace/plugin)"
        ));
    }
    Ok(name)
}

/// One Plugin declared in a prompt's `plugins` frontmatter list.
///
/// A declaration takes one of two forms. A plain id string declares a
/// required Plugin. A map names the id under `ref` and may also set
/// `optional` (default `false`) and `config`.
///
/// `config` holds only data written in the prompt. User-specific
/// configuration, such as credentials or server lists, never appears in the
/// prompt. The caller supplies it through the run services.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PluginDecl {
    /// The Plugin's global id (`namespace/plugin`, exactly 2 segments).
    id: GlobalName,
    /// Whether an absent Plugin skips with a log line instead of
    /// failing preparation.
    optional: bool,
    /// Prompt-side configuration data, when declared.
    config: Option<serde_yaml_ng::Value>,
}

impl PluginDecl {
    /// Returns the Plugin's global id (`namespace/plugin`).
    #[must_use]
    pub fn id(&self) -> &GlobalName {
        &self.id
    }

    /// Returns whether the Plugin is optional.
    ///
    /// When an optional Plugin is absent, preparation logs a line, skips
    /// it, and continues.
    #[must_use]
    pub fn is_optional(&self) -> bool {
        self.optional
    }

    /// Returns the `config` data written in the prompt, when declared.
    #[must_use]
    pub fn config(&self) -> Option<&serde_yaml_ng::Value> {
        self.config.as_ref()
    }
}

impl<'de> Deserialize<'de> for PluginDecl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(PluginDeclVisitor)
    }
}

/// Deserializes a Plugin declaration from either frontmatter form: a
/// bare id string or a `ref` map. A streaming visitor (not an untagged
/// buffer) so rejections keep their source position.
struct PluginDeclVisitor;

impl<'de> Visitor<'de> for PluginDeclVisitor {
    type Value = PluginDecl;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a Plugin id string or a map with `ref`, `optional`, and `config`")
    }

    fn visit_str<E>(self, text: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(PluginDecl {
            id: parse_plugin_id(text).map_err(E::custom)?,
            optional: false,
            config: None,
        })
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut reference: Option<String> = None;
        let mut optional: Option<bool> = None;
        let mut config: Option<serde_yaml_ng::Value> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "ref" => {
                    if reference.is_some() {
                        return Err(de::Error::duplicate_field("ref"));
                    }
                    reference = Some(map.next_value()?);
                }
                "optional" => {
                    if optional.is_some() {
                        return Err(de::Error::duplicate_field("optional"));
                    }
                    optional = Some(map.next_value()?);
                }
                "config" => {
                    if config.is_some() {
                        return Err(de::Error::duplicate_field("config"));
                    }
                    config = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        &["ref", "optional", "config"],
                    ));
                }
            }
        }
        let reference = reference.ok_or_else(|| de::Error::missing_field("ref"))?;
        Ok(PluginDecl {
            id: parse_plugin_id(&reference).map_err(de::Error::custom)?,
            optional: optional.unwrap_or(false),
            config,
        })
    }
}

/// The tool that fills one alias in a prompt's `tools` frontmatter map.
///
/// The slot is written as a plain string holding an exact global tool path.
/// It is filled by the tool in the assembled catalog whose id is exactly
/// that path, and every fill is journaled.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolSlot {
    /// An exact global tool path, filled by the catalog tool with that same
    /// id.
    Exact(ToolId),
}

impl<'de> Deserialize<'de> for ToolSlot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ToolSlotVisitor)
    }
}

/// Deserializes a tool slot: a bare string is an exact path. Any other
/// shape (a map, a number) is rejected with the exact-path expectation.
struct ToolSlotVisitor;

impl Visitor<'_> for ToolSlotVisitor {
    type Value = ToolSlot;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an exact tool path string")
    }

    fn visit_str<E>(self, text: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let id = ToolId::parse(text)
            .map_err(|error| E::custom(format!("invalid exact tool path `{text}`: {error}")))?;
        Ok(ToolSlot::Exact(id))
    }
}

/// The tool slots a prompt declares in its `tools` frontmatter map, keyed by
/// alias.
///
/// An alias is a prompt-local name: a letter followed by up to 63 letters,
/// digits, underscores, or hyphens. The model sees only the alias.
///
/// Parsing rejects the key `open`, which is reserved. Each alias is
/// installed as a global of the same name in the section's Lua VM. For that
/// reason, parsing also rejects an alias that names an Engine global, a Lua
/// standard-library global the sandbox keeps, or a Lua keyword. It rejects
/// an alias that matches a model role label for the same reason.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolSlots {
    /// The slots by alias.
    slots: BTreeMap<String, ToolSlot>,
}

impl ToolSlots {
    /// Returns the slot declared under `alias`, when present.
    #[must_use]
    pub fn get(&self, alias: &str) -> Option<&ToolSlot> {
        self.slots.get(alias)
    }

    /// Iterates the declared slots as `(alias, slot)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ToolSlot)> {
        self.slots
            .iter()
            .map(|(alias, slot)| (alias.as_str(), slot))
    }

    /// Returns the number of declared slots.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Returns whether the set of declared slots is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

impl<'de> Deserialize<'de> for ToolSlots {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let slots = deserialize_contract_map(
            deserializer,
            ContractKeys {
                map: "tools",
                what: "tool alias",
                deferred: Some("open"),
                installs_global: true,
            },
        )?;
        Ok(ToolSlots { slots })
    }
}

/// Refuses a name declared both as a tool alias and as a model role label:
/// both install as section VM globals of their own name, so the model
/// handle would silently replace the tool handle. Returns the refusal's
/// message, naming the first shared name in sorted order.
pub(crate) fn check_distinct_aliases(tools: &ToolSlots, models: &ModelRoles) -> Result<(), String> {
    match tools.iter().find(|(alias, _)| models.get(alias).is_some()) {
        Some((alias, _)) => Err(format!(
            "invalid frontmatter: `{alias}` is both a tool alias in `tools` and a model role \
             label in `models`; each installs as a section VM global of its own name, so the \
             two must differ"
        )),
        None => Ok(()),
    }
}

/// Refuses a `plugins:` list that names one Plugin id twice,
/// whatever each entry's form, `optional` flag, and `config`. Returns the
/// refusal's message, naming the first id declared again.
pub(crate) fn check_distinct_plugins(plugins: &[PluginDecl]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    match plugins.iter().find(|decl| !seen.insert(decl.id())) {
        Some(decl) => Err(format!(
            "invalid frontmatter: Plugin {} is declared more than once under plugins",
            decl.id()
        )),
        None => Ok(()),
    }
}

/// Refuses a tool slot whose Plugin is declared optional: a slot
/// requires its Plugin, so an absent optional one would fail the run
/// anyway. Returns the refusal's message, naming the first offending alias
/// in sorted order.
pub(crate) fn check_slot_plugins(tools: &ToolSlots, plugins: &[PluginDecl]) -> Result<(), String> {
    for (alias, slot) in tools.iter() {
        let ToolSlot::Exact(tool) = slot;
        let plugin = tool.plugin();
        if plugins
            .iter()
            .any(|decl| decl.is_optional() && declares(decl, &plugin))
        {
            return Err(format!(
                "invalid frontmatter: tool alias '{alias}' names {tool}, whose Plugin \
                 {plugin} is declared optional; a tool slot requires its Plugin"
            ));
        }
    }
    Ok(())
}

/// Whether `decl` declares `plugin`. Both ids have exactly two
/// segments, so equal namespace and Plugin segments are equal ids.
fn declares(decl: &PluginDecl, plugin: &PluginId) -> bool {
    decl.id().namespace() == plugin.namespace() && decl.id().plugin() == plugin.name()
}
