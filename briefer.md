---
name: briefer
description: Generate a report on an entity analyzed through the lens of Great Founder Theory.
promptforge: 1
max_tool_iterations: 12
---

# Briefer

```lua
models.always("writer",
    "A careful analysis model suited to structured reasoning and long-context review",
    { thinking = false, temperature = 0, context = 32768 })
tools.need("search", "Search the web and return a list of results.")
tools.need("fetch", "Fetch a URL and return its main content as markdown.")
```

Evidence-only recon port of the Briefer Step 2 packet (Report restored later).

## Main

```lua
models.use("writer")

local results = fanout("### Web Search", "### Topics")

store.write("evidence.md", table.concat(results, "\n\n"))
return "Evidence complete."
```

### Web Search

```lua
models.use("writer")
tools.add("search", "fetch")
```

Subject: {{ args }}
Section: {{ item }}

You are writing ONE section of an evidence packet about the Subject. Emit heading `## {{ item }}` as the first line, then fill that facet.

Turn budget (HARD):
1. Turn 1: your entire first response MUST be one `search` tool call (no prose, no headings). Query must include the Subject's exact name plus keywords for this Section.
2. Turn 2: after search returns, `fetch` 1-2 best URLs (prefer the Subject's own site, corporate filings or other primary records, and named press). Prefer one fetch when a single page is enough. Do not write the section yet.
3. Turn 3: only after at least one fetch attempt, output finished section markdown. No more tools unless every fetch failed - then one recovery search, then write. Keep the written section short enough to stay within a modest context budget.

Rules:
- Search hits are leads, not facts. Write ONLY from fetched page bodies.
- Every factual claim needs a short verbatim quote from a fetch body and that page's URL in parentheses after the claim.
- Never invent names, titles, years, officers, legal status, registry ids, addresses, CVE lists, quotes, or dates.
- If unsupported, write `UNKNOWN` for that field. Prefer thin truth over a complete-looking dossier.
- UNKNOWN does not skip tools: you still must search and attempt at least one fetch before writing UNKNOWN.
- Entity check (HARD): claims must be about the Subject named above. Discard lookalike organizations; use UNKNOWN rather than their facts.
- Output (HARD): plain markdown only. First line must be exactly `## ` plus the Section name. Do NOT wrap the section in triple-backtick fences. No preamble or process commentary.
- Scope: no morality, legality-as-verdict, product-quality, or "should it exist" judgments.
- Do not assume the Subject is a nonprofit, company, government body, or project. Infer entity type from sources; use identifiers and filings that fit that type.

What to fill by Section name:
- Subject Profile - legal or canonical name, founder/origin, founded/operational dates, legal/entity status, identifying numbers if sources give them, headquarters or home base, mission/charter (verbatim if available), key personnel, headcount/scale, structure/governance. Labeled fields. UNKNOWN if missing.
- Domain Primer - three to five numbered structural facts about this Subject's domain (not slogans). Each sourced.
- Domain Landscape - sector conditions; peers; ecosystem position; market structure class (monopoly, duopoly, oligopoly, competitive, monopsony, oligopsony, government-controlled, two-sided platform, franchise/licensed; note hybrids); upstream/downstream dependencies; extralegal costs if any; natural disaster exposure for THIS Subject's facilities/workforce/infrastructure only (never a different entity).
- Public Record - press, primary records, controversy, reputation. Prefer primary records and named press over directories and SEO pages.
- Domain-Specific Vulnerabilities - sector risks for THIS Subject with sources. No generic filler without a Subject link.

Query hints (adapt to the Subject; do not invent answers): official about/mission/leadership pages; Subject + governance OR leadership OR "annual report" OR "corporate filings"; Subject + structure OR history when Profile is thin.

```lua
assert(tools.calls["search"] > 0)
assert(tools.calls["fetch"] > 0)
local text = reply:gsub("^%s*```[Mm][Aa][Rr][Kk][Dd][Oo][Ww][Nn]%s*\n", ""):gsub("^%s*```%s*\n", "")
text = text:gsub("\n```%s*$", ""):gsub("%s+$", "")
return text
```

### Topics

* Subject Profile
* Domain Primer
* Domain Landscape
* Public Record
* Domain-Specific Vulnerabilities

## Epilog

```lua
return "Evidence complete."
```
