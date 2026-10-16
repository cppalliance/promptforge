//! Tests for the scheduler's `tool_call` arm: the model-issued form
//! (`call_id: Some`), which always resumes with content and reports its
//! `ToolResult` under the model's call id; the script form (`call_id:
//! None`), which keeps the raise-at-call-site behavior; local Lua tools,
//! whose handlers run inside the calling block coroutine - store calls,
//! nested local calls, and bound calls included - with no leaf work of
//! their own; and the reserved task names, refused before alias lookup.
//! The fixtures reach the
//! model-issued form through the test-only `tools.call_as_model` install
//! (`Scenario::raw_shims`); in production only the loop shim yields
//! a `call_id`.
//! The local Lua tool cases sit in `local_handlers`.

use promptforge_parser::test_support::one_section;

use super::models_loop::loop_scenario;
use super::*;
use crate::lua::ToolBinding;
use crate::test_support::recording::ToolResultReport;
use crate::test_support::scenario::Trace;

/// Records every observation and every `on_tool_result` report as one
/// rendered line, so a test reads the arm's whole reporting sequence.
#[derive(Default)]
pub(super) struct ToolRecorder(Mutex<Vec<String>>);

impl ToolRecorder {
    fn push(&self, line: String) {
        self.0
            .lock()
            .expect("the tool recorder mutex is not poisoned")
            .push(line);
    }

    pub(super) fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("the tool recorder mutex is not poisoned")
            .clone()
    }
}

impl Observer for ToolRecorder {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        self.push(format!("{section}: {event}"));
    }

    fn on_tool_result(&self, report: ToolResultReport<'_>) {
        let ToolResultReport {
            section,
            tool_call_id,
            alias,
            content,
            trust,
            ..
        } = report;
        self.push(format!(
            "{section}: tool_result id={tool_call_id} alias={alias} trust={trust:?} content={content}"
        ));
    }
}

/// A tool-call arm test's run of `source`: the loop models filled in (the
/// arm tests bypass the live H1 pass that would fill them), nothing
/// offered unless the test offers it, and the raw protocol shims exposed
/// so a section can yield a model-issued call.
fn arm_scenario(source: impl Into<String>) -> Scenario {
    loop_scenario(source).raw_shims()
}

/// The `ToolRecorder` replay of `trace`'s events.
fn tool_lines(trace: &Trace) -> ToolRecorder {
    let recorder = ToolRecorder::default();
    trace.replay(&recorder);
    recorder
}

#[test]
fn a_failing_bound_tool_with_a_call_id_resumes_with_untrusted_failure_text() {
    let source = one_section(
        "tools.offer('tools/failing')
         local out = tools.call_as_model('call_1', 'fail', {})
         assert(type(out) == 'string', 'a model-issued call always resumes with content')
         return out",
    );
    let trace = arm_scenario(source).offer("fail", failing_tool()).run();
    let out = trace.text();
    assert!(
        out.contains("<untrusted_input_") && out.contains("</untrusted_input_"),
        "the failure text is nonce-wrapped as untrusted, got: {out}"
    );
    assert!(
        out.contains("the tool's own backend failed"),
        "the wrapped text includes the tool's message, got: {out}"
    );
    let lines = tool_lines(&trace).lines();
    assert!(
        lines
            .iter()
            .any(|line| line == &format!("Only: {}", detail::TOOL_CALL_FAILED)),
        "the tool's failure is observed: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Only: tool_result id=call_1 alias=fail trust=Untrusted")),
        "ToolResult fires under the model's call id as untrusted: {lines:?}"
    );
}

#[test]
fn the_same_failing_tool_without_a_call_id_raises_kind_tool() {
    let source = one_section(
        "tools.offer('tools/failing')
         local ok, err = pcall(tools.call, 'tools/failing', {})
         assert(not ok, 'a script call raises the tool failure at the call site')
         return err.kind .. '|' .. tostring(err)",
    );
    let trace = arm_scenario(source).offer("fail", failing_tool()).run();
    let out = trace.text();
    assert!(
        out.starts_with("tool|"),
        "a script call's tool failure reads as kind `tool`, got: {out}"
    );
    assert!(
        out.contains("the tool's own backend failed"),
        "the raised message is the tool's own, got: {out}"
    );
    let lines = tool_lines(&trace).lines();
    assert!(
        !lines.iter().any(|line| line.contains("tool_result")),
        "a failed script call reports no ToolResult: {lines:?}"
    );
}

#[test]
fn a_bound_alias_with_no_implementation_in_the_test_driver_table_resumes_as_a_tool_error() {
    // The binding names an identity the Engine advertises and journals,
    // but the scenario holds no tool under it: it answers the effect with
    // the error instead of a call, and a script call raises it at the
    // call site.
    let source = one_section(
        "tools.offer('tools/echo')
         local ok, err = pcall(tools.call, 'tools/echo', { value = 'hi' })
         assert(not ok, 'an unresolvable identity raises at the call site')
         return err.kind .. '|' .. tostring(err)",
    );
    let binding = ToolBinding::for_test("tools_echo", "echo capability", &echo_tool().descriptor());
    let trace = arm_scenario(source).offering(vec![binding]).run();
    let out = trace.text();
    assert!(
        out.starts_with("tool|"),
        "the missing implementation reads as kind `tool`, got: {out}"
    );
    assert!(
        out.contains("no implementation in the test driver's tool table"),
        "the raised message names the test driver's tool table, got: {out}"
    );
    let lines = tool_lines(&trace).lines();
    assert!(
        !lines.iter().any(|line| line.contains("tool_result")),
        "nothing was called, so no ToolResult is reported: {lines:?}"
    );
}

/// The `tool_result` lines `recorder` saw, in order.
pub(super) fn tool_result_lines(recorder: &ToolRecorder) -> Vec<String> {
    recorder
        .lines()
        .into_iter()
        .filter(|line| line.contains("tool_result"))
        .collect()
}

#[test]
fn a_reserved_task_name_answers_unbound_tool_before_alias_lookup() {
    // `task_status` is registered as a local tool so the test proves the
    // reservation wins over a lookup that would otherwise succeed.
    let source = one_section(
        "tools.offer_local('task_status', 'shadow', {}, function() return 'shadowed' end)
         local kinds = {}
         for _, name in ipairs({ 'task', 'task_cancel', 'task_status', 'await_tasks' }) do
           local ok, err = pcall(tools.call, name, {})
           assert(not ok, name .. ' must be refused')
           kinds[#kinds + 1] = err.kind .. ':' .. err.name
         end
         return table.concat(kinds, ',')",
    );
    let trace = arm_scenario(source).run();
    assert_eq!(
        trace.text(),
        "unbound_tool:task,unbound_tool:task_cancel,unbound_tool:task_status,\
         unbound_tool:await_tasks"
    );
    let lines = tool_lines(&trace).lines();
    assert!(
        !lines.iter().any(|line| line.contains("tool_result")),
        "a reserved name dispatches nothing: {lines:?}"
    );
}

#[path = "tool_call_arm-local-handlers.rs"]
mod local_handlers;
