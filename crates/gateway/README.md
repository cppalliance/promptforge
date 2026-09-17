# crates/gateway/

`crates/gateway/` is the gateway family's private container - outside crates may name only `gateway-api` and `gateway-api-discovery` at the `crates/` root.

## gateway

The inference gateway itself (at `app/`): the always-on OpenAI-shaped service, the only process with an edge to an LLM backend. The workshop shell supervises it and the executor calls models through it. Depends on the whole family (api, api-discovery, config, logging, protocol, routing) plus shared-loopback and shared-progress, with local, stt, web-search, and config-ui behind cargo features.

## gateway-cloud-providers

The tiered cloud provider registry: per-provider fetch and normalization behind an injected reqwest client, plus the sheet-building binary. Its binary produces the provider model sheet the gateway serves at `/admin/cloud-models`. Depends on gateway-api.

## gateway-config

The gateway's configuration: single-file TOML, profile selection, and validation into a typed Config. Every gateway family crate reads its configuration through it. Depends on gateway-api.

## gateway-config-ui

The embedded config SPA served at `/config` behind the loopback wall. The gateway mounts it in config-ui builds. Depends on shared-loopback; build-ui is its build dependency.

## gateway-local

Local inference: GGUF provisioning, the artifact store, and the llama-server child lifecycle. The gateway runs local models through it in local builds, and the STT runtime shares its artifact store. Depends on gateway-config, gateway-protocol, gateway-routing, and shared-progress.

## gateway-logging

The logging sink: a bounded priority queue, rotation, and a worker-owned file writer behind `MakeWriter`. The gateway installs its subscriber at boot. No workspace dependencies.

## gateway-protocol

The wire protocol: OpenAI wire types, validation, and the Upstream abstraction with the shared HTTP client policy. Every crate that speaks to a backend goes through it. Depends on gateway-api and gateway-config; reqwest carries the transport.

## gateway-routing

The routing vocabulary: model and endpoint table entries and the dominion admission queues. The gateway resolves models and admits work through it. Depends on gateway-config and gateway-protocol.

## gateway-web-search

The Brave Search client: request validation and result post-processing behind WebSearchState. The gateway serves it at `/v1/tools/web_search` in web-search builds. Depends on gateway-config and gateway-protocol.

Note: `stt/` nests the speech-to-text subsystem one level deeper; `gateway-stt` is its only family-visible member.
