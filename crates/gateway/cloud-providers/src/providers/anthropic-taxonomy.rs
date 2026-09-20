//! Anthropic taxonomy: the model line is the family, and `-YYYYMMDD`
//! dated snapshots collapse onto their canonical entry when the base id
//! is in the same list. Anthropic's rules live in this sibling module so
//! the provider file stays under the workspace's 500-line ceiling.

use gateway_api_types::ModelEntry;

use crate::taxonomy::{SnapshotStyle, collapse_variants, strip_snapshot};

/// The entry's family: the model line - `opus`, `sonnet`, `haiku`,
/// `fable` - taken as the segment after the `claude-` prefix. An id
/// without the prefix contributes its own first segment.
fn family_of(id: &str) -> String {
    let rest = id.strip_prefix("claude-").unwrap_or(id);
    rest.split('-').next().unwrap_or(rest).to_owned()
}

/// Sets every entry's family, then collapses `-YYYYMMDD` snapshots onto
/// their canonical entries.
pub(crate) fn apply(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
    collapse_variants(entries, |id| strip_snapshot(id, SnapshotStyle::CompactDate));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use gateway_api_types::ModelEntry;

    use super::apply;

    /// Trimmed 2026-09-14 sheet excerpt: the real Anthropic ids.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-anthropic.json");

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
    fn fixture_ids_classify_into_line_families() {
        let by_id = classified();
        let table: &[(&str, &str)] = &[
            ("claude-fable-5-1", "fable"),
            ("claude-opus-5", "opus"),
            ("claude-sonnet-5", "sonnet"),
            ("claude-fable-5", "fable"),
            ("claude-opus-4-8", "opus"),
            ("claude-opus-4-7", "opus"),
            ("claude-sonnet-4-6", "sonnet"),
            ("claude-opus-4-6", "opus"),
            ("claude-opus-4-5-20251101", "opus"),
            ("claude-haiku-4-5-20251001", "haiku"),
            ("claude-sonnet-4-5-20250929", "sonnet"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
    }

    #[test]
    fn dated_snapshots_without_a_canonical_stay_canonical() {
        let by_id = classified();
        // The 2026-09-14 sheet carries the dated snapshots but not their
        // base ids, so nothing collapses.
        for entry in by_id.values() {
            assert!(
                entry.variant_of.is_none(),
                "{} must stay canonical: its base id is not in the list",
                entry.id
            );
            assert!(entry.variant.is_none());
        }
    }

    #[test]
    fn a_dated_snapshot_collapses_onto_its_canonical_when_present() {
        let mut entries = vec![
            crate::taxonomy::fixture::entry("claude-opus-4-5"),
            crate::taxonomy::fixture::entry("claude-opus-4-5-20251101"),
        ];
        apply(&mut entries);
        let variant = &entries[1];
        assert_eq!(variant.variant_of.as_deref(), Some("claude-opus-4-5"));
        assert_eq!(variant.variant.as_deref(), Some("20251101"));
        assert_eq!(
            variant.family, "opus",
            "the variant inherits the canonical's family"
        );
    }
}
