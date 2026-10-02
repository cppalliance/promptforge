# workshop-ui

- Shared state lives in a service with a change emitter, constructed once at the composition root and passed through constructors. Do not store application state in mutable module globals.
- Workshop agent controls target Cursor's workspace-sidebar agent surface, not the Glass Agents Window or editor-tab agent.
- Never `export *` from a feature `index.ts`; importers point at source files directly.
