//! Tests for the `web_search` result post-process pipeline and its helpers.

use super::*;

fn hit(title: &str, url: &str) -> SearchResult {
    SearchResult {
        title: title.to_string(),
        url: url.to_string(),
        description: String::new(),
        age: None,
        site_name: None,
        extra_snippets: Vec::new(),
    }
}

#[test]
fn sanitize_text_strips_controls_collapses_and_decodes() {
    let input = "A\u{0001}B\nC\tD   &amp; &lt;x&gt; &quot;q&#39; &apos;z";
    let out = sanitize_text(input, TITLE_MAX_CHARS);
    assert_eq!(out, "AB C D & <x> \"q' 'z");
}

#[test]
fn sanitize_text_caps_title_description_and_url_limits() {
    let title = "t".repeat(TITLE_MAX_CHARS + 10);
    assert_eq!(
        sanitize_text(&title, TITLE_MAX_CHARS).chars().count(),
        TITLE_MAX_CHARS
    );

    let desc = "d".repeat(DESCRIPTION_MAX_CHARS + 3);
    assert_eq!(
        sanitize_text(&desc, DESCRIPTION_MAX_CHARS).chars().count(),
        DESCRIPTION_MAX_CHARS
    );
}

#[test]
fn overlong_url_result_is_dropped_not_truncated() {
    // WSP-004: a URL longer than the cap is dropped whole rather than cut
    // mid-component into a broken link.
    let long = format!("https://example.com/{}", "u".repeat(URL_MAX_CHARS));
    let ok = hit("Keep", "https://a.com/1");
    let dropped = hit("Drop", &long);
    let out = post_process_results(vec![ok, dropped], true, &[], &[], 10, 10);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].title, "Keep");
}

#[test]
fn non_navigable_result_urls_are_dropped() {
    // WSP-003: a result whose URL is not a standards-parsed http(s) URL is
    // dropped rather than emitted as a broken link.
    assert!(is_navigable_url("https://example.com/path"));
    assert!(is_navigable_url("http://a.com"));
    assert!(!is_navigable_url("not-a-url"));
    assert!(!is_navigable_url("ftp://host/file"));
    assert!(!is_navigable_url("https:///"));
    let keep = hit("Keep", "https://a.com/1");
    let bogus = hit("Bogus", "not-a-url");
    let scheme = hit("Ftp", "ftp://a.com/x");
    let out = post_process_results(vec![keep, bogus, scheme], false, &[], &[], 10, 10);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].title, "Keep");
}

#[test]
fn strip_tracking_removes_utm_and_fbclid() {
    let url =
        "https://a.com/x?utm_source=1&keep=yes&fbclid=abc&gclid=1&mc_cid=2&mc_eid=3&utm_medium=x";
    assert_eq!(strip_tracking_params(url), "https://a.com/x?keep=yes");

    let only_track = "https://a.com/x?utm_source=1";
    assert_eq!(strip_tracking_params(only_track), "https://a.com/x");
}

#[test]
fn host_and_site_name_helpers() {
    assert_eq!(
        host_from_url("https://WWW.Example.COM:443/path"),
        Some("www.example.com".to_string())
    );
    assert_eq!(site_name_from_host("www.example.com"), "example.com");
    assert_eq!(site_name_from_host("example.com"), "example.com");
    assert_eq!(host_from_url("not-a-url"), Some("not-a-url".to_string()));
    assert_eq!(host_from_url("https:///"), None);
}

#[test]
fn age_is_sanitized_and_capped() {
    // WSP-001: provider-controlled `age` is sanitized (controls dropped) and
    // capped like every other retained string.
    let mut result = hit("T", "https://a.com/1");
    result.age = Some(format!("2 days\u{0001}ago {}", "x".repeat(AGE_MAX_CHARS)));
    let out = post_process_results(vec![result], false, &[], &[], 10, 10);
    let age = out[0].age.as_deref().expect("age kept");
    assert!(age.chars().count() <= AGE_MAX_CHARS);
    assert!(!age.contains('\u{0001}'));
}

#[test]
fn extra_snippets_are_capped_in_count_and_length() {
    let mut result = hit("T", "https://a.com/1");
    result.extra_snippets = (0..20).map(|i| format!("snippet {i}")).collect();
    result
        .extra_snippets
        .push("x".repeat(SNIPPET_MAX_CHARS + 50));
    let out = post_process_results(vec![result], false, &[], &[], 10, 10);
    assert_eq!(out.len(), 1);
    assert!(out[0].extra_snippets.len() <= MAX_EXTRA_SNIPPETS);
    for snippet in &out[0].extra_snippets {
        assert!(snippet.chars().count() <= SNIPPET_MAX_CHARS);
    }
}

#[test]
fn host_from_url_uses_real_parsing_then_lenient_fallback() {
    // Well-formed URL: standards parse.
    assert_eq!(
        host_from_url("https://WWW.Example.COM:8443/x?a=b#f"),
        Some("www.example.com".to_owned())
    );
    // Scheme-less host/path: lenient fallback still yields the host.
    assert_eq!(
        host_from_url("sub.example.com/path"),
        Some("sub.example.com".to_owned())
    );
}

#[test]
fn filter_domains_include_then_exclude() {
    let results = vec![
        hit("A", "https://a.com/1"),
        hit("B", "https://b.com/1"),
        hit("Sub", "https://sub.a.com/1"),
        hit("C", "https://c.com/1"),
    ];
    let include = vec!["a.com".to_string()];
    let included = filter_domains(results, &include, &[]);
    assert_eq!(included.len(), 2);
    assert_eq!(included[0].url, "https://a.com/1");
    assert_eq!(included[1].url, "https://sub.a.com/1");

    let exclude = vec!["sub.a.com".to_string()];
    let after = filter_domains(
        vec![
            hit("A", "https://a.com/1"),
            hit("Sub", "https://sub.a.com/1"),
        ],
        &include,
        &exclude,
    );
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].url, "https://a.com/1");
}

#[test]
fn diversify_hosts_worked_example_count_3() {
    let results = vec![
        hit("A1", "https://a.com/x?utm_source=1"),
        hit("A2", "https://a.com/y"),
        hit("A3", "https://a.com/z"),
        hit("B1", "https://b.com/1"),
    ];
    let out = post_process_results(results, true, &[], &[], 2, 3);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].url, "https://a.com/x");
    assert_eq!(out[0].title, "A1");
    assert_eq!(out[0].site_name.as_deref(), Some("a.com"));
    assert_eq!(out[1].url, "https://a.com/y");
    assert_eq!(out[1].title, "A2");
    assert_eq!(out[2].url, "https://b.com/1");
    assert_eq!(out[2].title, "B1");
    assert_eq!(out[2].site_name.as_deref(), Some("b.com"));
}

#[test]
fn diversify_hosts_three_plus_two_keeps_two_and_two_at_count_4() {
    let results = vec![
        hit("A1", "https://a.com/1"),
        hit("A2", "https://a.com/2"),
        hit("A3", "https://a.com/3"),
        hit("B1", "https://b.com/1"),
        hit("B2", "https://b.com/2"),
    ];
    let out = diversify_hosts(results, 2, 4);
    assert_eq!(out.len(), 4);
    assert_eq!(out[0].title, "A1");
    assert_eq!(out[1].title, "A2");
    assert_eq!(out[2].title, "B1");
    assert_eq!(out[3].title, "B2");
}
