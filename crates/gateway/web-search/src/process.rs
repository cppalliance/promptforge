//! Pure post-process helpers for `web_search` results.
//!
//! Order of application is fixed by the gateway contract: sanitize, optional
//! tracking strip, site_name, include/exclude domain filters, then host
//! diversity capped at the requested count.

use std::collections::HashMap;

use crate::service::SearchResult;

/// Max characters kept for a result title after sanitisation.
const TITLE_MAX_CHARS: usize = 512;
/// Max characters kept for a result description after sanitisation.
const DESCRIPTION_MAX_CHARS: usize = 4096;
/// Max characters kept for a result URL after tracking strip.
const URL_MAX_CHARS: usize = 2048;
/// Max characters kept for a single extra snippet after sanitisation (WSP-001).
const SNIPPET_MAX_CHARS: usize = 1024;
/// Max number of extra snippets kept per result (WSP-001).
const MAX_EXTRA_SNIPPETS: usize = 8;
/// Max characters kept for a result `age` after sanitisation (WSP-001).
const AGE_MAX_CHARS: usize = 64;

/// Sanitizes free text: drops most controls, collapses whitespace, trims, decodes a
/// fixed entity set, then caps by Unicode scalar count.
#[must_use]
fn sanitize_text(text: &str, max_chars: usize) -> String {
    // Bound the work up front (WSP-002): entity decoding and the final cap can
    // only shrink text, so processing more than a small multiple of `max_chars`
    // scalars is wasted effort on a hostile oversized input.
    let bound = max_chars.saturating_mul(8).max(max_chars);
    let text: String = text.chars().take(bound).collect();
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

/// Drops known tracking query parameters from `url`. Removes a trailing empty `?`.
///
/// Params removed when the name equals `fbclid`, `gclid`, `mc_cid`, `mc_eid`,
/// or starts with `utm_`. Does not truncate: an over-length URL is dropped by
/// the pipeline (WSP-004), never cut mid-component into a broken link.
#[must_use]
fn strip_tracking_params(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
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
    out
}

/// Extracts the hostname from `url` without a URL crate.
///
/// Handles optional scheme, `userinfo@`, and strips a trailing port. Returns
/// lowercase host text, or `None` when no host can be parsed.
#[must_use]
fn host_from_url(url: &str) -> Option<String> {
    // Prefer a standards-compliant parse for well-formed URLs (TOOLS-014), then
    // fall back to the lenient extractor for scheme-less or non-URL inputs
    // (used by domain-filter canonicalization).
    if let Ok(parsed) = url::Url::parse(url)
        && let Some(host) = parsed.host_str()
        && !host.is_empty()
    {
        return Some(host.to_ascii_lowercase());
    }
    host_from_url_lenient(url)
}

/// Whether `url` is a standards-parsed, navigable `http`/`https` URL with a
/// non-empty host (WSP-003).
///
/// Result URLs are handed to a consumer as clickable links, so a value that is
/// not a real URL (for example the lenient `not-a-url` case accepted for
/// domain-filter canonicalization) must never be emitted as a result.
#[must_use]
fn is_navigable_url(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(parsed) => {
            matches!(parsed.scheme(), "http" | "https")
                && parsed.host_str().is_some_and(|host| !host.is_empty())
        }
        Err(_) => false,
    }
}

fn host_from_url_lenient(url: &str) -> Option<String> {
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
fn site_name_from_host(host: &str) -> String {
    let lower = host.to_ascii_lowercase();
    lower
        .strip_prefix("www.")
        .unwrap_or(lower.as_str())
        .to_string()
}

/// Applies include then exclude domain filters.
///
/// Empty `include_domains` means no include filter. Empty `exclude_domains`
/// means no exclude filter. A hostname matches a listed domain when they are
/// equal (ASCII lowercase) or the hostname ends with `.` + domain.
#[must_use]
fn filter_domains(
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

/// Keeps results in order while each host group stays under `max_per_host`,
/// stopping once `count` results are kept.
///
/// Host groups use full hostname, lowercase, with one leading `www.` stripped.
#[must_use]
fn diversify_hosts(results: Vec<SearchResult>, max_per_host: u8, count: u8) -> Vec<SearchResult> {
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

/// Runs the full post-process pipeline on mapped Brave hits.
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
        .filter_map(|r| {
            let title = sanitize_text(&r.title, TITLE_MAX_CHARS);
            let description = sanitize_text(&r.description, DESCRIPTION_MAX_CHARS);
            let url = if strip_tracking {
                strip_tracking_params(&r.url)
            } else {
                r.url
            };
            // An over-length URL is dropped whole, never char-truncated into a
            // broken, non-navigable link (WSP-004).
            if url.chars().count() > URL_MAX_CHARS {
                return None;
            }
            // Only emit results whose URL is a real, navigable http(s) link
            // (WSP-003): a value that only survives the lenient authority parse
            // (for example `not-a-url`) is not a usable result URL.
            if !is_navigable_url(&url) {
                return None;
            }
            let site_name = host_from_url(&url).map(|h| site_name_from_host(&h));
            // Cap snippet count and per-snippet length, sanitising each (WSP-001).
            let extra_snippets = r
                .extra_snippets
                .into_iter()
                .take(MAX_EXTRA_SNIPPETS)
                .map(|snippet| sanitize_text(&snippet, SNIPPET_MAX_CHARS))
                .filter(|snippet| !snippet.is_empty())
                .collect();
            // `age` is provider-controlled free text; sanitize and cap it like
            // every other retained string (WSP-001).
            let age = r
                .age
                .map(|age| sanitize_text(&age, AGE_MAX_CHARS))
                .filter(|age| !age.is_empty());
            Some(SearchResult {
                title,
                url,
                description,
                age,
                site_name,
                extra_snippets,
            })
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
#[path = "process-tests.rs"]
mod tests;
