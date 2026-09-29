//! Tests for the control-markup inventory: table sanity, and the matcher's
//! spellings, bounds, and prose refusals.

use super::*;

/// Every full spelling of every Pipe, BareTag, and Literal table entry.
fn spellings() -> Vec<String> {
    let mut out = Vec::new();
    for group in CONTROL_MARKUP {
        for name in group.names {
            match group.shape {
                Shape::Pipe => {
                    out.push(format!("<|{name}|>"));
                    out.push(format!("<|{name}>"));
                    out.push(format!("<|/{name}|>"));
                    out.push(format!("<|/{name}>"));
                }
                Shape::BareTag => {
                    out.push(format!("<{name}>"));
                    out.push(format!("</{name}>"));
                }
                Shape::Literal => out.push((*name).to_owned()),
                Shape::DoubledAngle => {}
            }
        }
    }
    out
}

#[test]
fn every_table_spelling_matches_in_full() {
    for s in spellings() {
        assert_eq!(
            delimiter_len(&s, false),
            Some(s.len()),
            "inventory spelling {s:?} did not match in full"
        );
    }
}

#[test]
fn inventory_has_no_duplicate_spellings() {
    let all = spellings();
    let mut deduped = all.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(
        all.len(),
        deduped.len(),
        "duplicate spellings in the inventory"
    );
}

#[test]
fn every_group_names_a_family_and_entries() {
    for group in CONTROL_MARKUP {
        assert!(
            !group.family.is_empty(),
            "a group is missing its family label"
        );
        assert!(
            !group.names.is_empty(),
            "family {:?} has no entries",
            group.family
        );
    }
}

#[test]
fn fullwidth_class_matches_known_and_novel_spellings() {
    let user = "<\u{ff5c}User\u{ff5c}>";
    assert_eq!(delimiter_len(user, false), Some(user.len()));
    let sentence = "<\u{ff5c}begin\u{2581}of\u{2581}sentence\u{ff5c}>";
    assert_eq!(delimiter_len(sentence, false), Some(sentence.len()));
    let novel = "<\u{ff5c}SomeNew\u{ff5c}>";
    assert_eq!(
        delimiter_len(novel, false),
        Some(novel.len()),
        "the open class admits spellings added after the port"
    );
}

#[test]
fn fullwidth_class_stays_bounded_and_off_prose() {
    assert_eq!(
        delimiter_len("<\u{ff5c}\u{ff5c}>", false),
        None,
        "the name must start with a letter"
    );
    let at_bound = format!("<\u{ff5c}a{}\u{ff5c}>", "b".repeat(FULLWIDTH_NAME_MAX));
    assert_eq!(
        delimiter_len(&at_bound, false),
        Some(at_bound.len()),
        "a name at the 39-character bound still matches"
    );
    let past_bound = format!("<\u{ff5c}a{}\u{ff5c}>", "b".repeat(FULLWIDTH_NAME_MAX + 1));
    assert_eq!(
        delimiter_len(&past_bound, false),
        None,
        "the class is bounded"
    );
    assert_eq!(
        delimiter_len("<\u{ff5c} \u{ff5c}>", false),
        None,
        "fullwidth punctuation pairs stay as typed"
    );
}

#[test]
fn doubled_angle_sys_matches_only_at_the_second_bracket() {
    assert_eq!(delimiter_len("<<SYS>>", false), None);
    assert_eq!(delimiter_len("<SYS>>", true), Some("<SYS>>".len()));
    assert_eq!(delimiter_len("</SYS>>", true), Some("</SYS>>".len()));
    assert_eq!(
        delimiter_len("<SYS>>", false),
        None,
        "a single angle stays as typed"
    );
    assert_eq!(
        delimiter_len("< SYS>>", true),
        None,
        "the spaced form is stable"
    );
}

#[test]
fn closed_inventory_leaves_prose_alone() {
    let prose = [
        "<div>",
        "<span class=\"x\">",
        "List<String>",
        "[1]",
        "[inst]",
        "[UNKNOWN]",
        "<s >",
        "<tool>",
        "<|tool|",
        "<|unknown_name|>",
    ];
    for text in prose {
        assert_eq!(
            delimiter_len(text, false),
            None,
            "{text:?} must stay as typed"
        );
    }
}
