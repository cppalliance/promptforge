//! Vendor-agnostic taxonomy primitives: fixed-width, hand-parsed
//! snapshot-suffix strippers, the `vendor/model` prefix splitter, the
//! `:sku` suffix splitter, and the variant-collapse post-pass. Which
//! primitives apply, and the family rule itself, stay private to each
//! provider file.

use gateway_api::ModelEntry;

/// The snapshot-suffix styles observed across provider catalogs. Every
/// style is fixed-width and hand-parsed; there is no regex dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapshotStyle {
    /// `-YYYY-MM-DD`, e.g. `gpt-5.4-2026-03-05`.
    DashedDate,
    /// `-YYYYMMDD`, e.g. `claude-opus-4-5-20251101`.
    CompactDate,
    /// `-MMDD`, e.g. `grok-4.20-multi-agent-0309`.
    MonthDay,
    /// `-YYMM`, e.g. `mistral-large-2508`.
    YearMonth,
    /// `-MM-YYYY`, e.g. `deep-research-pro-preview-12-2025`.
    MonthYear,
}

/// Whether every byte of `text` is an ASCII digit, with at least one.
fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// Parse a digit run already validated by [`digits`].
fn number(text: &str) -> u32 {
    text.bytes()
        .fold(0u32, |acc, b| acc * 10 + u32::from(b - b'0'))
}

/// Whether the value is a calendar month number.
fn valid_month(month: u32) -> bool {
    (1..=12).contains(&month)
}

/// Whether the value is a calendar day number.
fn valid_day(day: u32) -> bool {
    (1..=31).contains(&day)
}

/// Whether the token is a version number: digits and dots with at least
/// one digit (`2.5`, `3`, `4.20`).
pub(crate) fn is_version_token(token: &str) -> bool {
    !token.is_empty()
        && token.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && token.bytes().any(|b| b.is_ascii_digit())
}

/// Whether the token is a version number with an optional trailing `o`
/// (`5`, `5.1`, `4o`): the OpenAI `gpt-4o` naming style.
pub(crate) fn is_version_token_o(token: &str) -> bool {
    is_version_token(token.strip_suffix('o').unwrap_or(token))
}

/// Split a snapshot suffix of the given style off `id`, returning the
/// base id and the suffix text without its leading dash. Returns `None`
/// when the trailing bytes are not a well-formed date in the style - a
/// wrong width, non-digit bytes, or impossible month or day numbers. A
/// four-digit suffix is deliberately ambiguous between [`SnapshotStyle::MonthDay`]
/// and [`SnapshotStyle::YearMonth`]; the provider picks its catalog's
/// style.
pub(crate) fn strip_snapshot(id: &str, style: SnapshotStyle) -> Option<(&str, &str)> {
    if !id.is_ascii() {
        return None;
    }
    let width = match style {
        SnapshotStyle::DashedDate => 11,
        SnapshotStyle::CompactDate => 9,
        SnapshotStyle::MonthDay | SnapshotStyle::YearMonth => 5,
        SnapshotStyle::MonthYear => 8,
    };
    if id.len() <= width {
        return None;
    }
    let (base, suffix) = id.split_at(id.len() - width);
    let suffix = suffix.strip_prefix('-')?;
    let valid = match style {
        SnapshotStyle::DashedDate => {
            digits(&suffix[0..4])
                && &suffix[4..5] == "-"
                && digits(&suffix[5..7])
                && &suffix[7..8] == "-"
                && digits(&suffix[8..10])
                && valid_month(number(&suffix[5..7]))
                && valid_day(number(&suffix[8..10]))
        }
        SnapshotStyle::CompactDate => {
            digits(suffix) && valid_month(number(&suffix[4..6])) && valid_day(number(&suffix[6..8]))
        }
        SnapshotStyle::MonthDay => {
            digits(suffix) && valid_month(number(&suffix[0..2])) && valid_day(number(&suffix[2..4]))
        }
        SnapshotStyle::YearMonth => digits(suffix) && valid_month(number(&suffix[2..4])),
        SnapshotStyle::MonthYear => {
            digits(&suffix[0..2])
                && &suffix[2..3] == "-"
                && digits(&suffix[3..7])
                && valid_month(number(&suffix[0..2]))
        }
    };
    valid.then_some((base, suffix))
}

/// Split a `vendor/model` id into its vendor prefix and model id, on the
/// first slash. Returns `None` when there is no slash or either side is
/// empty.
pub(crate) fn vendor_prefix(id: &str) -> Option<(&str, &str)> {
    let (vendor, model) = id.split_once('/')?;
    (!vendor.is_empty() && !model.is_empty()).then_some((vendor, model))
}

/// Split a `:free`/`:batch`-style SKU suffix off `id`, on the last colon,
/// returning the base id and the SKU text. Returns `None` when there is
/// no colon or either side is empty.
pub(crate) fn sku_suffix(id: &str) -> Option<(&str, &str)> {
    let (base, sku) = id.rsplit_once(':')?;
    (!base.is_empty() && !sku.is_empty()).then_some((base, sku))
}

/// The variant-collapse post-pass: for every entry whose id splits into
/// a base and a suffix, when the base id exists in the same list, mark
/// the entry as a variant of that canonical entry and copy the
/// canonical's family onto it. An entry whose base id is absent stays
/// canonical - the pass never emits a pointer to a nonexistent alias.
/// Families must already be set; the canonical's family is read as it
/// was before the pass.
pub(crate) fn collapse_variants(
    entries: &mut [ModelEntry],
    split: impl Fn(&str) -> Option<(&str, &str)>,
) {
    let families: std::collections::BTreeMap<String, String> = entries
        .iter()
        .map(|entry| (entry.id.clone(), entry.family.clone()))
        .collect();
    for entry in entries.iter_mut() {
        let Some((base, suffix)) = split(&entry.id) else {
            continue;
        };
        let Some(family) = families.get(base) else {
            continue;
        };
        entry.family = family.clone();
        entry.variant_of = Some(base.to_owned());
        entry.variant = Some(suffix.to_owned());
    }
}

#[cfg(test)]
pub(crate) mod fixture {
    use gateway_api::{ModelEntry, ModelKind, Thinking};
    use serde::Deserialize;

    /// A trimmed sheet excerpt: one provider's models reduced to ids.
    #[derive(Deserialize)]
    struct Fixture {
        models: Vec<FixtureModel>,
    }

    /// One model in a trimmed excerpt; the taxonomy reads only the id.
    #[derive(Deserialize)]
    struct FixtureModel {
        id: String,
    }

    /// A minimal entry carrying an id and nothing else, for taxonomy
    /// tests that need a list without a wire payload.
    pub(crate) fn entry(id: &str) -> ModelEntry {
        ModelEntry {
            id: id.to_owned(),
            display_name: id.to_owned(),
            family: String::new(),
            variant_of: None,
            variant: None,
            languages: Vec::new(),
            kind: ModelKind::Chat,
            released_at: None,
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

    /// Minimal entries carrying the real ids from a trimmed 2026-09-14
    /// sheet excerpt.
    pub(crate) fn entries(json: &str) -> Vec<ModelEntry> {
        let fixture: Fixture = serde_json::from_str(json).expect("the fixture must parse");
        fixture
            .models
            .iter()
            .map(|model| entry(&model.id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{entries, entry};
    use super::{
        SnapshotStyle, collapse_variants, is_version_token, is_version_token_o, sku_suffix,
        strip_snapshot, vendor_prefix,
    };

    #[test]
    fn version_token_accepts_digits_and_dots_with_a_digit() {
        assert!(is_version_token("2.5"));
        assert!(is_version_token("3"));
        assert!(is_version_token("4.20"));
        assert!(!is_version_token(""), "an empty token is no version");
        assert!(!is_version_token("."), "dots alone carry no digit");
        assert!(!is_version_token("4o"), "a letter is not a version byte");
    }

    #[test]
    fn version_token_o_tolerates_one_trailing_o() {
        assert!(is_version_token_o("4o"), "the gpt-4o naming style");
        assert!(is_version_token_o("5.1"));
        assert!(!is_version_token_o("o"), "the o alone carries no digit");
        assert!(
            !is_version_token_o("4o5"),
            "the o is a suffix, not an infix"
        );
    }

    #[test]
    fn dashed_date_strips_a_dated_suffix() {
        assert_eq!(
            strip_snapshot("gpt-5.4-2026-03-05", SnapshotStyle::DashedDate),
            Some(("gpt-5.4", "2026-03-05")),
            "the base id and the suffix text come apart"
        );
    }

    #[test]
    fn dashed_date_rejects_bad_month_wrong_width_and_absent_suffix() {
        assert_eq!(
            strip_snapshot("m-2026-13-05", SnapshotStyle::DashedDate),
            None,
            "month 13 is not a date"
        );
        assert_eq!(
            strip_snapshot("m-2026-03-5", SnapshotStyle::DashedDate),
            None,
            "the style is fixed-width"
        );
        assert_eq!(strip_snapshot("gpt-5.4", SnapshotStyle::DashedDate), None);
    }

    #[test]
    fn compact_date_strips_an_eight_digit_suffix() {
        assert_eq!(
            strip_snapshot("claude-opus-4-5-20251101", SnapshotStyle::CompactDate),
            Some(("claude-opus-4-5", "20251101"))
        );
        assert_eq!(
            strip_snapshot("m-20251301", SnapshotStyle::CompactDate),
            None,
            "month 13 is not a date"
        );
        assert_eq!(
            strip_snapshot("m-2025110", SnapshotStyle::CompactDate),
            None,
            "seven digits is the wrong width"
        );
    }

    #[test]
    fn month_day_strips_a_four_digit_suffix() {
        assert_eq!(
            strip_snapshot("grok-4.20-multi-agent-0309", SnapshotStyle::MonthDay),
            Some(("grok-4.20-multi-agent", "0309"))
        );
        assert_eq!(
            strip_snapshot("m-1260", SnapshotStyle::MonthDay),
            None,
            "day 60 is not a date"
        );
    }

    #[test]
    fn year_month_strips_a_four_digit_suffix() {
        assert_eq!(
            strip_snapshot("mistral-large-2508", SnapshotStyle::YearMonth),
            Some(("mistral-large", "2508"))
        );
        assert_eq!(
            strip_snapshot("m-2513", SnapshotStyle::YearMonth),
            None,
            "month 13 is not a date"
        );
    }

    #[test]
    fn ambiguous_four_digit_suffix_resolves_per_provider_choice() {
        // `-2508` as month-day has month 25, which is not a date; as
        // year-month it is August 2025. The primitive stays neutral: each
        // provider picks the style its catalog uses.
        assert_eq!(
            strip_snapshot("mistral-large-2508", SnapshotStyle::MonthDay),
            None
        );
        assert_eq!(
            strip_snapshot("mistral-large-2508", SnapshotStyle::YearMonth),
            Some(("mistral-large", "2508"))
        );
    }

    #[test]
    fn month_year_strips_a_dashed_suffix() {
        assert_eq!(
            strip_snapshot(
                "deep-research-pro-preview-12-2025",
                SnapshotStyle::MonthYear
            ),
            Some(("deep-research-pro-preview", "12-2025"))
        );
        assert_eq!(
            strip_snapshot("m-13-2025", SnapshotStyle::MonthYear),
            None,
            "month 13 is not a date"
        );
    }

    #[test]
    fn vendor_prefix_splits_on_the_slash() {
        assert_eq!(
            vendor_prefix("openai/gpt-6-astra"),
            Some(("openai", "gpt-6-astra"))
        );
        assert_eq!(vendor_prefix("gpt-5.4"), None, "no slash, no vendor");
        assert_eq!(vendor_prefix("openai/"), None, "an empty model is no split");
    }

    #[test]
    fn sku_suffix_splits_on_the_last_colon() {
        assert_eq!(
            sku_suffix("openai/gpt-6-astra:batch"),
            Some(("openai/gpt-6-astra", "batch")),
            "the vendor prefix survives the sku split"
        );
        assert_eq!(sku_suffix("openai/gpt-6-astra"), None, "no colon, no sku");
        assert_eq!(sku_suffix("model:"), None, "an empty sku is no split");
    }

    #[test]
    fn collapse_sets_variant_fields_and_copies_the_canonical_family() {
        let mut list = vec![entry("gpt-5.4"), entry("gpt-5.4-2026-03-05")];
        list[0].family = "gpt-5.4".to_owned();
        collapse_variants(&mut list, |id| {
            strip_snapshot(id, SnapshotStyle::DashedDate)
        });
        assert!(
            list[0].variant_of.is_none(),
            "the canonical stays canonical"
        );
        assert_eq!(list[1].variant_of.as_deref(), Some("gpt-5.4"));
        assert_eq!(list[1].variant.as_deref(), Some("2026-03-05"));
        assert_eq!(
            list[1].family, "gpt-5.4",
            "the variant inherits the canonical's family"
        );
    }

    #[test]
    fn collapse_refuses_a_pointer_to_a_nonexistent_alias() {
        let mut list = vec![entry("claude-opus-4-5-20251101")];
        list[0].family = "opus".to_owned();
        collapse_variants(&mut list, |id| {
            strip_snapshot(id, SnapshotStyle::CompactDate)
        });
        assert!(
            list[0].variant_of.is_none(),
            "the base id is not in the list, so the entry stays canonical"
        );
        assert!(list[0].variant.is_none());
        assert_eq!(list[0].family, "opus", "the entry keeps its own family");
    }

    #[test]
    fn collapse_leaves_suffix_less_ids_alone() {
        let mut list = entries(r#"{ "models": [ { "id": "gpt-5.4" }, { "id": "gpt-4o" } ] }"#);
        collapse_variants(&mut list, |id| {
            strip_snapshot(id, SnapshotStyle::DashedDate)
        });
        assert!(list.iter().all(|e| e.variant_of.is_none()));
    }
}
