# crates/promptforge/

`crates/promptforge/` is the promptforge family's private container - outside crates reach the family only through `promptforge-api-runtime` and `promptforge-api-types` at the `crates/` root.

## promptforge-lua

The sandboxed Lua runtime: the section VM, the coroutine protocol, and the host surface over a restricted mlua VM. The runtime executes every section's Lua block through it, and the parser splits fences for it. Depends on promptforge-api-types, promptforge-model-client, and promptforge-store.

## promptforge-parser

The prompt document parser: YAML frontmatter, the H1 and nested-section tree, and exact lua fence splitting. It is the runtime's first stop for every prompt file. Depends on promptforge-api-types and promptforge-lua.

## promptforge-store

Run-scoped virtual files: the Store facade over the shared VFS, exposed as `vfs.store(&access)`. Lua sections and the model share run files through it. Depends on promptforge-vfs and shared-vfs.

## promptforge-vfs

The promptforge VFS policy: the `/_promptforge` mount layout, the stock empty handle, and the mode policy. It is the policy layer every family VFS consumer mounts through. Depends on shared-vfs.

## promptforge-model-client

The gateway model client: OpenAI-shaped chat-completions transport, wire types, and the model catalog and binding vocabulary. The runtime and the Lua host call models through it. Depends on promptforge-api-types; reqwest carries the transport.

## promptforge-web

The web capability pack: the fetch and search tools in one bundle. The runtime mounts it as the `promptforge/web` capability. Depends on promptforge-api-types, promptforge-webfetch, and promptforge-web-search.

## promptforge-webfetch

The `web_fetch` tool: fetches a URL and returns its main content as markdown, behind the SSRF boundary. Packed into the web capability by promptforge-web. Depends on promptforge-api-types; reqwest, readabilityrs, and htmd carry the fetch and extraction.

## promptforge-web-search

The `web_search` tool: proxies a search query through the gateway so the vendor credential never leaves the server. Used by the runtime directly and by the web capability pack. Depends on promptforge-api-types; reqwest carries the transport.
