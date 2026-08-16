//! The MCP tool surface: the fixed set of built-ins this server publishes.
//!
//! No prompt is published as a tool of its own. A PromptForge prompt is a
//! command, invoked because a caller named it, so the catalog sits behind
//! [`RUN_PROMPT`] rather than in `tools/list`, and nothing this server offers
//! can be selected for a task the caller did not ask for by name.
//!
//! The list is the same set of built-ins for every catalog, though not the same
//! count for every build: the listing tool, the runner, and the collector are
//! always published, and the resolver joins them only when the `picker` feature
//! is compiled in. A default build publishes four; a build without `picker`
//! publishes three. Each is one entry in the built-in definitions, carrying its
//! name, its description, its schema, and whether this build publishes it.
//!
//! [`BuiltInTool`] is the single source of truth. Its variants drive the names,
//! the published metadata ([`tool_definitions`]), the publication rule
//! ([`publishes_built_in`]), the reserved-name set ([`reserved_names`], which
//! catalog resolution reads), AND the dispatch handler in [`crate::server`], so
//! a name cannot be a tool in one place and a legal prompt name in another.
//! Adding a built-in is adding one enum variant; the compiler then refuses to
//! build until that variant gains a name, a description, a schema, and a
//! publication rule (all exhaustive `match`es over the enum here) plus a handler
//! arm (an exhaustive `match` over the enum in the server's dispatch). A
//! published tool with no handler, or a handler for a tool that publishes no
//! metadata, cannot compile, so metadata and dispatch cannot drift.
//!
//! The one publication rule that varies is a property of the build rather than
//! of the catalog or the request: [`NEED_PROMPT`] needs the `picker` feature,
//! which carries the embedding model that ranks candidates.

#[cfg(test)]
mod tests;

use std::sync::{Arc, LazyLock};

use rmcp::model::{JsonObject, Tool};
use serde_json::Value;

/// The name of the built-in that reports the whole catalog.
pub(crate) const LIST_PROMPTS: &str = "list_prompts";

/// The name of the built-in that runs any enabled prompt by name.
pub(crate) const RUN_PROMPT: &str = "run_prompt";

/// The name of the built-in that collects a run which outlived its call.
pub(crate) const CHECK_RUN: &str = "check_run";

/// The name of the built-in that resolves a described prompt to names.
pub(crate) const NEED_PROMPT: &str = "need_prompt";

/// The register instruction and the two examples that steer a caller of
/// [`NEED_PROMPT`] into author phrasing.
///
/// A description that restates a prompt's own documentation resolves to the
/// right prompt far more often than the same description phrased as a
/// conversational goal, and nothing about the ranking engine closes that gap -
/// only the wording does. So this text is carried twice, once in the tool's
/// description and once in the `capability` parameter's own, because a client
/// may surface either one alone. It is a macro rather than a constant so the
/// second copy is the same literal and cannot drift from the first.
macro_rules! capability_register {
    () => {
        "State the capability the way a tool author would document it: an imperative phrase naming the operation and what it acts on, with no entity names, task specifics, or conversational framing. Good: \"Build a stakeholder position report for one entity.\" Bad: \"I need to know what Herb Sutter has said about ABI stability.\""
    };
}

const CAPABILITY_REGISTER: &str = capability_register!();

/// What a prompt's returned value is, said wherever that value is published.
///
/// A caller that reads no such statement treats the value as source material
/// and paraphrases it, which throws away the artifact the prompt was asked for.
/// The sentence names what the value is and who it is for, and says nothing
/// about any client or any way of displaying a result, so it stays true whether
/// the caller is a terminal, an editor, a web application, or a script. It is a
/// macro so every copy is the same literal and none can drift.
macro_rules! prompt_value {
    () => {
        "A prompt's value is a finished artifact written for the user to read, so pass it through as it stands rather than restating it."
    };
}

pub(crate) use prompt_value;

const LIST_PROMPTS_DESCRIPTION: &str = "Names the PromptForge prompts this server can run. Each entry carries the prompt's name, its description, and any problem that currently stops it running. The listing is read from the catalog as it stands, so a prompt written or edited since this conversation began is already in it. run_prompt takes a name from this listing.";

const RUN_PROMPT_DESCRIPTION: &str = concat!(
    "Runs the named PromptForge prompt and returns what it produced. This server executes prompts the caller names; the prompt argument is that name, as list_prompts reports it. ",
    prompt_value!()
);

const CHECK_RUN_DESCRIPTION: &str = concat!(
    "Collects a PromptForge run that outlived the call which started it. Takes the run id from a result whose status was running, and reports that run's status now, with its value once it has finished. ",
    prompt_value!()
);

const NEED_PROMPT_DESCRIPTION: &str = concat!(
    "Resolves a described PromptForge prompt to the names of the closest prompts, up to three, best first. It is for a caller who was given a prompt by description rather than by name; it returns names and runs nothing, and run_prompt takes one of them. ",
    capability_register!()
);

/// Every tool this server publishes, in the order `tools/list` reports them.
///
/// No prompt name is ever among them: a prompt is reached by naming it to
/// [`RUN_PROMPT`], so the list is fixed for the life of the process and a
/// prompt saved a second ago is callable with no reconnect and no restart.
///
/// The schemas are built once, on first use, and reused (see
/// [`BUILT_IN_DEFINITIONS`]): they are immutable and identical on every call, so
/// rebuilding them per `tools/list` would be pure waste.
#[must_use]
pub(crate) fn tool_definitions() -> Vec<Tool> {
    BUILT_IN_DEFINITIONS
        .iter()
        .filter(|built_in| built_in.tool.published())
        .map(|built_in| {
            Tool::new(
                built_in.tool.name(),
                built_in.tool.description(),
                Arc::clone(&built_in.input_schema),
            )
        })
        .collect()
}

/// The built-in tools this server can answer, in `tools/list` order.
///
/// This enum is the single source of truth that binds a built-in's published
/// metadata to its dispatch handler. Both the listing here and the dispatcher
/// in [`crate::server`] key on it, and both match it exhaustively: adding a
/// variant does not compile until that variant has a [`name`](BuiltInTool::name),
/// a [`description`](BuiltInTool::description), a schema
/// ([`build_schema`](BuiltInTool::build_schema)), a
/// [`published`](BuiltInTool::published) rule, AND a handler arm in the server's
/// dispatch. A published tool with no handler cannot exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltInTool {
    /// Reports the whole catalog.
    ListPrompts,
    /// Runs any enabled prompt by name.
    RunPrompt,
    /// Resolves a described prompt to names; published only with `picker`.
    NeedPrompt,
    /// Collects a run that outlived its call.
    CheckRun,
}

impl BuiltInTool {
    /// Every built-in, in `tools/list` order, whether or not this build
    /// publishes it.
    pub(crate) const ALL: [BuiltInTool; 4] = [
        BuiltInTool::ListPrompts,
        BuiltInTool::RunPrompt,
        BuiltInTool::NeedPrompt,
        BuiltInTool::CheckRun,
    ];

    /// The wire name a caller reaches this built-in by.
    pub(crate) fn name(self) -> &'static str {
        match self {
            BuiltInTool::ListPrompts => LIST_PROMPTS,
            BuiltInTool::RunPrompt => RUN_PROMPT,
            BuiltInTool::NeedPrompt => NEED_PROMPT,
            BuiltInTool::CheckRun => CHECK_RUN,
        }
    }

    /// The description `tools/list` publishes for this built-in.
    fn description(self) -> &'static str {
        match self {
            BuiltInTool::ListPrompts => LIST_PROMPTS_DESCRIPTION,
            BuiltInTool::RunPrompt => RUN_PROMPT_DESCRIPTION,
            BuiltInTool::NeedPrompt => NEED_PROMPT_DESCRIPTION,
            BuiltInTool::CheckRun => CHECK_RUN_DESCRIPTION,
        }
    }

    /// Whether this build publishes the built-in.
    ///
    /// The only rule that varies is [`NeedPrompt`](BuiltInTool::NeedPrompt),
    /// which needs the `picker` feature that carries the ranking model.
    pub(crate) fn published(self) -> bool {
        match self {
            BuiltInTool::ListPrompts | BuiltInTool::RunPrompt | BuiltInTool::CheckRun => true,
            BuiltInTool::NeedPrompt => cfg!(feature = "picker"),
        }
    }

    /// The built-in whose wire name is `name`, whether or not this build
    /// publishes it, or `None` when no built-in owns the name.
    pub(crate) fn from_name(name: &str) -> Option<BuiltInTool> {
        BuiltInTool::ALL
            .into_iter()
            .find(|tool| tool.name() == name)
    }

    /// Builds this built-in's input schema, called once per built-in when
    /// [`BUILT_IN_DEFINITIONS`] is first used and shared by every reader after.
    fn build_schema(self) -> Arc<JsonObject> {
        match self {
            BuiltInTool::ListPrompts => schema(
                &[(
                    "cursor",
                    Some(
                        "A pagination cursor from a previous listing's next_cursor, to read the page after it. Omitting it reads the first page.",
                    ),
                )],
                &[],
            ),
            BuiltInTool::RunPrompt => schema(
                &[
                    (
                        "prompt",
                        Some("The exact name of the prompt to run, as list_prompts reports it."),
                    ),
                    (
                        "args",
                        Some(
                            "The prompt's input, as one raw string. Omitting it passes the empty string.",
                        ),
                    ),
                    (
                        "input_file",
                        Some("Filesystem path to read into the prompt's store. Mutually exclusive with input_text."),
                    ),
                    (
                        "input_text",
                        Some("Text to place directly in the prompt's store. Mutually exclusive with input_file."),
                    ),
                    (
                        "output_file",
                        Some("Filesystem path to write the prompt's output file to. Omit to return output inline."),
                    ),
                ],
                &["prompt"],
            ),
            BuiltInTool::NeedPrompt => schema(
                &[("capability", Some(CAPABILITY_REGISTER))],
                &["capability"],
            ),
            BuiltInTool::CheckRun => schema(
                &[(
                    "run_id",
                    Some("The run id from an earlier result whose status was running."),
                )],
                &["run_id"],
            ),
        }
    }
}

/// One built-in paired with its input schema.
struct BuiltIn {
    tool: BuiltInTool,
    input_schema: Arc<JsonObject>,
}

/// Every built-in with its schema, built once via [`LazyLock`] and shared by
/// every reader thereafter: the set is fixed for the life of the process, so
/// its schemas are allocated on first use rather than rebuilt per `tools/list`.
static BUILT_IN_DEFINITIONS: LazyLock<[BuiltIn; 4]> = LazyLock::new(|| {
    BuiltInTool::ALL.map(|tool| BuiltIn {
        tool,
        input_schema: tool.build_schema(),
    })
});

/// Whether `name` is a built-in this build publishes.
///
/// Dispatch itself routes through [`BuiltInTool::from_name`] and
/// [`BuiltInTool::published`] directly; this is the same test read the tests use
/// to assert that publication tracks the listing, so it is compiled under test.
#[cfg(test)]
pub(crate) fn publishes_built_in(name: &str) -> bool {
    BuiltInTool::from_name(name).is_some_and(BuiltInTool::published)
}

/// Every name a built-in owns, in `tools/list` order, whether or not this build
/// publishes it.
///
/// Derived from [`BuiltInTool`] rather than kept as a second list, so the
/// reserved set catalog resolution reads to keep a prompt from taking a tool's
/// name (see [`crate::catalog`]) cannot drift from the names listing and
/// dispatch are keyed on. Every variant contributes its name whether or not this
/// build publishes it, so `need_prompt` is reserved even in a build without the
/// `picker` feature: a name legal in one build and not another is worse than one
/// never legal.
pub(crate) fn reserved_names() -> impl Iterator<Item = &'static str> {
    BuiltInTool::ALL.into_iter().map(BuiltInTool::name)
}

/// An object schema whose every property is a string.
///
/// That covers the whole surface: a prompt's `args`, a prompt name, a run id,
/// and a capability are all strings, so no other property type is needed and
/// none is offered.
fn schema(properties: &[(&str, Option<&str>)], required: &[&str]) -> Arc<JsonObject> {
    let mut declared = JsonObject::new();
    for &(name, description) in properties {
        let mut property = JsonObject::new();
        property.insert("type".to_owned(), Value::String("string".to_owned()));
        if let Some(description) = description {
            property.insert(
                "description".to_owned(),
                Value::String(description.to_owned()),
            );
        }
        declared.insert(name.to_owned(), Value::Object(property));
    }

    let mut schema = JsonObject::new();
    schema.insert("type".to_owned(), Value::String("object".to_owned()));
    schema.insert("properties".to_owned(), Value::Object(declared));
    // A strict object: a misspelled or obsolete argument is a caller defect, so
    // the schema rejects any property it did not declare rather than letting an
    // unknown key pass validation and silently run with a default.
    schema.insert("additionalProperties".to_owned(), Value::Bool(false));
    if !required.is_empty() {
        schema.insert(
            "required".to_owned(),
            Value::Array(
                required
                    .iter()
                    .map(|name| Value::String((*name).to_owned()))
                    .collect(),
            ),
        );
    }
    Arc::new(schema)
}
