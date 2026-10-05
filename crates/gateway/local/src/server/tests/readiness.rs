//! Readiness, drop, and respawn tests for the guarded child.

use super::*;

#[test]
fn a_child_serving_a_foreign_alias_never_becomes_ready() {
    // A child serving another attempt's alias never passes the identity
    // check, so the bounded poll times out and the start fails.
    const FAST_POLICY: StartupPolicy = StartupPolicy {
        attempts: 1,
        deadline: Duration::from_millis(300),
        interval: Duration::from_millis(10),
        http_timeout: Duration::from_millis(50),
    };
    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let interrupted = AtomicBool::new(false);
    let error = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        FAST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(|request: &SpawnRequest<'_>| {
            spawn_child_serving(request, "someone-else")
        }),
    )
    .expect_err("a foreign alias must never become ready");
    assert!(matches!(error, LocalError::Startup { .. }));
}

#[test]
fn retries_after_foreign_health_listener_wins_selected_port() {
    let foreign = FakeHttpServer::start("Qwen3-0.6B-Q8_0.gguf");
    let fresh_port = free_port().expect("select retry port");
    let mut ports = VecDeque::from([foreign.port, fresh_port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut identity_index = 0;
    let mut make_identity = || {
        let identity = deterministic_identity(identity_index);
        identity_index += 1;
        identity
    };
    let attempted_ports = Arc::new(Mutex::new(Vec::new()));
    let recorded_ports = Arc::clone(&attempted_ports);
    let interrupted = AtomicBool::new(false);

    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            recorded_ports
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.port);
            spawn_fake_child(request)
        }),
    )
    .expect("retry should reach the spawned fake server");

    assert_eq!(
        *attempted_ports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        [foreign.port, fresh_port]
    );
    assert_eq!(guard.port, fresh_port);
    assert_eq!(guard.model_alias(), "promptforge-test-model-1");
    assert_eq!(guard.api_key(), "promptforge-test-key-1");
}

#[test]
fn drop_kills_the_child_process() {
    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let child_id = Arc::new(Mutex::new(None));
    let recorded_id = Arc::clone(&child_id);
    let interrupted = AtomicBool::new(false);

    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            let child = spawn_fake_child(request)?;
            *recorded_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child.id());
            Ok(child)
        }),
    )
    .expect("fake child should become ready");
    assert!(listener_is_present(port, Duration::from_millis(100)));
    let id = child_id
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .expect("child id recorded");
    drop(guard);

    assert!(
        !process_is_alive(id),
        "ServerGuard Drop must kill the llama-server child"
    );
    assert!(!listener_is_present(port, Duration::from_millis(100)));
}

#[test]
fn respawn_reuses_port_and_identity_after_child_death() {
    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_log = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&spawn_log);
    let interrupted = AtomicBool::new(false);

    let mut guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    request.port,
                    request.model_alias.to_owned(),
                    request.api_key.to_owned(),
                ));
            spawn_fake_child(request)
        }),
    )
    .expect("fake child should become ready");

    let alias = guard.model_alias().to_owned();
    let key = guard.api_key().to_owned();
    let _ignored = guard.child.kill();
    let _ignored = guard.child.wait();
    assert!(!guard.is_running().expect("inspect dead child"));

    guard
        .respawn(
            Path::new("fake-llama-server"),
            Path::new("pinned-model.gguf"),
            &options(false),
            &AtomicBool::new(false),
        )
        .expect("respawn should become ready on the same port");

    assert_eq!(guard.port(), port);
    assert_eq!(guard.model_alias(), alias);
    assert_eq!(guard.api_key(), key);
    assert!(guard.is_running().expect("inspect respawned child"));
    let log = spawn_log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(log.len(), 2);
    assert_eq!(log[0], (port, alias.clone(), key.clone()));
    assert_eq!(log[1], (port, alias, key));
}
