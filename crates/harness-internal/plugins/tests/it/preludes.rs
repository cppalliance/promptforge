//! Preludes at activation: each activated Plugin's prelude is
//! returned in declaration order, and a Plugin that does not
//! activate (absent, or failing in `create`) contributes none.

use std::sync::Arc;

use harness_plugins::{
    Activation, Contribution, Plugin, PluginError, PluginId, PluginRegistry, RunServices, activate,
};
use promptforge::UnavailablePlugin;
use promptforge::cancel::CancelHandle;
use promptforge::plugins::Prelude;

use super::support::parse;

fn id(text: &str) -> PluginId {
    PluginId::parse(text).expect("the fixture id is valid")
}

/// A fixture Plugin contributing `prelude` when it has one and no
/// tools. `fail` turns its activation into an error.
struct Preluder {
    id: PluginId,
    prelude: Option<&'static str>,
    fail: bool,
}

impl Preluder {
    fn new(plugin: &str, prelude: Option<&'static str>) -> Preluder {
        Preluder {
            id: id(plugin),
            prelude,
            fail: false,
        }
    }

    fn failing(mut self) -> Preluder {
        self.fail = true;
        self
    }
}

impl Plugin for Preluder {
    fn id(&self) -> &PluginId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Plugin trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "A fixture Plugin contributing a prelude."
    }

    fn create(&self, _services: &RunServices) -> Result<Contribution, PluginError> {
        if self.fail {
            return Err(PluginError::message("the fixture cannot activate"));
        }
        Ok(Contribution {
            tools: Vec::new(),
            prelude: self.prelude.map(str::to_owned),
        })
    }
}

/// Activates a prompt whose frontmatter declares `plugins` (the
/// YAML list entries, one per line) against a registry holding
/// `installed`, on a Host with no input broker.
fn activate_declaring(plugins: &str, installed: Vec<Preluder>) -> Activation {
    let source = format!(
        "---\nname: preludes\ndescription: d\npromptforge: 0\nplugins:\n{plugins}\
         ---\n\n# Title\n\n## Only\n\nDone.\n"
    );
    let prompt = parse(&source, "preludes");
    let mut registry = PluginRegistry::new();
    for plugin in installed {
        registry
            .register(Arc::new(plugin))
            .expect("the fixture registers");
    }
    let services = RunServices::new(CancelHandle::new());
    activate(Some(&registry), &prompt, &services)
}

#[test]
fn activation_returns_each_contributed_prelude_in_declaration_order() {
    // The registry keeps its Plugins sorted by id, so declaring them
    // in reverse order tells declaration order apart from registry order.
    let activation = activate_declaring(
        "  - omega\n  - quiet\n  - alpha\n",
        vec![
            Preluder::new("alpha", Some("alpha = {}")),
            Preluder::new("quiet", None),
            Preluder::new("omega", Some("omega = {}")),
        ],
    );
    assert!(activation.requirements.is_satisfied());
    assert_eq!(
        activation.preludes,
        [
            Prelude::new(id("omega"), "omega = {}"),
            Prelude::new(id("alpha"), "alpha = {}"),
        ],
        "one prelude per contributing Plugin, in declaration order"
    );
}

#[test]
fn an_absent_plugin_contributes_no_prelude() {
    let activation = activate_declaring(
        "  - alpha\n  - absent\n",
        vec![Preluder::new("alpha", Some("alpha = {}"))],
    );
    assert_eq!(activation.requirements.missing_required, [id("absent")]);
    assert_eq!(
        activation.preludes,
        [Prelude::new(id("alpha"), "alpha = {}")]
    );
}

#[test]
fn a_plugin_whose_create_fails_contributes_no_prelude() {
    let activation = activate_declaring(
        "  - broken\n  - alpha\n",
        vec![
            Preluder::new("broken", Some("broken = {}")).failing(),
            Preluder::new("alpha", Some("alpha = {}")),
        ],
    );
    assert_eq!(
        activation.requirements.unavailable,
        [UnavailablePlugin::new(
            id("broken"),
            "the fixture cannot activate"
        )]
    );
    assert_eq!(
        activation.preludes,
        [Prelude::new(id("alpha"), "alpha = {}")]
    );
}
