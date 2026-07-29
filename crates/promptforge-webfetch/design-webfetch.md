# Building a guarded `web_fetch`: a URL-fetching tool a model can point anywhere without reaching inward

## Executive summary

This crate is a standalone Rust package that gives a language model one tool, `web_fetch`: hand it a URL, get back the page as readable markdown. The design problem is not fetching, which is easy; it is fetching *safely* when the URL comes from a model that may have just read an attacker-controlled page. A naive fetch is a server-side request forgery (SSRF) primitive: the model can be steered to `http://169.254.169.254/` or `http://10.0.0.5/` and made to read the deployment's own cloud-metadata service or internal hosts. Everything of interest here exists to close that gap while keeping the common call trivial.

The tool's surface is deliberately one required input and two optional ones: `url`, plus `raw` (render the whole page, don't extract the article) and `max_chars` (cap this call's text). It returns the page text behind a three-line provenance header the model reads: the final URL after redirects, whether the text was truncated, and how the text was produced. The safety comes from a layered policy: a URL-admission check before any packet leaves, an address check that runs *at DNS-resolution time on every hop* (so a name that resolves inward, a rebinding answer, or a redirect to an internal host is all caught at connect time, not at parse time), decompressed-byte size caps, content-type routing that refuses binaries and never sniffs an absent type, and a client that carries no cookie or credential on any hop. The single most important decision is that the address check filters resolved addresses rather than inspecting the URL string, because that is the only place a name that lies about where it points can be caught.

To act on this crate: construct the tool with defaults for a public-internet-only fetcher, or override the policy to reach a specific internal host through the one narrow escape hatch (`allow_exact`, keyed on host *and* exact address). The defaults refuse plain HTTP, allow only ports 80 and 443, reject bare IP-literal URLs, block a large table of private and special-use ranges, cap the body at 8 MiB and the text at 40,000 characters, and time out. If you remove this crate, every other crate still compiles: it is the only place HTTP, HTML extraction, and the SSRF policy live.

## Key design choices

1. **`web_fetch` is one tool with one required input and two optional knobs.** The call is `{ url }` in the common case; `raw` and `max_chars` exist for the pages where the default behavior discards what the model needs or returns more than it wants. Every knob has a default, so the whole surface past `url` is optional. The alternative, a wide always-on parameter set, was rejected because it taxes every simple call to serve the rare one.

2. **The return is text behind a provenance header the model reads, not a structured object.** Three labeled lines then a blank line then the content. The header names the final URL after redirects, a truncation flag, and the extraction mode. This is chosen over returning a bare string because the model must be able to cite *where the bytes actually came from* (which may differ from where it aimed, after redirects) and must know whether it is holding a complete document. It is chosen over a JSON envelope because the consumer is a model reading prose, and a header it reads inline costs nothing to parse.

3. **The address policy is enforced at DNS-resolution time, not on the URL string.** A URL-string check alone is defeated by a hostname that resolves to an internal address, and by DNS rebinding between the check and the connection. Running the check inside the resolver, on the actual addresses a host resolves to, is the only place these are caught. This is the load-bearing decision of the whole crate; reversing it would reopen the SSRF hole that everything else is built to close.

4. **The guarded resolver filters answers; it does not reject on the first blocked one.** A host that returns one public and one private address is still reachable at its public address, while a host that returns only blocked addresses fails. Reject-on-first-blocked would let a multi-answer host through on a later answer or fail a legitimate host on an unlucky ordering; filtering is the correct discipline and a named review check enforces it.

5. **`allow_exact` is keyed on host *and* exact address, never a range.** The only supported way to reach an internal host is an explicit `(host, ip)` pair. Keying on both means a rebinding answer of `evil.com -> 127.0.0.1` cannot inherit the exception granted to `localhost -> 127.0.0.1`: the host must match too. A host-agnostic allowlist would be a rebinding bypass; the pair is the whole point.

6. **Redirects re-run the full policy on every hop, and an `https -> http` downgrade is refused.** The resolver already blocks an internal *address* on every hop because it runs at connect time, but it cannot see URL-level facts, so a per-hop redirect policy re-checks scheme, userinfo, port, and IP literal on each target and caps the hop count. A redirect is an attacker's second bite: a public URL that 302s to `http://127.0.0.1/` must die at the hop, and the final URL is what the tool reports as provenance.

7. **Structured formats are all-or-nothing on the size cap; flat text truncates and flags.** A truncated prefix of JSON or XML (or HTML) is invalid or misleading, so an oversized structured body is refused outright. A prefix of flat `text/plain` is a legitimate partial result, so an oversized flat body is cut at the cap and flagged `truncated: true`. This split is a content-type decision, so the truncate-versus-refuse behavior lands together with content-type routing rather than as a size-only concern: the size cap cannot know which discipline to apply until the type is classified.

8. **The size cap counts decompressed bytes and prechecks `Content-Length`.** An honest `Content-Length` over the cap is refused before the body is read; a compressed body reports no usable length, so a streamed counter over the *decompressed* stream aborts mid-read the moment the running total exceeds the cap. Counting wire bytes would let a gzip bomb through: a few hundred compressed bytes expanding to megabytes. The decompressed count is what makes the same cap catch both an honest oversize and a bomb.

9. **An absent `Content-Type` is refused, not sniffed; binaries are refused with an actionable message.** HTML, XHTML, JSON, XML, and other `text/*` are accepted; PDF, octet-stream, images, audio, video, and archives are refused with a message naming the type and suggesting a next move so the model does not simply retry the same URL. Refusing to sniff an absent type avoids treating attacker-chosen bytes as a format the tool never verified. Content-type sniffing is a classic content-confusion vector; declining it is the safe default.

10. **The client carries no ambient identity on any hop.** No cookie store, no `Authorization` or credential header, on the first request and after every redirect. A fetcher pointed by a model must not replay the deployment's cookies or tokens to an arbitrary URL, and a redirect must not smuggle them onward. This is a cross-cutting property enforced at client construction and checked by tests that read the headers the server actually received, including after a redirect.

11. **URL admission runs before any network access and normalizes the host.** Scheme, userinfo, port, and IP-literal checks happen on the parsed URL at the top of the call, so a bad URL costs no request. IP literals in every obfuscated encoding (`0177.0.0.1`, `2130706433`, `127.1`, `[::1]`) normalize to a host the literal check catches. The URL's fragment is dropped (it never travels to the server) and its query is preserved untouched. Doing this pre-flight keeps the expensive network path clean and makes the cheapest refusals the fastest.

12. **HTML extraction defaults to article isolation, falls back automatically, and yields to a manual `raw` override.** The default isolates the main article and renders it to markdown; if that comes back near-empty, the whole page is rendered instead, automatically. `raw` is the manual override for the page that *would* pass the automatic check - a little prose above a large table - where article extraction would keep the prose and discard the table. The extraction mode is reported in the header so the model knows which of the three paths produced its text.

13. **The blocked-range table is data the crate owns, spanning IPv4 and IPv6 special-use space.** Loopback, RFC1918 private, CGNAT, link-local (including the `169.254.169.254` cloud-metadata address), documentation, benchmarking, multicast, reserved, and their IPv6 equivalents including IPv4-mapped, NAT64, unique-local, and 6to4. A deployment adds its own internal CIDRs through `deny_extra`. The list must be carried directly rather than referenced, and its coverage of the v6 disguises (IPv4-mapped loopback, NAT64-embedded addresses) is what stops a v4 block from being trivially bypassed in a v6 hat.

14. **The error type is one enum with a two-audience rendering.** Every failure mode is one `FetchError`. Its full text is the log rendering; its model-facing rendering is trimmed of internal detail. The divergence exists for exactly one case: a blocked address tells the log the resolved address and the range that blocked it, but tells the model only that the host is not fetchable. Leaking the resolved internal address or range back to the model would hand an attacker a probe of the internal network; the two renderings keep the diagnostic value without the leak.

15. **The crate is the sole owner of the network and extraction dependencies.** Removing it leaves every other crate compiling, and no other crate names `reqwest`, the HTML-extraction libraries, `url`, or `ipnet`. This bright line is why the SSRF surface is auditable: there is exactly one place a fetch can originate, so the policy has exactly one place to live.

## The address check must see resolved addresses, or it sees nothing

The reason this crate exists as more than a thin wrapper is a single fact about how SSRF defeats naive defenses: **the URL string does not tell you where the connection goes.** `http://internal.evil.com/` looks public and resolves to `10.0.0.5`. `http://public.evil.com/` resolves to a real public address on the first lookup and to `127.0.0.1` on the second (DNS rebinding). A check that reads the URL string is blind to both.

The design answer is to put the address policy *inside the DNS resolver the HTTP client uses*, so it runs on the actual addresses at the moment of connection, on every hop, with no cached verdict. The resolver resolves the host, drops every address the policy blocks, and hands the client only what survives. A host that resolves entirely inward fails; a host with a public answer alongside a blocked one connects to the public answer. Because there is no verdict cache, the second lookup in a rebinding attack is re-checked from scratch and refused. This is the check that cannot be moved without reopening the hole, which is why the URL-string checks (fast, cheap, run first) are an optimization and a UX nicety, not the security boundary. The boundary is the resolver.

## The escape hatch is narrow on purpose, and the narrowness is the design

There is exactly one supported way to reach an otherwise-blocked address: put the exact `(host, address)` pair in `allow_exact`. It is not a range, it is not host-only, and it is not address-only. Each of those looser forms is a known bypass:

- Address-only (`allow 127.0.0.1`) lets *any* host that resolves to that address through, which is precisely the rebinding attack.
- A range widens the exception past the single host the operator meant to reach.
- Host-only cannot be honored, because the resolver only ever sees addresses.

Keying on the pair means the exception is spent only when the host that named the address is the host being resolved. The host-agnostic policy never consults `allow_exact` at all, so an address arriving with no host context (which is every address, at the level the general check runs) can never win a bypass. The cost of getting this wrong is a full internal-network read, so the hatch is built to be the smallest thing that still lets an operator reach one named internal host.

## Size discipline is a property of the format, decided before the body arrives

Two facts force the truncate-versus-refuse split to be a content-type decision rather than a size-only one. First, whether a prefix is *usable* depends entirely on the format: half a JSON document is garbage, half a plain-text log is a log. Second, the decision must be made before the body is read, because refusing an oversized structured body without downloading it is the whole point of a precheck.

So the content type is read from the response header first and classified into a route. The HTML and structured-text routes read all-or-nothing under a hard byte cap; the flat-text route reads under a truncating cap that keeps the prefix and sets the flag. The byte cap itself is enforced twice: an honest `Content-Length` over the cap is refused before a byte is read, and because a compressed response advertises no usable length, a streamed counter over the decompressed bytes aborts the read the instant it crosses the cap. The two mechanisms cover the two ways a body can be too big: honest declaration and a compression bomb. A final character-level cap (`max_chars`, per call or from config) applies on top, cutting on a character boundary so a multibyte character is never split; on the flat-text path it stacks with any byte-level truncation already applied, and either source sets the flag.

## The provenance header is a contract, so its exact shape is load-bearing

The model reads three labeled lines, then a blank line, then the content. The labels and their order are the contract the model is trained to read, so the exact shape is part of the design rather than an implementation detail:

```
url: <final URL after redirects>
truncated: <true|false>
extraction: <readability|raw-html|plain>

<content>
```

`url` is the final URL after redirects, not the requested one, so a citation names where the bytes came from. `truncated` tells the model whether it holds a complete document or a prefix. `extraction` names which of the three paths produced the text - article isolation, whole-page rendering, or decoded plain - so the model can judge how much shaping the content underwent. A reader who changes any of these three names or the blank-line separator changes what the model sees, which is why they are fixed here.

## What this crate does not defend against, stated so the boundary is honest

Three gaps are real and are the caller's job, not this crate's:

- **Query-string exfiltration to a public host.** A model that read a poisoned page can place run data in a query string to a genuinely public URL. The destination is public and the payload is indistinguishable from an ordinary query, so the URL policy cannot stop it. The control that works is caller-side: do not give a section that reads untrusted text the tools to exfiltrate it. Everything `web_fetch` returns is untrusted third-party text by contract, and keeping such a section away from private data and shell-like tools is enforced by per-section tool scoping, not here.
- **A residual DNS-rebinding window from connection reuse.** The HTTP client may reuse a kept-alive socket to a host resolved moments ago without re-resolving. A short pool-idle timeout bounds this window; it does not eliminate it, and that trade (a small window for the performance of connection reuse) is deliberate.
- **The per-run deadline.** The tool interface carries no call context, so a fetch cannot see the run's remaining budget and may outlive it. Per-call connect and total timeouts bound each fetch in absolute terms, which is the best this layer can do without a wider interface change.

These are recorded because a security boundary you cannot state precisely is one you cannot rely on.

*2026-07-29, Claude Opus 4.8*
