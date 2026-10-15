//! Loading a library: the table of functions it hands back, the saved
//! value a second `require` returns, the load reports, one load in each
//! requiring section, and a child section that starts without its
//! parent's libraries.

use super::*;

#[test]
fn a_section_uses_the_table_of_functions_a_sibling_library_returns() {
    let source = prompt(
        "",
        &[
            (
                "Main",
                "
                local shared = require '## shared_stuff'
                return shared.search_all(argv.prose)",
            ),
            (
                "shared_stuff",
                "
                local M = {}
                function M.search_all(topic)
                  return 'searched ' .. topic .. ' everywhere'
                end
                return M",
            ),
        ],
    );
    let trace = require_case(source).args("rust").run();
    assert_eq!(trace.text(), "searched rust everywhere");
}

#[test]
fn a_second_require_returns_the_saved_value_without_rerunning_it() {
    let source = prompt(
        "",
        &[
            (
                "Main",
                "
                local first = require '## counted'
                local second = require '## counted'
                local no_first = require '## falsy'
                local no_second = require '## falsy'
                return table.concat({ tostring(first == second), var.counted, tostring(no_first),
                  tostring(no_second), var.falsy }, '|')",
            ),
            (
                "counted",
                "
                var.counted = (var.counted or 0) + 1
                return {}",
            ),
            (
                "falsy",
                "
                var.falsy = (var.falsy or 0) + 1
                return false",
            ),
        ],
    );
    let trace = require_case(source).run();
    assert_eq!(
        trace.text(),
        "true|1|false|false|1",
        "both libraries load once"
    );
}

#[test]
fn a_first_load_reports_its_start_and_end_naming_the_library() {
    let source = prompt(
        "",
        &[
            (
                "Main",
                "
                require '## lib'
                require '## lib'
                assert(not pcall(require, '## broken'), 'the broken library raises')
                return 'done'",
            ),
            ("lib", "loaded = true"),
            ("broken", "error('broken library')"),
        ],
    );
    let trace = require_case(source).run();
    assert_eq!(trace.text(), "done", "the caller catches the failure");
    assert_eq!(
        library_loads(&trace.records()),
        [
            load("Main", "Lua library load started: ## lib"),
            load("Main", "Lua library load succeeded: ## lib"),
            load("Main", "Lua library load started: ## broken"),
            load("Main", "Lua library load failed: ## broken"),
        ],
        "a saved library reports nothing on its second require"
    );
}

#[test]
fn load_time_code_runs_once_in_each_requiring_section() {
    let source = prompt(
        "",
        &[
            (
                "First",
                "
                require '## lib'
                require '## lib'",
            ),
            (
                "Second",
                "
                require '## lib'
                require '## lib'
                return tostring(var.loads)",
            ),
            ("lib", "var.loads = (var.loads or 0) + 1"),
        ],
    );
    let trace = require_case(source).run();
    assert_eq!(
        trace.text(),
        "2",
        "each section's VM loads the library once"
    );
    assert_eq!(
        library_loads(&trace.records()),
        [
            load("First", "Lua library load started: ## lib"),
            load("First", "Lua library load succeeded: ## lib"),
            load("Second", "Lua library load started: ## lib"),
            load("Second", "Lua library load succeeded: ## lib"),
        ]
    );
}

#[test]
fn a_child_section_starts_without_its_parents_libraries_and_loads_its_own() {
    let source = prompt(
        "",
        &[
            (
                "Parent",
                "
                require '## lib'
                assert(greet, 'the parent sees the library global')
                return call('### Child')",
            ),
            (
                "### Child",
                "
                local before = tostring(greet)
                require '## lib'
                return before .. '|' .. greet('child')",
            ),
            ("lib", "function greet(name) return 'hello ' .. name end"),
        ],
    );
    let trace = require_case(source).run();
    assert_eq!(
        trace.text(),
        "nil|hello child",
        "the child loads the library itself"
    );
    assert_eq!(
        library_loads(&trace.records()),
        [
            load("Parent", "Lua library load started: ## lib"),
            load("Parent", "Lua library load succeeded: ## lib"),
            load("Child", "Lua library load started: ## lib"),
            load("Child", "Lua library load succeeded: ## lib"),
        ]
    );
}
