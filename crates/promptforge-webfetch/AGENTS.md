# promptforge-webfetch

This crate fetches and converts one caller-supplied URL into Markdown.

- The caller defines URL scope. This provider does not search, crawl, or discover targets.
- Every initial request and redirect hop uses the guarded resolver, address pinning, redirect policy, and bounded body handling. No hop may bypass SSRF validation.
- Tool vocabulary comes from `promptforge-tools`. This provider does not depend on Core.
