/** The loudness, in dBFS, that reads 0; quieter chunks clamp to it. */
const FLOOR_DBFS = -60;
/** The loudness, in dBFS, that reads 1: full scale. */
const CEILING_DBFS = 0;
const FULL_SCALE = 32_768;

/**
 * One capture chunk's loudness for a level meter: the RMS of its
 * little-endian PCM16 samples in dBFS, mapped linearly from FLOOR_DBFS to
 * CEILING_DBFS onto 0 to 1 and clamped. A trailing odd byte is ignored,
 * and an empty or silent chunk reads 0.
 */
export function chunkLevel(pcm16: ArrayBuffer): number {
  const view = new DataView(pcm16);
  const samples = Math.floor(pcm16.byteLength / 2);
  let sum = 0;
  for (let index = 0; index < samples; index++) {
    const sample = view.getInt16(index * 2, true) / FULL_SCALE;
    sum += sample * sample;
  }
  if (sum === 0) {
    return 0;
  }
  const dbfs = 10 * Math.log10(sum / samples);
  return Math.min(1, Math.max(0, (dbfs - FLOOR_DBFS) / (CEILING_DBFS - FLOOR_DBFS)));
}
