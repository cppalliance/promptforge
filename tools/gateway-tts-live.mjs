// Live probe for the gateway speech surface. Dev-only; never in CI.
//
// Always builds the gateway first (`cargo build -p gateway`), so a stale
// binary can never be probed against a recorded current commit. Boots the
// fresh binary on an ephemeral loopback port with a throwaway
// Together-backed profile and asserts the speech and voices surfaces
// through the gateway's own responses.
//
// Invariant A19: the script never calls a vendor directly. The vendor key
// comes from the process environment only (no dotenv parsing, no .env
// reading) and is ferried only to the gateway subprocess environment; the
// throwaway config carries the `api_key = "${TOGETHER_API_KEY}"`
// interpolation reference. The key is never printed and never written to
// any file by this script.
//
// Usage: `node tools/gateway-tts-live.mjs`. Exit 0 on skip (no key in the
// environment) or when every assertion passes, 1 otherwise.

import { spawn, spawnSync } from "node:child_process";
import {
  closeSync,
  existsSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPOSITORY_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const KEY_ENV_VAR = "TOGETHER_API_KEY";
const TOGETHER_MODEL = "canopylabs/orpheus-3b-0.1-ft";
const GATEWAY_MODEL = "orpheus";
const PROFILE_NAME = "tts-live";
const VOICES = ["tara", "leah", "jess", "leo", "dan", "mia", "zac", "zoe"];
// Local-only shared secret for the throwaway config's [server] api_key; not
// a real credential, never leaves the loopback listener.
const GATEWAY_KEY = "tts-live-throwaway-local-key";
const PROBE_TEXT =
  "PromptForge gateway live probe: the quick brown fox jumps over the lazy dog.";
const EMOTION_TEXT =
  "Angle-bracket emotion tags must reach the provider untouched. <laugh>";
const READY_TIMEOUT_MS = 90_000;
const READY_POLL_MS = 250;
const CALL_TIMEOUT_MS = 180_000;
const STOP_GRACE_MS = 10_000;
const LOG_TAIL_CHARS = 2_000;

function sleep(ms) {
  return new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
}

export function renderConfig({ port }) {
  const voices = VOICES.map((voice) => JSON.stringify(voice)).join(", ");
  return `config-version = 2

[server]
bind = "127.0.0.1:${port}"
api_key = "${GATEWAY_KEY}"

[[endpoint]]
id = "together"
protocol = "openai"
base_url = "https://api.together.xyz/v1"
api_key = "\${TOGETHER_API_KEY}"

[[model]]
name = "${GATEWAY_MODEL}"
kind = "speech"
description = "Orpheus 3B conversational speech synthesis (live probe)"
upstream = "${TOGETHER_MODEL}"
endpoints = ["together"]
context = 8192
voices = [${voices}]

[[profile]]
name = "${PROFILE_NAME}"
models = ["${GATEWAY_MODEL}"]
`;
}

function gatewayBinaryPath(repo) {
  const name =
    process.platform === "win32" ? "promptforge-gateway.exe" : "promptforge-gateway";
  return join(repo, "target", "debug", name);
}

function buildGateway({ repo }) {
  const result = spawnSync("cargo", ["build", "-p", "gateway"], {
    cwd: repo,
    stdio: "inherit",
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`cargo build -p gateway exited with status ${result.status}`);
  }
}

function spawnGatewayBinary({ binary, configPath, profile, env, logPath }) {
  const logFd = openSync(logPath, "a");
  try {
    return spawn(
      binary,
      ["--config", configPath, "--profile", profile, "--no-tray"],
      { env, stdio: ["ignore", logFd, logFd] },
    );
  } finally {
    closeSync(logFd);
  }
}

function freePort() {
  return new Promise((resolvePort, reject) => {
    const probe = createServer();
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address();
      probe.close(() => resolvePort(port));
    });
  });
}

function logTail(logPath) {
  try {
    return readFileSync(logPath, "utf8").slice(-LOG_TAIL_CHARS);
  } catch {
    return "<no gateway log>";
  }
}

async function waitReady({ port, proc, fetchImpl, timeoutMs, pollMs, readTail }) {
  const deadline = Date.now() + timeoutMs;
  const url = `http://127.0.0.1:${port}/v1/models`;
  while (Date.now() < deadline) {
    if (proc.exitCode !== null || proc.signalCode !== null) {
      throw new Error(
        `gateway exited during boot (code ${proc.exitCode ?? proc.signalCode}); log tail:\n${readTail()}`,
      );
    }
    try {
      const response = await fetchImpl(url, {
        headers: { authorization: `Bearer ${GATEWAY_KEY}` },
        signal: AbortSignal.timeout(2_000),
      });
      if (response.status === 200) {
        return;
      }
    } catch {
      // Not up yet: connection refused or a timed-out poll attempt.
    }
    await sleep(pollMs);
  }
  throw new Error(
    `gateway did not serve within ${Math.round(timeoutMs / 1000)}s; log tail:\n${readTail()}`,
  );
}

async function stopGateway(proc) {
  if (proc.exitCode !== null || proc.signalCode !== null) {
    return;
  }
  const exited = new Promise((resolveExit) =>
    proc.once("exit", () => resolveExit(true)),
  );
  proc.kill("SIGTERM");
  const graceful = await Promise.race([
    exited,
    sleep(STOP_GRACE_MS).then(() => false),
  ]);
  if (graceful || proc.exitCode !== null || proc.signalCode !== null) {
    return;
  }
  proc.kill("SIGKILL");
  await Promise.race([exited, sleep(STOP_GRACE_MS)]);
}

async function speechCreate({ baseUrl, body, fetchImpl, timeoutMs }) {
  try {
    const response = await fetchImpl(`${baseUrl}/audio/speech`, {
      method: "POST",
      headers: {
        authorization: `Bearer ${GATEWAY_KEY}`,
        "content-type": "application/json",
      },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(timeoutMs),
    });
    const audio = Buffer.from(await response.arrayBuffer());
    return {
      ok: response.ok,
      status: response.status,
      contentType: response.headers.get("content-type") ?? "",
      transferEncoding: response.headers.get("transfer-encoding") ?? "",
      contentLength: response.headers.get("content-length") ?? "",
      bytes: audio.length,
      body: audio,
    };
  } catch (error) {
    return { ok: false, status: null, error: String(error).slice(0, 300) };
  }
}

async function voicesCall({ baseUrl, fetchImpl, timeoutMs }) {
  try {
    const response = await fetchImpl(`${baseUrl}/audio/voices`, {
      headers: { authorization: `Bearer ${GATEWAY_KEY}` },
      signal: AbortSignal.timeout(timeoutMs),
    });
    let payload = null;
    try {
      payload = await response.json();
    } catch {
      payload = null;
    }
    return { ok: response.ok, status: response.status, json: payload };
  } catch {
    return { ok: false, status: null, json: null };
  }
}

function isMp3(body) {
  return (
    body.subarray(0, 3).toString("latin1") === "ID3" ||
    (body.length > 1 && body[0] === 0xff && (body[1] & 0xe0) === 0xe0)
  );
}

function isWav(body) {
  return (
    body.length > 11 &&
    body.subarray(0, 4).toString("latin1") === "RIFF" &&
    body.subarray(8, 12).toString("latin1") === "WAVE"
  );
}

function describeSpeech(result) {
  if (!result.ok) {
    return `status=${result.status} error=${JSON.stringify(result.error ?? "")}`;
  }
  return (
    `status=${result.status} content-type=${JSON.stringify(result.contentType)} ` +
    `transfer-encoding=${JSON.stringify(result.transferEncoding)} ` +
    `content-length=${JSON.stringify(result.contentLength)} ` +
    `bytes=${result.bytes} magic=${result.body.subarray(0, 4).toString("hex")}`
  );
}

function createReporter(log) {
  const failures = [];
  return {
    failures,
    check(label, condition, detail) {
      log(`${condition ? "PASS" : "FAIL"} ${label}: ${detail}`);
      if (!condition) {
        failures.push(label);
      }
    },
    observe(label, detail) {
      log(`NOTE ${label}: ${detail}`);
    },
  };
}

async function runSpeechSurface({
  baseUrl,
  model,
  fetchImpl,
  callTimeoutMs,
  reporter,
}) {
  const call = (body) =>
    speechCreate({ baseUrl, body, fetchImpl, timeoutMs: callTimeoutMs });

  const defaultCall = await call({ model, voice: "tara", input: PROBE_TEXT });
  reporter.observe("speech default format", describeSpeech(defaultCall));
  reporter.check(
    "default format is mp3",
    defaultCall.ok &&
      defaultCall.contentType.split(";")[0].trim() === "audio/mpeg" &&
      defaultCall.bytes > 0 &&
      isMp3(defaultCall.body),
    describeSpeech(defaultCall),
  );
  reporter.check(
    "default response is streamed",
    defaultCall.ok && defaultCall.contentLength === "",
    `content-length=${JSON.stringify(defaultCall.contentLength ?? "")} ` +
      `transfer-encoding=${JSON.stringify(defaultCall.transferEncoding ?? "")}`,
  );

  const wav = await call({
    model,
    voice: "tara",
    input: PROBE_TEXT,
    response_format: "wav",
  });
  reporter.observe("speech wav", describeSpeech(wav));
  reporter.check(
    "wav format maps to audio/wav with a RIFF body",
    wav.ok &&
      wav.contentType.split(";")[0].trim() === "audio/wav" &&
      wav.bytes > 0 &&
      isWav(wav.body),
    describeSpeech(wav),
  );

  const emotion = await call({ model, voice: "tara", input: EMOTION_TEXT });
  reporter.observe("emotion-tag input", describeSpeech(emotion));
  reporter.check(
    "emotion-tag input returns 200 with audio",
    emotion.ok && emotion.bytes > 0,
    describeSpeech(emotion),
  );

  const voices = await voicesCall({ baseUrl, fetchImpl, timeoutMs: callTimeoutMs });
  const entries =
    voices.json && Array.isArray(voices.json.voices) ? voices.json.voices : null;
  const ids = entries ? entries.map((entry) => entry?.id) : [];
  const sorted = [...VOICES].sort();
  reporter.check(
    "voices union shape",
    voices.ok &&
      entries !== null &&
      JSON.stringify(ids) === JSON.stringify(sorted) &&
      entries.every(
        (entry) => entry && typeof entry === "object" && entry.name === entry.id,
      ),
    `status=${voices.status} ids=${JSON.stringify(ids)}`,
  );

  // `instructions` is a named optional wire field; `sample_rate` and the
  // bogus field ride the verbatim passthrough. All three must come back 2xx.
  for (const [field, value] of [
    ["instructions", "Speak with a calm tone."],
    ["sample_rate", 44100],
    ["promptforge_probe", 1],
  ]) {
    const probe = await call({ model, voice: "tara", input: PROBE_TEXT, [field]: value });
    reporter.check(`field '${field}' tolerated`, probe.ok, describeSpeech(probe));
  }

  reporter.observe(
    "429/503 envelopes",
    "not provoked against the paid provider; covered by the Rust integration suite",
  );
}

export async function runProbe({
  env = process.env,
  repo = REPOSITORY_ROOT,
  log = console.log,
  build = buildGateway,
  spawnGateway = spawnGatewayBinary,
  fetchImpl = fetch,
  readyTimeoutMs = READY_TIMEOUT_MS,
  readyPollMs = READY_POLL_MS,
  callTimeoutMs = CALL_TIMEOUT_MS,
} = {}) {
  const key = env[KEY_ENV_VAR];
  if (!key) {
    log(
      `SKIP: ${KEY_ENV_VAR} is not set in the process environment; the live speech probe did not run.`,
    );
    return 0;
  }

  const reporter = createReporter(log);
  let scratch;
  try {
    await build({ repo });
    const binary = gatewayBinaryPath(repo);
    if (!existsSync(binary)) {
      throw new Error(
        `cargo build finished but the gateway binary is missing: ${binary}`,
      );
    }
    const port = await freePort();
    scratch = mkdtempSync(join(tmpdir(), "promptforge-tts-live-"));
    const configPath = join(scratch, "gateway.toml");
    writeFileSync(configPath, renderConfig({ port }), "utf8");
    const logPath = join(scratch, "gateway.log");
    const proc = spawnGateway({
      binary,
      configPath,
      profile: PROFILE_NAME,
      env: { ...env, [KEY_ENV_VAR]: key },
      logPath,
    });
    try {
      await waitReady({
        port,
        proc,
        fetchImpl,
        timeoutMs: readyTimeoutMs,
        pollMs: readyPollMs,
        readTail: () => logTail(logPath),
      });
      log(`gateway up on 127.0.0.1:${port} (throwaway profile '${PROFILE_NAME}')`);
      await runSpeechSurface({
        baseUrl: `http://127.0.0.1:${port}/v1`,
        model: GATEWAY_MODEL,
        fetchImpl,
        callTimeoutMs,
        reporter,
      });
    } finally {
      await stopGateway(proc);
    }
  } catch (error) {
    log(`FAIL: ${error instanceof Error ? error.message : String(error)}`);
    return 1;
  } finally {
    if (scratch) {
      rmSync(scratch, { force: true, recursive: true, maxRetries: 5, retryDelay: 100 });
    }
  }

  if (reporter.failures.length > 0) {
    log(
      `LIVE FAIL: ${reporter.failures.length} assertion(s) failed: ${reporter.failures.join(", ")}`,
    );
    return 1;
  }
  log("LIVE OK: every assertion passed");
  return 0;
}

if (
  process.argv[1] !== undefined &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  runProbe().then(
    (code) => {
      process.exitCode = code;
    },
    (error) => {
      console.error(error instanceof Error ? error.message : String(error));
      process.exitCode = 1;
    },
  );
}
