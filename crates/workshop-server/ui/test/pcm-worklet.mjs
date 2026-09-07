import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const testDir = path.dirname(fileURLToPath(import.meta.url));
const uiDir = path.join(testDir, "..");
const fixturePath = path.join(
  uiDir,
  "..",
  "..",
  "gateway-stt",
  "tests",
  "fixtures",
  "audio",
  "pcm16le-24khz.json",
);

async function loadProcessor(
  name = "pcm16-capture",
  options = {},
  outputSampleRate = 24_000,
) {
  const source = await readFile(path.join(uiDir, "pcm-worklet.js"), "utf8");
  const processors = new Map();
  const messages = [];
  const port = {
    onmessage: null,
    postMessage(value, transfer) {
      messages.push({ value, transfer });
    },
  };
  const context = vm.createContext({
    sampleRate: outputSampleRate,
    AudioWorkletProcessor: class {
      constructor() {
        this.port = port;
      }
    },
    registerProcessor(name, constructor) {
      assert.equal(processors.has(name), false, `${name} is registered once`);
      processors.set(name, constructor);
    },
  });
  new vm.Script(source, { filename: "pcm-worklet.js" }).runInContext(context);
  assert.deepEqual([...processors.keys()], ["pcm-capture", "pcm16-capture"]);
  const Processor = processors.get(name);
  assert.ok(Processor, `the real worklet registers ${name}`);
  return { processor: new Processor(options), messages, port };
}

function bytesOf(buffer) {
  return [...new Uint8Array(buffer)];
}

test("the real worklet emits the shared fixture as exact little-endian PCM16", async () => {
  const fixture = JSON.parse(await readFile(fixturePath, "utf8"));
  assert.equal(fixture.encoding, "pcm_s16le");
  assert.equal(fixture.sample_rate_hz, 24_000);
  assert.equal(fixture.channels, 1);
  await assert.rejects(
    () => loadProcessor("pcm16-capture", {}, 16_000),
    /24 kHz/,
    "the PCM16 processor rejects a graph with the wrong output rate",
  );

  const { processor, messages } = await loadProcessor(
    "pcm16-capture",
    { processorOptions: { chunkSamples: fixture.samples.length } },
  );
  const floats = fixture.samples.map((sample) =>
    sample < 0 ? sample / 32_768 : sample / 32_767,
  );
  processor.process([[Float32Array.from(floats.slice(0, 3))]]);
  assert.equal(messages.length, 0, "a partial block is carried");
  processor.process([[Float32Array.from(floats.slice(3))]]);

  assert.equal(messages.length, 1);
  assert.equal(Object.prototype.toString.call(messages[0].value), "[object ArrayBuffer]");
  assert.equal(messages[0].transfer.length, 1);
  assert.equal(messages[0].transfer[0], messages[0].value);
  assert.deepEqual(bytesOf(messages[0].value), fixture.bytes);
});

test("the legacy processor keeps sending copied 16 kHz float blocks", async () => {
  const { processor, messages } = await loadProcessor("pcm-capture", {}, 16_000);
  const input = Float32Array.from([-0.5, 0, 0.75]);

  processor.process([[input]]);
  input.fill(1);

  assert.equal(messages.length, 1);
  assert.equal(Object.prototype.toString.call(messages[0].value), "[object ArrayBuffer]");
  assert.equal(messages[0].transfer[0], messages[0].value);
  assert.deepEqual([...new Float32Array(messages[0].value)], [-0.5, 0, 0.75]);
});

test("the real worklet clips samples and flushes only the carried partial block", async () => {
  const { processor, messages, port } = await loadProcessor(
    "pcm16-capture",
    { processorOptions: { chunkSamples: 4 } },
  );
  processor.process([[Float32Array.from([-2, 2, -0.5, 0.5, 0.25])]]);

  assert.deepEqual(bytesOf(messages[0].value), [0, 128, 255, 127, 0, 192, 0, 64]);
  assert.equal(messages.length, 1);
  port.onmessage({ data: { type: "flush" } });
  assert.deepEqual(bytesOf(messages[1].value), [0, 32]);
  assert.equal(messages[2].value.type, "flushed");
  port.onmessage({ data: { type: "flush" } });
  assert.equal(messages[3].value.type, "flushed");
});

test("clear resets carried PCM16 before the next flush", async () => {
  const { processor, messages, port } = await loadProcessor(
    "pcm16-capture",
    { processorOptions: { chunkSamples: 4 } },
  );

  processor.process([[Float32Array.from([-0.5, 0.5])]]);
  port.onmessage({ data: { type: "clear" } });
  processor.process([[Float32Array.from([0.25])]]);
  port.onmessage({ data: { type: "flush" } });

  assert.equal(messages.length, 2);
  assert.deepEqual(bytesOf(messages[0].value), [0, 32]);
  assert.equal(messages[1].value.type, "flushed");
});
