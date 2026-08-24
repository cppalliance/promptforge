---
produced: 2026-08-23
title: Prior art for gateway admission control, fairness, and model lifecycle (Triton, vLLM, TGI, SGLang, LiteLLM, Ollama, llama-swap, llama.cpp, APF, DRR, VTC)
---

# Prior art: admission control, fairness, model lifecycle

Web research synthesis for the gateway redesign. Three threads: serving engines, gateways/model managers, fair queuing.

## Serving engines

- **NVIDIA Triton rate limiter** - the direct ancestor of the dominion concept. Named, counted resources (`--rate-limit-resource=name:count[:device]`); model instance groups declare resource requirements; `global: true` makes one shared pool across the whole system that heterogeneous models contend for; contention resolved by `priority` as a RELATIVE WEIGHT (priority 2 gets half the scheduling chances of 1), not strict rank. Model repository with `--model-control-mode=explicit` gives load/unload API. Docs: <https://github.com/triton-inference-server/server/blob/main/docs/user_guide/rate_limiter.md>
- **vLLM**: admission bounded by `max_num_seqs` (count) + `max_num_batched_tokens` (tokens); `--scheduling-policy {fcfs, priority}`; preemption when KV cache is exhausted. No per-tenant fairness in the engine; tenancy lives in the separate router layer. Documented failure mode: KV-pool over-admission causes preemption thrash - an admission-control failure, not compute shortage. Fix prescribed by practitioners: size concurrency from memory, queue overflow at the gateway. This validates "VRAM primary, concurrency secondary."
- **TGI**: `MAX_CONCURRENT_REQUESTS` hard gate with immediate "overloaded" (429-class) rejection - deliberate fail-fast as an alternative to queueing. `WAITING_SERVED_RATIO` trades decode pause for prefill aggressiveness. Self-benchmarks a token budget from measured free VRAM at startup.
- **llama.cpp llama-server**: `--parallel N` slots; requests beyond slots queue internally with no configurable depth, no fairness, no backpressure signal (overflow is HTTP 503). Unified KV (`-kvu`) shares the whole context across slots. **Router mode** (shipped 2026): coordinator spawns one single-model child per model, routes by the request's `model` field, auto-loads on first use, LRU-evicts past `--models-max`. Architecturally identical to the gateway's one-child-per-model design - upstream validation, including crash isolation as the stated reason for multi-process.
- **SGLang**: admission checks token budget + KV availability + `max_running_requests`. Cache-aware admission (`lpm` longest-prefix-match scheduling). Priority scheduling with a PREEMPTION THRESHOLD (incoming priority must exceed running by a margin, default 10) to prevent churn. `mem_fraction_static` reserves a VRAM fraction for KV up front. `schedule_conservativeness` is an admission-aggressiveness dial.

## Gateways and model managers

- **LiteLLM proxy**: `model_list` maps one public `model_name` to N deployments, each with `rpm`/`tpm`/`weight`/`order`/`max_parallel_requests`. Routing strategies (`simple-shuffle`, `least-busy`, `usage-based-routing-v2`, `latency-based`, `cost-based`) are DualCache-backed selectors. `order` gives failover tiers. Cooldown registry keyed by deployment id with TTL; `allowed_fails` policy. Per-key/team budgets via Redis. **Anti-pattern verified in source**: per-deployment `max_parallel_requests` is a process-local `asyncio.Semaphore` that waits forever - no bounded queue, no fairness, not multi-instance. The dominion design (named shared cap + bounded queue + fairness) is strictly better.
- **Ollama**: residency is a TTL cache with LRU eviction. `keep_alive` (duration / 0 = unload now / negative = pin), `OLLAMA_MAX_LOADED_MODELS`, `OLLAMA_NUM_PARALLEL` (per-model concurrency, orthogonal to residency), `OLLAMA_MAX_QUEUE`.
- **llama-swap** (mostlygeek): groups with `swap`/`exclusive`/`persistent` flags; TTL unload gated on an in-flight request drain (a slow generation is never severed); group-level swap mutex. They outgrew boolean groups into a declarative co-residency matrix with per-model `evict_costs` and a lowest-cost eviction solver - evidence of the complexity spiral behind the rejected demand-driven load/unload feature.
- **KServe ModelMesh**: large registered inventory with a small resident set; LRU eviction; each loaded model declares its own `maxConcurrency` at load time with the queue living in the router layer - validates per-model `parallel` with gateway-side queuing.
- **Ray Serve / BentoML**: the admission vocabulary - `max_ongoing_requests`/`max_concurrency` (hard cap) + `max_queued_requests` (bounded wait, then 503) + autoscaling target as three distinct knobs.
- **Gateway chaining** is an established two-tier pattern (Envoy AI Gateway for global auth/rate-limit, LiteLLM-tier for model-aware admission). Multi-instance shared admission uses Redis-style atomic counters (LiteLLM `parallel_request_limiter_v3` Lua scripts).

## Fair queuing

- **Convergent result across three communities**: DRR (networking, 1996), Kubernetes API Priority and Fairness (seat-seconds), and VTC ("Fairness in Serving Large Language Models", OSDI 2024) all land on the same mechanism: per-client service counters measured in a COST unit (bytes / seat-seconds / weighted tokens), dispatch the least-served client, lift counters for returning clients. Request-count fairness (plain round-robin) is the weakest option under mixed request sizes; FIFO is worst.
- **Kubernetes APF**: classify requests into priority levels with `nominalConcurrencyShares` (relative shares); concurrency measured in seats (big requests occupy multiple); shuffle sharding bounds a hot flow's blast radius; queue-then-429 with `limitResponse: Queue|Reject`.
- **Envoy**: per-upstream circuit breakers protect the backend, not the tenants; no fairness across callers in the core proxy. Tenant fairness must be built at the gateway queue.
- **VTC** (<https://www.usenix.org/system/files/osdi24-sheng.pdf>): fairness over weighted input+output tokens (output weighted higher); token-granularity counters solve unknown output lengths; 2x service-difference bound. Directly implementable at a gateway without touching backends.
- **Sarathi-Serve** (OSDI 2024): chunked prefill bounds slot-holding time - the same trick as DRR's quantum.
- Head-of-line/convoy mitigation toolkit: per-class queues (SEDA), fair queuing (fq_codel), bounded work quanta (chunking), bounded queues + 429 over unbounded FIFO.

## Vocabulary adopted for the gateway design

| Gateway concept | Borrowed from |
|---|---|
| `max_concurrency` + `max_queue` + reject-on-full | Ray Serve / BentoML / TGI / APF |
| Named shared budget bound by id | Triton rate limiter (`global` resources) |
| `policy = "reject"` (fail-fast) | TGI "overloaded" |
| Priority as relative weight (if classes ever ship) | Triton |
| Preemption threshold (if preemption ever ships) | SGLang |
| Per-model `parallel` with gateway-side queue | ModelMesh, llama.cpp slots |
| Profile hot-swap lifecycle semantics | llama-swap groups (minus demand loading) |
