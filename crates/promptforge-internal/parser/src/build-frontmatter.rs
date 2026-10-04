//! The [`Frontmatter`] model and its `max_tool_iterations` cap, and the
//! frontmatter splitting and version detection that read a prompt's source.

use crate::contract::{ArgsDecl, ModelRoles, PluginDecl, ToolSlots};
use crate::{Error, ParseErrorKind, Result};

/// A declared input or output file in a prompt's frontmatter.
///
/// The `path` is the file's name in the store, which the prompt reads or
/// writes. The `description` documents the file's purpose and also feeds MCP
/// schema generation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct FileDecl {
    /// The store-internal path (e.g. `"paper.md"`).
    path: String,
    /// Human-readable purpose of this file.
    description: String,
}

impl FileDecl {
    /// Returns the file's path in the store, such as `paper.md`.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the human-readable description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }
}

/// The parsed frontmatter of a prompt file.
///
/// Parsing accepts only the keys this schema defines. Any other key, such as
/// a misspelled or extra one, is a prompt authoring error and fails the parse.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Frontmatter {
    /// The prompt's identifier, supplied explicitly by a caller.
    pub(crate) name: String,
    /// One-line description shown in prompt listings and name retrieval.
    pub(crate) description: String,
    /// The promptforge Engine major this file targets. Its presence marks the
    /// file as a promptforge prompt; `None` means the file is not one. Optional.
    #[serde(default)]
    pub(crate) promptforge: Option<u32>,
    /// Maximum model round trips a section's tool-call loop may take.
    ///
    /// A non-optional [`MaxToolIterations`]: an absent value deserializes to
    /// [`MaxToolIterations::Default`] (the runtime applies its own cap) and any
    /// explicit value is a positive, bounded count. Zero is unrepresentable.
    #[serde(default)]
    pub(crate) max_tool_iterations: MaxToolIterations,
    /// A file the prompt expects to find in the store when it starts.
    #[serde(default)]
    input: Option<FileDecl>,
    /// A file the prompt will leave in the store when it finishes.
    #[serde(default)]
    output: Option<FileDecl>,
    /// Plugins the prompt activates at prepare, in declaration order.
    #[serde(default)]
    plugins: Vec<PluginDecl>,
    /// Declared tool slots: alias to exact path.
    #[serde(default)]
    tools: ToolSlots,
    /// The typed args declaration; an absent `args:` key yields the default
    /// declaration (one optional string field named `prose`).
    #[serde(default)]
    args: ArgsDecl,
    /// Declared model roles: label to keywords, minimum, and description.
    #[serde(default)]
    models: ModelRoles,
}

/// The largest explicit `max_tool_iterations` a prompt may declare.
pub const MAX_TOOL_ITERATIONS: u32 = 1000;

/// A frontmatter tool-loop cap: absent or a positive, bounded count.
///
/// An omitted cap leaves the runtime's own default in force. Deserialization
/// rejects `0`, negatives, and values above [`MAX_TOOL_ITERATIONS`], so no
/// invalid cap can reach execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum MaxToolIterations {
    /// No explicit cap; the runtime applies its own default.
    #[default]
    Default,
    /// An explicit, positive, bounded cap.
    Limit(std::num::NonZeroU32),
}

impl MaxToolIterations {
    /// Resolves to a concrete iteration cap, using `default` when none was set.
    #[must_use]
    pub fn resolve(self, default: usize) -> usize {
        match self {
            MaxToolIterations::Default => default,
            MaxToolIterations::Limit(limit) => limit.get() as usize,
        }
    }

    /// Returns the explicit limit when one was declared.
    #[must_use]
    pub fn limit(self) -> Option<std::num::NonZeroU32> {
        match self {
            MaxToolIterations::Default => None,
            MaxToolIterations::Limit(limit) => Some(limit),
        }
    }
}

impl<'de> serde::Deserialize<'de> for MaxToolIterations {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize as i64 so negatives and > u32 values are caught, not wrapped.
        let raw = i64::deserialize(deserializer)?;
        if raw <= 0 {
            return Err(serde::de::Error::custom(format!(
                "max_tool_iterations must be a positive integer (>= 1), got {raw}"
            )));
        }
        if raw > i64::from(MAX_TOOL_ITERATIONS) {
            return Err(serde::de::Error::custom(format!(
                "max_tool_iterations must be <= {MAX_TOOL_ITERATIONS}, got {raw}"
            )));
        }
        // 1 <= raw <= MAX_TOOL_ITERATIONS, so both conversions are infallible.
        let value = u32::try_from(raw)
            .map_err(|_| serde::de::Error::custom("max_tool_iterations is out of range"))?;
        let limit = std::num::NonZeroU32::new(value)
            .ok_or_else(|| serde::de::Error::custom("max_tool_iterations must be non-zero"))?;
        Ok(MaxToolIterations::Limit(limit))
    }
}

impl Frontmatter {
    /// Returns the prompt's identifier, read from the `name:` key.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the one-line description shown in listings.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the Engine major version that the `promptforge:` key declares,
    /// when present.
    #[must_use]
    pub fn promptforge(&self) -> Option<u32> {
        self.promptforge
    }

    /// Returns the declared cap on model round trips in each section's
    /// tool-call loop. Returns `None` when the prompt omits the cap, so the
    /// caller's default applies.
    #[must_use]
    pub fn max_tool_iterations(&self) -> Option<std::num::NonZeroU32> {
        self.max_tool_iterations.limit()
    }

    /// Returns the declared input file, when present. The prompt expects to
    /// find this file in the store when it starts.
    #[must_use]
    pub fn input(&self) -> Option<&FileDecl> {
        self.input.as_ref()
    }

    /// Returns the declared output file, when present. The prompt leaves this
    /// file in the store when it finishes.
    #[must_use]
    pub fn output(&self) -> Option<&FileDecl> {
        self.output.as_ref()
    }

    /// Returns the declared Plugins, in declaration order.
    #[must_use]
    pub fn plugins(&self) -> &[PluginDecl] {
        &self.plugins
    }

    /// Returns the declared tool slots, which map each alias to an exact tool
    /// path.
    #[must_use]
    pub fn tools(&self) -> &ToolSlots {
        &self.tools
    }

    /// Returns the typed args declaration. A prompt that omits the `args:` key
    /// yields the default declaration (one optional string field named
    /// `prose`).
    #[must_use]
    pub fn args(&self) -> &ArgsDecl {
        &self.args
    }

    /// Returns the declared model roles, which map each label to a role.
    #[must_use]
    pub fn models(&self) -> &ModelRoles {
        &self.models
    }
}

/// Splits a file into its YAML frontmatter, its markdown body, and the
/// number of lines consumed by the frontmatter block (both `---` delimiters
/// and everything between them).
///
/// The file must open with a `---` line and close the frontmatter with another
/// `---` line starting at column 0; an indented `---` is YAML block scalar
/// text. `str::lines` handles both `\n` and `\r\n`.
pub(crate) fn split_frontmatter(input: &str) -> Result<(String, String, u32)> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input); // drop BOM
    let mut lines = input.lines();
    match lines.next() {
        Some(l) if l.trim() == "---" => {}
        _ => {
            return Err(Error::parse(
                ParseErrorKind::Frontmatter,
                "file must begin with a --- frontmatter delimiter",
            ));
        }
    }
    let mut yaml = String::new();
    let mut closed = false;
    let mut line_count: u32 = 1; // opening ---
    for line in lines.by_ref() {
        line_count += 1;
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if !closed {
        return Err(Error::parse(
            ParseErrorKind::Frontmatter,
            "frontmatter was not closed with ---",
        ));
    }
    let body = lines.collect::<Vec<_>>().join("\n");
    Ok((yaml, body, line_count))
}

/// Reports the promptforge Engine major declared in `source`'s frontmatter.
///
/// Returns `Some(major)` when `source` opens with a YAML frontmatter block that
/// declares a `promptforge:` key, and `None` otherwise. The check is lenient by
/// design and never errors or panics: a source with no frontmatter block,
/// malformed or unclosed frontmatter, or a frontmatter that simply omits the
/// key all read as `None` ("not a promptforge prompt"). No other frontmatter
/// field is required for detection.
#[must_use]
pub fn promptforge_version(source: &str) -> Option<u32> {
    /// Reads only the `promptforge` key, ignoring every other field so
    /// detection does not depend on a complete, valid [`Frontmatter`].
    #[derive(serde::Deserialize)]
    struct Probe {
        #[serde(default)]
        promptforge: Option<u32>,
    }

    let (yaml, _body, _lines) = split_frontmatter(source).ok()?;
    let probe: Probe = serde_yaml_ng::from_str(&yaml).ok()?;
    probe.promptforge
}
