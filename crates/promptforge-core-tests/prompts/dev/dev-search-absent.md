---
name: dev_search_absent
description: Dev-loop verification prompt that needs web search to exercise the absent-capability bind failure
promptforge: 1
---

# Dev Search Absent

```lua prompt
tools.need("search", "Search the web and return a list of results (title, url, description).")
```

This prompt declares a web-search capability. Without `PROMPTFORGE_TOKEN` the
dev registry carries only `web_fetch`, so binding must fail loudly as an
absent capability before any model call.

## Search

```lua
tools.add("search")
```

Search the web for PromptForge and summarize the top result in one sentence.
