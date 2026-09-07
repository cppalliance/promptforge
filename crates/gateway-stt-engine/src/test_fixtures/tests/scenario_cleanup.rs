use std::sync::Arc;

use super::*;

mod construction;
mod decode;

const WAIT: Duration = Duration::from_secs(1);

async fn run_blocked_decode(decoder: &ScriptedDecoder, engine: &Arc<SttEngine>, transcript: &str) {
    decoder.push_text(transcript);
    let decode = decoder
        .with_next_decode_blocked(
            WAIT,
            || {
                let engine = Arc::clone(engine);
                async move {
                    (tokio::spawn(async move {
                        engine
                            .decode(request(DecodeMode::Interim, vec![0.25], Vec::new(), ""))
                            .await
                    }),)
                }
            },
            |(decode,)| async { (decode,) },
        )
        .await
        .expect("follow-up decode reaches the blocked scenario");
    let (decode,) = decode;
    assert_eq!(
        tokio::time::timeout(WAIT, decode)
            .await
            .expect("released decode completes")
            .expect("decode task joins")
            .expect("scripted decode succeeds"),
        transcript
    );
}

fn run_timed_out_construction(decoder: &ScriptedDecoder) {
    let factory = ScriptedModelFactory::new(decoder.clone());
    let timeout = policy().with_startup_timeout(Duration::from_millis(20));
    let (result, ()) = factory
        .with_construction_blocked(
            WAIT,
            WAIT,
            |factory| SttEngine::new(factory, timeout),
            || (),
        )
        .expect("construction reaches the blocked scenario and its bounded result");
    assert!(matches!(
        result.expect_err("parked construction times out"),
        TranscribeError::InterimStartupTimedOut
    ));
    assert!(decoder.wait_until_worker_dropped(WAIT));
}
