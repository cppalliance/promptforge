//! Tool-scope schema/dispatch preparation for the model-visible tool set.

use std::collections::BTreeMap;

use crate::lua::ToolBinding;
use crate::model::ToolSchema;
use crate::{Error, Result};
use promptforge_api_types::event::lifecycle;

use promptforge_api_types::emitter::Emitter;

/// What kind of tool stands behind one alias a round advertised.
///
/// Produced by [`prepare_scoped_tools`] and recorded on the chain as the
/// round's advertised scope: the `chat` arm gates the model's requested
/// names against the map's keys, and the `tool_call` arm the loop shim
/// then yields resolves each name itself - a bound alias against the run's
/// tool catalog, a local alias against the section VM's handlers - so the
/// map holds no implementation.
#[derive(Debug, Clone)]
pub(crate) enum DispatchTarget {
    /// A bound live tool, resolved against the run's tool catalog.
    Bound,
    /// A Lua-local tool, answered by the section VM's handler.
    Local,
    /// One of the model's task built-ins (`task`, `task_cancel`,
    /// `task_status`, `await_tasks`), answered by the scheduler over its
    /// task arena.
    Builtin,
}

/// Builds the round's advertised scope under the validation boundary
/// pair, reported through the chain's `emitter` under `section`.
///
/// # Errors
/// Returns the schema-construction error of [`prepare_scoped_tools`].
pub(crate) fn prepare_effective_scope(
    bindings: &[ToolBinding],
    local_schemas: &[ToolSchema],
    emitter: &Emitter,
    section: &str,
) -> Result<(Vec<ToolSchema>, BTreeMap<String, DispatchTarget>)> {
    emitter.report(section, lifecycle::TOOL_SCOPE_VALIDATION_STARTED);
    let result = prepare_scoped_tools(bindings, local_schemas);
    emitter.report(
        section,
        if result.is_ok() {
            lifecycle::TOOL_SCOPE_VALIDATION_SUCCEEDED
        } else {
            lifecycle::TOOL_SCOPE_VALIDATION_FAILED
        },
    );
    result
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
        // binding's `model_description`); the catalog fallback is the
        // description the binding copied from the tool's descriptor at
        // fill time.
        let description = binding
            .model_description()
            .unwrap_or_else(|| binding.description())
            .to_owned();
        // F7: build every advertised schema through the validated constructor,
        // so an unusable wire name or a non-object JSON Schema is refused here
        // rather than sent to the model.
        let schema = ToolSchema::new(
            binding.alias().to_owned(),
            description,
            binding.schema().clone(),
        )
        .map_err(|error| Error::BindSchema {
            alias: binding.alias().to_owned(),
            source: Box::new(error),
        })?;
        schemas.push(schema);
        dispatch.insert(binding.alias().to_owned(), DispatchTarget::Bound);
    }
    // Local tools are prompt-author Lua functions with no live implementation;
    // the `tool_call` arm answers their calls on the section VM. The alias
    // was validated at `tools.add_local` registration.
    for schema in local_schemas {
        dispatch.insert(schema.name.clone(), DispatchTarget::Local);
        schemas.push(schema.clone());
    }
    Ok((schemas, dispatch))
}
