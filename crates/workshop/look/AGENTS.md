# look

`@workshop/look`, the Workshop family's visual layer. It was forked from `crates/shared-ui` on 2026-09-27 and has no sync obligation: changes here never need to reach `shared-ui`, and `shared-ui` changes never need to reach here.

- Visual things used across the family, or intended to be, belong here. App-wide mechanics (commands, menus, registries) go to `@workshop/platform`, not here.
- This package is the family's base layer. Its sources import only their own files and `lucide`; `test/boundary.mjs` fails on anything else, including `shared-ui`, the Workshop UI, and other `@workshop/*` packages.
- Token tiers: the palette (`tokens.css`), sizes (`sizes.css`), and semantic aliases (`semantic.css`) live here. Component tokens live with their components; the Workshop UI's overrides stay in `ui/src/tokens/component.css`. Colors, and every `look` component, are themed in `tokens.css`, while `sizes.css` and `semantic.css` hold the size scale and the semantic aliases that Workshop UI component CSS reads as `--ws-*` tokens.
- Two component-token blocks still sit in `tokens.css`: the agent-window block leaves when the agent view becomes its own crate, and the title bar block (`--titlebar-*`) leaves when the window frame is extracted.
- Components stay app-agnostic. They own no timer or document listener that outlives their element, and consumers own integration lifecycle.
- Component defaults remain overridable by consumer layers. Focus uses the matching state background or opacity behavior, not an invented outline, ring, or focus-like shadow.
- Keep third-party derivation notices and their source comments intact.
