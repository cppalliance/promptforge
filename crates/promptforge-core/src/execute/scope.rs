//! Tool-scope analysis, near-duplicate validation, and schema/dispatch
//! preparation for the model-visible tool set.

use std::collections::{BTreeMap, BTreeSet};

use promptforge_tool_picker::{ToolId as PickerToolId, ToolPicker};

use crate::client::ToolSchema;
use crate::lua::{ToolBinding, ToolBindings};
use crate::observe::{Observer, detail};
use crate::tools::ToolId;
use crate::{Error, NearDuplicateDiagnostic, Result};

/// One near-duplicate pair copied out of the picker's borrowing result.
///
/// The picker's [`promptforge_tool_picker::NearDuplicate`] borrows the picker,
/// but [`ToolAnalysis`] outlives one resolution and is cloned into fanout and
/// execute closures, so the pair's diagnostic values are copied out here.
#[derive(Debug, Clone)]
pub(crate) struct OwnedNearDuplicate {
    pub(crate) first_id: ToolId,
    pub(crate) second_id: ToolId,
    pub(crate) similarity: f32,
}

/// Frozen prompt-level tool identity maps plus the picker's near-duplicate
/// pairs, cloned into every section/fanout/execute closure that validates a
/// model-visible scope.
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolAnalysis {
    pub(crate) alias_to_id: BTreeMap<String, ToolId>,
    pub(crate) id_to_alias: BTreeMap<ToolId, String>,
    pub(crate) near_duplicates: Vec<OwnedNearDuplicate>,
}

/// Returns the alias frozen for a selected tool identity.
fn frozen_alias(analysis: &ToolAnalysis, id: &ToolId) -> Result<String> {
    analysis
        .id_to_alias
        .get(id)
        .cloned()
        .ok_or_else(|| Error::ToolScopeAnalysis {
            detail: "selected identity has no frozen alias".to_owned(),
        })
}

impl ToolAnalysis {
    pub(crate) fn new(bindings: &ToolBindings, picker: &ToolPicker) -> Result<Self> {
        let alias_to_id = bindings
            .bindings()
            .iter()
            .map(|binding| (binding.alias().to_owned(), binding.id().clone()))
            .collect();
        let id_to_alias = bindings
            .bindings()
            .iter()
            .map(|binding| (binding.id().clone(), binding.alias().to_owned()))
            .collect();
        let ids = bindings
            .bindings()
            .iter()
            .map(|binding| PickerToolId::new(binding.id().server(), binding.id().name()))
            .collect::<Vec<_>>();
        let near_duplicates = picker
            .near_duplicates(&ids)
            .map_err(|error| Error::ToolScopeAnalysisSource {
                source: Box::new(error),
            })?
            .iter()
            .map(|pair| OwnedNearDuplicate {
                first_id: ToolId::from_validated(
                    pair.first().id().server(),
                    pair.first().id().name(),
                ),
                second_id: ToolId::from_validated(
                    pair.second().id().server(),
                    pair.second().id().name(),
                ),
                similarity: pair.similarity(),
            })
            .collect();
        Ok(Self {
            alias_to_id,
            id_to_alias,
            near_duplicates,
        })
    }
}

/// How the tool loop reaches the tool behind one in-scope alias.
///
/// Produced by [`prepare_scoped_tools`]: bound tools carry their binding
/// (whose attached implementation is the dispatch target); local tools are
/// prompt-author Lua functions with no live implementation, marked here so
/// the loop routes them back into the section VM instead.
#[derive(Debug, Clone)]
pub(crate) enum DispatchTarget {
    /// A bound live tool, called through the binding's attached implementation.
    Bound(ToolBinding),
    /// A Lua-local tool, dispatched through the section's local dispatcher.
    Local,
}

pub(crate) fn prepare_effective_scope(
    analysis: &ToolAnalysis,
    bindings: &[ToolBinding],
    local_schemas: &[ToolSchema],
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<(Vec<ToolSchema>, BTreeMap<String, DispatchTarget>)> {
    observer.observe(execution, section, detail::TOOL_SCOPE_VALIDATION_STARTED);
    let result = validate_effective_scope_inner(analysis, bindings)
        .and_then(|()| prepare_scoped_tools(bindings, local_schemas));
    observer.observe(
        execution,
        section,
        if result.is_ok() {
            detail::TOOL_SCOPE_VALIDATION_SUCCEEDED
        } else {
            detail::TOOL_SCOPE_VALIDATION_FAILED
        },
    );
    result
}

pub(crate) fn validate_effective_scope_inner(
    analysis: &ToolAnalysis,
    bindings: &[ToolBinding],
) -> Result<()> {
    let effective = bindings
        .iter()
        .map(crate::lua::ToolBinding::id)
        .collect::<BTreeSet<_>>();
    for pair in &analysis.near_duplicates {
        if !effective.contains(&pair.first_id) || !effective.contains(&pair.second_id) {
            continue;
        }
        let first_alias = frozen_alias(analysis, &pair.first_id)?;
        let second_alias = frozen_alias(analysis, &pair.second_id)?;
        return Err(Error::NearDuplicateTools {
            diagnostic: Box::new(NearDuplicateDiagnostic {
                first_alias,
                first_id: pair.first_id.clone(),
                second_alias,
                second_id: pair.second_id.clone(),
                similarity: pair.similarity,
            }),
        });
    }
    Ok(())
}

pub(crate) fn prepare_scoped_tools(
    bindings: &[ToolBinding],
    local_schemas: &[ToolSchema],
) -> Result<(Vec<ToolSchema>, BTreeMap<String, DispatchTarget>)> {
    let mut schemas = Vec::with_capacity(bindings.len() + local_schemas.len());
    let mut dispatch = BTreeMap::new();
    for binding in bindings {
        // Model-facing description precedence: `tools.add` override >
        // `tools.bind`/`tools.always` override > the bound tool's catalog
        // text. The first two layers are already folded together by
        // `binding_for_scope` (the H2 add runtime overwrites the frozen
        // binding's `model_description`); the catalog fallback reads the
        // implementation attached at bind time.
        let description = binding
            .model_description()
            .unwrap_or_else(|| binding.tool().description())
            .to_owned();
        // F7: build every advertised schema through the validated constructor,
        // so an unusable wire name or a non-object JSON Schema is refused here
        // rather than sent to the model.
        let schema = ToolSchema::new(
            binding.alias().to_owned(),
            description,
            binding.tool().parameters_schema(),
        )
        .map_err(|error| Error::BindSchema {
            alias: binding.alias().to_owned(),
            source: Box::new(error),
        })?;
        schemas.push(schema);
        dispatch.insert(
            binding.alias().to_owned(),
            DispatchTarget::Bound(binding.clone()),
        );
    }
    // Local tools are prompt-author Lua functions with no live implementation;
    // the loop recognizes the `Local` marker and routes their calls back into
    // the section VM. The alias was validated at `tools.add_local`
    // registration.
    for schema in local_schemas {
        dispatch.insert(schema.name.clone(), DispatchTarget::Local);
        schemas.push(schema.clone());
    }
    Ok((schemas, dispatch))
}
