//! Explicit fanout: map a worker section over a list of items.
//!
//! A parent section's Lua epilog calls `fanout("### Worker", "### List")` to
//! run the worker template once per item parsed from the list section. Arms
//! execute sequentially in v1; each gets a fresh [`SectionVm`] with `item`
//! and `sys.taskid` injected. The invoker receives an ordered Lua table of
//! arm replies.

use std::collections::BTreeMap;

use serde_json::json;

use crate::bind::BoundPrompt;
use crate::client::GatewayClient;
use crate::debug::DebugCapture;
use crate::lua::{SectionVm, ToolBindings};
use crate::model::ModelBindings;
use crate::observe::{Observer, detail};
use crate::parser::Section;
use crate::store::StoreRef;
use crate::tools::ToolRegistry;
use crate::{Error, Result, subst};

/// Resolves a heading string like `"### Name"` against a list of sibling
/// sections, returning the matching section.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading does not start with `#` markers,
/// or when no sibling matches. The error message lists available siblings.
pub(crate) fn resolve_sibling<'a>(heading: &str, siblings: &'a [Section]) -> Result<&'a Section> {
    let stripped = heading.trim();
    if !stripped.starts_with('#') {
        return Err(Error::Lua(format!(
            "fanout heading must include ### markers, got bare name: {stripped}"
        )));
    }
    let level_end = stripped.find(|c: char| c != '#').unwrap_or(stripped.len());
    let name = stripped[level_end..].trim();
    if name.is_empty() {
        return Err(Error::Lua(format!(
            "fanout heading has no name: {stripped}"
        )));
    }

    for section in siblings {
        if section.name == name {
            return Ok(section);
        }
    }

    let available: Vec<String> = siblings
        .iter()
        .map(|s| format!("{} {}", "#".repeat(s.level.into()), s.name))
        .collect();
    Err(Error::Lua(format!(
        "fanout heading `{stripped}` not found; available siblings: {}",
        available.join(", ")
    )))
}

/// Everything a fanout needs from the invoker's context.
pub(crate) struct FanoutContext<'a> {
    pub args: &'a str,
    pub store: &'a StoreRef,
    pub execution: &'a str,
    pub observer: &'a dyn Observer,
    pub client: &'a Option<GatewayClient>, // owned option; arms clone as needed
    pub debug: Option<&'a dyn DebugCapture>,
    pub shared: Option<&'a crate::lua::LuaProgram>,
    pub bindings: &'a ToolBindings,
    pub models: &'a ModelBindings,
    pub bound: Option<&'a BoundPrompt>,
    pub registry: &'a ToolRegistry<'a>,
    pub max_tool_iterations: usize,
    pub last_reply: Option<&'a str>,
    pub when: &'a str,
    /// The 1-based id of the parent section that initiated the fanout.
    pub parent_id: usize,
}

/// Runs the worker section template once per item, sequentially.
///
/// Returns the ordered reply strings from each arm.
///
/// # Errors
/// Fails fast: the first arm error aborts the fanout.
#[expect(
    clippy::too_many_lines,
    reason = "the arm lifecycle is a linear sequence of fallible steps with per-step cleanup"
)]
pub(crate) async fn run_fanout_arms(
    worker: &Section,
    items: &[String],
    ctx: &FanoutContext<'_>,
) -> Result<Vec<String>> {
    let mut replies = Vec::with_capacity(items.len());
    let mut turns: u32 = 0;

    for (index, item_text) in items.iter().enumerate() {
        let taskid = (index + 1).to_string();
        ctx.observer
            .observe(ctx.execution, &worker.name, detail::FANOUT_ARM_STARTED);

        let mut vm = match ctx.shared {
            Some(shared) => SectionVm::new_with_shared_bindings(
                shared,
                ctx.bindings,
                ctx.models,
                ctx.execution,
                ctx.observer,
                &worker.name,
            )?,
            None => SectionVm::new(None, ctx.execution, ctx.observer, &worker.name)?,
        };

        let sys = json!({
            "when": ctx.when,
            "now": crate::execute::now_rfc3339(),
            "id": ctx.parent_id,
            "taskid": taskid,
        });

        if let Err(error) = vm.inject_host(ctx.args, &sys, ctx.store, ctx.last_reply) {
            vm.teardown(ctx.observer, &worker.name);
            return Err(error);
        }

        // Inject `item` as a Lua global for preamble/epilog access.
        if let Err(error) = vm.set_global_string("item", item_text) {
            vm.teardown(ctx.observer, &worker.name);
            return Err(error);
        }

        // Preamble
        let preamble_return = if let Some(program) = &worker.preamble {
            match vm.run_preamble(program, ctx.observer, &worker.name) {
                Ok(returned) => returned,
                Err(error) => {
                    vm.teardown(ctx.observer, &worker.name);
                    return Err(error);
                }
            }
        } else {
            None
        };

        if let Some(value) = preamble_return {
            vm.teardown(ctx.observer, &worker.name);
            ctx.observer
                .observe(ctx.execution, &worker.name, detail::FANOUT_ARM_FINISHED);
            replies.push(value);
            continue;
        }

        // Close scopes
        let scopes = match vm.close_scopes(ctx.observer, &worker.name) {
            Ok(scopes) => scopes,
            Err(error) => {
                vm.teardown(ctx.observer, &worker.name);
                return Err(error);
            }
        };
        let scope = scopes.tools;
        let completion_options = scopes
            .model
            .as_ref()
            .map(crate::model::ModelBinding::completion_options);

        // Substitution with item
        let var = match vm.var() {
            Ok(var) => var,
            Err(error) => {
                vm.teardown(ctx.observer, &worker.name);
                return Err(error);
            }
        };
        let prose = match subst::substitute(
            &worker.prose,
            ctx.args,
            ctx.last_reply,
            Some(item_text),
            &var,
            &sys,
        ) {
            Ok(prose) => prose,
            Err(error) => {
                vm.teardown(ctx.observer, &worker.name);
                return Err(error);
            }
        };

        // Model turn (if prose is non-empty)
        let mut arm_reply: Option<String> = None;
        if !prose.trim().is_empty() {
            let (schemas, dispatch) = match ctx.bound {
                Some(bound) => {
                    match crate::execute::prepare_effective_scope(
                        bound,
                        &scope,
                        ctx.registry,
                        ctx.execution,
                        ctx.observer,
                        &worker.name,
                    ) {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            vm.teardown(ctx.observer, &worker.name);
                            return Err(error);
                        }
                    }
                }
                None => (Vec::new(), BTreeMap::new()),
            };
            if let Some(client) = ctx.client {
                let text = match crate::execute::run_tool_loop(
                    client,
                    &schemas,
                    &dispatch,
                    ctx.registry,
                    prose,
                    ctx.max_tool_iterations,
                    crate::execute::SectionProgress {
                        execution: ctx.execution,
                        observer: ctx.observer,
                        section: &worker.name,
                        turns: &mut turns,
                        debug: ctx.debug,
                        completion_options: completion_options.as_ref(),
                    },
                )
                .await
                {
                    Ok(text) => text,
                    Err(error) => {
                        vm.teardown(ctx.observer, &worker.name);
                        return Err(error);
                    }
                };
                if let Err(error) = vm.bind_reply(&text, ctx.observer, &worker.name) {
                    vm.teardown(ctx.observer, &worker.name);
                    return Err(error);
                }
                arm_reply = Some(text);
            }
        }

        // Epilog
        let epilog_return = if let Some(program) = &worker.epilog {
            match vm.run_epilog(program, ctx.observer, &worker.name) {
                Ok(returned) => returned,
                Err(error) => {
                    vm.teardown(ctx.observer, &worker.name);
                    return Err(error);
                }
            }
        } else {
            None
        };

        vm.teardown(ctx.observer, &worker.name);
        ctx.observer
            .observe(ctx.execution, &worker.name, detail::FANOUT_ARM_FINISHED);

        let reply = epilog_return.or(arm_reply).unwrap_or_default();
        replies.push(reply);
    }

    Ok(replies)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_sibling_finds_exact_match() {
        let sections = vec![
            Section {
                name: "Worker".to_string(),
                level: 3,
                preamble: None,
                prose: String::new(),
                epilog: None,
                children: Vec::new(),
                items: Vec::new(),
            },
            Section {
                name: "Topics".to_string(),
                level: 3,
                preamble: None,
                prose: String::new(),
                epilog: None,
                children: Vec::new(),
                items: vec!["a".to_string()],
            },
        ];
        let found = resolve_sibling("### Worker", &sections).expect("must resolve");
        assert_eq!(found.name, "Worker");
    }

    #[test]
    fn resolve_sibling_missing_heading_lists_available() {
        let sections = vec![Section {
            name: "Worker".to_string(),
            level: 3,
            preamble: None,
            prose: String::new(),
            epilog: None,
            children: Vec::new(),
            items: Vec::new(),
        }];
        let err =
            resolve_sibling("### Missing", &sections).expect_err("missing heading must error");
        assert!(err.to_string().contains("### Worker"), "error was: {err}");
    }

    #[test]
    fn resolve_sibling_bare_name_errors() {
        let sections = vec![Section {
            name: "Worker".to_string(),
            level: 3,
            preamble: None,
            prose: String::new(),
            epilog: None,
            children: Vec::new(),
            items: Vec::new(),
        }];
        let err =
            resolve_sibling("Worker", &sections).expect_err("bare name without ### must error");
        assert!(err.to_string().contains("### markers"), "error was: {err}");
    }
}
