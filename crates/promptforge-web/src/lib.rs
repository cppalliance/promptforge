//! The `promptforge/web` capability: web fetch and search in one pack.
//!
//! A research prompt wants both tools or neither, so the first-party web
//! capability activates as one frontmatter line
//! (`capabilities: [promptforge/web]`) and contributes
//! `promptforge/web/fetch` and `promptforge/web/search` - the tools formerly
//! shipped as the separate `promptforge-webfetch` and `promptforge-web-search`
//! packs, combined under the single capability their ids already name.
//!
//! The host builds the capability once at registration with the gateway's API
//! root and bearer token (the search tool proxies through the gateway so the
//! vendor credential never leaves the server) and an optional fetch policy;
//! the prompt never sees either. Activation clones the pre-built tools into
//! the run's [`Contribution`].

use std::sync::Arc;

use shared_promptforge_api::capabilities::{
    Capability, CapabilityError, CapabilityErrorKind, CapabilityId, Contribution, RunServices,
};
use shared_promptforge_api::tools::ToolError;

use promptforge_web_search::WebSearch;
use promptforge_webfetch::WebFetch;
pub use promptforge_webfetch::{ConfigError, FetchConfig};

/// The first-party `promptforge/web` capability.
///
/// Contributes `promptforge/web/fetch` (a hardened page fetch rendering to
/// markdown) and `promptforge/web/search` (a search proxy through the
/// gateway). Both tools are built at construction, so a bad gateway root or
/// an empty token fails here - at host startup - rather than at a run's
/// prepare time.
///
/// # Examples
/// ```
/// use promptforge_web::Web;
/// use shared_promptforge_api::capabilities::Capability;
///
/// let capability = Web::new("https://gateway.example.com/v1", "bearer-token")?;
/// assert_eq!(capability.id().to_string(), "promptforge/web");
/// # Ok::<(), shared_promptforge_api::tools::ToolError>(())
/// ```
#[derive(Debug, Clone)]
pub struct Web {
    /// The stable identity, `promptforge/web`.
    id: CapabilityId,
    /// The fetch tool, built over its validated policy.
    fetch: WebFetch,
    /// The search tool, bound to the gateway root and bearer token.
    search: WebSearch,
}

impl Web {
    /// Builds the capability over the default fetch policy.
    ///
    /// `base_url` is the gateway's OpenAI-shaped API root (for example
    /// `https://gateway.example.com/v1`) and `token` the shared bearer token;
    /// both are validated here.
    ///
    /// # Errors
    /// Returns the search tool's [`ToolError`] when `base_url` is not a valid
    /// gateway API root or `token` is empty.
    pub fn new(base_url: &str, token: impl Into<String>) -> Result<Web, ToolError> {
        Ok(Web {
            id: CapabilityId::from_validated("promptforge/web"),
            fetch: WebFetch::new(),
            search: WebSearch::new(base_url, token)?,
        })
    }

    /// Replaces the default fetch policy with a validated custom one.
    ///
    /// # Errors
    /// Returns [`ConfigError`] if the HTTP client cannot be built for
    /// `config` (for example a TLS backend that fails to initialize).
    pub fn with_fetch_config(mut self, config: FetchConfig) -> Result<Web, ConfigError> {
        self.fetch = WebFetch::try_with_config(config)?;
        Ok(self)
    }
}

impl Capability for Web {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Fetch a web page as markdown and search the web through the gateway."
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
        if services.cancel.is_cancelled() {
            return Err(
                CapabilityError::message("promptforge/web: the run was cancelled")
                    .with_kind(CapabilityErrorKind::Cancelled),
            );
        }
        Ok(Contribution {
            tools: vec![Arc::new(self.fetch.clone()), Arc::new(self.search.clone())],
        })
    }
}

#[cfg(test)]
mod tests {
    use shared_promptforge_api::cancel::CancelHandle;
    use shared_promptforge_api::capabilities::{
        Capability, CapabilityErrorKind, CapabilityId, RunServices,
    };
    use shared_promptforge_api::tools::ToolId;

    use crate::Web;

    /// Fresh run services over an empty VFS and a live cancel handle.
    fn services() -> RunServices {
        RunServices::new(shared_vfs::VfsRef::builder().build(), CancelHandle::new())
    }

    #[test]
    fn activating_the_capability_contributes_both_tools_under_its_full_id() {
        let capability = Web::new("http://localhost", "tok").expect("valid configuration");
        assert_eq!(
            capability.id(),
            &CapabilityId::parse("promptforge/web").expect("valid capability id")
        );

        let contribution = capability.create(&services()).expect("activation succeeds");

        let mut ids: Vec<ToolId> = contribution.tools.iter().map(|tool| tool.id()).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                ToolId::parse("promptforge/web/fetch").expect("valid tool id"),
                ToolId::parse("promptforge/web/search").expect("valid tool id"),
            ]
        );
        for tool in &contribution.tools {
            assert!(
                capability.id().contains(&tool.id()),
                "every contributed tool lives under the capability's id: {}",
                tool.id()
            );
        }

        let mut wire_names: Vec<&str> = contribution
            .tools
            .iter()
            .map(|tool| tool.wire_name())
            .collect();
        wire_names.sort_unstable();
        assert_eq!(wire_names, ["web_fetch", "web_search"]);
    }

    #[test]
    fn construction_rejects_an_invalid_gateway_root_or_empty_token() {
        assert!(Web::new("not-a-url", "tok").is_err());
        assert!(Web::new("http://user:pass@host/v1", "tok").is_err());
        assert!(Web::new("http://localhost", "").is_err());
    }

    #[test]
    fn activation_on_a_cancelled_run_fails_as_cancelled() {
        let capability = Web::new("http://localhost", "tok").expect("valid configuration");
        let cancel = CancelHandle::new();
        cancel.cancel();
        let services = RunServices::new(shared_vfs::VfsRef::builder().build(), cancel);

        let err = capability
            .create(&services)
            .expect_err("a cancelled run must not activate");
        assert_eq!(err.kind(), CapabilityErrorKind::Cancelled);
        assert!(err.is_cancelled());
    }

    #[test]
    fn a_custom_fetch_policy_is_accepted() {
        let policy = promptforge_webfetch::FetchConfig::builder()
            .max_chars(10_000)
            .build()
            .expect("valid policy");
        let capability = Web::new("http://localhost", "tok")
            .expect("valid configuration")
            .with_fetch_config(policy)
            .expect("the custom policy builds a fetch tool");

        let contribution = capability.create(&services()).expect("activation succeeds");
        assert_eq!(contribution.tools.len(), 2);
    }
}
