use serde_json::{Value, json};

use super::{Tool, ToolError, ToolId, ToolOutput, ToolRegistry, ToolRegistryErrorKind};

struct FixtureTool;

#[async_trait::async_trait]
impl Tool for FixtureTool {
    fn id(&self) -> ToolId {
        ToolId::new("fixtures", "inspect").expect("fixture id is valid")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn wire_name(&self) -> &str {
        "inspect_wire"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "Inspect a fixture."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        })
    }

    async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::trusted(String::new()))
    }
}

struct RegistryFixtureTool {
    id_name: &'static str,
    wire_name: &'static str,
}

#[async_trait::async_trait]
impl Tool for RegistryFixtureTool {
    fn id(&self) -> ToolId {
        ToolId::new("fixtures", self.id_name).expect("fixture id is valid")
    }

    fn wire_name(&self) -> &str {
        self.wire_name
    }

    fn description(&self) -> &str {
        self.wire_name
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::trusted(String::new()))
    }
}

#[test]
fn trait_is_dyn_compatible() {
    let tools: Vec<Box<dyn Tool>> = Vec::new();
    assert!(tools.is_empty());
}

#[test]
fn tool_output_carries_mandatory_trust() {
    use super::{OutputTrust, ToolOutput};
    assert_eq!(ToolOutput::trusted("a").trust(), OutputTrust::Trusted);
    assert_eq!(ToolOutput::untrusted("b").trust(), OutputTrust::Untrusted);
    assert_eq!(ToolOutput::trusted("a").text(), "a");
}

#[test]
fn tool_registry_is_send_and_sync() {
    // The public dyn-bearing registry must stay `Send + Sync` so downstream
    // callers can share it across tasks; a representation change that dropped
    // either auto trait would fail to compile here (tools.rs F6).
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ToolRegistry<'static>>();
}

#[test]
fn tool_error_classifies_and_hides_source() {
    use super::{ToolError, ToolErrorKind};
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<ToolError>();

    let plain = ToolError::message("model-safe");
    assert_eq!(plain.kind(), ToolErrorKind::Other);
    assert_eq!(plain.to_string(), "model-safe");
    assert!(!plain.is_cancelled() && !plain.is_retryable());

    let cancelled = ToolError::message("stopped").with_kind(ToolErrorKind::Cancelled);
    assert!(cancelled.is_cancelled());

    let retry = ToolError::message("net").with_kind(ToolErrorKind::Transport);
    assert!(retry.is_retryable());

    let sourced = ToolError::with_source("wrap", std::io::Error::other("cause"));
    assert!(std::error::Error::source(&sourced).is_some());
    assert!(
        !sourced.to_string().contains("cause"),
        "Display must not expose the tool error source: {sourced}"
    );
}

#[test]
fn descriptor_surface_preserves_identity_description_and_schema() {
    let tool = FixtureTool;

    assert_eq!(
        tool.id(),
        ToolId::new("fixtures", "inspect").expect("valid id")
    );
    assert_eq!(tool.wire_name(), "inspect_wire");
    assert_eq!(tool.description(), "Inspect a fixture.");
    assert_eq!(
        tool.parameters_schema(),
        json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        })
    );
}

#[test]
fn registry_lookup_uses_stable_identity_not_wire_name() {
    let tool = FixtureTool;
    let registry = ToolRegistry::new([&tool as &dyn Tool]).expect("unique registry");

    let found = registry
        .get(&ToolId::new("fixtures", "inspect").expect("valid id"))
        .expect("the stable identity should resolve");
    assert_eq!(found.wire_name(), "inspect_wire");
    assert!(
        registry
            .get(&ToolId::new("fixtures", "inspect_wire").expect("valid id"))
            .is_none(),
        "the transport name must not become identity"
    );
}

#[test]
fn registry_preserves_order_and_first_match_lookup() {
    let first = RegistryFixtureTool {
        id_name: "inspect",
        wire_name: "first_inspect",
    };
    let middle = RegistryFixtureTool {
        id_name: "summarize",
        wire_name: "summarize",
    };
    let registry = ToolRegistry::new([&first as &dyn Tool, &middle as &dyn Tool])
        .expect("distinct identities build a registry");

    assert_eq!(
        registry
            .tools()
            .iter()
            .map(|tool| tool.wire_name())
            .collect::<Vec<_>>(),
        ["first_inspect", "summarize"]
    );
    assert_eq!(registry.len(), 2);
    assert_eq!(
        registry
            .get(&ToolId::new("fixtures", "inspect").expect("valid id"))
            .expect("the identity should resolve")
            .wire_name(),
        "first_inspect",
    );
}

#[test]
fn registry_rejects_duplicate_tool_ids() {
    let first = RegistryFixtureTool {
        id_name: "inspect",
        wire_name: "first_inspect",
    };
    let repeated = RegistryFixtureTool {
        id_name: "inspect",
        wire_name: "second_inspect",
    };
    let error = ToolRegistry::new([&first as &dyn Tool, &repeated as &dyn Tool])
        .expect_err("a repeated tool identity must be rejected at registration");
    assert_eq!(error.kind(), ToolRegistryErrorKind::DuplicateId);
    assert_eq!(
        error.duplicate_id(),
        Some(&ToolId::new("fixtures", "inspect").expect("valid id")),
        "the error must name the duplicated identity"
    );
}

#[test]
fn tool_id_new_rejects_empty_separator_and_control() {
    use super::ToolIdErrorKind;

    assert_eq!(
        ToolId::new("", "name").expect_err("empty server").kind(),
        ToolIdErrorKind::Empty
    );
    assert_eq!(
        ToolId::new("server", "").expect_err("empty name").kind(),
        ToolIdErrorKind::Empty
    );
    assert_eq!(
        ToolId::new("a/b", "name")
            .expect_err("separator in server")
            .kind(),
        ToolIdErrorKind::Separator
    );
    assert_eq!(
        ToolId::new("server", "a/b")
            .expect_err("separator in name")
            .kind(),
        ToolIdErrorKind::Separator
    );
    assert_eq!(
        ToolId::new("server", "na\u{7f}me")
            .expect_err("DEL control in name")
            .kind(),
        ToolIdErrorKind::Control
    );
    assert_eq!(
        ToolId::new("ser\tver", "name")
            .expect_err("tab control in server")
            .kind(),
        ToolIdErrorKind::Control
    );
    // A provider-invalid but structurally legal identity is accepted here;
    // provider acceptance is a runtime concern, not an identity invariant.
    assert!(ToolId::new("promptforge", "web_search").is_ok());
}

#[test]
fn registry_rejects_illegal_wire_name() {
    struct BadWire;

    #[async_trait::async_trait]
    impl Tool for BadWire {
        fn id(&self) -> ToolId {
            ToolId::new("fixtures", "bad_wire").expect("valid id")
        }
        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn wire_name(&self) -> &str {
            "bad/name"
        }
        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn description(&self) -> &str {
            "bad"
        }
        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::trusted(String::new()))
        }
    }

    let bad = BadWire;
    let error = ToolRegistry::new([&bad as &dyn Tool])
        .expect_err("an illegal wire name must be rejected at registration");
    assert_eq!(error.kind(), ToolRegistryErrorKind::InvalidWireName);
    assert!(error.duplicate_id().is_none());
}
