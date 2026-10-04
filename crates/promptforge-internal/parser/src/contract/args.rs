//! The `args` frontmatter key: typed arg declarations.
//!
//! The declaration advertises, documents, and derives the tool schema; it
//! does not enforce - enforcement belongs to the prompt's H1. There are no
//! freeform prompts: a prompt with no `args:` key gets the default
//! declaration of one optional string field named `prose`.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{ContractKeys, deserialize_contract_map};

/// The type of a declared prompt arg.
///
/// The `type` key of an arg declaration accepts only `string`, `boolean`,
/// `integer`, and `number`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ArgType {
    /// A string arg.
    String,
    /// A boolean arg.
    Boolean,
    /// An integer arg.
    Integer,
    /// A numeric arg.
    Number,
}

impl std::fmt::Display for ArgType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ArgType::String => "string",
            ArgType::Boolean => "boolean",
            ArgType::Integer => "integer",
            ArgType::Number => "number",
        })
    }
}

/// The declaration of one prompt arg: its type, whether it is optional, its
/// default, and its description.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ArgDecl {
    kind: ArgType,
    optional: bool,
    default: Option<serde_yaml_ng::Value>,
    description: Option<String>,
}

impl ArgDecl {
    /// Returns the declared type.
    #[must_use]
    pub fn kind(&self) -> ArgType {
        self.kind
    }

    /// Returns whether a call may omit this arg entirely.
    ///
    /// An omitted arg is absent from the call. An empty string is a supplied
    /// value.
    #[must_use]
    pub fn is_optional(&self) -> bool {
        self.optional
    }

    /// Returns the declared default, when present.
    #[must_use]
    pub fn default(&self) -> Option<&serde_yaml_ng::Value> {
        self.default.as_ref()
    }

    /// Returns the human-readable description, when present.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

/// The map form of an arg declaration (`type` is a Rust keyword).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArgDeclWire {
    /// The declared type.
    #[serde(rename = "type")]
    kind: ArgType,
    /// Whether a call may omit the field entirely.
    #[serde(default)]
    optional: bool,
    /// The declared default value.
    #[serde(default)]
    default: Option<serde_yaml_ng::Value>,
    /// The human-readable description.
    #[serde(default)]
    description: Option<String>,
}

/// Type sanity: a declared default must match the declared type.
fn default_matches(kind: ArgType, value: &serde_yaml_ng::Value) -> bool {
    match (kind, value) {
        (ArgType::String, serde_yaml_ng::Value::String(_))
        | (ArgType::Boolean, serde_yaml_ng::Value::Bool(_))
        | (ArgType::Number, serde_yaml_ng::Value::Number(_)) => true,
        (ArgType::Integer, serde_yaml_ng::Value::Number(n)) => n.is_i64() || n.is_u64(),
        _ => false,
    }
}

impl<'de> Deserialize<'de> for ArgDecl {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ArgDeclWire::deserialize(deserializer)?;
        if let Some(default) = &wire.default
            && !default_matches(wire.kind, default)
        {
            return Err(serde::de::Error::custom(format!(
                "the default does not match the declared type `{}`",
                wire.kind
            )));
        }
        Ok(ArgDecl {
            kind: wire.kind,
            optional: wire.optional,
            default: wire.default,
            description: wire.description,
        })
    }
}

/// The args a prompt declares: a map from each arg name to its declaration.
///
/// Every prompt has an args declaration. A prompt whose frontmatter omits
/// the `args` key gets the default declaration: one optional string arg
/// named `prose`. The input text of such a prompt arrives wrapped as
/// `argv = { prose = "<text>" }`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ArgsDecl {
    fields: BTreeMap<String, ArgDecl>,
    /// True only when the declaration is the implicit default (the `args:`
    /// key was absent): interface prose wraps into `argv.prose`. An
    /// explicit `args:` key - even one identical to the default shape - is
    /// a structured declaration and never wraps.
    implicit: bool,
}

impl ArgsDecl {
    /// Returns whether this is the default declaration, which a prompt gets
    /// when its frontmatter omits the `args` key.
    ///
    /// The input text of such a prompt arrives wrapped as `argv.prose`. This
    /// returns `false` for every explicit `args` key, even one that declares
    /// the same single `prose` arg.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.implicit
    }

    /// Returns the declaration of the arg named `name`, when present.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ArgDecl> {
        self.fields.get(name)
    }

    /// Iterates the declared args as `(name, declaration)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ArgDecl)> {
        self.fields.iter().map(|(name, decl)| (name.as_str(), decl))
    }

    /// Returns the number of declared args.
    #[must_use]
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Returns whether zero args are declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

impl Default for ArgsDecl {
    /// The default declaration: one optional string field named `prose`.
    fn default() -> Self {
        let mut fields = BTreeMap::new();
        fields.insert(
            "prose".to_owned(),
            ArgDecl {
                kind: ArgType::String,
                optional: true,
                default: None,
                description: Some("Freeform input for this prompt".to_owned()),
            },
        );
        ArgsDecl {
            fields,
            implicit: true,
        }
    }
}

impl<'de> Deserialize<'de> for ArgsDecl {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Arg names are `argv` fields, never globals, so a reserved name
        // such as the default declaration's `prose` is a valid arg name.
        let fields = deserialize_contract_map(
            deserializer,
            ContractKeys {
                map: "args",
                what: "arg name",
                deferred: None,
                installs_global: false,
            },
        )?;
        Ok(ArgsDecl {
            fields,
            implicit: false,
        })
    }
}
