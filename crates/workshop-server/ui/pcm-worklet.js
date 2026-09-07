"use strict";

const OUTPUT_SAMPLE_RATE = 24_000;
const DEFAULT_CHUNK_SAMPLES = OUTPUT_SAMPLE_RATE / 10;

// Legacy /stt capture remains 16 kHz mono f32 until its consumer migrates.
class PcmCaptureProcessor extends AudioWorkletProcessor {
  process(inputs) {
    const channel = inputs[0] && inputs[0][0];
    if (channel && channel.length > 0) {
      const copy = new Float32Array(channel);
      this.port.postMessage(copy.buffer, [copy.buffer]);
    }
    return true;
  }
}

// Converts the first input channel into exact little-endian mono PCM16.
// Full 100 ms chunks cross to the page immediately. A final partial chunk
// stays owned here until the page requests a flush before stopping.
class Pcm16CaptureProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    if (sampleRate !== OUTPUT_SAMPLE_RATE) {
      throw new Error(`pcm-capture requires a 24 kHz AudioContext, received ${sampleRate} Hz`);
    }
    const requested = options && options.processorOptions && options.processorOptions.chunkSamples;
    this.chunkSamples =
      Number.isSafeInteger(requested) && requested > 0 ? requested : DEFAULT_CHUNK_SAMPLES;
    this.pending = new ArrayBuffer(this.chunkSamples * 2);
    this.pendingView = new DataView(this.pending);
    this.pendingSamples = 0;
    this.port.onmessage = (event) => {
      const type = event && event.data && event.data.type;
      if (type === "clear") {
        this.pendingSamples = 0;
      } else if (type === "flush") {
        this.flush();
        this.port.postMessage({ type: "flushed" });
      }
    };
  }

  emit(samples) {
    const bytes = samples * 2;
    const output =
      samples === this.chunkSamples ? this.pending : this.pending.slice(0, bytes);
    this.port.postMessage(output, [output]);
    this.pending = new ArrayBuffer(this.chunkSamples * 2);
    this.pendingView = new DataView(this.pending);
    this.pendingSamples = 0;
  }

  flush() {
    if (this.pendingSamples > 0) {
      this.emit(this.pendingSamples);
    }
  }

  process(inputs) {
    const channel = inputs[0] && inputs[0][0];
    if (channel && channel.length > 0) {
      for (let index = 0; index < channel.length; index += 1) {
        const sample = Math.max(-1, Math.min(1, channel[index]));
        const pcm = Math.round(sample < 0 ? sample * 0x8000 : sample * 0x7fff);
        this.pendingView.setInt16(this.pendingSamples * 2, pcm, true);
        this.pendingSamples += 1;
        if (this.pendingSamples === this.chunkSamples) {
          this.emit(this.chunkSamples);
        }
      }
    }
    return true;
  }
}

registerProcessor("pcm-capture", PcmCaptureProcessor);
registerProcessor("pcm16-capture", Pcm16CaptureProcessor);
