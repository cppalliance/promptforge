//! Tool-scope near-duplicate validation and schema/dispatch preparation for
//! the model-visible tool set.

use std::collections::BTreeMap;

use crate::client::ToolSchema;
use crate::lua::ToolBinding;
use crate::observe::{Observer, detail};
use crate::{Error, NearDuplicateDiagnostic, Result};

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
    bindings: &[ToolBinding],
    local_schemas: &[ToolSchema],
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<(Vec<ToolSchema>, BTreeMap<String, DispatchTarget>)> {
    observer.observe(execution, section, detail::TOOL_SCOPE_VALIDATION_STARTED);
    let result = validate_effective_scope_inner(bindings)
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

/// The scope check is purely local: a clash errors when both halves of a
/// bind-time conflict enter one model-visible scope. Conflicts were recorded
/// symmetrically at bind time, so the first in-scope binding whose conflict
/// list names another in-scope alias is the diagnostic's first half.
pub(crate) fn validate_effective_scope_inner(bindings: &[ToolBinding]) -> Result<()> {
    for binding in bindings {
        for conflict in binding.conflicts() {
            let Some(other) = bindings
                .iter()
                .find(|candidate| candidate.alias() == conflict.alias)
            else {
                continue;
            };
            return Err(Error::NearDuplicateTools {
                diagnostic: Box::new(NearDuplicateDiagnostic {
                    first_alias: binding.alias().to_owned(),
                    first_id: binding.id().clone(),
                    second_alias: other.alias().to_owned(),
                    second_id: other.id().clone(),
                    similarity: conflict.similarity,
                }),
            });
        }
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
