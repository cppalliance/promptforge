//! Boot refuses an incomplete catalog and prints every fault at once.
//!
//! This runs the real binary because Decision 10 is about the process's exit
//! status and what it writes to standard error before it leaves; nothing short
//! of spawning it can show that. It sits in its own module so the stdio
//! transport tests stay about the transport.

#![expect(
    clippy::expect_used,
    reason = "test setup panics on failure, which is the desired behavior"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

/// A prompt that runs offline.
const ECHO: &str = "---\nname: echo\ndescription: Returns its argument\npromptforge: 1\n---\n\n\
# Test prompt\n\n## Main\n\n```lua\nreturn args\n```\n";

/// A prompt whose frontmatter name is not a legal tool name.
const SHOUTY: &str = "---\nname: Shouty\ndescription: An illegal tool name\npromptforge: 1\n---\n\n\
# Test prompt\n\n## Main\n\n```lua\nreturn args\n```\n";

/// A prompt that declares itself one and then carries no section.
const NO_SECTIONS: &str =
    "---\nname: hollow\ndescription: No sections at all\npromptforge: 1\n---\n\n# Only a title\n";

/// How long the child may take to refuse before the test kills it and fails.
const PATIENCE: Duration = Duration::from_secs(20);

/// Writes a prompts directory carrying `prompts` and the configuration that
/// globs it, returning the configuration's path. The bind address is inert here
/// because boot refuses before any transport is reached.
fn fixture(root: &Path, prompts: &[(&str, &str)]) -> PathBuf {
    let directory = root.join("prompts");
    fs::create_dir(&directory).expect("create the prompts directory");
    for (file, contents) in prompts {
        fs::write(directory.join(file), contents).expect("write the fixture prompt");
    }
    let config = root.join("prompts.toml");
    fs::write(
        &config,
        format!(
            "[server]\nbind = \"127.0.0.1:0\"\ntoken = \"unused\"\n\n\
             [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\nkey = \"gw\"\n\n\
             [paths]\nprompts = '{}'\n\n\
             [catalog]\ninclude = [\"*.md\"]\n",
            directory.display()
        ),
    )
    .expect("write the fixture configuration");
    config
}

#[tokio::test]
async fn a_catalog_with_two_faults_refuses_to_serve_and_prints_both() {
    // Decision 10: boot either produces a complete catalog or the process
    // refuses to start, and every fault is printed before the nonzero exit so an
    // operator fixes them in one pass rather than one restart each.
    let dir = tempfile::tempdir().expect("create a temporary directory");
    let config = fixture(
        dir.path(),
        &[
            ("echo.md", ECHO),
            ("upper.md", SHOUTY),
            ("hollow.md", NO_SECTIONS),
        ],
    );

    // `kill_on_drop` turns the timeout below into a clean kill: on elapse the
    // `wait_with_output` future is dropped, which drops the child and signals
    // it, so a hung boot fails this test alone instead of hanging the suite.
    let child = Command::new(env!("CARGO_BIN_EXE_promptforge-mcp-server"))
        .arg("serve")
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn the server");

    let output = tokio::time::timeout(PATIENCE, child.wait_with_output())
        .await
        .expect("boot refuses within the wait")
        .expect("collect the server's output");

    assert!(
        !output.status.success(),
        "an incomplete catalog is refused: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("catalog has 2 fault"), "{stderr}");
    for named in ["upper.md", "Shouty", "hollow.md"] {
        assert!(
            stderr.contains(named),
            "every fault names its prompt and its file: {stderr}"
        );
    }
}
