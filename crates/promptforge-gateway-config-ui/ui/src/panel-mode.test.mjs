// Pins the workshop-panel bridge client: with a pinned bridge origin in
// the URL, boot announces itself, waits for the workshop's context
// message, and mounts the live shell whose every gateway call rides
// postMessage - no direct gateway fetch, no sessionStorage key, and no
// progress SSE subscription exist in the frame (the workshop owns
// progress display). Apply, Revert All, and a starting download are
// announced to the parent; theme and initial-route context apply; a
// parent that never answers produces an error toast, not a hang; and a
// context message from a foreign origin is ignored.
import assert from "node:assert/strict";
import test from "node:test";

import {
  bootApp,
  gatewayStub,
  hfModelFixture,
  hfSearchFixture,
  modelsFixture,
  navigate,
  settle,
  systemFixture,
} from "./harness.mjs";

const WORKSHOP_ORIGIN = "http://127.0.0.1:7910";
const PANEL_URL = `http://127.0.0.1:8081/config/?mode=panel&bridge=${encodeURIComponent(
  WORKSHOP_ORIGIN,
)}`;

/** A dirty report so the Apply/Revert pair renders. */
function dirtyReport() {
  return { dirty: true, pending_files: ["profiles/default.toml"], changed_sections: [] };
}

/**
 * A scripted workshop parent: records every message the frame posts,
 * answers pf-bridge-ready with a context message, and forwards pf-api
 * requests to a gatewayStub the way the real workshop's server proxy
 * would - unless `mute(message)` says to stay silent (for timeout
 * staging). Messages sent before the jsdom window exists queue until
 * connect(dom).
 */
function makeParent(stub, { context = { theme: "dark", route: "#/models" }, mute } = {}) {
  const posted = [];
  const queued = [];
  let dispatchTo = null;
  const send = (data) => {
    if (dispatchTo) {
      dispatchTo(data);
    } else {
      queued.push(data);
    }
  };
  const bridgePost = (message) => {
    posted.push(message);
    if (message.type === "pf-bridge-ready") {
      send({ type: "pf-context", ...context });
      return;
    }
    if (message.type === "pf-api" && !(mute && mute(message))) {
      void stub
        .fetchFn(message.path, {
          method: message.method,
          headers: { "Content-Type": "application/json" },
          ...(message.body === null ? {} : { body: message.body }),
        })
        .then(async (response) => {
          send({
            type: "pf-api-result",
            id: message.id,
            status: response.status,
            contentType: response.headers.get("content-type"),
            body: await response.text(),
          });
        });
    }
  };
  return {
    posted,
    bridgePost,
    connect(dom) {
      dispatchTo = (data) =>
        dom.window.dispatchEvent(
          new dom.window.MessageEvent("message", { data, origin: WORKSHOP_ORIGIN }),
        );
      for (const data of queued.splice(0)) {
        dispatchTo(data);
      }
    },
    /** Dispatches a raw message claiming `origin`, for pinning checks. */
    spoof(dom, data, origin) {
      dom.window.dispatchEvent(new dom.window.MessageEvent("message", { data, origin }));
    },
    actions: () => posted.filter((m) => m.type === "pf-action").map((m) => m.action),
    apiPaths: () => posted.filter((m) => m.type === "pf-api").map((m) => m.path),
  };
}

/**
 * Boots panel mode against a scripted parent. `direct` spies on the
 * frame's own fetch: gateway-relative URLs fail loudly (the frame must
 * never dial the gateway itself); absolute hub URLs answer a canned
 * README, mirroring the key-free hub bytes the Discover view loads.
 */
async function bootPanel({ stub, parentOpts, timeoutMs } = {}) {
  const parent = makeParent(stub, parentOpts);
  const direct = [];
  const directSpy = async (input) => {
    const url = String(input);
    direct.push(url);
    if (/^https?:\/\//.test(url)) {
      return new Response("# readme", {
        status: 200,
        headers: { "content-type": "text/markdown" },
      });
    }
    throw new TypeError(`panel mode must not fetch the gateway directly: ${url}`);
  };
  const { dom, root } = await bootApp({
    url: PANEL_URL,
    stub: { fetchFn: directSpy },
    options: { bridgePost: parent.bridgePost, bridgeTimeoutMs: timeoutMs },
  });
  parent.connect(dom);
  await settle();
  return { dom, root, parent, direct };
}

test("panel mode routes every gateway call through the bridge, with no fetch and no SSE", async () => {
  const stub = gatewayStub({ config: modelsFixture(), pending: modelsFixture() });
  const { root, parent, direct } = await bootPanel({ stub });

  assert.equal(root.querySelector("#gateway-api-key"), null, "no key prompt in panel mode");
  assert.equal(root.querySelector("img.tab-medallion"), null, "no medallion in panel mode");
  assert.equal(
    root.querySelector(".banner")?.textContent?.includes("bridge pending"),
    false,
    "the bridge-pending banner is gone once the context arrives",
  );

  assert.ok(parent.apiPaths().includes("/admin/config"), "the config load rode the bridge");
  assert.ok(parent.apiPaths().includes("/admin/status"), "the status probe rode the bridge");
  assert.ok(
    root.textContent.includes("gpt-remote"),
    "a bridged call resolved: the models list renders the fixture model",
  );

  const gatewayDirect = direct.filter((url) => !/^https?:\/\//.test(url));
  assert.deepEqual(gatewayDirect, [], "the frame never fetches the gateway directly");
  assert.ok(
    !parent.apiPaths().includes("/admin/progress"),
    "no progress SSE subscription happens in panel mode - the workshop owns progress",
  );
  assert.ok(
    !stub.calls.some((call) => call.url.includes("/admin/progress")),
    "the progress stream is never reached at all",
  );
});

test("the workshop's theme and initial route context apply", async () => {
  const stub = gatewayStub({ config: modelsFixture(), pending: modelsFixture() });
  const { dom, root } = await bootPanel({
    stub,
    parentOpts: { context: { theme: "dark", route: "#/downloads" } },
  });
  assert.equal(root.getAttribute("data-theme"), "dark", "the theme context lands on the root");
  assert.equal(dom.window.location.hash, "#/downloads", "the initial route follows the context");
  assert.equal(
    root.querySelector("main h1.view-title")?.textContent,
    "Downloads",
    "the routed view mounted",
  );
});

test("a context message from a foreign origin is ignored", async () => {
  const stub = gatewayStub({ config: modelsFixture(), pending: modelsFixture() });
  const parent = makeParent(stub);
  // Boot without connecting the parent: only the spoofed message arrives.
  const { dom, root } = await bootApp({
    url: PANEL_URL,
    stub: { fetchFn: () => Promise.reject(new TypeError("no direct fetch")) },
    options: { bridgePost: parent.bridgePost },
  });
  parent.spoof(dom, { type: "pf-context", theme: "dark", route: "#/models" }, "https://evil.example");
  await settle();
  assert.match(
    root.querySelector(".banner")?.textContent ?? "",
    /bridge pending/i,
    "a foreign context never mounts the live shell",
  );
});

test("a non-loopback bridge origin stays inert and posts nothing", async () => {
  const stub = gatewayStub({ config: modelsFixture(), pending: modelsFixture() });
  const parent = makeParent(stub);
  const foreignUrl = `http://127.0.0.1:8081/config/?mode=panel&bridge=${encodeURIComponent(
    "https://evil.example",
  )}`;
  const { root } = await bootApp({
    url: foreignUrl,
    stub: { fetchFn: () => Promise.reject(new TypeError("no direct fetch")) },
    options: { bridgePost: parent.bridgePost },
  });
  await settle();
  assert.equal(
    parent.posted.length,
    0,
    "a non-loopback bridge origin makes no bridge announcement, so no traffic can reach it",
  );
  assert.match(
    root.querySelector(".banner")?.textContent ?? "",
    /bridge pending/i,
    "a non-loopback bridge origin never mounts the live shell",
  );
});

test("Apply and Revert All are announced to the workshop", async () => {
  const stub = gatewayStub({
    config: modelsFixture(),
    pending: modelsFixture(),
    dirty: dirtyReport(),
  });
  const { root, parent } = await bootPanel({ stub });

  root.querySelector(".apply-button").click();
  await settle();
  assert.deepEqual(parent.actions(), ["apply"], "a successful apply notifies the parent");

  // Fresh boot with pending changes again, for the revert path.
  const second = gatewayStub({
    config: modelsFixture(),
    pending: modelsFixture(),
    dirty: dirtyReport(),
  });
  const revertBoot = await bootPanel({ stub: second });
  revertBoot.root.querySelector(".revert-button").click();
  await settle();
  revertBoot.root.ownerDocument.querySelector(".confirm-overlay .button-danger").click();
  await settle();
  assert.deepEqual(
    revertBoot.parent.actions(),
    ["revert"],
    "a confirmed revert notifies the parent",
  );
});

test("a bridge timeout surfaces an error toast, not a hang", async () => {
  const stub = gatewayStub({
    config: modelsFixture(),
    pending: modelsFixture(),
    dirty: dirtyReport(),
  });
  const { root } = await bootPanel({
    stub,
    timeoutMs: 20,
    parentOpts: { mute: (message) => message.path === "/admin/config-apply" },
  });

  root.querySelector(".apply-button").click();
  await new Promise((resolve) => setTimeout(resolve, 60));
  await settle();
  const toast = root.ownerDocument.querySelector(".toast-error");
  assert.ok(toast, "the timed-out apply raises an error toast");
  assert.match(toast.textContent, /timed out/i, "the toast names the timeout");
});

test("a starting download is announced to the workshop", async () => {
  const stub = gatewayStub({
    config: modelsFixture(),
    pending: modelsFixture(),
    hfSearch: hfSearchFixture(),
    hfModels: { "unsloth/Qwen3-Test-8B-GGUF": hfModelFixture() },
    system: systemFixture(),
  });
  const { dom, root, parent } = await bootPanel({ stub });
  navigate(dom, "#/discover");
  await settle();
  const input = root.querySelector("#discover-search");
  input.value = "unsloth/Qwen3-Test-8B-GGUF";
  input.dispatchEvent(new dom.window.Event("input"));
  // The search box debounces on real timers before the detail loads.
  await new Promise((resolve) => setTimeout(resolve, 400));
  await settle();
  root.querySelector(".quant-download").click();
  await settle();
  assert.ok(
    parent.actions().includes("download-started"),
    "starting a download notifies the parent",
  );
});
