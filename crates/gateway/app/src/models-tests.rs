use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory, scripted_service};

use super::{CatalogModelInfo, CatalogModelsResponse};

#[test]
fn speech_catalog_metadata_is_generic_transcription_metadata() {
    let factory =
        ScriptedModelFactory::new(ScriptedDecoder::new()).with_final(ScriptedDecoder::new());
    let service = scripted_service(factory, 15, 500).expect("scripted service starts");
    let data = service
        .models()
        .iter()
        .map(CatalogModelInfo::speech)
        .collect();
    let value = serde_json::to_value(CatalogModelsResponse {
        object: "list",
        data,
    })
    .expect("catalog serializes");

    assert_eq!(
        value,
        serde_json::json!({
            "object": "list",
            "data": [
                {"id": "scripted-interim", "object": "model", "kind": "transcription"},
                {"id": "scripted-final", "object": "model", "kind": "transcription"},
                {"id": "realtime-transcribe", "object": "model", "kind": "transcription"},
            ],
        })
    );
    service.shutdown();
}
