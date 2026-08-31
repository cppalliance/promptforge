//! Ignored parity checks against a real staged `llama-server`.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::Family;

const LIVE_ENV: &str = "PROMPTFORGE_LIVE_CHAT_TEMPLATES";
const SERVER_ENV: &str = "PROMPTFORGE_LLAMA_SERVER";
const MODELS_ENV: &str = "PROMPTFORGE_CHAT_TEMPLATE_MODELS";
const API_KEY: &str = "promptforge-chat-template-live";
const START_TIMEOUT: Duration = Duration::from_secs(20 * 60);
static LIVE_SERIAL: Mutex<()> = Mutex::new(());

#[derive(Debug, Deserialize)]
struct LiveFixture {
    family: String,
    context_json: String,
    reference_output: String,
}

#[derive(Debug, Deserialize)]
struct SpecialTokens {
    bos_token: String,
    eos_token: String,
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ignored = self.0.kill();
        let _ignored = self.0.wait();
    }
}

struct LiveInputs {
    server: PathBuf,
    model: PathBuf,
    tokens: SpecialTokens,
    fixture: LiveFixture,
}

struct RunningServer {
    _template_directory: tempfile::TempDir,
    child: ChildGuard,
    base: String,
}

fn free_port() -> u16 {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral test port");
    listener
        .local_addr()
        .expect("read the ephemeral test address")
        .port()
}

fn load_inputs(family: Family) -> LiveInputs {
    let server = PathBuf::from(
        std::env::var_os(SERVER_ENV).expect("PROMPTFORGE_LLAMA_SERVER must name the staged binary"),
    );
    let model_directory = PathBuf::from(
        std::env::var_os(MODELS_ENV)
            .expect("PROMPTFORGE_CHAT_TEMPLATE_MODELS must name the fixture directory"),
    );
    let stem = family.canonical_name();
    let model = model_directory.join(format!("{stem}.gguf"));
    let token_path = model_directory.join(format!("{stem}.tokens.json"));
    let tokens: SpecialTokens = serde_json::from_slice(
        &std::fs::read(&token_path).expect("read the live model token fixture"),
    )
    .expect("the live model token fixture must be valid JSON");

    let fixtures: Vec<LiveFixture> = serde_json::from_str(include_str!("fixtures/golden.json"))
        .expect("generated golden fixtures must be valid JSON");
    let fixture = fixtures
        .into_iter()
        .find(|fixture| fixture.family == stem)
        .expect("every family must have a golden fixture");
    LiveInputs {
        server,
        model,
        tokens,
        fixture,
    }
}

fn start_server(family: Family, inputs: &LiveInputs) -> RunningServer {
    let stem = family.canonical_name();
    let temp = tempfile::tempdir().expect("create a live template directory");
    let template_path = temp.path().join(format!("{stem}.jinja"));
    std::fs::write(&template_path, family.template()).expect("stage the live template fixture");
    let port = free_port();
    let child = Command::new(&inputs.server)
        .args(["--host", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .args(["--api-key", API_KEY, "--model"])
        .arg(&inputs.model)
        .args(["--chat-template-file"])
        .arg(&template_path)
        .args(["--jinja", "--n-gpu-layers", "99"])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start the staged llama-server");
    RunningServer {
        _template_directory: temp,
        child: ChildGuard(child),
        base: format!("http://127.0.0.1:{port}"),
    }
}

fn wait_for_readiness(client: &reqwest::blocking::Client, running: &mut RunningServer, stem: &str) {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if client
            .get(format!("{}/health", running.base))
            .bearer_auth(API_KEY)
            .send()
            .is_ok_and(|response| response.status().is_success())
        {
            break;
        }
        assert!(
            running
                .child
                .0
                .try_wait()
                .expect("inspect the live llama-server process")
                .is_none(),
            "llama-server exited before readiness for {stem}"
        );
        assert!(
            Instant::now() < deadline,
            "llama-server readiness timed out for {stem}"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn apply_template(
    client: &reqwest::blocking::Client,
    base: &str,
    body: &serde_json::Value,
) -> String {
    let response = client
        .post(format!("{base}/apply-template"))
        .bearer_auth(API_KEY)
        .json(&body)
        .send()
        .expect("POST the fixture to llama-server /apply-template");
    let status = response.status();
    let response_body = response.text().expect("read /apply-template response");
    assert!(
        status.is_success(),
        "/apply-template returned {status}: {response_body}"
    );
    let response_json: serde_json::Value =
        serde_json::from_str(&response_body).expect("/apply-template must return JSON");
    response_json
        .as_str()
        .or_else(|| {
            response_json
                .get("prompt")
                .and_then(serde_json::Value::as_str)
        })
        .expect("/apply-template JSON must carry the rendered prompt")
        .to_owned()
}

fn live_request_body(context_json: &str) -> serde_json::Value {
    let mut body: serde_json::Value =
        serde_json::from_str(context_json).expect("golden context must be valid JSON");
    let object = body
        .as_object_mut()
        .expect("golden context must be a JSON object");
    let mut template_kwargs = serde_json::Map::new();
    for key in [
        "date_string",
        "enable_thinking",
        "preserve_thinking",
        "reasoning_effort",
    ] {
        let value = object
            .remove(key)
            .expect("golden context must carry every template kwarg");
        template_kwargs.insert(key.to_owned(), value);
    }
    object.remove("bos_token");
    object.remove("eos_token");
    object.insert(
        "chat_template_kwargs".to_owned(),
        serde_json::Value::Object(template_kwargs),
    );
    body
}

fn normalize_dynamic_output(family: Family, prompt: &str) -> String {
    if family != Family::GptOss {
        return prompt.to_owned();
    }
    let marker = "Current date: ";
    let start = prompt
        .find(marker)
        .map(|index| index + marker.len())
        .expect("GPT OSS output must carry its current date");
    let end = start + "2026-08-30".len();
    let value = prompt
        .get(start..end)
        .expect("GPT OSS current date must be ten ASCII bytes");
    assert!(
        value
            .bytes()
            .enumerate()
            .all(|(index, byte)| if matches!(index, 4 | 7) {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }),
        "GPT OSS current date must use YYYY-MM-DD"
    );
    let mut normalized = prompt.to_owned();
    normalized.replace_range(start..end, "2026-08-30");
    normalized
}

fn run_live(family: Family) {
    if std::env::var_os(LIVE_ENV).is_none() {
        eprintln!(
            "skipping: set {LIVE_ENV}=1, {SERVER_ENV}=<llama-server>, and \
             {MODELS_ENV}=<fixture-directory>"
        );
        return;
    }
    let _serial = LIVE_SERIAL
        .lock()
        .expect("the live-test serialization mutex must not be poisoned");
    let inputs = load_inputs(family);
    let body = live_request_body(&inputs.fixture.context_json);
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("build the live parity client");
    let mut running = start_server(family, &inputs);
    wait_for_readiness(&client, &mut running, family.canonical_name());
    let prompt = normalize_dynamic_output(family, &apply_template(&client, &running.base, &body));
    let expected = inputs
        .fixture
        .reference_output
        .replace("<BOS>", &inputs.tokens.bos_token)
        .replace("<EOS>", &inputs.tokens.eos_token);
    let stem = family.canonical_name();
    assert_eq!(prompt.as_bytes(), expected.as_bytes(), "{stem}");
}

macro_rules! live_case {
    ($name:ident, $family:expr) => {
        #[test]
        #[ignore = "requires staged b10082 llama-server, twelve family GGUF fixtures, and self-hosted CUDA; set PROMPTFORGE_LIVE_CHAT_TEMPLATES=1 to opt in"]
        fn $name() {
            run_live($family);
        }
    };
}

live_case!(live_llama_chatml_parity, Family::Chatml);
live_case!(live_llama_llama3_parity, Family::Llama3);
live_case!(live_llama_llama31_parity, Family::Llama31);
live_case!(live_llama_qwen25_parity, Family::Qwen25);
live_case!(live_llama_qwen3_parity, Family::Qwen3);
live_case!(live_llama_gemma3_parity, Family::Gemma3);
live_case!(live_llama_gemma4_parity, Family::Gemma4);
live_case!(live_llama_mistral_parity, Family::Mistral);
live_case!(live_llama_phi3_parity, Family::Phi3);
live_case!(live_llama_phi4_parity, Family::Phi4);
live_case!(live_llama_gpt_oss_parity, Family::GptOss);
live_case!(live_llama_zephyr_parity, Family::Zephyr);

#[test]
fn live_request_uses_llama_server_template_kwargs() {
    let context = r#"{
        "add_generation_prompt": true,
        "bos_token": "<BOS>",
        "date_string": "30 August 2026",
        "enable_thinking": false,
        "eos_token": "<EOS>",
        "messages": [],
        "preserve_thinking": false,
        "reasoning_effort": "medium"
    }"#;
    let body = live_request_body(context);
    assert_eq!(body["add_generation_prompt"], true);
    assert_eq!(body["messages"], serde_json::json!([]));
    assert_eq!(
        body["chat_template_kwargs"],
        serde_json::json!({
            "date_string": "30 August 2026",
            "enable_thinking": false,
            "preserve_thinking": false,
            "reasoning_effort": "medium",
        })
    );
    assert!(body.get("bos_token").is_none());
    assert!(body.get("eos_token").is_none());
    assert!(body.get("enable_thinking").is_none());
}

#[test]
fn live_gpt_oss_output_normalizes_only_a_valid_current_date() {
    let prompt = "before Current date: 2031-12-04 after";
    assert_eq!(
        normalize_dynamic_output(Family::GptOss, prompt),
        "before Current date: 2026-08-30 after"
    );
    assert_eq!(normalize_dynamic_output(Family::Chatml, prompt), prompt);
}
