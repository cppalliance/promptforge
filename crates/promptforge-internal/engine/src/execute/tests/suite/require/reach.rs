//! What a `require` reaches: a nested fanout worker reaches a top-level
//! library its prompt reads cannot see, a library's own `require` during
//! its load resolves from the library's position, and a library function
//! called later resolves `call`, `prompt.section`, and `require` from the
//! calling section.

use super::*;

#[test]
fn a_nested_fanout_worker_requires_a_top_level_library() {
    let source = PromptSource::new()
        .section(
            "Research",
            "
            local results = tasks.fanout('### Worker', { 'alpha', 'beta' })
            return table.concat(results, ',')",
        )
        .section("### Worker", "return require('## helpers').label(item)")
        .prose(
            "helpers",
            "
            for {{ item }}

            ```lua
            local seen = prose
            return { label = function(value) return value .. ' ' .. seen end }
            ```",
        );
    let trace = require_case(source.build()).run();
    assert_eq!(
        trace.text(),
        "alpha for alpha,beta for beta",
        "each arm renders the library prose with its own item"
    );
    let loads = library_loads(&trace.records());
    assert_eq!(
        loads
            .iter()
            .filter(|(section, detail)| section == "Worker"
                && detail == "Lua library load succeeded: ## helpers")
            .count(),
        2,
        "each arm's VM loads the library itself: {loads:?}"
    );
}

#[test]
fn a_nested_worker_requires_a_library_its_prompt_section_read_cannot_see() {
    let source = PromptSource::new()
        .section(
            "Research",
            "return tostring(tasks.fanout('### Worker', { 'alpha' })[1])",
        )
        .section(
            "### Worker",
            "
            local read = prompt.section('## house_style')
            return tostring(read) .. '|' .. require '## house_style'",
        )
        .prose(
            "house_style",
            "
            Plain words for {{ item }}.

            ```lua
            return prose
            ```",
        );
    let trace = require_case(source.build()).run();
    assert_eq!(
        trace.text(),
        "nil|Plain words for alpha.",
        "a parent's sibling is outside the visible set but within the reach"
    );
}

#[test]
fn a_load_resolves_from_the_library_and_a_later_call_from_the_caller() {
    let source = PromptSource::new()
        .section(
            "Main",
            "
            local lib = require '## lib'
            return lib.helped .. '|' .. lib.later()",
        )
        .prose(
            "### mine",
            "
            Mine text.

            ```lua
            return 'mine'
            ```",
        )
        .section(
            "lib",
            "
            local helped = require '### helper'
            return {
              helped = helped,
              later = function()
                local text = prompt.section('### mine')
                local found = text ~= nil and text:find('Mine text.', 1, true) ~= nil
                local helper_ok = pcall(require, '### helper')
                return table.concat({ require '### mine', call('### mine'), tostring(found),
                  tostring(helper_ok) }, ',')
              end,
            }",
        )
        .section("### helper", "return 'helped'");
    let trace = require_case(source.build()).run();
    assert_eq!(
        trace.text(),
        "helped|mine,mine,true,false",
        "the load reaches the library's child; the later call reaches the caller's \
         child and no longer the library's"
    );
}
