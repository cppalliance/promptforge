// Bundle smoke test: dist/app.js boots against dist/index.html in jsdom -
// the dock mounts the Workshop tree and the agent-session panel, the
// status bar reads the boot push, and the agent panel opens its own
// /agents/ws socket beside the workshop /ws connection.
// Run: node test/smoke.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("the bundled app boots the workbench", async (ctx) => {
  const { document, sockets, statusText, failures } = ctx;

  if (!document.querySelector("#dock .workshop-tree")) {
    failures.push("the Workshop tree did not mount in the dock");
  }
  if (!document.querySelector("#dock .agent-panel")) {
    failures.push("the agent-session panel did not mount in the dock");
  }
  if (statusText.textContent !== "Ready") {
    failures.push(`the boot status push did not render: "${statusText.textContent}"`);
  }
  const agentSockets = sockets.filter((socket) => socket.url.endsWith("/agents/ws"));
  if (agentSockets.length !== 1) {
    failures.push(`the agent panel must open exactly one /agents/ws socket, saw ${agentSockets.length}`);
  }
  const workshopSockets = sockets.filter(
    (socket) => socket.url.endsWith("/ws") && !socket.url.endsWith("/agents/ws"),
  );
  if (workshopSockets.length !== 1) {
    failures.push(`the app must open exactly one /ws socket, saw ${workshopSockets.length}`);
  }
});
