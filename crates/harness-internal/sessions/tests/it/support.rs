//! The inference brokers the suites hand a Harness: a scripted broker that
//! serves a fixed model list, answers every round with one reply, and
//! keeps each round it was handed, an offline broker that lists no model
//! and refuses every round, and a holding broker whose model list never
//! answers.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use harness_runner::performers::{BoxFuture, InferenceBroker, OnDelta};
use promptforge::model::{
    Completion, CompletionError, CompletionErrorKind, CompletionOptions, CompletionResult, Message,
    ModelBinding, ModelCatalog, ModelDescriptor, ModelId, StreamDelta, ThinkingMode, ToolSchema,
};
use tokio::sync::mpsc;

/// One round as a scripted broker saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Round {
    /// The model name the round's options send on the wire.
    pub(crate) model: String,
    /// Each message's text, in order.
    pub(crate) messages: Vec<String>,
    /// Whether the round was handed a delta callback.
    pub(crate) streamed: bool,
}

/// The rounds a scripted broker kept, shared with the test reading them.
pub(crate) type Rounds = Arc<Mutex<Vec<Round>>>;

/// What a scripted broker's `models()` answers.
enum Listing {
    Served(ModelCatalog),
    Failing(CompletionErrorKind),
}

/// Lists a fixed catalog (or fails the listing with one kind's fixed
/// phrase) and answers every round with `reply`, sent first as one text
/// delta when the round has a callback.
pub(crate) struct ScriptedBroker {
    listing: Listing,
    reply: String,
    served_by: String,
    rounds: Rounds,
}

impl ScriptedBroker {
    /// Serves a catalog of `models`, each with a 131072-token window, and
    /// replies `reply` from `served_by`.
    pub(crate) fn new(models: &[&str], reply: &str, served_by: &str) -> Self {
        let catalog = ModelCatalog::new(models.iter().map(|id| descriptor(id)))
            .expect("the scripted ids are distinct");
        Self {
            listing: Listing::Served(catalog),
            reply: reply.to_owned(),
            served_by: served_by.to_owned(),
            rounds: Rounds::default(),
        }
    }

    /// Fails every model listing as `kind` with that kind's fixed phrase.
    pub(crate) fn failing_models(kind: CompletionErrorKind) -> Self {
        Self {
            listing: Listing::Failing(kind),
            ..Self::new(&[], "", "")
        }
    }

    /// The rounds this broker keeps, readable after it moved into a
    /// Harness.
    pub(crate) fn rounds(&self) -> Rounds {
        Arc::clone(&self.rounds)
    }
}

impl InferenceBroker for ScriptedBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        let listed = match &self.listing {
            Listing::Served(catalog) => Ok(catalog.clone()),
            Listing::Failing(kind) => Err(CompletionError::new(*kind, kind.phrase())),
        };
        Box::pin(async move { listed })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        options: CompletionOptions,
        _round: promptforge::effect::Round,
        on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        self.rounds.lock().unwrap().push(Round {
            model: options.model().to_owned(),
            messages: messages
                .iter()
                .map(|message| message.content().to_owned())
                .collect(),
            streamed: on_delta.is_some(),
        });
        let reply = self.reply.clone();
        let served_by = self.served_by.clone();
        Box::pin(async move {
            if let Some(on_delta) = on_delta {
                on_delta(StreamDelta::Text(reply.clone()));
            }
            Completion::from_result(CompletionResult::Text(reply), served_by)
                .map(|completion| Box::new(completion.with_finish_reason("stop")))
        })
    }
}

/// Lists no model and refuses every round as `Unavailable`: a Host with
/// no model access, for suites whose agents never make a model round.
pub(crate) struct OfflineBroker;

impl InferenceBroker for OfflineBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        Box::pin(async { Ok(ModelCatalog::empty()) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _round: promptforge::effect::Round,
        _on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let kind = CompletionErrorKind::Unavailable;
        Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
    }
}

/// Holds every model listing forever and announces each one as it is
/// asked for: a Host whose broker has no model to offer yet. A round is
/// never made.
pub(crate) struct HoldingBroker {
    listings: mpsc::UnboundedSender<()>,
}

impl HoldingBroker {
    /// The broker and the receiver that hears each listing it holds.
    pub(crate) fn new() -> (Self, mpsc::UnboundedReceiver<()>) {
        let (listings, heard) = mpsc::unbounded_channel();
        (Self { listings }, heard)
    }
}

impl InferenceBroker for HoldingBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        let _ = self.listings.send(());
        Box::pin(std::future::pending())
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _round: promptforge::effect::Round,
        _on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let kind = CompletionErrorKind::Unavailable;
        Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
    }
}

/// A gateway model named `id` with a 131072-token window.
pub(crate) fn descriptor(id: &str) -> ModelDescriptor {
    ModelDescriptor::new(
        ModelId::gateway(id).expect("a scripted model name is valid"),
        id,
        NonZeroU32::new(131_072).expect("131072 is non-zero"),
        ThinkingMode::Switchable,
    )
}
