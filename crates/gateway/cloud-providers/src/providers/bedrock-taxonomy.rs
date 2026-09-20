//! Bedrock taxonomy: the vendor prefix of the `<vendor>.<model>` id is
//! the family, and there is no snapshot collapse - the `:0`-style
//! revision suffix is a version marker, not a dated snapshot or SKU.
//! Bedrock's rules live in this sibling module so the provider file
//! stays under the workspace's 500-line ceiling.

use gateway_api::ModelEntry;

/// The entry's family: the vendor segment before the first dot
/// (`amazon`, `anthropic`, `meta`, ...), and the whole id otherwise.
fn family_of(id: &str) -> String {
    id.split_once('.')
        .map_or_else(|| id.to_owned(), |(vendor, _)| vendor.to_owned())
}

/// Sets every entry's family.
pub(crate) fn apply(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
}

#[cfg(test)]
mod tests {
    use gateway_api::ModelEntry;

    use super::apply;

    #[test]
    fn catalog_ids_classify_into_vendor_families() {
        // Bedrock was unprovisioned for the 2026-09-14 sheet, so the
        // table follows the documented catalog.
        let mut entries: Vec<ModelEntry> = [
            "amazon.nova-pro-v1:0",
            "amazon.nova-lite-v1:0",
            "amazon.titan-embed-text-v2:0",
            "anthropic.claude-v2:1",
            "anthropic.claude-sonnet-4-5-v1:0",
            "meta.llama3-1-70b-instruct-v1:0",
            "mistral.mistral-large-2407-v1:0",
            "deepseek.r1-v1:0",
        ]
        .iter()
        .map(|id| crate::taxonomy::fixture::entry(id))
        .collect();
        apply(&mut entries);
        let by_id: std::collections::BTreeMap<String, ModelEntry> = entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        let table: &[(&str, &str)] = &[
            ("amazon.nova-pro-v1:0", "amazon"),
            ("amazon.titan-embed-text-v2:0", "amazon"),
            ("anthropic.claude-v2:1", "anthropic"),
            ("anthropic.claude-sonnet-4-5-v1:0", "anthropic"),
            ("meta.llama3-1-70b-instruct-v1:0", "meta"),
            ("mistral.mistral-large-2407-v1:0", "mistral"),
            ("deepseek.r1-v1:0", "deepseek"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
        for entry in by_id.values() {
            assert!(
                entry.variant_of.is_none(),
                "the `:0` revision suffix is not a variant marker: {}",
                entry.id
            );
        }
    }

    #[test]
    fn an_id_without_a_vendor_prefix_is_its_own_family() {
        let mut entries = vec![crate::taxonomy::fixture::entry("nova-pro")];
        apply(&mut entries);
        assert_eq!(entries[0].family, "nova-pro");
    }
}
