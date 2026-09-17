async fn commit_existing_item(
    socket: &mut Socket,
    append: &serde_json::Value,
    previous: Option<&String>,
) -> String {
    for _ in 0..5 {
        send(socket, append.clone()).await;
    }
    send(
        socket,
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    let committed = expect_type(socket, "input_audio_buffer.committed").await;
    let item_id = committed["item_id"]
        .as_str()
        .expect("committed item has an ID")
        .to_owned();
    assert_eq!(
        committed["previous_item_id"],
        previous.map_or(serde_json::Value::Null, |item| {
            serde_json::Value::String(item.clone())
        }),
        "{committed}"
    );
    let created = expect_type(socket, "conversation.item.created").await;
    assert_eq!(created["item"]["id"], item_id, "{created}");
    item_id
}

async fn expect_existing_completions(
    socket: &mut Socket,
    existing_items: &[String],
    expected_release: &serde_json::Value,
) {
    let mut completed_items = Vec::new();
    let mut released_item = None;
    for _ in existing_items {
        let completed = expect_type(
            socket,
            "conversation.item.input_audio_transcription.completed",
        )
        .await;
        if completed["transcript"] == expected_release["transcript"] {
            assert_eq!(
                completed["usage"]["type"], expected_release["usage"]["type"],
                "{completed}"
            );
            assert!(
                completed["usage"]["seconds"]
                    .as_f64()
                    .is_some_and(|seconds| seconds > 0.0),
                "{completed}"
            );
            released_item = completed["item_id"].as_str().map(str::to_owned);
        }
        completed_items.push(
            completed["item_id"]
                .as_str()
                .expect("completion has an item ID")
                .to_owned(),
        );
    }
    assert!(
        released_item.is_some(),
        "the canonical capacity-release completion is observed"
    );
    assert!(
        existing_items
            .iter()
            .all(|item| completed_items.contains(item)),
        "only the four existing items complete"
    );
}

async fn expect_retried_item(
    socket: &mut Socket,
    retry: serde_json::Value,
    existing_items: &[String],
) {
    send(socket, retry).await;
    let retried = expect_type(socket, "input_audio_buffer.committed").await;
    let retried_item = retried["item_id"]
        .as_str()
        .expect("retried commit has an item ID")
        .to_owned();
    assert_eq!(
        retried["previous_item_id"],
        serde_json::Value::String(existing_items.last().expect("four existing items").clone()),
        "{retried}"
    );
    assert!(
        !existing_items.contains(&retried_item),
        "retry promotes the preserved provisional input as a new durable item"
    );
    let created = expect_type(socket, "conversation.item.created").await;
    assert_eq!(created["item"]["id"], retried_item, "{created}");
    let completed = expect_type(
        socket,
        "conversation.item.input_audio_transcription.completed",
    )
    .await;
    assert_eq!(completed["item_id"], retried_item, "{completed}");
    assert_eq!(completed["transcript"], "retried canonical input");
}

#[tokio::test]
async fn saturated_commit_preserves_the_canonical_input_for_retry() {
    let fixtures = canonical_sequences();
    let mut append = canonical_client(
        &fixtures,
        "saturated_commit_retry",
        "input_audio_buffer.append",
    );
    append["audio"] = serde_json::json!(audio());
    let commit = canonical_message(
        &fixtures,
        "saturated_commit_retry",
        "client",
        "input_audio_buffer.commit",
        0,
    );
    let retry = canonical_message(
        &fixtures,
        "saturated_commit_retry",
        "client",
        "input_audio_buffer.commit",
        1,
    );
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    for transcript in [
        "released",
        "existing two",
        "existing three",
        "existing four",
    ] {
        final_decoder.push_text(transcript);
    }
    final_decoder.push_text("retried canonical input");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;

    let (existing_items, saturated, requests_at_saturation) = final_decoder
        .with_next_decode_blocked(
            PHASE_TIMEOUT,
            || async {
                let mut existing_items: Vec<String> = Vec::new();
                for _ in 0..4 {
                    let item_id =
                        commit_existing_item(&mut socket, &append, existing_items.last()).await;
                    existing_items.push(item_id);
                }
                (&mut socket, existing_items)
            },
            |(socket, existing_items)| async {
                assert_eq!(
                    final_decoder.requests().len(),
                    1,
                    "the serial final worker is blocked while four items own finalization"
                );

                for _ in 0..5 {
                    send(socket, append.clone()).await;
                }
                send(socket, commit).await;
                let saturated = expect_type(socket, "error").await;
                let requests_at_saturation = final_decoder.requests().len();
                (existing_items, saturated, requests_at_saturation)
            },
        )
        .await
        .expect("four committed items remain outstanding behind the blocked final worker");
    let expected_error = canonical_server(&fixtures, "saturated_commit_retry", "error");
    for field in ["type", "code", "message", "param", "event_id"] {
        assert_eq!(
            saturated["error"][field], expected_error["error"][field],
            "{field}: {saturated}"
        );
    }
    assert_eq!(
        requests_at_saturation, 1,
        "the rejected commit starts no fifth finalization"
    );

    let expected_release = canonical_server(
        &fixtures,
        "saturated_commit_retry",
        "conversation.item.input_audio_transcription.completed",
    );
    expect_existing_completions(&mut socket, &existing_items, &expected_release).await;
    expect_retried_item(&mut socket, retry, &existing_items).await;

    let final_requests = final_decoder.requests();
    assert_eq!(final_requests.len(), 5);
    assert_eq!(
        final_requests[4].samples(),
        final_requests[0].samples(),
        "retry finalizes exactly the same canonical audio as an accepted item"
    );

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
