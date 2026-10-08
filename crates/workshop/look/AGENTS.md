# look

`@workshop/look`, the Workshop family's visual layer. It was forked from `crates/shared-ui` on 2026-09-27 and has no sync obligation: changes here never need to reach `shared-ui`, and `shared-ui` changes never need to reach here.

- Components stay app-agnostic. They own no timer or document listener that outlives their element, and consumers own integration lifecycle.
- Focus follows Cursor: list rows, tree rows, menu rows, and inputs draw a 1px `--cursor-stroke-focused` outline at offset -1px (`--focus-outline`), and everything else uses the matching state background or opacity behavior. Do not invent a ring or a focus-like shadow.
- Keep third-party derivation notices and their source comments intact.
