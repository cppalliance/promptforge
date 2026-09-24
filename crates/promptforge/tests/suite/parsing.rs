//! Public-parser contracts: invalid fixtures report their exact
//! [`ParseErrorKind`] and message. The fixtures sit beside this suite
//! under `tests/prompts/invalid/`.

use promptforge::{ParseErrorKind, Prompt};

struct InvalidFixture {
    name: &'static str,
    source: &'static str,
    kind: ParseErrorKind,
    message_fragment: &'static str,
}

const INVALID_FIXTURES: &[InvalidFixture] = &[
    InvalidFixture {
        name: "invalid/missing-h1.md",
        source: include_str!("../prompts/invalid/missing-h1.md"),
        kind: ParseErrorKind::Structure,
        message_fragment: "requires an H1",
    },
    InvalidFixture {
        name: "invalid/removed-lua-prompt.md",
        source: include_str!("../prompts/invalid/removed-lua-prompt.md"),
        kind: ParseErrorKind::Fence,
        message_fragment: "`lua prompt` fence form was removed",
    },
    InvalidFixture {
        name: "invalid/malformed-epilog.md",
        source: include_str!("../prompts/invalid/malformed-epilog.md"),
        kind: ParseErrorKind::Lua,
        message_fragment: "section `Transform` epilog",
    },
    InvalidFixture {
        name: "invalid/list-h3-non-list-content.md",
        source: include_str!("../prompts/invalid/list-h3-non-list-content.md"),
        kind: ParseErrorKind::List,
        message_fragment: "empty bullet item",
    },
];

#[test]
fn invalid_prompt_files_report_public_error_contracts() {
    for fixture in INVALID_FIXTURES {
        let Err(error) = Prompt::parse(fixture.source, fixture.name).0 else {
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
