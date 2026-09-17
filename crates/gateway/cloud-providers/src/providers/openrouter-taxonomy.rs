//! OpenRouter taxonomy: the vendor prefix of the `vendor/model` id is
//! the family, and `:free`/`:batch`-style SKU suffixes collapse onto
//! their canonical entry when the base id is in the same list. The
//! output-modality kind rule lives here too. OpenRouter's rules live in
//! this sibling module so the provider file stays under the workspace's
//! 500-line ceiling.

use gateway_api::{ModelEntry, ModelKind};

use crate::taxonomy::{collapse_variants, sku_suffix, vendor_prefix};

/// The entry's family: the vendor segment before the slash (`openai`,
/// `anthropic`, `z-ai`, ...), and the whole id otherwise.
fn family_of(id: &str) -> String {
    vendor_prefix(id).map_or_else(|| id.to_owned(), |(vendor, _)| vendor.to_owned())
}

/// The workload, from the output modalities: the first non-text output
/// is the model's product; a text-only model is chat.
pub(crate) fn model_kind(output_modalities: &[String]) -> ModelKind {
    for modality in output_modalities {
        match modality.as_str() {
            "embeddings" => return ModelKind::Embedding,
            "image" => return ModelKind::Image,
            "video" => return ModelKind::Video,
            "audio" | "speech" => return ModelKind::Speech,
            "transcription" => return ModelKind::Transcription,
            "rerank" => return ModelKind::Classifier,
            _ => {}
        }
    }
    ModelKind::Chat
}

/// Set every entry's family, then collapse `:free`/`:batch` SKU
/// suffixes onto their canonical entries.
pub(crate) fn apply(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
    collapse_variants(entries, sku_suffix);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use gateway_api::{ModelEntry, ModelKind};

    use super::{apply, model_kind};

    /// Trimmed 2026-09-14 sheet excerpt: real OpenRouter ids, SKU
    /// variants included.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-openrouter.json");

    /// The fixture ids with the provider's taxonomy applied, by id.
    fn classified() -> BTreeMap<String, ModelEntry> {
        let mut entries = crate::taxonomy::fixture::entries(FIXTURE);
        apply(&mut entries);
        entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect()
    }

    #[test]
    fn fixture_ids_classify_into_vendor_families() {
        let by_id = classified();
        let table: &[(&str, &str)] = &[
            ("openai/gpt-5.2", "openai"),
            ("openai/gpt-6-astra", "openai"),
            ("anthropic/claude-fable-5.1", "anthropic"),
            ("google/gemini-2.5-flash", "google"),
            ("deepseek/deepseek-v4-flash-0731", "deepseek"),
            ("qwen/qwen3-235b-a22b", "qwen"),
            ("x-ai/grok-4.3", "x-ai"),
            ("z-ai/glm-5.3", "z-ai"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
    }

    #[test]
    fn sku_variants_collapse_onto_present_aliases() {
        let by_id = classified();
        let table: &[(&str, &str, &str)] = &[
            ("openai/gpt-6-astra:batch", "openai/gpt-6-astra", "batch"),
            (
                "anthropic/claude-fable-5.1:batch",
                "anthropic/claude-fable-5.1",
                "batch",
            ),
            (
                "google/gemini-2.5-flash:batch",
                "google/gemini-2.5-flash",
                "batch",
            ),
            (
                "deepseek/deepseek-v4-flash-0731:batch",
                "deepseek/deepseek-v4-flash-0731",
                "batch",
            ),
            ("x-ai/grok-4.3:batch", "x-ai/grok-4.3", "batch"),
        ];
        for &(id, base, sku) in table {
            let entry = &by_id[id];
            assert_eq!(entry.variant_of.as_deref(), Some(base), "{id}");
            assert_eq!(entry.variant.as_deref(), Some(sku), "{id}");
            assert_eq!(
                entry.family, by_id[base].family,
                "{id} inherits its canonical's family"
            );
        }
    }

    #[test]
    fn every_output_modality_maps_to_its_kind() {
        let cases: &[(&[&str], ModelKind)] = &[
            (&["embeddings"], ModelKind::Embedding),
            (&["image"], ModelKind::Image),
            (&["video"], ModelKind::Video),
            (&["audio"], ModelKind::Speech),
            (&["speech"], ModelKind::Speech),
            (&["transcription"], ModelKind::Transcription),
            (&["rerank"], ModelKind::Classifier),
            (&["text"], ModelKind::Chat),
        ];
        for (modalities, kind) in cases {
            let owned: Vec<String> = modalities.iter().map(|m| (*m).to_owned()).collect();
            assert_eq!(model_kind(&owned), *kind, "{modalities:?}");
        }
    }

    #[test]
    fn a_sku_without_a_canonical_stays_canonical() {
        let by_id = classified();
        // `cohere/north-mini-code:free` has no `cohere/north-mini-code`
        // alias in the list.
        let entry = &by_id["cohere/north-mini-code:free"];
        assert!(
            entry.variant_of.is_none(),
            "the base id is not in the list, so the entry stays canonical"
        );
        assert_eq!(entry.family, "cohere", "the vendor prefix still applies");
        for entry in by_id.values() {
            assert!(!entry.family.is_empty(), "{} has an empty family", entry.id);
        }
    }
}
