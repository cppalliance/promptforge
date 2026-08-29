// Disconnect recovery: a dropped /ws socket resets the bar to its
// reconnecting state, and the backoff opens a replacement socket (the
// first retry waits one second).
// Run: node test/disconnect-recovery.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("a dropped socket reconnects", async ({ chatSockets, statusText, sleep, failures }) => {
  const persistentSocket = chatSockets.find((socket) => socket.url.endsWith("/ws"));
  if (!persistentSocket) {
    failures.push("no /ws socket was opened at boot");
    return;
  }
  const socketCount = chatSockets.length;
  persistentSocket.onclose?.();
  if (statusText.textContent !== "Reconnecting...") {
    failures.push("a dropped /ws socket did not reset the status bar");
  }
  const reconnectDeadline = Date.now() + 5000;
  while (chatSockets.length === socketCount && Date.now() < reconnectDeadline) {
    await sleep(50);
  }
  if (chatSockets.length === socketCount) {
    failures.push("no replacement /ws socket opened after the reconnect backoff");
  } else if (!chatSockets[chatSockets.length - 1].url.endsWith("/ws")) {
    failures.push("the reconnect opened a socket that is not /ws");
  }
});
