//! A required section stays an ordinary section: when the walk reaches a
//! library it runs under the section rules, and the two placements that
//! keep a returning library away from the walk.

use crate::RunErrorKind;

use super::*;

#[test]
fn the_walk_runs_a_globals_only_library_harmlessly() {
    let source = prompt(
        "",
        &[
            (
                "Main",
                "
                require '## helpers'
                var.greeting = greet('main')",
            ),
            (
                "helpers",
                "function greet(name) return 'hello ' .. name end",
            ),
            ("Last", "return var.greeting .. '|' .. tostring(greet)"),
        ],
    );
    let trace = require_case(source).run();
    assert_eq!(
        trace.text(),
        "hello main|nil",
        "the library's globals stay in each VM that ran it"
    );
}

#[test]
fn the_walk_fails_the_run_on_a_library_that_returns_a_table() {
    let source = prompt(
        "",
        &[
            ("Main", "var.name = (require '## lib').name"),
            ("lib", "return { name = 'lib' }"),
        ],
    );
    let trace = require_case(source).run();
    let error = trace.error();
    assert_eq!(error.kind(), RunErrorKind::Lua, "{error}");
    assert!(
        error
            .to_string()
            .contains("cannot return a table as a result"),
        "the walk applies the section return rule to the library: {error}"
    );
}

#[test]
fn the_walk_ends_the_chain_with_a_library_that_returns_a_scalar() {
    let source = prompt(
        "",
        &[
            ("Main", "var.from_main = require '## lib'"),
            ("lib", "return 'from lib'"),
            ("Last", "error('the chain ends at the library')"),
        ],
    );
    let trace = require_case(source).run();
    assert_eq!(
        trace.text(),
        "from lib",
        "the library's scalar ends the chain"
    );
}

#[test]
fn a_returning_library_after_a_return_or_under_its_user_stays_off_the_walk() {
    let after = prompt(
        "",
        &[
            ("Main", "return (require '## lib').name"),
            ("lib", "return { name = 'after' }"),
        ],
    );
    let nested = prompt(
        "",
        &[
            ("Main", "var.name = (require '### lib').name"),
            ("### lib", "return { name = 'nested' }"),
            ("Last", "return var.name"),
        ],
    );
    for (source, expected) in [(after, "after"), (nested, "nested")] {
        let trace = require_case(source).run();
        assert_eq!(trace.text(), expected, "the walk never reaches the library");
    }
}
