//! Public-parser contracts: valid fixtures expose the structure their author
//! wrote, down to the Lua programs and tool-iteration limit behind it, and
//! every positive fixture parses offline, as does every shipped prompt. The
//! invalid fixtures' error contracts sit in the facade's suite.

use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use runvil_lua::LuaProgram;
use runvil_parser::detail;

use crate::parser::Prompt;

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
        name: "valid/require-library.md",
        source: include_str!("../../../../tests/prompts/valid/require-library.md"),
        verify: verify_require_library,
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
    assert_eq!(prompt.frontmatter().runvil(), Some(0));
    assert_eq!(prompt.title(), "Test");
    assert!(detail::h1_blocks(prompt).is_empty());
    assert_eq!(detail::sections(prompt).len(), 1);
    let entry = detail::entry(prompt).expect("fixture has sections");
    assert_eq!(entry.name(), "Run");
    assert_eq!(entry.level(), 2);
    assert_eq!(entry.prose(), "Done.");
    assert!(entry.prologue().is_none());
    assert!(entry.epilog().is_none());
}

fn verify_require_library(prompt: &Prompt) {
    assert_eq!(prompt.frontmatter().name(), "require_library");
    assert_eq!(
        prompt.frontmatter().description(),
        "Exercise a required library section and nested author prose"
    );
    assert_eq!(prompt.frontmatter().runvil(), Some(0));
    assert_eq!(prompt.title(), "Require Library");
    assert_eq!(detail::sections(prompt).len(), 3);

    let prepare = &detail::sections(prompt)[0];
    assert_eq!(prepare.name(), "Prepare");
    assert_eq!(prepare.level(), 2);
    assert_eq!(prepare.prose(), "Normalize the supplied subject.");
    assert!(prepare.prologue().is_none());
    assert_eq!(
        prepare.epilog().map(LuaProgram::source),
        Some("require '## helpers'\nvar.subject = normalize(args)")
    );
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
    assert_eq!(
        finish.epilog().map(LuaProgram::source),
        Some("require '## helpers'\nreturn normalize(var.subject)")
    );
    assert!(finish.children().is_empty());

    let helpers = &detail::sections(prompt)[2];
    assert_eq!(helpers.name(), "helpers");
    assert_eq!(helpers.level(), 2);
    assert_eq!(
        helpers.prologue().map(LuaProgram::source),
        Some("function normalize(value)\n    return string.lower(value)\nend")
    );
    assert!(helpers.epilog().is_none());
}

fn verify_prologue_prose_epilog(prompt: &Prompt) {
    assert_eq!(prompt.frontmatter().name(), "phase_boundaries");
    assert_eq!(
        prompt.frontmatter().description(),
        "Exercise an author-shaped prologue, prose, and epilog"
    );
    assert_eq!(prompt.frontmatter().runvil(), Some(0));
    assert_eq!(
        prompt.frontmatter().max_tool_iterations(),
        NonZeroU32::new(3)
    );
    assert_eq!(prompt.title(), "Phase Boundaries");
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

const POSITIVE_PARSE: &str = "fixture-positive-prompts";

fn collect_markdown(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read repository prompt directory") {
        let path = entry.expect("read repository prompt entry").path();
        if path.is_dir() {
            collect_markdown(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

fn every_prompt_under_parses(directory: &Path) {
    let mut files = Vec::new();
    collect_markdown(directory, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "{} holds no prompts to check",
        directory.display()
    );

    for path in files {
        let source = fs::read_to_string(&path).expect("read prompt fixture");
        Prompt::parse(&source, POSITIVE_PARSE)
            .0
            .unwrap_or_else(|error| panic!("{} must parse: {error}", path.display()));
    }
}

/// The `valid/` and `execution/` fixture directories are positive examples,
/// so a fixture no test happens to load still has to track the language
/// (`invalid/` is excluded by construction: those are meant to fail).
#[test]
fn every_positive_fixture_parses_offline() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/prompts");
    every_prompt_under_parses(&fixtures.join("valid"));
    every_prompt_under_parses(&fixtures.join("execution"));
}

#[test]
fn every_shipped_prompt_parses_offline() {
    every_prompt_under_parses(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../prompts"));
}
