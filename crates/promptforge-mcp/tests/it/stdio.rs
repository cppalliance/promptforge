//! The stdio transport, driven the way a local harness drives it.
//!
//! This one runs the real binary as a child process, because what is being
//! asserted is that `serve --stdio` speaks line-delimited JSON-RPC on this
//! process's own standard input and output, binds nothing, and asks for no
//! token. Nothing short of spawning it can show that.

#![expect(
    clippy::expect_used,
    reason = "test setup panics on failure, which is the desired behavior"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// A prompt that runs offline, published as its own tool.
const ECHO: &str = "---\nname: echo\ndescription: Returns its argument\nversion: 1\npromptforge: 1\n---\n\n\
## Main\n\n```lua\nreturn args\n```\n";

/// A prompt whose frontmatter name is not a legal tool name.
const SHOUTY: &str = "---\nname: Shouty\ndescription: An illegal tool name\nversion: 1\npromptforge: 1\n---\n\n\
## Main\n\n```lua\nreturn args\n```\n";

/// A prompt that declares itself one and then carries no section.
const NO_SECTIONS: &str = "---\nname: hollow\ndescription: No sections at all\nversion: 1\npromptforge: 1\n---\n\n# Only a title\n";

/// How long any single line may take to arrive before the test gives up.
const PATIENCE: Duration = Duration::from_secs(20);

/// Writes a prompts directory carrying `prompts` and the configuration that
/// globs it, returning the configuration's path.
///
/// The configuration names a bind address, which the stdio transport must
/// ignore. Port 0 would be bound to something arbitrary if stdio bound anything
/// at all; it binds nothing, so the line is inert.
fn fixture(root: &Path, prompts: &[(&str, &str)]) -> PathBuf {
    fixture_with(
        root,
        prompts,
        "bind = \"127.0.0.1:0\"\ntoken = \"unused-on-stdio\"\n",
    )
}

/// The same, with `server_lines` as the whole of the `[server]` table.
fn fixture_with(root: &Path, prompts: &[(&str, &str)], server_lines: &str) -> PathBuf {
    let directory = root.join("prompts");
    fs::create_dir(&directory).expect("create the prompts directory");
    for (file, contents) in prompts {
        fs::write(directory.join(file), contents).expect("write the fixture prompt");
    }
    let config = root.join("prompts.toml");
    fs::write(
        &config,
        format!(
            "[server]\n{server_lines}\n\
             [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"gw\"\n\n\
             [paths]\nprompts = '{}'\n\n\
             [catalog]\ninclude = [\"*.md\"]\ndefault_expose = \"tool\"\n",
            directory.display()
        ),
    )
    .expect("write the fixture configuration");
    config
}

/// The child, its pipes, and the temporary directory both read from.
struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _dir: tempfile::TempDir,
}

impl Session {
    /// Spawns `promptforge-mcp serve --stdio <config>` over a one-prompt
    /// catalog. The configuration names a bind address the transport must
    /// ignore.
    fn spawn() -> Session {
        let dir = tempfile::tempdir().expect("create a temporary directory");
        let config = fixture(dir.path(), &[("echo.md", ECHO)]);
        Session::spawn_at(dir, &config)
    }

    /// Spawns the same command over an already-written configuration.
    fn spawn_at(dir: tempfile::TempDir, config: &Path) -> Session {
        let mut child = Command::new(env!("CARGO_BIN_EXE_promptforge-mcp"))
            .arg("serve")
            .arg("--stdio")
            .arg(config)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn the server");
        let stdin = child.stdin.take().expect("the child's standard input");
        let stdout = child.stdout.take().expect("the child's standard output");
        Session {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            _dir: dir,
        }
    }

    /// Writes one JSON-RPC message as a line.
    async fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_string(message).expect("serialize the request");
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .expect("write to the child");
        self.stdin.flush().await.expect("flush the child's input");
    }

    /// Reads one JSON-RPC message from the next line.
    async fn receive(&mut self) -> Value {
        let mut line = String::new();
        let read = tokio::time::timeout(PATIENCE, self.stdout.read_line(&mut line))
            .await
            .expect("the server answers within the wait")
            .expect("read from the child");
        assert!(read > 0, "the server closed its output");
        serde_json::from_str(&line).expect("the server writes one JSON message per line")
    }
}

#[tokio::test]
async fn stdio_completes_initialize_and_lists_its_tools() {
    let mut session = Session::spawn();

    session
        .send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "stdio-smoke", "version": "0" },
            },
        }))
        .await;
    let initialized = session.receive().await;
    assert_eq!(initialized["id"], json!(1));
    assert_eq!(
        initialized["result"]["serverInfo"]["name"],
        json!("promptforge-mcp"),
        "no token was presented and the handshake completed anyway"
    );

    session
        .send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    session
        .send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
        .await;
    let listed = session.receive().await;
    assert_eq!(listed["id"], json!(2));
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools/list answers with an array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(
        names.contains(&"echo"),
        "the catalog's one direct prompt is published: {names:?}"
    );

    session.child.kill().await.expect("stop the server");
}

#[tokio::test]
async fn stdio_serves_a_configuration_that_carries_no_token() {
    // `[server].token` is a property of the HTTP surface. A local install is
    // spawned by its harness and reads no token at all, so requiring one in the
    // file stopped that install over a credential it never uses.
    let dir = tempfile::tempdir().expect("create a temporary directory");
    let config = fixture_with(dir.path(), &[("echo.md", ECHO)], "");
    let mut session = Session::spawn_at(dir, &config);

    session
        .send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "stdio-no-token", "version": "0" },
            },
        }))
        .await;
    let initialized = session.receive().await;
    assert_eq!(initialized["id"], json!(1));
    assert_eq!(
        initialized["result"]["serverInfo"]["name"],
        json!("promptforge-mcp"),
        "the file carries no [server].token and stdio serves anyway"
    );

    session.child.kill().await.expect("stop the server");
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

    let output = Command::new(env!("CARGO_BIN_EXE_promptforge-mcp"))
        .arg("serve")
        .arg(&config)
        .stdin(Stdio::null())
        .output()
        .await
        .expect("run the server");

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
