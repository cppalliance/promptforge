//! The 24 kHz stream a native client sends.

use std::ops::Range;

use base64::Engine as _;

/// Encodes the 24 kHz PCM16 input that the production resampler turns back
/// into exactly `pcm[output]`: input `3k` carries sample `2k`, and inputs
/// `3k + 1` and `3k + 2`, whose midpoint the resampler emits, both carry
/// sample `2k + 1`.
pub(crate) fn pcm24_payload(pcm: &[i16], output: Range<u64>) -> String {
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
