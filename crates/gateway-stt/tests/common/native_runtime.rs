// ArtifactStore's blocking HTTP client owns a private Tokio runtime that must
// be created and dropped outside an async Tokio context.

use std::time::Duration;

use crate::SpeechService;

pub(crate) fn start(config: gateway_config::Config) -> SpeechService {
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let startup = std::thread::spawn(move || {
        let service = SpeechService::new();
        let result = service
            .load_initial(&config, None, &tokio_util::sync::CancellationToken::new())
            .map(|()| service);
        drop(result_tx.send(result));
    });
    let service = result_rx
        .recv_timeout(Duration::from_secs(180))
        .expect("native runtime startup completes within its bound")
        .expect("engine loads");
    startup.join().expect("runtime startup thread does not panic");
    service
}

pub(crate) fn shutdown(service: SpeechService) {
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let shutdown = std::thread::spawn(move || {
        service.shutdown();
        let _ = finished_tx.send(());
    });
    finished_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("native runtime shutdown completes within its bound");
    shutdown
        .join()
        .expect("runtime shutdown thread does not panic");
}
