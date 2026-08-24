# Design Choices: promptforge-wb stage 1

Running log of design choices for the PromptForge Workbench stage 1 build. Each entry states the choice, the evidence behind it, and the cost it carries.

## 1. Two crates, promptforge-wb-server and promptforge-wb, as workspace members

- Choice: the workbench is split into an HTTP server binary (`promptforge-wb-server`) and a desktop window shell binary (`promptforge-wb`), both globbed into the promptforge workspace as `crates/*` members.
- Evidence: the workspace already ships one binary per concern (gateway, mcp-server, cli), and the workbench needs a local HTTP API the shell and a browser can both reach, which is two processes with different dependency stacks.
- Cost: two binaries to build, run, and eventually package together; the shell cannot call server internals directly and must go through the HTTP API.

## 2. axum for the HTTP server

- Choice: the workbench server is built on axum 0.8 with tower for testing.
- Evidence: axum is the ecosystem default HTTP server in the rust rulebook, is already a workspace dependency used by promptforge-gateway, and its Router is directly testable with tower's `ServiceExt::oneshot` without binding a port.
- Cost: pulls the tokio/tower stack into the workbench; handlers are async even where the work is synchronous.

## 3. Default bind 127.0.0.1:7910

- Choice: the server binds to 127.0.0.1:7910 when no override is given.
- Evidence: the workbench is a local companion to the desktop shell, so loopback-only is the safe default; 7910 is an unassigned high port that does not collide with common development servers.
- Cost: the port is fixed rather than discovered, so a collision fails the bind instead of picking another port; remote access requires an explicit future override.

## 4. wry/tao deferred to the shell step

- Choice: `promptforge-wb` is an empty binary skeleton; the wry/tao window and its dependency tree are not added yet.
- Evidence: wry/tao pull in large platform GUI stacks (WebView2, gtk) that dominate build time, and nothing in the scaffolding step needs a window; keeping them out lets the workspace build fast while the server takes shape.
- Cost: the shell cannot be run end to end until the shell step lands, and the wry/tao integration risk is concentrated there instead of spread out.
