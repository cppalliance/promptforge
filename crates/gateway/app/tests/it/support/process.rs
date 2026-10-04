//! Real-binary Gateway child processes whose teardown stays bounded even when a
//! race test panics.

use std::io::Read as _;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const TEST_START_READY_ENV: &str = "PROMPTFORGE_GATEWAY_TEST_START_READY";
const TEST_START_RELEASE_ENV: &str = "PROMPTFORGE_GATEWAY_TEST_START_RELEASE";

/// A real Gateway child whose teardown is bounded even when a race test
/// panics before its ordinary shutdown path.
pub(crate) struct GatewayProcess {
    child: Child,
}

impl GatewayProcess {
    /// Starts the production binary against an isolated profile and config.
    pub(crate) fn spawn(config: &Path, home: &Path) -> Self {
        Self::spawn_command(config, home)
            .spawn()
            .map(|child| Self { child })
            .expect("the Gateway race fixture spawns")
    }

    /// Starts the production binary with `profile` as its `--profile`, or
    /// with no `--profile` at all, so the boot resolves the selection from
    /// the environment and the sibling state file the way a restart does.
    pub(crate) fn spawn_selecting(config: &Path, home: &Path, profile: Option<&str>) -> Self {
        Self::command_selecting(config, home, profile)
            .spawn()
            .map(|child| Self { child })
            .expect("the Gateway fixture spawns")
    }

    /// Starts the production binary paused immediately before lease
    /// acquisition until `release` exists.
    #[cfg(feature = "test-fixtures")]
    pub(crate) fn spawn_gated(config: &Path, home: &Path, ready: &Path, release: &Path) -> Self {
        let child = Self::spawn_command(config, home)
            .env(TEST_START_READY_ENV, ready)
            .env(TEST_START_RELEASE_ENV, release)
            .spawn()
            .expect("the gated Gateway race fixture spawns");
        Self { child }
    }

    /// Starts the production binary with extra environment layered on the
    /// isolated home. The child command line is the only safe way to hand
    /// a spawned gateway a variable (edition 2024 makes `env::set_var`
    /// unsafe, which the workspace forbids).
    pub(crate) fn spawn_with_env(config: &Path, home: &Path, envs: &[(&str, &str)]) -> Self {
        let mut command = Self::spawn_command(config, home);
        command.envs(envs.iter().copied());
        command
            .spawn()
            .map(|child| Self { child })
            .expect("the Gateway fixture spawns")
    }

    /// Starts the default binary with rendezvous-looking environment that
    /// must be inert when the test fixture feature is absent.
    #[cfg(not(feature = "test-fixtures"))]
    fn spawn_with_inert_rendezvous(
        config: &Path,
        home: &Path,
        ready: &Path,
        release: &Path,
    ) -> Self {
        let child = Self::spawn_command(config, home)
            .env(TEST_START_READY_ENV, ready)
            .env(TEST_START_RELEASE_ENV, release)
            .spawn()
            .expect("the default Gateway fixture spawns");
        Self { child }
    }

    fn spawn_command(config: &Path, home: &Path) -> Command {
        Self::command_selecting(config, home, Some("main"))
    }

    fn command_selecting(config: &Path, home: &Path, profile: Option<&str>) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"));
        command.arg("--config").arg(config);
        if let Some(profile) = profile {
            command.arg("--profile").arg(profile);
        }
        command
            .arg("--print-url")
            .env("USERPROFILE", home)
            .env("HOME", home)
            .env_remove("RUST_LOG")
            .env_remove("PROMPTFORGE_PROFILE")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child
            .try_wait()
            .expect("observe the Gateway race fixture")
    }

    pub(crate) fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the Gateway race fixture did not exit within {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn stdout(&mut self) -> String {
        let mut output = String::new();
        self.child
            .stdout
            .take()
            .expect("the Gateway fixture has piped stdout")
            .read_to_string(&mut output)
            .expect("read the Gateway fixture stdout");
        output
    }

    pub(crate) fn stderr(&mut self) -> String {
        let mut output = String::new();
        self.child
            .stderr
            .take()
            .expect("the Gateway fixture has piped stderr")
            .read_to_string(&mut output)
            .expect("read the Gateway fixture stderr");
        output
    }

    pub(crate) fn stop(&mut self, timeout: Duration) {
        if self.try_wait().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.wait_for_exit(timeout);
    }
}

impl Drop for GatewayProcess {
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
