//! Unit tests for the launch-race lock and the Gateway instance lease.

use super::*;

use std::io::{Read, Write as _};
use std::net::TcpListener;
use std::sync::mpsc;

/// The test process's own image name, so the pid and image checks
/// pass and the loser reaches the probe path.
fn own_image_name() -> String {
    std::env::current_exe()
        .expect("current exe")
        .file_name()
        .expect("the exe has a file name")
        .to_string_lossy()
        .into_owned()
}

/// A fixture gateway answering health and the presented key with 200.
fn fixture_gateway() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let port = listener.local_addr().expect("fixture address").port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            for _ in 0..2 {
                let mut buffer = [0u8; 1024];
                if stream.read(&mut buffer).is_err() {
                    break;
                }
                if stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                    .is_err()
                {
                    break;
                }
            }
        }
    });
    port
}

/// A live gateway discovery file on the fixture gateway, owned by the test
/// process.
fn live_file(port: u16) -> GatewayDiscoveryFile {
    GatewayDiscoveryFile {
        port,
        api_key: "key".to_owned(),
        pid: std::process::id(),
        epoch: 1_757_000_000,
        version: "0.2.0".to_owned(),
        started_at: "2026-09-03T12:00:00Z".to_owned(),
    }
}

#[test]
fn an_empty_run_dir_elects_a_launcher() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let decision = launch_or_attach_named(dir.path(), "irrelevant", Duration::from_secs(1))
        .expect("the race settles");
    assert!(
        matches!(decision, LaunchDecision::Launch(_)),
        "no file means this caller launches"
    );
    assert!(
        lock_file_path(dir.path()).exists(),
        "the lock file was created"
    );
}

#[test]
fn process_ownership_is_separate_from_parent_launch_election() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let lease = GatewayInstanceLease::try_acquire(dir.path())
        .expect("attempt the process lease")
        .expect("the first process acquires its lease");
    let decision = launch_or_attach_named(dir.path(), "irrelevant", Duration::from_millis(100))
        .expect("the independent parent launch election settles");
    assert!(
        matches!(decision, LaunchDecision::Launch(_)),
        "holding the process lease never contends with LaunchLock"
    );
    assert!(
        GatewayInstanceLease::try_acquire(dir.path())
            .expect("contend for process ownership")
            .is_none(),
        "a second process lease cannot coexist"
    );
    drop(lease);
    assert!(
        GatewayInstanceLease::try_acquire(dir.path())
            .expect("reacquire released process ownership")
            .is_some(),
        "dropping the handle releases ownership"
    );
}

#[test]
fn a_live_file_attaches_without_contesting_the_lock() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = live_file(fixture_gateway());
    file.write_to(dir.path()).expect("write");

    let decision = launch_or_attach_named(dir.path(), &own_image_name(), Duration::from_secs(5))
        .expect("the race settles");
    match decision {
        LaunchDecision::Attach(attached) => assert_eq!(attached, file),
        LaunchDecision::Launch(_) => panic!("a live gateway must be attached, not relaunched"),
    }
}

#[test]
fn the_race_loser_attaches_to_the_winners_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let image = own_image_name();
    // The winner holds the lock with no file written yet.
    let LaunchDecision::Launch(winner) =
        launch_or_attach_named(dir.path(), &image, Duration::from_secs(5))
            .expect("the first caller wins the lock")
    else {
        panic!("an empty run dir elects a launcher");
    };

    // The loser waits; the winner's file appears mid-wait.
    let loser_dir = dir.path().to_owned();
    let loser = std::thread::spawn(move || {
        launch_or_attach_named(&loser_dir, &image, Duration::from_secs(10))
            .expect("the loser's race settles")
    });
    std::thread::sleep(Duration::from_millis(100));
    let file = live_file(fixture_gateway());
    file.write_to(dir.path()).expect("the winner writes");

    match loser.join().expect("the loser thread ran") {
        LaunchDecision::Attach(attached) => assert_eq!(attached, file),
        LaunchDecision::Launch(_) => {
            panic!("the loser must attach to the winner, not launch a second gateway")
        }
    }
    drop(winner);
}

#[test]
fn a_loser_becomes_the_launcher_when_the_winner_dies_silent() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let image = own_image_name();
    {
        let LaunchDecision::Launch(winner) =
            launch_or_attach_named(dir.path(), &image, Duration::from_secs(5))
                .expect("the first caller wins the lock")
        else {
            panic!("an empty run dir elects a launcher");
        };
        // The winner dies (or gives up) without writing a file.
        drop(winner);
    }

    let decision = launch_or_attach_named(dir.path(), &image, Duration::from_secs(5))
        .expect("the race settles");
    assert!(
        matches!(decision, LaunchDecision::Launch(_)),
        "a dead winner's lock passes to the next launcher"
    );
}

#[test]
fn a_loser_times_out_when_the_winner_stays_silent() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let image = own_image_name();
    let LaunchDecision::Launch(_winner) =
        launch_or_attach_named(dir.path(), &image, Duration::from_secs(5))
            .expect("the first caller wins the lock")
    else {
        panic!("an empty run dir elects a launcher");
    };

    let error = launch_or_attach_named(dir.path(), &image, Duration::from_millis(150))
        .expect_err("a silent winner starves the loser");
    assert!(
        matches!(error, SidecarError::LaunchTimeout { .. }),
        "the loser reports the timeout: {error}"
    );
}

#[test]
fn cancellation_joins_a_blocked_launch_race_without_launching() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let image = own_image_name();
    let LaunchDecision::Launch(winner) =
        launch_or_attach_named(dir.path(), &image, Duration::from_secs(5))
            .expect("the first caller wins the lock")
    else {
        panic!("an empty run dir elects a launcher");
    };
    let cancellation = crate::CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let run_dir = dir.path().to_owned();
    let worker_image = image.clone();
    let (entered, blocked) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        launch_or_attach_named_cancellable_with(
            &run_dir,
            &worker_image,
            Duration::from_secs(30),
            &worker_cancellation,
            |delay| {
                entered.send(()).expect("announce blocked launch race");
                worker_cancellation.wait_timeout(delay)
            },
        )
    });
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("the launch-race phase blocks deterministically");

    let started = Instant::now();
    cancellation.cancel();
    let result = worker.join().expect("the launch-race worker joins");

    assert!(
        started.elapsed() < Duration::from_millis(250),
        "cancellation bounds the blocked launch race"
    );
    assert!(matches!(result, Err(SidecarError::Cancelled)));
    assert!(
        !crate::paths::gateway_discovery_file_path(dir.path()).exists(),
        "the cancelled loser never publishes or launches"
    );
    drop(winner);
}
