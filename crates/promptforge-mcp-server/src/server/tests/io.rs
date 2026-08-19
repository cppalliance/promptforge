//! Input/output file support: mutual exclusion, frontmatter agreement, store
//! seeding, and inline output extraction.

use rmcp::model::ErrorCode;
use serde_json::json;
use std::sync::Arc;

use super::{
    Catalog, CatalogHandle, Config, OnBroken, PromptForgeServer, call, input_prompt, output_prompt,
    prepared, server, structured_of, text_of, write,
};

// ---- validation ----

#[tokio::test]
async fn input_file_and_input_text_are_mutually_exclusive() {
    let (_dir, server) = server();
    let Err(error) = server
        .dispatch(call(
            "run_prompt",
            json!({
                "prompt": "echo",
                "input_file": "/tmp/a.txt",
                "input_text": "some text"
            }),
        ))
        .await
    else {
        panic!("providing both input_file and input_text is the client's bug")
    };
    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn input_provided_without_a_declaration_fails_the_run() {
    // The echo prompt has no `input:` in its frontmatter, so providing
    // input_text is a mismatch the runner rejects.
    let (_dir, server) = server();
    let result = server
        .dispatch(call(
            "run_prompt",
            json!({ "prompt": "echo", "input_text": "unexpected" }),
        ))
        .await
        .expect("a mismatch is a run failure, not a protocol error");

    assert_eq!(result.is_error, Some(true));
    let structured = structured_of(&result);
    assert_eq!(structured["status"], json!("failed"));
    assert!(
        text_of(&result).contains("declares no input"),
        "the failure should name the mismatch: {}",
        text_of(&result)
    );
}

#[tokio::test]
async fn input_declared_but_not_provided_fails_the_run() {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    write(dir.path(), "reader.md", &input_prompt("reader", "paper.md"));
    let config = Config::from_toml_str(&format!(
        "[server]\napi_key = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n[catalog]\ninclude = [\"*.md\"]\n",
        dir.path().display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = prepared(&config);
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        tools,
    );

    let result = server
        .dispatch(call("run_prompt", json!({ "prompt": "reader" })))
        .await
        .expect("a missing input is a run failure, not a protocol error");

    assert_eq!(result.is_error, Some(true));
    let structured = structured_of(&result);
    assert_eq!(structured["status"], json!("failed"));
}

// ---- happy paths ----

#[tokio::test]
async fn input_text_seeds_the_store_for_the_prompt() {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    write(dir.path(), "reader.md", &input_prompt("reader", "paper.md"));
    let config = Config::from_toml_str(&format!(
        "[server]\napi_key = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n[catalog]\ninclude = [\"*.md\"]\n",
        dir.path().display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = prepared(&config);
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        tools,
    );

    let result = server
        .dispatch(call(
            "run_prompt",
            json!({ "prompt": "reader", "input_text": "seeded content" }),
        ))
        .await
        .expect("a prompt with matching input is not a protocol error");

    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        text_of(&result),
        "seeded content",
        "the Lua read the store entry that input_text populated"
    );
    let structured = structured_of(&result);
    assert_eq!(structured["status"], json!("completed"));
    assert_eq!(structured["value"], json!("seeded content"));
}

#[tokio::test]
async fn output_returned_inline_when_no_output_file_specified() {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    write(
        dir.path(),
        "writer.md",
        &output_prompt("writer", "report.md"),
    );
    let config = Config::from_toml_str(&format!(
        "[server]\napi_key = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n[catalog]\ninclude = [\"*.md\"]\n",
        dir.path().display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = prepared(&config);
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        tools,
    );

    let result = server
        .dispatch(call("run_prompt", json!({ "prompt": "writer" })))
        .await
        .expect("a prompt with output is not a protocol error");

    assert_eq!(result.is_error, Some(false));
    let text = text_of(&result);
    assert!(
        text.contains("produced content"),
        "the output content should appear inline: {text}"
    );
    let structured = structured_of(&result);
    assert_eq!(structured["status"], json!("completed"));
}
