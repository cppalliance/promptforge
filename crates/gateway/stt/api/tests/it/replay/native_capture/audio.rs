//! The 24 kHz stream a native capture sends and the speech layout it records.

use std::ops::Range;

use base64::Engine as _;
use gateway_stt_engine::{EnergyDetector, EnginePolicy, SpeechDetector};

const FRAME_SAMPLES: usize = EnginePolicy::DETECTOR_CHUNK_SAMPLES;
const FRAME: u64 = FRAME_SAMPLES as u64;

/// Lists the runs the take's loudness detector reads as speech on the
/// segmenter's frame grid from sample 0, so a take scripted with them
/// segments like the clip.
pub(super) fn speech_runs(pcm: &[i16]) -> Vec<[u64; 2]> {
    let samples = pcm
        .iter()
        .map(|sample| f32::from(*sample) / 32_768.0)
        .collect::<Vec<_>>();
    let mut detector = EnergyDetector;
    let mut runs: Vec<[u64; 2]> = Vec::new();
    for (index, frame) in samples.as_chunks::<FRAME_SAMPLES>().0.iter().enumerate() {
        let start = u64::try_from(index).expect("the frame index fits u64") * FRAME;
        if detector
            .classify(frame)
            .expect("the energy detector never fails")
        {
            match runs.last_mut() {
                Some(run) if run[1] == start => run[1] = start + FRAME,
                _ => runs.push([start, start + FRAME]),
            }
        }
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
fn speech_runs_cover_whole_frames_on_the_grid_from_sample_zero() {
    let speech = i16::MAX / 2;
    let mut pcm = vec![0; FRAME_SAMPLES * 5 + 100];
    pcm[FRAME_SAMPLES + 160..FRAME_SAMPLES * 3].fill(speech);
    pcm[FRAME_SAMPLES * 5..].fill(speech);
    assert_eq!(
        speech_runs(&pcm),
        [[FRAME, FRAME * 3]],
        "speech that starts inside a frame reads from that frame's start, and a partial last \
         frame is never classified"
    );
}
