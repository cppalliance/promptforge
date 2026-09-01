// Voice on the booted workbench: the mic mounts on the agent session's
// input, the capability probe reaches /voice/capability, a click with no
// wait pinned names the blocker on the real status bar, a live take lights
// the real REC badge, and a dropped /voice socket clears it. The
// behaviors themselves are pinned by test/agent-voice.mjs against the
// view; this proves the composition root wires the view to the bar.
// Run: node test/agent-voice-boot.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("voice dictation is wired into the booted agent session", async (ctx) => {
  const { document, recEl, statusText, emitAgent, voiceSockets, sleep, failures } = ctx;

  // The session view (and its mic) shows once a session is acknowledged.
  emitAgent({ type: "agent_session", session: "s1", agent: "chat" });
  const mic = document.querySelector("#dock .agent-session__mic");
  const input = document.querySelector("#dock .agent-session__input");
  if (!mic || !input) {
    failures.push("the agent session mounted no mic beside its input");
    return;
  }
  if (recEl.classList.contains("status-bar__rec--active")) {
    failures.push("the REC badge must start idle");
  }
  // The probe resolves a tick after mount.
  await sleep(20);

  // Clicks the mic and waits for a fresh /voice socket with a message
  // listener; null when no take began.
  async function startTake() {
    const before = voiceSockets().length;
    mic.click();
    const deadline = Date.now() + 2000;
    while (Date.now() < deadline) {
      const opened = voiceSockets();
      if (opened.length > before && typeof opened.at(-1).onmessage === "function") {
        return opened.at(-1);
      }
      await sleep(10);
    }
    return null;
  }

  // No wait pinned: the click is refused and the bar says why.
  const gated = await startTake();
  if (gated) {
    failures.push("a mic click with no wait pinned opened a /voice socket");
  }
  if (!statusText.textContent.includes("isn't asking for input")) {
    failures.push(`a gated click named no blocker on the status bar (got "${statusText.textContent}")`);
  }

  // A pinned wait opens the mic; the take lights the real REC badge.
  emitAgent({ type: "input_required", token: "tok1" });
  const voiceSocket = await startTake();
  if (!voiceSocket) {
    failures.push("the mic click did not open a /voice socket once a wait was pinned");
    return;
  }
  if (!voiceSocket.sent.includes("start")) {
    failures.push("the take did not send start on its /voice socket");
  }
  if (!recEl.classList.contains("status-bar__rec--active")) {
    failures.push("starting voice capture did not light the REC badge");
  }
  voiceSocket.onmessage({ data: JSON.stringify({ type: "interim", committed: "hello", tentative: "" }) });
  if (input.value !== "hello" || !input.readOnly) {
    failures.push(`the interim did not land in the readOnly agent input (got "${input.value}")`);
  }

  // The scripted socket never fires onclose on its own; a drop clears the badge.
  voiceSocket.onclose?.();
  if (recEl.classList.contains("status-bar__rec--active")) {
    failures.push("a dropped voice socket did not clear the REC badge");
  }
  if (input.readOnly) {
    failures.push("a dropped voice socket did not lift the input's readOnly");
  }

  // Closing the Agent tab from its tab chip disposes the panel, the view,
  // and the voice handle: a click on the detached mic starts nothing.
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
