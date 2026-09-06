use base64::Engine as _;

const INPUT_SAMPLE_RATE: usize = 24_000;
const OUTPUT_SAMPLE_RATE: usize = 16_000;
const BYTES_PER_SAMPLE: usize = size_of::<i16>();
const MAX_BUFFERED_SECONDS: usize = 30;
const MIN_COMMIT_MILLISECONDS: usize = 100;

pub(super) const MAX_APPEND_AUDIO_BYTES: usize = 15 * 1024 * 1024;
pub(super) const MAX_BUFFERED_AUDIO_BYTES: usize =
    INPUT_SAMPLE_RATE * BYTES_PER_SAMPLE * MAX_BUFFERED_SECONDS;
pub(super) const MIN_COMMIT_SAMPLES: usize = INPUT_SAMPLE_RATE * MIN_COMMIT_MILLISECONDS / 1_000;

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub(super) enum AudioError {
    #[error("audio must be canonical padded Base64")]
    InvalidBase64,
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
    input_samples: usize,
}

impl CommittedAudio {
    pub(super) fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub(super) const fn input_samples(&self) -> usize {
        self.input_samples
    }

    #[allow(clippy::cast_precision_loss)]
    pub(super) fn duration_seconds(&self) -> f64 {
        self.input_samples as f64 / INPUT_SAMPLE_RATE as f64
    }
}

#[derive(Debug, Default)]
pub(super) struct AudioBuffer {
    input_bytes: usize,
    input_samples: usize,
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
        if self.input_samples < MIN_COMMIT_SAMPLES {
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
        std::mem::take(&mut self.resampler.output)
    }

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
        .map_err(|_| AudioError::InvalidBase64)?;
    if decoded.len() > MAX_APPEND_AUDIO_BYTES {
        return Err(AudioError::AppendTooLarge {
            max_bytes: MAX_APPEND_AUDIO_BYTES,
        });
    }
    Ok(decoded)
}

#[derive(Debug, Default)]
struct Resampler24To16 {
    input_index: usize,
    next_output_twice: usize,
    previous: Option<f32>,
    output: Vec<f32>,
    output_samples: usize,
}

impl Resampler24To16 {
    fn push(&mut self, sample: f32) {
        let input_twice = self.input_index * 2;
        if self.next_output_twice == input_twice {
            self.emit(sample);
            self.next_output_twice += 3;
        } else if self.next_output_twice < input_twice {
            let previous = self.previous.unwrap_or(sample);
            self.emit(previous.midpoint(sample));
            self.next_output_twice += 3;
        }
        self.previous = Some(sample);
        self.input_index += 1;
    }

    fn flush(&mut self) {
        if self.next_output_twice < self.input_index * 2
            && let Some(previous) = self.previous
        {
            self.emit(previous);
            self.next_output_twice += 3;
        }
        debug_assert_eq!(
            self.output_samples,
            (self.input_index * OUTPUT_SAMPLE_RATE).div_ceil(INPUT_SAMPLE_RATE)
        );
    }

    fn emit(&mut self, sample: f32) {
        self.output.push(sample);
        self.output_samples += 1;
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
        assert_eq!(decode_base64("%%%"), Err(AudioError::InvalidBase64));
        assert_eq!(decode_base64("YQ"), Err(AudioError::InvalidBase64));
        for alias in [
            "YR==", "YS==", "YT==", "YU==", "YV==", "YW==", "YX==", "YY==", "YZ==", "Ya==", "Yb==",
            "Yc==", "Yd==", "Ye==", "Yf==", "YWJ=", "YWK=", "YWL=", "YQ===", "YWI==", "YWJj=",
        ] {
            assert_eq!(
                decode_base64(alias),
                Err(AudioError::InvalidBase64),
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
}
