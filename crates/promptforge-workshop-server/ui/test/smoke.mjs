// Minimal end-to-end core: boots the real bundle (dist/index.html +
// dist/app.js) through the shared workbench fixture and proves the two
// full-stack paths work - one chat round-trip (frame out on /ws, scripted
// reply rendered into the history) and one voice take (interim lands in
// the composer, the final replaces it). Per-feature slices of the old
// monolithic smoke test live in the sibling tests under test/ (mount
// structure, title bar, wire contract, status bar, voice behaviors,
// disconnect and abort recovery). Run: node test/smoke.mjs (after
// `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("smoke: boot, one chat round-trip, one voice take", async (ctx) => {
  const { document, history, input, submitChat, startTake, failures } = ctx;

  if (!document.querySelector("#dock .mur-app")) {
    failures.push("the chat UI did not mount inside the dock");
  }

  const request = await submitChat("Hello?");
  if (!request) {
    failures.push("no chat frame was sent on the /ws socket");
  }
  if (!history.textContent.includes("Hello back")) {
    failures.push("the assistant reply did not render in the chat history");
  }

  // The take reads the composer's cursor when it starts, so stage the
  // empty composer before the mic click.
  input.value = "";
  input.setSelectionRange(0, 0);
  const takeSocket = await startTake();
  if (!takeSocket) {
    failures.push("the mic click did not open a /voice socket with a message listener");
    return;
  }
  takeSocket.onmessage({
    data: JSON.stringify({ type: "interim", committed: "hello world", tentative: "" }),
  });
  if (input.value !== "hello world") {
    failures.push(`the interim transcript did not land in the composer: "${input.value}"`);
  }
  takeSocket.onmessage({ data: JSON.stringify({ type: "final", text: "hello world" }) });
  if (input.value !== "hello world") {
    failures.push(`the final transcript did not land in the composer: "${input.value}"`);
  }
  takeSocket.onclose?.();
});
