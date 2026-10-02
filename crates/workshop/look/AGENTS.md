# look

`@workshop/look`, the Workshop family's visual layer. It was forked from `crates/shared-ui` on 2026-09-27 and has no sync obligation: changes here never need to reach `shared-ui`, and `shared-ui` changes never need to reach here.

- Components stay app-agnostic. They own no timer or document listener that outlives their element, and consumers own integration lifecycle.
- Focus uses the matching state background or opacity behavior, not an invented outline, ring, or focus-like shadow.
- Keep third-party derivation notices and their source comments intact.
