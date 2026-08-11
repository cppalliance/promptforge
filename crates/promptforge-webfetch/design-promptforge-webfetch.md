# Design: promptforge-webfetch public API and SSRF hardening

`promptforge-webfetch` is a small library that implements one `web_fetch` `Tool`
consumed by `promptforge-cli`, `promptforge-dev`, and `promptforge-mcp-server`.
It is security-critical: a model supplies the URL, so the crate is the SSRF
boundary between an untrusted argument and the network. This document specifies
the smallest coherent public API, the internal hardening that closes every
high-severity finding, the SSRF invariants the design must guarantee, the
old-to-new migration for the three downstream call sites, the required tests,
and a disposition for every API-related finding.

The crate is unpublished (`publish = false`) and every downstream site uses only
`WebFetch::new()`, so the entire low-level surface can be removed now without a
breaking release and without touching any of the three consumers.

## Current public API (grounded with `cargo public-api`)

Six implementation modules are `pub`, and most of their internals are re-exported
at the crate root. The surface, verbatim in shape:

- Modules: `pub mod address, config, error, redirect, resolver, url_policy`.
- `pub const BLOCKED_CIDRS: &[&str]`
- `pub fn addr_allowed(IpAddr, &FetchConfig) -> bool`
- `pub fn addr_allowed_for_host(&str, IpAddr, &FetchConfig) -> bool`
- `pub struct FetchConfig` - `#[non_exhaustive]`, `Debug + Clone + Default`, but
  all twelve policy fields are `pub`: `allow_http`, `allow_ports`,
  `allow_ip_literals`, `deny_extra: Vec<IpNet>`, `allow_exact: Vec<(String, IpAddr)>`,
  `max_redirects`, `max_bytes`, `max_chars`, `connect_timeout`, `timeout`,
  `pool_idle_timeout`, `user_agent`.
- `pub enum FetchError` - `#[non_exhaustive]`, 15 variants with public fields,
  `model_facing()`, `is_recoverable()`, and `impl From<FetchError> for ToolError`.
- `pub fn check_redirect(&[url::Url], &url::Url, &FetchConfig) -> Result<(), FetchError>`
- `pub fn redirect_policy(FetchConfig) -> reqwest::redirect::Policy`
- `pub struct GuardedResolver<L = SystemLookup>`, `pub trait Lookup`, `pub struct SystemLookup`
- `pub fn check_url(&str, &FetchConfig) -> Result<url::Url, FetchError>`
- `pub struct WebFetch` with `new()`, `with_config(FetchConfig)` (documented to
  panic), `impl Default`, and `impl Tool`.

This leaks `ipnet::IpNet`, `url::Url`, `reqwest::redirect::Policy`,
`reqwest::dns::Resolve`, a boxed-future DNS seam, a raw CIDR string table, and a
large concrete error enum into the crate's semver surface. None of it is used by
any consumer.

## Proposed public API (the whole surface)

The natural product surface is `WebFetch` plus one validated configuration entry
point and one opaque configuration error. Everything else becomes crate-private.

```rust
// src/lib.rs - facade only: crate docs, module declarations, re-exports.
mod address;
mod config;
mod error;
mod redirect;
mod resolver;
mod response;   // body reads + decode + extraction (split out of lib.rs)
mod tool;       // WebFetch and its Tool impl (split out of lib.rs)
mod url_policy;

pub use crate::config::{ConfigError, FetchConfig, FetchConfigBuilder};
pub use crate::tool::WebFetch;
```

### `WebFetch`

```rust
#[derive(Debug, Clone)]
pub struct WebFetch { /* private: reqwest::Client, Arc<FetchConfig> */ }

impl WebFetch {
    /// Constructs a `WebFetch` with the built-in default policy. Infallible:
    /// the default policy is a compile-time-valid constant.
    #[must_use]
    pub fn new() -> WebFetch;

    /// Constructs a `WebFetch` with a validated custom policy.
    ///
    /// # Errors
    /// Returns [`ConfigError`] if the HTTP client cannot be built for `config`
    /// (for example a TLS backend that fails to initialize). The policy itself
    /// is already validated by [`FetchConfig`] construction, so no policy field
    /// can trigger a failure here.
    pub fn try_with_config(config: FetchConfig) -> Result<WebFetch, ConfigError>;
}

impl Default for WebFetch { fn default() -> WebFetch { WebFetch::new() } }

#[async_trait::async_trait]
impl Tool for WebFetch { /* id, wire_name, description, parameters_schema, call */ }
```

`with_config` (infallible, panicking) is removed in favor of `try_with_config`.
`new()` and `Default` are retained unchanged, so the three consumers are untouched.

### `FetchConfig` (opaque, validated) and its builder

```rust
#[derive(Debug, Clone)]
pub struct FetchConfig { /* all fields private; holds validated newtypes */ }

impl FetchConfig {
    /// Starts a builder seeded with the default policy.
    #[must_use]
    pub fn builder() -> FetchConfigBuilder;
}

impl Default for FetchConfig { /* the built-in safe policy, HTTPS-only */ }

#[derive(Debug, Clone)]
pub struct FetchConfigBuilder { /* private */ }

impl FetchConfigBuilder {
    #[must_use] pub fn allow_http(self, yes: bool) -> Self;
    #[must_use] pub fn allow_ports(self, ports: impl IntoIterator<Item = u16>) -> Self;
    #[must_use] pub fn allow_ip_literals(self, yes: bool) -> Self;
    /// Adds a denied CIDR range, parsed from text so `ipnet` never appears in
    /// the public API. Invalid text is reported by `build`.
    #[must_use] pub fn deny_cidr(self, cidr: impl Into<String>) -> Self;
    /// Adds an exact host+address escape hatch. Host is canonicalized and
    /// validated by `build`.
    #[must_use] pub fn allow_host_address(self, host: impl Into<String>, addr: IpAddr) -> Self;
    #[must_use] pub fn max_redirects(self, n: usize) -> Self;
    #[must_use] pub fn max_bytes(self, n: usize) -> Self;
    #[must_use] pub fn max_chars(self, n: usize) -> Self;
    #[must_use] pub fn connect_timeout(self, d: Duration) -> Self;
    #[must_use] pub fn timeout(self, d: Duration) -> Self;
    #[must_use] pub fn pool_idle_timeout(self, d: Duration) -> Self;
    #[must_use] pub fn user_agent(self, ua: impl Into<String>) -> Self;

    /// Validates every field and produces an immutable [`FetchConfig`].
    ///
    /// # Errors
    /// Returns [`ConfigError`] for a header-invalid user agent, a zero or
    /// over-ceiling limit, a malformed CIDR, or a malformed exact-host entry.
    pub fn build(self) -> Result<FetchConfig, ConfigError>;
}

/// Opaque configuration error. Its representation is private and free to change.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct ConfigError(/* private ConfigErrorRepr */);
```

Internally the config stores validated newtypes (all crate-private): a
header-safe `UserAgent`, `MaxBytes(NonZeroUsize)` and `MaxChars(NonZeroUsize)`
each clamped to a hard operational ceiling, a bounded `MaxRedirects`, positive
timeouts, a `Vec<IpNet>` of parsed denied ranges, and a `Vec<HostAddressException>`
of canonicalized host+address pairs.

## Removals and privatizations

| Item | Current | New |
|---|---|---|
| `mod address`, `mod redirect`, `mod resolver`, `mod url_policy`, `mod error` | `pub mod` | `mod` (crate-private) |
| `BLOCKED_CIDRS`, `addr_allowed`, `addr_allowed_for_host` | `pub` + root re-export | `pub(crate)`, re-exports removed |
| `check_redirect`, `redirect_policy` | `pub` + re-export | `pub(crate)`, re-exports removed |
| `GuardedResolver`, `Lookup`, `SystemLookup` | `pub` + re-export | `pub(crate)` (test seam retained internally) |
| `check_url` | `pub` + re-export | `pub(crate)` |
| `FetchError`, its variants, `model_facing`, `is_recoverable`, `From<FetchError> for ToolError` | `pub` | `pub(crate)`; `ToolError` mapping done privately at the `Tool::call` boundary, no public `From` |
| `FetchConfig` public fields | 12 `pub` fields | private; builder + `Default` only |
| `WebFetch::with_config` (panicking) | `pub` | removed, replaced by `try_with_config` |

This removes `ipnet`, `url`, `reqwest::redirect::Policy`, and `reqwest::dns::Resolve`
from the public API entirely.

## SSRF invariants

The design must make these true by construction, not by the caller remembering
a rule. Each is stated so a test can falsify it.

1. **Connect only to a fully classified address.** Every outbound connection, on
   the first hop and after every redirect, is made only to an `IpAddr` that
   passed `addr_allowed_for_host(host, ip, config)`. reqwest is handed a filtered
   address vector by `GuardedResolver`; it is never handed a blocked fallback.
2. **IP literals are classified, never exempt.** A literal-host URL does not go
   through the DNS resolver (hyper connects to it directly), so the literal is
   classified inside `check_url`: parse the literal to `IpAddr` and require
   `addr_allowed_for_host`. `allow_ip_literals` permits literal *syntax* only; it
   never grants a blocked address class. A blocked literal is reachable solely
   through the exact host+address escape hatch. This is the primary fix.
3. **No ambient proxy.** The client is built with `.no_proxy()`. An ambient
   `HTTP_PROXY`/`HTTPS_PROXY` or system proxy would resolve and connect to the
   target beyond `GuardedResolver`, defeating the boundary; it is disabled.
4. **Full policy re-runs on every redirect hop.** `check_redirect` re-runs the
   complete URL policy (scheme, userinfo, port, literal classification) and
   refuses an HTTPS-to-HTTP downgrade, and the guarded resolver re-classifies the
   redirect target's addresses at connect time. No verdict is cached.
5. **Every non-global address class is blocked.** The address policy denies every
   destination that is not globally reachable. The built-in table is completed
   with the currently missing IPv6 classes - `::/96` (IPv4-compatible),
   `fec0::/10` (deprecated site-local), and `3fff::/20` (RFC 9637 documentation) -
   and IPv4-embedded IPv6 forms are normalized to their embedded IPv4 value and
   reclassified so `::127.0.0.1`, `::10.0.0.1`, and `::169.254.169.254` are denied.
6. **No ambient identity on any hop.** No cookie store, no default `Authorization`,
   and `.referer(false)` so a query-bearing source URL cannot leak its query to a
   cross-origin redirect target via `Referer`.
7. **Limits cannot be disabled.** Body, character, and redirect caps and timeouts
   are bounded newtypes validated at config construction; none can be zero or
   unbounded, and a model-supplied per-call `max_chars` is `min(requested, ceiling)`.

The classifier is refactored into one private function returning a structured
reason (an internal `AddressClass` / block-reason enum), giving tests a single
exhaustive seam and making a future registry update a private change rather than
a public-API mutation.

## Internal hardening summary

- **`url_policy`**: classify literal hosts against the address policy; `check_url`
  keeps returning a validated `url::Url` internally (module is now private, so a
  post-return mutation concern is confined to the crate).
- **`resolver`**: add `.no_proxy()` at client build; store `Arc<FetchConfig>` and
  clone the `Arc` into each resolving future; preserve the `std::io::Error` cause
  behind a `#[source]` field in the private DNS error.
- **`redirect`**: disable automatic `Referer`; correct the `previous.len()`
  comment to "ordinal of the prospective redirect."
- **`address`**: complete the non-global table, normalize IPv4-embedded IPv6,
  expose one private structured classifier.
- **`error`**: private representation with `#[source]` on URL-parse, DNS, and
  body-read causes; replace the `is_recoverable` bool with one exhaustive
  `classify() -> Disposition` (`SoftOutput` vs `Hard(ToolErrorKind)`) with no
  wildcard, mapped to `ToolError` at the `Tool::call` boundary; route the
  flat-text body-read failure through the same soft mapping as the other routes.
- **`config`**: validated newtypes and a fallible builder; a hard `max_chars`
  ceiling separate from the per-call requested value.
- **Manifest**: set `version = "0.0.0"` (crate is `publish = false`); drop the
  unused reqwest `stream` and `charset` features.

The async CPU-bound extraction and peak-memory concerns (readability/htmd on the
runtime thread; intermediate `String` amplification) are real but are not
API-shaped; they are noted as follow-up internal hardening and are not required
to land the API change without regressing the build.

## Migration map: the three downstream call sites

| Call site | Current use | New use | Forced signature change? |
|---|---|---|---|
| `promptforge-cli/src/tools.rs` | `use promptforge_webfetch::WebFetch; Arc::new(WebFetch::new())` | identical | No |
| `promptforge-dev/src/tools.rs` | `use promptforge_webfetch::WebFetch; Arc::new(WebFetch::new())` | identical | No |
| `promptforge-mcp-server/src/server/bind.rs` | `use promptforge_webfetch::WebFetch; Arc::new(WebFetch::new())` | identical | No |

**Forced downstream signature changes: 0.** All three consumers use only
`WebFetch::new()`, which is retained with an unchanged signature. The new
`try_with_config`/builder surface is purely additive. The crate's own unit and
integration tests that call `WebFetch::with_config(FetchConfig { .. })` are
internal and are migrated to `WebFetch::try_with_config(FetchConfig::builder()...build()?)`
in the same sweep; they are not downstream call sites.

## Required tests

SSRF bypass attempts (the security core; each maps to an invariant):

- IP-literal classification with `allow_ip_literals = true`: a public literal is
  admitted, while loopback (`127.0.0.1`), link-local metadata
  (`169.254.169.254`), RFC1918, mapped-IPv4 (`::ffff:127.0.0.1`), NAT64
  (`64:ff9b::7f00:1`), IPv4-compatible (`::127.0.0.1`, `::10.0.0.1`,
  `::169.254.169.254`), multicast, and reserved literals are all refused.
- Completed non-global table: `fec0::1`, `3fff::1`, and the boundary addresses of
  every blocked range are denied; representative global unicast is allowed.
- Ambient-proxy: with a controlled `HTTP_PROXY`/`HTTPS_PROXY` set, no request is
  delegated to the proxy (the internal target is never contacted).
- Redirect re-validation: an injected lookup where the initial host is allowed but
  the redirect host resolves only to a blocked address - the redirected target is
  never contacted; redirect targets with userinfo, a blocked port, a disallowed
  scheme, and encoded IP literals are refused.
- Redirect referer/credentials: a query-bearing source URL redirecting to a
  distinct allowed host - the target receives no `Referer`, `Authorization`, or
  `Cookie`.
- Redirect cap boundaries: `max_redirects = 0` refuses the first redirect;
  exactly the cap is followed; cap + 1 is refused.

Config validation:

- `build()` rejects a newline/CR user agent, zero and over-ceiling `max_bytes`/
  `max_chars`/`max_redirects`, a malformed CIDR, and a malformed/duplicate exact
  host, each with a `ConfigError`.
- Per-call `max_chars` above the configured ceiling is clamped to the ceiling.

Behavior/regression (retain existing coverage, plus):

- Error source chain: URL-parse, DNS, and mid-stream body-read failures keep a
  reachable `Error::source`.
- Classification: an exhaustive table asserting every internal variant's
  `Disposition` and `ToolErrorKind`, including `BlockedAddress` and
  `NoAllowedAddress`.
- Flat-text (`text/plain`) mid-stream failure returns soft untrusted output, like
  the HTML and structured routes.

## Disposition of every API-related finding

| Finding | Disposition |
|---|---|
| PF-WF-LIB-001 (critical, IP-literal SSRF bypass) | Fixed. Invariant 2: classify literals in `check_url`; `allow_ip_literals` grants syntax only. |
| PF-WF-LIB-002 (with_config panic on bad user agent) | Fixed. `with_config` removed; `try_with_config` fallible; user agent validated in the builder. |
| PF-WF-LIB-003 (facade over-exposes internals/deps) | Fixed. All six modules private; only `WebFetch`, `FetchConfig`, `FetchConfigBuilder`, `ConfigError` public. |
| PF-WF-LIB-004 (public config fields, invalid states) | Fixed. Private validated newtypes + fallible builder. |
| PF-WF-LIB-005 (CPU-bound extraction on runtime) | Deferred. Non-API internal hardening (spawn_blocking + processing deadline); noted, not in this sweep. |
| PF-WF-LIB-006 (flat-text body-read hard vs soft mismatch) | Fixed. Route through the same soft mapping. |
| PF-WF-LIB-007 (post-processing memory amplification) | Deferred. Non-API internal hardening; noted. |
| PF-WF-LIB-008 (per-call max_chars has no ceiling) | Fixed. Hard ceiling in config; per-call = `min(requested, ceiling)`; schema maximum. |
| PF-WF-LIB-009 (lib.rs 1,769 lines) | Fixed. Split into `tool`, `response`; `lib.rs` becomes a facade. |
| PF-WF-LIB-010 (no doctests on public items) | Fixed. Doctests on `WebFetch::new`, `try_with_config`, and the builder. |
| PF-WF-ADDR-001 (`::/96` IPv4-compatible allowed) | Fixed. Block `::/96` / normalize embedded IPv4 (invariant 5). |
| PF-WF-ADDR-002 (`fec0::/10` site-local allowed) | Fixed. Added to table. |
| PF-WF-ADDR-003 (`3fff::/20` documentation allowed) | Fixed. Added; classifier keyed on "not globally reachable" with recorded snapshot. |
| PF-WF-ADDR-004 (classifier test gaps) | Fixed. Canonical classification matrix test. |
| PF-WF-ADDR-005 (`BLOCKED_CIDRS` is public) | Fixed. `pub(crate)`; behavior exposed only via `WebFetch`. |
| PF-WF-ADDR-006 (no doctests on address fns) | Resolved by removal from the public API. |
| WFC-001 (public fields, no validating ctor) | Fixed. Private fields + fallible builder. |
| WFC-002 (unvalidated user agent panics) | Fixed. Header-safe `UserAgent` newtype; fallible build. |
| WFC-003 (unbounded limits) | Fixed. Bounded newtypes with operational ceilings. |
| WFC-004 (zero-capable limits/timeouts) | Fixed. `NonZero` limits and positive timeouts; shared char-limit validator. |
| WFC-005 (stringly `allow_exact`) | Fixed. Canonicalized `HostAddressException`; dedup; reject malformed. |
| WFC-006 (`IpNet`/tuple/repr leak) | Fixed. Private storage; builder takes CIDR text; crate-owned names. |
| WFC-007 (no config doctest) | Fixed. Builder doctest. |
| WFC-008 (no `PartialEq`/`Eq`, no default test) | Fixed. Derive `PartialEq`/`Eq` on the validated type; default-policy test. |
| PFW-ERR-001 (causes discarded) | Fixed. `#[source]` on URL/DNS/body-read causes in the private repr. |
| PFW-ERR-002 (large public enum + public `From`) | Fixed. `FetchError` private; `ToolError` mapping private at the boundary. |
| PFW-ERR-003 (misclassification: scheme/redirect as Transport) | Fixed. Exhaustive `classify()` decides disposition and kind independently. |
| PFW-ERR-004 (`matches!` silent default) | Fixed. Exhaustive `match`, no wildcard, adjacent to the boundary mapping. |
| PFW-ERR-005 (untested cross-crate contract) | Fixed. Table-driven disposition/kind + source-chain tests. |
| PFWEB-REDIRECT-001 (Referer leak) | Fixed. `.referer(false)` (invariant 6). |
| PFWEB-REDIRECT-002 (public redirect helpers) | Fixed. `pub(crate)`; dependency types no longer in the public API. |
| PFWEB-REDIRECT-003 (cap comment + boundary tests) | Fixed. Comment corrected; boundary tests added. |
| PFWEB-REDIRECT-004 (redirect SSRF test gaps) | Fixed. Table-driven + injected-lookup redirect tests. |
| PFWEB-REDIRECT-005 (no doctests) | Resolved by removal from the public API. |
| WF-RES-001 (ambient proxy bypass) | Fixed. `.no_proxy()` (invariant 3). |
| WF-RES-002 (resolver seam exported) | Fixed. `resolver` private; types `pub(crate)`; generic kept as internal test seam. |
| WF-RES-003 (DNS cause discarded) | Fixed. `#[source]` on the private DNS error. |
| WF-RES-004 (full `FetchConfig` clone per lookup) | Fixed. `Arc<FetchConfig>` cloned per future. |
| WF-RES-005 (no doctests/Send-Sync asserts) | Resolved by removal; internal `Send + Sync` compile assertions retained. |
| URLP-001 (literal bypass) | Fixed. Same as PF-WF-LIB-001. |
| URLP-002 (mutable admitted URL leaks) | Resolved. `check_url` is now crate-private; admission stays on the complete path. |
| URLP-003 (no doctest) | Resolved by removal from the public API. |
| URLP-004 (parse cause discarded) | Fixed. `#[source]` on the private URL-parse error. |
| PF-WEBFETCH-MANIFEST-001 (version sentinel) | Fixed. `version = "0.0.0"`. |
| PF-WEBFETCH-MANIFEST-002 (unused `stream` feature) | Fixed. Removed. |
| PF-WEBFETCH-MANIFEST-003 (unused `charset` feature) | Fixed. Removed. |
| PF-WEBFETCH-MANIFEST-004 (missing readme/keywords) | Optional. Add a README and metadata; not required for the API change. |

## Self-check

Every API-related finding above is dispositioned (fixed, resolved by removal, or
explicitly deferred as non-API internal hardening). The design is implementable
in one sweep: the six modules become private, the config gains a validated
builder, `with_config` is replaced by `try_with_config`, and the
address/resolver/redirect/error hardening is confined to now-private modules.
Because `WebFetch::new()` and `Default` are unchanged, the workspace continues to
build clean with zero edits to the three downstream crates.

*2026-08-10 21:04 - Opus 4.8 (Cursor agent)*
