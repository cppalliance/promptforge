# promptforge-webfetch

This crate fetches and converts one known URL: it retrieves the page and returns its main content as markdown. That is the whole scope.

- Tool vocabulary (`Tool`, `ToolId`, `ToolOutput`, `ToolError`, and their kinds) comes from `promptforge-tools`. This crate does not depend on `promptforge-core`.
- No search, crawling, or discovery: the caller supplies the URL.
- SSRF defenses (address pinning, redirect policy, bounded bodies) stay in this crate and apply to every fetch.
