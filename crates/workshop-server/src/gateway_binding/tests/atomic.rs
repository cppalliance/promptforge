use super::*;

#[test]
fn synchronized_reads_never_observe_a_torn_replacement_snapshot() {
    let gateway = crate::test_gateway::ValidatedGateway::spawn("new-key");
    let validated =
        validated_connection(&gateway, "new-key", 1_778_000_001, "2026-09-07T18:00:01Z");
    let new_url = format!("http://127.0.0.1:{}", gateway.port());
    let binding = GatewayBinding::new("http://127.0.0.1:54375", "old-key").expect("binding builds");
    let updater = binding.updater();
    let (target_tx, target_rx) = std::sync::mpsc::sync_channel(0);
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);
    let (observed_tx, observed_rx) = std::sync::mpsc::sync_channel(0);

    let samples = std::thread::scope(|scope| {
        let reader_binding = binding.clone();
        let reader_validated = validated.clone();
        let reader_url = new_url.clone();
        let reader = scope.spawn(move || {
            let mut samples = 0_usize;
            while let Ok(target_generation) = target_rx.recv() {
                ready_tx.send(()).expect("the publisher remains live");
                loop {
                    let snapshot = reader_binding.snapshot();
                    samples += 1;
                    if snapshot.generation().is_multiple_of(2) {
                        assert_eq!(snapshot.client().base_url, "http://127.0.0.1:54375");
                        assert_eq!(snapshot.client().api_key, "old-key");
                        assert_eq!(snapshot.base_url(), "http://127.0.0.1:54375");
                        assert_eq!(snapshot.api_key(), "old-key");
                        assert!(snapshot.identity.is_none());
                        assert!(
                            format!("{:?}", snapshot.model_client().expect("old model client"))
                                .contains("http://127.0.0.1:54375/v1")
                        );
                    } else {
                        assert_eq!(snapshot.client().base_url, reader_url);
                        assert_eq!(snapshot.client().api_key, "new-key");
                        assert_eq!(snapshot.base_url(), reader_url);
                        assert_eq!(snapshot.api_key(), "new-key");
                        assert_eq!(snapshot.identity.as_ref(), Some(&reader_validated));
                        assert!(
                            format!("{:?}", snapshot.model_client().expect("new model client"))
                                .contains(&format!("{reader_url}/v1"))
                        );
                    }
                    if snapshot.generation() >= target_generation {
                        break;
                    }
                    std::thread::yield_now();
                }
                observed_tx
                    .send(())
                    .expect("the publisher observes the sample");
            }
            samples
        });

        for generation in 1_u64..=256 {
            target_tx.send(generation).expect("the reader remains live");
            ready_rx.recv().expect("the reader begins sampling");
            if generation % 2 == 0 {
                binding
                    .replace("http://127.0.0.1:54375", "old-key")
                    .expect("the configured snapshot republishes");
            } else {
                updater
                    .replace_sidecar(&validated)
                    .expect("the sidecar snapshot publishes");
            }
            observed_rx
                .recv()
                .expect("the reader observes the published generation");
        }
        drop(target_tx);
        reader.join().expect("the reader does not panic")
    });

    assert!(samples >= 256, "every synchronized publication is sampled");
}
