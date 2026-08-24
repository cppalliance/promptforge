---
produced: 2026-08-23
title: Verified findings about the current promptforge-gateway code that motivate the dominion refactor
---

# Verified current-code findings (pre-refactor)

Facts verified by reading the code on 2026-08-23, with file references. These motivate the dominion refactor; keep them citable so the plan stands alone.

## Devices and lanes share nothing at runtime

- `Routing::from_config` builds one `EndpointLane` per ENDPOINT (`src/routing.rs:105-108`). The concurrency number may come from `endpoint.concurrency` or the referenced `device.concurrency`, but two endpoints referencing the same remote device each get their own independent queue with the full value. The device is a source for a number, not a shared limit.
- A lane's concurrency value is used in two places (`src/local/mod.rs:82-105`): as the llama-server child's `--parallel N` argument and as that model's own queue limit (`EndpointLane::new(concurrency, &config.queue)` per model). Two models bound to the same lane each get their own queue with the full lane concurrency. Lanes do not partition anything at runtime.
- Consequence: the `[[device]]` / `[[device.lane]]` hierarchy is config structure without runtime semantics. Dominions replace it with actual sharing.

## VRAM is modeled nowhere

- No config field or check considers GPU memory. Co-residency failures (two local models whose combined footprint exceeds the card) surface as llama-server child OOMs at runtime. The refactor adds the load-time co-residency check.

## Profile switch semantics (src/lib.rs, admin_switch_profile)

- Switches are serialized by a dedicated mutex; the new config is fully built and validated off the live lock; commit is a single write-lock swap.
- Old local children are torn down before new ones start so two profiles never hold VRAM simultaneously. A start failure leaves the previous profile authenticated and remote-routable but without its local models (documented degraded state).
- The bearer key rotates on a successful switch today (`live.key = new_key`). This ends with the `[server]` boot-ownership rule (see the decisions file).
- The listener binds once in `run` before serving; a switched-to profile with a different `bind` is silently inert today. The `[server]` hard-error rule fixes this.

## Include and env machinery (src/profile.rs)

- `include = [...]` resolves depth-first, relative to the including file; absolute paths and `../` parents are deliberately permitted (PROFILE-009 policy comment). Cycle detection via canonicalized paths; depth cap 16 (MAX_INCLUDE_DEPTH).
- Merge is keyed-array by `id` (endpoint, device) or `name` (model, local_model); later definitions replace earlier ones wholesale; device lanes union by id (PROFILE-002); orphan `[[device.lane]]` blocks attach via an explicit `device` field. All of the lane machinery is deleted by the refactor.
- `load_env_chain` walks the include chain root-first loading sibling `.env` files via dotenvy (never overrides). Cut to two env files (profile + boot) by the 2026-08-23 ruling; the config crate becomes side-effect-free.
- `${VAR}` interpolation runs after TOML parsing, only on string leaves (CFG-007), so interpolated secrets can never corrupt document structure.

## Wire types are deliberately not shared (src/wire.rs module docs)

- The gateway and the executor define their own copies of the wire structs: "JSON is the contract and each side's struct is shaped by its role." The wire-types export question is parked, not forgotten.
- `tool_dialect` / `tools_mode` are catalog fields whose vocabulary is owned by promptforge-core (WIRE-005). Normalization phase N2 moves them gateway-internal.

## Auth

- `check_auth` + `secret_eq` (src/lib.rs): SHA-256 both tokens, then `subtle::ct_eq` - constant-time, length-hiding. litellm-rust independently converged on the same pattern (extractor + subtle + fail-closed).

## Config crate extraction dependencies (step 1 of the refactor)

- `config.rs` imports `QueueConfig` from `queue.rs` - QueueConfig must move to the config crate while the rest of queue.rs stays.
- `profile.rs` imports `default_promptforge_root` from `local/artifacts` - it must move too (profile.rs and cache-dir resolution need it).
- Error types that move: `ConfigError` (error.rs) and the `ConfigError`/`ConfigErrorKind` portion of api_error.rs. `GatewayError`, `ServeError`, `StartupError` stay in the gateway.
