//! Prepare's fill functions: catalog assembly from the activated
//! capabilities' contributions, tool slot filling against the assembled
//! catalog, and the trivial model fill.

use std::sync::Arc;

use promptforge_parser::{FuzzySlot, ModelKeyword, ToolSlot, ToolSlots};
use promptforge_tool_picker::{Catalog as PickerCatalog, Outcome, ToolDescriptor, ToolPicker};
use shared_promptforge_api::capabilities::{CapabilityId, Contribution};
use shared_promptforge_api::tools::Tool;

use crate::model::ThinkingMode;
use crate::parser::Prompt;
use crate::tools::ToolCatalog;

use super::bindings::{ModelBindings, ToolBindings};
use super::requirements::{RequirementCheck, Requirements, UnmetRequirement};

/// Assembles the run's tool catalog from the activated capabilities'
/// contributions in declaration order.
///
/// Containment is total and enforced here: every contributed tool's id
/// must sit under its contributing capability's full id
/// (`namespace/pack/name` for a `namespace/pack` capability). A
/// violating tool - like a repeated id or a transport-illegal wire
/// name - is rejected at assembly: logged and never admitted to the
/// catalog.
pub(super) fn assemble_catalog(activated: &[(CapabilityId, Contribution)]) -> ToolCatalog {
    let mut accepted: Vec<Arc<dyn Tool>> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (capability, contribution) in activated {
        for tool in &contribution.tools {
            let id = tool.id();
            if !capability.contains(&id) {
                tracing::warn!(
                    capability = %capability,
                    tool = %id,
                    "contributed tool id escapes its capability's id; rejected at assembly"
                );
                continue;
            }
            if !seen.insert(id.clone()) {
                tracing::warn!(
                    capability = %capability,
                    tool = %id,
                    "contributed tool id repeats an earlier contribution; rejected at assembly"
                );
                continue;
            }
            // The catalog is the transport boundary: validate the wire
            // name per tool so one bad tool costs only itself.
            if let Err(error) = ToolCatalog::new(std::slice::from_ref(tool)) {
                tracing::warn!(
                    capability = %capability,
                    tool = %id,
                    %error,
                    "contributed tool failed catalog validation; rejected at assembly"
                );
                continue;
            }
            accepted.push(Arc::clone(tool));
        }
    }
    match ToolCatalog::new(&accepted) {
        Ok(catalog) => catalog,
        Err(error) => {
            // Every accepted tool passed containment, uniqueness, and
            // wire-name validation above, so this build cannot fail;
            // the arm is defensive.
            tracing::warn!(%error, "catalog assembly failed after per-tool validation");
            ToolCatalog::default()
        }
    }
}

/// Fills the prompt's declared tool slots against the assembled catalog,
/// journaling every fill into the returned bindings.
///
/// Exact slots fill by identity: an exact path's first two segments name
/// its capability, so a slot whose capability is inactive (absent from
/// `activated`) lands in [`Requirements::missing_required`] and the run
/// fails until satisfied. A slot whose capability IS active but whose
/// tool is absent from the catalog - the contribution was rejected at
/// assembly, or the capability never contributed that name - is not a
/// missing capability: installing changes nothing. It is warned and
/// left unfilled, and advertising the unfilled alias fails at run time,
/// exactly like an unfillable required fuzzy slot.
/// Fuzzy slots fill through the picker rebuilt over the run's assembled
/// catalog - never the deployment catalog - so the fuzz can only resolve
/// to a tool an activated capability contributed. An unfillable optional
/// fuzzy slot skips with a log line; an unfillable required fuzzy slot
/// is warned and left unfilled, and advertising the unfilled alias fails
/// at run time.
///
/// After the fills, the bind-time conflict scan records near-duplicate
/// pairs among the filled tools symmetrically on the bindings, so the
/// scope check errors when both halves of a clash enter one
/// model-visible scope. Binding records, never fails.
pub(super) fn fill_tool_bindings(
    prompt: &Prompt,
    catalog: &ToolCatalog,
    activated: &[CapabilityId],
    picker: Option<&ToolPicker>,
    requirements: &mut Requirements,
) -> ToolBindings {
    let mut bindings = ToolBindings::default();
    let slots = prompt.frontmatter().tools();
    let run_picker = fuzzy_fill_picker(slots, catalog, picker);
    for (alias, slot) in slots.iter() {
        match slot {
            ToolSlot::Exact(id) => {
                if let Some(tool) = catalog.get(id) {
                    tracing::info!(alias, tool = %id, "tool slot filled");
                    bindings.bind(alias, tool);
                } else {
                    let capability = id.capability();
                    if activated.contains(&capability) {
                        // The capability is active but the tool is not in
                        // the catalog: the contribution was rejected at
                        // assembly or never made. Reporting the capability
                        // as missing would fail the run unsatisfiably -
                        // installing it changes nothing - so warn and
                        // leave the alias unbound instead.
                        tracing::warn!(
                            alias,
                            tool = %id,
                            capability = %capability,
                            "exact tool slot's capability is active but \
                             contributed no such tool; unfilled - \
                             advertising the alias fails at run time"
                        );
                    } else {
                        tracing::warn!(
                            alias,
                            tool = %id,
                            capability = %capability,
                            "exact tool slot's capability is inactive"
                        );
                        if !requirements.missing_required.contains(&capability) {
                            requirements.missing_required.push(capability);
                        }
                    }
                }
            }
            ToolSlot::Fuzzy(fuzzy) => {
                let filled = run_picker
                    .as_ref()
                    .and_then(|picker| fill_fuzzy_slot(alias, fuzzy, catalog, picker));
                match filled {
                    Some(tool) => {
                        tracing::info!(
                            alias,
                            want = fuzzy.want(),
                            tool = %tool.id(),
                            "fuzzy tool slot filled"
                        );
                        bindings.bind(alias, tool);
                    }
                    None if fuzzy.is_optional() => {
                        tracing::info!(
                            alias,
                            want = fuzzy.want(),
                            "optional fuzzy tool slot unfilled; skipped"
                        );
                    }
                    None => {
                        tracing::warn!(
                            alias,
                            want = fuzzy.want(),
                            "required fuzzy tool slot unfilled; \
                             advertising the alias fails at run time"
                        );
                    }
                }
            }
            // The open host-offered posture is deferred; a posture this
            // fill does not model leaves its alias unbound.
            _ => {
                tracing::warn!(alias, "tool slot has an unrecognized posture; unfilled");
            }
        }
    }
    record_near_duplicate_conflicts(&mut bindings, run_picker.as_ref().or(picker));
    bindings
}

/// The bind-time conflict scan: near-duplicate pairs among the filled
/// tools, recorded symmetrically on the bindings so the scope check
/// errors when both halves of a clash enter one model-visible scope.
/// The scan prefers the run's fuzzy-fill picker - indexed over exactly
/// the run's catalog - and falls back to the environment picker's stored
/// vectors when no fuzzy slot needed the re-index; a similarity is a
/// property of the tools' descriptions, so both indexes agree on a pair
/// they both carry. Binding records, never fails: an unanalyzable set
/// (no picker, or a filled tool the picker never indexed) logs and
/// records nothing.
fn record_near_duplicate_conflicts(bindings: &mut ToolBindings, picker: Option<&ToolPicker>) {
    let ids = bindings.bound_ids();
    if ids.len() < 2 {
        return;
    }
    let Some(picker) = picker else {
        return;
    };
    match picker.near_duplicates(&ids) {
        Ok(pairs) => {
            for pair in &pairs {
                bindings.record_conflict(
                    pair.first().id(),
                    pair.second().id(),
                    f64::from(pair.similarity()),
                );
            }
        }
        Err(error) => {
            tracing::warn!(%error, "the near-duplicate conflict scan failed; none recorded");
        }
    }
}

/// Builds the run's fuzzy-fill picker: the environment picker's loaded
/// model re-indexed over the assembled catalog, so a fuzzy slot resolves
/// only among tools the activated capabilities contributed. Returns
/// `None` - leaving every fuzzy slot unfilled - when no fuzzy slot is
/// declared (the re-index is skipped entirely), when the environment has
/// no picker, or when the re-index fails.
fn fuzzy_fill_picker(
    slots: &ToolSlots,
    catalog: &ToolCatalog,
    picker: Option<&ToolPicker>,
) -> Option<ToolPicker> {
    if !slots
        .iter()
        .any(|(_, slot)| matches!(slot, ToolSlot::Fuzzy(_)))
    {
        return None;
    }
    let Some(picker) = picker else {
        tracing::warn!(
            "fuzzy tool slots declared but the environment has no picker; they go unfilled"
        );
        return None;
    };
    let descriptors: Vec<ToolDescriptor> = catalog
        .tools()
        .iter()
        .map(|tool| ToolDescriptor::new(tool.id(), tool.description(), tool.parameters_schema()))
        .collect();
    match picker.rebuild(PickerCatalog::new(descriptors)) {
        Ok(rebuilt) => Some(rebuilt),
        Err(error) => {
            tracing::warn!(%error, "the run's fuzzy-fill picker failed to build; fuzzy slots go unfilled");
            None
        }
    }
}

/// Resolves one fuzzy slot through the run's picker and looks the picked
/// identity up in the assembled catalog (it must be there - the picker
/// indexed exactly those tools). A non-bind outcome or a failed query
/// fills nothing; the caller applies the slot's optionality.
fn fill_fuzzy_slot(
    alias: &str,
    fuzzy: &FuzzySlot,
    catalog: &ToolCatalog,
    picker: &ToolPicker,
) -> Option<Arc<dyn Tool>> {
    match picker.resolve(fuzzy.want()) {
        Ok(Outcome::Bind(descriptor)) => catalog.get(descriptor.id()),
        Ok(Outcome::Absent) => None,
        Ok(Outcome::Duplicate(candidates) | Outcome::Ambiguous(candidates)) => {
            let ids: Vec<String> = candidates
                .iter()
                .map(|tool| tool.id().to_string())
                .collect();
            tracing::warn!(
                alias,
                candidates = ?ids,
                "fuzzy tool slot matched several tools; unfilled"
            );
            None
        }
        Ok(_) => {
            tracing::warn!(
                alias,
                "the picker reported an unrecognized outcome; unfilled"
            );
            None
        }
        Err(error) => {
            tracing::warn!(alias, %error, "the fuzzy fill query failed; unfilled");
            None
        }
    }
}

/// v1's deliberately trivial fill: binds every declared role to the
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
                ModelKeyword::NoThinking if model.thinking() != ThinkingMode::Never => {
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
        // The vocabulary is closed today; a future mode reports as
        // unknown rather than breaking the report.
        _ => "unknown",
    }
}
