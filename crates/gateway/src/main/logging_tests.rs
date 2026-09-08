use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use base64::Engine as _;
use futures_util::{SinkExt as _, StreamExt as _};
use gateway::{Config, Gateway, ProfilesContext, ServeOptions};
use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory, scripted_service};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const CHILD_STATE_DIR: &str = "PROMPTFORGE_ALIGNMENT_LOG_CHILD_STATE_DIR";
const CHILD_TEST: &str = "logging_tests::gateway_no_alignment_uses_production_logging_child";
const BEARER_SENTINEL: &str = "PROTECTED_BEARER_SENTINEL";
const PROMPT_SENTINEL: &str = "PROTECTED_PROMPT_SENTINEL";
const TRANSCRIPT_SENTINEL: &str = "PROTECTED_TRANSCRIPT_SENTINEL";
const PATH_SENTINEL: &str = r"C:\PROTECTED_PATH_SENTINEL\model.gguf";
const INJECTED_SENTINEL: &str = "INJECTED_PROTECTED_SENTINEL";

#[test]
fn mounted_no_alignment_is_drained_through_production_logging() {
    let temp = tempfile::tempdir().expect("temporary logging state");
    let output = Command::new(std::env::current_exe().expect("current test executable"))
        .args(["--exact", CHILD_TEST, "--ignored", "--nocapture"])
        .env(CHILD_STATE_DIR, temp.path())
        .env("RUST_LOG", super::DEFAULT_LOG_FILTER)
        .output()
        .expect("isolated logging child starts");
    assert!(
        output.status.success(),
        "logging child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let log_path = temp.path().join("logs").join("gateway.log");
    let log = std::fs::read_to_string(&log_path).expect("drained gateway log");
    assert_eq!(
        log.matches("warning_code=\"forced_final_overlap_estimated\"")
            .count(),
        5,
        "every unaligned successor records one estimated warning: {log}"
    );
    for expected in [
        "warning_code=\"forced_final_overlap_estimated\"",
        "prior_decode_start=0",
        "prior_decode_end=160000",
        "current_decode_start=32000",
        "current_decode_end=320000",
        "overlap_start=32000",
        "overlap_end=160000",
        "prior_decode_start=32000",
        "prior_decode_end=320000",
        "current_decode_start=192000",
        "current_decode_end=480000",
        "overlap_start=192000",
        "overlap_end=320000",
        "projection_input_bytes=",
        "projection_tokens=",
        "projection_audio_before_overlap=",
        "projection_audio_total=",
        "projection_rounded_tokens=",
        "projection_selected_tokens=",
        "projection_punctuation_examined=",
        "projection_punctuation_candidates=",
        "projection_rounding=\"nearest_ties_earlier\"",
    ] {
        assert!(log.contains(expected), "missing {expected}: {log}");
    }
    assert!(
        !log.contains("forced_final_overlap_unaligned"),
        "estimated reconciliation must not record the retired failure: {log}"
    );
    let audio = audio_payload();
    for protected in [
        TRANSCRIPT_SENTINEL,
        &audio[..64],
        PROMPT_SENTINEL,
        BEARER_SENTINEL,
        PATH_SENTINEL,
        INJECTED_SENTINEL,
    ] {
        assert!(
            !log.contains(protected),
            "protected value survived: {protected}"
        );
    }
}

#[test]
fn a_process_lease_loser_leaves_the_canonical_log_untouched() {
    let temp = tempfile::tempdir().expect("temporary Gateway state");
    let state_dir = temp.path().join(".promptforge");
    let run_dir = state_dir.join("run");
    let logs = state_dir.join("logs");
    std::fs::create_dir_all(&logs).expect("create seeded log directory");
    let log_path = logs.join("gateway.log");
    std::fs::write(&log_path, "owner-log-sentinel").expect("seed the owner's canonical log");
    let _owner = shared_sidecar::GatewayInstanceLease::try_acquire(&run_dir)
        .expect("acquire the owner lease")
        .expect("the test owns the process lease");
    let connection_path = shared_sidecar::connection_file_path(&run_dir);
    std::fs::write(&connection_path, b"owner-is-still-publishing")
        .expect("seed an unreadable owner record");
    let options = ServeOptions::new(None, None::<gateway::ProfileName>).with_run_dir(run_dir);

    let error = gateway::settle_gateway_startup(&options, Duration::from_millis(150))
        .expect_err("the process lease loser times out before logging");
    assert!(
        error.to_string().contains("process owner"),
        "the console-only error names the ownership timeout: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(&log_path).expect("read the seeded log"),
        "owner-log-sentinel",
        "the loser never initializes or rotates canonical logging"
    );
    assert!(
        !logs.join("gateway.log.1").exists(),
        "the loser creates no retained log"
    );
    assert_eq!(
        std::fs::read(&connection_path).expect("read the seeded owner record"),
        b"owner-is-still-publishing",
        "the loser never cleans or rewrites shared connection state"
    );
}

#[test]
fn a_lease_holder_resolution_failure_leaves_the_canonical_log_untouched() {
    let temp = tempfile::tempdir().expect("temporary Gateway state");
    let state_dir = temp.path().join(".promptforge");
    let run_dir = state_dir.join("run");
    let logs = state_dir.join("logs");
    std::fs::create_dir_all(&logs).expect("create seeded log directory");
    let log_path = logs.join("gateway.log");
    std::fs::write(&log_path, "owner-log-sentinel").expect("seed the canonical log");
    let connection_path = shared_sidecar::connection_file_path(&run_dir);
    std::fs::create_dir_all(&connection_path).expect("create unreadable connection fixture");
    let options = ServeOptions::new(None, None::<gateway::ProfileName>).with_run_dir(run_dir);

    let error = gateway::settle_gateway_startup(&options, Duration::from_millis(150))
        .expect_err("connection resolution fails before logging");

    assert!(
        matches!(
            error,
            gateway::GatewayStartupError::Resolve(shared_sidecar::SidecarError::Read { .. })
        ),
        "the console-only failure preserves its source: {error:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&log_path).expect("read the seeded log"),
        "owner-log-sentinel",
        "failed resolution never initializes or rotates canonical logging"
    );
    assert!(
        !logs.join("gateway.log.1").exists(),
        "failed resolution creates no retained log"
    );
    assert!(
        connection_path.is_dir(),
        "failed resolution leaves uncertain connection state untouched"
    );
}

fn audio_payload() -> String {
    let samples = (0..240_000)
        .map(|index| {
            if index % 2 == 0 {
                0x1357_i16
            } else {
                0x2468_i16
            }
        })
        .flat_map(i16::to_le_bytes)
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(samples)
}

#[test]
#[ignore = "spawned by the production logging parent"]
fn gateway_no_alignment_uses_production_logging_child() {
    let Some(state_dir) = std::env::var_os(CHILD_STATE_DIR) else {
        return;
    };
    let logging = super::init_logging_for_state(Some(PathBuf::from(state_dir)))
        .expect("production Gateway logging starts");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("test runtime starts");
    runtime.block_on(drive_no_alignment());
    drop(runtime);
    logging
        .shutdown()
        .expect("production Gateway logging drains");
}

fn unaligned_decoder() -> ScriptedDecoder {
    let final_decoder = ScriptedDecoder::new();
    for window in 0..6 {
        let tokens = if window == 0 { 10 } else { 18 };
        let mut text = (0..tokens)
            .map(|token| format!("w{window}t{token}"))
            .collect::<Vec<_>>()
            .join(" ");
        if window == 0 {
            text.push(' ');
            text.push_str(TRANSCRIPT_SENTINEL);
        }
        if window == 1 {
            text.push(' ');
            text.push_str(PATH_SENTINEL);
            text.push(' ');
            text.push_str(INJECTED_SENTINEL);
        }
        final_decoder.push_text(text);
    }
    final_decoder
}

async fn append_unaligned_windows(
    socket: &mut Socket,
    final_decoder: &ScriptedDecoder,
    audio: &str,
) {
    for count in 1..=6 {
        send(
            socket,
            serde_json::json!({
                "type": "input_audio_buffer.append",
                "audio": audio
            }),
        )
        .await;
        let observer = final_decoder.clone();
        assert!(
            tokio::task::spawn_blocking(move || {
                observer.wait_for_completed(count, Duration::from_secs(10))
            })
            .await
            .expect("decode observer joins"),
            "forced decode {count} completes"
        );
    }
}

async fn expect_healthy_completion(socket: &mut Socket) {
    send(
        socket,
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    for _ in 0..12 {
        let event = receive(socket).await;
        match event["type"].as_str() {
            Some("conversation.item.input_audio_transcription.completed") => return,
            Some("conversation.item.input_audio_transcription.failed" | "error") => {
                panic!("estimated reconciliation emitted a terminal failure: {event}")
            }
            Some("input_audio_buffer.committed" | "conversation.item.created") => {}
            other => panic!("unexpected mounted logging event {other:?}: {event}"),
        }
    }
    panic!("mounted session did not complete after estimated reconciliation");
}

async fn drive_no_alignment() {
    let final_decoder = unaligned_decoder();
    let service = scripted_service(
        ScriptedModelFactory::new(ScriptedDecoder::new()).with_final(final_decoder.clone()),
        15,
        500,
    )
    .expect("scripted speech starts");
    let config = Config::from_toml_str(&format!(
        "config-version = 2\n\
         [server]\n\
         bind = \"127.0.0.1:0\"\n\
         api_key = \"{BEARER_SENTINEL}\"\n\
         trust_loopback = false\n"
    ))
    .expect("Gateway config parses");
    let gateway = Gateway::new(&config, ProfilesContext::default()).with_speech_service(service);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mounted listener binds");
    let address = listener.local_addr().expect("mounted address");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        gateway
            .serve(listener, async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let mut socket = connect(address).await;
    assert_eq!(receive(&mut socket).await["type"], "session.created");
    send(
        &mut socket,
        serde_json::json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "audio": {
                    "input": {
                        "transcription": {
                            "prompt": PROMPT_SENTINEL
                        }
                    }
                },
                "include": []
            }
        }),
    )
    .await;
    assert_eq!(receive(&mut socket).await["type"], "session.updated");

    let audio = audio_payload();
    append_unaligned_windows(&mut socket, &final_decoder, &audio).await;
    expect_healthy_completion(&mut socket).await;

    socket.close(None).await.expect("Realtime socket closes");
    let _ = shutdown_tx.send(());
    server
        .await
        .expect("mounted Gateway task joins")
        .expect("mounted Gateway serves");
}

async fn connect(address: std::net::SocketAddr) -> Socket {
    let mut request = format!("ws://{address}/v1/realtime?intent=transcription")
        .into_client_request()
        .expect("Realtime request builds");
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {BEARER_SENTINEL}")).expect("bearer header"),
    );
    let (socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("Realtime socket connects");
    assert_eq!(response.status(), 101);
    socket
}

async fn send(socket: &mut Socket, value: serde_json::Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("client event sends");
}

async fn receive(socket: &mut Socket) -> serde_json::Value {
    let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .expect("server frame arrives")
        .expect("socket remains open")
        .expect("server frame is valid");
    serde_json::from_str(message.to_text().expect("server frame is text"))
        .expect("server frame is JSON")
}
