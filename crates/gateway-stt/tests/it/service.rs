//! Public speech-facade integration tests.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gateway_config::{Config, ProfileName};
use gateway_stt::SpeechService;
use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory, scripted_service};
use tower::ServiceExt as _;

use crate::common::fixture_service;

#[test]
fn clones_observe_one_complete_scripted_generation() {
    let factory = ScriptedModelFactory::new(ScriptedDecoder::new())
        .with_final(ScriptedDecoder::new())
        .with_gpu_available(true);
    let service = scripted_service(factory, 15, 500).expect("scripted service starts");
    let clone = service.clone();

    let status = clone.status();
    assert!(status.configured());
    assert!(status.ready());
    assert!(status.gpu());
    assert_eq!(status.generation(), Some(1));
    assert_eq!(
        clone
            .models()
            .iter()
            .map(gateway_stt::SpeechModelInfo::name)
            .collect::<Vec<_>>(),
        ["scripted-interim", "scripted-final", "realtime-transcribe"]
    );

    service.shutdown();
    assert!(!clone.status().ready());
    assert!(clone.models().is_empty());
}

#[test]
fn logical_realtime_model_requires_both_physical_roles() {
    let service = scripted_service(ScriptedModelFactory::new(ScriptedDecoder::new()), 15, 500)
        .expect("single-model scripted service starts");

    assert_eq!(
        service
            .models()
            .iter()
            .map(gateway_stt::SpeechModelInfo::name)
            .collect::<Vec<_>>(),
        ["scripted-interim"]
    );

    service.shutdown();
}

#[expect(
    clippy::expect_used,
    reason = "fixture construction fails with the named catalog invariant"
)]
fn selected_speech_config(models: &str, selected: &str) -> Config {
    Config::from_toml_str(&format!(
        "config-version = 2\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
         {models}\
         [[profile]]\nname = \"speech\"\nmodels = {selected}\n"
    ))
    .expect("speech catalog parses")
    .select_profile(&ProfileName::parse("speech").expect("profile name"))
    .expect("speech profile selects")
}

#[test]
fn physical_interim_cannot_claim_the_logical_realtime_identity() {
    let config = selected_speech_config(
        "[[stt_model]]\nname = \"realtime-transcribe\"\nrole = \"interim\"\n\
         source = \"/missing-interim.bin\"\nvram_gb = 1.0\n",
        "[\"realtime-transcribe\"]",
    );
    let error = SpeechService::new()
        .prepare(&config, None)
        .expect_err("the logical name is reserved before artifact access");
    assert!(error.to_string().contains("reserved"), "{error}");
}

#[test]
fn physical_final_in_a_pair_cannot_claim_the_logical_realtime_identity() {
    let config = selected_speech_config(
        "[[stt_model]]\nname = \"physical-interim\"\nrole = \"interim\"\n\
         source = \"/missing-interim.bin\"\nvram_gb = 1.0\n\
         [[stt_model]]\nname = \"realtime-transcribe\"\nrole = \"final\"\n\
         source = \"/missing-final.bin\"\nvram_gb = 1.0\n",
        "[\"physical-interim\", \"realtime-transcribe\"]",
    );
    let error = SpeechService::new()
        .prepare(&config, None)
        .expect_err("the logical name is reserved before artifact access");
    assert!(error.to_string().contains("reserved"), "{error}");
}

#[tokio::test]
async fn facade_routes_keep_the_temporary_legacy_capability() {
    let response = SpeechService::new()
        .routes()
        .oneshot(
            Request::builder()
                .uri("/stt/capability")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("route answers");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    assert_eq!(&body[..], br#"{"gpu":false,"engine":false}"#);
}

#[test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
fn switch_in_loads_and_switch_out_fully_unloads_the_generation() {
    let service = fixture_service(false);
    assert!(service.status().ready());
    assert_eq!(service.models()[0].name(), "speech");
    service.shutdown();
    assert!(!service.status().ready());
    assert!(service.models().is_empty());
}
