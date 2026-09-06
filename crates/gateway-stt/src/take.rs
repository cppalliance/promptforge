//! Per-take speech state and finalization ownership.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use gateway_stt_engine::{SttEngine, TranscribeError};
use tokio::sync::{mpsc, oneshot};

use crate::segment::Segmenter;

#[derive(Debug, PartialEq, Eq)]
struct AgreementSnapshot {
    agreed: String,
    tentative: String,
}

#[derive(Debug, Default)]
struct LocalAgreement {
    previous: String,
}

impl LocalAgreement {
    fn observe(&mut self, hypothesis: &str) -> AgreementSnapshot {
        let agreed_end = if self.previous.is_empty() {
            0
        } else {
            matching_token_prefix_end(&self.previous, hypothesis)
        };
        self.previous.clear();
        self.previous.push_str(hypothesis);
        AgreementSnapshot {
            agreed: hypothesis[..agreed_end].to_owned(),
            tentative: hypothesis[agreed_end..].to_owned(),
        }
    }
}

fn matching_token_prefix_end(previous: &str, current: &str) -> usize {
    let previous = token_spans(previous);
    let current = token_spans(current);
    previous
        .iter()
        .zip(&current)
        .take_while(|((left, _, _), (right, _, _))| left == right)
        .map(|(_, (_, _, end))| *end)
        .last()
        .unwrap_or(0)
}

fn token_spans(text: &str) -> Vec<(&str, usize, usize)> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (index, character) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        match (start, character.is_whitespace()) {
            (None, false) => start = Some(index),
            (Some(begin), true) => {
                tokens.push((&text[begin..index], begin, index));
                start = None;
            }
            _ => {}
        }
    }
    tokens
}

fn after_token_prefix(text: &str, tokens: usize) -> &str {
    if tokens == 0 {
        return text;
    }
    token_spans(text)
        .get(tokens - 1)
        .map_or("", |(_, _, end)| &text[*end..])
}

fn append_transcript(text: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(piece);
}

fn tail(buffer: &[f32], window: usize) -> &[f32] {
    &buffer[buffer.len().saturating_sub(window)..]
}

#[derive(Debug, Default)]
struct FinalizedState {
    text: String,
    failure: Option<String>,
    samples: usize,
}

#[derive(Debug, Default)]
struct InterimState {
    agreement: LocalAgreement,
    promoted: String,
    agreement_finalized: String,
    committed: String,
    last_committed: String,
    last_tentative: String,
    finalized_at_last_speech: String,
}

#[derive(Debug, Default)]
struct TakeState {
    buffer: Mutex<Vec<f32>>,
    segmenter: Mutex<Segmenter>,
    finalized: Mutex<FinalizedState>,
    interim: Mutex<InterimState>,
}

impl TakeState {
    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn finalized(&self) -> String {
        Self::lock(&self.finalized).text.clone()
    }

    fn record_finalized(&self, result: Result<String, TranscribeError>, samples: Option<usize>) {
        let mut state = Self::lock(&self.finalized);
        match result {
            Ok(text) if state.failure.is_none() => {
                append_transcript(&mut state.text, &text);
                if let Some(samples) = samples {
                    state.samples = samples;
                }
            }
            Err(error) if state.failure.is_none() => state.failure = Some(error.to_string()),
            Ok(_) | Err(_) => {}
        }
    }

    fn record_failure(&self, failure: String) {
        let mut state = Self::lock(&self.finalized);
        if state.failure.is_none() {
            state.failure = Some(failure);
        }
    }

    fn has_failure(&self) -> bool {
        Self::lock(&self.finalized).failure.is_some()
    }

    fn finalized_samples(&self) -> usize {
        Self::lock(&self.finalized).samples
    }

    fn completion(&self) -> Result<String, String> {
        let mut state = Self::lock(&self.finalized);
        match state.failure.take() {
            Some(failure) => Err(failure),
            None => Ok(state.text.clone()),
        }
    }
}

#[derive(Debug)]
enum FinalCommand {
    Segment {
        samples: Vec<f32>,
        end: usize,
    },
    Complete {
        tail: Vec<f32>,
        reply: oneshot::Sender<Result<String, String>>,
    },
}

#[derive(Debug)]
struct FinalPipeline {
    commands: mpsc::UnboundedSender<FinalCommand>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FinalPipeline {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// All mutable and immutable state belonging to one speech take.
#[derive(Debug)]
pub(crate) struct Take {
    guidance: Arc<[String]>,
    state: Arc<TakeState>,
    final_pipeline: Option<FinalPipeline>,
}

impl Take {
    pub(crate) fn new(guidance: Vec<String>, engine: Option<Arc<SttEngine>>) -> Self {
        let guidance = Arc::<[String]>::from(guidance);
        let state = Arc::new(TakeState::default());
        let final_pipeline = engine
            .filter(|engine| engine.has_final_pass())
            .map(|engine| spawn_final_pipeline(engine, Arc::clone(&guidance), Arc::clone(&state)));
        Self {
            guidance,
            state,
            final_pipeline,
        }
    }

    #[cfg(test)]
    fn without_final(guidance: Vec<String>) -> Self {
        Self::new(guidance, None)
    }

    pub(crate) fn guidance(&self) -> &[String] {
        &self.guidance
    }

    pub(crate) fn append(&self, samples: &[f32]) {
        TakeState::lock(&self.state.buffer).extend_from_slice(samples);
    }

    pub(crate) fn submit_closed_segments(&self) {
        let Some(pipeline) = &self.final_pipeline else {
            return;
        };
        loop {
            let segment = {
                let buffer = TakeState::lock(&self.state.buffer);
                TakeState::lock(&self.state.segmenter)
                    .poll(&buffer)
                    .map(|range| (buffer[range.clone()].to_vec(), range.end))
            };
            let Some((samples, end)) = segment else {
                break;
            };
            if pipeline
                .commands
                .send(FinalCommand::Segment { samples, end })
                .is_err()
            {
                self.state
                    .record_failure("final transcription pipeline exited".to_owned());
                break;
            }
        }
    }

    pub(crate) fn consumed(&self) -> usize {
        TakeState::lock(&self.state.segmenter).consumed()
    }

    pub(crate) fn uncommitted_snapshot(&self, window_samples: usize) -> Vec<f32> {
        let consumed = self.consumed();
        let buffer = TakeState::lock(&self.state.buffer);
        let uncommitted = &buffer[consumed.min(buffer.len())..];
        tail(uncommitted, window_samples).to_vec()
    }

    pub(crate) fn fallback_snapshot(&self, window_samples: usize) -> Vec<f32> {
        let finalized = self.state.finalized_samples();
        let buffer = TakeState::lock(&self.state.buffer);
        let pending = &buffer[finalized.min(buffer.len())..];
        tail(pending, window_samples).to_vec()
    }

    pub(crate) fn fallback_len(&self) -> usize {
        let finalized = self.state.finalized_samples();
        TakeState::lock(&self.state.buffer)
            .len()
            .saturating_sub(finalized)
    }

    pub(crate) fn finalized(&self) -> String {
        self.state.finalized()
    }

    pub(crate) fn fallback_transcript(&self, tail: &str) -> String {
        let mut transcript = self.finalized();
        append_transcript(&mut transcript, tail);
        transcript
    }

    #[cfg(test)]
    fn record_finalized(&self, result: Result<String, TranscribeError>) {
        self.state.record_finalized(result, None);
    }

    #[cfg(test)]
    fn record_failure(&self, failure: impl Into<String>) {
        self.state.record_failure(failure.into());
    }

    #[cfg(test)]
    fn take_failure(&self) -> Option<String> {
        TakeState::lock(&self.state.finalized).failure.take()
    }

    pub(crate) fn next_interim(&self, hypothesis: &str) -> Option<(String, String)> {
        let finalized = self.finalized();
        let mut state = TakeState::lock(&self.state.interim);
        if state.agreement_finalized != finalized {
            let finalized_delta = finalized
                .strip_prefix(&state.agreement_finalized)
                .unwrap_or_default();
            let unpromoted =
                after_token_prefix(finalized_delta, token_spans(&state.promoted).len());
            append_transcript(&mut state.committed, unpromoted.trim());
            state.agreement = LocalAgreement::default();
            state.promoted.clear();
            state.agreement_finalized.clone_from(&finalized);
        }
        let suffix_start = matching_token_prefix_end(&state.promoted, hypothesis);
        let suffix = hypothesis[suffix_start..].trim_start();
        let agreement = state.agreement.observe(suffix);
        let tentative = agreement.tentative.trim_start().to_owned();
        state.agreement.previous.clone_from(&tentative);
        let promoted = agreement.agreed.trim();
        append_transcript(&mut state.promoted, promoted);
        append_transcript(&mut state.committed, promoted);
        if !hypothesis.is_empty() {
            state.finalized_at_last_speech.clone_from(&finalized);
        } else if finalized.len() <= state.finalized_at_last_speech.len() {
            return None;
        }
        let committed = state.committed.clone();
        if committed == state.last_committed && tentative == state.last_tentative {
            return None;
        }
        state.last_committed.clone_from(&committed);
        state.last_tentative.clone_from(&tentative);
        Some((committed, tentative))
    }

    pub(crate) async fn complete(&self) -> Option<Result<String, String>> {
        let pipeline = self.final_pipeline.as_ref()?;
        let tail = {
            let consumed = self.consumed();
            let buffer = TakeState::lock(&self.state.buffer);
            buffer[consumed.min(buffer.len())..].to_vec()
        };
        let (reply, reply_rx) = oneshot::channel();
        if pipeline
            .commands
            .send(FinalCommand::Complete { tail, reply })
            .is_err()
        {
            return Some(Err("final transcription pipeline exited".to_owned()));
        }
        Some(
            reply_rx
                .await
                .unwrap_or_else(|_| Err("final transcription pipeline exited".to_owned())),
        )
    }
}

fn spawn_final_pipeline(
    engine: Arc<SttEngine>,
    guidance: Arc<[String]>,
    state: Arc<TakeState>,
) -> FinalPipeline {
    let (commands, receiver) = mpsc::unbounded_channel();
    let task = tokio::spawn(run_final_pipeline(
        receiver,
        guidance,
        state,
        move |samples, guidance, finalized| {
            let engine = Arc::clone(&engine);
            async move {
                if !engine.has_final_pass() {
                    return None;
                }
                Some(
                    engine
                        .decode(gateway_stt_engine::DecodeRequest::new(
                            gateway_stt_engine::DecodeMode::Final,
                            samples,
                            guidance,
                            finalized,
                        ))
                        .await,
                )
            }
        },
    ));
    FinalPipeline { commands, task }
}

async fn run_final_pipeline<D, F>(
    mut receiver: mpsc::UnboundedReceiver<FinalCommand>,
    guidance: Arc<[String]>,
    state: Arc<TakeState>,
    mut decode: D,
) where
    D: FnMut(Vec<f32>, Vec<String>, String) -> F,
    F: Future<Output = Option<Result<String, TranscribeError>>>,
{
    while let Some(command) = receiver.recv().await {
        let (samples, finalized_samples, completion) = match command {
            FinalCommand::Segment { samples, end } => (samples, Some(end), None),
            FinalCommand::Complete { tail, reply } => (tail, None, Some(reply)),
        };
        if !state.has_failure() {
            let finalized = state.finalized();
            match decode(samples, guidance.to_vec(), finalized).await {
                Some(result) => state.record_finalized(result, finalized_samples),
                None => {
                    state.record_failure("final transcription worker is unavailable".to_owned());
                }
            }
        }
        if let Some(reply) = completion {
            let _ = reply.send(state.completion());
            break;
        }
    }
    drop(guidance);
    drop(state);
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Weak};
    use std::time::Duration;

    use tokio::sync::{mpsc, oneshot};

    use super::{FinalCommand, LocalAgreement, Take, run_final_pipeline};

    #[test]
    fn tail_returns_the_trailing_window() {
        let buffer: Vec<f32> = (0u8..10).map(f32::from).collect();
        assert_eq!(super::tail(&buffer, 4), &[6.0, 7.0, 8.0, 9.0]);
        assert_eq!(super::tail(&buffer, 100), &buffer);
        assert_eq!(super::tail(&[], 4), &[] as &[f32]);
    }

    #[test]
    fn local_agreement_requires_two_hypotheses_and_preserves_whitespace() {
        let mut agreement = LocalAgreement::default();
        let first = agreement.observe("ask not what");
        assert_eq!(first.agreed, "");
        assert_eq!(first.tentative, "ask not what");

        let second = agreement.observe("ask not who");
        assert_eq!(second.agreed, "ask not");
        assert_eq!(second.tentative, " who");
        assert_eq!(
            format!("{}{}", second.agreed, second.tentative),
            "ask not who"
        );
    }

    #[test]
    fn production_interims_promote_locally_agreed_words() {
        let take = Take::without_final(Vec::new());
        assert_eq!(
            take.next_interim("ask not what"),
            Some((String::new(), "ask not what".to_owned()))
        );
        assert_eq!(
            take.next_interim("ask not who"),
            Some(("ask not".to_owned(), "who".to_owned()))
        );
        assert_eq!(
            take.next_interim("ask not who"),
            Some(("ask not who".to_owned(), String::new()))
        );
        assert_eq!(
            take.next_interim("ask not when"),
            Some(("ask not who".to_owned(), "when".to_owned()))
        );
    }

    #[test]
    fn finalization_preserves_a_divergent_promoted_prefix() {
        let take = Take::without_final(Vec::new());
        assert_eq!(
            take.next_interim("ask not your country"),
            Some((String::new(), "ask not your country".to_owned()))
        );
        let promoted = take
            .next_interim("ask not your country")
            .expect("the repeated hypothesis promotes its words")
            .0;
        assert_eq!(promoted, "ask not your country");

        take.record_finalized(Ok("ask not your kingdom".to_owned()));
        let committed = take
            .next_interim("new tail")
            .expect("speech after finalization emits another interim")
            .0;

        assert_eq!(committed, promoted);
        assert!(committed.starts_with("ask not your country"));
    }

    #[test]
    fn finalized_history_and_guidance_are_isolated_per_take() {
        let first = Take::without_final(vec!["MCP".to_owned()]);
        let second = Take::without_final(vec!["GGUF".to_owned()]);

        first.record_finalized(Ok("ask not".to_owned()));
        second.record_finalized(Ok("what you".to_owned()));

        assert_eq!(first.guidance(), ["MCP"]);
        assert_eq!(first.finalized(), "ask not");
        assert_eq!(second.guidance(), ["GGUF"]);
        assert_eq!(second.finalized(), "what you");
    }

    #[test]
    fn finalized_segments_aggregate_in_arrival_order() {
        let take = Take::without_final(Vec::new());
        take.record_finalized(Ok("ask not".to_owned()));
        take.record_finalized(Ok("what you can do".to_owned()));
        assert_eq!(take.finalized(), "ask not what you can do");
    }

    #[test]
    fn a_take_retains_its_first_final_failure() {
        let take = Take::without_final(Vec::new());
        take.record_failure("first");
        take.record_failure("second");
        let failure = take.take_failure().expect("the take owns its failure");
        assert_eq!(failure, "first");
    }

    #[test]
    fn tail_failure_fallback_preserves_successful_closed_segments() {
        let take = Take::without_final(Vec::new());
        take.record_finalized(Ok("successful segment".to_owned()));
        take.record_failure("tail failed");

        assert_eq!(take.state.completion(), Err("tail failed".to_owned()));
        assert_eq!(
            take.fallback_transcript("fallback tail"),
            "successful segment fallback tail"
        );
    }

    #[test]
    fn closed_segment_failure_does_not_duplicate_a_successful_tail_in_fallback() {
        let take = Take::without_final(Vec::new());
        take.record_failure("closed segment failed");
        take.record_finalized(Ok("successful tail".to_owned()));

        assert_eq!(
            take.state.completion(),
            Err("closed segment failed".to_owned())
        );
        assert_eq!(take.fallback_transcript("fallback tail"), "fallback tail");
    }

    #[tokio::test]
    async fn failed_segment_audio_remains_in_the_fallback_window() {
        let take = Take::without_final(Vec::new());
        let successful = vec![1.0; 4];
        let failed = vec![2.0; 3];
        let skipped = vec![3.0; 2];
        let tail = vec![4.0];
        take.append(
            &[
                successful.clone(),
                failed.clone(),
                skipped.clone(),
                tail.clone(),
            ]
            .concat(),
        );

        let (commands, receiver) = mpsc::unbounded_channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let decode_calls = Arc::clone(&calls);
        let task = tokio::spawn(run_final_pipeline(
            receiver,
            Arc::from([]),
            Arc::clone(&take.state),
            move |_, _, _| {
                let call = decode_calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    Some(match call {
                        0 => Ok("successful".to_owned()),
                        1 => return None,
                        _ => panic!("decoding must stop after the first failure"),
                    })
                }
            },
        ));
        commands
            .send(FinalCommand::Segment {
                samples: successful,
                end: 4,
            })
            .expect("the successful segment queues");
        commands
            .send(FinalCommand::Segment {
                samples: failed.clone(),
                end: 7,
            })
            .expect("the failed segment queues");
        commands
            .send(FinalCommand::Segment {
                samples: skipped.clone(),
                end: 9,
            })
            .expect("the skipped segment queues");
        let (reply, completion) = oneshot::channel();
        commands
            .send(FinalCommand::Complete {
                tail: tail.clone(),
                reply,
            })
            .expect("completion queues");

        assert!(
            completion
                .await
                .expect("the completion pipeline replies")
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("the completed pipeline terminates before the deadline")
            .expect("the completed pipeline task succeeds");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            take.fallback_snapshot(usize::MAX),
            [failed, skipped, tail].concat()
        );
    }

    #[tokio::test]
    async fn completed_pipeline_releases_its_retained_dependency() {
        let (commands, receiver) = mpsc::unbounded_channel();
        let state = Arc::new(super::TakeState::default());
        let retained = Arc::new(());
        let weak: Weak<()> = Arc::downgrade(&retained);
        let pipeline_retained = Arc::clone(&retained);
        let task = tokio::spawn(run_final_pipeline(
            receiver,
            Arc::from([]),
            state,
            move |_, _, _| {
                let retained = Arc::clone(&pipeline_retained);
                async move {
                    drop(retained);
                    Some(Ok(String::new()))
                }
            },
        ));
        drop(retained);
        let (reply, completion) = oneshot::channel();
        commands
            .send(FinalCommand::Complete {
                tail: Vec::new(),
                reply,
            })
            .expect("completion queues");

        assert_eq!(
            completion.await.expect("the completion pipeline replies"),
            Ok(String::new())
        );
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("the completed pipeline terminates before the deadline")
            .expect("the completed pipeline task succeeds");
        assert!(
            weak.upgrade().is_none(),
            "pipeline completion releases its retained engine-like dependency"
        );
    }
}
