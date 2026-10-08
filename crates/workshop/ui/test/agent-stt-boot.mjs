// Dictation on the booted workbench: the mic mounts on the agent session's
// input, negotiates the Realtime hypothesis extension, names a missing
// wait on the real status bar, lights the real recording LED for a live
// take, and dims it when the Realtime socket drops. The composition root
// owns the recording LED and hands it to dictation through STT_STATUS
// (src/services/stt-status.ts), registered before the layout boots: an
// agent panel restored from a saved layout lights the LED too, and so
// does a second agent panel opened beside the first. A dictating panel's
// capture presence reads its own dock panel: the live tab title for its
// label, and its `agent` or `agent:<instance>` id for its reveal. The behaviors
// themselves are pinned by test/agent-stt.mjs against the view; this
// proves the composition root wires the view to the bar.
//
// bootWorkbench exits the process, so each scenario boots in its own child
// process; run without arguments this file drives them all and fails if
// any does. Run: node test/agent-stt-boot.mjs (after `npm run build`).
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { bootWorkbench } from "./helpers/boot.mjs";

const LIT = "status-bar__led--red";

// A v5 layout envelope as buildLayoutEnvelope writes it (tree left, agent
// right) under distinctive group ids, so a restore is told from the
// default layout by the zone map alone.
const RESTORED_LAYOUT = {
  version: 5,
  zones: { left: "restored-left", right: "restored-right" },
  overrides: {},
  layout: {
    grid: {
      root: {
        type: "branch",
        data: [
          { type: "leaf", data: { views: ["tree"], activeView: "tree", id: "restored-left" }, size: 100 },
          { type: "leaf", data: { views: ["agent"], activeView: "agent", id: "restored-right" }, size: 100 },
        ],
        size: 100,
      },
      width: 100,
      height: 100,
      orientation: "HORIZONTAL",
    },
    panels: {
      tree: { id: "tree", contentComponent: "tree", tabComponent: "panel-tab", title: "Workshop" },
      agent: { id: "agent", contentComponent: "agent", tabComponent: "panel-tab", title: "Agent Session" },
    },
    activeGroup: "restored-right",
  },
};

// Polls `probe` every 10 ms until it answers truthy or two seconds pass,
// answering the last value.
async function poll(sleep, probe) {
  const deadline = Date.now() + 2000;
  let value = probe();
  while (!value && Date.now() < deadline) {
    await sleep(10);
    value = probe();
  }
  return value;
}

// Clicks `mic` and waits for the shared production capture service to
// report recording on the already-negotiated Realtime socket: answers
// that socket once the recording LED is lit, or null when it never lights.
async function startTake({ recEl, sttSockets, sleep }, mic) {
  mic.click();
  return poll(sleep, () => {
    const socket = sttSockets().at(-1);
    return socket && typeof socket.onmessage === "function" && recEl.classList.contains(LIT)
      ? socket
      : null;
  });
}

// Pins a wait on the newest agent socket, starts a take from `mic`, and
// drops its Realtime socket, recording whether the LED lit for the take
// and dimmed on the drop.
async function recordOnce(ctx, mic, where) {
  const { emitAgent, recEl, failures } = ctx;
  emitAgent({ type: "input_required", token: `tok-${where}` });
  const socket = await startTake(ctx, mic);
  if (!socket) {
    failures.push(`a take on ${where} did not light the recording LED`);
    return;
  }
  socket.onclose?.();
  if (recEl.classList.contains(LIT)) {
    failures.push(`a dropped Realtime socket on ${where} did not dim the recording LED`);
  }
}

// Closes `panel` through its Dockview api and waits for `mic` to leave
// the document, so the stores the take built dispose inside the run.
async function closePanel({ document, sleep, failures }, panel, mic, where) {
  panel.api.close();
  if (!(await poll(sleep, () => !document.contains(mic)))) {
    failures.push(`closing ${where} did not unmount its mic`);
  }
}

const scenarios = {
  // The default layout's agent session: the full dictation path.
  wired: () =>
    bootWorkbench("dictation is wired into the booted agent session", async (ctx) => {
      const { document, recEl, statusText, emitAgent, sleep, failures } = ctx;

      // The session view (and its mic) shows once a session is acknowledged.
      emitAgent({ type: "agent_session", session: "s1", agent: "chat" });
      const mic = document.querySelector("#dock .ws-agent-session__action");
      const input = document.querySelector("#dock .ws-prompt-input__editor");
      if (!mic || !input) {
        failures.push("the agent session mounted no mic beside its input");
        return;
      }
      if (recEl.classList.contains(LIT)) {
        failures.push("the recording LED must start dark");
      }
      // Realtime negotiation resolves a tick after mount.
      await sleep(20);

      // No wait pinned: the click is refused and the bar says why.
      const gated = await startTake(ctx, mic);
      if (gated) {
        failures.push("a mic click with no wait pinned opened a Realtime socket");
      }
      if (!statusText.textContent.includes("isn't asking for input")) {
        failures.push(`a gated click named no blocker on the status bar (got "${statusText.textContent}")`);
      }

      // A pinned wait opens the mic; the take lights the real recording LED.
      emitAgent({ type: "input_required", token: "tok1" });
      const sttSocket = await startTake(ctx, mic);
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
      if (!recEl.classList.contains(LIT)) {
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
      if (recEl.classList.contains(LIT)) {
        failures.push("a dropped Realtime socket did not dim the recording LED");
      }
      if (input.getAttribute("contenteditable") !== "true") {
        failures.push("a dropped Realtime socket did not lift the input's read-only lock");
      }

      // Closing the Agent tab from its tab chip disposes the panel, the view,
      // and the stt handle: a click on the detached mic starts nothing.
      const agentTab = [...document.querySelectorAll("#dock .dv-default-tab")].find(
        (tab) => tab.querySelector(".dv-default-tab-content")?.textContent === "New Agent",
      );
      const closeAction = agentTab?.querySelector(".dv-default-tab-action");
      if (!closeAction) {
        failures.push("no closable tab action found for the New Agent tab");
        return;
      }
      closeAction.click();
      if (!(await poll(sleep, () => !document.contains(mic)))) {
        failures.push("closing the New Agent tab did not unmount its input form");
        return;
      }
      if (await startTake(ctx, mic)) {
        failures.push("a click on the closed tab's detached mic started a take");
      }
    }),

  // An agent panel restored from the workspace's saved layout: its
  // factory resolves the dictation port the composition root registered
  // before the dock (and the layout restore) existed.
  restored: () =>
    bootWorkbench(
      "a restored agent panel lights the recording LED",
      async (ctx) => {
        const { document, emitAgent, recEl, registeredServiceIds, resolveService, sleep, failures } = ctx;
        const zones = resolveService("workshop.zoneState");
        if (zones.groupFor("right") !== "restored-right") {
          failures.push(`the boot did not restore the saved layout (right zone is ${zones.groupFor("right")})`);
          return;
        }
        const ids = registeredServiceIds();
        const sttAt = ids.indexOf("workshop.sttStatus");
        const dockAt = ids.indexOf("workshop.dock");
        if (sttAt === -1 || dockAt === -1 || sttAt > dockAt) {
          failures.push(
            `STT_STATUS must register before the dock boots; registration order: ${JSON.stringify(ids)}`,
          );
        }
        if (recEl.classList.contains(LIT)) {
          failures.push("the recording LED must start dark");
        }
        emitAgent({ type: "agent_session", session: "s1", agent: "chat" });
        const mic = document.querySelector("#dock .ws-agent-session__action");
        if (!mic) {
          failures.push("the restored agent session mounted no mic");
          return;
        }
        await sleep(20);
        await recordOnce(ctx, mic, "the restored agent panel");
        await closePanel(ctx, resolveService("workshop.dock").getPanel("agent"), mic, "the restored agent panel");
      },
      { uiState: { workspace: { layout: RESTORED_LAYOUT } } },
    ),

  // A second agent panel beside the boot's: both resolve the one port, so
  // the second panel's take lights the same LED.
  twoAgents: () =>
    bootWorkbench("a second agent panel lights the recording LED", async (ctx) => {
      const { agentPanel, document, emitAgent, sockets, resolveService, sleep, failures } = ctx;
      const dock = resolveService("workshop.dock");
      const first = dock.getPanel("agent");
      const second = dock.addPanel({
        id: "agent:second",
        component: "agent",
        title: "Agent Session",
        params: { instance: "second" },
        position: { referenceGroup: first.group.id },
      });
      const agentSockets = () => sockets.filter((socket) => socket.url.endsWith("/agents/ws"));
      const opened = await poll(
        sleep,
        () => agentSockets().length === 2 && agentSockets()[1].readyState === 1,
      );
      if (!opened) {
        failures.push("the second agent panel opened no session socket of its own");
        return;
      }
      emitAgent({ type: "agent_session", session: "s2", agent: "chat" });
      const mic = [...document.querySelectorAll("#dock .ws-agent-session__action")].find(
        (candidate) => !agentPanel.contains(candidate),
      );
      if (!mic) {
        failures.push("the second agent session mounted no mic");
        return;
      }
      if (dock.panels.filter((panel) => panel.id.startsWith("agent")).length !== 2) {
        failures.push("the dock does not hold two agent panels");
      }
      await sleep(20);
      await recordOnce(ctx, mic, "the second agent panel");
      await closePanel(ctx, second, mic, "the second agent panel");
    }),

  // The dictating panel's capture presence reads the dock panel that hosts
  // it: the label follows the live tab title, and reveal reactivates that
  // panel - `agent` for the boot-time singleton, which carries no instance
  // param, and `agent:<instance>` for an opened one - without opening another.
  presence: () =>
    bootWorkbench("a dictating agent panel's presence names its tab and reveals its own panel", async (ctx) => {
      const { agentPanel, document, emitAgent, sockets, resolveService, sleep, failures } = ctx;
      const dock = resolveService("workshop.dock");
      const capture = resolveService("workshop.speechCapture");
      const first = dock.getPanel("agent");

      emitAgent({ type: "agent_session", session: "s1", agent: "chat" });
      const firstMic = agentPanel.querySelector(".ws-agent-session__action");
      if (!firstMic) {
        failures.push("the boot agent session mounted no mic");
        return;
      }
      await sleep(20);
      emitAgent({ type: "input_required", token: "tok-boot" });
      const firstTake = await startTake(ctx, firstMic);
      if (!firstTake) {
        failures.push("a take on the boot agent panel did not light the recording LED");
        return;
      }
      first.api.setTitle("Boot session");
      if (capture.presence?.label() !== "Boot session (chat)") {
        failures.push(`the boot panel's presence did not read its tab title (got "${capture.presence?.label()}")`);
      }

      const second = dock.addPanel({
        id: "agent:a",
        component: "agent",
        title: "Agent Session",
        params: { instance: "a" },
        position: { referenceGroup: first.group.id },
      });
      const agentSockets = () => sockets.filter((socket) => socket.url.endsWith("/agents/ws"));
      if (!(await poll(sleep, () => agentSockets().length === 2 && agentSockets()[1].readyState === 1))) {
        failures.push("the instance panel opened no session socket of its own");
        return;
      }
      const panelCount = dock.panels.length;
      if (dock.activePanel?.id !== "agent:a") {
        failures.push(`the opened instance panel was not active before the reveal (got ${dock.activePanel?.id})`);
      }
      capture.presence?.reveal();
      if (dock.activePanel?.id !== "agent" || dock.panels.length !== panelCount) {
        failures.push(
          `the boot panel's reveal did not reactivate the singleton (active ${dock.activePanel?.id}, ${dock.panels.length} panels)`,
        );
      }
      firstTake.onclose?.();
      if (!(await poll(sleep, () => capture.presence === null))) {
        failures.push("ending the boot panel's take did not clear the presence");
      }

      emitAgent({ type: "agent_session", session: "s2", agent: "chat" });
      second.api.setActive();
      const secondMic = [...document.querySelectorAll("#dock .ws-agent-session__action")].find(
        (candidate) => !agentPanel.contains(candidate),
      );
      if (!secondMic) {
        failures.push("the instance agent session mounted no mic");
        return;
      }
      await sleep(20);
      emitAgent({ type: "input_required", token: "tok-a" });
      if (!(await startTake(ctx, secondMic))) {
        failures.push("a take on the instance panel did not light the recording LED");
        return;
      }
      second.api.setTitle("Session A");
      if (capture.presence?.label() !== "Session A (chat)") {
        failures.push(`the instance panel's presence did not read its tab title (got "${capture.presence?.label()}")`);
      }
      first.api.setActive();
      if (dock.activePanel?.id !== "agent") {
        failures.push(`the boot panel was not active before the instance reveal (got ${dock.activePanel?.id})`);
      }
      capture.presence?.reveal();
      if (dock.activePanel?.id !== "agent:a" || dock.panels.length !== panelCount) {
        failures.push(
          `the instance panel's reveal did not reactivate agent:a (active ${dock.activePanel?.id}, ${dock.panels.length} panels)`,
        );
      }

      await closePanel(ctx, second, secondMic, "the instance agent panel");
      if (!(await poll(sleep, () => capture.presence === null))) {
        failures.push("closing the dictating instance panel did not clear the presence");
      }
      await closePanel(ctx, first, firstMic, "the boot agent panel");
    }),
};

const scenarioFlag = process.argv.find((arg) => arg.startsWith("--scenario="));
if (scenarioFlag) {
  const scenario = scenarios[scenarioFlag.slice("--scenario=".length)];
  if (!scenario) {
    console.error(`unknown scenario ${scenarioFlag}; known: ${Object.keys(scenarios).join(", ")}`);
    process.exit(1);
  }
  await scenario();
} else {
  // Driver: one child per scenario, output forwarded, any failure fails
  // the file.
  const self = fileURLToPath(import.meta.url);
  let failed = 0;
  for (const name of Object.keys(scenarios)) {
    const code = await new Promise((resolve) => {
      const child = spawn(process.execPath, [self, `--scenario=${name}`], { stdio: "inherit" });
      child.on("exit", (exitCode) => resolve(exitCode ?? 1));
      child.on("error", () => resolve(1));
    });
    if (code !== 0) failed += 1;
  }
  process.exit(failed === 0 ? 0 : 1);
}
