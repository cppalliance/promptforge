# Web Fetch

Hand a language model one tool and let it read the web. `promptforge-webfetch` fetches a URL, extracts the useful content, and returns it as markdown the model can cite - while enforcing an SSRF boundary that prevents the model from reaching your internal network no matter what URL it supplies. The common call is one argument (`url`). The security is layered and runs at DNS-resolution time on every hop, so it catches names that resolve inward, rebinding attacks, and redirect chains that point somewhere they should not.

By the end of this chapter you will know how to wire the tool into a promptforge pipeline, tune its policy for your deployment, and trust it with model-supplied URLs.

## Fetching a Page

Construct the tool and call it with a URL:

```rust
use promptforge_webfetch::WebFetch;
use promptforge_core::tools::Tool;

let tool = WebFetch::new();
let output = tool.call(serde_json::json!({ "url": "https://example.com/article" })).await?;
println!("{}", output.text());
```

The tool accepts one required argument (`url`) and two optional ones (`raw` and `max_chars`). It performs a GET, classifies the response by content type, and returns the text behind a provenance header:

```text
url: https://example.com/article
truncated: false
extraction: readability

# Article Title

The main content rendered as markdown...
```

The three header fields are a contract:

- **url** - the final URL after any redirects, so the model knows where its text came from
- **truncated** - whether the text was cut short by a size cap
- **extraction** - which of three processing paths produced the output: `readability` (article isolation), `raw-html` (whole-page render), or `plain` (non-HTML text returned verbatim)

## How Content Is Processed

The response's `Content-Type` header decides the route before the body is downloaded.

### HTML

Content types `text/html` and `application/xhtml+xml` are processed with a readability algorithm that isolates the main article and renders it to markdown. If the extracted article is shorter than 100 characters, the whole page is rendered instead, automatically. The `extraction:` header tells you which path fired.

### Structured Text

Content types `application/json`, `application/xml`, `text/xml`, and any `+json`/`+xml` suffix are returned verbatim as decoded text. No extraction, no transformation.

### Flat Text

All other `text/*` types are returned decoded. If the text exceeds the byte cap, the prefix is kept and `truncated: true` is set.

### Unsupported Types

PDF, images, audio, video, and `application/octet-stream` are refused with a message naming the content type so the model can try a different URL.

### Missing Content-Type

Refused. The tool does not sniff.

### Raw Mode

Use `raw` when article extraction would discard the content you want - for example a page that is mostly a data table:

```rust
let output = tool.call(serde_json::json!({
    "url": "https://example.com/pricing",
    "raw": true
})).await?;
```

This forces whole-page rendering and reports `extraction: raw-html`. Ignored for non-HTML responses.

Responses compressed with gzip or brotli are decompressed transparently.

## Size Limits and Truncation

Two caps govern how much data the tool accepts:

- **Byte cap** (`max_bytes`, default 8 MiB) - the largest decompressed response body. A declared `Content-Length` over this cap is refused before any bytes are read. A streaming body that crosses it mid-read is aborted.
- **Character cap** (`max_chars`, default 40,000) - the longest text returned to the model. Text is cut on a character boundary so multibyte characters are never split.

The two caps interact differently depending on the content type:

| Route | Body over byte cap | Text over char cap |
|---|---|---|
| HTML | Refused (incomplete HTML is invalid) | Truncated, flagged |
| Structured (JSON, XML) | Refused (truncated prefix is invalid) | Truncated, flagged |
| Flat text | Truncated at byte cap, flagged | Truncated at char cap, flagged |

A per-call `max_chars` argument lets the model request less text for one call:

```rust
let output = tool.call(serde_json::json!({
    "url": "https://example.com/long-page",
    "max_chars": 5000
})).await?;
```

The per-call value is clamped to the configured ceiling - a model cannot request more than the policy allows, only less.

## Security Policy

The default policy (`WebFetch::new()`) is safe for fetching the public internet: HTTPS only, ports 80 and 443, no bare IP-literal URLs, every non-globally-reachable address blocked.

### Customizing the Policy

Use the builder to adjust the defaults:

```rust
use std::time::Duration;
use promptforge_webfetch::{FetchConfig, WebFetch};

let policy = FetchConfig::builder()
    .allow_http(true)
    .allow_ports([80, 443, 8080])
    .max_bytes(16 * 1024 * 1024)
    .max_chars(100_000)
    .timeout(Duration::from_secs(60))
    .user_agent("my-service/1.0")
    .build()?;

let tool = WebFetch::try_with_config(policy)?;
```

Every setter returns `self` for chaining. Validation happens once at `.build()`, which returns `ConfigError` for any invalid field. The available knobs:

| Knob | Default | Ceiling | Notes |
|---|---|---|---|
| `allow_http` | `false` | - | Whether `http://` URLs are permitted |
| `allow_ports` | `[80, 443]` | - | Replaces the port allowlist |
| `allow_ip_literals` | `false` | - | Grants literal syntax only; address still classified |
| `deny_cidr` | (none) | - | Adds a blocked CIDR range (can call multiple times) |
| `allow_host_address` | (none) | - | Exact escape hatch (see below) |
| `max_redirects` | `5` | `20` | Zero refuses all redirects |
| `max_bytes` | 8 MiB | 64 MiB | Must be >= 1 |
| `max_chars` | `40,000` | `10,000,000` | Must be >= 1 |
| `connect_timeout` | 5s | 60s | Must be > 0 |
| `timeout` | 20s | 300s | Must be > 0 |
| `pool_idle_timeout` | 10s | 600s | Must be > 0 |
| `user_agent` | `"promptforge-webfetch/0.0"` | - | Must be a valid HTTP header value |

### Reaching an Internal Host

By default, every non-globally-reachable address is blocked. The only supported way to reach one is an exact host-plus-address pair:

```rust
use std::net::IpAddr;
use promptforge_webfetch::FetchConfig;

let addr: IpAddr = "10.0.5.42".parse()?;
let policy = FetchConfig::builder()
    .allow_http(true)
    .allow_ports([80, 443, 8080])
    .allow_host_address("wiki.internal.corp", addr)
    .build()?;
```

The escape hatch is deliberately narrow:

- Keyed on **both** host and address, so `evil.com` resolving to `10.0.5.42` does not inherit the exception
- Grants access to exactly one address, not a range
- The host is canonicalized (lowercased, trailing dot stripped) so case variants match

You can also block additional ranges for your deployment:

```rust
let policy = FetchConfig::builder()
    .deny_cidr("10.99.0.0/16")
    .deny_cidr("172.20.0.0/14")
    .build()?;
```

## The SSRF Boundary

The tool enforces four layers of defense, in order:

### URL Admission

Runs before any network access. Rejects bad schemes, embedded userinfo, non-allowed ports, and bare IP literals that map to blocked addresses. Catches obfuscated IPv4 encodings (`0177.0.0.1`, `2130706433`, `127.1`, `[::ffff:127.0.0.1]`).

### Guarded DNS Resolver

Runs at connect time on every hop. Resolves the host, filters the answers through the address policy, hands only the allowed addresses to the HTTP client. A host that resolves entirely to blocked addresses fails. A host with mixed public/private answers connects to the public one. No verdict is cached, so a DNS-rebinding answer is caught on the hop that returns it.

### Redirect Re-validation

Runs on every redirect hop. Re-runs the full URL policy on the redirect target. Refuses HTTPS-to-HTTP downgrades. Enforces the hop cap. The resolver re-classifies the redirect target's addresses at connect time.

### No Ambient Identity

The client carries no cookies, no `Authorization` header, no `Referer`, and disables ambient proxy (`HTTP_PROXY`/`HTTPS_PROXY`). A redirect cannot smuggle credentials to a cross-origin target.

### Blocked Address Table

The built-in table covers all IPv4 and IPv6 special-use space: loopback, RFC1918, CGNAT, link-local (including `169.254.169.254`), documentation, benchmarking, multicast, reserved, and IPv6 equivalents including IPv4-mapped, NAT64, unique-local, and deprecated site-local. IPv4-embedded IPv6 addresses (`::ffff:127.0.0.1`, `::10.0.0.1`) are normalized to their embedded IPv4 value and reclassified.

## Error Behavior

Errors split into two categories based on whether a retry makes sense.

### Soft Outcomes

Returned as tool text the model can act on:

- HTTP error status (404, 500, etc.)
- Timeouts
- DNS failures
- Unsupported or absent content type
- Body too large
- Body read failure mid-stream
- Redirect refused
- Blocked scheme (`http` when only `https` is allowed)

### Hard Errors

The URL itself is invalid and no retry will help:

- Unparseable URL
- URL contains userinfo
- Port not on the allowlist
- IP literal not allowed
- Address is blocked / no allowed address for the host

When a blocked address is reported to the model, only the host name appears in the message - never the resolved address or the blocking range. Query strings and fragments are redacted from all diagnostic URLs so a `?token=secret` never reaches logs or model output.
