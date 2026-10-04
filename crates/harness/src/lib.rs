//! The Harness a Host builds for each run. [`Harness::new`] takes the
//! Host's recorder, inference broker, timer, capability registry, and
//! services, [`Harness::run`] runs one prompt to its [`RunReport`], and the
//! Host steers that run through its [`RunControl`].
//!
//! The crate's `run-prompt` example runs one prompt from start to end.

pub use harness_capabilities::USER_INPUT_ASK_TOOL;
pub use harness_runner::Harness;
pub use harness_runner::HarnessError;
pub use harness_runner::RunControl;
pub use harness_runner::RunReport;
pub use harness_runner::RunRequest;
pub use harness_runner::display_chain;
pub use harness_runner::environment::CurrentModelError;
pub use harness_runner::environment::HostSnapshot;
pub use harness_runner::files::OutputError;
pub use harness_runner::performers::BoxFuture;
pub use harness_runner::performers::InferenceBroker;
pub use harness_runner::performers::Timer;

pub mod capability {
    //! Capabilities, the registry a Host installs them in, and the services
    //! they read.

    pub use harness_capabilities::Capability;
    pub use harness_capabilities::CapabilityError;
    pub use harness_capabilities::CapabilityErrorKind;
    pub use harness_capabilities::CapabilityRegistry;
    pub use harness_capabilities::Contribution;
    pub use harness_capabilities::HostServices;
    pub use harness_capabilities::INPUT_BROKER;
    pub use harness_capabilities::InputBroker;
    pub use harness_capabilities::InputError;
    pub use harness_capabilities::RegistryError;
    pub use harness_capabilities::RegistryErrorKind;
    pub use harness_capabilities::RunServices;
    pub use harness_capabilities::ServiceError;
    pub use harness_capabilities::ServiceId;
    pub use harness_capabilities::ServiceKey;
    pub use harness_capabilities::Tool;
    pub use harness_capabilities::UserInput;
    pub use promptforge::capabilities::CapabilityId;
}

pub mod record {
    //! The recorder a run writes every effect, answer, and event to, and an
    //! in-memory recorder.

    pub use harness_runner::recorder::MemoryRecorder;
    pub use harness_runner::recorder::Record;
    pub use harness_runner::recorder::RecordKind;
    pub use harness_runner::recorder::RecorderError;
    pub use harness_runner::recorder::RecorderFuture;
    pub use harness_runner::recorder::RunId;
    pub use harness_runner::recorder::RunMeta;
    pub use harness_runner::recorder::RunOutcome;
    pub use harness_runner::recorder::RunRecorder;
}

pub mod vfs {
    //! The file handle a Host hands a run, and the origin each operation
    //! carries.

    pub use promptforge::vfs::Origin;
    pub use promptforge::vfs::VfsError;
    pub use promptforge::vfs::VfsRef;
}
