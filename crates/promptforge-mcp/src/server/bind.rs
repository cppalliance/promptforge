//! Binding a prompt's declared tool names, and the gateway its run talks to.
//!
//! A prompt names the canonical tools it needs in its frontmatter; the runner
//! turns that list into live instances before handing them to the executor.
//! `web_fetch` runs in this process, `web_search` proxies through the gateway,
//! and an unrecognized name fails the run rather than being dropped.
//!
//! This mirrors the same twenty lines in the CLI deliberately. The two callers
//! differ in where the gateway comes from - the CLI reads the environment and
//! may have no gateway at all, while a configured server always has one, so
//! `web_search` here can never be refused for want of credentials. A third
//! copy would be the point at which a shared crate earns its keep; a second one
//! is not.

use promptforge_core::client::{DEFAULT_MODEL, GatewayClient};
use promptforge_core::tools::{Tool, WebSearch};
use promptforge_webfetch::WebFetch;

use crate::config::GatewayConfig;

/// Builds the tool instances a prompt requested, in the order it named them.
///
/// # Errors
/// Returns a caller-facing message naming the tool when the prompt requests one
/// this server does not implement.
pub(super) fn select_tools(
    requested: &[String],
    gateway: &GatewayConfig,
) -> Result<Vec<Box<dyn Tool>>, String> {
    let mut tools: Vec<Box<dyn Tool>> = Vec::with_capacity(requested.len());
    for name in requested {
        let tool: Box<dyn Tool> = match name.as_str() {
            "web_fetch" => Box::new(WebFetch::new()),
            "web_search" => Box::new(WebSearch::new(&gateway.url, gateway.token.expose())),
            other => return Err(format!("the prompt requests an unknown tool: {other}")),
        };
        tools.push(tool);
    }
    Ok(tools)
}

/// The client a run's model calls go through, built from the configuration
/// rather than the environment: setting an environment variable is `unsafe`
/// under edition 2024 and this workspace forbids unsafe, so a configured server
/// hands the executor a client instead of arranging for one to be found.
pub(super) fn gateway_client(gateway: &GatewayConfig) -> GatewayClient {
    let model = gateway.model.as_deref().unwrap_or(DEFAULT_MODEL);
    GatewayClient::new(&gateway.url, gateway.token.expose(), model)
}

#[cfg(test)]
mod tests {
    use super::{gateway_client, select_tools};
    use crate::config::Config;

    fn gateway(model: &str) -> Config {
        Config::from_toml_str(&format!(
            "[server]\ntoken = \"t\"\n\n[gateway]\nurl = \"http://127.0.0.1:8081/v1/\"\ntoken = \"gw\"\n{model}"
        ))
        .expect("the fixture configuration parses")
    }

    #[test]
    fn no_declared_tools_binds_nothing() {
        let config = gateway("");
        let tools = select_tools(&[], &config.gateway).expect("an empty list binds");
        assert!(tools.is_empty());
    }

    #[test]
    fn both_canonical_tools_bind() {
        let config = gateway("");
        let tools = select_tools(
            &["web_fetch".to_owned(), "web_search".to_owned()],
            &config.gateway,
        )
        .expect("both canonical names bind");
        let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
        assert_eq!(names, ["web_fetch", "web_search"]);
    }

    #[test]
    fn an_unknown_tool_is_refused_by_name() {
        let config = gateway("");
        let Err(message) = select_tools(&["nope".to_owned()], &config.gateway) else {
            panic!("an unknown tool should not bind")
        };
        assert!(
            message.contains("nope"),
            "message should name it: {message}"
        );
    }

    #[test]
    fn a_configured_model_wins_over_the_default() {
        let config = gateway("model = \"some-other-model\"\n");
        assert_eq!(gateway_client(&config.gateway).model(), "some-other-model");
    }

    #[test]
    fn no_configured_model_falls_back_to_the_core_default() {
        let config = gateway("");
        assert_eq!(
            gateway_client(&config.gateway).model(),
            promptforge_core::client::DEFAULT_MODEL
        );
    }
}
