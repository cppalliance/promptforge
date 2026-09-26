# Contributing to the guide

The guide has three documentation sets, one per audience: `src/gateway/`, `src/workshop/`, and `src/language/`. Each set builds as its own book. Chapters inside a set start with a numeric prefix that fixes the reading order.

## Ownership

- Each book's `SUMMARY.md` and overview `index.md` belong to the assembler, which writes them only into the staged books under `target/site-books/`. Do not add them to `src/`; build the books with `cargo xtask site --books-only`.
- Chapters and `src/introduction.md` are hand-edited. Fixes land directly in the file.

## Freshness

There is no freshness gate. The `site.yml` workflow only builds and deploys the checked-in books. No generator exists for these sets yet, so update a chapter by hand when its sources change.

## House rules

- No em-dash and no double-dash. Use a single dash.
- Open every code fence with four backticks.
- Keep each paragraph on one line.
