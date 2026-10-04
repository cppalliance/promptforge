//! Local upstream concurrency, recovery, and shutdown tests.

use super::*;

#[test]
fn local_upstream_concurrent_sends_respawn_child_at_most_once() {
    // UPSTREAM-005: two concurrent transport failures on a dead child serialize
    // through the guard mutex, so recovery respawns the child exactly once.
    use crate::upstream::LocalUpstream;
    use gateway_protocol::upstream::Upstream;
    use gateway_protocol::wire::ChatRequest;
    use serde_json::Map;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
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
            *counted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            spawn_fake_child(request)
        }),
    )
    .expect("initial start should become ready");

    let alias = guard.model_alias().to_owned();
    let _ignored = guard.child.kill();
    let _ignored = guard.child.wait();
    assert!(!guard.is_running().expect("inspect dead child"));

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    );
    let make_req = || ChatRequest {
        model: "qwen-local".to_owned(),
        messages: Vec::new(),
        stream: false,
        rest: Map::new(),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let (first, second) = runtime.block_on(async {
        tokio::join!(
            upstream.send(make_req(), &alias),
            upstream.send(make_req(), &alias)
        )
    });

    // Exactly one respawn (initial + one), even under two concurrent failures.
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        2
    );
    assert!(
        first.is_ok() || second.is_ok(),
        "at least one concurrent send should succeed after the single respawn"
    );
}

#[test]
fn recover_if_dead_is_a_noop_for_a_live_but_unreachable_child() {
    // UPSTREAM-005: when a transport failure occurs but the child is still
    // running (live-but-unreachable), recovery is a no-op returning Ok(false) -
    // it never respawns a child that has not actually died.
    use crate::upstream::LocalUpstream;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
    let interrupted = AtomicBool::new(false);

    // The child is started and stays alive (never killed).
    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            *counted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            spawn_fake_child(request)
        }),
    )
    .expect("initial start should become ready");

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    );

    // The child is alive, so recovery must not respawn.
    assert!(
        !upstream.test_recover().expect("recover ok"),
        "a live child must not be respawned"
    );
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        1,
        "recovery of a live child must not spawn a replacement"
    );
}

#[test]
fn local_upstream_shutdown_kills_child_and_disables_respawn() {
    // PFGL-MOD-001/PF-GW-SERVER-004: an explicit shutdown terminates the child
    // and prevents any later transport failure from respawning it.
    use crate::upstream::LocalUpstream;
    use gateway_protocol::upstream::Upstream;
    use gateway_protocol::wire::ChatRequest;
    use serde_json::Map;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
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
            *counted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            let child = spawn_fake_child(request)?;
            *recorded_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child.id());
            Ok(child)
        }),
    )
    .expect("initial start should become ready");

    let alias = guard.model_alias().to_owned();
    let pid = child_id
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .expect("child id recorded");

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    );

    // Explicit teardown kills the child even though the upstream (an
    // Arc<dyn Upstream> stand-in) is still referenced below.
    upstream.shutdown().expect("teardown should succeed");
    assert!(
        !process_is_alive(pid),
        "shutdown must terminate the llama-server child"
    );

    // A send now fails (child dead) and must NOT respawn: spawn count stays at 1.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let result = runtime.block_on(upstream.send(
        ChatRequest {
            model: "qwen-local".to_owned(),
            messages: Vec::new(),
            stream: false,
            rest: Map::new(),
        },
        &alias,
    ));
    assert!(result.is_err(), "send to a shut-down upstream must fail");
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        1,
        "a shut-down upstream must never respawn its child"
    );
}

#[test]
fn switch_shutdown_terminates_an_in_flight_respawned_child() {
    // PFGL-MOD-001/PF-GW-SERVER-004: a shutdown concurrent with an in-flight
    // recovery/respawn must cancel the respawn and terminate the freshly spawned
    // child, so no old child can outlive a profile switch.
    use crate::upstream::LocalUpstream;
    use gateway_protocol::upstream::Upstream;
    use gateway_protocol::wire::ChatRequest;
    use serde_json::Map;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
    let (started_tx, started_rx) = std::sync::mpsc::channel::<u32>();
    let interrupted = AtomicBool::new(false);

    // Spawn #1 becomes ready; the respawn (spawn #2) is a live-but-unreachable
    // child that never serves readiness, so wait_until_ready blocks on cancel.
    let mut guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            let n = {
                let mut count = counted
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *count += 1;
                *count
            };
            if n == 1 {
                spawn_fake_child(request)
            } else {
                let child = spawn_blocked_child(request)?;
                let _ = started_tx.send(child.id());
                Ok(child)
            }
        }),
    )
    .expect("initial start should become ready");

    // Kill the ready child so the first request triggers a recovery/respawn.
    let _ignored = guard.child.kill();
    let _ignored = guard.child.wait();
    assert!(!guard.is_running().expect("inspect dead child"));

    let upstream = Arc::new(LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    ));

    // Background: a send() whose forward fails (dead child) drives recovery into
    // an in-flight respawn of the blocked child.
    let send_upstream = Arc::clone(&upstream);
    let send_handle = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        runtime.block_on(send_upstream.send(
            ChatRequest {
                model: "qwen-local".to_owned(),
                messages: Vec::new(),
                stream: false,
                rest: Map::new(),
            },
            "qwen-local",
        ))
    });

    // Wait until the respawn has spawned the blocked child.
    let blocked_pid = started_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("respawn should spawn the blocked child");
    assert!(
        process_is_alive(blocked_pid),
        "blocked child should be alive mid-respawn"
    );

    // The switch teardown cancels the in-flight respawn and terminates the child.
    upstream.shutdown().expect("teardown should succeed");

    assert!(
        !process_is_alive(blocked_pid),
        "an in-flight respawned child must not outlive the switch"
    );

    let send_result = send_handle.join().expect("send thread");
    assert!(
        send_result.is_err(),
        "a send whose respawn was cancelled must return an error"
    );
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        2,
        "shutdown must not permit a further respawn"
    );
}
