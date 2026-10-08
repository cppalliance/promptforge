//! Tests for PCM16 Base64 decoding, resampling, and commit validation.

use base64::Engine as _;
use serde::Deserialize;

use super::{
    AudioBuffer, AudioError, MAX_APPEND_AUDIO_BYTES, MAX_BUFFERED_AUDIO_BYTES, MIN_COMMIT_SAMPLES,
    Resampler24To16, decode_base64, resampled_inputs, resampled_sample,
};

#[derive(Deserialize)]
struct PcmFixture {
    encoding: String,
    sample_rate_hz: u32,
    channels: u8,
    samples: Vec<i16>,
    bytes: Vec<u8>,
    base64: String,
}

fn fixture() -> PcmFixture {
    serde_json::from_str(include_str!("../tests/fixtures/audio/pcm16le-24khz.json"))
        .expect("audio fixture parses")
}

fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

fn encoded(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[test]
fn language_neutral_fixture_pins_exact_pcm16le_bytes() {
    let fixture = fixture();
    assert_eq!(fixture.encoding, "pcm_s16le");
    assert_eq!(fixture.sample_rate_hz, 24_000);
    assert_eq!(fixture.channels, 1);
    assert_eq!(fixture.bytes, pcm_bytes(&fixture.samples));
    assert_eq!(
        decode_base64(&fixture.base64).expect("fixture Base64 decodes"),
        fixture.bytes
    );
}

#[test]
fn base64_rejects_invalid_and_noncanonical_encodings_and_decoded_oversize() {
    assert!(matches!(
        decode_base64("%%%"),
        Err(AudioError::InvalidBase64(_))
    ));
    assert!(matches!(
        decode_base64("YQ"),
        Err(AudioError::InvalidBase64(_))
    ));
    for alias in [
        "YR==", "YS==", "YT==", "YU==", "YV==", "YW==", "YX==", "YY==", "YZ==", "Ya==", "Yb==",
        "Yc==", "Yd==", "Ye==", "Yf==", "YWJ=", "YWK=", "YWL=", "YQ===", "YWI==", "YWJj=",
    ] {
        assert!(
            matches!(decode_base64(alias), Err(AudioError::InvalidBase64(_))),
            "{alias}"
        );
    }
    let at_limit = vec![0_u8; MAX_APPEND_AUDIO_BYTES];
    assert_eq!(
        decode_base64(&encoded(&at_limit))
            .expect("the exact append limit decodes")
            .len(),
        MAX_APPEND_AUDIO_BYTES
    );
    let over_limit = vec![0_u8; MAX_APPEND_AUDIO_BYTES + 1];
    assert_eq!(
        decode_base64(&encoded(&over_limit)),
        Err(AudioError::AppendTooLarge {
            max_bytes: MAX_APPEND_AUDIO_BYTES,
        })
    );
}

#[test]
fn invalid_base64_returns_the_decode_failure_as_its_source() {
    use std::error::Error as _;

    let error = decode_base64("%%%").expect_err("invalid Base64 is rejected");
    let AudioError::InvalidBase64(source) = &error else {
        panic!("invalid Base64 names its cause: {error}");
    };
    assert_eq!(*source, base64::DecodeError::InvalidByte(0, b'%'));
    assert!(
        error
            .source()
            .is_some_and(<dyn std::error::Error + 'static>::is::<base64::DecodeError>),
        "the error chain includes the decoder failure"
    );
}

#[test]
fn held_odd_byte_and_resampling_match_unsplit_input() {
    let input = (0..MIN_COMMIT_SAMPLES + 5)
        .map(|index| i16::try_from(index % 1024).expect("fixture sample fits") - 512)
        .collect::<Vec<_>>();
    let bytes = pcm_bytes(&input);
    let mut whole = AudioBuffer::default();
    whole
        .append_base64(&encoded(&bytes))
        .expect("whole append succeeds");
    let expected = whole.commit().expect("whole commit succeeds");
    for split in [1, 2, 3, 47, bytes.len() - 1] {
        let mut chunked = AudioBuffer::default();
        chunked
            .append_base64(&encoded(&bytes[..split]))
            .expect("first chunk succeeds");
        chunked
            .append_base64(&encoded(&bytes[split..]))
            .expect("second chunk succeeds");
        let actual = chunked.commit().expect("chunked commit succeeds");
        assert_eq!(actual.samples(), expected.samples(), "split at {split}");
        assert_eq!(actual.input_samples(), expected.input_samples());
        assert!((actual.duration_seconds() - expected.duration_seconds()).abs() < f64::EPSILON);
    }
}

const INPUT_RATE_HZ: f64 = 24_000.0;
const OUTPUT_RATE_HZ: f64 = 16_000.0;
/// Outputs skipped before a measurement, past the filter's start transient.
const SETTLE_OUTPUTS: usize = 160;
/// 100 ms at 16 kHz: whole cycles of every measured frequency, so one DFT
/// bin reads a tone's amplitude without leakage from the others.
const MEASURED_OUTPUTS: usize = 1_600;
/// -55 dB, a margin under the filter's 60 dB stopband.
const STOPBAND_AMPLITUDE: f64 = 0.001_778;

/// The 16 kHz outputs of a 24 kHz tone long enough to settle and measure.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the tone is computed in f64 and pushed as an f32 sample"
)]
fn resampled_tone(frequency_hz: f64, amplitude: f64) -> Vec<f32> {
    let mut resampler = Resampler24To16::default();
    for index in 0..4_000_u32 {
        let time = f64::from(index) / INPUT_RATE_HZ;
        resampler.push((amplitude * (std::f64::consts::TAU * frequency_hz * time).sin()) as f32);
    }
    resampler.flush();
    resampler.output
}

/// The amplitude of `frequency_hz` in the measured span of 16 kHz `output`.
fn amplitude(output: &[f32], frequency_hz: f64) -> f64 {
    let span = &output[SETTLE_OUTPUTS..SETTLE_OUTPUTS + MEASURED_OUTPUTS];
    let (real, imaginary) =
        span.iter()
            .zip(0_u32..)
            .fold((0.0, 0.0), |(real, imaginary), (sample, index)| {
                let phase =
                    std::f64::consts::TAU * frequency_hz * f64::from(index) / OUTPUT_RATE_HZ;
                let sample = f64::from(*sample);
                (
                    real + sample * phase.cos(),
                    imaginary - sample * phase.sin(),
                )
            });
    2.0 * real.hypot(imaginary) / f64::from(u32::try_from(span.len()).expect("span fits u32"))
}

#[test]
fn a_position_resampled_alone_matches_the_stream_bit_for_bit() {
    let input =
        |index: u64| f32::from(u16::try_from(index * 7_919 % 4_096).expect("fits")) / 4_096.0;
    let mut resampler = Resampler24To16::default();
    for index in 0..301 {
        resampler.push(input(index));
    }
    resampler.flush();
    for (position, sample) in (0_u64..).zip(&resampler.output) {
        assert_eq!(
            resampled_sample(position, input).to_bits(),
            sample.to_bits(),
            "output {position}"
        );
        assert!(
            *resampled_inputs(position).end() <= 300,
            "output {position}"
        );
    }
}

#[test]
fn each_odd_output_waits_for_the_input_after_it_or_the_flush() {
    for pushed in 1..=12_usize {
        let mut resampler = Resampler24To16::default();
        for _ in 0..pushed {
            resampler.push(0.25);
        }
        let pending = usize::from(pushed % 3 == 2);
        assert_eq!(
            resampler.output.len() + pending,
            (pushed * 2).div_ceil(3),
            "{pushed}"
        );
        resampler.flush();
        assert_eq!(resampler.output.len(), (pushed * 2).div_ceil(3), "{pushed}");
    }
}

#[test]
fn a_passband_tone_passes_at_unit_gain_one_millisecond_late() {
    let output = resampled_tone(1_000.0, 0.5);
    for (index, sample) in output
        .iter()
        .enumerate()
        .skip(SETTLE_OUTPUTS)
        .take(MEASURED_OUTPUTS)
    {
        let late = f64::from(u32::try_from(index).expect("index fits u32")) - 16.0;
        let expected = 0.5 * (std::f64::consts::TAU * 1_000.0 * late / OUTPUT_RATE_HZ).sin();
        assert!(
            (f64::from(*sample) - expected).abs() < 1e-3,
            "output {index} is {sample}, expected {expected}"
        );
    }
}

#[test]
fn a_tone_near_the_passband_edge_keeps_its_level_and_casts_no_image() {
    let output = resampled_tone(6_000.0, 1.0);
    let level = amplitude(&output, 6_000.0);
    assert!(
        (0.9886..1.0116).contains(&level),
        "6 kHz stays within 0.1 dB, got {level}"
    );
    let image = amplitude(&output, 2_000.0);
    assert!(
        image < STOPBAND_AMPLITUDE,
        "6 kHz casts a 2 kHz image of {image}"
    );
}

#[test]
fn a_tone_above_the_output_nyquist_rate_does_not_fold_into_speech() {
    let output = resampled_tone(10_000.0, 1.0);
    for heard_hz in [6_000.0, 2_000.0] {
        let folded = amplitude(&output, heard_hz);
        assert!(
            folded < STOPBAND_AMPLITUDE,
            "10 kHz folds to {heard_hz} Hz at {folded}"
        );
    }
}

#[test]
fn commit_resamples_constant_audio_unchanged() {
    let samples = vec![i16::MIN; MIN_COMMIT_SAMPLES + 1];
    let mut audio = AudioBuffer::default();
    audio
        .append_base64(&encoded(&pcm_bytes(&samples)))
        .expect("append succeeds");
    let committed = audio.commit().expect("commit succeeds");
    assert_eq!(committed.samples().len(), (samples.len() * 2).div_ceil(3));
    assert!(
        committed
            .samples()
            .iter()
            .all(|sample| (*sample - -1.0).abs() < 1e-5),
        "unit-sum phases pass a constant within f32 rounding"
    );
}

#[test]
fn clear_discards_odd_byte_resampler_and_duration_state() {
    let mut reused = AudioBuffer::default();
    reused
        .append_base64(&encoded(&[0x7f, 0x01, 0x80]))
        .expect("partial append succeeds");
    reused.clear();
    let clean_samples = vec![123_i16; MIN_COMMIT_SAMPLES];
    let clean_bytes = pcm_bytes(&clean_samples);
    reused
        .append_base64(&encoded(&clean_bytes))
        .expect("append after clear succeeds");
    let mut fresh = AudioBuffer::default();
    fresh
        .append_base64(&encoded(&clean_bytes))
        .expect("fresh append succeeds");
    assert_eq!(
        reused.commit().expect("reused commit succeeds"),
        fresh.commit().expect("fresh commit succeeds")
    );
}

#[test]
fn duration_uses_complete_input_samples_and_commit_rejects_odd_pcm() {
    let mut audio = AudioBuffer::default();
    let samples = vec![0_i16; MIN_COMMIT_SAMPLES];
    let mut bytes = pcm_bytes(&samples);
    bytes.push(0xaa);
    audio
        .append_base64(&encoded(&bytes))
        .expect("append holds the odd byte");
    assert!((audio.buffered_duration_seconds() - 0.1).abs() < f64::EPSILON);
    assert_eq!(audio.commit(), Err(AudioError::IncompletePcm16Sample));
}

#[test]
fn commit_enforces_the_minimum_duration() {
    let mut audio = AudioBuffer::default();
    audio
        .append_base64(&encoded(&pcm_bytes(&vec![0_i16; MIN_COMMIT_SAMPLES - 1])))
        .expect("short audio appends");

    assert_eq!(
        audio.commit(),
        Err(AudioError::CommitTooShort { minimum_ms: 100 })
    );
}

#[test]
fn buffered_audio_accepts_thirty_seconds_and_rejects_one_more_sample() {
    let exact = vec![0_u8; MAX_BUFFERED_AUDIO_BYTES];
    let mut audio = AudioBuffer::default();
    audio
        .append_base64(&encoded(&exact))
        .expect("thirty seconds is accepted");
    assert!((audio.buffered_duration_seconds() - 30.0).abs() < f64::EPSILON);

    assert_eq!(
        audio.append_base64(&encoded(&0_i16.to_le_bytes())),
        Err(AudioError::BufferTooLong {
            maximum_seconds: 30,
        })
    );
}

#[test]
fn lifetime_duration_survives_output_drains_past_thirty_seconds() {
    let one_second = encoded(&pcm_bytes(&vec![123_i16; 24_000]));
    let mut audio = AudioBuffer::default();
    let mut retained = 0;
    for _ in 0..31 {
        audio
            .append_base64(&one_second)
            .expect("lifetime input is not a retained-PCM limit");
        retained += audio.take_resampled().len();
    }
    let committed = audio.commit().expect("lifetime input commits");

    assert_eq!(committed.input_samples(), 31_u64 * 24_000);
    assert_eq!(retained + committed.samples().len(), 31 * 16_000);
    assert!((committed.duration_seconds() - 31.0).abs() < f64::EPSILON);
}

#[test]
fn odd_byte_and_resampler_continuity_survive_repeated_output_drains() {
    let input = (0..31 * 24_000 + 5)
        .map(|index| i16::try_from(index % 2_048).expect("fixture sample fits") - 1_024)
        .collect::<Vec<_>>();
    let bytes = pcm_bytes(&input);

    let mut coarse = AudioBuffer::default();
    let coarse_split = 29 * 24_000 * 2 + 1;
    coarse
        .append_base64(&encoded(&bytes[..coarse_split]))
        .expect("coarse odd chunk appends");
    let mut expected = coarse.take_resampled();
    coarse
        .append_base64(&encoded(&bytes[coarse_split..]))
        .expect("the coarse odd byte completes its sample");
    let committed = coarse.commit().expect("coarse stream commits");
    expected.extend_from_slice(committed.samples());

    let mut compacted = AudioBuffer::default();
    let mut actual = Vec::new();
    let mut start = 0;
    for end in [1, 9_601, 48_003, 240_007, bytes.len()] {
        compacted
            .append_base64(&encoded(&bytes[start..end]))
            .expect("compacted chunk appends");
        actual.extend(compacted.take_resampled());
        start = end;
    }
    let committed = compacted.commit().expect("compacted stream commits");
    actual.extend_from_slice(committed.samples());

    assert_eq!(actual, expected);
    assert_eq!(committed.input_samples(), 31_u64 * 24_000 + 5);
}

#[test]
fn odd_byte_and_drained_output_cross_the_u64_lifetime_boundary_without_phase_overflow() {
    let samples = [123_i16, -456, 789];
    let bytes = pcm_bytes(&samples);
    let mut near_limit = AudioBuffer {
        input_samples: u64::MAX - 3,
        ..AudioBuffer::default()
    };
    near_limit
        .append_base64(&encoded(&bytes[..1]))
        .expect("the odd byte is held at the lifetime boundary");
    assert!(near_limit.take_resampled().is_empty());
    near_limit
        .append_base64(&encoded(&bytes[1..]))
        .expect("the held byte's sample reaches the exact lifetime limit");
    let actual = near_limit.take_resampled();

    let mut ordinary = AudioBuffer::default();
    ordinary
        .append_base64(&encoded(&bytes[..1]))
        .expect("ordinary odd byte is held");
    assert!(ordinary.take_resampled().is_empty());
    ordinary
        .append_base64(&encoded(&bytes[1..]))
        .expect("the ordinary held byte's sample appends");
    assert_eq!(actual, ordinary.take_resampled());
    assert_eq!(near_limit.input_samples, u64::MAX);
    assert_eq!(
        near_limit.append_base64(&encoded(&0_i16.to_le_bytes())),
        Err(AudioError::BufferTooLong {
            maximum_seconds: 30,
        })
    );
    assert!(near_limit.take_resampled().is_empty());
}
