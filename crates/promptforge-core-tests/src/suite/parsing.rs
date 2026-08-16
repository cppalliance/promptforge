//! Public-parser contracts: valid fixtures expose their author-shaped structure
//! and invalid fixtures report their exact [`ParseErrorKind`] and message.

use std::num::NonZeroU32;

use promptforge_core::observe::NullObserver;
use promptforge_core::parser::{LuaProgram, MaxToolIterations, ParseErrorKind, Prompt};

struct ValidFixture {
    name: &'static str,
    source: &'static str,
    verify: fn(&Prompt),
}

const VALID_FIXTURES: &[ValidFixture] = &[
    ValidFixture {
        name: "valid/minimal.md",
        source: include_str!("../../prompts/valid/minimal.md"),
        verify: verify_minimal,
    },
    ValidFixture {
        name: "valid/shared-library.md",
        source: include_str!("../../prompts/valid/shared-library.md"),
        verify: verify_shared_library,
    },
    ValidFixture {
        name: "valid/prologue-prose-epilog.md",
        source: include_str!("../../prompts/valid/prologue-prose-epilog.md"),
        verify: verify_prologue_prose_epilog,
    },
];

struct InvalidFixture {
    name: &'static str,
    source: &'static str,
    kind: ParseErrorKind,
    message_fragment: &'static str,
}

const INVALID_FIXTURES: &[InvalidFixture] = &[
    InvalidFixture {
        name: "invalid/missing-h1.md",
        source: include_str!("../../prompts/invalid/missing-h1.md"),
        kind: ParseErrorKind::Structure,
        message_fragment: "requires an H1",
    },
    InvalidFixture {
        name: "invalid/removed-lua-prompt.md",
        source: include_str!("../../prompts/invalid/removed-lua-prompt.md"),
        kind: ParseErrorKind::Fence,
        message_fragment: "`lua prompt` fence form was removed",
    },
    InvalidFixture {
        name: "invalid/malformed-epilog.md",
        source: include_str!("../../prompts/invalid/malformed-epilog.md"),
        kind: ParseErrorKind::Lua,
        message_fragment: "section `Transform` epilog",
    },
    InvalidFixture {
        name: "invalid/list-h3-non-list-content.md",
        source: include_str!("../../prompts/invalid/list-h3-non-list-content.md"),
        kind: ParseErrorKind::List,
        message_fragment: "empty bullet item",
    },
];

#[test]
fn valid_prompt_files_parse_through_the_public_api() {
    for fixture in VALID_FIXTURES {
        let prompt = Prompt::parse(fixture.source, fixture.name, &NullObserver::default())
            .unwrap_or_else(|error| panic!("fixture {} failed to parse: {error}", fixture.name));
        // Call the verifier directly so its own assertion and source line remain
        // the reported failure rather than a generic wrapper.
        (fixture.verify)(&prompt);
    }
}

#[test]
fn invalid_prompt_files_report_public_error_contracts() {
    for fixture in INVALID_FIXTURES {
        let Err(error) = Prompt::parse(fixture.source, fixture.name, &NullObserver::default())
        else {
            panic!("fixture {} unexpectedly parsed", fixture.name);
        };
        assert_eq!(
            error.kind(),
            fixture.kind,
            "fixture {} returned the wrong error kind: {error:?}",
            fixture.name
        );
        assert!(
            error.to_string().contains(fixture.message_fragment),
            "fixture {} error did not contain {:?}: {error}",
            fixture.name,
            fixture.message_fragment
        );
    }
}

fn verify_minimal(prompt: &Prompt) {
    assert_eq!(prompt.frontmatter().name(), "test");
    assert_eq!(prompt.frontmatter().description(), "minimum valid");
    assert_eq!(prompt.frontmatter().promptforge(), Some(1));
    assert_eq!(prompt.title(), "Test");
    assert!(prompt.replay().is_none());
    assert!(prompt.h1_blocks().is_empty());
    assert_eq!(prompt.sections().len(), 1);
    let entry = prompt.entry().expect("fixture has sections");
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
    assert_eq!(prompt.frontmatter().promptforge(), Some(1));
    assert_eq!(prompt.title(), "Shared Library");
    assert_eq!(
        prompt.replay().map(LuaProgram::source),
        Some("function normalize(value)\n    return string.lower(value)\nend")
    );
    assert_eq!(prompt.sections().len(), 2);

    let prepare = &prompt.sections()[0];
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

    let finish = &prompt.sections()[1];
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
    assert_eq!(prompt.frontmatter().promptforge(), Some(1));
    assert_eq!(
        prompt.frontmatter().max_tool_iterations(),
        MaxToolIterations::Limit(NonZeroU32::new(3).expect("3 is non-zero"))
    );
    assert_eq!(prompt.title(), "Phase Boundaries");
    assert!(prompt.replay().is_none());
    assert_eq!(prompt.sections().len(), 2);

    let transform = prompt.entry().expect("fixture has sections");
    assert_eq!(transform.name(), "Transform");
    assert_eq!(
        transform.prologue().map(LuaProgram::source),
        Some("var.subject = args")
    );
    assert_eq!(transform.prose(), "Write about {{ var.subject }}.");
    assert_eq!(
        transform.epilog().map(LuaProgram::source),
        Some("return reply")
    );
    assert!(transform.children().is_empty());

    let fallback = &prompt.sections()[1];
    assert_eq!(fallback.name(), "Fallback");
    assert_eq!(fallback.prose(), "This section has prose only.");
    assert!(fallback.prologue().is_none());
    assert!(fallback.epilog().is_none());
}
