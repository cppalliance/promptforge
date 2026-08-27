// Where a take's text lands in the composer: an interim inserts at the
// cursor and leaves the cursor after the insert, the final replaces the
// interim text in place, a selection is replaced outright, and a second
// take composes at the cursor position the first take left behind.
// (readOnly during the take is pinned by test/voice-readonly-take.mjs.)
// Run: node test/voice-cursor-insert.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("voice takes insert at the cursor", async ({ input, startTake, failures }) => {
  // Insert-at-cursor: with "ab" in the textarea and the cursor between a
  // and b, record an interim "X"; assert "aXb" and the cursor sits after X.
  input.value = "ab";
  input.setSelectionRange(1, 1);
  let takeSocket = await startTake();
  if (!takeSocket) {
    failures.push("insert-at-cursor: mic click did not open a /voice socket");
    return;
  }
  takeSocket.onmessage({ data: JSON.stringify({ type: "interim", committed: "X", tentative: "" }) });
  if (input.value !== "aXb") {
    failures.push(`insert-at-cursor: expected "aXb", got "${input.value}"`);
  }
  if (input.selectionStart !== 2) {
    failures.push(`insert-at-cursor: cursor expected at 2, got ${input.selectionStart}`);
  }
  takeSocket.onmessage({ data: JSON.stringify({ type: "final", text: "Y" }) });
  if (input.value !== "aYb") {
    failures.push(`insert-at-cursor final: expected "aYb", got "${input.value}"`);
  }
  // The scripted socket doesn't auto-fire onclose; trigger it so voice
  // state resets before the next scenario.
  takeSocket.onclose?.();

  // Selection replacement: with "ab" fully selected, record an interim "X";
  // assert the box shows "X".
  input.value = "ab";
  input.setSelectionRange(0, 2);
  takeSocket = await startTake();
  if (!takeSocket) {
    failures.push("selection-replace: mic click did not open a /voice socket");
    return;
  }
  takeSocket.onmessage({ data: JSON.stringify({ type: "interim", committed: "X", tentative: "" }) });
  if (input.value !== "X") {
    failures.push(`selection-replace: expected "X", got "${input.value}"`);
  }
  takeSocket.onclose?.();

  // Multi-take composition: first take inserts " hello" at the end, second
  // take inserts " world" at the new cursor position (after "hello").
  input.value = "start";
  input.setSelectionRange(5, 5);
  const take1Socket = await startTake();
  if (!take1Socket) {
    failures.push("multi-take: first mic click did not open a /voice socket");
    return;
  }
  take1Socket.onmessage({ data: JSON.stringify({ type: "final", text: " hello" }) });
  if (input.value !== "start hello") {
    failures.push(`multi-take: after take 1 expected "start hello", got "${input.value}"`);
  }
  take1Socket.onclose?.();
  const take2Socket = await startTake();
  if (!take2Socket) {
    failures.push("multi-take: second mic click did not open a /voice socket");
    return;
  }
  take2Socket.onmessage({ data: JSON.stringify({ type: "final", text: " world" }) });
  if (input.value !== "start hello world") {
    failures.push(`multi-take: after take 2 expected "start hello world", got "${input.value}"`);
  }
  if (input.readOnly) {
    failures.push("multi-take: readOnly not cleared after second take");
  }
  take2Socket.onclose?.();
});
