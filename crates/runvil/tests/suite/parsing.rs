//! Public-parser contracts: invalid fixtures report their exact
//! [`ParseErrorKind`] and message.

use std::fs;
use std::path::Path;

use runvil::{ParseErrorKind, Prompt};

use super::support::TestResult;

const INVALID_FIXTURES: [(&str, ParseErrorKind, &str); 6] = [
    ("missing-h1", ParseErrorKind::Structure, "requires an H1"),
    (
        "unrecognized-lua-label",
        ParseErrorKind::Fence,
        "unrecognized fence label",
    ),
    (
        "lua-fence-opener",
        ParseErrorKind::Fence,
        "fence opening line must be exactly",
    ),
    (
        "lua-fence-closer",
        ParseErrorKind::Fence,
        "fence closing line must be exactly",
    ),
    (
        "malformed-epilog",
        ParseErrorKind::Lua,
        "section `Transform` epilog",
    ),
    (
        "list-h3-non-list-content",
        ParseErrorKind::List,
        "empty bullet item",
    ),
];

#[test]
fn invalid_prompt_files_report_public_error_contracts() -> TestResult {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/prompts/invalid");
    for (stem, kind, message_fragment) in INVALID_FIXTURES {
        let name = format!("invalid/{stem}.md");
        let source = fs::read_to_string(dir.join(format!("{stem}.md")))?;
        let Err(error) = Prompt::parse(&source, &name).0 else {
            return Err(format!("fixture {name} unexpectedly parsed").into());
        };
        assert_eq!(
            error.kind(),
            kind,
            "fixture {name} returned the wrong error kind: {error:?}"
        );
        assert!(
            error.to_string().contains(message_fragment),
            "fixture {name} error did not contain {message_fragment:?}: {error}"
        );
    }
    Ok(())
}
