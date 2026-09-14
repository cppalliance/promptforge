//! Prepare-pass integration tests: capability resolution against the
//! registry (missing required reported, absent optional skipped and
//! logged), the run's services reaching `create`, activation failure
//! semantics, and the per-run VFS claims isolation matrix.

use std::io;
use std::sync::{Arc, Mutex};

use promptforge_api::capabilities::CapabilityRegistry;
use promptforge_api::execute::{Environment, RunContext};
use promptforge_api::parser::Prompt;
use shared_promptforge_api::cancel::CancelHandle;
use shared_promptforge_api::capabilities::{
    Capability, CapabilityError, CapabilityId, Contribution, RunServices,
};
use shared_promptforge_api::observe::NullObserver;
use shared_vfs::{HostBackend, Origin, VfsError, VfsRef};

/// A prompt declaring `promptforge/web` as a required capability.
const DECLARES_REQUIRED: &str = concat!(
    "---\n",
    "name: declares-required\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `promptforge/web` as an optional capability.
const DECLARES_OPTIONAL: &str = concat!(
    "---\n",
    "name: declares-optional\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - ref: promptforge/web\n",
    "    optional: true\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring no capabilities at all.
const DECLARES_NOTHING: &str = concat!(
    "---\n",
    "name: declares-nothing\n",
    "description: d\n",
    "promptforge: 0\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// Parses a fixture prompt.
fn parse(source: &str, execution: &str) -> Prompt {
    Prompt::parse(source, execution, &NullObserver::default()).expect("the fixture prompt parses")
}

/// What one activation observed: the marker round-trip through the
/// services VFS and the cancellation handle it was handed.
#[derive(Debug)]
struct Activation {
    /// The marker read back through the services VFS, when it round-tripped.
    marker: Option<String>,
    /// The cancellation handle `create` received.
    cancel: CancelHandle,
}

/// A fixture capability recording each activation's services. `fail`
/// turns every activation into a [`CapabilityError`].
struct Fixture {
    id: CapabilityId,
    description: String,
    fail: bool,
    activations: Arc<Mutex<Vec<Activation>>>,
}

impl Fixture {
    /// Builds a fixture capability registered under `id`.
    fn new(id: &str, fail: bool) -> (Arc<Fixture>, Arc<Mutex<Vec<Activation>>>) {
        let activations = Arc::new(Mutex::new(Vec::new()));
        let fixture = Arc::new(Fixture {
            id: CapabilityId::parse(id).expect("the fixture id is valid"),
            description: format!("The {id} fixture capability."),
            fail,
            activations: Arc::clone(&activations),
        });
        (fixture, activations)
    }
}

impl Capability for Fixture {
    fn id(&self) -> &CapabilityId {
        &self.id
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
        if self.fail {
            return Err(CapabilityError::message("the fixture cannot activate"));
        }
        let path = format!("{}/activated.txt", promptforge_vfs::STORE_MOUNT);
        let access = services
            .vfs
            .acquire(Origin::new("fixture activation"))
            .map_err(|error| {
                CapabilityError::with_source("the fixture could not acquire", error)
            })?;
        access
            .write(&path, b"active")
            .map_err(|error| CapabilityError::with_source("the fixture could not write", error))?;
        let marker = access
            .read(&path)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        self.activations
            .lock()
            .expect("the activations lock is not poisoned")
            .push(Activation {
                marker,
                cancel: services.cancel.clone(),
            });
        Ok(Contribution::default())
    }
}

/// A shared buffer a fmt subscriber writes log lines into.
#[derive(Clone, Default)]
struct Buffer {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl io::Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes
            .lock()
            .expect("the buffer lock is not poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Runs `f` under a fmt subscriber writing into a shared buffer and
/// returns everything the subscriber captured.
fn captured_logs(f: impl FnOnce()) -> String {
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = buffer
        .bytes
        .lock()
        .expect("the buffer lock is not poisoned");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A unique temporary directory that removes itself on drop. The suite has
/// no tempfile dependency; this mirrors shared-vfs's own test helper.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "promptforge-api-prepare-{}-{unique}-{name}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).expect("the temp dir creates");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_missing_required_capability_is_reported() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let env = Environment::new();
    let (_ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-missing"));
    assert!(requirements.unmet_requirements.is_empty());
    assert_eq!(
        requirements.missing_required,
        [CapabilityId::parse("promptforge/web").expect("the id is valid")]
    );
    assert!(!requirements.is_satisfied());
}

#[test]
fn an_absent_optional_capability_is_skipped_and_logged() {
    let prompt = parse(DECLARES_OPTIONAL, "declares-optional");
    let env = Environment::new();
    let logs = captured_logs(|| {
        let (_ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-optional"));
        assert!(requirements.missing_required.is_empty());
        assert!(requirements.is_satisfied());
    });
    assert!(
        logs.contains("promptforge/web"),
        "the skip log line names the capability: {logs}"
    );
}

#[test]
fn activation_receives_the_runs_own_services() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let (fixture, activations) = Fixture::new("promptforge/web", false);
    let mut registry = CapabilityRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let env = Environment::new().registry(registry);
    let cancel = CancelHandle::new();
    let (ctx, requirements) = env.prepare(
        &prompt,
        RunContext::new("prepare-services").cancel(cancel.clone()),
    );
    assert!(requirements.is_satisfied());
    // The host-supplied cancellation handle reached `create` unchanged.
    let activations = activations.lock().expect("the lock is not poisoned");
    assert_eq!(activations.len(), 1, "create ran exactly once");
    assert_eq!(activations[0].marker.as_deref(), Some("active"));
    assert!(!activations[0].cancel.is_cancelled());
    cancel.cancel();
    assert!(
        activations[0].cancel.is_cancelled(),
        "the activated handle is the run's own"
    );
    drop(activations);
    // The services VFS is the run's prepared handle: the activation's
    // marker is readable through the context's store mount.
    let access = ctx
        .vfs_handle()
        .acquire(Origin::new("post-prepare read"))
        .expect("the prepared handle acquires");
    let marker = format!("{}/activated.txt", promptforge_vfs::STORE_MOUNT);
    assert_eq!(
        access.read(&marker).expect("the marker persists"),
        b"active"
    );
}

#[test]
fn a_required_activation_failure_is_logged_and_reported() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let (fixture, _activations) = Fixture::new("promptforge/web", true);
    let mut registry = CapabilityRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let env = Environment::new().registry(registry);
    let logs = captured_logs(|| {
        // A present-but-failing required capability leaves the run
        // without something the prompt declared: it is reported like an
        // absent one, and the failure is also a log line.
        let (_ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-failing"));
        assert_eq!(
            requirements.missing_required,
            [CapabilityId::parse("promptforge/web").expect("the id is valid")]
        );
        assert!(!requirements.is_satisfied());
    });
    assert!(
        logs.contains("promptforge/web"),
        "the failure log line names the capability: {logs}"
    );
}

#[test]
fn an_optional_activation_failure_is_logged_and_contributes_nothing() {
    let prompt = parse(DECLARES_OPTIONAL, "declares-optional");
    let (fixture, _activations) = Fixture::new("promptforge/web", true);
    let mut registry = CapabilityRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let env = Environment::new().registry(registry);
    let logs = captured_logs(|| {
        // An optional capability that fails to activate is only a log
        // line: the prompt declared it could run without.
        let (_ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-failing"));
        assert!(requirements.is_satisfied());
    });
    assert!(
        logs.contains("promptforge/web"),
        "the failure log line names the capability: {logs}"
    );
}

#[test]
fn two_runs_writing_the_same_store_path_do_not_conflict() {
    let prompt = parse(DECLARES_NOTHING, "declares-nothing");
    let env = Environment::new();
    let (ctx_a, _) = env.prepare(&prompt, RunContext::new("run-a"));
    let (ctx_b, _) = env.prepare(&prompt, RunContext::new("run-b"));
    let access_a = ctx_a
        .vfs_handle()
        .acquire(Origin::new("run-a"))
        .expect("run a acquires");
    let access_b = ctx_b
        .vfs_handle()
        .acquire(Origin::new("run-b"))
        .expect("run b acquires");
    let path = format!("{}/paper.md", promptforge_vfs::STORE_MOUNT);
    // Both writes proceed while both accesses are live: each run's store
    // is its own storage under its own claims table.
    access_a.write(&path, b"from a").expect("run a writes");
    access_b.write(&path, b"from b").expect("run b writes");
    assert_eq!(access_a.read(&path).expect("run a reads"), b"from a");
    assert_eq!(access_b.read(&path).expect("run b reads"), b"from b");
}

#[test]
fn two_runs_writing_the_same_host_file_through_the_shared_base_conflict() {
    let temp = TempDir::new("shared-base");
    let base = VfsRef::builder()
        .mount(
            "/",
            HostBackend::rooted(&temp.0).expect("the temp dir roots the host backend"),
        )
        .build();
    let env = Environment::new().base_vfs(base);
    let prompt = parse(DECLARES_NOTHING, "declares-nothing");
    let (ctx_a, _) = env.prepare(&prompt, RunContext::new("run-a"));
    let (ctx_b, _) = env.prepare(&prompt, RunContext::new("run-b"));
    let access_a = ctx_a
        .vfs_handle()
        .acquire(Origin::new("run-a"))
        .expect("run a acquires");
    access_a
        .write("/shared.txt", b"from a")
        .expect("run a writes the host file");
    let access_b = ctx_b
        .vfs_handle()
        .acquire(Origin::new("run-b"))
        .expect("run b acquires");
    // The shared base's claims table sees two live identities on one path.
    let error = access_b
        .write("/shared.txt", b"from b")
        .expect_err("run b conflicts with run a's live claim");
    assert!(
        matches!(error, VfsError::Conflict(_)),
        "a determinism violation, not a backend error: {error}"
    );
    // The conflicting write never partially applied.
    assert_eq!(
        std::fs::read(temp.0.join("shared.txt")).expect("run a's write landed on disk"),
        b"from a"
    );
}
