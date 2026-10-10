//! Prepare's fill function: the trivial model fill.

use promptforge_parser::ModelKeyword;

use crate::model::ThinkingMode;
use crate::parser::Prompt;

use super::bindings::ModelBindings;
use super::requirements::{RequirementCheck, Requirements, UnmetRequirement};

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
