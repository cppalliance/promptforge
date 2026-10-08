//! Tests for how an MCP answer becomes tool output or a tool error.

use std::time::Duration;

use promptforge_plugin::{OutputTrust, ToolErrorKind};
use rmcp::ServiceError;
use rmcp::model::{CallToolResult, ServerResult};
use serde_json::{Value, json};

use super::{output, render, service_error};

fn result(value: Value) -> CallToolResult {
    serde_json::from_value(value).expect("a tool result parses")
}

fn text_of(value: Value) -> String {
    let output = render(&result(value)).expect("a success renders");
    assert_eq!(output.trust(), OutputTrust::Untrusted);
    output.text().to_owned()
}

#[test]
fn text_blocks_join_with_blank_lines() {
    let text = text_of(json!({ "content": [
        { "type": "text", "text": "one" },
        { "type": "text", "text": "two" }
    ] }));
    assert_eq!(text, "one\n\ntwo");
}

#[test]
fn an_embedded_text_resource_is_included() {
    let text = text_of(json!({ "content": [
        { "type": "text", "text": "before" },
        { "type": "resource", "resource": { "uri": "file:///a.txt", "mimeType": "text/plain", "text": "inside" } }
    ] }));
    assert_eq!(text, "before\n\ninside");
}

#[test]
fn media_and_binary_resources_become_placeholders_not_their_data() {
    let text = text_of(json!({ "content": [
        { "type": "image", "data": "AAAA", "mimeType": "image/png" },
        { "type": "audio", "data": "BBBB", "mimeType": "audio/wav" },
        { "type": "resource", "resource": { "uri": "file:///b.bin", "mimeType": "application/pdf", "blob": "CCCC" } },
        { "type": "resource", "resource": { "uri": "file:///c.bin", "blob": "DDDD" } }
    ] }));
    assert_eq!(
        text,
        "[image omitted: image/png]\n\n[audio omitted: audio/wav]\n\n[binary resource omitted: application/pdf]\n\n[binary resource omitted: unknown type]"
    );
    for data in ["AAAA", "BBBB", "CCCC", "DDDD"] {
        assert!(!text.contains(data));
    }
}

#[test]
fn a_resource_link_becomes_its_uri() {
    let text = text_of(json!({ "content": [
        { "type": "resource_link", "uri": "https://example.com/r", "name": "r" }
    ] }));
    assert_eq!(text, "[resource: https://example.com/r]");
}

#[test]
fn a_result_with_no_blocks_gives_its_structured_content() {
    assert_eq!(
        text_of(json!({ "content": [], "structuredContent": { "b": 2, "a": [1, 2] } })),
        r#"{"a":[1,2],"b":2}"#
    );
    assert_eq!(
        text_of(json!({ "structuredContent": { "n": 1 } })),
        r#"{"n":1}"#
    );
}

#[test]
fn content_blocks_win_over_structured_content() {
    let text = text_of(
        json!({ "content": [{ "type": "text", "text": "shown" }], "structuredContent": { "n": 1 } }),
    );
    assert_eq!(text, "shown");
}

#[test]
fn a_result_with_nothing_is_empty_untrusted_text() {
    assert_eq!(text_of(json!({ "content": [] })), "");
}

#[test]
fn an_is_error_result_is_a_backend_error_with_the_servers_text() {
    let error = render(&result(json!({ "isError": true, "content": [
        { "type": "text", "text": "no such paper" }
    ] })))
    .expect_err("an isError result fails");
    assert_eq!(error.kind(), ToolErrorKind::Backend);
    assert_eq!(error.to_string(), "no such paper");
}

#[test]
fn an_is_error_result_without_text_still_says_it_failed() {
    let error = render(&result(json!({ "isError": true, "content": [] }))).expect_err("fails");
    assert_eq!(error.kind(), ToolErrorKind::Backend);
    assert!(error.to_string().contains("error"));
}

#[test]
fn an_is_error_false_result_is_a_success() {
    assert_eq!(
        text_of(json!({ "isError": false, "content": [{ "type": "text", "text": "ok" }] })),
        "ok"
    );
}

#[test]
fn a_timeout_is_a_transport_error_naming_the_deadline() {
    let error = service_error(&ServiceError::Timeout {
        timeout: Duration::from_secs(300),
    });
    assert_eq!(error.kind(), ToolErrorKind::Transport);
    assert!(error.to_string().contains("300 seconds"));
    assert!(error.is_retryable());
}

#[test]
fn a_lost_connection_is_a_transport_error() {
    assert_eq!(
        service_error(&ServiceError::TransportClosed).kind(),
        ToolErrorKind::Transport
    );
}

#[test]
fn a_dropped_transport_error_does_not_repeat_the_url_in_its_text() {
    use std::any::TypeId;

    use rmcp::transport::DynamicTransportError;

    let error = service_error(&ServiceError::TransportSend(
        DynamicTransportError::from_parts(
            "streamable-http",
            TypeId::of::<()>(),
            "error sending request for url (http://host/mcp?key=ghp_secret)".into(),
        ),
    ));
    assert_eq!(error.kind(), ToolErrorKind::Transport);
    assert!(!error.to_string().contains("ghp_secret"), "{error}");
    assert!(!error.to_string().contains("http://"), "{error}");
}

#[test]
fn a_json_rpc_error_from_the_server_is_a_backend_error_with_its_message() {
    let error = service_error(&ServiceError::McpError(rmcp::ErrorData::invalid_params(
        "bad id", None,
    )));
    assert_eq!(error.kind(), ToolErrorKind::Backend);
    assert!(error.to_string().contains("bad id"));
}

#[test]
fn a_failed_request_maps_through_output_and_a_good_one_renders() {
    let failed = output(Err(ServiceError::TransportClosed)).expect_err("fails");
    assert_eq!(failed.kind(), ToolErrorKind::Transport);
    let good = output(Ok(ServerResult::CallToolResult(result(
        json!({ "content": [{ "type": "text", "text": "fine" }] }),
    ))))
    .expect("renders");
    assert_eq!(good.text(), "fine");
    assert_eq!(good.trust(), OutputTrust::Untrusted);
}

#[test]
fn an_answer_that_is_not_a_tool_result_is_a_backend_error() {
    let other = output(Ok(ServerResult::empty(()))).expect_err("not a tool result");
    assert_eq!(other.kind(), ToolErrorKind::Backend);
}
