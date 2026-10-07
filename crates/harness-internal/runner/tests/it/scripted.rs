//! The scripted Host the Harness suites run against: a broker that lists
//! one model and plays each round from a script, performers whose every
//! future is held until it is torn down, a Plugin whose one tool is
//! held, and an operator who answers each question with the next text a
//! test sends.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use harness_runner::HostContext;
use harness_runner::performers::{BoxFuture, InferenceBroker, Timer};
use promptforge::effect::Round;
use promptforge::model::{
    Completion, CompletionError, CompletionErrorKind, CompletionOptions, CompletionResult, Message,
    ModelBinding, ModelCatalog, ModelDescriptor, ModelId, ThinkingMode, ToolSchema,
};
use promptforge_plugin::{
    HostServices, Package, Plugin, PluginFuture, PluginId, ServiceKey, ToolContext, ToolDescriptor,
    ToolError, ToolId, ToolOutput,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::asker::{ASKER, AskBroker, broker_services};

/// The one model the scripted broker lists.
pub(crate) const MODEL: &str = "m";

/// What a fake keeps, shared with the test that reads it.
pub(crate) type Kept<T> = Arc<Mutex<Vec<T>>>;

/// Plays one model round: the round and its messages in, its answer out.
type ChatScript = dyn Fn(Round, Vec<Message>) -> BoxFuture<Result<Box<Completion>, CompletionError>>
    + Send
    + Sync;

/// What a scripted broker's `models()` does.
#[derive(Clone, Copy)]
pub(crate) enum Listing {
    /// Lists the one scripted model.
    Lists,
    /// Fails as this kind with the kind's fixed phrase.
    Fails(CompletionErrorKind),
    /// Never answers.
    Holds,
}

/// Lists the scripted model and plays every round through its script,
/// keeping each round it was handed and counting the listings it was
/// asked for.
pub(crate) struct ScriptedBroker {
    listing: Listing,
    script: Arc<ChatScript>,
    rounds: Kept<Round>,
    listings: Arc<AtomicUsize>,
}

impl ScriptedBroker {
    /// A broker that plays every round through `script`.
    pub(crate) fn new(
        script: impl Fn(Round, Vec<Message>) -> BoxFuture<Result<Box<Completion>, CompletionError>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            listing: Listing::Lists,
            script: Arc::new(script),
            rounds: Kept::default(),
            listings: Arc::default(),
        }
    }

    /// A broker that replies `re: <last message>` to every round.
    pub(crate) fn replying() -> Self {
        Self::new(|_round, messages| {
            let asked = messages
                .last()
                .map(|message| message.content().to_owned())
                .unwrap_or_default();
            Box::pin(async move { reply(format!("re: {asked}")) })
        })
    }

    /// This broker with `listing` as what its `models()` does.
    pub(crate) fn listing(self, listing: Listing) -> Self {
        Self { listing, ..self }
    }

    /// The rounds this broker keeps, readable after it moved into a
    /// Harness.
    pub(crate) fn rounds(&self) -> Kept<Round> {
        Arc::clone(&self.rounds)
    }

    /// The count of listings this broker was asked for.
    pub(crate) fn listings(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.listings)
    }
}

impl InferenceBroker for ScriptedBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        self.listings.fetch_add(1, Ordering::SeqCst);
        match self.listing {
            Listing::Lists => Box::pin(async { Ok(catalog()) }),
            Listing::Fails(kind) => {
                Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
            }
            Listing::Holds => Box::pin(std::future::pending()),
        }
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        round: Round,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        self.rounds.lock().unwrap().push(round);
        (self.script)(round, messages)
    }
}

/// The scripted catalog: the one model, with a 131072-token window.
fn catalog() -> ModelCatalog {
    ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway(MODEL).expect("the model id is valid"),
        "The scripted model",
        NonZeroU32::new(131_072).expect("131072 is non-zero"),
        ThinkingMode::Never,
    )])
    .expect("one model is a valid catalog")
}

/// A text completion of `text` from the scripted model.
pub(crate) fn reply(text: impl Into<String>) -> Result<Box<Completion>, CompletionError> {
    Completion::from_result(CompletionResult::Text(text.into()), MODEL).map(Box::new)
}

/// Counts the futures a held performer started and the ones torn down. A
/// held future never resolves, so only a drop ends it.
#[derive(Default)]
pub(crate) struct Held {
    started: AtomicUsize,
    dropped: AtomicUsize,
}

impl Held {
    pub(crate) fn started(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }

    pub(crate) fn dropped(&self) -> usize {
        self.dropped.load(Ordering::SeqCst)
    }

    /// A future that counts itself started on its first poll and dropped
    /// when torn down, and never resolves.
    fn hold<T: Send + 'static>(self: &Arc<Self>) -> BoxFuture<T> {
        let held = Arc::clone(self);
        Box::pin(async move {
            held.started.fetch_add(1, Ordering::SeqCst);
            let _dropped = CountOnDrop(held);
            std::future::pending::<T>().await
        })
    }
}

/// Counts one drop on its [`Held`].
struct CountOnDrop(Arc<Held>);

impl Drop for CountOnDrop {
    fn drop(&mut self) {
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

/// A timer whose every sleep is held.
pub(crate) struct HeldTimer(pub(crate) Arc<Held>);

impl Timer for HeldTimer {
    fn sleep(&self, _seconds: f64) -> BoxFuture<()> {
        self.0.hold()
    }
}

/// A broker whose every round is held.
pub(crate) fn held_broker(held: &Arc<Held>) -> ScriptedBroker {
    let held = Arc::clone(held);
    ScriptedBroker::new(move |_round, _messages| held.hold())
}

/// The Host-wide service the hold Plugin's construct reads its [`Held`]
/// from.
const HELD: ServiceKey<Held> = ServiceKey::new("tests/held");

/// The fixture Plugin `harness`: one tool, `harness/hold`, whose every
/// call is held, and which a stop leaves in flight when its configuration
/// sets `survives_stop`.
const HOLD: Package = Package {
    name: "tests/harness",
    prelude: None,
    needs: &[],
    construct: construct_hold,
};

struct HoldPlugin {
    held: Arc<Held>,
    tools: Vec<ToolDescriptor>,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the Package construct signature fixes the argument types"
)]
fn construct_hold(
    name: &PluginId,
    config: Value,
    services: &HostServices,
) -> Result<Arc<dyn Plugin>, ToolError> {
    let held = services
        .get(&HELD)
        .ok_or_else(|| ToolError::message("the hold Plugin needs tests/held"))?;
    let descriptor = ToolDescriptor::new(
        ToolId::parse(&format!("{name}/hold")).unwrap(),
        "hold",
        "Hold until the call is dropped.",
        json!({ "type": "object", "properties": {} }),
    )
    .survives_stop(config["survives_stop"].as_bool().unwrap_or(false));
    Ok(Arc::new(HoldPlugin {
        held,
        tools: vec![descriptor],
    }))
}

impl Plugin for HoldPlugin {
    fn tools(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    fn call<'a>(
        &'a self,
        _cx: ToolContext<'a>,
        _args: Value,
    ) -> PluginFuture<'a, Result<ToolOutput, ToolError>> {
        self.held.hold()
    }
}

/// A Host holding the hold Plugin over `held`.
pub(crate) fn hold_host(held: &Arc<Held>) -> HostContext {
    host_holding(held, false)
}

/// A Host holding the hold Plugin over `held`, its tool marked to
/// survive a stop.
pub(crate) fn surviving_hold_host(held: &Arc<Held>) -> HostContext {
    host_holding(held, true)
}

fn host_holding(held: &Arc<Held>, survives_stop: bool) -> HostContext {
    let mut services = HostServices::new();
    services.provide(&HELD, Arc::clone(held)).unwrap();
    let mut host = HostContext::new(services);
    host.install(HOLD, None, json!({ "survives_stop": survives_stop }))
        .unwrap();
    host
}

/// `host` with the fixture ask Plugin installed as `user-input`.
pub(crate) fn with_asker(mut host: HostContext) -> HostContext {
    host.install(ASKER, None, Value::Null).unwrap();
    host
}

/// The operator: counts each question asked and each one torn down
/// unanswered, and answers each with the next text the test sends.
pub(crate) struct Operator {
    pub(crate) asked: Arc<AtomicUsize>,
    pub(crate) abandoned: Arc<AtomicUsize>,
    answers: tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>,
}

impl Operator {
    /// The operator, beside the sender the test answers through.
    pub(crate) fn new() -> (Arc<Self>, mpsc::UnboundedSender<String>) {
        let (answer, answers) = mpsc::unbounded_channel();
        let operator = Arc::new(Self {
            asked: Arc::default(),
            abandoned: Arc::default(),
            answers: tokio::sync::Mutex::new(answers),
        });
        (operator, answer)
    }

    /// Run services whose input broker is this operator.
    pub(crate) fn services(self: &Arc<Self>) -> HostServices {
        broker_services(Arc::clone(self) as Arc<dyn AskBroker>)
    }
}

#[async_trait::async_trait]
impl AskBroker for Operator {
    async fn wait(&self) -> Result<String, ToolError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let mut unanswered = Unanswered(Some(Arc::clone(&self.abandoned)));
        let text = self.answers.lock().await.recv().await;
        unanswered.0 = None;
        text.ok_or_else(|| ToolError::message("the operator left"))
    }
}

/// Counts a question torn down before its answer arrived.
struct Unanswered(Option<Arc<AtomicUsize>>);

impl Drop for Unanswered {
    fn drop(&mut self) {
        if let Some(abandoned) = &self.0 {
            abandoned.fetch_add(1, Ordering::SeqCst);
        }
    }
}
