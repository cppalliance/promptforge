//! The frontmatter contract keys: `capabilities`, `tools`, `args`, `models`.
//!
//! The YAML is the whole contract: capabilities install, tools bind, models
//! declare, args type. Parsing validates the static shape - capability id
//! arity, each capability declared once, the alias grammar on slot keys,
//! the reserved names no tool alias or model role label may take, no tool
//! slot backed by an optional capability, the closed model-keyword
//! vocabulary, arg name and type sanity - and exposes the FULL declaration
//! on the parsed [`Prompt`](crate::Prompt); satisfying the declaration
//! against the host environment is prepare's job, never the parser's.
//!
//! `args` and `models` are defined in submodules; this root owns the
//! capability and tool-slot shapes plus the map deserializer all four keys
//! share.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use promptforge_types::capabilities::CapabilityId;
use promptforge_types::names::GlobalName;
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
pub(crate) struct ContractKeys {
    /// The frontmatter key the map sits under (`tools`), for error
    /// messages.
    pub(crate) map: &'static str,
    /// The key kind (`tool alias`), for error messages.
    pub(crate) what: &'static str,
    /// A key that satisfies the grammar but is rejected because its
    /// posture is deferred (the open toolset's `open`).
    pub(crate) deferred: Option<&'static str>,
    /// Whether each key installs as a section VM global of its own name,
    /// so a reserved name ([`promptforge_lua::RESERVED_NAMES`]) is refused.
    pub(crate) installs_global: bool,
}

/// Deserializes a contract map (`tools`, `models`, `args`): string keys
/// validated against the alias grammar and `keys`, values deserialized as
/// `T`, duplicates rejected.
pub(crate) fn deserialize_contract_map<'de, D, T>(
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

/// Parses a capability id: a [`GlobalName`] of exactly two segments
/// (`namespace/pack`).
///
/// The grammar accepts two or three segments, so the arity check counts
/// separators: exactly one `/` is two segments. A `@` version pin never
/// gets that far - the charset rejects it (v1 is unversioned).
fn parse_capability_id(text: &str) -> Result<GlobalName, String> {
    let name = GlobalName::parse(text)
        .map_err(|error| format!("invalid capability id `{text}`: {error}"))?;
    if text.matches('/').count() != 1 {
        return Err(format!(
            "invalid capability id `{text}`: a capability id has exactly 2 segments (namespace/pack)"
        ));
    }
    Ok(name)
}

/// A capability declaration: a plain id string (a required capability) or a
/// `ref` map holding the `optional` flag and prompt-side `config` data.
///
/// User-specific configuration (credentials, server lists) is host-supplied
/// through the run services and never named in the prompt; `config` is
/// prompt-side data only.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CapabilityDecl {
    /// The capability's global id (`namespace/pack`, exactly 2 segments).
    id: GlobalName,
    /// Whether an absent capability skips with a log line instead of
    /// failing preparation.
    optional: bool,
    /// Prompt-side configuration data, when declared.
    config: Option<serde_yaml_ng::Value>,
}

impl CapabilityDecl {
    /// Returns the capability's global id (`namespace/pack`).
    #[must_use]
    pub fn id(&self) -> &GlobalName {
        &self.id
    }

    /// Returns whether the capability is optional (skip-and-log when
    /// absent).
    #[must_use]
    pub fn is_optional(&self) -> bool {
        self.optional
    }

    /// Returns the prompt-side configuration data, when declared.
    #[must_use]
    pub fn config(&self) -> Option<&serde_yaml_ng::Value> {
        self.config.as_ref()
    }
}

impl<'de> Deserialize<'de> for CapabilityDecl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(CapabilityDeclVisitor)
    }
}

/// Deserializes a capability declaration from either frontmatter form: a
/// bare id string or a `ref` map. A streaming visitor (not an untagged
/// buffer) so rejections keep their source position.
struct CapabilityDeclVisitor;

impl<'de> Visitor<'de> for CapabilityDeclVisitor {
    type Value = CapabilityDecl;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a capability id string or a map with `ref`, `optional`, and `config`")
    }

    fn visit_str<E>(self, text: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(CapabilityDecl {
            id: parse_capability_id(text).map_err(E::custom)?,
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
        Ok(CapabilityDecl {
            id: parse_capability_id(&reference).map_err(de::Error::custom)?,
            optional: optional.unwrap_or(false),
            config,
        })
    }
}

/// One tool slot's filling posture: an exact global path filled by identity
/// against the assembled catalog (every fill is journaled).
///
/// `#[non_exhaustive]`: the open host-offered posture is deferred and joins
/// this enum when it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolSlot {
    /// An exact global tool path, filled by identity against the catalog.
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

/// The prompt's declared tool slots: alias to slot.
///
/// Aliases are prompt-local (the alias grammar); the model only ever sees
/// the alias, never the global path. The reserved `open` key (the deferred
/// open toolset posture) is rejected at parse, so a prompt cannot silently
/// half-declare the posture. Each alias installs as a section VM global,
/// so an alias that names a host global, a Lua standard-library global the
/// sandbox keeps, or a Lua keyword is rejected too, as is an alias that is
/// also a model role label.
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

    /// Returns whether no slots are declared.
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

/// Refuses a `capabilities:` list that names one capability id twice,
/// whatever each entry's form, `optional` flag, and `config`. Returns the
/// refusal's message, naming the first id declared again.
pub(crate) fn check_distinct_capabilities(capabilities: &[CapabilityDecl]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    match capabilities.iter().find(|decl| !seen.insert(decl.id())) {
        Some(decl) => Err(format!(
            "invalid frontmatter: capability {} is declared more than once under capabilities",
            decl.id()
        )),
        None => Ok(()),
    }
}

/// Refuses a tool slot whose capability is declared optional: a slot
/// requires its capability, so an absent optional one would fail the run
/// anyway. Returns the refusal's message, naming the first offending alias
/// in sorted order.
pub(crate) fn check_slot_capabilities(
    tools: &ToolSlots,
    capabilities: &[CapabilityDecl],
) -> Result<(), String> {
    for (alias, slot) in tools.iter() {
        let ToolSlot::Exact(tool) = slot;
        let capability = tool.capability();
        if capabilities
            .iter()
            .any(|decl| decl.is_optional() && declares(decl, &capability))
        {
            return Err(format!(
                "invalid frontmatter: tool alias '{alias}' names {tool}, whose capability \
                 {capability} is declared optional; a tool slot requires its capability"
            ));
        }
    }
    Ok(())
}

/// Whether `decl` declares `capability`. Both ids have exactly two
/// segments, so equal namespace and pack segments are equal ids.
fn declares(decl: &CapabilityDecl, capability: &CapabilityId) -> bool {
    decl.id().namespace() == capability.namespace() && decl.id().pack() == capability.pack()
}
