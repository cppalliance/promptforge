//! Chat-completion proofs against the warm CUDA server: MTP drafting timings, a
//! parsed tool call, and an image completion through the projector.

use std::net::SocketAddr;

use serde_json::Value;

use super::COMPLETION_TIMEOUT;

/// One non-streaming chat completion through the gateway, bounded by the
/// completion phase timeout.
async fn chat_completion(client: &reqwest::Client, addr: SocketAddr, request: &Value) -> Value {
    let response = tokio::time::timeout(
        COMPLETION_TIMEOUT,
        client
            .post(format!("http://{addr}/v1/chat/completions"))
            .bearer_auth("test-token")
            .json(request)
            .send(),
    )
    .await
    .expect("chat completion exceeded the phase timeout")
    .expect("chat completion send failed");
    let status = response.status();
    let body = tokio::time::timeout(COMPLETION_TIMEOUT, response.text())
        .await
        .expect("chat completion body exceeded the phase timeout")
        .expect("chat completion body read failed");
    assert_eq!(status.as_u16(), 200, "chat completion failed: {body}");
    serde_json::from_str(&body).expect("chat completion body is JSON")
}

/// Phase 6: an MTP completion under deterministic sampling must show the
/// drafter both proposed and landed tokens in the response's `timings`.
pub(super) async fn prove_mtp(client: &reqwest::Client, addr: SocketAddr) {
    let completion = chat_completion(
        client,
        addr,
        &serde_json::json!({
            "model": "gemma-4",
            "messages": [{
                "role": "user",
                "content": "Write the integers from 1 through 100, separated by one space, and output nothing else."
            }],
            "temperature": 0,
            "seed": 42,
            "presence_penalty": 0,
            "max_tokens": 512
        }),
    )
    .await;
    assert_mtp_timings(&completion);
}

/// Phase 8: a tool call through the chat completions path must parse.
pub(super) async fn prove_tool_call(client: &reqwest::Client, addr: SocketAddr) {
    let body = chat_completion(
        client,
        addr,
        &serde_json::json!({
            "model": "gemma-4",
            "messages": [{
                "role": "user",
                "content": "What is the weather in Paris right now? Use the get_weather function."
            }],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get the current weather in a named city",
                    "parameters": {
                        "type": "object",
                        "properties": { "city": { "type": "string" } },
                        "required": ["city"]
                    }
                }
            }],
            "temperature": 0,
            "seed": 42,
            "presence_penalty": 0,
            "max_tokens": 128
        }),
    )
    .await;
    assert_tool_call(&body);
}

/// Phase 9: a real image-content completion through the projector must
/// describe the generated test image.
pub(super) async fn prove_image_completion(client: &reqwest::Client, addr: SocketAddr) {
    let body = chat_completion(
        client,
        addr,
        &serde_json::json!({
            "model": "gemma-4",
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": "The image is split vertically into two solid-color halves. Name the color of the left half and the color of the right half."
                    },
                    { "type": "image_url", "image_url": { "url": test_image_data_url() } }
                ]
            }],
            "temperature": 0,
            "seed": 42,
            "presence_penalty": 0,
            "max_tokens": 128
        }),
    )
    .await;
    let reply = body
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no text content in image completion: {body}"))
        .to_lowercase();
    assert!(
        reply.contains("red") && reply.contains("blue"),
        "image completion must name both colors, got: {reply}"
    );
}

/// Phase 6 helper: the response's `timings` extension must show the MTP
/// drafter both proposed and landed tokens.
fn assert_mtp_timings(body: &Value) {
    let timings = body
        .get("timings")
        .unwrap_or_else(|| panic!("no timings in response: {body}"));
    let drafted = timings
        .get("draft_n")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("no draft_n in timings: {timings}"));
    let accepted = timings
        .get("draft_n_accepted")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("no draft_n_accepted in timings: {timings}"));
    eprintln!("mtp timings: {timings}");
    assert!(drafted > 0, "the drafter proposed no tokens: {timings}");
    assert!(accepted > 0, "no drafted tokens were accepted: {timings}");
}

/// Phase 8: the model's reply must contain a tool call whose function
/// arguments parse as JSON.
fn assert_tool_call(body: &Value) {
    let tool_calls = body
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no tool_calls in response: {body}"));
    assert!(!tool_calls.is_empty(), "empty tool_calls: {body}");
    let function = tool_calls[0].get("function").expect("tool call function");
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .expect("tool call function name");
    assert!(!name.is_empty(), "empty tool call name: {body}");
    let arguments = function.get("arguments").expect("tool call arguments");
    let parsed: Value = match arguments {
        Value::String(text) => {
            serde_json::from_str(text).expect("tool call arguments string parses as JSON")
        }
        other => other.clone(),
    };
    assert!(
        parsed.is_object(),
        "tool call arguments not an object: {body}"
    );
}

/// Standard base64, so the test image needs no extra dependency.
pub(super) fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = usize::from(chunk[0]);
        let b1 = usize::from(*chunk.get(1).unwrap_or(&0));
        let b2 = usize::from(*chunk.get(2).unwrap_or(&0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        encoded.push(char::from(ALPHABET[(triple >> 18) & 63]));
        encoded.push(char::from(ALPHABET[(triple >> 12) & 63]));
        encoded.push(if chunk.len() > 1 {
            char::from(ALPHABET[(triple >> 6) & 63])
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            char::from(ALPHABET[triple & 63])
        } else {
            '='
        });
    }
    encoded
}

/// A 64x64 PNG, left half pure red and right half pure blue, as a data URL.
fn test_image_data_url() -> String {
    let mut pixels = Vec::with_capacity(64 * 64 * 3);
    for _row in 0..64 {
        for column in 0..64 {
            if column < 32 {
                pixels.extend_from_slice(&[255, 0, 0]);
            } else {
                pixels.extend_from_slice(&[0, 0, 255]);
            }
        }
    }
    let mut png_bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut png_bytes, 64, 64);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    // `write_header` consumes the encoder; dropping the writer writes IEND.
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(&pixels).expect("png encode");
    drop(writer);
    format!("data:image/png;base64,{}", base64_encode(&png_bytes))
}
