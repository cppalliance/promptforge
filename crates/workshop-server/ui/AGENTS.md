# workshop-ui

Embedded TypeScript UI under `crates/workshop-server/ui/`: workshop chrome, agent controls, and the SPA served by workshop-server.

- Imports flow from `ui` through `services` to `base`, never in reverse. `main.ts` is the composition root and nothing imports it.
- Shared state lives in a service with a change emitter, constructed once at the composition root and passed through constructors. Do not store application state in mutable module globals.
- Workshop agent controls target Cursor's workspace-sidebar agent surface, not the Glass Agents Window or editor-tab agent. Workbench chrome uses VS Code theme tokens, and Cursor-native controls use the shared Cursor design tokens.
