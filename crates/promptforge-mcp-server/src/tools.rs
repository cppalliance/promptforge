//! The MCP tool surface: the fixed set of built-ins this server publishes.
//!
//! No prompt is published as a tool of its own. A PromptForge prompt is a
//! command, invoked because a caller named it, so the catalog sits behind
//! [`RUN_PROMPT`] rather than in `tools/list`, and nothing this server offers
//! can be selected for a task the caller did not ask for by name.
//!
//! The list is therefore the same four entries for every catalog: the listing
//! tool, the runner, the resolver, and the collector. Each is one entry in
//! [`built_in_definitions`], carrying its name, its description, its schema,
//! and whether this build publishes it. A fifth built-in is one more entry
//! there and no edit anywhere else.
//!
//! The one publication rule left is a property of the build rather than of the
//! catalog or the request: [`NEED_PROMPT`] needs the `picker` feature, which
//! carries the embedding model that ranks candidates.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool};
use serde_json::Value;

/// The name of the built-in that reports the whole catalog.
pub const LIST_PROMPTS: &str = "list_prompts";

/// The name of the built-in that runs any enabled prompt by name.
pub const RUN_PROMPT: &str = "run_prompt";

/// The name of the built-in that collects a run which outlived its call.
pub const CHECK_RUN: &str = "check_run";

/// The name of the built-in that resolves a described prompt to names.
pub const NEED_PROMPT: &str = "need_prompt";

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

const LIST_PROMPTS_DESCRIPTION: &str = "Names the PromptForge prompts this server can run. Each entry carries the prompt's name, its description, its version, and any problem that currently stops it running. The listing is read from the catalog as it stands, so a prompt written or edited since this conversation began is already in it. run_prompt takes a name from this listing.";

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
/// # Examples
/// ```
/// # use promptforge_mcp_server::tool_definitions;
/// let published = tool_definitions();
/// let names: Vec<&str> = published.iter().map(|t| t.name.as_ref()).collect();
/// assert!(names.contains(&"run_prompt"));
/// ```
#[must_use]
pub fn tool_definitions() -> Vec<Tool> {
    built_in_definitions()
        .into_iter()
        .filter(|built_in| built_in.published)
        .map(|built_in| Tool::new(built_in.name, built_in.description, built_in.input_schema))
        .collect()
}

/// One built-in: how it is published, and whether this build publishes it.
struct BuiltIn {
    name: &'static str,
    description: &'static str,
    input_schema: Arc<JsonObject>,
    published: bool,
}

/// Every built-in, in `tools/list` order, each paired with the rule that
/// publishes it, so adding a built-in is adding a row.
///
/// This is the single statement of what the server offers. Both the listing and
/// the dispatcher read it, so a built-in absent from `tools/list` is one the
/// handler refuses as well, and the two cannot drift into a tool that answers a
/// call it never advertised.
fn built_in_definitions() -> [BuiltIn; 4] {
    [
        BuiltIn {
            name: LIST_PROMPTS,
            description: LIST_PROMPTS_DESCRIPTION,
            input_schema: schema(&[], &[]),
            published: true,
        },
        BuiltIn {
            name: RUN_PROMPT,
            description: RUN_PROMPT_DESCRIPTION,
            input_schema: schema(
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
                ],
                &["prompt"],
            ),
            published: true,
        },
        BuiltIn {
            name: NEED_PROMPT,
            description: NEED_PROMPT_DESCRIPTION,
            input_schema: schema(
                &[("capability", Some(CAPABILITY_REGISTER))],
                &["capability"],
            ),
            published: cfg!(feature = "picker"),
        },
        BuiltIn {
            name: CHECK_RUN,
            description: CHECK_RUN_DESCRIPTION,
            input_schema: schema(
                &[(
                    "run_id",
                    Some("The run id from an earlier result whose status was running."),
                )],
                &["run_id"],
            ),
            published: true,
        },
    ]
}

/// Whether `name` is a built-in this build publishes.
///
/// The dispatcher asks before it answers a built-in, so a name this build
/// leaves out of `tools/list` - `need_prompt` without the `picker` feature - is
/// a method that does not exist rather than one the handler answers anyway.
pub(crate) fn publishes_built_in(name: &str) -> bool {
    built_in_definitions()
        .iter()
        .any(|built_in| built_in.name == name && built_in.published)
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
