//! Tool-scope schema/dispatch preparation for the model-visible tool set.

use std::collections::BTreeMap;

use crate::client::ToolSchema;
use crate::lua::ToolBinding;
use crate::observe::{Observer, detail};
use crate::{Error, Result};

/// What kind of tool stands behind one alias a round advertised.
///
/// Produced by [`prepare_scoped_tools`] and recorded on the chain as the
/// round's advertised scope: the `chat` arm gates the model's requested
/// names against the map's keys, and the `tool_call` arm the loop shim
/// then yields resolves each name itself - a bound alias against the run's
/// tool catalog, a local alias against the section VM's handlers - so the
/// map carries no implementation.
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

pub(crate) fn prepare_effective_scope(
    bindings: &[ToolBinding],
    local_schemas: &[ToolSchema],
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<(Vec<ToolSchema>, BTreeMap<String, DispatchTarget>)> {
    observer.observe(execution, section, detail::TOOL_SCOPE_VALIDATION_STARTED);
    let result = prepare_scoped_tools(bindings, local_schemas);
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
