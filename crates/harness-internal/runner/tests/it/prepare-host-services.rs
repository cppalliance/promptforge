//! The Host's services at preparation: a declared Plugin that needs a
//! service activates with the provider the Host's map holds, and a run
//! whose Host provides no such service is refused naming the service.

use super::*;

use std::sync::Mutex;

use harness_plugins::{ServiceId, ServiceKey};

/// The test-only service the fixture needs.
const GREETING: ServiceKey<str> = ServiceKey::new("tests/greeting");

/// A prompt declaring the greeter Plugin, with nothing to run.
const DECLARES_GREETER: &str = "---\nname: declares-greeter\ndescription: d\npromptforge: 0\n\
    plugins:\n  - greeter\n---\n\n# Title\n\n## Only\n\nDone.\n";

/// A fixture Plugin needing [`GREETING`], which records the greeting
/// each activation read.
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
            id: PluginId::parse("greeter").unwrap(),
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
            .contains("- greeter needs tests/greeting, and the environment provides none"),
        "the notice names the Plugin and the missing service: {error}"
    );
    assert!(greetings.is_empty(), "the Plugin never activated");
}
