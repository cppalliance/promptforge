//! What raises: a cycle, a heading out of reach or misspelled, a `jump`
//! during a load, and a Plugin prelude that defines or calls `require`.
//! A library that raises while loading runs again on the next `require`,
//! and a raise in library code reports the library's location and
//! prompt-file line.

use promptforge_types::plugins::Prelude;

use crate::RunErrorKind;
use crate::execute::tests::preludes::prelude;
use crate::test_support::scenario::Trace;

use super::*;

/// Runs `source` with `preludes` installed in every section VM.
#[track_caller]
fn run_with_preludes(source: &str, preludes: Vec<Prelude>) -> Trace {
    require_case(source.to_owned())
        .context(|ctx| ctx.preludes(preludes))
        .run()
}

/// The text of a run that fails as a Lua failure.
#[track_caller]
fn lua_failure(trace: &Trace) -> String {
    let error = trace.error();
    assert_eq!(error.kind(), RunErrorKind::Lua, "{error}");
    error.to_string()
}

#[test]
fn a_cycle_raises_naming_its_sections_in_order() {
    let source = prompt(
        "",
        &[
            ("Main", "require '## a'"),
            ("a", "require '## b'"),
            ("b", "require '## a'"),
        ],
    );
    let message = lua_failure(&require_case(source).run());
    assert!(
        message.contains("require cycle: ## a -> ## b -> ## a"),
        "the cycle names both libraries: {message}"
    );
}

#[test]
fn an_out_of_reach_or_misspelled_heading_parses_and_raises_when_the_call_runs() {
    let source = PromptSource::new()
        .section(
            "Main",
            "
            local _, far = pcall(require, '### deep')
            local _, typo = pcall(require, '## helpres')
            return tostring(far) .. '\\n' .. tostring(typo)",
        )
        .prose("other", "")
        .section("### deep", "return 'deep'")
        .section("helpers", "return 'helpers'");
    let trace = require_case(source.build()).run();
    let (far, typo) = trace.text().split_once('\n').expect("two messages");
    assert!(
        far.contains(
            "section heading `### deep` not found; available sections: ## other, ## helpers"
        ),
        "a sibling's child is out of reach: {far}"
    );
    assert!(
        typo.contains(
            "section heading `## helpres` not found; available sections: ## other, ## helpers"
        ),
        "a misspelled heading names the reach: {typo}"
    );
}

#[test]
fn a_jump_during_a_load_raises() {
    let source = prompt(
        "",
        &[
            ("Main", "require '## jumper'"),
            ("jumper", "jump('## Last')"),
            ("Last", "return 'jumped'"),
        ],
    );
    let message = lua_failure(&require_case(source).run());
    assert!(
        message.contains("jump is unavailable while a library loads"),
        "the load refuses the jump: {message}"
    );
}

#[test]
fn a_library_that_raises_while_loading_runs_again_on_the_next_require() {
    let source = prompt(
        "",
        &[
            (
                "Main",
                "
                var.attempts = 0
                local first_ok = pcall(require, '## flaky')
                local second = require '## flaky'
                return tostring(first_ok) .. '|' .. second .. '|' .. var.attempts",
            ),
            (
                "flaky",
                "
                var.attempts = var.attempts + 1
                if var.attempts == 1 then error('the first load fails') end
                return 'loaded'",
            ),
        ],
    );
    let trace = require_case(source).run();
    assert_eq!(trace.text(), "false|loaded|2", "the second load succeeds");
}

#[test]
fn a_prelude_cannot_define_require_or_call_it_while_it_installs() {
    let source = prompt(
        "",
        &[("Main", "return 'unreachable'"), ("lib", "return 'lib'")],
    );
    let defines = lua_failure(&run_with_preludes(
        &source,
        vec![prelude("shadow", "function require() return 'mine' end")],
    ));
    assert!(
        defines.contains("Plugin `shadow`: its prelude defines the global `require`")
            && defines.contains("which is reserved as an Engine global"),
        "require is reserved: {defines}"
    );
    let calls = lua_failure(&run_with_preludes(
        &source,
        vec![prelude("eager", "require '## lib'")],
    ));
    assert!(
        calls.contains("Plugin `eager`: its prelude failed to load")
            && calls.contains("attempt to call a nil value (global 'require')"),
        "the prelude phase has no require: {calls}"
    );
}

#[test]
fn a_raise_during_the_load_reports_the_library_line() {
    let source = "\
        ---\n\
        name: t\n\
        description: d\n\
        promptforge: 0\n\
        ---\n\
        \n\
        # Require\n\
        \n\
        ## Main\n\
        \n\
        ```lua\n\
        require '## lib'\n\
        ```\n\
        \n\
        ## lib\n\
        \n\
        ```lua\n\
        local ready = true\n\
        error('load fault')\n\
        ```\n";
    let message = lua_failure(&require_case(source.to_owned()).run());
    assert!(
        message.contains("section `lib` prologue:19: load fault"),
        "the raise names the library block and its prompt line 19: {message}"
    );
}

#[test]
fn a_raise_while_a_long_titled_library_loads_reports_the_library_line() {
    let source = "\
        ---\n\
        name: t\n\
        description: d\n\
        promptforge: 0\n\
        ---\n\
        \n\
        # Require\n\
        \n\
        ## Main\n\
        \n\
        ```lua\n\
        require '## Shared Research Helper Functions'\n\
        ```\n\
        \n\
        ## Shared Research Helper Functions\n\
        \n\
        ```lua\n\
        local ready = true\n\
        error('load fault')\n\
        ```\n";
    let message = lua_failure(&require_case(source.to_owned()).run());
    assert!(
        message.contains("section `Shared Research Helper Functions` prologue:19: load fault"),
        "the raise names the library block and its prompt line 19: {message}"
    );
}

#[test]
fn a_raise_from_a_library_function_called_later_reports_the_library_line() {
    let source = "\
        ---\n\
        name: t\n\
        description: d\n\
        promptforge: 0\n\
        ---\n\
        \n\
        # Require\n\
        \n\
        ## Main\n\
        \n\
        ```lua\n\
        require '## lib'\n\
        boom()\n\
        ```\n\
        \n\
        ## lib\n\
        \n\
        ```lua\n\
        function boom()\n\
        error('later fault')\n\
        end\n\
        ```\n";
    let message = lua_failure(&require_case(source.to_owned()).run());
    assert!(
        message.contains("section `lib` prologue:20: later fault"),
        "the raise names the library block and its prompt line 20: {message}"
    );
    assert!(
        message.contains("section `Main` prologue:13:"),
        "the caller's frame maps to its own prompt line 13: {message}"
    );
}
