// The wire contract of a chat submission against /ws: the app opens exactly
// one persistent socket on boot, the boot catalog auto-selects its first
// entry so the frame carries that model id, the frame is the OpenAI shape
// (type "chat", numeric id, messages, no stream flag), and the rendered
// reply's markdown link comes out of the sanitizer stamped target="_blank"
// rel="noopener". Run: node test/chat-wire-contract.mjs (after `npm run
// build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("a chat submission honors the /ws wire contract", async ({ chatSockets, history, submitChat, failures }) => {
  const request = await submitChat("Hello?");

  // Sanitized anchors open externally: the sanitizer must stamp
  // target="_blank" and rel="noopener" on every rendered link.
  const replyLink = history.querySelector('a[href="https://example.com/"]');
  if (!replyLink) {
    failures.push("the assistant reply's markdown link did not render as an anchor");
  } else {
    if (replyLink.getAttribute("target") !== "_blank") {
      failures.push('a sanitized anchor is missing target="_blank"');
    }
    if (replyLink.getAttribute("rel") !== "noopener") {
      failures.push('a sanitized anchor is missing rel="noopener"');
    }
  }

  const socket = chatSockets[0];
  if (!socket) {
    failures.push("no /ws socket was opened");
    return;
  }
  // The take-free boot opens one /ws socket and nothing else.
  if (chatSockets.length !== 1) {
    failures.push(`expected one persistent /ws socket, saw ${chatSockets.length}`);
  }
  if (!socket.url.endsWith("/ws")) failures.push(`chat socket opened the wrong URL: ${socket.url}`);
  if (!request) {
    failures.push("no chat frame was sent on the socket");
    return;
  }
  if (request.type !== "chat") failures.push("the frame is not a chat frame");
  if (typeof request.id !== "number") failures.push("chat frame carried no numeric id");
  if (request.model !== "test-model") {
    failures.push("the boot catalog's first entry was not auto-selected");
  }
  if ("stream" in request) failures.push("chat frame must not carry a stream flag");
  const first = request.messages?.[0];
  if (!first || first.role !== "user" || first.content !== "Hello?") {
    failures.push("chat frame messages are not the OpenAI shape");
  }
});
