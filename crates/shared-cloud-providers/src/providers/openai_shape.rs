//! The shared OpenAI list-response shape: a `data` array in a `list`
//! envelope, Bearer auth, no pagination. Each provider speaking this
//! dialect defines its own wire model struct - with whatever extra fields
//! its endpoint reports - and its own normalization; this module carries
//! only the envelope, the single-request fetch, and the conservative base
//! entry every dialect entry starts from.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use shared_gateway_api::{ModelEntry, ModelKind, Thinking};
use time::OffsetDateTime;

use crate::FetchError;

/// The list envelope every OpenAI-dialect endpoint speaks.
#[derive(Debug, Deserialize)]
pub(crate) struct ListResponse<M> {
    /// The listed models.
    pub data: Vec<M>,
}

/// Fetch the whole list in one request; the dialect has no pagination.
pub(crate) async fn fetch_list<M: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    key: &str,
) -> Result<Vec<M>, FetchError> {
    let response: ListResponse<M> = client
        .get(url)
        .bearer_auth(key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(response.data)
}

/// The conservative base entry for one listed model: chat kind, every
/// capability false, every optional field empty. Provider files overwrite
/// the fields their endpoint actually reports; an IDs-only endpoint
/// yields this entry unchanged. The dialect reports no display name, so
/// the id doubles as the display name.
pub(crate) fn base_entry(id: &str, created: Option<i64>) -> ModelEntry {
    ModelEntry {
        id: id.to_owned(),
        display_name: id.to_owned(),
        kind: ModelKind::Chat,
        released_at: created
            .and_then(|unix| OffsetDateTime::from_unix_timestamp(unix).ok())
            .map(OffsetDateTime::date),
        context_window: None,
        max_output: None,
        images: false,
        pdf_input: false,
        video_input: false,
        audio_input: false,
        batch: false,
        citations: false,
        code_execution: false,
        structured_outputs: false,
        tool_calling: false,
        thinking: Thinking::default(),
        effort_levels: Vec::new(),
        default_effort: None,
        pricing: None,
        deprecation: None,
    }
}

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;

    #[test]
    fn base_entry_is_conservative() {
        let entry = base_entry("some-model", None);
        assert_eq!(entry.id, "some-model");
        assert_eq!(
            entry.display_name, "some-model",
            "the id doubles as the display name"
        );
        assert_eq!(entry.kind, ModelKind::Chat);
        assert_eq!(entry.released_at, None, "no created, no release date");
        assert_eq!(entry.context_window, None);
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.pdf_input);
        assert!(!entry.video_input && !entry.audio_input);
        assert!(!entry.batch && !entry.citations && !entry.code_execution);
        assert!(!entry.structured_outputs && !entry.tool_calling);
        assert!(!entry.thinking.supported);
        assert!(!entry.thinking.enabled && !entry.thinking.adaptive);
        assert!(entry.effort_levels.is_empty());
        assert_eq!(entry.default_effort, None);
        assert!(entry.pricing.is_none());
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn created_unix_seconds_map_to_a_calendar_date() {
        let entry = base_entry("some-model", Some(1_782_864_000));
        assert_eq!(
            entry.released_at,
            Date::from_calendar_date(2026, Month::July, 1).ok(),
            "1782864000 is 2026-07-01T00:00:00Z"
        );
    }
}
