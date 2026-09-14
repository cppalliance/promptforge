---
name: README and CI overhaul
overview: Replace the 798-line README with a crisp front page (badges, sizzle, cyberpunk banner strips, code snippets, build instructions). Move internals to DEVELOPMENT.md. Add CI workflow, LICENSE, and 6 section-banner images sliced from one generated composition.
todos:
  - id: license
    content: Add LICENSE file (Boost Software License 1.0)
    status: completed
  - id: ci
    content: Create .github/workflows/ci.yml (fmt + clippy + test)
    status: completed
  - id: migrate
    content: Migrate current README technical content to DEVELOPMENT.md (verify not stale)
    status: completed
  - id: image-gen
    content: Generate one tall cyberpunk composition (1024x1536) and slice into 6 horizontal banner strips
    status: completed
  - id: readme
    content: Write new README.md (badges, sizzle, banner strips per section, examples, build instructions, under 200 lines)
    status: completed
  - id: verify
    content: Confirm CI badge resolves, README renders clean, images display, links valid
    status: completed
isProject: false
---

# README and CI Overhaul

## Deliverables

1. **New README.md** - crisp, inverted-pyramid, badges + sizzle + banner strips + quick examples + install/build/run + links
2. **DEVELOPMENT.md** - verified technical content migrated from old README
3. **`.github/workflows/ci.yml`** - fmt, clippy, test on push/PR
4. **`LICENSE`** - Boost Software License 1.0
5. **6 banner strip images** - sliced from one generated cyberpunk composition

## README structure (top to bottom)

- Badges (CI status, license, Rust version)
- `# PromptForge`
- Sizzle paragraph (2-3 sentences: what it is, why it matters, key differentiator)
- Banner strip 1 (header - workbench with circuit boards and glowing data tablets)
- **What you get** - 4-5 bullet features with one emoji each
- Banner strip 2 (features - row of android heads, different glowing eyes)
- **Quick example** - a 15-line prompt showing the pattern (preamble + section + prose + epilog)
- Banner strip 3 (example - holographic code projection above a terminal)
- **Getting started** - prerequisites, clone, build, run gateway, run a prompt (shell blocks)
- Banner strip 4 (getting started - leather gloves gripping wrench and soldering iron, sparks)
- **How it works** - one paragraph + mermaid flow diagram
- Banner strip 5 (architecture - cross-section of robot torso showing internal wiring)
- **Project layout** - table of crates with one-line descriptions
- **Documentation** - links to user-guide.md, DEVELOPMENT.md, design-core.md
- Banner strip 6 (docs - filing cabinets and blueprint scrolls in neon-lit corridor)
- **Contributing** - one paragraph pointing to DEVELOPMENT.md
- **License** - BSL-1.0 with link to LICENSE file

Target: under 200 lines (excluding image tags). Dense, scannable, no glop.

## Banner image generation

**Source composition:** One tall portrait image (1024x1536, aspect 3:4). Dystopian cyberpunk workshop aesthetic - William Gibson Sprawl, dark industrial, grime and cables, neon accents (teal/cyan + orange sparks), humanoid robots, electronics, neural interfaces. Unified palette across the full height but distinct focal subjects per band.

**Reference images:** Use the existing promptforge robot-workshop image and the Falco how-to image (dark leather apron, android on slab, shelf of faces, wet concrete, forge-glow + cold neon) as style anchors.

**6 horizontal bands (top to bottom in the source):**

1. Overhead workbench - circuit boards, glowing data tablets, tangled cables
2. Shelf of android heads - each with different glowing eye color (tools metaphor)
3. Holographic code projection - green-on-black floating above a grimy terminal
4. Hands in leather gloves - wrench + soldering iron, orange sparks flying
5. Robot torso cross-section - internal wiring, connections, modular architecture
6. Dark corridor - filing cabinets, blueprint scrolls, cold neon strip lighting

**Slicer:** Python script using Pillow. Takes the source PNG, divides height by 6, crops each strip, saves as `images/banner-01.png` through `images/banner-06.png`. Each strip is 1024x256.

## GitHub Actions CI

```yaml
name: CI
on: [push, pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo test --workspace
```

Badge: `![CI](https://github.com/cppalliance/promptforge/actions/workflows/ci.yml/badge.svg)`

## DEVELOPMENT.md migration

Move from current README (verify each against current codebase):
- Gateway configuration (profiles, TOML examples, endpoint config)
- Store API triad table
- Tool configuration (web_search knobs)
- Architecture notes (crate relationships, boundary diagram)
- Model catalog and binding flow
- Development workflow (promptforge-dev, env vars)

Run as async subagent: read current README + source code, verify each item is still accurate, drop stale content, write DEVELOPMENT.md.

## LICENSE

Standard Boost Software License 1.0 text:

```
Boost Software License - Version 1.0 - August 17th, 2003

Permission is hereby granted, free of charge, to any person or organization
obtaining a copy of the software and accompanying documentation covered by
this license (the "Software") to use, reproduce, display, distribute,
execute, and transmit the Software, and to prepare derivative works of the
Software, and to permit third-parties to whom the Software is furnished to
do so, all subject to the following:

The copyright notices in the Software and this entire statement, including
the above license grant, this restriction and the following disclaimer,
must be included in all copies of the Software, in whole or in part, and
all derivative works of the Software, unless such copies or derivative
works are solely in the form of machine-executable object code generated by
a source language processor.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE, TITLE AND NON-INFRINGEMENT. IN NO EVENT
SHALL THE COPYRIGHT HOLDERS OR ANYONE DISTRIBUTING THE SOFTWARE BE LIABLE
FOR ANY DAMAGES OR OTHER LIABILITY, WHETHER IN CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

Copyright line: `Copyright (c) 2024-2026 The C++ Alliance, Inc. (https://cppalliance.org)`
