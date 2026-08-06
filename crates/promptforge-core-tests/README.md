# `promptforge-core-tests`

This unpublished workspace binary crate owns complete, author-shaped PromptForge files that exercise the public `promptforge-core` parse, deterministic declaration-binding, and offline execution APIs. It complements the core crate's narrow inline grammar and lifecycle tests without creating a second parser or fixture discovery mechanism.

Run the offline fixture harness with:

```text
cargo test -p promptforge-core-tests
```

Every fixture is registered by name with an explicit `include_str!` in `src/suite.rs`, so adding a file without adding its expected public structure, error contract, or execution assertion cannot silently expand the suite. Valid fixtures assert parsed frontmatter, titles, shared Lua, section trees, and phase boundaries. Invalid fixtures assert the public error variant and a stable message fragment. Tool-free execution fixtures run shared declarations through public `bind_tool_declarations` with a deterministic no-tools resolver, then use the public parsed-`Prompt` compatibility input to `execute::run`. They assert exact Lua checkpoint sequences, scalar preamble early return, and store-backed fall-through across isolated sections using stable execution IDs and a mutex-backed observer. A concurrent regression partitions one shared recording by execution ID.

The shipped-prompt smoke test discovers every Markdown file under the repository's `prompts/` directory, rejects concrete tool names, and requires each file to parse. The MCP server's owner tests retain the shipped-prompt semantic binding assertion against its complete live registry.

The harness does not construct a picker, load a model, provision production tools, start `llama-server`, make gateway or generation-model calls, or require credentials. Artifact synchronization regressions do start copies of the test executable against loopback HTTP and temporary caches. A cold workspace build may still acquire assets required by core's existing transitive picker dependency. The binary entry point is intentionally inert until the explicit real-model runner is added.

## Pinned real-model artifacts

The crate now contains the provisioner that the later runner will call. It caches all files under repository-root `.model-cache/`, which is gitignored:

- Official llama.cpp release `b10082` CPU archives for Windows x86-64 and arm64, Ubuntu x86-64 and arm64, and macOS x86-64 and arm64. Every platform entry commits its official GitHub release URL and release API SHA-256.
- Official `Qwen/Qwen3-0.6B-GGUF` file `Qwen3-0.6B-Q8_0.gguf`, approximately 639 MB, from Hugging Face with SHA-256 `9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031`.

Downloads are streamed to `.part`, synchronized, verified, and renamed only after the digest matches. A per-artifact file lock synchronizes threads and processes, and every waiter rechecks the cache after acquiring its lock. Cache paths are confined below `.model-cache/`; symlink and Windows reparse-point components are rejected. Server archives reject portable traversal spellings, NTFS alternate-data-stream names, links, and unsupported entry types before extracting into a confined staging directory. Install tree markers cover file contents and Unix permission modes, so lost executable permission is detected and repaired from the cached archive. Cache hits make no request. Stale partials and corrupt model files, archives, executables, extracted dependencies, or permissions are rejected and repaired. Unsupported operating-system and architecture pairs fail before any request.

Ordinary tests never call the production URLs. They run against loopback fake HTTP servers, temporary caches, and tiny generated ZIP and tar.gz archives, covering cache hits, digest failures, interrupted responses, stale partial repair, corrupt file and permission repair, traversal and link rejection, cache confinement, atomic install, and thread/process contention. No test launches `llama-server` or downloads the real model yet.
