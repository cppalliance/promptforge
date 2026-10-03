#![doc = include_str!("lib.md")]

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

pub mod cancel {
    #![doc = include_str!("cancel.md")]

    pub use harness_runner::cancel::CancelHandle;
    pub use harness_runner::cancel::current;
    pub use harness_runner::cancel::is_cancelled;
    pub use harness_runner::cancel::maybe_scope;
    pub use harness_runner::cancel::scope;
    pub use harness_runner::cancel::wait_cancelled;
}

pub mod capability {
    #![doc = include_str!("capability.md")]

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
    #![doc = include_str!("record.md")]

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
    #![doc = include_str!("vfs.md")]

    pub use promptforge::vfs::Origin;
    pub use promptforge::vfs::VfsError;
    pub use promptforge::vfs::VfsRef;
}
