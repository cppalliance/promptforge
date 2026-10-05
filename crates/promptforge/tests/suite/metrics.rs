//! Reading a call's metrics: a canned completion measures nothing until
//! metrics are attached.

use std::error::Error;

use promptforge::metrics::{CallMetrics, ClientTiming, Usage};
use promptforge::model::{Completion, CompletionResult};

#[test]
fn a_canned_completion_measures_nothing_until_metrics_are_attached() -> Result<(), Box<dyn Error>> {
    let reply = CompletionResult::Text("hi there".to_owned());
    let canned = Completion::from_result(reply, "canned")?;
    assert!(canned.metrics().is_none());
    let usage = Usage {
        prompt_tokens: 12,
        completion_tokens: 5,
        total_tokens: 17,
        cached_tokens: None,
        reasoning_tokens: None,
    };
    let client = ClientTiming {
        ttft_ms: None,
        mean_itl_ms: None,
        e2e_ms: 40.0,
    };
    let metrics = CallMetrics {
        usage: Some(usage),
        llama: None,
        vllm: None,
        client: Some(client),
    };
    let measured = canned.with_metrics(metrics.clone());
    assert_eq!(measured.metrics(), Some(&metrics));
    Ok(())
}
