// Dictation on the booted workbench: the mic mounts on the agent session's
// input, negotiates the Realtime hypothesis extension, names a missing
// wait on the real status bar, lights the real recording LED for a live
// take, and dims it when the Realtime socket drops. The
// behaviors themselves are pinned by test/agent-stt.mjs against the
// view; this proves the composition root wires the view to the bar.
// Run: node test/agent-stt-boot.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("dictation is wired into the booted agent session", async (ctx) => {
  const { document, recEl, statusText, emitAgent, sttSockets, sleep, failures } = ctx;

  // The session view (and its mic) shows once a session is acknowledged.
  emitAgent({ type: "agent_session", session: "s1", agent: "chat" });
  const mic = document.querySelector("#dock .ws-agent-session__mic");
  const input = document.querySelector("#dock .ws-prompt-input__editor");
  if (!mic || !input) {
    failures.push("the agent session mounted no mic beside its input");
    return;
  }
  if (recEl.classList.contains("status-bar__led--recording")) {
    failures.push("the recording LED must start dark");
  }
  // Realtime negotiation resolves a tick after mount.
  await sleep(20);

  // Clicks the mic and waits for the shared production capture service to
  // report recording on the already-negotiated Realtime socket.
  async function startTake() {
    mic.click();
    const deadline = Date.now() + 2000;
    while (Date.now() < deadline) {
      const socket = sttSockets().at(-1);
      if (
        socket &&
        typeof socket.onmessage === "function" &&
        recEl.classList.contains("status-bar__led--recording")
      ) {
        return socket;
      }
      await sleep(10);
    }
    return null;
  }

  // No wait pinned: the click is refused and the bar says why.
  const gated = await startTake();
  if (gated) {
    failures.push("a mic click with no wait pinned opened a Realtime socket");
  }
  if (!statusText.textContent.includes("isn't asking for input")) {
    failures.push(`a gated click named no blocker on the status bar (got "${statusText.textContent}")`);
  }

  // A pinned wait opens the mic; the take lights the real recording LED.
  emitAgent({ type: "input_required", token: "tok1" });
  const sttSocket = await startTake();
  if (!sttSocket) {
    failures.push("the mic click did not start capture once a wait was pinned");
    return;
  }
  if (
    !sttSocket.sent
      .map((event) => JSON.parse(event))
      .some((event) => event.type === "session.update")
  ) {
    failures.push("the Realtime socket did not negotiate the hypothesis extension");
  }
  if (!recEl.classList.contains("status-bar__led--recording")) {
    failures.push("starting dictation did not light the recording LED");
  }
  sttSocket.onmessage({
    data: JSON.stringify({
      type: "input_audio_buffer.committed",
      event_id: "boot_committed",
      item_id: "boot_item",
      previous_item_id: null,
    }),
  });
  sttSocket.onmessage({
    data: JSON.stringify({
      type: "conversation.item.input_audio_transcription.hypothesis",
      event_id: "boot_hypothesis",
      item_id: "boot_item",
      content_index: 0,
      revision: 1,
      transcript: "hello",
      finalized: "hel",
      agreed: "l",
      tentative: "o",
      audio_start_ms: 0,
      audio_end_ms: 100,
    }),
  });
  if (input.textContent !== "hello" || input.getAttribute("contenteditable") !== "false") {
    failures.push(`the interim did not land in the read-only agent input (got "${input.textContent}")`);
  }

  // The scripted socket never fires onclose on its own; a drop dims the LED.
  sttSocket.onclose?.();
  if (recEl.classList.contains("status-bar__led--recording")) {
    failures.push("a dropped Realtime socket did not dim the recording LED");
  }
  if (input.getAttribute("contenteditable") !== "true") {
    failures.push("a dropped Realtime socket did not lift the input's read-only lock");
  }

  // Closing the Agent tab from its tab chip disposes the panel, the view,
  // and the stt handle: a click on the detached mic starts nothing.
  const agentTab = [...document.querySelectorAll("#dock .dv-default-tab")].find(
    (tab) => tab.querySelector(".dv-default-tab-content")?.textContent === "Agent Session",
  );
  const closeAction = agentTab?.querySelector(".dv-default-tab-action");
  if (!closeAction) {
    failures.push("no closable tab action found for the Agent Session tab");
    return;
  }
  closeAction.click();
  const closeDeadline = Date.now() + 2000;
  while (document.contains(mic) && Date.now() < closeDeadline) {
    await sleep(20);
  }
  if (document.contains(mic)) {
    failures.push("closing the Agent Session tab did not unmount its input form");
    return;
  }
  if (await startTake()) {
    failures.push("a click on the closed tab's detached mic started a take");
  }
});
