// Disconnect recovery: a dropped /ws socket resets the bar to its
// reconnecting state, and the backoff opens a replacement socket (the
// first retry waits one second).
// Run: node test/disconnect-recovery.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("a dropped socket reconnects", async ({ sockets, wsSocket, statusText, sleep, failures }) => {
  const persistentSocket = wsSocket();
  if (!persistentSocket) {
    failures.push("no /ws socket was opened at boot");
    return;
  }
  const socketCount = sockets.length;
  persistentSocket.onclose?.();
  if (statusText.textContent !== "Reconnecting...") {
    failures.push("a dropped /ws socket did not reset the status bar");
  }
  const reconnectDeadline = Date.now() + 5000;
  while (sockets.length === socketCount && Date.now() < reconnectDeadline) {
    await sleep(50);
  }
  if (sockets.length === socketCount) {
    failures.push("no replacement /ws socket opened after the reconnect backoff");
  } else if (wsSocket() === persistentSocket) {
    failures.push("the reconnect did not open a fresh /ws socket");
  }
});
