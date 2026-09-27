# shared-ui

Shared TypeScript and CSS primitives for the Gateway configuration UI and Workshop UI.

- `tokens.css` owns the shared design-token vocabulary consumed by both product UIs.
- This package is a base-layer dependency and never imports from either product UI.
- A behavioral primitive belongs here only when both product UIs consume it. Product-specific controls and integrations stay with their products.
- Shared components stay app-agnostic. They own no timer or document listener that outlives their element, and consumers own integration lifecycle.
- Shared component defaults remain overridable by product UI layers. Focus uses the matching state background or opacity behavior, not an invented outline, ring, or focus-like shadow.
- Keep third-party derivation notices and their source comments intact.
