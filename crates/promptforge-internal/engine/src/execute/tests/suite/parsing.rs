//! Public-parser contracts: valid fixtures expose the structure their author
//! wrote, down to the Lua programs and tool-iteration limit behind it. The
//! invalid fixtures' error contracts sit in the facade's suite.

use std::num::NonZeroU32;

use promptforge_parser::detail;

use crate::parser::{LuaProgram, Prompt};

struct ValidFixture {
    name: &'static str,
    source: &'static str,
    verify: fn(&Prompt),
}

const VALID_FIXTURES: &[ValidFixture] = &[
    ValidFixture {
        name: "valid/minimal.md",
        source: include_str!("../../../../tests/prompts/valid/minimal.md"),
        verify: verify_minimal,
    },
    ValidFixture {
        name: "valid/shared-library.md",
        source: include_str!("../../../../tests/prompts/valid/shared-library.md"),
        verify: verify_shared_library,
    },
    ValidFixture {
        name: "valid/prologue-prose-epilog.md",
        source: include_str!("../../../../tests/prompts/valid/prologue-prose-epilog.md"),
        verify: verify_prologue_prose_epilog,
    },
];

#[test]
fn valid_prompt_files_parse_through_the_public_api() {
    for fixture in VALID_FIXTURES {
        let prompt = Prompt::parse(fixture.source, fixture.name)
            .0
            .unwrap_or_else(|error| panic!("fixture {} failed to parse: {error}", fixture.name));
        // Call the verifier directly so its own assertion and source line remain
        // the reported failure rather than a generic wrapper.
        (fixture.verify)(&prompt);
    }
}

fn verify_minimal(prompt: &Prompt) {
    assert_eq!(prompt.frontmatter().name(), "test");
    assert_eq!(prompt.frontmatter().description(), "minimum valid");
    assert_eq!(prompt.frontmatter().promptforge(), Some(0));
    assert_eq!(prompt.title(), "Test");
    assert!(detail::replay(prompt).is_none());
    assert!(detail::h1_blocks(prompt).is_empty());
    assert_eq!(detail::sections(prompt).len(), 1);
    let entry = detail::entry(prompt).expect("fixture has sections");
    assert_eq!(entry.name(), "Run");
    assert_eq!(entry.level(), 2);
    assert_eq!(entry.prose(), "Done.");
    assert!(entry.prologue().is_none());
    assert!(entry.epilog().is_none());
}

fn verify_shared_library(prompt: &Prompt) {
    assert_eq!(prompt.frontmatter().name(), "shared_library");
    assert_eq!(
        prompt.frontmatter().description(),
        "Exercise an H1 shared library and nested author prose"
    );
    assert_eq!(prompt.frontmatter().promptforge(), Some(0));
    assert_eq!(prompt.title(), "Shared Library");
    assert_eq!(
        detail::replay(prompt).map(LuaProgram::source),
        Some("function normalize(value)\n    return string.lower(value)\nend")
    );
    assert_eq!(detail::sections(prompt).len(), 2);

    let prepare = &detail::sections(prompt)[0];
    assert_eq!(prepare.name(), "Prepare");
    assert_eq!(prepare.level(), 2);
    assert_eq!(prepare.prose(), "Normalize the supplied subject.");
    assert!(prepare.prologue().is_none());
    assert!(prepare.epilog().is_none());
    assert_eq!(prepare.children().len(), 1);
    assert_eq!(prepare.children()[0].name(), "Author note");
    assert_eq!(prepare.children()[0].level(), 3);
    assert_eq!(
        prepare.children()[0].prose(),
        "This nested prose remains attached to Prepare."
    );

    let finish = &detail::sections(prompt)[1];
    assert_eq!(finish.name(), "Finish");
    assert_eq!(finish.prose(), "Return the normalized subject.");
    assert!(finish.children().is_empty());
}

fn verify_prologue_prose_epilog(prompt: &Prompt) {
    assert_eq!(prompt.frontmatter().name(), "phase_boundaries");
    assert_eq!(
        prompt.frontmatter().description(),
        "Exercise an author-shaped prologue, prose, and epilog"
    );
    assert_eq!(prompt.frontmatter().promptforge(), Some(0));
    assert_eq!(
        prompt.frontmatter().max_tool_iterations(),
        NonZeroU32::new(3)
    );
    assert_eq!(prompt.title(), "Phase Boundaries");
    assert!(detail::replay(prompt).is_none());
    assert_eq!(detail::sections(prompt).len(), 2);

    let transform = detail::entry(prompt).expect("fixture has sections");
    assert_eq!(transform.name(), "Transform");
    assert_eq!(
        transform.prologue().map(LuaProgram::source),
        Some("var.subject = args")
    );
    assert_eq!(transform.prose(), "Write about {{ var.subject }}.");
    assert_eq!(
        transform.epilog().map(LuaProgram::source),
        Some("return models.infer(prose)")
    );
    assert!(transform.children().is_empty());

    let fallback = &detail::sections(prompt)[1];
    assert_eq!(fallback.name(), "Fallback");
    assert_eq!(fallback.prose(), "This section has prose only.");
    assert!(fallback.prologue().is_none());
    assert!(fallback.epilog().is_none());
}
