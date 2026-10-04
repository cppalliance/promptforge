//! Tests for sheet assembly and the release sheet download.

use gateway_api_types::{ModelKind, Thinking};
use time::format_description::well_known::Rfc3339;

use super::*;

#[path = "sheet-tests-build.rs"]
mod build;

fn entry(id: &str) -> ModelEntry {
    ModelEntry {
        id: id.to_owned(),
        display_name: id.to_owned(),
        family: "test-family".to_owned(),
        variant_of: None,
        variant: None,
        languages: Vec::new(),
        kind: ModelKind::Chat,
        released_at: None,
        context_window: Some(200_000),
        max_output: Some(8_192),
        images: false,
        pdf_input: false,
        video_input: false,
        audio_input: false,
        batch: false,
        citations: false,
        code_execution: false,
        structured_outputs: true,
        tool_calling: true,
        thinking: Thinking::default(),
        effort_levels: Vec::new(),
        default_effort: None,
        pricing: None,
        deprecation: None,
    }
}

fn pinned(text: &str) -> OffsetDateTime {
    OffsetDateTime::parse(text, &Rfc3339).expect("pinned timestamp must parse")
}

#[tokio::test]
async fn fetch_sheet_reports_http_errors() {
    let client = reqwest::Client::new();
    let result = fetch_sheet(&client, "http://127.0.0.1:1/models.json").await;
    let Err(err) = result else {
        panic!("an unreachable release URL must not parse as a sheet");
    };
    assert!(
        matches!(err, FetchError::Http(_)),
        "expected a transport error, got {err:?}"
    );
    // The variant renders only its own message; the stderr note walks
    // the chain so the transport cause still reaches the run report.
    let cause = std::error::Error::source(&err)
        .expect("the Http variant returns its transport cause")
        .to_string();
    assert!(!err.to_string().contains(&cause));
    assert!(
        crate::error_chain(&err).contains(&cause),
        "the chain rendering must include the cause: {}",
        crate::error_chain(&err)
    );
}

#[tokio::test]
async fn fetch_sheet_parses_a_successful_response() {
    use std::io::Write as _;

    let fetched_at = pinned("2026-09-14T13:00:00Z");
    let expected = Sheet {
        schema_version: 1,
        generated_at: fetched_at,
        providers: BTreeMap::from([(
            "test-provider".to_owned(),
            ProviderSlice {
                display_name: "Test Provider".to_owned(),
                tier: Tier::Prime,
                status: SliceStatus::Ok,
                fetched_at: Some(fetched_at),
                openai_base_url: None,
                env_vars: Vec::new(),
                models: vec![entry("m1")],
            },
        )]),
    };
    let body = serde_json::to_string(&expected).expect("sheet fixture must serialize");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let addr = listener.local_addr().expect("fixture server addr");
    let server = std::thread::spawn(move || {
        use std::io::Read as _;

        let (mut stream, _) = listener.accept().expect("accept fixture client");
        // Read the request first: replying before the client finishes
        // sending is an HTTP protocol error. A short read timeout bounds
        // the capture without a sleep; once the client awaits the
        // response, the next read simply times out.
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
        let mut buf = [0_u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("write fixture response");
    });
    let client = reqwest::Client::new();
    let sheet = fetch_sheet(&client, &format!("http://{addr}/models.json"))
        .await
        .expect("a 2xx response with a valid sheet body must parse");
    server.join().expect("fixture server must finish");
    assert_eq!(sheet.schema_version, 1);
    assert_eq!(sheet.generated_at, fetched_at);
    let slice = &sheet.providers["test-provider"];
    assert_eq!(slice.display_name, "Test Provider");
    assert_eq!(slice.tier, Tier::Prime);
    assert_eq!(slice.status, SliceStatus::Ok);
    assert_eq!(slice.fetched_at, Some(fetched_at));
    assert_eq!(slice.models.len(), 1);
    assert_eq!(slice.models[0].id, "m1");
    assert_eq!(slice.models[0].context_window, Some(200_000));
}
