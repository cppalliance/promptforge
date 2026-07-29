//! Resolve prompt-declared tool names into concrete tool instances.
//!
//! A prompt lists the canonical tool names it needs in its frontmatter. The CLI
//! turns that list into live [`Tool`] instances before handing them to the
//! executor. `web_fetch` runs locally and is always available; `web_search`
//! proxies through the gateway and therefore needs the gateway base URL and
//! bearer token.

use promptforge_core::tools::{Tool, WebSearch};
use promptforge_webfetch::WebFetch;

/// Build the tool instances a prompt requested, resolving each canonical name
/// to its implementation.
///
/// `web_fetch` is always available (it runs locally). `web_search` requires the
/// gateway base URL and token, since it proxies through the gateway.
///
/// # Errors
/// Returns an error naming the tool if the prompt requests an unknown tool, or
/// requests `web_search` when the gateway credentials are not available.
pub(crate) fn select_tools(
    requested: &[String],
    base_url: Option<&str>,
    token: Option<&str>,
) -> Result<Vec<Box<dyn Tool>>, String> {
    let mut tools: Vec<Box<dyn Tool>> = Vec::with_capacity(requested.len());
    for name in requested {
        let tool: Box<dyn Tool> = match name.as_str() {
            "web_fetch" => Box::new(WebFetch::new()),
            "web_search" => match (base_url, token) {
                (Some(base_url), Some(token)) => Box::new(WebSearch::new(base_url, token)),
                _ => {
                    return Err(
                        "prompt requests web_search but no gateway is configured (set \
                         PROMPTFORGE_BASE_URL and PROMPTFORGE_TOKEN)"
                            .to_string(),
                    );
                }
            },
            other => return Err(format!("unknown tool: {other}")),
        };
        tools.push(tool);
    }
    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::select_tools;

    #[test]
    fn empty_request_yields_no_tools() {
        let tools = select_tools(&[], None, None).expect("empty request should succeed");
        assert!(tools.is_empty());
    }

    #[test]
    fn web_fetch_is_always_available() {
        let tools = select_tools(&["web_fetch".into()], None, None)
            .expect("web_fetch should not require a gateway");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "web_fetch");
    }

    #[test]
    fn web_search_without_gateway_errors() {
        let Err(err) = select_tools(&["web_search".into()], None, None) else {
            panic!("web_search should require a gateway")
        };
        assert!(
            err.contains("gateway"),
            "error should mention the gateway, got: {err}"
        );
    }

    #[test]
    fn web_search_with_gateway_resolves() {
        let tools = select_tools(&["web_search".into()], Some("http://x/v1"), Some("t"))
            .expect("web_search should resolve with credentials");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "web_search");
    }

    #[test]
    fn unknown_tool_errors() {
        let Err(err) = select_tools(&["nope".into()], None, None) else {
            panic!("unknown tool should error")
        };
        assert!(
            err.contains("unknown"),
            "error should mention unknown tool, got: {err}"
        );
    }
}
