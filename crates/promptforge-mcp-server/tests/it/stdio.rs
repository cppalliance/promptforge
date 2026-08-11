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
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// A prompt that runs offline.
const ECHO: &str = "---\nname: echo\ndescription: Returns its argument\npromptforge: 1\n---\n\n\
# Test prompt\n\n## Main\n\n```lua\nreturn args\n```\n";

/// How long any single line may take to arrive before the test gives up.
const PATIENCE: Duration = Duration::from_secs(20);

/// The most bytes one line may carry before the test treats the server as
/// misbehaving. Every real frame here is a few hundred bytes; the cap only
/// exists so a server that never sends a newline cannot make the read allocate
/// without bound.
const LINE_LIMIT: u64 = 1 << 20;

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
             [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\nkey = \"gw\"\n\n\
             [paths]\nprompts = '{}'\n\n\
             [catalog]\ninclude = [\"*.md\"]\n",
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
    /// Spawns `promptforge-mcp-server serve --stdio <config>` over a one-prompt
    /// catalog. The configuration names a bind address the transport must
    /// ignore.
    fn spawn() -> Session {
        let dir = tempfile::tempdir().expect("create a temporary directory");
        let config = fixture(dir.path(), &[("echo.md", ECHO)]);
        Session::spawn_at(dir, &config)
    }

    /// Spawns the same command over an already-written configuration.
    fn spawn_at(dir: tempfile::TempDir, config: &Path) -> Session {
        let mut child = Command::new(env!("CARGO_BIN_EXE_promptforge-mcp-server"))
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
    ///
    /// The read is bounded twice over: by `PATIENCE` in time and by
    /// [`LINE_LIMIT`] in bytes, so neither a silent server nor one that never
    /// terminates a line can hang or exhaust the test.
    async fn receive(&mut self) -> Value {
        let mut line = String::new();
        let mut limited = (&mut self.stdout).take(LINE_LIMIT);
        let read = tokio::time::timeout(PATIENCE, limited.read_line(&mut line))
            .await
            .expect("the server answers within the wait")
            .expect("read from the child");
        assert!(read > 0, "the server closed its output");
        assert!(
            line.ends_with('\n'),
            "the server's line stayed within {LINE_LIMIT} bytes"
        );
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
        json!("promptforge-mcp-server"),
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
        names.contains(&"run_prompt"),
        "the runner is how the catalog is reached: {names:?}"
    );
    assert!(
        !names.contains(&"echo"),
        "a prompt is never published as a tool of its own: {names:?}"
    );

    session.child.kill().await.expect("stop the server");
}

#[tokio::test]
async fn stdio_leaves_the_configured_bind_address_unlistened() {
    use std::net::{SocketAddr, TcpListener};

    use tokio::net::TcpStream;

    // Reserve a loopback port, learn its number, then release it. The address
    // is now free and is what the configuration names; if `--stdio` honored the
    // bind, the child would be listening here by the time the handshake returns.
    let reserved = TcpListener::bind("127.0.0.1:0").expect("reserve a loopback port");
    let addr: SocketAddr = reserved.local_addr().expect("read the reserved address");
    drop(reserved);

    let dir = tempfile::tempdir().expect("create a temporary directory");
    let config = fixture_with(
        dir.path(),
        &[("echo.md", ECHO)],
        &format!("bind = \"{addr}\"\ntoken = \"unused-on-stdio\"\n"),
    );
    let mut session = Session::spawn_at(dir, &config);

    // Drive initialize to completion so the child is fully up: whatever it was
    // ever going to bind, it has bound by the time this reply arrives.
    session
        .send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "stdio-bind-check", "version": "0" },
            },
        }))
        .await;
    let initialized = session.receive().await;
    assert_eq!(initialized["id"], json!(1));

    // Observe the invariant directly: connect to the configured address and
    // require the connection to be refused. A refusal proves nothing is
    // listening there; a success would prove the transport bound its HTTP
    // surface. The attempt is bounded by `PATIENCE` so a stall cannot pass for
    // a refusal.
    //
    // Limitation: the reserved port is released before the child starts, so an
    // unrelated process could in principle claim it in that window. On loopback
    // with a freshly reserved ephemeral port this is vanishingly unlikely, and
    // it could only mask the fault, never manufacture one - an accepted
    // connection still fails the test.
    match tokio::time::timeout(PATIENCE, TcpStream::connect(addr)).await {
        Ok(Ok(_stream)) => {
            panic!("stdio bound {addr}: its HTTP surface is listening")
        }
        Ok(Err(_refused)) => {}
        Err(elapsed) => {
            panic!(
                "connecting to {addr} neither refused nor completed: {elapsed} within {PATIENCE:?}"
            )
        }
    }

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
        json!("promptforge-mcp-server"),
        "the file carries no [server].token and stdio serves anyway"
    );

    session.child.kill().await.expect("stop the server");
}
