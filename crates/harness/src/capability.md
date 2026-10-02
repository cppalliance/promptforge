Install the capabilities your agents declare, and provide the services those capabilities read.

You need this when an agent lists a capability under `capabilities:` in its frontmatter, such as the built-in `chat`, which declares `promptforge/user-input` and `promptforge/web`.

# Where this fits

[The crate overview](crate) builds `desk`, a Host that runs the built-in `chat` agent. A *capability* is a named set of tools and Lua that the Harness adds to a run when the agent declares it, such as `promptforge/user-input`, which gives the agent `input.ask()`. Your program registers each one it offers in a [`CapabilityRegistry`] and hands the registry to [`Harness::new`](crate::Harness::new). Every run of every session resolves its declarations against that registry. The Harness adds one capability of its own: when your registry holds no `promptforge/web`, each gateway you push builds the Harness's web capability, the fetch and search tools `chat` declares.

Some capabilities need something only your program has, such as a client, a setting, or a runtime handle. Each such thing is a *service*: an object your program puts in a [`HostServices`] map under a named id, beside the registry. A capability names the services it needs, and reads them when a run activates it.

# Install capabilities and their services

`desk` wants `chat` to ask the operator, and wants its own agents to know the word limit `desk` sets for every reply. The first is the shipped [`UserInput`] capability. The second is a capability of `desk`'s own that reads the limit as a service.

Registering capabilities feels like building a router: you add each handler under its name once, and requests find them by name. Unlike a router, the registry is fixed when the Harness is built, and a run whose agent requires a name you never registered is refused as it prepares.

````
use harness::capability::{
    Capability, CapabilityError, CapabilityId, CapabilityRegistry, Contribution, HostServices,
    RunServices, ServiceId, ServiceKey, UserInput,
};
use harness::record::MemoryRecorder;
use harness::{Harness, HarnessConfig};
use std::error::Error;
use std::sync::Arc;

// 1. desk's word limit is a service: an id bound to the type desk provides.
const WORD_LIMIT: ServiceKey<u32> = ServiceKey::new("com.example.desk/word-limit");

// 2. A capability that needs the limit, and hands it to the agent's Lua as `desk.word_limit`.
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

// 3. Register every capability desk's agents may declare.
let mut capabilities = CapabilityRegistry::new();
capabilities.register(Arc::new(UserInput::new()))?;
capabilities.register(Arc::new(Limits { id: CapabilityId::parse("com.example.desk/limits")? }))?;

// 4. Provide the services those capabilities read.
let mut services = HostServices::new();
services.provide(&WORD_LIMIT, Arc::new(200))?;
assert!(services.provides(&WORD_LIMIT.id()));

// 5. Hand both to the Harness, beside the config and the recorder.
let config = HarnessConfig { agents_path: "desk/agents".into() };
let harness = Harness::new(config, Arc::new(MemoryRecorder::new()), capabilities, services);
assert_eq!(harness.discover(), ["chat"]);
# Ok::<(), Box<dyn Error>>(())
````

1. Step 1 declares the service's key as a `const`. A [`ServiceKey`] binds an id, `namespace/name` like a capability id, to the Rust type its provider has, here `u32`. Your program provides the service under the key, and the capability reads it under the same key.
2. Step 2 implements [`Capability`] for `Limits`. [`Capability::needs`] lists the [`ServiceId`] of every service it reads. [`Capability::create`] runs once for each run that declares `com.example.desk/limits`, reads the limit from the [`RunServices`] it is given, and returns a [`Contribution`]. Its `prelude` is Lua every section of the run installs, so the agent reads `desk.word_limit`. A capability can also contribute [`Tool`]s, which the agent binds under `tools:`.
3. Step 3 registers [`UserInput`], the `promptforge/user-input` capability `chat` declares, and `Limits` under `com.example.desk/limits`. [`CapabilityRegistry::register`] refuses a second capability with the same id, or one whose id differs from a registered one only by `-`, `_`, or `.`, with a [`RegistryError`].
4. Step 4 provides the limit with [`HostServices::provide`], which refuses an id that is not `namespace/name`, or one already provided, with a [`ServiceError`]. [`HostServices::provides`] confirms the service is there under the key's type.
5. Step 5 builds the Harness with both. It keeps them for as long as it lives, and every run of every session resolves against them.

The Harness supplies one service itself: each session hands its runs the operator's input handler, which `UserInput` reads. You register `UserInput`, and the session does the rest, so an agent that calls `input.ask()` reaches your operator through [`Session::subscribe_waits`](crate::Session::subscribe_waits).

What reaches a run depends on how the agent declares the capability:

- A required capability that is not registered refuses the run with `RequirementsUnmet`, and the notice says `missing required capability:` and its id.
- A registered required capability that needs a service you did not provide refuses the run too, and the notice names the capability and the service id. Its `create` never runs.
- An optional capability that is not registered is skipped, and the run goes on without it.
- A registered optional capability whose service is missing still activates, and decides for itself how to work without it.

Each refusal fails the run as it prepares, not the launch: [`Harness::launch`](crate::Harness::launch) returns the session, and [`Session::subscribe_errors`](crate::Session::subscribe_errors) reports [`FailureKind::RunFailed`](crate::FailureKind::RunFailed) as the session closes.

You might expect the Harness to bring the capabilities it ships, the way a framework turns on its defaults. Instead, it adds only its web, and only when your registry holds no `promptforge/web`: even `promptforge/user-input` reaches an agent only when your program registers [`UserInput`]. Your program decides what every agent may do, and a `promptforge/web` you register replaces the Harness's.

Register what your agents declare, provide what those capabilities need, and hand both to `Harness::new`. [Where to go next](crate#where-to-go-next) lists the other pages.

# Reference

## Capability

[`Capability`] is the trait a capability implements. Register one value per capability in a [`CapabilityRegistry`]. [Install capabilities and their services](#install-capabilities-and-their-services) implements one.

- [`Capability::id`]: the `namespace/pack` id an agent declares; it must not change between calls.
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

[`CapabilityRegistry`] holds the capabilities your program offers, by id. Pass it to [`Harness::new`](crate::Harness::new); an empty one is a Harness whose agents may declare no required capability except `promptforge/web`, which each gateway you push builds while your registry holds none.

- [`CapabilityRegistry::register`]: refuses a repeated id, or a punctuation twin of a registered one, with a [`RegistryError`].
- [`CapabilityRegistry::get`]: the capability under an id.

## Contribution

[`Contribution`] is what [`Capability::create`] adds to one run: `tools`, each under the capability's own id, and an optional Lua `prelude` every section installs. Build it with `..Contribution::default()`, so a field added later does not break your code.

## HostServices

[`HostServices`] maps service ids to the objects your program provides. Pass it to [`Harness::new`](crate::Harness::new) beside the registry. Cloning it shares the providers.

- [`HostServices::provide`]: refuses an id that is not `namespace/name`, or one already provided, with a [`ServiceError`].
- [`HostServices::get`]: the provider under a key, or `None` when it is missing or was provided as another type.
- [`HostServices::provides`]: whether a provider of the id's type is there.

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

[`UserInput`] is the `promptforge/user-input` capability, which gives an agent `input.ask()` and the ask tool. Register it so agents such as `chat` can ask the operator. Each session supplies the input handler it needs.
