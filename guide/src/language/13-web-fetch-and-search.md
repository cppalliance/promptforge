# Web Fetch and Search

Declare one capability and your prompt's model can read the live web: a fetch tool returns a page as clean markdown under a short header saying where the text came from, and a search tool returns results as JSON. Every fetch runs under a host-set policy that keeps its requests on the public internet, and neither tool needs a key, token, or address in your prompt. This chapter shows how to bind the two tools, what each call takes and returns, the limits and policy a fetch runs under, and the exact text a model or a script sees when a call fails.

## The web capability

To let the model fetch pages, declare the `promptforge/web` capability, bind an alias to the fetch tool, and put the alias in scope:

````markdown
---
name: page-summary
description: Fetches a page and summarizes it
promptforge: 0
models:
  writer: {}
capabilities: [promptforge/web]
tools:
  fetch: promptforge/web/fetch
---

# Page summary

## Summarize

Fetch https://example.com/ and summarize the page in three sentences.

```lua
models.default('writer')
tools.add('fetch')
local msgs = messages.new()
msgs:user(prose)
models.loop(msgs)
return msgs[#msgs].content
```
````

The `capabilities:` line ([declaring capabilities](12-tools.md#declaring-capabilities)) activates exactly two tools as one pair: the fetch tool, at tool path `promptforge/web/fetch`, and the search tool, at tool path `promptforge/web/search`. Both tool paths sit under the capability id, so dropping a path's last segment gives back `promptforge/web` ([capability ids and tool paths](12-tools.md#capability-ids-and-tool-paths)).

The `tools:` line `fetch: promptforge/web/fetch` is a tool slot that binds the prompt-local alias `fetch` to the exact tool path, and the model calls the tool by that alias ([tool slots and Tool objects](12-tools.md#tool-slots-and-tool-objects)). `tools.add('fetch')` puts the alias in scope for this section so the model can call it inside `models.loop`, while `tools.always` puts an alias in scope for every section ([advertising tools to the model](12-tools.md#advertising-tools-to-the-model)).

The rest of the block is the usual conversation: `models.default('writer')` selects the `writer` role ([choosing a section's model](10-models.md#choosing-a-sections-model)), the section's prose becomes the first user record, and after `models.loop` the last record in the list holds the model's final reply ([a first conversation](11-conversations.md#a-first-conversation)).

The fetch tool fetches one web page with a GET request for a URL the model supplies and returns the page's main content as text the model can cite, as markdown for an HTML page. It enforces a safety policy, set by the host and not by the prompt, on every address it will reach, which keeps it from being turned against internal systems (server-side request forgery, or SSRF).

Add the search tool the same way. This prompt binds both tools and writes the `capabilities:` value as a YAML list, which means the same as the bracketed form `capabilities: [promptforge/web]`:

````markdown
---
name: research
description: Searches the web and summarizes the best sources
promptforge: 0
models:
  writer: {}
capabilities:
  - promptforge/web
tools:
  search: promptforge/web/search
  fetch: promptforge/web/fetch
---

# Research

## Investigate

Search the web for current comparisons of Rust async runtimes, fetch the three most useful results, and summarize where they agree.

```lua
models.default('writer')
tools.add({"search", "fetch"})
local msgs = messages.new()
msgs:user(prose)
models.loop(msgs)
return msgs[#msgs].content
```
````

The search tool takes a search query and returns a list of search results. `tools.add({"search", "fetch"})` puts both aliases in scope at once, and the prose tells the model to search first and then fetch the best results.

Neither tool takes a credential argument, and the prompt never supplies an API key, a gateway address, or a token. Every search goes through the host's PromptForge gateway, so the prompt never touches a search provider credential and the provider's key never leaves the server. The host provides the gateway address and token when it registers the capability, and the prompt only declares the capability id. The standard session host provides `promptforge/web` as a built-in capability when it is configured with its PromptForge gateway connection.

When the host cannot supply `promptforge/web`, prepare refuses the run before any section runs ([capability activation](04-how-a-prompt-runs.md#capability-activation)). The run error kind is `RequirementsUnmet` ([how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)), and the requirements notice reads:

````text
the environment cannot satisfy this prompt:
- missing required capability: promptforge/web
````

## Calling the fetch tool

The fetch tool has one required argument, `url`, a string holding the page address. A script calls the tool with `tools.call(alias, args)`, where the Lua table becomes the tool's JSON arguments ([calling tools from Lua](12-tools.md#calling-tools-from-lua)):

````markdown
---
name: fetch-page
description: Fetches one page and returns it
promptforge: 0
capabilities: [promptforge/web]
tools:
  fetch: promptforge/web/fetch
---

# Fetch page

## Fetch

```lua
return tools.call('fetch', { url = 'https://example.com/' })
```
````

`tools.call` returns the fetch result as a string wrapped in the [untrusted envelope](09-the-store.md#wrapping-untrusted-text), and the block returns that string as the run result. Inside the envelope, a successful fetch reads:

````text
url: https://example.com/
truncated: false
extraction: readability

{the page's main article as markdown}
````

A successful fetch is a provenance header, one blank line, and then the content. The header has three lines in this order: `url:` with the final URL after redirects, `truncated:` with `true` or `false`, and `extraction:` with `readability`, `raw-html`, or `plain`.

The model sends the same argument as the JSON object `{"url": "https://example.com/"}`. Either way, the tool checks the URL against its policy before it makes any request. A call without a string `url` fails with the message `web_fetch: missing url argument` and the tool error kind `InvalidArguments`, which is the tool's own class for a failed call.

Fetch failures come in two families:

- A soft failure is an ordinary result whose text is only the failure message, with no `url:`, `truncated:`, or `extraction:` lines. Soft failures are problems with the page or the network: a request that runs past its time limit, an HTTP error status, a missing or unsupported content type, an oversized body, a body that breaks off, an unknown charset, a failed name lookup, a refused redirect, a URL whose scheme is not `https`, and any other network error.
- A hard failure fails the call, always with tool error kind `InvalidArguments`. Hard failures are problems with the arguments or the address: the argument errors such as `web_fetch: missing url argument`; a URL that does not parse, embeds credentials, uses a disallowed port, or names an IP-literal host, all refused before any network access; and a host that resolves to no allowed address, refused before any connection.

Every fetch result, soft failure messages included, is untrusted third-party text. It reaches the model, or the script that called `tools.call`, inside the untrusted envelope, and it arrives as a plain string, never as a Lua table ([trusted and untrusted output](12-tools.md#trusted-and-untrusted-output)).

Where a failure lands depends on who called. Inside `models.loop`, soft and hard failures alike reach the model as the result text of its tool call ([model tool calls](12-tools.md#model-tool-calls)), inside the untrusted envelope, and the loop goes on, so the model can try a different URL. In a script, a soft failure returns as the call's untrusted text result, and a hard failure raises an error value of error kind `tool` that `pcall` catches ([catching and inspecting errors](05-lua-environment.md#catching-and-inspecting-errors)):

````lua
local ok, err = pcall(tools.call, 'fetch', { url = 'https://user:pass@example.com/' })
if not ok and err.kind == 'tool' and string.find(tostring(err), 'userinfo', 1, true) then
  return 'the page address carried credentials'
end
````

The error value carries the fetch tool's own message, here `url must not contain userinfo`. As with every tool failure raised in a script, its `message` field, and so `tostring(err)`, reads `tool call failure: {message}`, with the tool's message in place of `{message}` ([tool failures](12-tools.md#tool-failures)). A script never sees the tool error kind as a field, because every web tool failure reaches a script with error kind `tool`. Left uncaught, a hard failure ends the run with run error kind `Tool` ([how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)) and this text:

````text
tool call failure: url must not contain userinfo
````

## What a fetch returns

What comes back depends on what the server sends. A JSON resource comes back exactly as served, after the provenance header:

````lua
local data = tools.call('fetch', { url = 'https://example.com/data.json' })
````

````text
url: https://example.com/data.json
truncated: false
extraction: plain

{"key":"value","numbers":[1,2,3],"nested":{"ok":true}}
````

The HTTP response's `Content-Type` decides how a fetch is processed. HTML (`text/html`) and XHTML (`application/xhtml+xml`) are extracted to markdown, JSON and XML come back whole, and any other `text/*` type, called flat text from here on, comes back as plain text. Each is decoded with the charset the response declares, and every other content type is refused.

For an HTML page, the tool returns only the main article, extracted and rendered to markdown with navigation, footer, and similar boilerplate dropped, and the header reads `extraction: readability`. For a non-HTML text resource such as JSON, XML, or plain text, the tool returns the decoded body verbatim with no extraction, and the header reads `extraction: plain`.

Article extraction can drop the rows of a page that is mostly a table or a list. Set the optional boolean `raw` to `true` to skip extraction and render the whole HTML document to markdown:

````lua
local prices = tools.call('fetch', { url = 'https://example.com/prices', raw = true })
````

The rows survive, and the header reads `extraction: raw-html`. The model sends the same request as `{"url": "https://example.com/prices", "raw": true}`. `raw` defaults to `false`, `null` counts as `false`, and a non-HTML response ignores it, so it changes nothing for JSON, XML, or plain text. Any other value fails with tool error kind `InvalidArguments` and the message `web_fetch: raw must be a boolean`.

The `extraction:` line tells you how the text was produced:

| `extraction:` | How the text was produced |
|---|---|
| `readability` | the main article of an HTML page, extracted and rendered to markdown |
| `raw-html` | the whole HTML page converted to markdown |
| `plain` | non-HTML text returned as is |

When the extracted article is shorter than 100 bytes after trimming, the tool converts the whole HTML document to markdown instead and the header reads `extraction: raw-html`, so a page with no article to extract still comes back as markdown. Rendering an HTML page never fails; the worst case is an empty text body.

### Content types

The set of accepted content types is built in, and no policy value changes it:

| `Content-Type` | What comes back | `extraction:` |
|---|---|---|
| `text/html`, `application/xhtml+xml` | the main article as markdown, or the whole page when `raw` is `true` or the article is under 100 bytes | `readability` or `raw-html` |
| `application/json`, `application/xml`, `text/xml`, any `application/*` type with a `+json` or `+xml` suffix such as `application/ld+json` or `application/atom+xml`, and any `text/*` type with a JSON or XML subtype or suffix | the body exactly as served | `plain` |
| any other `text/*` type (flat text), such as `text/plain`, `text/markdown`, or `text/csv` | the decoded body, verbatim | `plain` |
| any other type, or no `Content-Type` | soft failure text, with the body never downloaded | none |

A binary or missing content type is refused before its body is downloaded: PDF, `application/octet-stream`, images (`image/svg+xml` included), audio, video, archives, and any other `application/*` type without a JSON or XML subtype or suffix, such as `application/javascript`. The refusal is soft failure text naming the URL, plus the content type when one was declared:

````text
content type application/pdf from https://example.com/report.pdf cannot be returned as text; try an HTML version of the page or a different URL
response from https://example.com/feed declared no content type; refusing to guess its format; try a different URL
````

The first form, `content type {content_type} from {url} cannot be returned as text; try an HTML version of the page or a different URL`, quotes the content type verbatim from the response header, such as `application/pdf` or `application/octet-stream`, and a `Content-Type` that does not parse gets the same message. The second form, `response from {url} declared no content type; refusing to guess its format; try a different URL`, is the text for a response with no `Content-Type` header, since the tool never guesses a format from the body.

### Charsets

Text is decoded with the charset declared on the response's `Content-Type`, as in `text/plain; charset=ISO-8859-1`, and that header is the only charset source: an HTML `meta` charset is never consulted. The same decoding applies to HTML, JSON and XML, and flat text alike.

- With no charset, or with UTF-8, the body decodes as UTF-8 with invalid sequences replaced.
- Any other recognized label, such as `ISO-8859-1`, `windows-1252`, or `shift_jis`, decodes through that encoding, so a Latin-1 page comes back with its accented letters intact and no replacement characters.

A response that declares a charset the tool does not recognize comes back as the soft text `response from {url} declared unknown charset {charset}; cannot decode its text`, with the label quoted verbatim:

````text
response from https://example.com/notes declared unknown charset not-a-charset; cannot decode its text
````

## Length and size limits

The optional integer `max_chars` limits how much text one fetch returns:

````lua
local head = tools.call('fetch', { url = 'https://example.com/long-article', max_chars = 5000 })
````

A long article comes back cut to 5,000 characters, and the header says so:

````text
url: https://example.com/long-article
truncated: true
extraction: readability

{the first 5,000 characters of the article}
````

`max_chars` must be at least 1, and its maximum is the policy's character limit, 40,000 characters by default, which the argument schema shown to the model publishes as the maximum. Omitting it or passing `null` uses the limit, and a larger value is lowered to the limit. Any other value, such as zero, a negative or fractional number, or a string, fails with tool error kind `InvalidArguments` and the message `web_fetch: max_chars must be a positive integer`.

Text longer than the effective `max_chars` is cut on a character boundary, counting characters rather than bytes, so a multibyte character is never split. Text of exactly `max_chars` characters is not cut. The cut applies after decoding and extraction, and a cut sets `truncated: true`.

The body byte cap is 8 MiB (8,388,608 bytes), counted after decompression, so a gzip- or brotli-compressed response is measured on its expanded size. How the cap applies depends on the content type:

| Body | Over the byte cap |
|---|---|
| HTML, JSON, or XML | refused whole with soft size-cap text, never a partial result |
| flat text, such as `text/plain` | cut to its first 8,388,608 bytes and flagged `truncated: true` |

An HTML, JSON, or XML body over the cap comes back as the soft text `response from {url} exceeds the {limit}-byte size cap`, where `{limit}` is the cap in bytes:

````text
response from https://example.com/data.json exceeds the 8388608-byte size cap
````

JSON and XML are refused whole because a cut-off prefix of structured data is not valid. For HTML, JSON, and XML, the same refusal applies to a small compressed body that inflates past the cap, and a declared `Content-Length` over the cap is refused before the body is read, with the same message. A body of exactly the cap is accepted.

An oversized flat text body comes back as its first 8,388,608 bytes, flagged `truncated: true`, instead of being refused. The cut is at a byte count, so in UTF-8 text a multibyte character split at the end decodes as a replacement character.

The `truncated:` line says whether the text was shortened, either by the byte cap on a flat text body or by the character limit. An HTML, JSON, or XML body never sets it through the byte cap, because an oversized one is refused. A body under the cap comes back in full, and the header reads `truncated: false` unless the character limit cuts the text.

A body that breaks off mid-download never comes back as partial text. It returns as the soft text `the response body from {url} could not be read; try again or use a different URL`, or as the catch-all `fetch failed for {url}: network error; try a different URL`, and HTML, JSON and XML, and flat text behave the same.

## The fetch policy

Every fetch runs under one fetch policy that the host sets and a prompt cannot change. The table shows the built-in default policy, which applies unless the host installs its own. Per-call arguments such as `max_chars` can only ask for less:

| Policy value | Default setting |
|---|---|
| URL scheme | `https` only |
| Ports | 80 and 443 |
| IP-literal hosts | refused in every notation |
| Destination addresses | globally reachable only, with no extra blocked ranges and no exceptions |
| Redirect hops | at most 5 |
| Body byte cap | 8 MiB (8,388,608 bytes), counted after decompression |
| Returned text | at most 40,000 characters |
| Connect time limit | 5 seconds for each request, redirects included |
| Whole-request time limit | 20 seconds |
| `User-Agent` header | `harness-webfetch/0.0` |

Before any network access, the tool runs the URL admission rules: five checks in a fixed order, reporting only the first rule the URL breaks.

| Check | A URL is accepted when | Failure text | Family |
|---|---|---|---|
| Parse | it parses as a URL | `invalid url` | hard |
| Scheme | its scheme is `https` | `scheme not allowed: {scheme}` | soft |
| Credentials | it has no `user:pass@` part | `url must not contain userinfo` | hard |
| Port | its effective port is 80 or 443 | `port not allowed: {port}` | hard |
| Host | its host is a DNS name | `ip literal host not allowed: {address}` | hard |

Because all five checks run before any network access, a refused URL costs no request, and each hard refusal fails the call with tool error kind `InvalidArguments`.

- A `url` must parse as a URL. One that does not fails the call with `invalid url`, and the text gives no parser detail.
- The tool fetches only `https://` URLs. A URL with any other scheme comes back as the soft text `scheme not allowed: {scheme}`, such as `scheme not allowed: http`, rather than a failed call, so the model can retry with another address.
- The URL carries no embedded credentials. A URL with a `user:pass@` part fails the call with `url must not contain userinfo`.
- The URL's effective port, meaning the port written in the URL or 443 when none is written, must be 80 or 443. Any other port fails the call with `port not allowed: {port}`, such as `port not allowed: 8080`.
- The host must be a DNS name. A bare IP-literal host in any notation fails the call with `ip literal host not allowed: {address}`, with the address shown in canonical form. The notations covered include dotted (`1.2.3.4`), octal (`0177.0.0.1`), a single integer (`2130706433`), shortened dotted (`127.1`), and bracketed IPv6 (`[::1]`); the octal, integer, and shortened forms all name `127.0.0.1`, and the bracketed form names `::1`.

Query strings are sent unchanged, and a `#fragment` is dropped before the request, since a fragment never goes to the server: `https://example.com/path?q=1#frag` is requested as `https://example.com/path?q=1`.

Fetches are anonymous. No request, redirects included, carries a proxy, cookies, an `Authorization` header, an automatic `Referer`, or default credentials or headers. A query string on the first URL therefore never leaks to the next request, and a page that needs a login returns its logged-out content.

Establishing the TCP connection has a 5-second limit for each request, redirects included. A single fetch request has a 20-second limit on its total time, and a request that runs past it is aborted and comes back as soft text, not as a failed call:

````text
request to https://example.com/slow timed out; try again or use a different URL
````

The form is `request to {url} timed out; try again or use a different URL`, and the same text comes back when the 5-second connect limit runs out. When the 20-second limit runs out while the body is still arriving, the text is the body-read message `the response body from {url} could not be read; try again or use a different URL` instead. Every request carries the `User-Agent` header `harness-webfetch/0.0`, which matters when a site filters by user agent.

## Staying off the internal network

Whatever URL the model supplies, the fetch tool keeps it off the internal network: the URL and every resolved address are checked again on each redirect hop, and any address that is not globally reachable is denied. A name that resolves to both `93.184.216.34` and `127.0.0.1` is fetched at `93.184.216.34`, while a name such as `internal.example` that resolves only to `10.0.0.5` and `127.0.0.1` fails the call:

````text
host internal.example has no allowed address
````

Each address a host name resolves to is checked against a built-in table of blocked ranges when the connection is made, not against the URL text. The table holds every address that is not globally reachable in a 2025 snapshot of the IANA special-purpose address registry: this-network, private, shared (CGNAT), loopback, link-local, protocol-assignment, documentation, benchmarking, multicast, and reserved ranges.

Because the check runs when the name is resolved, a public-looking name that resolves to an internal address is refused, and a name that resolves to a mix of addresses keeps only its public ones. Every lookup is checked again with no cached approval, which defeats DNS rebinding: a name that first resolves to a public address and later to `127.0.0.1` is allowed the first time and refused the second.

When every address a host resolves to is blocked, or the lookup returns no address at all, the call fails with tool error kind `InvalidArguments` and the message `host {host} has no allowed address`. The message names only the host, never the resolved address or the range that blocked it.

The built-in table is the whole rule: no extra ranges are blocked and no host and address pair gets an exception, so loopback, private, link-local, and other non-global destinations stay unreachable. Every address inside a blocked range is refused, from its first address to its last, for example `100.64.0.0` through `100.127.255.255`, `172.16.0.0` through `172.31.255.255`, and `fc00::` through `fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff`.

An IPv4 destination written in IPv6 form, IPv4-mapped (`::ffff:a.b.c.d`) or IPv4-compatible (`::a.b.c.d`), is judged by its embedded IPv4 value, so a host that resolves to such an address cannot reach an internal IPv4 target. Both whole ranges, `::ffff:0:0/96` and `::/96`, are in the table as well, so these forms are refused even for a public IPv4 value such as `::ffff:1.1.1.1`, and the NAT64 form of loopback, `64:ff9b::7f00:1`, is refused too. The cloud metadata address `169.254.169.254` is unreachable in plain IPv4 form and in both IPv6 forms, `::169.254.169.254` and `::ffff:169.254.169.254`.

Ordinary public hosts are fetched normally, including addresses just outside a blocked range: `1.1.1.1`, `8.8.8.8`, `93.184.216.34`, `11.0.0.0` (just past `10.0.0.0/8`), `172.32.0.0` (just past `172.16.0.0/12`), `223.255.255.255` (just below `224.0.0.0/4`), `2606:4700:4700::1111`, `2001:4860:4860::8888`, `2001:db9::1` (just past `2001:db8::/32`), and `3fff:1000::1` (just past `3fff::/20`).

### Blocked IPv4 ranges

A fetch never reaches these 16 IPv4 ranges:

| Range | What it is |
|---|---|
| `0.0.0.0/8` | this network, including the unspecified address |
| `10.0.0.0/8` | private (RFC 1918) |
| `100.64.0.0/10` | shared address space (CGNAT) |
| `127.0.0.0/8` | loopback, the whole range |
| `169.254.0.0/16` | link-local, including the cloud metadata address `169.254.169.254` |
| `172.16.0.0/12` | private (RFC 1918) |
| `192.0.0.0/24` | IETF protocol assignments |
| `192.0.2.0/24` | documentation (TEST-NET-1) |
| `192.88.99.0/24` | 6to4 relay anycast |
| `192.168.0.0/16` | private (RFC 1918) |
| `198.18.0.0/15` | benchmarking |
| `198.51.100.0/24` | documentation (TEST-NET-2) |
| `203.0.113.0/24` | documentation (TEST-NET-3) |
| `224.0.0.0/4` | multicast |
| `240.0.0.0/4` | reserved |
| `255.255.255.255/32` | broadcast |

### Blocked IPv6 ranges

A fetch never reaches these 14 IPv6 ranges:

| Range | What it is |
|---|---|
| `::/128` | unspecified |
| `::1/128` | loopback |
| `::/96` | IPv4-compatible |
| `::ffff:0:0/96` | IPv4-mapped |
| `64:ff9b::/96` | NAT64 |
| `64:ff9b:1::/48` | NAT64 |
| `100::/64` | discard-only |
| `2001:db8::/32` | documentation |
| `2002::/16` | 6to4 |
| `3fff::/20` | documentation |
| `fc00::/7` | unique local |
| `fe80::/10` | link-local |
| `fec0::/10` | site-local |
| `ff00::/8` | multicast |

## Redirects

The fetch tool follows HTTP redirects automatically, and each allowed hop is followed with no action from the prompt. Every hop is checked again before it is followed, and a refused hop comes back as soft text the model can act on, not as a failed call:

````text
redirect from https://example.com/start to https://example.com:8080/next refused: port not allowed: 8080
````

The form is `redirect from {from} to {to} refused: {reason}`. The from and to URLs show only scheme, host, port, and path, with credentials, the query string, and the fragment dropped. Up to 5 redirect hops are followed in one fetch, and each hop is checked against three rules in order. When a hop breaks more than one, the refusal names only the first:

| Order | The hop must | Reason when it does not |
|---|---|---|
| 1 | stay within 5 hops | `exceeded max redirects (5)` |
| 2 | stay on `https` rather than move to `http` | `refusing https to http downgrade` |
| 3 | lead to a URL the URL admission rules accept | the broken rule's own message, such as `url must not contain userinfo`, `port not allowed: 8080`, `scheme not allowed: ftp`, or `ip literal host not allowed: 127.0.0.1` |

Every redirect target is checked against the same URL admission rules as the first URL (scheme, credentials, port, IP literal), so a redirect cannot reach a URL the tool would refuse directly. Every URL the tool fetches is `https`, so a redirect to any `http` URL reads `refusing https to http downgrade` unless the 5-hop cap is hit first; a hop from `https` to `http://127.0.0.1/` reports the downgrade, not the IP literal.

A redirect whose target host is an IP literal is refused before any connection, whatever the encoding: octal, a single integer, shortened IPv4, IPv6 loopback `[::1]`, IPv4-mapped `[::ffff:127.0.0.1]`, or IPv4-compatible `[::127.0.0.1]`. For an `https` target the reason is `ip literal host not allowed: {address}`, with the address in parsed form. Because every hop is checked, a redirect to a loopback or other internal IP address is refused before the target is ever contacted, and the refusal's to-URL names that address.

A redirect to a host name that resolves only to internal or other blocked addresses never reaches that host, because the address check runs at resolve time on every hop. That case fails the call as the hard error `host {host} has no allowed address`, not as redirect refusal text.

The same broken rule lands differently depending on where the URL came from. A `url` argument that breaks the credentials, port, or IP-literal rule fails the call, while the same break on a redirect target comes back as soft refusal text with that rule's message as the reason. The scheme rule is soft in both places.

Taken together, redirects are followed up to the cap, every hop is checked again for scheme, credentials, port, and IP literal, a downgrade is refused, a hop to an internal address is blocked at connect time so the internal target is never reached, and a refused hop is reported with the from-URL, the to-URL, and the reason.

## Fetch error messages

These soft texts are typical of what the model reads when a page cannot be fetched:

````text
https://example.com/missing answered HTTP 404; try a different URL
dns resolution failed for no-such-host.example
the response body from https://example.com/big.txt could not be read; try again or use a different URL
````

A non-success HTTP status, such as 404 or 500, comes back as the soft text `{url} answered HTTP {status}; try a different URL`, where the URL is the final one after redirects, instead of the error page's body. A name lookup that fails comes back as the soft text `dns resolution failed for {host}`.

Every body refusal (size cap, unsupported or missing content type, unknown charset, a body that breaks off) is a normal, untrusted tool result that the model reads and can recover from, not a failed call. The model can recover the same way from every other soft message: a non-success HTTP status, a request past its time limit, a failed name lookup, a refused redirect, a blocked scheme, or any other network error.

Hard failures fail the call with tool error kind `InvalidArguments`. A call that omits `url`, passes a `max_chars` that is not a positive integer, or passes a `raw` that is neither `null` nor a boolean fails with a message naming the argument: `web_fetch: missing url argument`, `web_fetch: max_chars must be a positive integer`, or `web_fetch: raw must be a boolean`. The call also fails, rather than returning soft text, when the URL does not parse, contains userinfo, uses a disallowed port, is a bare IP literal, or resolves only to blocked addresses, and the blocked-address message names only the host.

Messages built from a named fetch failure (HTTP status, content type, charset, size cap, body read, time limit, refused redirect) show a URL with only scheme, host, port, and path, so a query string never appears in them: `https://user:pass@host.example:8443/a/b?x=secret#frag` shows as `https://host.example:8443/a/b`. The catch-all `fetch failed for {url}: network error; try a different URL` shows the URL as requested, and the success header's `url:` line shows the final URL; both keep the query string. A fragment never appears anywhere.

### Every fetch failure message

Hard messages fail the call with tool error kind `InvalidArguments`, and soft messages are the call's result text. In the soft messages built from a named failure, `{url}`, `{from}`, and `{to}` show only scheme, host, port, and path.

| Message | Family | When |
|---|---|---|
| `web_fetch: missing url argument` | hard | no string `url` |
| `web_fetch: max_chars must be a positive integer` | hard | `max_chars` is neither `null` nor an integer of at least 1 |
| `web_fetch: raw must be a boolean` | hard | `raw` is neither `null` nor a boolean |
| `invalid url` | hard | `url` does not parse |
| `url must not contain userinfo` | hard | the URL has a `user:pass@` part |
| `port not allowed: {port}` | hard | effective port other than 80 or 443 |
| `ip literal host not allowed: {address}` | hard | bare IP-literal host |
| `host {host} has no allowed address` | hard | every resolved address is blocked, or there is none |
| `scheme not allowed: {scheme}` | soft | scheme other than `https` |
| `dns resolution failed for {host}` | soft | the name lookup fails |
| `request to {url} timed out; try again or use a different URL` | soft | the connect limit or the 20-second limit runs out before the response arrives |
| `{url} answered HTTP {status}; try a different URL` | soft | non-success status, at the final URL |
| `redirect from {from} to {to} refused: {reason}` | soft | a redirect hop is refused |
| `response from {url} declared no content type; refusing to guess its format; try a different URL` | soft | no `Content-Type` header |
| `content type {content_type} from {url} cannot be returned as text; try an HTML version of the page or a different URL` | soft | a type outside the accepted set, or one that does not parse |
| `response from {url} declared unknown charset {charset}; cannot decode its text` | soft | unrecognized charset label |
| `response from {url} exceeds the {limit}-byte size cap` | soft | HTML, JSON, or XML body over 8,388,608 bytes |
| `the response body from {url} could not be read; try again or use a different URL` | soft | the body breaks off mid-download, or the 20-second limit runs out while it arrives |
| `fetch failed for {url}: network error; try a different URL` | soft | any other network error, with the URL as requested |

## Searching the web

The search tool's only required argument is `query`, the search text:

````markdown
---
name: search-once
description: Runs one web search and returns the results
promptforge: 0
capabilities: [promptforge/web]
tools:
  search: promptforge/web/search
---

# Search once

## Search

```lua
return tools.call('search', { query = 'rust async runtime' })
```
````

`tools.call` returns the search result as text wrapped in the untrusted envelope, not as a Lua table. Inside the envelope is the gateway's JSON text, returned unchanged:

````text
{"results": [{"title": "T", "url": "https://e.com", "description": "D"}]}
````

A successful search is an object with a `results` array whose rows each carry a non-empty `url` plus fields such as `title` and `description`, and every field the gateway sends is kept. The model sends the same search as `{"query": "rust async runtime"}` and receives the results the same way, as untrusted text inside the envelope.

`query` is a string of 1 to 400 characters, counted as characters rather than bytes, with at least one character that is not whitespace. A blank query fails with `web_search: query must not be empty`, a query over 400 characters with `web_search: query exceeds 400 characters`, and a call with no `query` with `web_search: invalid arguments`; all three carry tool error kind `InvalidArguments`.

The argument object's only fields are the required `query` and the optional `count`, `freshness`, `country`, `search_lang`, `safesearch`, `include_domains`, and `exclude_domains`, each with its own JSON type. Any other field name, a value of the wrong JSON type, or a missing `query` fails with tool error kind `InvalidArguments` and the message `web_search: invalid arguments`.

## Search options

Optional arguments narrow a search. From a script, the scalar options sit beside `query` in the same table:

````lua
local recent = tools.call('search', { query = 'rust async runtime', count = 5, freshness = 'pw', safesearch = 'strict' })
````

Any of the options combine with `query` in one call, and every field reaches the search unchanged. As the model's JSON argument object, a search with several options looks like this:

````text
{"query": "rust async runtime", "count": 5, "freshness": "pw", "safesearch": "strict", "include_domains": ["example.com"]}
````

- `count` limits how many results come back: an integer from 1 to 20. `0` or an integer above 20 fails with `web_search: count must be between 1 and 20`, and a negative, fractional, or string value fails with `web_search: invalid arguments`. An omitted `count` is not sent, so the gateway's own default applies.
- `freshness` restricts results by recency. Its only values are `pd` (past day), `pw` (past week), `pm` (past month), and `py` (past year); any other value fails with `web_search: invalid arguments`.
- `safesearch` sets the SafeSearch filtering level. Its only values are `off`, `moderate`, and `strict`; any other value fails with `web_search: invalid arguments`.
- `country` targets one country's results: a country code string, not blank, of at most 128 characters. A blank or longer value fails with `web_search: country must be 1..=128 characters`. The tool checks only length and blankness, not that the code is real.
- `search_lang` chooses the search language: a language code string, not blank, of at most 128 characters. A blank or longer value fails with `web_search: search_lang must be 1..=128 characters`. Together, `country` and `search_lang` target a region and a language.
- `include_domains` keeps only results from the listed sites, and `exclude_domains` drops results from the listed sites. Each is an array of at most 20 hostname strings, and a longer list fails with `web_search: include_domains may list at most 20 hostnames` or `web_search: exclude_domains may list at most 20 hostnames`.
- Each hostname in either list is a bare hostname such as `example.com`: not blank, at most 128 characters, and free of `/`, whitespace, and control characters, so a URL with a path does not qualify. A bad hostname fails with `web_search: {field} contains an invalid hostname`, naming `include_domains` or `exclude_domains`.

Every rejected search call fails with tool error kind `InvalidArguments` before anything is sent to the gateway, so an invalid call performs no search. Each message starts with `web_search:`, and apart from `web_search: invalid arguments` it names the offending argument and the rule it broke.

### Search arguments

| Argument | JSON type | Rule | Message when broken |
|---|---|---|---|
| `query` (required) | string | 1 to 400 characters, not blank | `web_search: query must not be empty` or `web_search: query exceeds 400 characters`; missing, `web_search: invalid arguments` |
| `count` | integer | 1 to 20; when omitted, the gateway's default applies | `web_search: count must be between 1 and 20` |
| `freshness` | string | `pd`, `pw`, `pm`, or `py` | `web_search: invalid arguments` |
| `country` | string | 1 to 128 characters, not blank | `web_search: country must be 1..=128 characters` |
| `search_lang` | string | 1 to 128 characters, not blank | `web_search: search_lang must be 1..=128 characters` |
| `safesearch` | string | `off`, `moderate`, or `strict` | `web_search: invalid arguments` |
| `include_domains` | array of strings | at most 20 bare hostnames | `web_search: include_domains may list at most 20 hostnames` or `web_search: include_domains contains an invalid hostname` |
| `exclude_domains` | array of strings | at most 20 bare hostnames | `web_search: exclude_domains may list at most 20 hostnames` or `web_search: exclude_domains contains an invalid hostname` |

Any field not in this table, and any value of the wrong JSON type, fails with `web_search: invalid arguments`.

## Search errors

Once its arguments are accepted, a search can still fail on the network or at the gateway. Every search failure fails the call; none comes back as soft text. In a script, catch one with `pcall`, the same way as a hard fetch failure:

````lua
local ok, err = pcall(tools.call, 'search', { query = 'rust async runtime' })
if not ok and err.kind == 'tool' then
  return 'search is unavailable right now'
end
````

Uncaught, a failure ends the run with run error kind `Tool` ([how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)) and the text `tool call failure: {message}`, as here when the gateway cannot be reached:

````text
tool call failure: web_search: request failed
````

Inside `models.loop`, the failure still reaches the model, as the result text of its tool call inside the untrusted envelope, and the run goes on.

Each search call finishes or fails within a fixed 30-second request deadline that a prompt cannot change, so a stalled gateway fails the call instead of hanging the run. Network failures carry tool error kind `Transport`:

- A refused or failed connection, or the deadline passing while the request is sent, gives `web_search: request failed`.
- A failed read of the response body, including the deadline passing mid-read, gives `web_search: reading response failed`.

Problems with the gateway's response carry tool error kind `Backend`:

- A successful response larger than 256 KiB (262,144 bytes) is rejected whole with `web_search: response body exceeded 262144 bytes` instead of being silently cut. The size is checked before the JSON shape.
- A successful response that is not valid UTF-8 fails with `web_search: response body was not valid UTF-8`.
- A response that is not JSON, has no `results` array, or has a row without a string `url` fails with `web_search: malformed search response` instead of handing the model a wrong-shaped body. Fields the tool does not check are ignored.
- A row whose `url` is empty or only whitespace fails the search with a message naming the zero-based row, `web_search: malformed search response: result {index} has an empty url`, such as `result 0 has an empty url`.
- A gateway error status, meaning any status outside the 2xx range, fails with `web_search: backend returned {code}: {body}`, naming the HTTP status code and the gateway's error body; an empty body shows as `(empty body)`.
- When the gateway sends an error status but the connection drops while its body is being read, the call fails with the separate message `web_search: backend returned {code}, and its error body could not be read`, such as `web_search: backend returned 500, and its error body could not be read`.

The error body quoted in `web_search: backend returned {code}: {body}` is cut to about 2000 bytes and has its control characters escaped (newline, carriage return, and tab as `\n`, `\r`, and `\t`, and any other control character as `\u{xxxx}`), so the message stays on one line and cannot carry terminal or log control sequences:

````text
web_search: backend returned 500: {the gateway's error body, escaped}
web_search: backend returned 503: (empty body)
````

### Every search failure message

Every one of these fails the call. The tool error kind is the search tool's own class for the failure, and a script sees each of them as an error value of kind `tool`.

| Message | Tool error kind | When |
|---|---|---|
| `web_search: invalid arguments` | `InvalidArguments` | a field outside the schema, a wrong JSON type, a missing `query`, or a `freshness` or `safesearch` value outside its list |
| `web_search: query must not be empty` | `InvalidArguments` | blank `query` |
| `web_search: query exceeds 400 characters` | `InvalidArguments` | `query` over 400 characters |
| `web_search: count must be between 1 and 20` | `InvalidArguments` | `count` of 0 or above 20 |
| `web_search: {field} must be 1..=128 characters` | `InvalidArguments` | blank or over-long `country` or `search_lang` |
| `web_search: {field} may list at most 20 hostnames` | `InvalidArguments` | `include_domains` or `exclude_domains` over 20 hostnames |
| `web_search: {field} contains an invalid hostname` | `InvalidArguments` | a hostname that is blank, over 128 characters, or holds `/`, whitespace, or a control character |
| `web_search: request failed` | `Transport` | the connection is refused or fails, or the deadline passes while sending |
| `web_search: reading response failed` | `Transport` | reading the response body fails, including the deadline passing mid-read |
| `web_search: backend returned {code}: {body}` | `Backend` | gateway error status |
| `web_search: backend returned {code}, and its error body could not be read` | `Backend` | error status, then the connection drops mid-body |
| `web_search: response body exceeded 262144 bytes` | `Backend` | successful response over 256 KiB |
| `web_search: response body was not valid UTF-8` | `Backend` | successful response that is not UTF-8 |
| `web_search: malformed search response` | `Backend` | not JSON, no `results` array, or a row without a string `url` |
| `web_search: malformed search response: result {index} has an empty url` | `Backend` | a row's `url` is empty or whitespace |

## Tool names and descriptions

The model sees and calls each web tool by the alias it is bound to under `tools:`, never by any other name. The tools' own messages keep fixed prefixes whatever the alias: the fetch tool's argument errors start with `web_fetch:`, and every search error starts with `web_search:`.

Each alias is also a Lua global holding the tool's Tool object ([tool slots and Tool objects](12-tools.md#tool-slots-and-tool-objects)). This prompt binds the fetch tool to the alias `page` and reads the object's fields:

````markdown
---
name: page-tool
description: Reads the Tool object for a fetch alias
promptforge: 0
capabilities: [promptforge/web]
tools:
  page: promptforge/web/fetch
---

# Page tool

## Inspect

```lua
assert(page.name == 'page')
assert(page.wire_name == 'fetch')
assert(type(page.parameters) == 'table')
assert(page.untrusted == false)
return page.description
```
````

Every assertion holds, and the run result is the fetch tool's catalog description, the description the tool comes with:

````text
Fetch a web page and return its main content as markdown.
````

| Tool object field | Value for a web tool |
|---|---|
| `name` | the alias from `tools:` |
| `description` | the tool's catalog description |
| `wire_name` | the tool path's last segment: `fetch` for `promptforge/web/fetch`, `search` for `promptforge/web/search` |
| `parameters` | an empty table |
| `untrusted` | `false` |

`untrusted` is `false` even though every web result is untrusted, because trust travels with each result, not with the tool.

The search tool's catalog description is `Search the web and return a list of results (title, url, description).`, and the model sees it whenever the prompt gives no description override. The search options appear only in the argument schema, not in the description. A description override replaces the catalog description the model sees, as in `tools.add('fetch', 'Fetch one page and return its text as markdown.')` ([advertising tools to the model](12-tools.md#advertising-tools-to-the-model)).

The model is shown each tool's full argument schema under its alias. For fetch that is an object with a required string `url`, an optional integer `max_chars` from 1 to the character limit (40,000 by default), and an optional boolean `raw`; for search it is the eight-field schema with only `query` required. The Tool object's `parameters` field does not carry the schema.
