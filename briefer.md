---
name: briefer
description: Generate a report on an entity analyzed through the lens of Great Founder Theory.
promptforge: 1
max_tool_iterations: 100
---

# Briefer

```lua
models.always("writer",
    "A careful analysis model suited to structured reasoning and long-context review",
    { thinking = false, temperature = 0, context = 65536 })
tools.need("search", "Search the web and return a list of results.")
tools.need("fetch", "Fetch a URL and return its main content as markdown.")
```

This is a promptforge port of the Briefer tool.

## Main

```lua
models.use("writer")

local results = fanout("### Web Search", "### Topics")

store.write("evidence.md", table.concat(results, "\n"))
```

### Web Search

```lua
models.use("writer")
tools.add("search", "fetch")
```

Subject: {{ args }}
Topic: {{ item }}

You are assembling one evidence section for the Topic. Follow this protocol exactly:

1. First response: call `search`. Do not write evidence yet.
2. After search returns, call `fetch` on 1-2 of the most relevant pages before writing anything final.
3. Search hits are leads, not facts. Write evidence ONLY from fetched page bodies. Every claim must include a short verbatim quote copied from a fetch body and that page's URL.
4. Never invent names, titles, founding years, board members, legal status, addresses, CVE lists, quotes, or dates. Do not stamp a research date unless a source states it.
5. If fetch text does not support a subclaim, write `UNKNOWN` for that subclaim.
6. Prefer thin truthful packets over complete-looking dossiers.
7. When tools are done, output only the finished evidence markdown for this Topic. No preamble, no process commentary.

```lua
assert(tools.calls["search"] > 0)
assert(tools.calls["fetch"] > 0)
return reply
```

### Topics

* profile
* founder
* stated mission
* organizational structure
* age, scale
* domain
* sector conditions
* press reports
* natural disaster exposure
* vulnerabilities

## Report

```lua
var.evidence = store.inject("evidence.md")
```

Evidence packet:

{{ var.evidence }}

You are a research analyst describing an entity. Write the following information as a structured report. ALWAYS use evidence from the packet. NEVER invent or fabricate facts.

- Subject Profile - founding, leadership, structure, stated mission
- Domain Primer - three to five structural facts a reader needs to understand this domain
- Domain Landscape - sector conditions, competitors, ecosystem position, market structure classification (monopoly, duopoly, oligopoly, competitive, monopsony, oligopsony, government-controlled, two-sided platform, franchise/licensed; note hybrids), upstream and downstream dependencies, extralegal operating costs (corruption, organized crime, extortion, informal payments, contract enforcement failure, IP theft; note jurisdictions and segments), natural disaster exposure (earthquake, hurricane, flood, drought, wildfire, tsunami; note facilities and regions)
- Public Record - press, analysis, filings, controversy, reputation
- Domain-Specific Vulnerabilities - sector-specific risks with sources

```lua
store.write("report.md", (reply:gsub("%s+$", "")) .. "\n\n*" .. sys.when .. " - " .. sys.model .. "*")
```

## Epilog

```
return "Report complete."
```
