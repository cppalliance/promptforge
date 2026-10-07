//! The 24 kHz stream a native capture sends and the speech layout it records.

use std::ops::Range;

use base64::Engine as _;
use gateway_stt_engine::EnginePolicy;

const FRAME_SAMPLES: usize = EnginePolicy::SAMPLE_RATE * 30 / 1_000;
const FRAME: u64 = FRAME_SAMPLES as u64;

/// Lists the runs the segmenter reads as speech on its 30 ms frame grid,
/// which restarts at each of `grid_origins`, so audio synthesized from them
/// segments like the clip. The segmenter skips the part of a frame that a
/// restart cuts short; that part keeps the clip's own reading, so windows
/// synthesized over it stay as loud as the clip's.
pub(super) fn speech_runs(pcm: &[i16], grid_origins: &[u64]) -> Vec<[u64; 2]> {
    let samples = pcm
        .iter()
        .map(|sample| f32::from(*sample) / 32_768.0)
        .collect::<Vec<_>>();
    let total = u64::try_from(samples.len()).expect("the clip length fits u64");
    let index = |sample: u64| usize::try_from(sample).expect("the clip index fits usize");
    let mut runs: Vec<[u64; 2]> = Vec::new();
    let mut start = 0;
    while start + FRAME <= total {
        let end = grid_origins
            .iter()
            .copied()
            .find(|origin| (start + 1..start + FRAME).contains(origin))
            .unwrap_or(start + FRAME);
        if !EnginePolicy::is_silence(&samples[index(start)..index(end)]) {
            match runs.last_mut() {
                Some(run) if run[1] == start => run[1] = end,
                _ => runs.push([start, end]),
            }
        }
        start = end;
    }
    runs
}

/// Encodes the 24 kHz PCM16 input that the production resampler turns back
/// into exactly `pcm[output]`: input `3k` carries sample `2k`, and inputs
/// `3k + 1` and `3k + 2`, whose midpoint the resampler emits, both carry
/// sample `2k + 1`.
pub(super) fn pcm24_payload(pcm: &[i16], output: Range<u64>) -> String {
    let bytes = (input_samples(output.start)..input_samples(output.end))
        .flat_map(|input| {
            let position = input / 3 * 2 + u64::from(input % 3 != 0);
            let index = usize::try_from(position).expect("the clip index fits usize");
            pcm[index].to_le_bytes()
        })
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

const fn input_samples(output: u64) -> u64 {
    output / 2 * 3 + output % 2
}

#[test]
fn speech_runs_follow_the_frame_grid_through_a_restart() {
    let speech = i16::MAX / 2;
    let mut pcm = vec![0; FRAME_SAMPLES * 4];
    pcm[FRAME_SAMPLES..FRAME_SAMPLES * 3].fill(speech);
    assert_eq!(speech_runs(&pcm, &[]), [[FRAME, FRAME * 3]]);

    let origin = FRAME + 160;
    assert_eq!(
        speech_runs(&pcm, &[origin]),
        [[FRAME, FRAME * 3 + 160]],
        "after the restart frames start at {origin}, so speech reads a frame later"
    );
}
