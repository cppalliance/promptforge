//! That the `prompts.toml` the repository ships resolves the prompts it ships,
//! and that a real session reaches them through the built-ins alone.
//!
//! The two are one artifact: a configuration that parses on its own says nothing
//! about whether the catalog it names is servable, and boot refuses an
//! incomplete catalog. This test is the only thing that keeps the shipped pair
//! honest, since every other catalog test writes its own prompts into a
//! temporary directory. It asserts from the client's side of an in-process
//! session, because that is the surface a harness actually reaches: `list_prompts`
//! is the whole shipped catalog, and `tools/list` is the fixed built-ins whatever
//! the catalog holds.
//!
//! Two things are supplied here that the process supplies at runtime. The
//! `${VAR}` values become literals, because setting an environment variable is
//! `unsafe` under edition 2024 and this workspace forbids unsafe. And the
//! prompts directory becomes absolute, because the shipped path is relative to
//! the working directory the server is started from - the repository root -
//! while a test runs from its own crate; it is rewritten in the TOML itself,
//! since the parsed configuration is opaque.

#![expect(
    clippy::expect_used,
    reason = "test setup panics on failure, which is the desired behavior"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use promptforge_mcp_server::{
    Catalog, CatalogHandle, Config, OnBroken, PreparedTools, PromptForgeServer,
};
use rmcp::model::{CallToolRequestParams, CallToolResponse};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};

/// The shipped configuration, read at compile time so a rename breaks the build
/// rather than the test.
const SHIPPED: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../prompts.toml"));

/// The names the repository ships, in the order the catalog reports them.
const SHIPPED_PROMPTS: [&str; 5] = [
    "analyst_example",
    "echo",
    "greet",
    "hello",
    "research_person",
];

/// The repository root, two levels above this crate.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root exists")
}

/// Replaces every `${VAR}` with a literal, leaving `$$` alone.
///
/// The values themselves are not what is under test - only `[server].token`
/// being non-blank is checked at load - so one placeholder serves for all of
/// them.
fn without_variables(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find("${") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let close = after.find('}').expect("every ${ has its }");
        out.push_str("supplied-by-the-environment");
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The shipped configuration, pointed at the repository's own prompts.
///
/// The relative `[paths].prompts` is rewritten to an absolute path in the TOML
/// before it is parsed, because the loaded configuration exposes no field to set
/// afterward.
fn shipped_config() -> Config {
    let prompts = workspace_root().join("prompts");
    assert!(
        prompts.is_dir(),
        "[paths].prompts names the repository's own prompts directory: {}",
        prompts.display()
    );
    let raw = without_variables(SHIPPED).replace(
        "prompts = \"prompts\"",
        &format!("prompts = '{}'", prompts.display()),
    );
    Config::from_toml_str(&raw).expect("the shipped prompts.toml parses")
}

/// A client driving a server over the shipped configuration on an in-process
/// session.
///
/// Resolving the catalog first is boot's own rule: it refuses an incomplete
/// catalog, so this succeeding is the same statement as `serve prompts.toml`
/// reaching its transport. The prepared tools load through the public boot seam
/// pointed at the shipped (unreachable, in this test) gateway, which degrades to
/// an empty model catalog without a network round trip.
async fn shipped_client() -> RunningService<RoleClient, ()> {
    let config = shipped_config();
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the shipped catalog resolves");
    let config = Arc::new(config);
    let catalog = Arc::new(CatalogHandle::new(catalog));
    let tools = Arc::new(
        PreparedTools::load(&config)
            .await
            .expect("prepare the shipped live tools"),
    );
    let server = PromptForgeServer::new(config, catalog, tools);
    let (server_io, client_io) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        let Ok(running) = server.serve(server_io).await else {
            return;
        };
        let _quit = running.waiting().await;
    });
    ().serve(client_io)
        .await
        .expect("the in-process client initializes")
}

#[tokio::test]
async fn the_shipped_configuration_resolves_and_lists_its_prompts() {
    let client = shipped_client().await;
    let response = client
        .call_tool_once(CallToolRequestParams::new("list_prompts"))
        .await
        .expect("list_prompts answers");
    let CallToolResponse::Complete(result) = response else {
        panic!("this server answers a call with its result")
    };
    let structured = result
        .structured_content
        .expect("list_prompts carries structured content");
    let prompts = structured["prompts"]
        .as_array()
        .expect("the listing is an array");
    let names: Vec<&str> = prompts
        .iter()
        .map(|prompt| prompt["name"].as_str().expect("each entry names a prompt"))
        .collect();
    assert_eq!(names, SHIPPED_PROMPTS);
    for prompt in prompts {
        assert!(
            prompt["problem"].is_null(),
            "every shipped prompt is healthy: {prompt}"
        );
    }
}

#[tokio::test]
async fn the_shipped_catalog_reaches_tools_list_through_the_built_ins_alone() {
    let client = shipped_client().await;
    let listed = client.list_tools(None).await.expect("tools/list answers");
    let names: Vec<&str> = listed.tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert!(
        names.contains(&"run_prompt"),
        "the runner is how a prompt is reached: {names:?}"
    );
    for shipped in SHIPPED_PROMPTS {
        assert!(
            !names.contains(&shipped),
            "{shipped} reached tools/list: {names:?}"
        );
    }
}
