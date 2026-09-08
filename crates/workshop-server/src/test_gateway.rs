//! Named local Gateway process shared by capability and supervision tests.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use shared_sidecar::{ConnectionFile, ValidatedConnection};

mod process;

#[cfg(windows)]
const GATEWAY_EXE_NAME: &str = "promptforge-gateway.exe";
#[cfg(not(windows))]
const GATEWAY_EXE_NAME: &str = "promptforge-gateway";

const CONTROL_ADDRESS_ENV: &str = "PROMPTFORGE_TEST_GATEWAY_CONTROL_ADDRESS";
const EXPECTED_KEY_ENV: &str = "PROMPTFORGE_TEST_GATEWAY_EXPECTED_KEY";
const INSTANCE_LEASE_RUN_DIR_ENV: &str = "PROMPTFORGE_TEST_GATEWAY_INSTANCE_LEASE_RUN_DIR";

/// A named child Gateway that passes production process-image validation.
#[derive(Debug)]
pub struct ValidatedGateway {
    child: Child,
    port: u16,
    control: TcpStream,
    _directory: tempfile::TempDir,
}

impl ValidatedGateway {
    #[cfg(test)]
    pub(crate) fn spawn(expected_key: &str) -> Self {
        Self::spawn_in_with_lease(
            expected_key,
            "test_gateway::validated_gateway_fixture_process",
            None,
        )
    }

    #[cfg(test)]
    fn spawn_with_instance_lease(expected_key: &str, run_dir: &std::path::Path) -> Self {
        Self::spawn_in_with_lease(
            expected_key,
            "test_gateway::validated_gateway_fixture_process",
            Some(run_dir),
        )
    }

    /// Starts the fixture in the current test binary through `fixture_test`.
    ///
    /// The named ignored test must call
    /// [`run_validated_gateway_fixture_process`].
    ///
    /// # Panics
    /// Panics when the fixture listener, copied test image, child process, or
    /// readiness handshake cannot be created.
    #[must_use]
    pub fn spawn_in(expected_key: &str, fixture_test: &str) -> Self {
        Self::spawn_in_with_lease(expected_key, fixture_test, None)
    }

    #[expect(
        clippy::expect_used,
        reason = "test fixture setup fails immediately with the failed invariant"
    )]
    fn spawn_in_with_lease(
        expected_key: &str,
        fixture_test: &str,
        lease_run_dir: Option<&std::path::Path>,
    ) -> Self {
        let control = TcpListener::bind("127.0.0.1:0").expect("bind fixture control");
        control
            .set_nonblocking(true)
            .expect("make fixture control nonblocking");
        let directory = tempfile::TempDir::new().expect("create fixture executable directory");
        let executable = directory.path().join(GATEWAY_EXE_NAME);
        std::fs::copy(
            std::env::current_exe().expect("locate test executable"),
            &executable,
        )
        .expect("copy test executable under the Gateway image name");
        let mut command = Command::new(&executable);
        command
            .args(["--exact", fixture_test, "--ignored"])
            .env(
                CONTROL_ADDRESS_ENV,
                control
                    .local_addr()
                    .expect("read fixture control address")
                    .to_string(),
            )
            .env(EXPECTED_KEY_ENV, expected_key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(run_dir) = lease_run_dir {
            command.env(INSTANCE_LEASE_RUN_DIR_ENV, run_dir);
        }
        let mut child = command
            .spawn()
            .expect("start the named Gateway fixture process");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match control.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        child.try_wait().expect("observe fixture process").is_none(),
                        "the named Gateway fixture exited before becoming ready"
                    );
                    assert!(
                        Instant::now() < deadline,
                        "the named Gateway fixture did not become ready"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept fixture control connection: {error}"),
            }
        };
        let mut port = [0_u8; 2];
        stream
            .read_exact(&mut port)
            .expect("read fixture Gateway port");
        Self {
            child,
            port: u16::from_be_bytes(port),
            control: stream,
            _directory: directory,
        }
    }

    /// Returns the fixture's loopback port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Produces a production-validated capability for this child.
    ///
    /// # Panics
    /// Panics when the supplied bearer or boot identity does not validate
    /// against the running fixture.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "test fixture validation fails immediately with the failed invariant"
    )]
    pub fn validate(&self, api_key: &str, epoch: u64, started_at: &str) -> ValidatedConnection {
        ValidatedConnection::validate(self.connection_file(api_key, epoch, started_at))
            .expect("the named local Gateway validates")
    }

    /// Builds a connection file naming this child and the supplied boot data.
    #[must_use]
    pub fn connection_file(&self, api_key: &str, epoch: u64, started_at: &str) -> ConnectionFile {
        ConnectionFile {
            port: self.port,
            api_key: api_key.to_owned(),
            pid: self.child.id(),
            epoch,
            version: "test".to_owned(),
            started_at: started_at.to_owned(),
        }
    }

    /// Waits up to `timeout` for an authenticated shutdown request.
    ///
    /// # Panics
    /// Panics when the control socket cannot be configured or read, or when
    /// the child sends an invalid control marker.
    #[expect(
        clippy::expect_used,
        reason = "test fixture observation fails immediately with the failed invariant"
    )]
    pub fn received_shutdown(&mut self, timeout: Duration) -> bool {
        self.control
            .set_read_timeout(Some(timeout))
            .expect("set fixture control timeout");
        let mut marker = [0_u8; 1];
        match self.control.read_exact(&mut marker) {
            Ok(()) => {
                assert_eq!(marker, [1], "the fixture reports only shutdown requests");
                true
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                false
            }
            Err(error) => panic!("read fixture shutdown marker: {error}"),
        }
    }

    #[cfg(test)]
    fn terminate_within(&mut self, timeout: Duration) {
        if self
            .child
            .try_wait()
            .expect("observe named Gateway fixture")
            .is_none()
        {
            let _ = self.child.kill();
        }
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .child
                .try_wait()
                .expect("observe named Gateway fixture")
                .is_some()
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the named Gateway fixture did not stop within {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ValidatedGateway {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
#[test]
#[ignore = "runs only as a named child process"]
fn validated_gateway_fixture_process() {
    run_validated_gateway_fixture_process();
}

#[cfg(test)]
#[test]
fn a_named_fixture_releases_its_process_lease_when_terminated() {
    let run_dir = tempfile::TempDir::new().expect("create lease run directory");
    let mut gateway = ValidatedGateway::spawn_with_instance_lease("fixture-key", run_dir.path());
    assert!(
        shared_sidecar::GatewayInstanceLease::try_acquire(run_dir.path())
            .expect("contend for the named fixture lease")
            .is_none(),
        "the live fixture owns the process lease"
    );

    gateway.terminate_within(Duration::from_secs(5));
    assert!(
        shared_sidecar::GatewayInstanceLease::try_acquire(run_dir.path())
            .expect("recover the named fixture lease")
            .is_some(),
        "the operating system releases the lease when the named fixture dies"
    );
}

/// Runs the child half of [`ValidatedGateway`] inside an ignored test.
///
/// This function never returns during a successful fixture run.
///
/// # Panics
/// Panics when required fixture environment, listener, control connection,
/// request acceptance, or shutdown reporting cannot be established.
pub fn run_validated_gateway_fixture_process() {
    process::run();
}
