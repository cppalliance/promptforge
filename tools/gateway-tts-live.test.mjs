import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  closeSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { renderConfig, runProbe } from "./gateway-tts-live.mjs";

// A fake credential: the secret-free test needs a canary value to hunt for,
// and the ferrying test needs a value the fake gateway can compare against.
const CANARY = "ttest-canary-not-a-real-key-9f8e7d6c";

// The gateway double: a Node script that parses --config for its bind port
// and serves canned speech, voices, and readiness responses on loopback.
// Behavior modes arrive through its environment, exactly where the real
// gateway would find TOGETHER_API_KEY.
const FAKE_GATEWAY_SOURCE = `
import { readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";

const configPath = process.argv[process.argv.indexOf("--config") + 1];
const config = readFileSync(configPath, "utf8");
const port = Number(config.match(/bind = "127\\.0\\.0\\.1:(\\d+)"/)[1]);
const mode = process.env.FAKE_GATEWAY_MODE ?? "ok";
if (process.env.FAKE_KEY_SINK) {
  const seen = process.env.TOGETHER_API_KEY;
  const expected = process.env.FAKE_EXPECT_KEY;
  const verdict = seen === undefined ? "absent" : seen === expected ? "match" : "mismatch";
  writeFileSync(process.env.FAKE_KEY_SINK, verdict);
}
if (mode === "exit-now") {
  console.error("fake gateway refusing to boot");
  process.exit(1);
}
const MP3 = Buffer.from([0x49, 0x44, 0x33, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10]);
const WAV = Buffer.concat([
  Buffer.from("RIFF", "latin1"),
  Buffer.alloc(4),
  Buffer.from("WAVE", "latin1"),
  Buffer.alloc(8),
]);
const VOICE_IDS = ["dan", "jess", "leah", "leo", "mia", "tara", "zac", "zoe"];

if (mode === "never-listen") {
  setInterval(() => {}, 1000);
} else {
  const server = createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (url.pathname === "/v1/models") {
      response.writeHead(200, { "content-type": "application/json" });
      response.end("{}");
      return;
    }
    if (url.pathname === "/v1/audio/voices") {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({ voices: VOICE_IDS.map((id) => ({ id, name: id })) }));
      return;
    }
    if (url.pathname === "/v1/audio/speech" && request.method === "POST") {
      let body = "";
      request.on("data", (chunk) => {
        body += chunk;
      });
      request.on("end", () => {
        if (mode === "error500") {
          response.writeHead(500, { "content-type": "application/json" });
          response.end(JSON.stringify({ error: { message: "upstream exploded" } }));
          return;
        }
        const parsed = JSON.parse(body);
        const wav = parsed.response_format === "wav" || mode === "wrong-format";
        response.writeHead(200, {
          "content-type": wav ? "audio/wav" : "audio/mpeg",
          "transfer-encoding": "chunked",
        });
        response.end(wav ? WAV : MP3);
      });
      return;
    }
    response.writeHead(404, { "content-type": "application/json" });
    response.end(JSON.stringify({ error: { message: "not found" } }));
  });
  server.listen(port, "127.0.0.1");
}
`;

function makeHarness(t, mode, { withKey = true } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "tts-live-test-"));
  t.after(() => rmSync(dir, { force: true, recursive: true, maxRetries: 5, retryDelay: 100 }));
  const fakePath = join(dir, "fake-gateway.mjs");
  writeFileSync(fakePath, FAKE_GATEWAY_SOURCE, "utf8");
  const sinkPath = join(dir, "key-sink.txt");
  const repo = join(dir, "repo");
  const binaryDir = join(repo, "target", "debug");
  mkdirSync(binaryDir, { recursive: true });
  const binaryName =
    process.platform === "win32" ? "promptforge-gateway.exe" : "promptforge-gateway";
  writeFileSync(join(binaryDir, binaryName), "fake binary", "utf8");

  const lines = [];
  const events = [];
  const spawned = [];
  const env = {
    ...process.env,
    FAKE_GATEWAY_MODE: mode,
    FAKE_KEY_SINK: sinkPath,
    FAKE_EXPECT_KEY: CANARY,
  };
  if (withKey) {
    env.TOGETHER_API_KEY = CANARY;
  } else {
    delete env.TOGETHER_API_KEY;
  }
  const deps = {
    env,
    repo,
    log: (line) => lines.push(line),
    build: async () => {
      events.push("build");
    },
    spawnGateway: ({ configPath, profile, env: childEnv, logPath }) => {
      events.push("spawn");
      const logFd = openSync(logPath, "a");
      const child = spawn(
        process.execPath,
        [fakePath, "--config", configPath, "--profile", profile, "--no-tray"],
        { env: childEnv, stdio: ["ignore", logFd, logFd] },
      );
      closeSync(logFd);
      spawned.push(child);
      return child;
    },
    readyTimeoutMs: 5_000,
    readyPollMs: 50,
    callTimeoutMs: 5_000,
  };
  return { lines, events, spawned, deps, sinkPath };
}

function assertAllDead(spawned) {
  assert.ok(spawned.length > 0, "at least one gateway child was spawned");
  for (const child of spawned) {
    assert.notEqual(
      child.exitCode ?? child.signalCode,
      null,
      "the gateway subprocess was terminated",
    );
  }
}

test("passes against a conforming gateway and ferries the key only to the subprocess", async (t) => {
  const { lines, spawned, deps, sinkPath } = makeHarness(t, "ok");
  const code = await runProbe(deps);
  assert.equal(code, 0);
  const output = lines.join("\n");
  assert.match(output, /gateway up on 127\.0\.0\.1:\d+/);
  assert.match(output, /PASS default format is mp3/);
  assert.match(output, /PASS default response is streamed/);
  assert.match(output, /PASS wav format maps to audio\/wav with a RIFF body/);
  assert.match(output, /PASS emotion-tag input returns 200 with audio/);
  assert.match(output, /PASS voices union shape/);
  assert.match(output, /PASS field 'instructions' tolerated/);
  assert.match(output, /PASS field 'sample_rate' tolerated/);
  assert.match(output, /PASS field 'promptforge_probe' tolerated/);
  assert.match(output, /LIVE OK: every assertion passed/);
  assert.equal(readFileSync(sinkPath, "utf8"), "match");
  assertAllDead(spawned);
});

test("always builds the gateway before booting it", async (t) => {
  const { events, deps } = makeHarness(t, "ok");
  const code = await runProbe(deps);
  assert.equal(code, 0);
  assert.deepEqual(events.slice(0, 2), ["build", "spawn"]);
  assert.equal(events.filter((event) => event === "build").length, 1);
});

test("fails loudly when the gateway exits during boot", async (t) => {
  const { lines, spawned, deps } = makeHarness(t, "exit-now");
  const code = await runProbe(deps);
  assert.equal(code, 1);
  const output = lines.join("\n");
  assert.match(output, /gateway exited during boot/);
  assert.match(output, /refusing to boot/);
  assertAllDead(spawned);
});

test("fails when the gateway never serves within the readiness deadline", async (t) => {
  const { lines, spawned, deps } = makeHarness(t, "never-listen");
  deps.readyTimeoutMs = 600;
  const code = await runProbe(deps);
  assert.equal(code, 1);
  assert.match(lines.join("\n"), /did not serve within/);
  assertAllDead(spawned);
});

test("fails when the speech surface answers an error envelope", async (t) => {
  const { lines, spawned, deps } = makeHarness(t, "error500");
  const code = await runProbe(deps);
  assert.equal(code, 1);
  const output = lines.join("\n");
  assert.match(output, /FAIL default format is mp3: status=500/);
  assert.match(output, /LIVE FAIL: \d+ assertion\(s\) failed/);
  assertAllDead(spawned);
});

test("fails when a response violates the speech contract", async (t) => {
  const { lines, spawned, deps } = makeHarness(t, "wrong-format");
  const code = await runProbe(deps);
  assert.equal(code, 1);
  const output = lines.join("\n");
  assert.match(output, /FAIL default format is mp3/);
  assert.match(output, /PASS wav format maps to audio\/wav with a RIFF body/);
  assertAllDead(spawned);
});

test("terminates the gateway subprocess on success and on failure", async (t) => {
  for (const mode of ["ok", "error500", "exit-now"]) {
    const { spawned, deps } = makeHarness(t, mode);
    await runProbe(deps);
    assertAllDead(spawned);
  }
});

test("skips with exit 0 when TOGETHER_API_KEY is absent from the environment", async (t) => {
  const { lines, deps } = makeHarness(t, "ok", { withKey: false });
  deps.build = async () => {
    throw new Error("build must not run without a key");
  };
  deps.spawnGateway = () => {
    throw new Error("spawn must not run without a key");
  };
  const code = await runProbe(deps);
  assert.equal(code, 0);
  assert.match(lines.join("\n"), /SKIP: TOGETHER_API_KEY is not set/);
});

test("never prints the key and the config carries only the interpolation reference", async (t) => {
  const { lines, deps, sinkPath } = makeHarness(t, "ok");
  const code = await runProbe(deps);
  assert.equal(code, 0);
  assert.ok(
    !lines.join("\n").includes(CANARY),
    "the key never appears in the probe output",
  );
  const config = renderConfig({ port: 12345 });
  assert.ok(!config.includes(CANARY), "the config never contains the key value");
  assert.ok(
    config.includes('api_key = "${TOGETHER_API_KEY}"'),
    "the config carries only the interpolation reference",
  );
  assert.equal(readFileSync(sinkPath, "utf8"), "match");
});
