//! Tests for the entry: its two shapes, its variables, and its headers.

use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{Value, json};

use super::{LOCAL_NOT_SUPPORTED, RemoteEntry, Vars, expand, parse};

const SECRET: &str = "s3cr3t-value-0042";

fn lookup(key: &str) -> Option<String> {
    match key {
        "TOKEN" => Some(SECRET.to_owned()),
        "PLAIN" => Some("plain".to_owned()),
        _ => None,
    }
}

fn vars() -> Vars<'static> {
    Vars {
        env: &lookup,
        home: Some("/home/ann".to_owned()),
    }
}

fn refusal(config: &Value) -> String {
    parse(config, &vars())
        .expect_err("the entry is refused")
        .to_string()
}

fn accepted(config: &Value) -> RemoteEntry {
    parse(config, &vars()).expect("the entry is accepted")
}

/// The value of the header `name`, which is lowercase.
fn header(entry: &RemoteEntry, name: &'static str) -> String {
    let value: &HeaderValue = entry
        .headers
        .get(&HeaderName::from_static(name))
        .expect("the header is present");
    String::from_utf8_lossy(value.as_bytes()).into_owned()
}

#[test]
fn a_remote_entry_keeps_its_url_and_headers() {
    let entry = accepted(&json!({
        "url": "https://example.com/mcp",
        "headers": { "X-MCP-Tools": "a,b", "Authorization": "Bearer abc" }
    }));
    assert_eq!(entry.url, "https://example.com/mcp");
    assert_eq!(entry.headers.len(), 2);
    assert_eq!(header(&entry, "x-mcp-tools"), "a,b");
    assert_eq!(header(&entry, "authorization"), "Bearer abc");
}

#[test]
fn a_local_entry_is_refused_as_not_supported_without_any_env_value() {
    let reason = refusal(&json!({
        "command": "npx",
        "args": ["-y", "server"],
        "env": { "API_KEY": SECRET, "OTHER": "${env:NOT_SET_ANYWHERE}" },
        "cwd": "/tmp"
    }));
    assert_eq!(reason, LOCAL_NOT_SUPPORTED);
    assert!(!reason.contains(SECRET));
}

#[test]
fn a_local_entry_is_refused_before_its_variables_are_resolved() {
    let reason = refusal(&json!({ "command": "${env:NOT_SET_ANYWHERE}" }));
    assert_eq!(reason, LOCAL_NOT_SUPPORTED);
}

#[test]
fn a_local_entry_with_a_wrong_value_type_is_refused_naming_the_field() {
    for (config, field) in [
        (json!({ "command": 5 }), "`command`"),
        (json!({ "command": "x", "args": "one" }), "`args`"),
        (json!({ "command": "x", "args": [1] }), "`args`"),
        (json!({ "command": "x", "env": ["A"] }), "`env`"),
        (json!({ "command": "x", "env": { "A": 1 } }), "`env.A`"),
        (json!({ "command": "x", "cwd": 3 }), "`cwd`"),
    ] {
        let reason = refusal(&config);
        assert!(reason.contains(field), "{reason} names {field}");
    }
}

#[test]
fn an_entry_with_both_shapes_or_neither_is_refused() {
    assert!(refusal(&json!({ "url": "https://a", "command": "x" })).contains("both"));
    assert!(refusal(&json!({})).contains("neither"));
    assert!(refusal(&json!({ "headers": {} })).contains("neither"));
}

#[test]
fn a_config_that_is_not_an_object_is_refused() {
    for config in [Value::Null, json!("https://a"), json!([1])] {
        assert!(refusal(&config).contains("JSON object"));
    }
}

#[test]
fn a_remote_entry_with_a_wrong_value_type_is_refused_naming_the_field() {
    for (config, field) in [
        (json!({ "url": 5 }), "`url`"),
        (json!({ "url": "https://a", "headers": ["A"] }), "`headers`"),
        (
            json!({ "url": "https://a", "headers": { "A": 1 } }),
            "`headers.A`",
        ),
    ] {
        let reason = refusal(&config);
        assert!(reason.contains(field), "{reason} names {field}");
    }
}

#[test]
fn a_url_that_is_not_http_is_refused_without_echoing_it() {
    let reason = refusal(&json!({ "url": format!("ftp://{SECRET}@host") }));
    assert!(reason.contains("http://"));
    assert!(!reason.contains(SECRET));
}

#[test]
fn the_legacy_sse_type_is_refused_and_any_other_type_is_ignored() {
    assert!(refusal(&json!({ "type": "sse", "url": "https://a" })).contains("sse"));
    for kind in [json!("http"), json!("streamable-http"), json!(7)] {
        accepted(&json!({ "type": kind, "url": "https://a" }));
    }
}

#[test]
fn unknown_fields_are_ignored() {
    accepted(&json!({ "url": "https://a", "timeout": 5, "disabled": false }));
}

#[test]
fn env_and_home_variables_expand_in_the_url_and_in_header_values() {
    let entry = accepted(&json!({
        "url": "https://example.com/${env:PLAIN}/${userHome}",
        "headers": { "Authorization": "Bearer ${env:TOKEN}" }
    }));
    assert_eq!(entry.url, "https://example.com/plain//home/ann");
    assert_eq!(header(&entry, "authorization"), format!("Bearer {SECRET}"));
}

#[test]
fn an_unset_env_variable_is_refused_naming_it_in_the_url_and_in_a_header() {
    let in_url = refusal(&json!({ "url": "https://a/${env:MISSING_ONE}" }));
    assert!(in_url.contains("MISSING_ONE") && in_url.contains("`url`"));
    let in_header = refusal(
        &json!({ "url": "https://a", "headers": { "Authorization": "${env:MISSING_TWO}" } }),
    );
    assert!(in_header.contains("MISSING_TWO") && in_header.contains("`Authorization`"));
}

#[test]
fn home_without_a_home_directory_is_refused() {
    let none = Vars {
        env: &lookup,
        home: None,
    };
    let reason = parse(&json!({ "url": "https://a/${userHome}" }), &none)
        .expect_err("no home")
        .to_string();
    assert!(reason.contains("userHome"));
}

#[test]
fn any_other_variable_form_passes_through_unchanged() {
    for text in [
        "${PINECONE_PUBLIC_API_KEY}",
        "${workspaceFolder}",
        "${input:token}",
        "${",
        "${unterminated",
        "$ {env:TOKEN",
    ] {
        assert_eq!(expand(text, &vars()).expect("passes through"), text);
    }
}

#[test]
fn text_around_a_variable_and_two_variables_in_one_value_expand_in_place() {
    assert_eq!(
        expand("a-${env:PLAIN}-${env:PLAIN}-b", &vars()).expect("expands"),
        "a-plain-plain-b"
    );
}

#[test]
fn reserved_headers_are_refused_naming_the_key_in_any_letter_case() {
    for key in [
        "accept",
        "Accept",
        "ACCEPT",
        "Mcp-Session-Id",
        "mcp-session-id",
        "MCP-SESSION-ID",
        "Last-Event-Id",
        "last-event-id",
        "LAST-EVENT-ID",
    ] {
        let reason = refusal(&json!({ "url": "https://a", "headers": { key: "x" } }));
        assert!(reason.contains(&format!("`{key}`")), "{reason} names {key}");
    }
}

#[test]
fn an_invalid_header_name_or_value_is_refused_naming_the_key_only() {
    let name = refusal(&json!({ "url": "https://a", "headers": { "bad name": "x" } }));
    assert!(name.contains("`bad name`"));
    let value =
        refusal(&json!({ "url": "https://a", "headers": { "X-Key": format!("{SECRET}\n") } }));
    assert!(value.contains("`X-Key`"));
    assert!(!value.contains(SECRET));
}

#[test]
fn debug_output_names_header_keys_and_no_value() {
    let entry = accepted(&json!({
        "url": format!("https://example.com/{SECRET}"),
        "headers": { "Authorization": format!("Bearer {SECRET}"), "X-Other": "v" }
    }));
    let shown = format!("{entry:?}");
    assert!(shown.contains("authorization") && shown.contains("x-other"));
    assert!(!shown.contains(SECRET));
    let value = &entry.headers[&HeaderName::from_static("authorization")];
    let header_shown = format!("{value:?}");
    assert!(!header_shown.contains(SECRET));
}

#[test]
fn no_refusal_holds_a_secret_that_the_entry_supplied() {
    let configs = [
        json!({ "url": "https://a", "headers": { "Accept": SECRET } }),
        json!({ "url": "https://a", "headers": { "Authorization": format!("${{env:MISSING}}{SECRET}") } }),
        json!({ "type": "sse", "url": format!("https://a/{SECRET}") }),
        json!({ "url": format!("https://a/{SECRET}"), "command": SECRET }),
    ];
    for config in configs {
        assert!(!refusal(&config).contains(SECRET));
    }
}
