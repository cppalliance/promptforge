//! Tests for splitting a client prompt into guidance terms.

use super::{GLOSSARY_BYTE_LIMIT, TERM_SEPARATOR, prompt_terms};

#[test]
fn a_comma_list_splits_into_trimmed_terms_in_order() {
    assert_eq!(prompt_terms("MCP,  GGUF ,Lua"), ["MCP", "GGUF", "Lua"]);
}

#[test]
fn empty_and_blank_terms_are_dropped() {
    assert_eq!(prompt_terms(" , MCP,, \t ,GGUF, "), ["MCP", "GGUF"]);
}

#[test]
fn a_prompt_without_terms_yields_none() {
    assert!(prompt_terms("").is_empty());
    assert!(prompt_terms(" ,, ").is_empty());
}

#[test]
fn a_prompt_without_commas_is_one_term_with_inner_spaces_kept() {
    assert_eq!(prompt_terms("  private prompt "), ["private prompt"]);
}

#[test]
fn an_oversized_term_list_keeps_only_the_leading_terms_that_fit_the_glossary() {
    let prompt = (0..100_000)
        .map(|index| format!("t{index}"))
        .collect::<Vec<_>>()
        .join(",");

    let terms = prompt_terms(&prompt);

    assert!(
        terms.len() < 300,
        "{} terms would all be rebuilt on every decode",
        terms.len()
    );
    for (index, term) in terms.iter().enumerate() {
        assert_eq!(
            term,
            &format!("t{index}"),
            "the leading terms are kept in order"
        );
    }
    let joined = terms.join(TERM_SEPARATOR);
    assert!(joined.len() <= GLOSSARY_BYTE_LIMIT);
    let next = format!("{joined}{TERM_SEPARATOR}t{}", terms.len());
    assert!(
        next.len() > GLOSSARY_BYTE_LIMIT,
        "the next term would not have fit, so none that fits was dropped"
    );
}

#[test]
fn a_run_of_one_byte_terms_is_capped_at_the_glossary_limit() {
    let terms = prompt_terms(&"a,".repeat(1_000_000));

    // Each term costs one byte plus a two-byte separator, less one separator.
    assert_eq!(
        terms.len(),
        (GLOSSARY_BYTE_LIMIT + TERM_SEPARATOR.len()) / 3
    );
    assert!(terms.iter().all(|term| term == "a"));
}

#[test]
fn a_term_longer_than_the_glossary_is_dropped_with_everything_after_it() {
    let oversized = "x".repeat(GLOSSARY_BYTE_LIMIT + 1);

    assert_eq!(prompt_terms(&format!("MCP,{oversized},GGUF")), ["MCP"]);
    assert!(prompt_terms(&oversized).is_empty());
}

#[test]
fn a_term_that_exactly_fills_the_glossary_is_kept() {
    let exact = "x".repeat(GLOSSARY_BYTE_LIMIT);

    assert_eq!(prompt_terms(&exact), [exact]);
}
