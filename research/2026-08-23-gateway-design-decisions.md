---
produced: 2026-08-23
title: PromptForge gateway redesign decisions - dominions, boot rules, profiles, normalization
---

# Gateway redesign decisions (2026-08-23)

Decisions made with Vinnie during the gateway redesign session. Each entry records the decision and its reason. There is exactly one plan: `dominion_refactor_ae33684d.plan.md` (titled "Gateway program").

## Boot and profiles

- The config path comes from the CLI positional argument or `PROMPTFORGE_GATEWAY_CONFIG` (CLI wins; neither set is a usage error). The profiles directory is always the sibling `profiles/` next to the config file. `--profiles-dir` is removed.
- Boot requires `--profile NAME`. Every gateway has at least one profile; the initial loaded set always has a name. There is no anonymous boot.
- The boot file is the catalog and infrastructure; it is never loaded as the runtime config directly. The named profile is loaded with include resolution (typically `include = ["../gateway.toml"]` plus an optional `models = [...]` allowlist) and becomes the initial config. The single-file user writes one minimal profile containing only the include line.
- Includes remain free-form (PROFILE-009): a profile may include a different file than the boot path or be self-contained. The boot path only anchors the profiles dir. At startup the gateway logs the resolved include chain, plus a note when the boot file is not in the chain (the likely-mistake case).
- `[server]` is owned by the boot file, enforced as a HARD ERROR at boot and at profile switch: the profile's merged `[server]` must equal the boot file's `[server]` exactly. Consequence: the socket and the gateway bearer key are fixed for the process lifetime; the key no longer rotates on switch.
- The `[server]` check is VALUE equality, not path provenance. Both sides are compared after `${VAR}` interpolation. Provenance tracking was rejected: the merge machinery does not record per-table origins, the boot file's own `[server]` may come from an include, and identical values are identical behavior.

## Secrets / env files

- Env files are cut to two: precedence is the process environment, then `<profile>.env`, then the boot file's sibling env file (e.g. `gateway.env`). dotenvy never overrides, so earlier wins. Included files' env files are no longer loaded.
- The config crate does no env-file loading: `Config::load` / `load_profile` interpolate from the process environment, and populating it is the binary's job. A library that reads a config must not mutate the process environment as a side effect.
- Reason for the cut: the architecture has exactly two meaningful levels (catalog and profile); defaults that are not secrets belong in TOML as literals; secrets cannot ship in shared env files; the deep chain's distinctive use case is empty.

## Dominions (admission control)

- `[[device]]`, `[[device.lane]]`, `[queue]`, `endpoint.concurrency`, `endpoint.device`, `local_model.device`, `local_model.lane` are all deleted. Verified fact motivating this: devices and lanes only supply a concurrency number; each endpoint or model gets its own queue with that number, so nothing is actually shared at runtime.
- New `[[dominion]]`: `id`, `kind` (remote|local), `max_concurrency` (absent = unlimited), `max_queue` (default 100), `policy` (queue|reject, default queue), `fair_scheduling` (default true), `vram_gb` (local only). Endpoints and local models bind by explicit id. One binding rule for everything.
- Dominions are truly shared: one runtime queue instance per dominion, `Arc`-shared by every bound endpoint. This is new behavior.
- No inline `endpoint.concurrency`: one way to cap. An endpoint without a dominion is unlimited.
- VRAM is the primary budget, concurrency the secondary one. A local dominion declares `vram_gb`; each local model declares a footprint estimate; validation rejects a set whose sum exceeds the budget. Runs at boot and at profile switch.
- `parallel` moves onto `[[local_model]]` (default 1) and feeds both the llama-server child's `--parallel` and the model's queue limit (preserving the existing invariant).
- Fairness: per-client round-robin (`X-PromptForge-Client`) is the only discipline. No abstraction layer for future disciplines: a one-variant enum is ceremony, and if DRR/token-cost fairness ever unparks, the change is contained in queue.rs. The fairness key is a self-asserted header; trusted-host callers only.

## Profile allowlist

- A profile may declare top-level `models = ["name", ...]`. After include-merge, `[[model]]` and `[[local_model]]` filter to the listed names - both remote and local, so the loaded set IS the catalog and `/v1/models` shows exactly the subset.
- Absent key means everything. Unknown names are hard validation errors. The list is stored on `Config` as `model_allowlist: Option<Vec<String>>` so `/admin/status` can report the selection.

## Rejected features (do not re-propose without a new use case)

- Demand-driven per-model load/unload (llama-swap-style): too much mechanism (swap serialization, in-flight drain, eviction policy, co-residency solving) for too little gain. The VRAM-constrained single-GPU operator manually chooses a profile per workload; a demand-loaded request would eat the model's cold-start latency anyway. "Model packs" already ships as profile hot-swapping (`POST /admin/switch-profile`).
- Anthropic inbound shim (accepting Anthropic-shaped requests from clients): two ingress dialects create more work for clients; the gateway's purpose is exactly one way of doing things. Clients are always OpenAI-shaped.
- Multi-instance shared admission (Redis-style counters): not needed; dominions are single-process by design. The LiteLLM proxy v3 pattern is recorded as prior art if a second gateway instance ever exists.

## Program shape

- Extract `promptforge-gateway-config` FIRST as a behavior-preserving move with the current structs, then reshape inside the extracted crate. Churn on the fresh public API is free because the gateway is the only consumer.
- Middle phase: embeddings (`POST /v1/embeddings`, OpenAI shape), classifiers (rerank convention shared by llama-server/vLLM/Jina), and streaming (SSE relay; permit held for the stream's lifetime; disconnect cancels upstream; per-chunk validation). Streaming is a requirement, not a deferral.
- Normalization is ALWAYS LAST: zero-dialect OpenAI normalization (see the prior-art file for the translator design). Until it completes, promptforge-core's client-side dialect machinery stays alive.
- Execution follows `tools-public/rulebooks/vibe-rulebook.md` and `tools-public/rulebooks/rust-rulebook.md`. Obsolescence sweeps run on the Verify schedule: whole-crate passes for code made obsolete, in their own commits.
