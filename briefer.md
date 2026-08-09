---
name: briefer
description: Generate a report on an entity analyzed through the lens of Great Founder Theory.
promptforge: 1
max_tool_iterations: 24
---

# Briefer

```lua shared
models.always("writer",
    "A careful analysis model suited to structured reasoning and long-context review",
    { thinking = false, temperature = 0, context = 32768 })
tools.need("search", "Search the web and return a list of results.")
tools.need("fetch", "Fetch a URL and return its main content as markdown.")
```

Briefer Step 2: fanout evidence packet, then a packet-bound Report.

## Main

```lua
models.use("writer")

local results = fanout("### Web Search", "### Topics")

store.write("evidence.md", table.concat(results, "\n\n"))
-- no return: scalar prologue return stops fall-through into Report
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
1. Turn 1: your entire first response MUST be one `search` tool call (no prose, no headings). Query must include the Subject's exact name plus keywords for this Section. For Subject Profile, include filing keywords (EIN OR ProPublica OR "Form 990" OR registry OR "about us" OR leadership) so Profile can fill legal status and identifiers, not only the marketing homepage.
2. Turn 2: after search returns, `fetch` 1-2 best **https** URLs (prefer the Subject's own site, corporate filings or other primary records, and named press). If a hit is `http://`, fetch the `https://` form of the same host/path instead. Prefer one fetch when a single page is enough. Do not write the section yet.
3. Turn 3: only after at least one fetch attempt, output finished section markdown. No more tools unless every fetch failed - then one recovery search, then write. Keep the written section short enough to stay within a modest context budget.

Rules:
- Search hits are leads, not facts. Write ONLY from fetched page bodies.
- Every factual claim needs a short verbatim quote from a fetch body and that page's URL in parentheses after the claim.
- Never invent names, titles, years, officers, legal status, registry ids, addresses, CVE lists, quotes, or dates.
- If unsupported, write `UNKNOWN` for that field value only. Prefer thin truth over a complete-looking dossier.
- Never append `(UNKNOWN)` after a sourced claim. A field is either a sourced value or `UNKNOWN`, not both.
- UNKNOWN does not skip tools: you still must search and attempt at least one fetch before writing UNKNOWN.
- Entity check (HARD): claims must be about the Subject named above. Discard lookalike organizations and unrelated senses of the same word; use UNKNOWN rather than their facts.
- Founder/origin (HARD): only if a source explicitly says founder/co-founder/founded by. Officer, CEO, president, or director titles alone are not founder evidence.
- Output (HARD): plain markdown only. First line must be exactly `## ` plus the Section name. Do NOT wrap the section in triple-backtick fences. No preamble or process commentary. Do NOT embed other packet facets as nested mini-headings (`Subject Profile`, `Domain Primer`, etc.) inside this section - fill only the facet named in `## {{ item }}`.
- Scope: no morality, legality-as-verdict, product-quality, or "should it exist" judgments.
- Do not assume the Subject is a nonprofit, company, government body, or project. Infer entity type from sources; use identifiers and filings that fit that type.

What to fill by Section name:
- Subject Profile - legal or canonical name, founder/origin, founded/operational dates, legal/entity status, identifying numbers if sources give them, headquarters or home base, mission/charter (verbatim if available), key personnel, headcount/scale, structure/governance. Labeled fields. UNKNOWN if missing. When search surfaces an IRS/ProPublica/Charity Navigator or equivalent registry hit for the Subject, fetch it and copy EIN/legal status/officers into Profile (same facts may also appear in Public Record). Supporting nonprofits/foundations/sponsors are not the Subject: put their facts under structure/governance with their own names; do not use their staff counts as the Subject's headcount.
- Domain Primer - three to five numbered structural facts about this Subject's domain (not slogans). Each sourced.
- Domain Landscape - labeled fields: sector conditions; named peers or substitutes (external competing organizations/libraries only - not the Subject's own product codes, taxonomies, subsidiaries, or libraries it funds/maintains; when Wikipedia or comparison pages name alternatives in "See also"/equivalent lists, list those names+URLs as peers - do not write `UNKNOWN` while also naming them in parentheses); ecosystem position; market structure class (must pick one label or hybrid from: monopoly, duopoly, oligopoly, competitive, monopsony, oligopsony, government-controlled, two-sided platform, franchise/licensed); upstream/downstream dependencies; extralegal costs if any; natural disaster exposure for THIS Subject's own facilities/workforce/infrastructure only (UNKNOWN if sources do not name the Subject's sites - never a lookalike entity). If the Subject is a project/library collection, do not treat a supporting foundation as the same legal entity unless sources equate them - label each distinctly. For this section HARD: Turn-1 search must include the word Wikipedia; if any hit is a Wikipedia article about the Subject or its domain peers, fetch that https URL in Turn 2 before writing.
- Public Record - press, primary records, controversy, reputation about the Subject entity only. Prefer primary records and named press over directories and SEO pages. Do NOT restate Subject Profile / Primer / Landscape / Vulnerabilities as nested mini-sections inside Public Record - write prose or short bullets for this facet alone. If a founder/principal has separate personal political history, mention only when sources tie it to the Subject entity; otherwise leave it out of this section.
- Domain-Specific Vulnerabilities - organizational/sector risks that attach to THIS Subject (funding concentration, key-person, dependency on specific projects/standards processes, reputational or regulatory exposure named in sources). Prefer Subject-named security audits, vulnerability disclosure gaps, and supply-chain findings when fetched. Subject-named operational challenges from the Subject's own about/mission/news pages (resource gaps, volunteer maintenance risk, memory-safety exposure of dependents) count as sourced risks when quoted with URL - do not emit a bare UNKNOWN if those pages were fetched. Do not paste generic language-wide CVE scanner marketing (e.g. industry-wide "70% of vulnerabilities" pages that never name the Subject). UNKNOWN beats invented CVE filler.

Query hints (adapt to the Subject; do not invent answers): official about/mission/leadership pages; Subject + governance OR leadership OR "annual report" OR "corporate filings"; Subject + structure OR history when Profile is thin; for Landscape peers use Subject + (alternatives OR "compared to" OR vs OR competitors OR "similar libraries" OR "similar projects" OR FactSet OR Refinitiv OR "S&P Global" as fits the domain) and fetch the Subject Wikipedia page when it appears; for Vulnerabilities add Subject + (security audit OR OSTIF OR CVE OR "vulnerability disclosure" OR supply chain OR Talos OR NVD OR database OR "security advisory" OR funding OR sustainability OR "key person" OR concentration) and prefer fetching one Subject-owned about/mission/news page or a Subject-named product security advisory among the Turn-2 fetches so org risks can be quoted.

```lua
local searches = tools.calls["search"] or 0
local fetches = tools.calls["fetch"] or 0
if searches == 0 or fetches == 0 then
    return "## " .. tostring(item) .. "\n\nUNKNOWN\n\n(section incomplete: required search/fetch not performed)"
end
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

## Report

```lua
models.use("writer")
var.evidence = store.inject("evidence.md")
```

Evidence packet:

{{ var.evidence }}

You are a research analyst describing the Subject using ONLY the evidence packet above. NEVER invent or fabricate facts. If a field is missing or marked UNKNOWN in the packet, write UNKNOWN - do not fill gaps from memory.

Write a structured report with these sections (same names as the packet facets):

- Subject Profile - founding, leadership, structure, stated mission, identifiers/HQ/scale when present
- Domain Primer - three to five structural facts a reader needs for this domain
- Domain Landscape - sector conditions, named external peers, ecosystem position, market structure class, upstream/downstream dependencies, extralegal costs, natural disaster exposure for the Subject only
- Public Record - press, filings, controversy, reputation about the Subject entity
- Domain-Specific Vulnerabilities - Subject-linked organizational or product risks with sources

Plain markdown. No process commentary. No claims absent from the packet.

```lua
store.write("report.md", (reply:gsub("%s+$", "")) .. "\n\n*" .. sys.when .. " - " .. sys.model .. "*")
return "Report complete."
```

## Epilog

```lua
return "Report complete."
```
