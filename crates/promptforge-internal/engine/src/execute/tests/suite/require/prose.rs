//! Library prose: a library block reads its own `prose`, rendered from
//! the caller's environment, and the caller's `prose` comes back
//! unchanged, whether the caller read it first or not and whether the
//! library finished or raised.

use super::*;

#[test]
fn a_library_hands_its_prose_to_the_caller_and_leaves_the_callers_prose_alone() {
    let source = PromptSource::new()
        .prose(
            "Main",
            "
            First {{ var.word }}.

            ```lua
            var.word = 'one'
            local before = prose
            var.word = 'changed'
            style = 'terse'
            local text = require '## house_style'
            local ok = pcall(require, '## broken_style')
            var.summary = table.concat({ before, prose, text, tostring(ok) }, '|')
            ```

            Second {{ var.word }}.

            ```lua
            var.word = 'two'
            local ok = pcall(require, '## broken_style')
            var.word = 'three'
            return table.concat({ var.summary, tostring(ok), var.broken_saw, prose }, '|')
            ```",
        )
        .prose(
            "house_style",
            "
            Write in a {{ style }} voice for {{ args }}.

            ```lua
            return prose
            ```",
        )
        .prose(
            "broken_style",
            "
            Broken {{ style }}.

            ```lua
            var.broken_saw = prose
            error('the style broke')
            ```",
        );
    let trace = require_case(source.build()).args("readers").run();
    assert_eq!(
        trace.text(),
        "First one.|First one.|Write in a terse voice for readers.|false|false|Broken terse.\
         |Second three.",
        "the memo the caller read survives both loads, and an unread prose renders \
         from the caller's own template after a failed load"
    );
}
