# Dominions and Queues

This chapter teaches you dominions: named compute pools that cap concurrency, park or reject excess callers, and schedule waiting clients fairly. Dominions are how you keep one busy model from starving the rest.

## Declare a dominion

A dominion is a `[[dominion]]` entry:

````
[[dominion]]
id = "pool-r"
kind = "remote"
max_concurrency = 4
max_queue = 100
policy = "queue"
fair_scheduling = true
````

Each entry has an `id`, a `kind` of `remote` or `local`, a `max_concurrency`, a `max_queue` defaulting to 100, a `policy` of `queue` or `reject` defaulting to `queue`, a `fair_scheduling` flag defaulting to true, and a `vram_gb` budget for local pools.

Bind an endpoint or a local model to a dominion by name:

````
[[endpoint]]
id = "openai"
protocol = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"
dominion = "pool-r"
````

Endpoints bind to remote dominions, and local models bind to local dominions. A wrong-kind or undefined binding is rejected. An endpoint or local model without a dominion binding is unlimited; it behaves as when no cap is set at all.

## Budget VRAM

A local dominion can carry a `vram_gb` budget, and each profile's selected models must fit within it:

````
[[dominion]]
id = "gpu0"
kind = "local"
max_concurrency = 2
vram_gb = 24
````

An overflow fails validation with an error naming the dominion and the excess. Fractional estimates such as 1.22 are accepted. Zero, negative, NaN, and infinite estimates fail.

## Choose a full-capacity policy

The default `queue` policy parks callers up to the depth limit. The `reject` policy turns the caller away immediately, and the gateway answers 429:

````
policy = "reject"
````

You can distinguish admission failures by status code. A full waiting queue answers 503 with code `queue_full`. A fail-fast rejection answers 429 with code `queue_rejected`. A queue torn down while the caller waited reports the queue as unavailable. A profile switch that cancels an in-flight request gets its own error: the gateway answers 503 with code `profile_switch` and the message "request cancelled for profile switch", distinct from `queue_full` and `queue_rejected`.

## Schedule fairly

Turn on fair scheduling so waiting callers are served in per-client round-robin order, keyed by the `X-PromptForge-Client` request header:

````
fair_scheduling = true
````

The header is a self-asserted hint. Values over 64 bytes or outside the alphanumeric, dash, underscore, dot, and colon charset fold into the shared `default` bucket. At most 32 distinct client labels are tracked.

## How slots behave

A streaming request holds its dominion concurrency slot for the stream's whole lifetime, so a second request waits until the first stream ends. A cancelled queued request frees its waiting slot, and capacity recovers without a restart.

