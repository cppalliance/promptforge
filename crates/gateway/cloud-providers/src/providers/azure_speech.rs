//! Azure Speech provider: the public descriptor plus the private
//! variance of `GET /speechtotext/v3.2/models/base` under
//! `https://{region}.cognitiveservices.azure.com` -
//! `Ocp-Apim-Subscription-Key` auth, `@nextLink` pagination, and
//! normalization of the per-locale base models: the trailing UUID of
//! `self` as the id, `displayName`, `createdDateTime`, and
//! `properties.deprecationDates.transcription` into `Deprecation`. The
//! locale populates `languages` and the family; the feature flags are
//! ignored.
//!
//! Extra environment variables beyond the descriptor's
//! `AZURE_SPEECH_KEY`: `AZURE_SPEECH_REGION` (required - the endpoint
//! is regional with no default, so absent records `unavailable`).
//!
//! Docs: <https://learn.microsoft.com/en-us/rest/api/speechtotext/models/list-base-models>

use gateway_api_types::{Deprecation, EnvRole, ModelEntry, ModelKind, Tier};
use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};

use crate::providers::openai_shape::base_entry;
use crate::{EnvVarSpec, FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "AZURE_SPEECH_KEY";

/// Environment variable holding the endpoint region.
const REGION_ENV: &str = "AZURE_SPEECH_REGION";

/// The Azure Speech provider descriptor. The base URL contains the
/// `{region}` placeholder, filled from `AZURE_SPEECH_REGION`.
pub const PROVIDER: Provider = Provider {
    name: "azure_speech",
    display_name: "Azure Speech",
    tier: Tier::Subprime,
    key_env: Some(KEY_ENV),
    base_url: "https://{region}.cognitiveservices.azure.com",
    openai_base_url: None,
    env_vars: &[
        EnvVarSpec {
            name: KEY_ENV,
            role: EnvRole::Key,
            default: None,
        },
        EnvVarSpec {
            name: REGION_ENV,
            role: EnvRole::Config,
            default: None,
        },
    ],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/speechtotext/v3.2/models/base";

/// Fetches and normalizes Azure Speech's base-model list, following
/// `@nextLink` until the final page.
pub(crate) async fn fetch(
    client: &reqwest::Client,
    base_url: &str,
    key: Option<&str>,
) -> Result<Vec<ModelEntry>, FetchError> {
    let Some(key) = key else {
        return Err(FetchError::MissingKey {
            name: PROVIDER.name.to_owned(),
            key_env: KEY_ENV,
        });
    };
    let region = region_from(std::env::var(REGION_ENV).ok())?;
    fetch_all(client, base_url, key, &region).await
}

/// The endpoint region from the environment. Absent or blank records
/// `unavailable` (a failed fetch naming the missing variable), since
/// the endpoint is regional with no default.
fn region_from(env_value: Option<String>) -> Result<String, FetchError> {
    env_value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| FetchError::MissingKey {
            name: PROVIDER.name.to_owned(),
            key_env: REGION_ENV,
        })
}

/// The paginated fetch, with the region as a parameter so tests need
/// no environment.
async fn fetch_all(
    client: &reqwest::Client,
    base_url: &str,
    key: &str,
    region: &str,
) -> Result<Vec<ModelEntry>, FetchError> {
    let mut url = format!("{}{MODELS_PATH}", base_url.replace("{region}", region));
    let mut entries = Vec::new();
    loop {
        let page: Page = client
            .get(&url)
            .header("Ocp-Apim-Subscription-Key", key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        entries.extend(page.values.iter().map(normalize_model));
        let Some(next) = next_link(&page) else {
            break;
        };
        url = next;
    }
    apply_taxonomy(&mut entries);
    Ok(entries)
}

/// The next page's URL. An absent or empty `@nextLink` ends traversal,
/// so a malformed page can never loop the fetch forever.
fn next_link(page: &Page) -> Option<String> {
    page.next_link.clone().filter(|link| !link.is_empty())
}

/// One page of the list response.
#[derive(Debug, Deserialize)]
struct Page {
    values: Vec<WireModel>,
    #[serde(rename = "@nextLink")]
    next_link: Option<String>,
}

/// One base model as the wire reports it. `description`, `kind`,
/// `status`, `lastActionDateTime`, `features`, and `customProperties`
/// are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireModel {
    /// The model's URL; the trailing UUID is the id.
    #[serde(rename = "self")]
    self_url: String,
    display_name: Option<String>,
    locale: Option<String>,
    created_date_time: Option<String>,
    properties: Option<WireProperties>,
}

/// The properties block: the sheet reads only the deprecation dates.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireProperties {
    deprecation_dates: Option<WireDeprecationDates>,
}

/// The per-feature deprecation dates; for this provider the sheet
/// reads only transcription.
#[derive(Debug, Deserialize)]
struct WireDeprecationDates {
    transcription: Option<String>,
}

/// Normalizes one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let id = model.self_url.rsplit('/').next().unwrap_or(&model.self_url);
    let mut entry = base_entry(id, None);
    entry.kind = ModelKind::Transcription;
    if let Some(name) = &model.display_name {
        entry.display_name.clone_from(name);
    }
    if let Some(locale) = &model.locale {
        entry.languages = vec![locale.clone()];
    }
    entry.released_at = model.created_date_time.as_deref().and_then(parse_wire_date);
    let sunset = model
        .properties
        .as_ref()
        .and_then(|properties| properties.deprecation_dates.as_ref())
        .and_then(|dates| dates.transcription.as_deref());
    if let Some(sunset) = sunset {
        entry.deprecation = Some(Deprecation {
            status: "deprecated".to_owned(),
            date: parse_wire_date(sunset),
            replacement: None,
        });
    }
    entry
}

/// Parses a wire timestamp into a calendar date; an unparseable value
/// yields `None`.
fn parse_wire_date(value: &str) -> Option<Date> {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .map(OffsetDateTime::date)
}

/// Sets every entry's family: the catalog is per-locale base models, so
/// the locale is the family; a model with no locale is its own family
/// (its id is a UUID). There is no snapshot collapse - the ids have no
/// suffixes.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = entry
            .languages
            .first()
            .cloned()
            .unwrap_or_else(|| entry.id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First page of the documented `PaginatedBaseModels` shape
    /// (2026-09-14 research extraction): two base models, with
    /// `@nextLink` set so pagination continues.
    const PAGE_1: &str = r#"{
  "values": [
    {
      "self": "https://westus.cognitiveservices.azure.com/speechtotext/v3.2/models/base/8e0f3d2a-9a1b-4c5d-8e6f-0a1b2c3d4e5f",
      "kind": "Base",
      "displayName": "English (US) 20250601",
      "description": "English (United States) base model",
      "locale": "en-US",
      "createdDateTime": "2025-06-01T00:00:00Z",
      "lastActionDateTime": "2025-06-01T00:00:00Z",
      "status": "Succeeded",
      "properties": {
        "deprecationDates": {
          "transcription": "2027-06-01T00:00:00Z"
        }
      },
      "features": {
        "supportsTranscriptions": true,
        "supportsEndpoints": false,
        "supportsTranscriptionsOnSpeechContainers": true,
        "supportedOutputFormats": ["Display", "Simple"],
        "supportsAdaptationsWith": ["AcousticFilesTranscription", "LanguageFilesTranscription"]
      }
    },
    {
      "self": "https://westus.cognitiveservices.azure.com/speechtotext/v3.2/models/base/1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d",
      "kind": "Base",
      "displayName": "German (Germany) 20250601",
      "description": "German (Germany) base model",
      "locale": "de-DE",
      "createdDateTime": "2025-06-01T00:00:00Z",
      "lastActionDateTime": "2025-06-01T00:00:00Z",
      "status": "Succeeded",
      "properties": {
        "deprecationDates": {}
      },
      "features": {
        "supportsTranscriptions": true,
        "supportsEndpoints": false,
        "supportsTranscriptionsOnSpeechContainers": false,
        "supportedOutputFormats": ["Display"],
        "supportsAdaptationsWith": []
      }
    }
  ],
  "@nextLink": "https://westus.cognitiveservices.azure.com/speechtotext/v3.2/models/base?skip=2"
}"#;

    /// Second and final page: one model with no properties block.
    const PAGE_2: &str = r#"{
  "values": [
    {
      "self": "https://westus.cognitiveservices.azure.com/speechtotext/v3.2/models/base/9f8e7d6c-5b4a-3c2d-1e0f-9a8b7c6d5e4f",
      "kind": "Base",
      "displayName": "Japanese (Japan) 20241001",
      "locale": "ja-JP",
      "createdDateTime": "2024-10-01T00:00:00Z",
      "lastActionDateTime": "2024-10-01T00:00:00Z",
      "status": "Succeeded",
      "features": {
        "supportsTranscriptions": true,
        "supportsEndpoints": false,
        "supportsTranscriptionsOnSpeechContainers": false,
        "supportedOutputFormats": ["Display"],
        "supportsAdaptationsWith": []
      }
    }
  ]
}"#;

    fn page(json: &str) -> Page {
        serde_json::from_str(json).expect("fixture must parse as a page")
    }

    fn entries(json: &str) -> Vec<ModelEntry> {
        page(json).values.iter().map(normalize_model).collect()
    }

    #[test]
    fn absent_region_records_unavailable() {
        let err = region_from(None).expect_err("an absent region must fail the fetch");
        assert!(
            matches!(err, FetchError::MissingKey { .. }),
            "an absent region fails like a missing key: {err:?}"
        );
        assert!(
            err.to_string().contains("AZURE_SPEECH_REGION"),
            "the error names the missing variable: {err}"
        );
        let err = region_from(Some(String::new())).expect_err("a blank region must fail the fetch");
        assert!(matches!(err, FetchError::MissingKey { .. }));
    }

    #[test]
    fn base_model_maps_id_kind_and_dates() {
        let entry = &entries(PAGE_1)[0];
        assert_eq!(
            entry.id, "8e0f3d2a-9a1b-4c5d-8e6f-0a1b2c3d4e5f",
            "the id is the trailing UUID of the self URL"
        );
        assert_eq!(entry.display_name, "English (US) 20250601");
        assert_eq!(entry.kind, ModelKind::Transcription);
        assert_eq!(
            entry.released_at,
            Date::from_calendar_date(2025, time::Month::June, 1).ok(),
            "createdDateTime maps to released_at"
        );
        let deprecation = entry.deprecation.as_ref().expect("deprecation must map");
        assert_eq!(deprecation.status, "deprecated");
        assert_eq!(
            deprecation.date,
            Date::from_calendar_date(2027, time::Month::June, 1).ok(),
            "deprecationDates.transcription is the sunset date"
        );
        assert_eq!(deprecation.replacement, None);
        assert_eq!(
            entry.context_window, None,
            "a speech model reports no limits"
        );
    }

    #[test]
    fn empty_deprecation_dates_record_no_deprecation() {
        let entry = &entries(PAGE_1)[1];
        assert!(
            entry.deprecation.is_none(),
            "no transcription deprecation date, no deprecation entry"
        );
    }

    #[test]
    fn absent_properties_block_is_conservative() {
        let entry = &entries(PAGE_2)[0];
        assert_eq!(entry.id, "9f8e7d6c-5b4a-3c2d-1e0f-9a8b7c6d5e4f");
        assert_eq!(entry.kind, ModelKind::Transcription);
        assert!(entry.deprecation.is_none());
        assert_eq!(
            entry.released_at,
            Date::from_calendar_date(2024, time::Month::October, 1).ok()
        );
    }

    #[test]
    fn pagination_follows_next_link() {
        let first = page(PAGE_1);
        let link = next_link(&first).expect("a page with @nextLink must yield a link");
        assert!(
            link.ends_with("?skip=2"),
            "the link passes through verbatim: {link}"
        );
        let second = page(PAGE_2);
        assert_eq!(next_link(&second), None, "the final page ends traversal");
    }

    #[test]
    fn pagination_stops_on_empty_link() {
        let page = page(r#"{ "values": [], "@nextLink": "" }"#);
        assert_eq!(
            next_link(&page),
            None,
            "an empty link must not loop the fetch forever"
        );
    }

    #[test]
    fn locale_maps_to_languages_and_family() {
        let mut entries = entries(PAGE_1);
        assert_eq!(
            entries[0].languages,
            ["en-US"],
            "the per-locale base model lists its locale as its language"
        );
        apply_taxonomy(&mut entries);
        assert_eq!(entries[0].family, "en-US", "the locale is the family");
        assert_eq!(entries[1].family, "de-DE");
    }

    #[test]
    fn second_page_locale_maps_the_same_way() {
        let mut entries = entries(PAGE_2);
        assert_eq!(entries[0].languages, ["ja-JP"]);
        apply_taxonomy(&mut entries);
        assert_eq!(entries[0].family, "ja-JP");
    }

    #[test]
    fn absent_locale_falls_back_to_the_id() {
        let mut entries = entries(
            r#"{
  "values": [
    {
      "self": "https://westus.cognitiveservices.azure.com/speechtotext/v3.2/models/base/9f8e7d6c-5b4a-3c2d-1e0f-9a8b7c6d5e4f",
      "displayName": "Unlocalized 20241001"
    }
  ]
}"#,
        );
        assert!(entries[0].languages.is_empty());
        apply_taxonomy(&mut entries);
        assert_eq!(
            entries[0].family, entries[0].id,
            "a model with no locale is its own family"
        );
    }

    #[test]
    fn descriptor_publishes_key_and_region_and_no_chat_base() {
        assert_eq!(PROVIDER.openai_base_url, None);
        assert_eq!(PROVIDER.env_vars.len(), 2);
        assert_eq!(PROVIDER.env_vars[0].name, KEY_ENV);
        assert_eq!(PROVIDER.env_vars[0].role, gateway_api_types::EnvRole::Key);
        assert_eq!(PROVIDER.env_vars[0].default, None);
        assert_eq!(PROVIDER.env_vars[1].name, REGION_ENV);
        assert_eq!(
            PROVIDER.env_vars[1].role,
            gateway_api_types::EnvRole::Config
        );
        assert_eq!(PROVIDER.env_vars[1].default, None);
    }

    #[tokio::test]
    async fn fetch_sends_the_subscription_key_header() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let addr = listener.local_addr().expect("fixture server addr");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture client");
            // Read the request first, as in the sheet.rs fixture server:
            // replying before the client finishes sending is an HTTP
            // protocol error. The read timeout bounds the capture.
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
            let mut request = Vec::new();
            let mut buf = [0_u8; 4096];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{PAGE_2}",
                PAGE_2.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write fixture response");
            request
        });
        let client = reqwest::Client::new();
        let entries = fetch_all(&client, &format!("http://{addr}"), "test-key", "westus")
            .await
            .expect("a keyed fetch against the fixture must succeed");
        let request = String::from_utf8(server.join().expect("fixture server must finish"))
            .expect("the request is ASCII");
        assert!(
            request.contains("ocp-apim-subscription-key: test-key"),
            "the request includes the subscription key header: {request}"
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "9f8e7d6c-5b4a-3c2d-1e0f-9a8b7c6d5e4f");
    }
}
