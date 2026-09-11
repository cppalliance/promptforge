use base64::Engine as _;

const INPUT_SAMPLE_RATE: u64 = 24_000;
const INPUT_SAMPLE_RATE_USIZE: usize = 24_000;
const BYTES_PER_SAMPLE: usize = size_of::<i16>();
const MAX_BUFFERED_SECONDS: usize = 30;
const MIN_COMMIT_MILLISECONDS: usize = 100;

pub(super) const MAX_APPEND_AUDIO_BYTES: usize = 15 * 1024 * 1024;
pub(super) const MAX_BUFFERED_AUDIO_BYTES: usize =
    INPUT_SAMPLE_RATE_USIZE * BYTES_PER_SAMPLE * MAX_BUFFERED_SECONDS;
pub(super) const MIN_COMMIT_SAMPLES: usize =
    INPUT_SAMPLE_RATE_USIZE * MIN_COMMIT_MILLISECONDS / 1_000;

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub(super) enum AudioError {
    #[error("audio must be canonical padded Base64")]
    #[non_exhaustive]
    InvalidBase64(#[source] base64::DecodeError),
    #[error("decoded audio exceeds the {max_bytes} byte append limit")]
    AppendTooLarge { max_bytes: usize },
    #[error("PCM16 audio ended with an incomplete sample")]
    IncompletePcm16Sample,
    #[error("audio buffer exceeds {maximum_seconds} seconds")]
    BufferTooLong { maximum_seconds: usize },
    #[error("committed audio must be at least {minimum_ms} milliseconds")]
    CommitTooShort { minimum_ms: usize },
}

#[derive(Debug, PartialEq)]
pub(super) struct CommittedAudio {
    samples: Vec<f32>,
    input_samples: u64,
}

impl CommittedAudio {
    #[cfg(test)]
    pub(super) fn samples(&self) -> &[f32] {
        &self.samples
    }

    #[cfg(test)]
    pub(super) const fn input_samples(&self) -> u64 {
        self.input_samples
    }

    #[allow(clippy::cast_precision_loss)]
    pub(super) fn duration_seconds(&self) -> f64 {
        self.input_samples as f64 / INPUT_SAMPLE_RATE as f64
    }

    pub(super) fn into_samples(self) -> Vec<f32> {
        self.samples
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct AudioBuffer {
    input_bytes: usize,
    pub(super) input_samples: u64,
    odd_byte: Option<u8>,
    resampler: Resampler24To16,
}

impl AudioBuffer {
    pub(super) fn append_base64(&mut self, payload: &str) -> Result<(), AudioError> {
        let bytes = decode_base64(payload)?;
        let next_bytes =
            self.input_bytes
                .checked_add(bytes.len())
                .ok_or(AudioError::BufferTooLong {
                    maximum_seconds: MAX_BUFFERED_SECONDS,
                })?;
        if next_bytes > MAX_BUFFERED_AUDIO_BYTES {
            return Err(AudioError::BufferTooLong {
                maximum_seconds: MAX_BUFFERED_SECONDS,
            });
        }
        let carried = usize::from(self.odd_byte.is_some());
        let complete_samples = (bytes.len() + carried) / BYTES_PER_SAMPLE;
        let _ = self
            .input_samples
            .checked_add(u64::try_from(complete_samples).map_err(|_| {
                AudioError::BufferTooLong {
                    maximum_seconds: MAX_BUFFERED_SECONDS,
                }
            })?)
            .ok_or(AudioError::BufferTooLong {
                maximum_seconds: MAX_BUFFERED_SECONDS,
            })?;

        self.input_bytes = next_bytes;
        let mut bytes = bytes.into_iter();
        if let Some(low) = self.odd_byte.take() {
            if let Some(high) = bytes.next() {
                self.push_sample(i16::from_le_bytes([low, high]));
            } else {
                self.odd_byte = Some(low);
                return Ok(());
            }
        }

        while let Some(low) = bytes.next() {
            if let Some(high) = bytes.next() {
                self.push_sample(i16::from_le_bytes([low, high]));
            } else {
                self.odd_byte = Some(low);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn commit(&mut self) -> Result<CommittedAudio, AudioError> {
        self.validate_commit()?;
        Ok(self.commit_validated())
    }

    pub(super) fn commit_validated(&mut self) -> CommittedAudio {
        self.resampler.flush();
        let samples = std::mem::take(&mut self.resampler.output);
        let input_samples = self.input_samples;
        self.clear();
        CommittedAudio {
            samples,
            input_samples,
        }
    }

    pub(super) fn validate_commit(&self) -> Result<(), AudioError> {
        if self.odd_byte.is_some() {
            return Err(AudioError::IncompletePcm16Sample);
        }
        if self.input_samples < MIN_COMMIT_SAMPLES as u64 {
            return Err(AudioError::CommitTooShort {
                minimum_ms: MIN_COMMIT_MILLISECONDS,
            });
        }
        Ok(())
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn take_resampled(&mut self) -> Vec<f32> {
        self.input_bytes = usize::from(self.odd_byte.is_some());
        let mut output = std::mem::take(&mut self.resampler.output);
        output.shrink_to_fit();
        output
    }

    #[cfg(test)]
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn buffered_duration_seconds(&self) -> f64 {
        self.input_samples as f64 / INPUT_SAMPLE_RATE as f64
    }

    fn push_sample(&mut self, sample: i16) {
        self.resampler.push(f32::from(sample) / 32_768.0);
        self.input_samples += 1;
    }
}

pub(super) fn decode_base64(payload: &str) -> Result<Vec<u8>, AudioError> {
    const MAX_BASE64_CHARS: usize = MAX_APPEND_AUDIO_BYTES.div_ceil(3) * 4;
    if payload.len() > MAX_BASE64_CHARS {
        return Err(AudioError::AppendTooLarge {
            max_bytes: MAX_APPEND_AUDIO_BYTES,
        });
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(AudioError::InvalidBase64)?;
    if decoded.len() > MAX_APPEND_AUDIO_BYTES {
        return Err(AudioError::AppendTooLarge {
            max_bytes: MAX_APPEND_AUDIO_BYTES,
        });
    }
    Ok(decoded)
}

#[derive(Clone, Debug, Default)]
struct Resampler24To16 {
    phase: u8,
    previous: Option<f32>,
    output: Vec<f32>,
}

impl Resampler24To16 {
    fn push(&mut self, sample: f32) {
        match self.phase {
            0 => self.emit(sample),
            1 => {}
            2 => self.emit(self.previous.unwrap_or(sample).midpoint(sample)),
            _ => unreachable!("the resampler phase is modulo three"),
        }
        self.previous = Some(sample);
        self.phase = if self.phase == 2 { 0 } else { self.phase + 1 };
    }

    fn flush(&mut self) {
        if self.phase == 2
            && let Some(sample) = self.previous
        {
            self.emit(sample);
        }
    }

    fn emit(&mut self, sample: f32) {
        self.output.push(sample);
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use serde::Deserialize;

    use super::{
        AudioBuffer, AudioError, MAX_APPEND_AUDIO_BYTES, MAX_BUFFERED_AUDIO_BYTES,
        MIN_COMMIT_SAMPLES, Resampler24To16, decode_base64,
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
    fn invalid_base64_carries_the_decode_failure_as_its_source() {
        use std::error::Error as _;

        let error = decode_base64("%%%").expect_err("invalid Base64 is rejected");
        let AudioError::InvalidBase64(source) = &error else {
            panic!("invalid Base64 names its cause: {error}");
        };
        assert_eq!(*source, base64::DecodeError::InvalidByte(0, b'%'));
        assert!(
            error
                .source()
                .is_some_and(<(dyn std::error::Error + 'static)>::is::<base64::DecodeError>),
            "the error chain carries the decoder failure"
        );
    }

    #[test]
    fn odd_byte_carry_and_resampling_match_unsplit_input() {
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

    #[test]
    fn resampler_uses_one_continuous_linear_timeline() {
        let mut resampler = Resampler24To16::default();
        for sample in [0.0, 2.0, 4.0, 6.0, 8.0] {
            resampler.push(sample);
        }
        resampler.flush();
        assert_eq!(resampler.output, [0.0, 3.0, 6.0, 8.0]);
    }

    #[test]
    fn commit_flushes_the_last_resampler_position() {
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
                .all(|sample| (*sample - -1.0).abs() < f32::EPSILON)
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
            .expect("append carries the odd byte");
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
            .expect("coarse carry completes");
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
            .expect("the odd byte is carried at the lifetime boundary");
        assert!(near_limit.take_resampled().is_empty());
        near_limit
            .append_base64(&encoded(&bytes[1..]))
            .expect("the carried sample reaches the exact lifetime limit");
        let actual = near_limit.take_resampled();

        let mut ordinary = AudioBuffer::default();
        ordinary
            .append_base64(&encoded(&bytes[..1]))
            .expect("ordinary odd byte is carried");
        assert!(ordinary.take_resampled().is_empty());
        ordinary
            .append_base64(&encoded(&bytes[1..]))
            .expect("ordinary carried samples append");
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
}
