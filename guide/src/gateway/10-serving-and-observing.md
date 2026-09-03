# Serving and Observing

This chapter teaches you the running gateway: the HTTP endpoints it serves, the tools it can host, and the health, logs, and observability surface you operate day to day. You already run a configured gateway with a profile and its models.

## Web search

Enable the built-in web-search tool with a `[tools.web_search]` section:

````
[tools.web_search]
provider = "brave"
api_key = "${BRAVE_API_KEY}"
default_count = 10
max_count = 20
max_per_host = 2
strip_tracking = true
````

The provider is locked to `brave`. The `base_url` defaults to the Brave Search endpoint and must be an HTTP(S) URL. The `default_count` must not exceed `max_count`. Freshness and safesearch defaults are closed vocabularies, not free text. The gateway calls the Brave Search API at `{base_url}/web/search` with the configured API key sent in the `X-Subscription-Token` header.

Callers run a web search through POST /v1/tools/web_search. The request body carries a `query` and optional `count`, `freshness`, `country`, `search_lang`, `safesearch`, `include_domains`, and `exclude_domains`. Unknown fields are rejected. The query is trimmed, rejected when empty, and capped at 512 characters. Caller knobs are validated before any provider call: freshness must be `pd`, `pw`, `pm`, `py`, or a date range; safesearch must be `off`, `moderate`, or `strict`; country is a 2-letter code; the search language is a 2 or 3 letter code; each domain entry must be a bare valid domain. The count defaults to `default_count` and clamps into 1 through `max_count`. The gateway over-fetches up to three times the requested count, capped at `max_count`, so post-processing filters still yield enough results. Omitted freshness and safesearch fall back to the configured defaults.

Results carry `title`, `url`, `site_name`, and `extra_snippets`. Result text is sanitized and capped, results are diversified by host at `max_per_host`, and a result whose URL is not navigable or is over 2048 characters is dropped. When `strip_tracking` is on, known tracking parameters such as `utm_*`, `fbclid`, `gclid`, `mc_cid`, and `mc_eid` are removed from result URLs. Include and exclude domain lists match the host itself or any subdomain.

When no `[tools.web_search]` section is configured, the route answers 404. The route exists only in builds compiled with the `web-search` feature. Search provider failures surface with a `web_search: ` prefix on the error, so you can distinguish search upstream errors from other gateway errors. The search service is built from the active profile's `[tools.web_search]` section and reloads on profile switch. The provider credential never appears in logs.

## Host the workshop

Have the gateway host the workshop UI on a second loopback listener by adding a `[workshop]` section:

````
[workshop]
bind = "127.0.0.1:7910"
open_browser = true
````

The bind address defaults to 127.0.0.1:7910, and a non-loopback bind is refused at startup. The section is boot-only, like `[server]`. The hosted workshop derives its client URL from the gateway's own `[server]` bind and reuses the same `api_key`, so you configure no second credential. A port-0 bind resolves to the actually bound port in the derived URL. The workshop's state directory and agent-program directory default to the boot config file's directory.

Setting `open_browser = true` opens the system browser at the workshop URL once the gateway is serving; a browser that fails to open only logs a warning. A gateway built without the `workshop` feature logs a warning and hosts nothing when the boot config carries a `[workshop]` section. Both the gateway and workshop listeners answer /health and /v1/models on their own ports. The workshop's stop outcome appears in the logs: graceful, forced down after its drain window, or stopped with an error.

## Manage the cache

Manage the blob cache through the gateway's cache routes. GET /v1/cache lists entries with source URL, path, SHA-256, and size. Only blobs carrying a `.meta.json` sidecar appear in the listing, and listing reads the sidecar metadata only; it never re-hashes the blobs. POST /v1/cache downloads a blob with an optional pin and streams progress events ending in a ready event. DELETE removes one blob by digest. Cache downloads validate the source URL and the pin before any network access. A cache download lands in the same slot layout that local model provisioning uses, so a cache download is a provisioning cache hit for the same URL, and vice versa.

GET /admin/orphans lists cache files that no configured model references, so leftovers can be adopted or deleted. GET /admin/model-info reports a GGUF file's header summary (architecture, layer count, parameter count, and chat template) without loading the model; only files inside the artifact cache can be inspected, and escaping or missing paths are refused. POST /admin/reveal opens the host's file manager at a model or config file; reveal requests are confined three ways: loopback-only, bearer key required, and the path must canonicalize to strictly inside the artifact cache.

The gateway restricts the cache root to your own account at startup and refuses to run when it cannot, failing with a cache-not-private error.

## Status, progress, and metrics

GET /admin/status reports the active profile, the models it exposes, and a config generation that changes when the gateway restarts. GET /admin/profiles lists the profiles in the loaded catalog.

GET /admin/progress streams every long-running operation in the process as one server-sent event stream. A fresh subscriber first receives live operations replayed, then every event. Heartbeat comment lines arrive every 15 seconds while idle.

Download progress renders as terminal progress bars when the gateway runs on a TTY, and as tracing log lines otherwise.

GET /admin/system reports host metrics: CPU, RAM, the cache drive, and the first NVIDIA GPU's VRAM. The GPU field is absent, never an error, when no capable driver is present. You can also pull bounded captured stdout and stderr tails for each running local model as diagnostics.

GET /admin/chat-templates returns a bearer-authenticated catalog of chat template families, known model-to-family mappings, and each pending local model's effective template decision.

You can search Hugging Face and read model details and READMEs through the gateway's hub proxy. A missing or invalid `HF_TOKEN` surfaces as a distinct "set HF_TOKEN" error. Hub search queries are validated against a closed allowlist before any upstream call, and repository paths must be an exact owner/name pair of hub-legal segments.

## Errors and limits

Every request failure reaches the client in the OpenAI error envelope: an object with `message`, `type`, and `code` under `error`, with a stable HTTP status. Examples: 401 `unauthorized`, 404 `model_not_found`, 400 `malformed_request`, 400 `kind_mismatch`, 429 `queue_rejected`, 503 `queue_full`, 503 `profile_switch`, 503 `partial_start`, 422 `config_write_rejected`, and 422 `model_info_error`.

Outbound calls to any backend have fixed timeouts: 10 seconds to connect and 120 seconds for a whole non-streaming request. Streaming connections are bounded only by the connect timeout. Response bodies the gateway reads are capped: 64 KiB for error bodies and 4 MiB for success JSON bodies.

Malformed client requests are rejected at the boundary. An empty model name, an empty messages array, an unsupported message role, or a message with neither content nor a tool call all fail validation. Request fields the gateway does not name pass through to the backend verbatim, while the reserved keys `model`, `messages`, and `stream` may not be smuggled in twice. Embeddings requests accept one string or a batch of strings, with an optional `encoding_format` of `float` or `base64`; an empty batch is rejected. Rerank requests carry a query, a document set, and an optional `top_n` limit; an empty query or document set is rejected.

## Reading failures

The error code distinguishes a connection that never reached the provider from a mid-flight failure. The first is safe to retry; nothing was billed. The second is not safe to retry blindly. A backend's own client-error status, for example 429, passes through to the caller with code `upstream_client_error` instead of a generic 502. A model of the wrong kind is refused with 400 `kind_mismatch` before any upstream call. A request for a workload the resolved model cannot serve is rejected with 400 `model_unavailable`.

When the gateway recovers from a malformed tool fence in an emulated tool dialect, the response message carries a `gateway_warning` extension field. The turn never fails, and protocol junk never appears as final text. Streaming clients still receive tool calls from an emulated-dialect model: the gateway buffers one upstream round trip and re-emits the rewritten response as synthetic chunks, with a trailing summary chunk carrying usage and timings.

A malformed upstream stream chunk is logged and skipped without ending the stream. A mid-stream transport failure ends the stream with an error. An upstream error status fails a streaming request before any chunk is delivered, returned as a JSON 502, never as a stream that dies mid-flight. A client disconnect cancels the upstream request.

