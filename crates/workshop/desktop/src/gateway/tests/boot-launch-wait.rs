//! Launch-wait coverage against a fake wait clock.

use std::cell::Cell;
use std::path::Path;
use std::time::{Duration, Instant};

use gateway_api_discovery::{CancellationToken, GatewayDiscoveryFile, HealthError, ProbeError};

use super::super::{fixture_gateway, live_file, probe_own_image};
use crate::gateway::supervisor::{
    RECOVERY_POLL_INTERVAL, SystemClock, WaitClock, wait_for_launched_file_cancellable_with,
};

/// A discovery-file port the fake-clock tests never probe.
const UNPROBED_PORT: u16 = 9;

/// A wait clock that advances only when the wait pauses, by exactly the
/// requested delay, and never reports cancellation.
struct FakeClock<'hook> {
    now: Cell<Instant>,
    pauses: Cell<usize>,
    on_pause: Option<Box<dyn Fn(usize) + 'hook>>,
}

impl<'hook> FakeClock<'hook> {
    fn new() -> Self {
        Self {
            now: Cell::new(Instant::now()),
            pauses: Cell::new(0),
            on_pause: None,
        }
    }

    /// Runs `hook` with the one-based pause number after each pause.
    fn on_pause(hook: impl Fn(usize) + 'hook) -> Self {
        Self {
            on_pause: Some(Box::new(hook)),
            ..Self::new()
        }
    }

    fn pauses(&self) -> usize {
        self.pauses.get()
    }
}

impl WaitClock for FakeClock<'_> {
    fn now(&self) -> Instant {
        self.now.get()
    }

    fn pause(&self, delay: Duration, _cancellation: &CancellationToken) -> bool {
        let pause = self.pauses.get() + 1;
        self.pauses.set(pause);
        self.now.set(self.now.get() + delay);
        if let Some(hook) = &self.on_pause {
            hook(pause);
        }
        false
    }
}

/// A health failure built without any network probe.
fn probe_timeout(url: &str, status_line: String) -> HealthError {
    HealthError::Timeout {
        url: url.to_owned(),
        timeout: Duration::from_millis(250),
        source: ProbeError::UnexpectedStatus { status_line },
    }
}

/// Runs the launch wait against the test binary's own process image.
fn launch_wait_with<Clock, Health>(
    run_dir: &Path,
    budget: Duration,
    clock: &Clock,
    health: Health,
) -> anyhow::Result<GatewayDiscoveryFile>
where
    Clock: WaitClock,
    Health: FnMut(&str, Duration, &CancellationToken) -> Result<(), HealthError>,
{
    wait_for_launched_file_cancellable_with(
        run_dir,
        budget,
        &CancellationToken::new(),
        clock,
        health,
        |run_dir, _| probe_own_image(run_dir),
    )
}

fn launch_wait(run_dir: &Path, budget: Duration) -> anyhow::Result<GatewayDiscoveryFile> {
    launch_wait_with(
        run_dir,
        budget,
        &SystemClock,
        gateway_api_discovery::wait_for_health_cancellable,
    )
}

#[test]
fn the_launch_wait_returns_once_the_file_appears_and_answers() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let file = live_file(fixture_gateway("key"), "key");
    let clock = FakeClock::on_pause(|pause| {
        if pause == 1 {
            file.write_to(run.path())
                .expect("the launched gateway writes");
        }
    });

    let waited = launch_wait_with(
        run.path(),
        Duration::from_secs(5),
        &clock,
        gateway_api_discovery::wait_for_health_cancellable,
    )
    .expect("the validated file lands and answers");

    assert_eq!(waited, file);
    assert_eq!(
        clock.pauses(),
        1,
        "the file lands during the first pause and the next poll attaches"
    );
}

#[test]
fn the_launch_wait_times_out_when_no_file_appears() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let error = launch_wait(run.path(), Duration::from_millis(150))
        .expect_err("a gateway that never writes must not hang boot");
    assert!(
        error
            .to_string()
            .contains("no validated gateway discovery file"),
        "the error names the missing file: {error}"
    );
}

#[test]
fn the_launch_wait_rejects_a_key_the_live_process_does_not_accept() {
    let run = tempfile::TempDir::new().expect("tempdir");
    live_file(fixture_gateway("accepted-key"), "rejected-key")
        .write_to(run.path())
        .expect("write");

    let error = launch_wait(run.path(), Duration::from_millis(150))
        .expect_err("an unaccepted discovery-file key must not publish");
    let message = format!("{error:#}");
    assert!(
        message.contains("no validated gateway discovery file"),
        "the error names the validation failure: {message}"
    );
    assert!(
        message.contains("bearer was rejected") && !message.contains("rejected-key"),
        "the error reports the rejection without exposing the key: {message}"
    );
}

#[test]
fn the_launch_wait_completes_when_a_dead_port_file_is_replaced_by_a_live_one() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let live = live_file(fixture_gateway("key"), "key");
    assert_ne!(
        live.port, UNPROBED_PORT,
        "the stale file names another port"
    );
    live_file(UNPROBED_PORT, "key")
        .write_to(run.path())
        .expect("write the stale file");
    let clock = FakeClock::new();
    let mut probes = 0;

    let waited = launch_wait_with(
        run.path(),
        Duration::from_secs(5),
        &clock,
        |url, budget, cancellation| {
            probes += 1;
            if probes == 1 {
                assert_eq!(url, format!("http://127.0.0.1:{UNPROBED_PORT}"));
                live.write_to(run.path())
                    .expect("the gateway rewrites its discovery file");
                return Err(probe_timeout(url, "HTTP/1.1 503 Unavailable".to_owned()));
            }
            assert_eq!(url, format!("http://127.0.0.1:{}", live.port));
            gateway_api_discovery::wait_for_health_cancellable(url, budget, cancellation)
        },
    )
    .expect("the wait polls past the stale file and attaches to the live gateway");

    assert_eq!(waited, live);
    assert_eq!(
        probes, 2,
        "one failed probe of the stale file, then one of the live gateway"
    );
}

#[test]
fn the_launch_wait_fails_at_its_budget_with_the_last_probe_error() {
    let run = tempfile::TempDir::new().expect("tempdir");
    live_file(UNPROBED_PORT, "key")
        .write_to(run.path())
        .expect("write the stale file");
    let budget = Duration::from_millis(600);
    let polls = usize::try_from(
        budget
            .as_nanos()
            .div_ceil(RECOVERY_POLL_INTERVAL.as_nanos()),
    )
    .expect("the poll count fits in usize");
    let clock = FakeClock::new();
    let mut probes = 0;

    let error = launch_wait_with(run.path(), budget, &clock, |url, _, _| {
        probes += 1;
        Err(probe_timeout(url, format!("HTTP/1.1 503 probe {probes}")))
    })
    .expect_err("a file that never names a live gateway must not hang boot");

    assert_eq!(
        probes,
        polls + 1,
        "one probe at the start and one after each poll interval up to the budget"
    );
    assert_eq!(
        clock.pauses(),
        polls,
        "the wait pauses only before the budget"
    );
    let message = format!("{error:#}");
    assert!(
        message.contains("no validated gateway discovery file"),
        "the error names the budget failure: {message}"
    );
    assert!(
        message.contains(&format!(
            "unexpected health response: HTTP/1.1 503 probe {probes}"
        )),
        "the error reports the last probe: {message}"
    );
}
