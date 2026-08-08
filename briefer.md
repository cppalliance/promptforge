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
    { thinking = false, temperature = 0 })
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

Search the web up to 3 times: find out {{ item }} about the Subject, and write your findings as a structured output. Output only the finished evidence document. No preamble, no process commentary.

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

You are a research analyist describing an entity. Write the following information as a structured report. ALWAYS use evidence from the packet. NEVER invent or fabricate facts.

- Subject Profile - founding, leadership, structure, stated mission
- Domain Primer - three to five structural facts a reader needs to understand this domain
- Domain Landscape - sector conditions, competitors, ecosystem position, market structure classification (monopoly, duopoly, oligopoly, competitive, monopsony, oligopsony, government-controlled, two-sided platform, franchise/licensed; note hybrids), upstream and downstream dependencies, extralegal operating costs (corruption, organized crime, extortion, informal payments, contract enforcement failure, IP theft; note jurisdictions and segments), natural disaster exposure (earthquake, hurricane, flood, drought, wildfire, tsunami; note facilities and regions)
- Public Record - press, analysis, filings, controversy, reputation
- Domain-Specific Vulnerabilities - sector-specific risks with sources

```lua
store.write("report.md", reply)
```

## Epilog

```
return "Report complete."
```
