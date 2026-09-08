#[derive(Clone, Default)]
struct HourRelayProbe {
    appends: Arc<std::sync::atomic::AtomicUsize>,
    commits: Arc<std::sync::atomic::AtomicUsize>,
}

async fn hour_upstream(State(probe): State<HourRelayProbe>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        if socket
            .send(Message::Text(
                r#"{"type":"session.created","event_id":"hour_session","session":{"id":"hour","object":"realtime.transcription_session","modalities":["audio","text"],"input_audio_format":"pcm16","input_audio_transcription":{"model":"mounted"},"turn_detection":null,"input_audio_noise_reduction":null,"include":[]}}"#
                    .into(),
            ))
            .await
            .is_err()
        {
            return;
        }
        while let Some(Ok(Message::Text(text))) = socket.recv().await {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) else {
                return;
            };
            match event["type"].as_str() {
                Some("session.update") => {
                    let updated = r#"{"type":"session.updated","event_id":"hour_updated","session":{"id":"hour","object":"realtime.transcription_session","modalities":["audio","text"],"input_audio_format":"pcm16","input_audio_transcription":{"model":"mounted"},"turn_detection":null,"input_audio_noise_reduction":null,"include":[]}}"#;
                    if socket.send(Message::Text(updated.into())).await.is_err() {
                        return;
                    }
                }
                Some("input_audio_buffer.append") => {
                    probe.appends.fetch_add(1, Ordering::AcqRel);
                }
                Some("input_audio_buffer.commit") => {
                    probe.commits.fetch_add(1, Ordering::AcqRel);
                    for frame in [
                        r#"{"type":"input_audio_buffer.committed","event_id":"hour_committed","item_id":"item_hour","previous_item_id":null}"#,
                        r#"{"type":"conversation.item.created","event_id":"hour_item","previous_item_id":null,"item":{"id":"item_hour","object":"realtime.item","type":"message","status":"completed","role":"user","content":[{"type":"input_audio","transcript":null}]}}"#,
                        r#"{"type":"conversation.item.input_audio_transcription.completed","event_id":"hour_completed","item_id":"item_hour","content_index":0,"transcript":"hour complete","usage":{"type":"duration","seconds":3600.0}}"#,
                    ] {
                        if socket.send(Message::Text(frame.into())).await.is_err() {
                            return;
                        }
                    }
                }
                _ => return,
            }
        }
    })
}

#[tokio::test]
async fn canonical_sequences_cross_the_fake_upstream_unchanged_without_browser_bearer() {
    let fixture = FixtureUpstream {
        frames: Arc::new(canonical_server_frames()),
        ..FixtureUpstream::default()
    };
    let gateway = spawn_gateway(
        Router::new()
            .route("/v1/realtime", get(fixture_upstream))
            .with_state(fixture.clone()),
    )
    .await;
    let server = TestServer::spawn(&gateway);
    let url = server.ws_url("/v1/realtime?browser=query");
    let mut request = request_with(&url, None, None);
    request.headers_mut().insert(
        header::AUTHORIZATION,
        "Bearer browser-secret"
            .parse()
            .expect("browser bearer is a header"),
    );
    let (mut socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("Workshop fixture relay upgrades");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

    for expected in fixture.frames.iter() {
        let ClientMessage::Text(actual) = recv(&mut socket).await else {
            panic!("canonical fixture remains a text payload");
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&actual).expect("relayed event parses"),
            serde_json::from_str::<serde_json::Value>(expected).expect("fixture event parses")
        );
    }
    let opaque = "opaque: not JSON, not speech state";
    socket
        .send(ClientMessage::Text(opaque.into()))
        .await
        .expect("opaque browser text sends");
    assert_eq!(recv(&mut socket).await, ClientMessage::Text(opaque.into()));

    assert!(fixture.gateway_bearer_seen.load(Ordering::Acquire));
    assert!(
        !fixture.browser_bearer_seen.load(Ordering::Acquire),
        "the browser bearer never reaches the fake Gateway"
    );
    socket.close(None).await.expect("fixture socket closes");
}

#[tokio::test]
async fn hour_equivalent_one_take_crosses_the_opaque_relay() {
    let probe = HourRelayProbe::default();
    let gateway = spawn_gateway(
        Router::new()
            .route("/v1/realtime", get(hour_upstream))
            .with_state(probe.clone()),
    )
    .await;
    let server = TestServer::spawn(&gateway);
    let (mut socket, response) =
        tokio_tungstenite::connect_async(server.ws_url("/v1/realtime?intent=transcription"))
            .await
            .expect("the Workshop hour relay upgrades");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    let ClientMessage::Text(created) = recv(&mut socket).await else {
        panic!("the session event remains text");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&created).expect("session parses")["type"],
        "session.created"
    );
    socket
        .send(ClientMessage::Text(
            r#"{"type":"session.update","session":{"type":"transcription","include":[]}}"#.into(),
        ))
        .await
        .expect("session update crosses the relay");
    let ClientMessage::Text(updated) = recv(&mut socket).await else {
        panic!("the update remains text");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&updated).expect("update parses")["type"],
        "session.updated"
    );

    let bounded_chunk =
        ClientMessage::Text(r#"{"type":"input_audio_buffer.append","audio":"AQA="}"#.into());
    for _ in 0..360 {
        socket
            .send(bounded_chunk.clone())
            .await
            .expect("one reused ten-second-equivalent chunk crosses");
    }
    socket
        .send(ClientMessage::Text(
            r#"{"type":"input_audio_buffer.commit","event_id":"hour_commit"}"#.into(),
        ))
        .await
        .expect("the sole commit crosses");

    let mut item_ids = Vec::new();
    for expected in [
        "input_audio_buffer.committed",
        "conversation.item.created",
        "conversation.item.input_audio_transcription.completed",
    ] {
        let ClientMessage::Text(frame) = recv(&mut socket).await else {
            panic!("the hour result remains text");
        };
        let event =
            serde_json::from_str::<serde_json::Value>(&frame).expect("hour event parses");
        assert_eq!(event["type"], expected);
        let item_id = event["item_id"]
            .as_str()
            .or_else(|| event["item"]["id"].as_str())
            .expect("every item event carries its owner");
        item_ids.push(item_id.to_owned());
        if expected.ends_with("completed") {
            assert_eq!(event["usage"]["seconds"], 3_600.0);
        }
    }
    assert_eq!(item_ids, ["item_hour", "item_hour", "item_hour"]);
    assert_eq!(probe.appends.load(Ordering::Acquire), 360);
    assert_eq!(probe.commits.load(Ordering::Acquire), 1);
    socket.close(None).await.expect("fixture socket closes");
}
