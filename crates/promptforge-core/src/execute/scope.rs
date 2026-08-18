//! Tool-scope analysis, near-duplicate validation, and schema/dispatch
//! preparation for the model-visible tool set.

use std::collections::{BTreeMap, BTreeSet};

use promptforge_tool_picker::{ToolId as PickerToolId, ToolPicker};

use crate::client::ToolSchema;
use crate::lua::{ToolBinding, ToolBindings};
use crate::observe::{Observer, detail};
use crate::tools::{ToolId, ToolRegistry};
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

pub(crate) fn prepare_effective_scope(
    analysis: &ToolAnalysis,
    bindings: &[ToolBinding],
    local_schemas: &[ToolSchema],
    registry: &ToolRegistry<'_>,
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<(Vec<ToolSchema>, BTreeMap<String, ToolId>)> {
    observer.observe(execution, section, detail::TOOL_SCOPE_VALIDATION_STARTED);
    let result = validate_effective_scope_inner(analysis, bindings)
        .and_then(|()| prepare_scoped_tools(bindings, local_schemas, registry));
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
        let first_alias = analysis
            .id_to_alias
            .get(&pair.first_id)
            .cloned()
            .ok_or_else(|| Error::ToolScopeAnalysis {
                detail: "selected identity has no frozen alias".to_owned(),
            })?;
        let second_alias = analysis
            .id_to_alias
            .get(&pair.second_id)
            .cloned()
            .ok_or_else(|| Error::ToolScopeAnalysis {
                detail: "selected identity has no frozen alias".to_owned(),
            })?;
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
    registry: &ToolRegistry<'_>,
) -> Result<(Vec<ToolSchema>, BTreeMap<String, ToolId>)> {
    let mut schemas = Vec::with_capacity(bindings.len() + local_schemas.len());
    let mut dispatch = BTreeMap::new();
    for binding in bindings {
        let tool = registry
            .get(binding.id())
            .ok_or_else(|| Error::UnknownScopedTool(binding.alias().to_owned()))?;
        // Default model-facing text stays the registry description so bind
        // capability strings never leak into schemas unless the author
        // overrode `.description` on the Tool object before tools.add.
        let description = binding
            .model_description()
            .unwrap_or_else(|| tool.description())
            .to_owned();
        // F7: build every advertised schema through the validated constructor,
        // so an unusable wire name or a non-object JSON Schema is refused here
        // rather than sent to the model.
        let schema = ToolSchema::new(
            binding.alias().to_owned(),
            description,
            tool.parameters_schema(),
        )
        .map_err(|error| Error::BindSchema {
            alias: binding.alias().to_owned(),
            source: Box::new(error),
        })?;
        schemas.push(schema);
        dispatch.insert(binding.alias().to_owned(), binding.id().clone());
    }
    // Local tools dispatch under a sentinel identity the tool loop recognizes
    // by server name; they never enter the registry. The alias was validated
    // at `tools.local` registration, so identity construction cannot fail.
    for schema in local_schemas {
        dispatch.insert(
            schema.name.clone(),
            ToolId::from_validated("local", schema.name.clone()),
        );
        schemas.push(schema.clone());
    }
    Ok((schemas, dispatch))
}
