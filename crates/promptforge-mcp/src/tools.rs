//! The MCP tool surface: what a resolved catalog publishes in `tools/list`.
//!
//! Two kinds of tool appear there. A prompt exposed as `tool` gets its own
//! entry, named and described from its frontmatter, so the calling model can
//! select it directly. The built-ins - the listing tool, the runner, the
//! collector, and the retrieval tool - are how the rest of the catalog is
//! reached without spending a context slot per prompt.
//!
//! Every built-in is one entry in [`built_in_tools`], carrying its name, its
//! description, its schema, and the rule that decides whether this catalog
//! publishes it. A fifth built-in is one more entry there and no edit anywhere
//! else.
//!
//! Publication rules, all of them a property of the catalog rather than of the
//! request: the listing tool, the runner, and the retrieval tool appear when at
//! least one prompt is exposed as `list`, since with nothing behind them they
//! would answer every call with an empty catalog. The collector appears
//! whenever anything is published at all, because a direct call can outlive its
//! reply deadline exactly as a listed one can. The retrieval tool additionally
//! needs the `picker` feature, which carries the embedding model that ranks
//! candidates.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool};
use serde_json::Value;

use crate::catalog::{Catalog, Entry};

/// The name of the built-in that reports the whole catalog.
pub const LIST_PROMPTS: &str = "list_prompts";

/// The name of the built-in that runs any enabled prompt by name.
pub const RUN_PROMPT: &str = "run_prompt";

/// The name of the built-in that collects a run which outlived its call.
pub const CHECK_RUN: &str = "check_run";

/// The name of the built-in that retrieves candidate prompts for a capability.
pub const NEED_PROMPT: &str = "need_prompt";

/// The register instruction and the two examples that steer a caller of
/// [`NEED_PROMPT`] into author phrasing.
///
/// A need that restates a prompt's own documentation retrieves the right prompt
/// far more often than the same need phrased as a conversational goal, and
/// nothing about the ranking engine closes that gap - only the wording does. So
/// this text is carried twice, once in the tool's description and once in the
/// `capability` parameter's own, because a client may surface either one alone.
/// It is a macro rather than a constant so the second copy is the same literal
/// and cannot drift from the first.
macro_rules! capability_register {
    () => {
        "State the capability the way a tool author would document it: an imperative phrase naming the operation and what it acts on, with no entity names, task specifics, or conversational framing. Good: \"Build a stakeholder position report for one entity.\" Bad: \"I need to know what Herb Sutter has said about ABI stability.\""
    };
}

const CAPABILITY_REGISTER: &str = capability_register!();

const LIST_PROMPTS_DESCRIPTION: &str = "List every PromptForge prompt this server can run. Each entry carries the prompt's name, its description, its version, whether it also has its own tool, and any problem that currently stops it running. The listing is live, so a prompt written or edited since this conversation began is already in it. Run one with run_prompt.";

const RUN_PROMPT_DESCRIPTION: &str = "Run a PromptForge prompt and return what it produced. If you are unsure of a prompt's name, call list_prompts first and use a name from it rather than guessing.";

const CHECK_RUN_DESCRIPTION: &str = "Collect a PromptForge run that outlived the call which started it. Pass the run id from a result whose status was running, and get that run's status now, with its value once it has finished.";

const NEED_PROMPT_DESCRIPTION: &str = concat!(
    "Find the PromptForge prompts closest to a capability you need, up to three of them, best first. ",
    capability_register!(),
    " It returns candidates and runs nothing; choose one and call run_prompt with the name it handed you."
);

/// Every tool the catalog publishes, in the order `tools/list` reports them:
/// the direct prompts in catalog order, then the built-ins.
///
/// A broken entry that is exposed as `tool` keeps its tool, described by the
/// problem that stops it running. Dropping it would remove the tool from the
/// list while every connected client still holds a cached copy of it, so the
/// call would arrive anyway and the model would have read nothing to warn it.
///
/// # Examples
/// ```
/// # use promptforge_mcp::{Catalog, Config, OnBroken, tool_definitions};
/// # let dir = tempfile::tempdir()?;
/// # std::fs::write(dir.path().join("echo.md"), "---\nname: echo\ndescription: Echo the input\nversion: 1\npromptforge: 1\n---\n\n## Main\n\n```lua\nreturn args\n```\n")?;
/// # let config = Config::from_toml_str(&format!("[server]\ntoken = \"t\"\n\n[gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"t\"\n\n[paths]\nprompts = '{}'\n\n[catalog]\ninclude = [\"*.md\"]\ndefault_expose = \"tool\"\n", dir.path().display()))?;
/// let catalog = Catalog::resolve(&config, OnBroken::Reject)?;
/// let published = tool_definitions(&catalog);
/// let names: Vec<&str> = published.iter().map(|t| t.name.as_ref()).collect();
/// assert_eq!(names, ["echo", "check_run"]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[must_use]
pub fn tool_definitions(catalog: &Catalog) -> Vec<Tool> {
    let mut tools: Vec<Tool> = catalog
        .entries()
        .iter()
        .filter(|entry| entry.is_direct())
        .map(prompt_tool)
        .collect();
    tools.extend(built_in_tools(Shape::of(catalog)));
    tools
}

/// The tool a directly exposed prompt publishes: its frontmatter name, its
/// frontmatter description, and the one-string argument schema every prompt
/// shares, since a run takes a single raw `args` string and nothing else.
fn prompt_tool(entry: &Entry) -> Tool {
    let description = match entry.problem() {
        Some(problem) => format!("unavailable: {problem}"),
        None => entry.description().to_owned(),
    };
    Tool::new(entry.name().to_owned(), description, args_schema())
}

/// What the catalog holds, which is all the publication rules depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shape {
    /// At least one prompt is reachable only through the built-ins.
    listed: bool,
    /// At least one prompt has its own tool.
    direct: bool,
}

impl Shape {
    fn of(catalog: &Catalog) -> Shape {
        Shape {
            listed: catalog.entries().iter().any(|entry| !entry.is_direct()),
            direct: catalog.entries().iter().any(Entry::is_direct),
        }
    }
}

/// The built-ins this catalog publishes, in `tools/list` order.
///
/// One entry per built-in, each pairing the definition with the rule that
/// publishes it, so adding a built-in is adding a row.
fn built_in_tools(shape: Shape) -> Vec<Tool> {
    let definitions = [
        (
            LIST_PROMPTS,
            LIST_PROMPTS_DESCRIPTION,
            schema(&[], &[]),
            shape.listed,
        ),
        (
            RUN_PROMPT,
            RUN_PROMPT_DESCRIPTION,
            schema(
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
            shape.listed,
        ),
        (
            NEED_PROMPT,
            NEED_PROMPT_DESCRIPTION,
            schema(
                &[("capability", Some(CAPABILITY_REGISTER))],
                &["capability"],
            ),
            shape.listed && cfg!(feature = "picker"),
        ),
        (
            CHECK_RUN,
            CHECK_RUN_DESCRIPTION,
            schema(
                &[(
                    "run_id",
                    Some("The run id from an earlier result whose status was running."),
                )],
                &["run_id"],
            ),
            shape.listed || shape.direct,
        ),
    ];

    definitions
        .into_iter()
        .filter(|&(_, _, _, published)| published)
        .map(|(name, description, input_schema, _)| Tool::new(name, description, input_schema))
        .collect()
}

/// The input schema every directly exposed prompt carries: one optional string
/// property named `args`, because [`promptforge_core::execute::run`] takes a
/// single raw argument string. A missing `args` is the empty string, so nothing
/// is required.
fn args_schema() -> Arc<JsonObject> {
    schema(&[("args", None)], &[])
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
