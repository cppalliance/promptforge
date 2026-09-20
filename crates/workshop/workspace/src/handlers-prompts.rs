//! The `/prompts/*` route handlers: parsing prompt text into the Run
//! window's contract DTO.
//!
//! The parse route is pure - no state, no disk, no clock: it turns the
//! posted markdown into the contract the Run window renders, or a `422`
//! envelope naming the parse failure kind and line. The parser's own
//! types are `Deserialize`-only, so the wire DTO is hand-written
//! `Serialize` structs built from [`Frontmatter`] accessors.

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};

use promptforge_api_runtime::parser::{
    ArgDecl, ArgsDecl, CapabilityDecl, FileDecl, Frontmatter, ModelKeyword, ModelRole, ParseError,
    ParseErrorKind, Prompt, ToolSlot,
};
use workshop_protocol::ErrorEnvelope;

use crate::workspace::Workspace;

/// The prompt routes, merged into the subsystem's router by the parent
/// module so they share its state type, deadline tier, and cross-site
/// guard. The handlers use no state: the parse is pure.
pub(super) fn routes() -> axum::Router<Workspace> {
    axum::Router::new().route("/prompts/contract", post(contract))
}

/// The JSON body of `POST /prompts/contract`.
#[derive(Debug, Deserialize)]
pub(crate) struct ContractRequest {
    /// The prompt's display name (the file's, when it came from one).
    name: String,
    /// The prompt markdown to parse.
    text: String,
}

/// The Run-window contract: everything the panel renders, built from
/// the parsed frontmatter.
#[derive(Debug, Serialize)]
pub(crate) struct ContractResponse {
    /// The prompt's identifier.
    name: String,
    /// The one-line description shown in listings.
    description: String,
    /// The declared promptforge engine major; `null` when absent.
    promptforge: Option<u32>,
    /// The declared tool-loop cap; `null` for the runtime default.
    max_tool_iterations: Option<u32>,
    /// The declared input file; `null` when absent.
    input: Option<FileDto>,
    /// The declared output file; `null` when absent.
    output: Option<FileDto>,
    /// The declared capabilities, in declaration order.
    capabilities: Vec<CapabilityDto>,
    /// The declared tool slots, sorted by alias.
    tools: Vec<ToolDto>,
    /// The typed args declaration.
    args: ArgsDto,
    /// The declared model roles, sorted by label.
    models: Vec<ModelDto>,
}

/// A declared input or output file.
#[derive(Debug, Serialize)]
pub(crate) struct FileDto {
    /// The store-internal path.
    path: String,
    /// The human-readable purpose.
    description: String,
}

impl From<&FileDecl> for FileDto {
    fn from(decl: &FileDecl) -> Self {
        Self {
            path: decl.path().to_owned(),
            description: decl.description().to_owned(),
        }
    }
}

/// A declared capability: its global id and optionality.
#[derive(Debug, Serialize)]
pub(crate) struct CapabilityDto {
    /// The capability's global id (`namespace/pack`).
    id: String,
    /// Whether an absent capability skips instead of failing.
    optional: bool,
}

impl From<&CapabilityDecl> for CapabilityDto {
    fn from(decl: &CapabilityDecl) -> Self {
        Self {
            id: decl.id().to_string(),
            optional: decl.is_optional(),
        }
    }
}

/// One tool slot, tagged by filling posture.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum ToolDto {
    /// An exact global tool path.
    Exact {
        /// The prompt-local alias.
        alias: String,
        /// The canonical `namespace/pack/name` path.
        path: String,
    },
}

impl ToolDto {
    /// Builds the DTO for the slot declared under `alias`, or `None` for
    /// a posture this wire format predates.
    fn new(alias: &str, slot: &ToolSlot) -> Option<Self> {
        match slot {
            ToolSlot::Exact(id) => Some(Self::Exact {
                alias: alias.to_owned(),
                path: id.to_string(),
            }),
            // The deferred open posture has no wire form yet.
            _ => None,
        }
    }
}

/// The typed args declaration.
#[derive(Debug, Serialize)]
pub(crate) struct ArgsDto {
    /// True when the declaration is the implicit default (no `args:` key).
    implicit: bool,
    /// The declared fields, sorted by name.
    fields: Vec<ArgDto>,
}

impl From<&ArgsDecl> for ArgsDto {
    fn from(decl: &ArgsDecl) -> Self {
        Self {
            implicit: decl.is_default(),
            fields: decl
                .iter()
                .map(|(name, arg)| ArgDto::new(name, arg))
                .collect(),
        }
    }
}

/// One declared arg.
#[derive(Debug, Serialize)]
pub(crate) struct ArgDto {
    /// The arg name.
    name: String,
    /// The declared type (`string`, `boolean`, `integer`, `number`).
    #[serde(rename = "type")]
    kind: String,
    /// Whether a call may omit the field.
    optional: bool,
    /// The declared default; `null` when absent.
    default: Option<serde_json::Value>,
    /// The human-readable description; `null` when absent.
    description: Option<String>,
}

impl ArgDto {
    /// Builds the DTO for the arg declared under `name`.
    fn new(name: &str, decl: &ArgDecl) -> Self {
        Self {
            name: name.to_owned(),
            kind: decl.kind().to_string(),
            optional: decl.is_optional(),
            // A declared default is validated against the declared type
            // at parse, so it is always a JSON-representable scalar.
            default: decl
                .default()
                .and_then(|value| serde_json::to_value(value).ok()),
            description: decl.description().map(str::to_owned),
        }
    }
}

/// One declared model role.
#[derive(Debug, Serialize)]
pub(crate) struct ModelDto {
    /// The prompt-local role label.
    label: String,
    /// The declared keywords (kebab-case wire vocabulary).
    keywords: Vec<&'static str>,
    /// The minimum context window in tokens; `null` when absent.
    min_context: Option<u32>,
    /// The role's prose description; `null` when absent.
    description: Option<String>,
}

impl ModelDto {
    /// Builds the DTO for the role declared under `label`.
    fn new(label: &str, role: &ModelRole) -> Self {
        Self {
            label: label.to_owned(),
            keywords: role.keywords().iter().copied().map(keyword_wire).collect(),
            min_context: role.min_context().map(std::num::NonZeroU32::get),
            description: role.description().map(str::to_owned),
        }
    }
}

/// The kebab-case wire form of a model keyword.
fn keyword_wire(keyword: ModelKeyword) -> &'static str {
    match keyword {
        ModelKeyword::Thinking => "thinking",
        ModelKeyword::NoThinking => "no-thinking",
        ModelKeyword::Frontier => "frontier",
        ModelKeyword::Fast => "fast",
        ModelKeyword::Small => "small",
        ModelKeyword::Creative => "creative",
        ModelKeyword::Chat => "chat",
        // A keyword added after this DTO predates its wire form.
        _ => "unknown",
    }
}

impl From<&Frontmatter> for ContractResponse {
    fn from(frontmatter: &Frontmatter) -> Self {
        Self {
            name: frontmatter.name().to_owned(),
            description: frontmatter.description().to_owned(),
            promptforge: frontmatter.promptforge(),
            max_tool_iterations: frontmatter
                .max_tool_iterations()
                .limit()
                .map(std::num::NonZeroU32::get),
            input: frontmatter.input().map(FileDto::from),
            output: frontmatter.output().map(FileDto::from),
            capabilities: frontmatter
                .capabilities()
                .iter()
                .map(CapabilityDto::from)
                .collect(),
            tools: frontmatter
                .tools()
                .iter()
                .filter_map(|(alias, slot)| ToolDto::new(alias, slot))
                .collect(),
            args: ArgsDto::from(frontmatter.args()),
            models: frontmatter
                .models()
                .iter()
                .map(|(label, role)| ModelDto::new(label, role))
                .collect(),
        }
    }
}

/// Parses the posted prompt text and answers the contract DTO, or a
/// `422` envelope when the text is not a valid prompt.
pub(crate) async fn contract(Json(body): Json<ContractRequest>) -> Response {
    // The contract needs the tree alone; the parse-time events are not
    // this route's to log.
    match Prompt::parse(&body.text, &body.name).0 {
        Ok(prompt) => (
            StatusCode::OK,
            Json(ContractResponse::from(prompt.frontmatter())),
        )
            .into_response(),
        Err(error) => parse_failure(&error),
    }
}

/// Renders a parse failure as the standard error envelope: the machine
/// code is `parse_<kind>`, and the message carries the `line N: ` prefix
/// when the parser located the failure.
fn parse_failure(error: &ParseError) -> Response {
    let code = match error.kind() {
        ParseErrorKind::Frontmatter => "parse_frontmatter",
        ParseErrorKind::Structure => "parse_structure",
        ParseErrorKind::Fence => "parse_fence",
        ParseErrorKind::List => "parse_list",
        ParseErrorKind::Lua => "parse_lua",
        // A kind added after this route predates its wire code.
        _ => "parse_error",
    };
    let message = match error.line() {
        Some(line) => format!("line {line}: {error}"),
        None => error.to_string(),
    };
    let envelope = ErrorEnvelope::new(message, code);
    // Serializing the envelope cannot fail: two strings only.
    let body =
        serde_json::to_string(&envelope).unwrap_or_else(|_| "prompt parse failed".to_owned());
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

#[cfg(test)]
#[path = "handlers-prompts-tests.rs"]
mod tests;
