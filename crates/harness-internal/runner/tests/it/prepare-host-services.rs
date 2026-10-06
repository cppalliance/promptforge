//! The Host's services at preparation: a declared Plugin that needs a
//! service activates with the provider the Host's map holds; a run whose
//! Host provides no such service is refused naming the service when the
//! Plugin is required, and prepares with the Plugin activated
//! without it, recording the gap, when optional.

use super::*;

use std::sync::Mutex;

use harness_plugins::{ServiceId, ServiceKey, activate};
use promptforge::Prompt;

/// The test-only service the fixture needs.
const GREETING: ServiceKey<str> = ServiceKey::new("tests/greeting");

/// A prompt declaring the greeter Plugin, with nothing to run.
const DECLARES_GREETER: &str = "---\nname: declares-greeter\ndescription: d\npromptforge: 0\n\
    plugins:\n  - tests/greeter\n---\n\n# Title\n\n## Only\n\nDone.\n";

/// A prompt declaring the greeter Plugin as optional, with nothing to
/// run.
const DECLARES_GREETER_OPTIONALLY: &str = "---\nname: declares-greeter\ndescription: d\n\
    promptforge: 0\nplugins:\n  - ref: tests/greeter\n    optional: true\n---\n\n\
    # Title\n\n## Only\n\nDone.\n";

/// A fixture Plugin needing [`GREETING`], which records the greeting
/// each activation read, or `None` when it activated without one.
struct Greeter {
    id: PluginId,
    greetings: Arc<Mutex<Vec<Option<String>>>>,
}

impl Plugin for Greeter {
    fn id(&self) -> &PluginId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Plugin trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "Records the greeting the Host provides."
    }

    fn needs(&self) -> &[ServiceId] {
        const NEEDS: &[ServiceId] = &[GREETING.id()];
        NEEDS
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, PluginError> {
        let greeting = services.get(&GREETING).map(|greeting| greeting.to_string());
        self.greetings.lock().unwrap().push(greeting);
        Ok(Contribution::default())
    }
}

/// A registry holding the greeter, which records into `greetings`.
fn greeter_registry(greetings: &Arc<Mutex<Vec<Option<String>>>>) -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    registry
        .register(Arc::new(Greeter {
            id: PluginId::parse("tests/greeter").unwrap(),
            greetings: Arc::clone(greetings),
        }))
        .unwrap();
    registry
}

/// Prepares `source` with `host` as the Host's services, and returns the
/// preparation beside the greetings its activations read.
async fn prepare_greeter(
    source: &str,
    host: HostServices,
) -> (Result<Prepared, PrepareError>, Vec<Option<String>>) {
    let greetings = Arc::new(Mutex::new(Vec::new()));
    let recorder = recorder();
    let mut services = services(&recorder, Some(Arc::new(greeter_registry(&greetings))));
    services.services = host;
    let prepared = prepare(source, "", services).await;
    let greetings = greetings.lock().unwrap().clone();
    (prepared, greetings)
}

#[tokio::test]
async fn a_plugin_activates_with_the_service_the_hosts_map_provides() {
    let mut host = HostServices::new();
    host.provide(&GREETING, Arc::from("hello")).unwrap();
    let (prepared, greetings) = prepare_greeter(DECLARES_GREETER, host).await;
    assert!(prepared.is_ok(), "the Host's service meets the need");
    assert_eq!(
        greetings,
        [Some("hello".to_owned())],
        "activation read the Host's provider"
    );
}

#[tokio::test]
async fn a_plugin_whose_service_the_host_lacks_is_refused_naming_the_service() {
    let (prepared, greetings) = prepare_greeter(DECLARES_GREETER, HostServices::new()).await;
    let Err(PrepareError::Refused { error, .. }) = prepared else {
        panic!("a required Plugin without its service refuses the run");
    };
    assert!(
        error
            .to_string()
            .contains("- tests/greeter needs tests/greeting, and the environment provides none"),
        "the notice names the Plugin and the missing service: {error}"
    );
    assert!(greetings.is_empty(), "the Plugin never activated");
}

#[tokio::test]
async fn an_optional_plugin_whose_service_the_host_lacks_activates_without_it() {
    let (prepared, greetings) =
        prepare_greeter(DECLARES_GREETER_OPTIONALLY, HostServices::new()).await;
    assert!(
        prepared.is_ok(),
        "an optional Plugin's gap does not refuse the run"
    );
    assert_eq!(
        greetings,
        [None::<String>],
        "the Plugin activated without the greeting"
    );
}

#[test]
fn an_optional_plugin_whose_service_the_host_lacks_records_the_service_gap() {
    let greetings = Arc::new(Mutex::new(Vec::new()));
    let registry = greeter_registry(&greetings);
    let prompt = Prompt::parse(DECLARES_GREETER_OPTIONALLY, "declares-greeter")
        .0
        .unwrap();
    let services = RunServices::with_host(CancelHandle::new(), HostServices::new());
    let activation = activate(Some(&registry), &prompt, &services);
    assert!(activation.requirements.is_satisfied());
    assert_eq!(activation.service_gaps.len(), 1, "one gap is recorded");
    let gap = &activation.service_gaps[0];
    assert_eq!(gap.plugin.to_string(), "tests/greeter");
    assert_eq!(gap.service, GREETING.id());
}
