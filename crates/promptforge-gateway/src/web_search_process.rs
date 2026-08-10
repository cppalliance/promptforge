//! Pure post-process helpers for `web_search` results.
//!
//! Order of application is fixed by the gateway contract: sanitize, optional
//! tracking strip, site_name, include/exclude domain filters, then host
//! diversity capped at the requested count.

use std::collections::HashMap;

use crate::tools::SearchResult;

/// Max characters kept for a result title after sanitisation.
pub(crate) const TITLE_MAX_CHARS: usize = 512;
/// Max characters kept for a result description after sanitisation.
pub(crate) const DESCRIPTION_MAX_CHARS: usize = 4096;
/// Max characters kept for a result URL after tracking strip.
pub(crate) const URL_MAX_CHARS: usize = 2048;

/// Sanitize free text: drop most controls, collapse whitespace, trim, decode a
/// fixed entity set, then cap by Unicode scalar count.
#[must_use]
pub(crate) fn sanitize_text(text: &str, max_chars: usize) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\n' || c == '\t' {
            cleaned.push(' ');
        } else if !c.is_control() {
            cleaned.push(c);
        }
    }
    let collapsed = collapse_whitespace(&cleaned);
    let trimmed = collapsed.trim();
    let decoded = decode_entities(trimmed);
    truncate_chars(&decoded, max_chars)
}

/// Drop known tracking query parameters from `url`. Removes a trailing empty `?`.
///
/// Params removed when the name equals `fbclid`, `gclid`, `mc_cid`, `mc_eid`,
/// or starts with `utm_`.
#[must_use]
pub(crate) fn strip_tracking_params(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return truncate_chars(url, URL_MAX_CHARS);
    };
    let (query, fragment) = match query.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (query, None),
    };
    let mut kept: Vec<&str> = Vec::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let name = pair.split('=').next().unwrap_or(pair);
        if is_tracking_param(name) {
            continue;
        }
        kept.push(pair);
    }
    let mut out = String::from(base);
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    truncate_chars(&out, URL_MAX_CHARS)
}

/// Extract the hostname from `url` without a URL crate.
///
/// Handles optional scheme, `userinfo@`, and strips a trailing port. Returns
/// lowercase host text, or `None` when no host can be parsed.
#[must_use]
pub(crate) fn host_from_url(url: &str) -> Option<String> {
    let rest = match url.split_once("://") {
        Some((_, after)) => after,
        None => url.strip_prefix("//").unwrap_or(url),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    let host_port = match authority.rsplit_once('@') {
        Some((_, host_port)) => host_port,
        None => authority,
    };
    if host_port.is_empty() {
        return None;
    }
    // Bracketed IPv6: keep inside brackets; otherwise strip :port.
    let host = if let Some(inner) = host_port.strip_prefix('[') {
        let end = inner.find(']')?;
        &inner[..end]
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Hostname group / display name: lowercase host with one leading `www.` removed.
#[must_use]
pub(crate) fn site_name_from_host(host: &str) -> String {
    let lower = host.to_ascii_lowercase();
    lower
        .strip_prefix("www.")
        .unwrap_or(lower.as_str())
        .to_string()
}

/// Apply include then exclude domain filters.
///
/// Empty `include_domains` means no include filter. Empty `exclude_domains`
/// means no exclude filter. A hostname matches a listed domain when they are
/// equal (ASCII lowercase) or the hostname ends with `.` + domain.
#[must_use]
pub(crate) fn filter_domains(
    results: Vec<SearchResult>,
    include_domains: &[String],
    exclude_domains: &[String],
) -> Vec<SearchResult> {
    let include: Vec<String> = include_domains
        .iter()
        .map(|d| d.to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();
    let exclude: Vec<String> = exclude_domains
        .iter()
        .map(|d| d.to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();

    results
        .into_iter()
        .filter(|r| {
            let host = host_from_url(&r.url).unwrap_or_default();
            if !include.is_empty() && !include.iter().any(|d| host_matches_domain(&host, d)) {
                return false;
            }
            if !exclude.is_empty() && exclude.iter().any(|d| host_matches_domain(&host, d)) {
                return false;
            }
            true
        })
        .collect()
}

/// Keep results in order while each host group stays under `max_per_host`,
/// stopping once `count` results are kept.
///
/// Host groups use full hostname, lowercase, with one leading `www.` stripped.
#[must_use]
pub(crate) fn diversify_hosts(
    results: Vec<SearchResult>,
    max_per_host: u8,
    count: u8,
) -> Vec<SearchResult> {
    let mut kept = Vec::new();
    let mut per_host: HashMap<String, u8> = HashMap::new();
    let max_per_host = max_per_host.max(1);
    let count = count as usize;

    for result in results {
        if kept.len() >= count {
            break;
        }
        let group = host_from_url(&result.url)
            .map(|h| site_name_from_host(&h))
            .unwrap_or_default();
        let n = per_host.entry(group).or_insert(0);
        if *n >= max_per_host {
            continue;
        }
        *n += 1;
        kept.push(result);
    }
    kept
}

/// Run the full post-process pipeline on mapped Brave hits.
///
/// Steps: sanitize title/description, optional tracking strip + URL cap,
/// set `site_name`, include then exclude domain filters, diversify hosts.
#[must_use]
pub(crate) fn post_process_results(
    results: Vec<SearchResult>,
    strip_tracking: bool,
    include_domains: &[String],
    exclude_domains: &[String],
    max_per_host: u8,
    count: u8,
) -> Vec<SearchResult> {
    let prepared: Vec<SearchResult> = results
        .into_iter()
        .map(|r| {
            let title = sanitize_text(&r.title, TITLE_MAX_CHARS);
            let description = sanitize_text(&r.description, DESCRIPTION_MAX_CHARS);
            let url = if strip_tracking {
                strip_tracking_params(&r.url)
            } else {
                truncate_chars(&r.url, URL_MAX_CHARS)
            };
            let site_name = host_from_url(&url).map(|h| site_name_from_host(&h));
            SearchResult {
                title,
                url,
                description,
                age: r.age,
                site_name,
                extra_snippets: r.extra_snippets,
            }
        })
        .collect();
    let filtered = filter_domains(prepared, include_domains, exclude_domains);
    diversify_hosts(filtered, max_per_host, count)
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

fn decode_entities(text: &str) -> String {
    // Decode `&amp;` first so double-encoded forms like `&amp;lt;` resolve.
    let mut s = text.replace("&amp;", "&");
    s = s.replace("&lt;", "<");
    s = s.replace("&gt;", ">");
    s = s.replace("&quot;", "\"");
    s = s.replace("&#39;", "'");
    s = s.replace("&apos;", "'");
    s
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

fn is_tracking_param(name: &str) -> bool {
    matches!(name, "fbclid" | "gclid" | "mc_cid" | "mc_eid") || name.starts_with("utm_")
}

fn host_matches_domain(host: &str, domain: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == *domain || host.ends_with(&format!(".{domain}"))
}

#[cfg(test)]
mod tests {
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

        let url = format!("https://example.com/{}", "u".repeat(URL_MAX_CHARS));
        assert_eq!(strip_tracking_params(&url).chars().count(), URL_MAX_CHARS);
    }

    #[test]
    fn strip_tracking_removes_utm_and_fbclid() {
        let url = "https://a.com/x?utm_source=1&keep=yes&fbclid=abc&gclid=1&mc_cid=2&mc_eid=3&utm_medium=x";
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
}
