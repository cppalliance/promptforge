use promptforge_core::Error;
use promptforge_core::lua::LuaProgram;
use promptforge_core::observe::NullObserver;
use promptforge_core::parser::Prompt;

struct ValidFixture {
    name: &'static str,
    source: &'static str,
    verify: fn(&Prompt),
}

const VALID_FIXTURES: &[ValidFixture] = &[
    ValidFixture {
        name: "valid/minimal.md",
        source: include_str!("../prompts/valid/minimal.md"),
        verify: verify_minimal,
    },
    ValidFixture {
        name: "valid/shared-library.md",
        source: include_str!("../prompts/valid/shared-library.md"),
        verify: verify_shared_library,
    },
    ValidFixture {
        name: "valid/preamble-prose-epilog.md",
        source: include_str!("../prompts/valid/preamble-prose-epilog.md"),
        verify: verify_preamble_prose_epilog,
    },
];

#[derive(Clone, Copy, Debug)]
enum ErrorKind {
    Parse,
    LuaCompile,
}

struct InvalidFixture {
    name: &'static str,
    source: &'static str,
    kind: ErrorKind,
    message_fragment: &'static str,
}

const INVALID_FIXTURES: &[InvalidFixture] = &[
    InvalidFixture {
        name: "invalid/missing-h1.md",
        source: include_str!("../prompts/invalid/missing-h1.md"),
        kind: ErrorKind::Parse,
        message_fragment: "requires an H1",
    },
    InvalidFixture {
        name: "invalid/misplaced-shared-lua.md",
        source: include_str!("../prompts/invalid/misplaced-shared-lua.md"),
        kind: ErrorKind::Parse,
        message_fragment: "must immediately follow the H1",
    },
    InvalidFixture {
        name: "invalid/malformed-epilog.md",
        source: include_str!("../prompts/invalid/malformed-epilog.md"),
        kind: ErrorKind::LuaCompile,
        message_fragment: "section `Transform` epilog",
    },
];

#[test]
fn valid_prompt_files_parse_through_the_public_api() {
    for fixture in VALID_FIXTURES {
        let prompt = Prompt::parse(fixture.source, fixture.name, &NullObserver)
            .unwrap_or_else(|error| panic!("fixture {} failed to parse: {error}", fixture.name));
        let verification = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (fixture.verify)(&prompt);
        }));
        assert!(
            verification.is_ok(),
            "fixture {} did not match its expected public structure",
            fixture.name
        );
    }
}

#[test]
fn invalid_prompt_files_report_public_error_contracts() {
    for fixture in INVALID_FIXTURES {
        let Err(error) = Prompt::parse(fixture.source, fixture.name, &NullObserver) else {
            panic!("fixture {} unexpectedly parsed", fixture.name);
        };

        let variant_matches = matches!(
            (fixture.kind, &error),
            (ErrorKind::Parse, Error::Parse(_)) | (ErrorKind::LuaCompile, Error::LuaCompile { .. })
        );
        assert!(
            variant_matches,
            "fixture {} returned the wrong error variant: expected {:?}, got {error:?}",
            fixture.name, fixture.kind
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
    assert_eq!(prompt.frontmatter.name, "test");
    assert_eq!(prompt.frontmatter.description, "minimum valid");
    assert_eq!(prompt.frontmatter.promptforge, Some(1));
    assert_eq!(prompt.title, "Test");
    assert!(prompt.shared.is_none());
    assert!(prompt.description_text.is_empty());
    assert_eq!(prompt.sections.len(), 1);
    assert_eq!(prompt.entry().name, "Run");
    assert_eq!(prompt.entry().level, 2);
    assert_eq!(prompt.entry().prose, "Done.");
    assert!(prompt.entry().preamble.is_none());
    assert!(prompt.entry().epilog.is_none());
}

fn verify_shared_library(prompt: &Prompt) {
    assert_eq!(prompt.frontmatter.name, "shared_library");
    assert_eq!(
        prompt.frontmatter.description,
        "Exercise an H1 shared library and nested author prose"
    );
    assert_eq!(prompt.frontmatter.promptforge, Some(1));
    assert_eq!(prompt.title, "Shared Library");
    assert_eq!(
        prompt.shared.as_ref().map(LuaProgram::source),
        Some("function normalize(value)\n    return string.lower(value)\nend")
    );
    assert_eq!(
        prompt.description_text,
        "The shared helper is available to each executable section."
    );
    assert_eq!(prompt.sections.len(), 2);

    let prepare = &prompt.sections[0];
    assert_eq!(prepare.name, "Prepare");
    assert_eq!(prepare.level, 2);
    assert_eq!(prepare.prose, "Normalize the supplied subject.");
    assert!(prepare.preamble.is_none());
    assert!(prepare.epilog.is_none());
    assert_eq!(prepare.children.len(), 1);
    assert_eq!(prepare.children[0].name, "Author note");
    assert_eq!(prepare.children[0].level, 3);
    assert_eq!(
        prepare.children[0].prose,
        "This nested prose remains attached to Prepare."
    );

    let finish = &prompt.sections[1];
    assert_eq!(finish.name, "Finish");
    assert_eq!(finish.prose, "Return the normalized subject.");
    assert!(finish.children.is_empty());
}

fn verify_preamble_prose_epilog(prompt: &Prompt) {
    assert_eq!(prompt.frontmatter.name, "phase_boundaries");
    assert_eq!(
        prompt.frontmatter.description,
        "Exercise an author-shaped preamble, prose, and epilog"
    );
    assert_eq!(prompt.frontmatter.promptforge, Some(1));
    assert_eq!(
        prompt.frontmatter.default_return.as_deref(),
        Some("fallback")
    );
    assert_eq!(prompt.frontmatter.max_tool_iterations, Some(3));
    assert_eq!(prompt.title, "Phase Boundaries");
    assert!(prompt.shared.is_none());
    assert_eq!(prompt.description_text, "Transform one model response.");
    assert_eq!(prompt.sections.len(), 2);

    let transform = prompt.entry();
    assert_eq!(transform.name, "Transform");
    assert_eq!(
        transform.preamble.as_ref().map(LuaProgram::source),
        Some("var.subject = args")
    );
    assert_eq!(transform.prose, "Write about {{ var.subject }}.");
    assert_eq!(
        transform.epilog.as_ref().map(LuaProgram::source),
        Some("return reply")
    );
    assert!(transform.children.is_empty());

    let fallback = &prompt.sections[1];
    assert_eq!(fallback.name, "Fallback");
    assert_eq!(fallback.prose, "This section has prose only.");
    assert!(fallback.preamble.is_none());
    assert!(fallback.epilog.is_none());
}
