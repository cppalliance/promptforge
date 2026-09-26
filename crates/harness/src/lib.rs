#![doc = include_str!("lib.md")]

pub use harness_runner::display_chain;
pub use harness_sessions::environment::CatalogBinding;
pub use harness_sessions::environment::GatewayBinding;
pub use harness_sessions::environment::HostSnapshot;
pub use harness_sessions::input::WaitError;
pub use harness_sessions::input::WaitFrame;
pub use harness_sessions::protocol::Delta;
pub use harness_sessions::protocol::DeltaKind;
pub use harness_sessions::protocol::LaunchRequest;
pub use harness_sessions::protocol::SessionEvent;
pub use harness_sessions::protocol::SessionId;
pub use harness_sessions::runtime::Harness;
pub use harness_sessions::runtime::HarnessConfig;
pub use harness_sessions::runtime::LaunchError;
pub use harness_sessions::session::FailureKind;
pub use harness_sessions::session::Session;
pub use harness_sessions::session::SessionFailure;
pub use harness_sessions::transition::SessionState;

pub mod cancel {
    #![doc = include_str!("cancel.md")]

    pub use harness_runner::cancel::CancelHandle;
    pub use harness_runner::cancel::current;
    pub use harness_runner::cancel::is_cancelled;
    pub use harness_runner::cancel::maybe_scope;
    pub use harness_runner::cancel::scope;
    pub use harness_runner::cancel::wait_cancelled;
}
