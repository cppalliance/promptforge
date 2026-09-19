//! The run-scoped sets built from the prepared bindings, and the `argv`
//! derivation: the pieces `RunState::new` assembles once per run.

use promptforge_parser::ModelKeyword;

use crate::lua::{ToolBinding, ToolSet};
use crate::model::{ModelBinding, ModelInvocation, ModelSet};
use crate::parser::Prompt;

use super::super::config::RunContext;

/// Builds the run's shared tool set from the prepared bindings: every
/// filled slot becomes a binding carrying the tool's descriptor data (its
/// schema, description, and output kind), so run-time execution never
/// consults the catalog again and never holds an implementation. Unfilled
/// slots produce no binding: advertising or calling the alias fails at run
/// time, exactly as prepare's report promised.
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
    set
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
        // The vocabulary is closed today; a future keyword reports its
        // debug spelling rather than breaking the fill.
        _ => "unknown",
    }
}

/// Builds the run's shared model set from the prepared bindings: every
/// filled role becomes a binding under its label, carrying the role's
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
