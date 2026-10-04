//! Guarding and watching the files a Host hands a run: a policy refuses an
//! undeclared path before the operation watcher sees it, and while the run
//! waits on its question, the Host's edit of a file the run read conflicts
//! and leaves the run's view intact.

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use harness::capability::{
    CapabilityRegistry, HostServices, INPUT_BROKER, InputBroker, InputError, UserInput,
};
use harness::record::MemoryRecorder;
use harness::vfs::{Origin, VfsError, VfsRef};
use harness::{Harness, HostSnapshot, RunRequest};
use promptforge::vfs::{MemoryBackend, Op, Policy, Verdict, VfsPath};

use crate::support::{Clock, Offline};

/// Reads the notes, asks the operator, and writes the approved notes.
const REVIEW: &str = concat!(
    "---\nname: review\ndescription: Reads the notes, then asks the operator\npromptforge: 0\n",
    "capabilities:\n  - promptforge/user-input\n",
    "input: { path: notes.md, description: The operator's notes }\n",
    "output: { path: summary.md, description: The approved notes }\n",
    "---\n\n# Review\n\n## Approve\n\n```lua\n",
    "local notes = store.read('notes.md')\n",
    "local answer = input.ask()\n",
    "store.write('summary.md', notes .. ' ' .. answer)\n",
    "```\n",
);

/// Each watched operation's origin label and file.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Refuses every path except the two files the review prompt declares.
struct DeskFiles;

impl Policy for DeskFiles {
    fn check(&self, _op: Op, path: &VfsPath) -> Verdict {
        if ["/notes.md", "/summary.md"].contains(&path.as_str()) {
            Verdict::Allow
        } else {
            Verdict::Deny(format!("{path} is not a desk file"))
        }
    }
}

/// A memory store behind [`DeskFiles`], beside the origins its watcher
/// saw.
fn desk_vfs() -> (VfsRef, Seen) {
    let seen = Seen::default();
    let sink = Arc::clone(&seen);
    let vfs = VfsRef::builder()
        .store("/", MemoryBackend::new())
        .policy(DeskFiles)
        .on_op(move |event| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((event.origin().label.clone(), event.origin().file.clone()));
        })
        .build();
    (vfs, seen)
}

/// The Host's input broker: before it answers, the Host edits the notes
/// the run already read, and keeps how that edit went.
struct Edit {
    vfs: VfsRef,
    outcome: Arc<Mutex<Option<Result<(), VfsError>>>>,
}

#[async_trait]
impl InputBroker for Edit {
    async fn wait(&self) -> Result<String, InputError> {
        let edit = self
            .vfs
            .acquire_store(Origin::new("desk edit"))
            .and_then(|view| view.write("notes.md", b"Ship on Monday."));
        *self.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Some(edit);
        Ok("Approved.".to_owned())
    }
}

#[test]
fn a_policy_refuses_an_undeclared_path_before_the_watcher_sees_it() {
    let (vfs, seen) = desk_vfs();
    vfs.acquire_store(Origin::new("desk seed"))
        .expect("the handle declares a store")
        .write("notes.md", b"Ship on Friday.")
        .expect("the policy allows a declared file");
    let stray = vfs
        .acquire_store(Origin::new("desk stray"))
        .expect("the handle declares a store")
        .write("secret.md", b"x");
    assert!(matches!(stray, Err(VfsError::PermissionDenied { .. })));
    assert_eq!(
        seen.lock().unwrap()[..],
        [("desk seed".to_owned(), file!().to_owned())]
    );
}

#[tokio::test]
async fn a_hosts_edit_of_a_file_the_run_read_conflicts_while_the_run_waits_on_its_question() {
    let (vfs, seen) = desk_vfs();
    vfs.acquire_store(Origin::new("desk seed"))
        .expect("the handle declares a store")
        .write("notes.md", b"Ship on Friday.")
        .expect("the policy allows a declared file");

    let edited = Arc::new(Mutex::new(None));
    let operator: Arc<dyn InputBroker> = Arc::new(Edit {
        vfs: vfs.clone(),
        outcome: Arc::clone(&edited),
    });
    let mut services = HostServices::new();
    services
        .provide(&INPUT_BROKER, operator)
        .expect("the input broker is provided once");
    let mut capabilities = CapabilityRegistry::new();
    capabilities
        .register(Arc::new(UserInput::new()))
        .expect("the user input capability registers once");
    let harness = Harness::new(
        Arc::new(MemoryRecorder::new()),
        Arc::new(Offline),
        Arc::new(Clock),
        capabilities,
        services,
    );
    let request = RunRequest {
        name: "desk-review-1".to_owned(),
        source: REVIEW.to_owned(),
        args: String::new(),
        input_text: None,
        vfs: vfs.clone(),
        host: HostSnapshot::default(),
    };
    let report = harness
        .run(request)
        .await
        .expect("the run reaches an outcome");

    assert!(matches!(
        edited.lock().unwrap().take(),
        Some(Err(VfsError::Conflict { .. }))
    ));
    let notes = vfs
        .acquire_store(Origin::new("desk check"))
        .expect("the handle declares a store")
        .read_string("notes.md")
        .expect("the notes stay readable");
    assert_eq!(notes, "Ship on Friday.");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|(_, file)| file == "Review")
    );
    assert_eq!(
        report.output.expect("the run completed"),
        "Ship on Friday. Approved."
    );
}
