//! Infer-round reporting: a handle-form `models.infer` round reports
//! `model_turn_completed`, then `thinking` when the backend supplied
//! reasoning, then one `assistant_reply` with `origin = Infer`.

use promptforge_types::event::ReplyOrigin;
use promptforge_types::metrics::CallMetrics;

use super::*;

/// Records an infer round's boundary observations and content hooks as one
/// line each, so the whole sequence is asserted rather than a count: the
/// infer turn must read `model_turn_completed` -> (`thinking` when the
/// backend supplied reasoning) -> `assistant_reply` with `origin =
/// Infer`.
#[derive(Default)]
struct InferRoundRecorder(Mutex<Vec<String>>);

impl InferRoundRecorder {
    fn push(&self, line: String) {
        self.0
            .lock()
            .expect("the infer round recorder mutex is not poisoned")
            .push(line);
    }

    fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("the infer round recorder mutex is not poisoned")
            .clone()
    }
}

impl Observer for InferRoundRecorder {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        self.push(format!("{section}: {event}"));
    }

    fn on_thinking(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        model: &str,
        text: &str,
    ) {
        self.push(format!(
            "{section}: thinking turn={turn} model={model} text={text}"
        ));
    }

    fn on_assistant_reply(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        text: &str,
        finish_reason: Option<&str>,
        model: &str,
        metrics: Option<&CallMetrics>,
        origin: ReplyOrigin,
    ) {
        self.push(format!(
            "{section}: assistant_reply origin={origin:?} turn={turn} text={text} finish={finish_reason:?} model={model} metrics={}",
            metrics.is_some()
        ));
    }
}

/// The index of the first recorded line containing `needle`, panicking with
/// the whole sequence when it is absent.
fn line_index(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no recorded line contains {needle:?}: {lines:#?}"))
}

/// Runs one handle-form `models.infer` round scripted with `reply` and
/// returns the run's output beside every line the recorder saw.
async fn run_infer_round(reply: ScriptedReply) -> (String, Vec<String>) {
    let gateway = ScriptedChat::new(vec![reply]);
    let recorder = Arc::new(InferRoundRecorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        return models.get('writer'):infer('say hello')\n\
        ```\n";
    let prompt = bound_with_tools(md);
    let out = run(
        &prompt,
        "",
        &[],
        &TestStore::new(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            client: Some(gateway_client(&gateway)),
            debug: None,
        },
    )
    .await
    .expect("handle-form infer must return text");
    (out, recorder.lines())
}

/// A text reply carrying a `reasoning_content` side channel.
fn resp_text_with_reasoning(content: &str, reasoning: &str) -> ScriptedReply {
    ScriptedReply::Text {
        model: MOCK_MODEL.to_owned(),
        content: content.to_owned(),
        finish_reason: None,
        reasoning: Some(reasoning.to_owned()),
        metrics: None,
    }
}

#[tokio::test]
async fn infer_round_reports_model_turn_completed_then_an_infer_origin_assistant_reply() {
    // The infer turn's reporting order: the completed boundary, then one
    // `assistant_reply` tagged `origin = Infer`.
    let (out, lines) = run_infer_round(resp_text("pong")).await;
    assert_eq!(out, "pong");

    let completed = line_index(&lines, "Model turn completed");
    let reply = line_index(&lines, "assistant_reply");
    assert_eq!(
        reply,
        completed + 1,
        "the infer reply must immediately follow the completed turn: {lines:#?}"
    );
    assert!(
        lines[reply].contains("origin=Infer")
            && lines[reply].contains("text=pong")
            && lines[reply].contains("turn=1"),
        "the infer reply must carry the round's infer origin, text, and turn: {lines:#?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("origin=Chat")),
        "an infer round must not report a chat-origin reply: {lines:#?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("thinking")),
        "a reply with no reasoning must not report a thinking block: {lines:#?}"
    );
}

#[tokio::test]
async fn infer_round_reports_thinking_between_the_completed_turn_and_the_reply() {
    // The thinking parity path: reasoning the backend supplied reaches the
    // observer as `thinking`, ordered after the completed boundary and
    // before the infer-origin `assistant_reply`.
    let (out, lines) = run_infer_round(resp_text_with_reasoning("pong", "let me think")).await;
    assert_eq!(out, "pong");

    let completed = line_index(&lines, "Model turn completed");
    let thinking = line_index(&lines, "thinking");
    let reply = line_index(&lines, "assistant_reply");
    assert_eq!(
        thinking,
        completed + 1,
        "thinking must follow the completed turn: {lines:#?}"
    );
    assert_eq!(
        reply,
        completed + 2,
        "the infer reply must follow the thinking block: {lines:#?}"
    );
    assert!(
        lines[thinking].contains("text=let me think") && lines[thinking].contains("turn=1"),
        "the thinking block must carry the reasoning text and turn: {lines:#?}"
    );
    assert!(
        lines[reply].contains("text=pong") && lines[reply].contains("origin=Infer"),
        "the infer reply must carry the round's text and infer origin: {lines:#?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("origin=Chat")),
        "an infer round must not report a chat-origin reply: {lines:#?}"
    );
}
