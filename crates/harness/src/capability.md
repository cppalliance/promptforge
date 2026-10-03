Install the capabilities your prompts declare, and provide the services those capabilities read.

You need this when a prompt lists a capability under `capabilities:` in its frontmatter, such as Workshop's built-in `chat`, which declares `promptforge/user-input` and `promptforge/web`.

# Where this fits

[The crate overview](crate) builds `desk`, a Host that runs prompts for one person. A *capability* is a named set of tools and Lua that the Harness adds to a run when the prompt declares it, such as `promptforge/user-input`, which gives the prompt `input.ask()`. Your program registers each one it offers in a [`CapabilityRegistry`] and hands the registry to each run's [`Harness::new`](crate::Harness::new). The run resolves its declarations against that registry, and the Harness adds no capability of its own. `promptforge/web`, the fetch and search tools `chat` declares, is the `harness_web::Web` capability of the `harness-web` crate, which your program registers like any other.

Some capabilities need something only your program has, such as a client, a setting, a runtime handle, or a way to reach the operator. Each such thing is a *service*: an object your program puts in a [`HostServices`] map under a named id, beside the registry. A capability names the services it needs, and reads them when a run activates it.

# Install capabilities and their services

`desk` wants its prompts to ask the operator, and to know the word limit `desk` sets for every reply. The first is the shipped [`UserInput`] capability, which reads `desk`'s input broker as a service. The second is a capability of `desk`'s own that reads the limit as a service.

Registering capabilities feels like building a router: you add each handler under its name once, and requests find them by name. Unlike a router, the registry is fixed when the Harness is built, and a run whose prompt requires a name you never registered is refused as it prepares.

````
use async_trait::async_trait;
use harness::capability::{
    Capability, CapabilityError, CapabilityId, CapabilityRegistry, Contribution, HostServices,
    INPUT_BROKER, InputBroker, InputError, RunServices, ServiceId, ServiceKey, UserInput,
};
use harness::record::MemoryRecorder;
use harness::Harness;
use std::error::Error;
use std::sync::Arc;
# use harness::{BoxFuture, InferenceBroker, Timer};
# use promptforge::model::{Completion, CompletionError, CompletionErrorKind, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema};
# struct Offline;
# impl InferenceBroker for Offline {
#     fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
#         Box::pin(async { Ok(ModelCatalog::empty()) })
#     }
#     fn chat(&self, _: ModelBinding, _: Vec<Message>, _: Vec<ToolSchema>, _: CompletionOptions, _: promptforge::effect::Round) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
#         let kind = CompletionErrorKind::Unavailable;
#         Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
#     }
# }
# struct Clock;
# impl Timer for Clock {
#     fn sleep(&self, seconds: f64) -> BoxFuture<()> {
#         Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)))
#     }
# }

// 1. desk's word limit is a service: an id bound to the type desk provides.
const WORD_LIMIT: ServiceKey<u32> = ServiceKey::new("com.example.desk/word-limit");

// 2. A capability that needs the limit, and hands it to the prompt's Lua as `desk.word_limit`.
struct Limits {
    id: CapabilityId,
}

impl Capability for Limits {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    fn description(&self) -> &str {
        "The word limit desk sets for every reply."
    }

    fn needs(&self) -> &[ServiceId] {
        const NEEDS: &[ServiceId] = &[WORD_LIMIT.id()];
        NEEDS
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
        let limit = services
            .get(&WORD_LIMIT)
            .ok_or_else(|| CapabilityError::message("desk set no word limit"))?;
        let prelude = format!("desk = {{ word_limit = {limit} }}");
        Ok(Contribution { prelude: Some(prelude), ..Contribution::default() })
    }
}

// 3. desk's input broker, which `UserInput` reads: here the operator always types the same text.
struct Operator;

#[async_trait]
impl InputBroker for Operator {
    async fn wait(&self) -> Result<String, InputError> {
        Ok("Keep it short.".to_owned())
    }
}

// 4. Register every capability desk's prompts may declare.
let mut capabilities = CapabilityRegistry::new();
capabilities.register(Arc::new(UserInput::new()))?;
capabilities.register(Arc::new(Limits { id: CapabilityId::parse("com.example.desk/limits")? }))?;

// 5. Provide the services those capabilities read.
let mut services = HostServices::new();
services.provide(&WORD_LIMIT, Arc::new(200))?;
let operator: Arc<dyn InputBroker> = Arc::new(Operator);
services.provide(&INPUT_BROKER, operator)?;
assert!(services.provides(&WORD_LIMIT.id()) && services.provides(&INPUT_BROKER.id()));

// 6. Hand both to the run's Harness, beside the recorder, the broker, and the timer.
let _harness = Harness::new(Arc::new(MemoryRecorder::new()), Arc::new(Offline), Arc::new(Clock), capabilities, services);
# Ok::<(), Box<dyn Error>>(())
````

1. Step 1 declares the service's key as a `const`. A [`ServiceKey`] binds an id, `namespace/name` like a capability id, to the Rust type its provider has, here `u32`. Your program provides the service under the key, and the capability reads it under the same key.
2. Step 2 implements [`Capability`] for `Limits`. [`Capability::needs`] lists the [`ServiceId`] of every service it reads. [`Capability::create`] runs once for each run that declares `com.example.desk/limits`, reads the limit from the [`RunServices`] it is given, and returns a [`Contribution`]. Its `prelude` is Lua every section of the run installs, so the prompt reads `desk.word_limit`. A capability can also contribute [`Tool`]s, which the prompt binds under `tools:`.
3. Step 3 implements [`InputBroker`] for `Operator`: the service `UserInput` waits on for the operator's next message, as the main page's [Answer the operator](crate#answer-the-operator) tour teaches.
4. Step 4 registers [`UserInput`], the `promptforge/user-input` capability, and `Limits` under `com.example.desk/limits`. [`CapabilityRegistry::register`] refuses a second capability with the same id, or one whose id differs from a registered one only by `-`, `_`, or `.`, with a [`RegistryError`].
5. Step 5 provides the limit and the input broker with [`HostServices::provide`], which refuses an id that is not `namespace/name`, or one already provided, with a [`ServiceError`]. [`HostServices::provides`] confirms each service is there under its key's type.
6. Step 6 builds the run's Harness with both, after the recorder, the broker, here `Offline`, and the timer, here `Clock`, both defined in hidden lines.

Each run gets its own Harness, so your program hands each one a clone of its registry and services. Cloning shares the capabilities and the providers. To reach the operator who launched one run, clone your base services for that run and provide its own input broker under [`INPUT_BROKER`] on the clone; leave `INPUT_BROKER` out of the base, because a second `provide` for the same id is refused.

What reaches a run depends on how the prompt declares the capability:

- A required capability that is not registered refuses the run with `RequirementsUnmet`, and the notice says `missing required capability:` and its id.
- A registered required capability that needs a service you did not provide refuses the run too, and the notice names the capability and the service id. Its `create` never runs.
- An optional capability that is not registered is skipped, and the run goes on without it.
- A registered optional capability whose service is missing still activates, and decides for itself how to work without it.

Each refusal fails the run as it prepares: [`Harness::run`](crate::Harness::run) returns a report whose outcome is `Failed` with the kind `RequirementsUnmet`.

You might expect the Harness to bring the capabilities PromptForge ships, the way a framework turns on its defaults. Instead, it adds none: `promptforge/user-input` reaches a prompt only when your program registers [`UserInput`] and provides an input broker, and `promptforge/web` only when it registers `harness_web::Web` and provides the search provider and tokio runtime handle that crate's `SEARCH_PROVIDER` and `TOKIO_RUNTIME` keys name. Your program decides what every prompt may do.

Register what your prompts declare, provide what those capabilities need, and hand both to each run's `Harness::new`. [Where to go next](crate#where-to-go-next) lists the other pages.

# Reference

## Capability

[`Capability`] is the trait a capability implements. Register one value per capability in a [`CapabilityRegistry`]. [Install capabilities and their services](#install-capabilities-and-their-services) implements one.

- [`Capability::id`]: the `namespace/pack` id a prompt declares; it must not change between calls.
- [`Capability::needs`]: the services it reads; the default is none.
- [`Capability::conflicts`]: capabilities it cannot run beside; declaring both refuses the run naming both.
- [`Capability::create`]: runs once per run that declares it, and must not panic.

## CapabilityError

[`CapabilityError`] says why [`Capability::create`] failed. Its message is written for a model to read, and any cause sits behind [`Error::source`](std::error::Error::source). A required capability that fails refuses the run as missing; an optional one is left out.

## CapabilityErrorKind

[`CapabilityErrorKind`] classifies a [`CapabilityError`]: `Activation`, `Cancelled`, or `Other`. Match it with a wildcard arm, because it may gain variants.

## CapabilityId

[`CapabilityId`] is a capability's `namespace/pack` id, in lowercase ASCII letters, digits, `-`, `_`, and `.`. Build one with [`CapabilityId::parse`]. A namespace is reverse-DNS, such as `com.example.desk`, or `promptforge`, which is reserved for the capabilities PromptForge ships.

## CapabilityRegistry

[`CapabilityRegistry`] holds the capabilities your program offers, by id. Pass a clone to each run's [`Harness::new`](crate::Harness::new); an empty one is a Harness whose prompt may declare no required capability.

- [`CapabilityRegistry::register`]: refuses a repeated id, or a punctuation twin of a registered one, with a [`RegistryError`].
- [`CapabilityRegistry::get`]: the capability under an id.

## Contribution

[`Contribution`] is what [`Capability::create`] adds to one run: `tools`, each under the capability's own id, and an optional Lua `prelude` every section installs. Build it with `..Contribution::default()`, so a field added later does not break your code.

## HostServices

[`HostServices`] maps service ids to the objects your program provides. Pass it to [`Harness::new`](crate::Harness::new) beside the registry. Cloning it shares the providers.

- [`HostServices::provide`]: refuses an id that is not `namespace/name`, or one already provided, with a [`ServiceError`].
- [`HostServices::get`]: the provider under a key, or `None` when it is missing or was provided as another type.
- [`HostServices::provides`]: whether a provider of the id's type is there.

## INPUT_BROKER

[`INPUT_BROKER`] is the key a run's [`InputBroker`] is provided under, as an `Arc<dyn InputBroker>`. [`UserInput`] reads it. A run with no provider under it has nobody to ask.

## InputBroker

[`InputBroker`] is the trait your program implements to carry a question to the operator. Its one method, `wait`, returns the operator's next message byte for byte. The Harness polls it inside the run's future, so it must not block while polled, and the Harness drops its future when a cancel drops the question, so a broker that shows a prompt clears it then. It is an `async_trait` trait.

## InputError

[`InputError`] is an [`InputBroker`]'s failure to produce the operator's message. Build it with [`InputError::message`], or [`InputError::with_source`] to keep a cause behind [`Error::source`](std::error::Error::source). The prompt sees its message at its `input.ask()` call.

## RegistryError

[`RegistryError`] says why [`CapabilityRegistry::register`] refused a capability. The registry keeps its first registration.

## RegistryErrorKind

[`RegistryErrorKind`] classifies a [`RegistryError`]: `DuplicateId` or `NormalizationCollision`. Match it with a wildcard arm, because it may gain variants.

## RunServices

[`RunServices`] is what [`Capability::create`] receives for one run: the run's filesystem as `vfs`, its cancel flag as `cancel`, and the services the run has. Read a service with [`RunServices::get`]. Build one with [`RunServices::new`] or [`RunServices::with_host`] to test a capability without a Harness.

## ServiceError

[`ServiceError`] says why [`HostServices::provide`] refused a provider: `InvalidId` for an id that is not `namespace/name`, or `DuplicateId` for one already provided. The map is unchanged.

## ServiceId

[`ServiceId`] names a service in [`Capability::needs`]. Get one from [`ServiceKey::id`]. Two ids with the same literal name the same service, whatever their types.

## ServiceKey

[`ServiceKey`] binds a service id to the type its provider has. Declare each key once, as a `const`, in the crate that defines the service, and use it both to provide the service and to read it.

## Tool

[`Tool`] is the trait a contributed tool implements: its id, wire name, description, parameter schema, and an async `call`. A tool's id sits under its capability's id, as `namespace/pack/name`.

## UserInput

[`UserInput`] is the `promptforge/user-input` capability, which gives a prompt `input.ask()` and the ask tool. Register it, and provide an [`InputBroker`] under [`INPUT_BROKER`], so a prompt can ask the operator.
