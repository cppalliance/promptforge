//! The run-scoped sets built from the prepared bindings, and the `argv`
//! derivation: the pieces `RunState::new` assembles once per run.

use std::collections::{BTreeMap, BTreeSet};

use promptforge_model_client::detail::tool_schema_new;
use promptforge_parser::ModelKeyword;
use promptforge_types::tools::ToolDescriptor;

use crate::execute::scheduler::RESERVED_TOOL_NAMES;
use crate::lua::{ToolBinding, ToolSet};
use crate::model::{ModelBinding, ModelInvocation, ModelSet};
use crate::parser::Prompt;

use super::super::config::RunContext;

/// Builds the run's shared tool set from the prepared bindings: every
/// filled slot becomes a binding with the tool's descriptor data (its
/// schema, description, and output kind), so run-time execution never
/// consults the catalog again and never holds an implementation. Unfilled
/// slots produce no binding: advertising or calling the alias fails at run
/// time, exactly as prepare's report promised. The offering is bound
/// beside the slots.
pub(super) fn bound_tool_set(prompt: &Prompt, ctx: &RunContext) -> ToolSet {
    let mut set = ToolSet::default();
    for (alias, _) in prompt.frontmatter().tools().iter() {
        let Some(tool) = ctx.tool_bindings.resolve(alias) else {
            continue;
        };
        // The exact path says nothing prose-like; the tool's own catalog
        // text stands in as the binding's description.
        set.bindings.push(ToolBinding::from_descriptor(alias, tool));
    }
    set.offered = offered_bindings(prompt, ctx);
    set
}

/// Binds the offering: every catalog tool whose Plugin the prompt's
/// frontmatter doesn't declare, in tool id order, under its id with `/`
/// and `.` replaced by `_` (`web/fetch` becomes `web_fetch`). A tool whose
/// name can't be offered is left out with a log line.
fn offered_bindings(prompt: &Prompt, ctx: &RunContext) -> Vec<ToolBinding> {
    let frontmatter = prompt.frontmatter();
    let declared = frontmatter.plugins();
    let aliases: BTreeSet<&str> = frontmatter.tools().iter().map(|(alias, _)| alias).collect();
    let mut tools: Vec<&ToolDescriptor> = ctx
        .tools
        .tools()
        .iter()
        .filter(|tool| !declared.contains(&tool.id.plugin()))
        .collect();
    tools.sort_by(|left, right| left.id.cmp(&right.id));
    let mut taken = BTreeSet::new();
    let mut offered = Vec::new();
    for tool in tools {
        let name = tool.id.to_string().replace(['/', '.'], "_");
        if let Some(reason) = offer_refusal(&name, tool, &aliases, &taken) {
            tracing::warn!(tool = %tool.id, %name, %reason, "a tool is left out of the offering");
            continue;
        }
        offered.push(ToolBinding::from_descriptor(&name, tool));
        taken.insert(name);
    }
    offered
}

/// Why `tool` can't be offered under `name`: a frontmatter tool alias or a
/// task built-in has the name, no model round could advertise it, or an
/// earlier offered tool has it. `None` when it can be offered.
fn offer_refusal(
    name: &str,
    tool: &ToolDescriptor,
    aliases: &BTreeSet<&str>,
    taken: &BTreeSet<String>,
) -> Option<String> {
    if aliases.contains(name) {
        return Some("a frontmatter tool alias has the same name".to_owned());
    }
    if RESERVED_TOOL_NAMES.contains(&name) {
        return Some("a task built-in has the same name".to_owned());
    }
    if let Err(error) = tool_schema_new(
        name,
        tool.description.as_str(),
        tool.parameters_schema.clone(),
    ) {
        return Some(error.to_string());
    }
    if taken.contains(name) {
        return Some("an earlier offered tool has the same name".to_owned());
    }
    None
}

/// Binds every tool in the prepared catalog under its full id, keyed by
/// that id: what a script `tools.call` falls back to when no frontmatter
/// alias matches. These bindings stay out of the tool set, so they never
/// become globals, never enter a section's scope, and are never
/// advertised.
pub(super) fn catalog_bindings(ctx: &RunContext) -> BTreeMap<String, ToolBinding> {
    ctx.tools
        .tools()
        .iter()
        .map(|tool| {
            let id = tool.id.to_string();
            let binding = ToolBinding::from_descriptor(&id, tool);
            (id, binding)
        })
        .collect()
}

/// Every tool and model alias the prompt's frontmatter declares, filled or
/// not: the names a Plugin prelude's globals must not take, so whether
/// a prelude installs depends only on the frontmatter.
pub(super) fn frontmatter_aliases(prompt: &Prompt) -> Vec<String> {
    let frontmatter = prompt.frontmatter();
    frontmatter
        .tools()
        .iter()
        .map(|(alias, _)| alias)
        .chain(frontmatter.models().iter().map(|(label, _)| label))
        .map(str::to_owned)
        .collect()
}

/// The keyword's stable kebab-case spelling, journaled onto the binding as
/// the role's capability set.
fn keyword_name(keyword: ModelKeyword) -> &'static str {
    match keyword {
        ModelKeyword::Thinking => "thinking",
        ModelKeyword::NoThinking => "no-thinking",
        ModelKeyword::Frontier => "frontier",
        ModelKeyword::Fast => "fast",
        ModelKeyword::Small => "small",
        ModelKeyword::Creative => "creative",
        ModelKeyword::Chat => "chat",
        // `ModelKeyword` is `#[non_exhaustive]`; an unlisted keyword
        // reports as unknown rather than breaking the fill.
        _ => "unknown",
    }
}

/// Builds the run's shared model set from the prepared bindings: every
/// filled role becomes a binding under its label, holding the role's
/// keyword set (the handle's `capabilities`) and the hard-keyword thinking
/// switch as the frozen invocation. Unfilled roles produce no binding:
/// `models.use` on the label fails at run time.
pub(super) fn bound_model_set(prompt: &Prompt, ctx: &RunContext) -> ModelSet {
    let mut set = ModelSet::default();
    for (label, role) in prompt.frontmatter().models().iter() {
        let Some(descriptor) = ctx.model_bindings.resolve(label) else {
            continue;
        };
        let mut thinking = None;
        for keyword in role.keywords() {
            match keyword {
                ModelKeyword::Thinking => thinking = Some(true),
                ModelKeyword::NoThinking => thinking = Some(false),
                _ => {}
            }
        }
        let binding = ModelBinding::new(
            label,
            role.description()
                .unwrap_or_else(|| descriptor.description()),
            descriptor.id().clone(),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking,
            },
            descriptor.context(),
        )
        .with_capabilities(
            role.keywords()
                .iter()
                .map(|keyword| keyword_name(*keyword))
                .map(str::to_owned)
                .collect(),
        );
        set.bindings.push(binding);
    }
    set
}

/// Derives a section's `argv` from the args string under the prompt's
/// declaration: a default-declared prompt wraps the interface prose into
/// the default shape (`argv.prose`, with the empty string present, not
/// absent); a structured declaration parses the string as JSON, and a parse
/// failure or a JSON `null` reads as nil (`if argv then` is the malformed
/// check). The executor never hard-errors on shape.
pub(super) fn derive_argv(prompt: &Prompt, args: &str) -> Option<serde_json::Value> {
    if prompt.frontmatter().args().is_default() {
        return Some(serde_json::json!({ "prose": args }));
    }
    match serde_json::from_str(args) {
        Ok(serde_json::Value::Null) | Err(_) => None,
        Ok(value) => Some(value),
    }
}
