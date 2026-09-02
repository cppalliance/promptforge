"use strict";

// Ships each mono f32 PCM block to the page, which forwards it over the
// /stt WebSocket. Runs on the audio rendering thread inside an
// AudioContext constructed at 16 kHz, so blocks arrive already resampled.
class PcmCaptureProcessor extends AudioWorkletProcessor {
  process(inputs) {
    const channel = inputs[0] && inputs[0][0];
    if (channel && channel.length > 0) {
      // The engine reuses its input buffers, so the block is copied before
      // crossing to the main thread.
      const copy = new Float32Array(channel);
      this.port.postMessage(copy.buffer, [copy.buffer]);
    }
    // No output is written; the node renders silence into the graph.
    return true;
  }
}

registerProcessor("pcm-capture", PcmCaptureProcessor);
