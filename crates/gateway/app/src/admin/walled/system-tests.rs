use gateway_config::Config;

use crate::test_support::serve;

#[cfg(feature = "stt")]
#[test]
fn speech_snapshot_serializes_only_generic_facade_facts() {
    let snapshot = super::SpeechSnapshot::from(gateway_stt::SpeechService::new().status());

    assert_eq!(
        serde_json::json!(snapshot),
        serde_json::json!({
            "configured": false,
            "ready": false,
            "gpu": false,
        })
    );
}

/// A minimal profile rooting the artifact cache at `cache_dir`.
fn system_config(cache_dir: &std::path::Path) -> Config {
    Config::from_toml_str(&format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"
# Strict bearer auth: the tests below pin that a missing key is refused.
trust_loopback = false

[local]
cache_dir = '{cache_dir}'
"#,
        cache_dir = cache_dir.display(),
    ))
    .expect("the fixture profile parses")
}

#[tokio::test]
async fn admin_system_reports_plausible_cpu_ram_and_disk() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let addr = serve(system_config(temp.path())).await;
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/admin/system"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("the request sends");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("a JSON body");

    let cpu = &body["cpu"];
    assert!(
        cpu["logical_cores"].as_u64().expect("logical_cores") > 0,
        "a running host has at least one logical core"
    );
    assert!(
        cpu["utilization_percent"].as_f64().is_some(),
        "utilization is always a number"
    );
    assert!(
        cpu["frequency_mhz"].is_u64(),
        "frequency is always a number, 0 when unknown"
    );

    let total_ram = body["ram"]["total_bytes"].as_u64().expect("ram total");
    let used_ram = body["ram"]["used_bytes"].as_u64().expect("ram used");
    assert!(total_ram > 0, "a running host has RAM installed");
    assert!(
        used_ram > 0 && used_ram <= total_ram,
        "used RAM is nonzero and within the installed total"
    );

    // The tempdir cache root sits on a mounted drive, so the disk card
    // resolves on every platform the suite runs on.
    let total_disk = body["disk"]["total_bytes"].as_u64().expect("disk total");
    let used_disk = body["disk"]["used_bytes"].as_u64().expect("disk used");
    assert!(total_disk > 0, "the cache drive has a capacity");
    assert!(used_disk <= total_disk, "usage cannot exceed capacity");

    // GPU is genuinely optional: absent on hosts without an NVIDIA
    // driver (CI), present with a name and a nonzero VRAM total where
    // NVML loads. The endpoint must succeed either way.
    if let Some(gpu) = body.get("gpu") {
        assert!(
            gpu["name"].as_str().is_some_and(|name| !name.is_empty()),
            "a reported GPU carries its device name"
        );
        assert!(
            gpu["vram_total_bytes"].as_u64().expect("vram total") > 0,
            "a reported GPU has VRAM"
        );
    }
}

#[tokio::test]
async fn admin_system_requires_bearer_auth() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let addr = serve(system_config(temp.path())).await;
    let http = reqwest::Client::new();

    let unauthenticated = http
        .get(format!("http://{addr}/admin/system"))
        .send()
        .await
        .expect("the request sends");
    assert_eq!(
        unauthenticated.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a request without a bearer token is refused"
    );

    let wrong_key = http
        .get(format!("http://{addr}/admin/system"))
        .bearer_auth("wrong-token")
        .send()
        .await
        .expect("the request sends");
    assert_eq!(
        wrong_key.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a request with the wrong bearer token is refused"
    );
}
