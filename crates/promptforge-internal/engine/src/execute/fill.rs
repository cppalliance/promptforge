//! Prepare's fill functions: tool slot filling by identity against the
//! caller-supplied catalog, and the trivial model fill.

use promptforge_parser::{ModelKeyword, ToolSlot};

use crate::model::ThinkingMode;
use crate::parser::Prompt;
use crate::tools::ToolCatalog;

use super::bindings::{ModelBindings, ToolBindings};
use super::requirements::{RequirementCheck, Requirements, UnmetRequirement};

/// Fills the prompt's declared tool slots against the caller-supplied
/// catalog, journaling every fill into the returned bindings.
///
/// Exact slots fill by identity: an exact path's first segment names
/// its Plugin, so a slot whose Plugin contributed nothing to the
/// catalog - it was never activated - lands in
/// [`Requirements::missing_required`] and the run fails until satisfied. A
/// slot whose Plugin DID contribute to the catalog but not the named
/// tool - the contribution was rejected at assembly, or the Plugin
/// never offered that name - is not a missing Plugin: installing
/// changes nothing. It is warned and left unfilled, and advertising the
/// unfilled alias fails at run time.
pub(super) fn fill_tool_bindings(
    prompt: &Prompt,
    catalog: &ToolCatalog,
    requirements: &mut Requirements,
) -> ToolBindings {
    let mut bindings = ToolBindings::default();
    let slots = prompt.frontmatter().tools();
    for (alias, slot) in slots.iter() {
        // A posture this fill does not model leaves its alias unbound.
        let ToolSlot::Exact(id) = slot else {
            continue;
        };
        if let Some(tool) = catalog.get(id) {
            bindings.bind(alias, tool.clone());
            continue;
        }
        let plugin = id.plugin();
        let plugin_present = catalog.tools().iter().any(|tool| plugin.contains(&tool.id));
        // A Plugin that contributed to the catalog but not this tool
        // (the contribution was rejected at assembly or never made) is not
        // missing: reporting it would fail the run unsatisfiably, since
        // installing it changes nothing. The alias stays unbound instead,
        // and advertising it fails at run time with the alias named.
        if !plugin_present && !requirements.missing_required.contains(&plugin) {
            requirements.missing_required.push(plugin);
        }
    }
    bindings
}

/// The trivial model fill: binds every declared role to the
/// context's current model and checks each role's hard keywords and
/// context minimum against its descriptor, reporting required versus
/// actual into [`Requirements::unmet_requirements`]. With no current
/// model there is nothing to fill or check.
pub(super) fn fill_model_bindings(
    prompt: &Prompt,
    model: Option<&crate::model::ModelDescriptor>,
    requirements: &mut Requirements,
) -> ModelBindings {
    let mut bindings = ModelBindings::default();
    let Some(model) = model else {
        return bindings;
    };
    for (label, role) in prompt.frontmatter().models().iter() {
        if let Some(minimum) = role.min_context()
            && model.context() < minimum
        {
            requirements.unmet_requirements.push(UnmetRequirement {
                role: label.to_owned(),
                check: RequirementCheck::ContextMinimum,
                required: minimum.to_string(),
                actual: model.context().to_string(),
            });
        }
        for keyword in role.keywords() {
            // Soft keywords document author intent; only the hard
            // keywords have a descriptor property to check against.
            let failed = match keyword {
                ModelKeyword::Thinking if model.thinking() == ThinkingMode::Never => {
                    Some("thinking")
                }
                ModelKeyword::NoThinking if model.thinking() == ThinkingMode::Always => {
                    Some("no-thinking")
                }
                _ => None,
            };
            if let Some(required) = failed {
                requirements.unmet_requirements.push(UnmetRequirement {
                    role: label.to_owned(),
                    check: RequirementCheck::HardKeyword,
                    required: required.to_owned(),
                    actual: thinking_name(model.thinking()).to_owned(),
                });
            }
        }
        bindings.bind(label, model.clone());
    }
    bindings
}

/// The thinking capability as a stable word for required-versus-actual
/// reporting.
fn thinking_name(thinking: ThinkingMode) -> &'static str {
    match thinking {
        ThinkingMode::Never => "Never",
        ThinkingMode::Always => "Always",
        ThinkingMode::Switchable => "Switchable",
        // `ThinkingMode` is `#[non_exhaustive]`; an unlisted mode reports
        // as unknown rather than breaking the report.
        _ => "unknown",
    }
}
