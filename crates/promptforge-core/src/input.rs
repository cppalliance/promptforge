//! The generic input broker: one host policy behind user input.
//!
//! Two surfaces consume the same broker. A section's direct
//! `user_input()` call suspends on the broker and resumes with
//! `(text, available)`: `available` is `true` for real operator text and
//! `false` when the host had no input, in which case `text` is the fixed
//! [`INPUT_UNAVAILABLE_FALLBACK`] sentence. The flag rides beside the
//! text, so a human typing exactly the fallback sentence can never spoof
//! the unavailable state. The model-visible [`InputTool`] adapts the same
//! broker for a `models.loop` tool scope: the model's call suspends on
//! the broker and its answer lands as the correlated tool result.
//!
//! The host policies are the broker's: a blocking broker parks the wait
//! until the host delivers (the section's VM and message history stay
//! intact), an unavailable answer (or no configured broker at all) is the
//! unavailable-fallback policy, and a broker error is the failure policy,
//! raising a typed [`RunErrorKind::Input`](crate::RunErrorKind::Input)
//! failure at the Lua call site. Waits and responses are recorded through
//! the run's [`Observer`] - a wait-opened observation and
//! a byte-exact `on_user_input` report - without any replay machinery.

use std::fmt;
use std::sync::Arc;

use crate::observe::{Observer, detail};
use crate::tools::{Tool, ToolError, ToolId, ToolOutput};

/// The fixed sentence a `user_input` call or [`InputTool`] result carries
/// when the host has no input to give.
///
/// The sentence is deliberately unremarkable: the availability flag, not
/// the text, distinguishes the fallback from operator input, so the
/// sentence never needs to be unguessable.
pub const INPUT_UNAVAILABLE_FALLBACK: &str =
    "User input is unavailable in this host; continue without it.";

/// What the broker produced for one input request.
///
/// `#[non_exhaustive]`: a future policy (for example a deferred
/// continuation-capable wait) can add variants without breaking
/// implementors.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputOutcome {
    /// The operator supplied text, delivered byte-exact.
    Text(String),
    /// The host had no input to give: the call resolves to
    /// [`INPUT_UNAVAILABLE_FALLBACK`] with `available` false.
    Unavailable,
}

/// A broker's failure to produce input.
///
/// The message is host-authored and safe to surface at the Lua call site
/// and (through [`InputTool`]) to the model; an underlying cause hides
/// behind [`std::error::Error::source`].
#[derive(Debug)]
#[non_exhaustive]
pub struct InputError {
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl InputError {
    /// Builds a failure carrying only a message.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::input::InputError;
    ///
    /// let error = InputError::message("the input device is gone");
    /// assert_eq!(error.to_string(), "the input device is gone");
    /// ```
    #[must_use]
    pub fn message(text: impl Into<String>) -> InputError {
        InputError {
            message: text.into(),
            source: None,
        }
    }

    /// Builds a failure with `source` as the hidden `#[source]` cause.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::input::InputError;
    ///
    /// let cause = std::io::Error::other("socket reset");
    /// let error = InputError::with_source("the input device is gone", cause);
    /// assert!(std::error::Error::source(&error).is_some());
    /// ```
    #[must_use]
    pub fn with_source(
        text: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> InputError {
        InputError {
            message: text.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Dissolves the error into its message and optional cause.
    pub(crate) fn into_parts(self) -> (String, Option<Box<dyn std::error::Error + Send + Sync>>) {
        (self.message, self.source)
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for InputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// The host policy behind user input: one asynchronous request per wait.
///
/// The executor calls [`user_input`](Self::user_input) when a section's
/// `user_input()` runs or the model-visible [`InputTool`] is dispatched,
/// and suspends the caller until the future resolves. An implementation
/// that blocks until its host delivers input is the blocking policy;
/// answering [`InputOutcome::Unavailable`] is the unavailable-fallback
/// policy; an [`InputError`] is the failure policy. Implementations must
/// be `Send + Sync`, must not panic, and should return promptly when the
/// host tears the wait down.
#[async_trait::async_trait]
pub trait InputBroker: Send + Sync {
    /// Waits for the host's answer to one input request for `section` of
    /// `execution`.
    ///
    /// # Errors
    /// Returns an [`InputError`] when the host fails the wait rather than
    /// answering or declining it.
    async fn user_input(&self, execution: &str, section: &str) -> Result<InputOutcome, InputError>;
}

/// The model-visible input tool: adapts the run's [`InputBroker`] into a
/// [`Tool`] a `models.loop` tool scope can advertise, so the model can
/// ask the operator mid-loop and the answer lands as the correlated tool
/// result.
///
/// The tool is a host primitive, constructed per run (or session) with
/// the reporting coordinates its observations carry. Its output is
/// trusted plain text: the operator's answer byte-exact, or
/// [`INPUT_UNAVAILABLE_FALLBACK`] when the broker declines. A broker
/// failure surfaces as a narrow [`ToolError`].
pub struct InputTool {
    /// The broker every call waits on.
    broker: Arc<dyn InputBroker>,
    /// The execution identifier the tool's observations carry.
    execution: String,
    /// The section name the tool's observations carry.
    section: String,
    /// Where waits and responses are recorded.
    observer: Arc<dyn Observer>,
}

impl fmt::Debug for InputTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputTool")
            .field("execution", &self.execution)
            .field("section", &self.section)
            .finish_non_exhaustive()
    }
}

impl InputTool {
    /// Builds the tool over `broker`, reporting under `execution` and
    /// `section` to `observer`.
    ///
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    ///
    /// use promptforge_core::input::{InputBroker, InputError, InputOutcome, InputTool};
    /// use promptforge_core::observe::NullObserver;
    ///
    /// struct Console;
    ///
    /// #[async_trait::async_trait]
    /// impl InputBroker for Console {
    ///     async fn user_input(
    ///         &self,
    ///         _execution: &str,
    ///         _section: &str,
    ///     ) -> Result<InputOutcome, InputError> {
    ///         Ok(InputOutcome::Unavailable)
    ///     }
    /// }
    ///
    /// let tool = InputTool::new(
    ///     Arc::new(Console),
    ///     "example-run",
    ///     "chat",
    ///     Arc::new(NullObserver::default()),
    /// );
    /// ```
    #[must_use]
    pub fn new(
        broker: Arc<dyn InputBroker>,
        execution: &str,
        section: &str,
        observer: Arc<dyn Observer>,
    ) -> InputTool {
        InputTool {
            broker,
            execution: execution.to_owned(),
            section: section.to_owned(),
            observer,
        }
    }
}

#[async_trait::async_trait]
impl Tool for InputTool {
    fn id(&self) -> ToolId {
        ToolId::from_validated("promptforge", "user_input")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "user_input"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Give the user the opportunity to add a prompt and wait for their typed input."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    /// Opens one broker wait and suspends until it resolves.
    ///
    /// The wait is recorded through the observer before the broker is
    /// called, and an operator answer is recorded byte-exact through
    /// `on_user_input`. Arguments are accepted but unused in the active
    /// contract: the broker owns how the wait reaches the operator.
    ///
    /// # Errors
    /// Returns a [`ToolError`] carrying the broker's message and cause
    /// when the broker fails the wait.
    async fn call(&self, _args: serde_json::Value) -> Result<ToolOutput, ToolError> {
        self.observer.observe(
            &self.execution,
            &self.section,
            detail::USER_INPUT_WAIT_STARTED,
        );
        match self.broker.user_input(&self.execution, &self.section).await {
            Ok(InputOutcome::Text(text)) => {
                self.observer
                    .on_user_input(&self.execution, &self.section, &text);
                Ok(ToolOutput::trusted(text))
            }
            Ok(InputOutcome::Unavailable) => Ok(ToolOutput::trusted(INPUT_UNAVAILABLE_FALLBACK)),
            // The whole broker error rides as the source, so its own
            // cause stays reachable through the chain.
            Err(error) => {
                let message = error.to_string();
                Err(ToolError::with_source(message, error))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::observe::{NullObserver, Observation};

    /// A broker that always answers with the same operator text.
    struct TextBroker(&'static str);

    #[async_trait::async_trait]
    impl InputBroker for TextBroker {
        async fn user_input(
            &self,
            _execution: &str,
            _section: &str,
        ) -> Result<InputOutcome, InputError> {
            Ok(InputOutcome::Text(self.0.to_owned()))
        }
    }

    /// A broker reporting the host has no input to give.
    struct UnavailableBroker;

    #[async_trait::async_trait]
    impl InputBroker for UnavailableBroker {
        async fn user_input(
            &self,
            _execution: &str,
            _section: &str,
        ) -> Result<InputOutcome, InputError> {
            Ok(InputOutcome::Unavailable)
        }
    }

    /// A broker whose every request fails with a caused error.
    struct FailingBroker;

    #[async_trait::async_trait]
    impl InputBroker for FailingBroker {
        async fn user_input(
            &self,
            _execution: &str,
            _section: &str,
        ) -> Result<InputOutcome, InputError> {
            Err(InputError::with_source(
                "the input device is gone",
                std::io::Error::other("socket reset"),
            ))
        }
    }

    /// Records fixed observations and `on_user_input` content reports.
    #[derive(Default)]
    struct Recorder {
        events: std::sync::Mutex<Vec<Observation>>,
        inputs: std::sync::Mutex<Vec<String>>,
    }

    impl Observer for Recorder {
        fn observe(&self, _execution: &str, _section: &str, event: Observation) {
            self.events
                .lock()
                .expect("the recorder mutex must not be poisoned")
                .push(event);
        }

        fn on_user_input(&self, _execution: &str, _section: &str, text: &str) {
            self.inputs
                .lock()
                .expect("the recorder mutex must not be poisoned")
                .push(text.to_owned());
        }
    }

    fn tool(broker: Arc<dyn InputBroker>, observer: Arc<dyn Observer>) -> InputTool {
        InputTool::new(broker, "input-test", "Only", observer)
    }

    #[tokio::test]
    async fn the_tool_returns_operator_text_as_trusted_output_and_records_it() {
        let recorder = Arc::new(Recorder::default());
        let tool = tool(
            Arc::new(TextBroker("line1\r\nline2 \"quoted\" \u{1F980}")),
            recorder.clone() as Arc<dyn Observer>,
        );
        let output = tool
            .call(serde_json::json!({}))
            .await
            .expect("the broker answers");
        assert_eq!(
            output.trust(),
            promptforge_tools::OutputTrust::Trusted,
            "operator input is first-party: no guard wrap may apply"
        );
        assert_eq!(output.text(), "line1\r\nline2 \"quoted\" \u{1F980}");
        assert_eq!(
            recorder
                .events
                .lock()
                .expect("the recorder mutex must not be poisoned")
                .as_slice(),
            &[Observation::UserInputWaitStarted],
            "the wait is recorded exactly once"
        );
        assert_eq!(
            recorder
                .inputs
                .lock()
                .expect("the recorder mutex must not be poisoned")
                .as_slice(),
            &["line1\r\nline2 \"quoted\" \u{1F980}".to_owned()],
            "the response is recorded byte-exact"
        );
    }

    #[tokio::test]
    async fn an_unavailable_broker_answer_becomes_the_fallback_sentence() {
        let tool = tool(
            Arc::new(UnavailableBroker),
            Arc::new(NullObserver::default()) as Arc<dyn Observer>,
        );
        let output = tool
            .call(serde_json::json!({}))
            .await
            .expect("an unavailable answer is not a failure");
        assert_eq!(output.text(), INPUT_UNAVAILABLE_FALLBACK);
    }

    #[test]
    fn the_advertised_schema_has_no_question_and_the_description_promises_no_question_channel() {
        let tool = tool(
            Arc::new(UnavailableBroker),
            Arc::new(NullObserver::default()) as Arc<dyn Observer>,
        );
        let schema = tool.parameters_schema();
        let properties = schema["properties"]
            .as_object()
            .expect("the schema advertises a properties object");
        assert!(
            properties.is_empty(),
            "the tool takes no arguments: the broker owns how input is gathered, got {properties:?}"
        );
        let description = tool.description().to_lowercase();
        assert!(
            !description.contains("question") && !description.contains("ask"),
            "the description must not promise an ask-a-question channel, got: {description}"
        );
    }

    #[tokio::test]
    async fn a_broker_failure_is_a_tool_error_with_its_cause() {
        let tool = tool(
            Arc::new(FailingBroker),
            Arc::new(NullObserver::default()) as Arc<dyn Observer>,
        );
        let error = tool
            .call(serde_json::json!({}))
            .await
            .expect_err("the broker failure fails the call");
        assert_eq!(error.to_string(), "the input device is gone");
        assert!(
            std::error::Error::source(&error).is_some(),
            "the broker's cause survives as the tool error's source"
        );
    }
}
