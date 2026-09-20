//! Integration tests for the sheet-building binary: the binary runs
//! against a recorded previous-sheet fixture served over loopback HTTP,
//! and the emitted `cloud-provider-models.json` must parse as a
//! schema-valid [`Sheet`].

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output};

use gateway_api::{Sheet, SliceStatus};
use gateway_cloud_providers::providers;

/// The binary under test, built by Cargo alongside the integration test.
const BIN: &str = env!("CARGO_BIN_EXE_shared-cloud-providers");

/// The environment variable carrying the previous release's sheet URL.
const PREVIOUS_SHEET_URL_ENV: &str = "MODELS_SHEET_PREVIOUS_URL";

/// A recorded previous release: one fresh Anthropic slice with one model.
const PREVIOUS_SHEET_JSON: &str = r#"{
  "schema_version": 1,
  "generated_at": "2026-09-01T00:00:00Z",
  "providers": {
    "anthropic": {
      "display_name": "Anthropic",
      "tier": "prime",
      "status": "ok",
      "fetched_at": "2026-09-01T00:00:00Z",
      "openai_base_url": "https://api.anthropic.com/v1",
      "env_vars": [
        { "name": "ANTHROPIC_API_KEY", "role": "key", "default": null }
      ],
      "models": [
        {
          "id": "recorded-m1",
          "display_name": "Recorded M1",
          "family": "recorded",
          "variant_of": null,
          "variant": null,
          "languages": [],
          "kind": "chat",
          "released_at": null,
          "context_window": 200000,
          "max_output": 8192,
          "images": true,
          "pdf_input": false,
          "video_input": false,
          "audio_input": false,
          "batch": false,
          "citations": false,
          "code_execution": false,
          "structured_outputs": true,
          "tool_calling": true,
          "thinking": { "supported": true, "enabled": true, "adaptive": false },
          "effort_levels": ["low", "high"],
          "default_effort": "high",
          "pricing": null,
          "deprecation": null
        }
      ]
    }
  }
}"#;

/// Serves one HTTP response with `status` carrying `body`, returning the
/// URL to request.
fn serve_once(status: &'static str, body: &'static str) -> String {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
        panic!("bind fixture server");
    };
    let Ok(addr) = listener.local_addr() else {
        panic!("fixture server addr");
    };
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            panic!("accept fixture client");
        };
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
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        assert!(
            stream.write_all(response.as_bytes()).is_ok(),
            "write fixture response"
        );
    });
    format!("http://{addr}/models.json")
}

/// A unique output path in the temp directory for one test run.
fn output_path(test: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "shared-cloud-providers-{test}-{}.json",
        std::process::id()
    ))
}

/// An empty temp home directory: the binary's secrets loader must never
/// read the operator's real `~/.promptforge/cloud-provider-secrets.env`
/// during a fixture run.
fn empty_home(test: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "shared-cloud-providers-empty-home-{test}-{}",
        std::process::id()
    ));
    let Ok(()) = std::fs::create_dir_all(&home) else {
        panic!("create the empty fixture home");
    };
    home
}

/// Runs the binary with every provider key stripped from the environment,
/// so no host credential can turn a fixture run into a live fetch.
/// Keyless providers (no `key_env`) have no credential to strip: they
/// still fetch live, so their slice status depends on egress and the
/// universal `unavailable` assertions below exempt them.
fn run_binary(output: &PathBuf, previous_url: Option<&str>) -> Output {
    let mut command = Command::new(BIN);
    command.arg(output);
    for provider in providers() {
        if let Some(key_env) = provider.key_env {
            command.env_remove(key_env);
        }
    }
    match previous_url {
        Some(url) => command.env(PREVIOUS_SHEET_URL_ENV, url),
        None => command.env_remove(PREVIOUS_SHEET_URL_ENV),
    };
    let home = empty_home("default");
    command.env("HOME", &home).env("USERPROFILE", &home);
    let Ok(output) = command.output() else {
        panic!("run the sheet-building binary");
    };
    output
}

/// Reads the emitted sheet, failing with the binary's stderr when the
/// run itself failed.
fn read_output(output: &PathBuf, result: &Output) -> Sheet {
    assert!(
        result.status.success(),
        "the binary must exit successfully: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let Ok(json) = std::fs::read_to_string(output) else {
        panic!("the binary must write the output file");
    };
    let Ok(sheet) = serde_json::from_str(&json) else {
        panic!("the output must parse as a schema-valid Sheet");
    };
    sheet
}

#[test]
fn binary_emits_valid_sheet_and_propagates_stale_slices() {
    let url = serve_once("200 OK", PREVIOUS_SHEET_JSON);
    let output = output_path("stale");
    let result = run_binary(&output, Some(&url));
    let sheet = read_output(&output, &result);
    let _ = std::fs::remove_file(&output);

    assert_eq!(sheet.schema_version, 1);
    assert_eq!(
        sheet.providers.len(),
        providers().len(),
        "every registered provider must appear in the sheet"
    );
    let anthropic = &sheet.providers["anthropic"];
    assert_eq!(
        anthropic.status,
        SliceStatus::Stale,
        "a failed fetch must propagate the previous slice as stale"
    );
    assert_eq!(
        anthropic.models.len(),
        1,
        "stale propagation keeps the recorded models"
    );
    assert_eq!(anthropic.models[0].id, "recorded-m1");
    let openai = &sheet.providers["openai"];
    assert_eq!(
        openai.status,
        SliceStatus::Unavailable,
        "a provider with no key and no previous slice records unavailable"
    );
    assert!(openai.models.is_empty());
}

#[test]
fn binary_secrets_file_overrides_environment() {
    let url = serve_once("200 OK", PREVIOUS_SHEET_JSON);
    // A temp home whose secrets file points at the fixture server, while
    // the process environment holds a deliberately wrong URL: the run can
    // only succeed if the file loaded and overrode the environment.
    let home = std::env::temp_dir().join(format!(
        "shared-cloud-providers-secrets-home-{}",
        std::process::id()
    ));
    let secrets_dir = home.join(".promptforge");
    let Ok(()) = std::fs::create_dir_all(&secrets_dir) else {
        panic!("create the fixture secrets directory");
    };
    let secrets_path = secrets_dir.join("cloud-provider-secrets.env");
    let Ok(()) = std::fs::write(&secrets_path, format!("{PREVIOUS_SHEET_URL_ENV}={url}\n")) else {
        panic!("write the fixture secrets file");
    };
    let output = output_path("secrets-override");
    let mut command = Command::new(BIN);
    command.arg(&output);
    for provider in providers() {
        if let Some(key_env) = provider.key_env {
            command.env_remove(key_env);
        }
    }
    command.env(PREVIOUS_SHEET_URL_ENV, "http://127.0.0.1:1/models.json");
    command.env("HOME", &home).env("USERPROFILE", &home);
    let Ok(result) = command.output() else {
        panic!("run the sheet-building binary");
    };
    let sheet = read_output(&output, &result);
    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_dir_all(&home);

    let anthropic = &sheet.providers["anthropic"];
    assert_eq!(
        anthropic.status,
        SliceStatus::Stale,
        "the secrets file must override the environment's wrong previous-sheet URL"
    );
    assert_eq!(anthropic.models.len(), 1);
    assert_eq!(anthropic.models[0].id, "recorded-m1");
}

#[test]
fn binary_tolerates_first_run_without_previous_sheet() {
    let output = output_path("first-run");
    let result = run_binary(&output, None);
    let sheet = read_output(&output, &result);
    let _ = std::fs::remove_file(&output);

    assert_eq!(sheet.schema_version, 1);
    assert_eq!(sheet.providers.len(), providers().len());
    for provider in providers() {
        if provider.key_env.is_none() {
            continue;
        }
        let slice = &sheet.providers[provider.name];
        assert_eq!(
            slice.status,
            SliceStatus::Unavailable,
            "first run with no keys must record `{}` as unavailable, not fail",
            provider.name
        );
    }
}

#[test]
fn binary_fails_without_writing_when_previous_sheet_errors() {
    let url = serve_once("500 Internal Server Error", "boom");
    let output = output_path("previous-500");
    let _ = std::fs::remove_file(&output);
    let result = run_binary(&output, Some(&url));

    assert!(
        !result.status.success(),
        "a 500 previous-sheet response must fail the run"
    );
    assert!(
        !output.exists(),
        "a failed run must not write the output file"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains(url.as_str()),
        "stderr must name the failing URL: {stderr}"
    );
}

#[test]
fn binary_treats_404_previous_sheet_as_first_run() {
    let url = serve_once("404 Not Found", "not found");
    let output = output_path("previous-404");
    let result = run_binary(&output, Some(&url));
    let sheet = read_output(&output, &result);
    let _ = std::fs::remove_file(&output);

    assert_eq!(sheet.schema_version, 1);
    assert_eq!(sheet.providers.len(), providers().len());
    for provider in providers() {
        if provider.key_env.is_none() {
            continue;
        }
        let slice = &sheet.providers[provider.name];
        assert_eq!(
            slice.status,
            SliceStatus::Unavailable,
            "a 404 previous sheet means first run: `{}` must record unavailable",
            provider.name
        );
    }
}

#[test]
fn binary_reports_each_failed_keyed_provider_on_stderr() {
    let output = output_path("stderr-notes");
    let result = run_binary(&output, None);
    read_output(&output, &result);
    let _ = std::fs::remove_file(&output);

    let stderr = String::from_utf8_lossy(&result.stderr);
    for provider in providers() {
        if provider.key_env.is_none() {
            continue;
        }
        let note = format!("note: {} fetch failed", provider.name);
        assert!(
            stderr.contains(&note),
            "stderr must carry one note per failed keyed provider (`{}`): {stderr}",
            provider.name
        );
    }
}

#[test]
fn binary_defaults_output_to_the_profile_dir() {
    // No output argument: the sheet must land at
    // `<home>/.promptforge/cloud-provider-models.json`.
    let home = empty_home("default-output");
    let mut command = Command::new(BIN);
    for provider in providers() {
        if let Some(key_env) = provider.key_env {
            command.env_remove(key_env);
        }
    }
    command.env_remove(PREVIOUS_SHEET_URL_ENV);
    command.env("HOME", &home).env("USERPROFILE", &home);
    let Ok(result) = command.output() else {
        panic!("run the sheet-building binary");
    };
    let output = home.join(".promptforge").join("cloud-provider-models.json");
    let sheet = read_output(&output, &result);
    let _ = std::fs::remove_dir_all(&home);

    assert_eq!(sheet.schema_version, 1);
    assert_eq!(sheet.providers.len(), providers().len());
}

#[test]
fn binary_fails_without_writing_when_previous_sheet_is_unparseable() {
    let url = serve_once("200 OK", "this is not a sheet");
    let output = output_path("previous-invalid");
    let _ = std::fs::remove_file(&output);
    let result = run_binary(&output, Some(&url));

    assert!(
        !result.status.success(),
        "an unparseable 200 previous-sheet body must fail the run"
    );
    assert!(
        !output.exists(),
        "a failed run must not write the output file"
    );
}
