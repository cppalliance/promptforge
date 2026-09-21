//! Azure AI Foundry taxonomy: the publisher is the family, and the
//! card's inference tasks name the workload, falling back to the output
//! modality for a task vocabulary this mapping does not know. Foundry's
//! rules sit in this sibling module so the provider file stays under
//! the workspace's 500-line ceiling.

use gateway_api_types::{Deprecation, ModelEntry, ModelKind};
use time::Date;

/// The lifecycle labels that mean a model is on its way out. Every
/// other label - generally available, preview - is a live model, and
/// the comparison is lowercased because the catalog spells the same
/// label both "Generally Available" and "Generally available".
const SUNSET_LIFECYCLES: &[&str] = &["retired", "deprecated", "legacy"];

/// Sunset information from the lifecycle label and the parsed
/// retirement date. A live label with no date is no deprecation; a date
/// alone still is one, under a neutral status, because a retirement
/// date is the stronger signal.
pub(crate) fn deprecation_of(lifecycle: Option<&str>, date: Option<Date>) -> Option<Deprecation> {
    let label = lifecycle
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_lowercase);
    let sunset = label
        .as_deref()
        .is_some_and(|label| SUNSET_LIFECYCLES.contains(&label));
    if !sunset && date.is_none() {
        return None;
    }
    Some(Deprecation {
        status: if sunset {
            label.unwrap_or_default()
        } else {
            "retires".to_owned()
        },
        date,
        replacement: None,
    })
}

/// The family for one card: the publisher, lowercased. The catalog
/// spans many vendors' models under one Azure offering, so the
/// publisher is the useful first grouping level, as with the registry's
/// other multi-vendor catalog. A card with no publisher yields `None`
/// and keeps its id as the family.
pub(crate) fn family_of(publisher: Option<&str>) -> Option<String> {
    publisher
        .map(str::trim)
        .filter(|publisher| !publisher.is_empty())
        .map(str::to_lowercase)
}

/// The workload, from the card's inference tasks. An unrecognized task
/// falls through to the output modality, so a task Azure adds later
/// still lands in the right bucket instead of defaulting to chat.
pub(crate) fn model_kind(tasks: &[String], output_modalities: &[String]) -> ModelKind {
    for task in tasks {
        let kind = match task.as_str() {
            "chat-completion" | "chat-completions" | "responses" | "messages"
            | "text-generation" => ModelKind::Chat,
            "embeddings" => ModelKind::Embedding,
            "text-to-image" | "image-to-image" => ModelKind::Image,
            "text-to-speech" | "audio-generation" => ModelKind::Speech,
            "speech-to-text" | "automatic-speech-recognition" | "speech-translation" => {
                ModelKind::Transcription
            }
            "text-classification"
            | "image-classification"
            | "image-to-text"
            | "summarization"
            | "translation"
            | "detect-language"
            | "data-generation"
            | "health-entity-extraction"
            | "document-pii-extraction" => ModelKind::Classifier,
            _ => continue,
        };
        return kind;
    }
    kind_from_outputs(output_modalities)
}

/// The workload from the output modality alone, for cards whose task
/// list names nothing the sheet models.
fn kind_from_outputs(output_modalities: &[String]) -> ModelKind {
    let has = |wanted: &str| output_modalities.iter().any(|modality| modality == wanted);
    if has("embeddings") {
        ModelKind::Embedding
    } else if has("video") {
        ModelKind::Video
    } else if has("image") {
        ModelKind::Image
    } else if has("audio") {
        ModelKind::Speech
    } else {
        ModelKind::Chat
    }
}

/// Fills the family for entries with no publisher - including the
/// id-only entries the registry's taxonomy tests build - so every entry
/// leaves the fetch with one. Normalization sets the publisher family
/// from the card; there is no snapshot collapse, because catalog slugs
/// have no dated suffixes.
pub(crate) fn apply(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        if entry.family.is_empty() {
            entry.family = entry.id.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn publisher_is_the_family_lowercased() {
        assert_eq!(
            family_of(Some("Black Forest Labs")).as_deref(),
            Some("black forest labs")
        );
        assert_eq!(family_of(Some("  OpenAI  ")).as_deref(), Some("openai"));
        assert_eq!(family_of(Some("   ")), None, "a blank publisher is none");
        assert_eq!(family_of(None), None);
    }

    #[test]
    fn inference_tasks_name_the_workload() {
        let cases: &[(&str, ModelKind)] = &[
            ("messages", ModelKind::Chat),
            ("responses", ModelKind::Chat),
            ("chat-completion", ModelKind::Chat),
            ("embeddings", ModelKind::Embedding),
            ("text-to-image", ModelKind::Image),
            ("text-to-speech", ModelKind::Speech),
            ("speech-to-text", ModelKind::Transcription),
            ("automatic-speech-recognition", ModelKind::Transcription),
            ("text-classification", ModelKind::Classifier),
            ("document-pii-extraction", ModelKind::Classifier),
        ];
        for &(task, expected) in cases {
            assert_eq!(model_kind(&strings(&[task]), &[]), expected, "task {task}");
        }
    }

    #[test]
    fn the_first_recognized_task_wins() {
        assert_eq!(
            model_kind(&strings(&["text-to-image", "image-to-image"]), &[]),
            ModelKind::Image
        );
    }

    #[test]
    fn an_unknown_task_falls_back_to_the_output_modality() {
        let unknown = strings(&["some-future-task"]);
        assert_eq!(
            model_kind(&unknown, &strings(&["embeddings"])),
            ModelKind::Embedding
        );
        assert_eq!(model_kind(&unknown, &strings(&["video"])), ModelKind::Video);
        assert_eq!(
            model_kind(&unknown, &strings(&["text", "image"])),
            ModelKind::Image
        );
        assert_eq!(
            model_kind(&unknown, &strings(&["audio"])),
            ModelKind::Speech
        );
        assert_eq!(
            model_kind(&unknown, &strings(&["text"])),
            ModelKind::Chat,
            "a text-only card with no known task is chat"
        );
        assert_eq!(
            model_kind(&[], &[]),
            ModelKind::Chat,
            "a card reporting nothing at all is chat"
        );
    }

    #[test]
    fn live_lifecycle_spellings_are_not_deprecations() {
        for label in ["Generally Available", "Generally available", "Preview"] {
            let live = deprecation_of(Some(label), None);
            assert!(live.is_none(), "{label} is a live model");
        }
        let legacy = deprecation_of(Some("Legacy"), None).expect("Legacy is a sunset label");
        assert_eq!(legacy.status, "legacy");
        assert_eq!(legacy.date, None, "a sunset label alone carries no date");
        let dated = deprecation_of(Some("Preview"), Date::from_ordinal_date(2027, 1).ok())
            .expect("a retirement date deprecates whatever the label says");
        assert_eq!(
            dated.status, "retires",
            "a date under a live label gets a neutral status"
        );
        assert!(deprecation_of(None, None).is_none(), "nothing reported");
    }

    #[test]
    fn apply_fills_only_empty_families() {
        let mut entries = vec![
            crate::taxonomy::fixture::entry("gpt-5.4"),
            crate::taxonomy::fixture::entry("claude-mythos-5-1"),
        ];
        entries[1].family = "anthropic".to_owned();
        apply(&mut entries);
        assert_eq!(
            entries[0].family, "gpt-5.4",
            "an entry with no publisher keeps its id as the family"
        );
        assert_eq!(
            entries[1].family, "anthropic",
            "a publisher family set during normalization survives"
        );
        assert!(entries.iter().all(|entry| entry.variant_of.is_none()));
    }
}
