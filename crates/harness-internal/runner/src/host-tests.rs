//! Tests for the Host's install and each run's snapshot: the default name,
//! the refused packages and names, a construct failure stored as
//! unavailable, the requirements a run reports, the tools its catalog
//! keeps, the preludes it installs, and where each call goes.

use std::sync::Arc;

use promptforge::effect::{ToolCallOrigin, ToolCaller};
use promptforge::vfs::{Origin, VfsRef};
use promptforge::{MissingService, Prompt, UnavailablePlugin};
use promptforge_plugin::{
    HostServices, Package, Plugin, PluginFuture, PluginId, ServiceId, ServiceKey, ToolContext,
    ToolDescriptor, ToolError, ToolId, ToolOutput,
};
use serde_json::{Value, json};

use super::{HostContext, InstallError};
use crate::performers::ToolPerformer;

/// A Host-wide service a fixture's `construct` reads.
const BACKEND: ServiceKey<str> = ServiceKey::new("tests/backend");

/// A per-run service a fixture's calls read.
const SESSION: ServiceKey<str> = ServiceKey::new("tests/session");

const NEEDS_SESSION: &[ServiceId] = &[SESSION.id()];

/// Offers the tools its configuration lists and answers each call with
/// the name it was installed under and the called tool's id.
struct Listed {
    name: PluginId,
    tools: Vec<ToolDescriptor>,
}

impl Plugin for Listed {
    fn tools(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    fn call<'a>(
        &'a self,
        cx: ToolContext<'a>,
        _args: Value,
    ) -> PluginFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move { Ok(ToolOutput::trusted(format!("{}:{}", self.name, cx.tool()))) })
    }
}

/// Builds a [`Listed`] from a configuration that is a list of
/// `{ id, wire, survives_stop }` tool entries, taking each id as given.
#[expect(
    clippy::needless_pass_by_value,
    clippy::unnecessary_wraps,
    reason = "the Package construct signature fixes the argument and return types"
)]
fn listed(
    name: &PluginId,
    config: Value,
    _services: &HostServices,
) -> Result<Arc<dyn Plugin>, ToolError> {
    let mut tools = Vec::new();
    for entry in config.as_array().into_iter().flatten() {
        let id = ToolId::parse(entry["id"].as_str().unwrap()).unwrap();
        let wire = entry["wire"].as_str().unwrap();
        let schema = json!({ "type": "object", "properties": {} });
        let descriptor = ToolDescriptor::new(id, wire, "A listed tool.", schema)
            .survives_stop(entry["survives_stop"].as_bool().unwrap_or(false));
        tools.push(descriptor);
    }
    Ok(Arc::new(Listed {
        name: name.clone(),
        tools,
    }))
}

/// Builds a [`Listed`] with one `<name>/use` tool, failing when the Host
/// provides no [`BACKEND`].
fn backed(
    name: &PluginId,
    _config: Value,
    services: &HostServices,
) -> Result<Arc<dyn Plugin>, ToolError> {
    if services.get(&BACKEND).is_none() {
        return Err(ToolError::message(
            "backed needs tests/backend, and this host provides none",
        ));
    }
    listed(name, json!([tool(&format!("{name}/use"), "use")]), services)
}

/// A package named `name` whose Plugin offers the configured tools.
const fn package(name: &'static str) -> Package {
    Package {
        name,
        prelude: None,
        needs: &[],
        construct: listed,
    }
}

/// One tool entry of a [`listed`] configuration.
fn tool(id: &str, wire: &str) -> Value {
    json!({ "id": id, "wire": wire })
}

fn name(text: &str) -> PluginId {
    PluginId::parse(text).unwrap()
}

fn host() -> HostContext {
    HostContext::new(HostServices::new())
}

/// A prompt with the given `plugins:` and `tools:` frontmatter lines.
fn prompt(frontmatter: &str) -> Prompt {
    let source = format!(
        "---\nname: host-test\ndescription: d\npromptforge: 0\n{frontmatter}---\n\n\
         # Title\n\n## Only\n\nDone.\n"
    );
    let (prompt, _events) = Prompt::parse(&source, "host-test");
    prompt.unwrap()
}

/// The ids in a run's catalog, in catalog order.
fn catalog_ids(host: &HostContext, services: HostServices, prompt: &Prompt) -> Vec<String> {
    let (run, _env, _requirements) = host.begin_run(services, prompt);
    run.catalog()
        .tools()
        .iter()
        .map(|tool| tool.id.to_string())
        .collect()
}

#[test]
fn install_names_the_plugin_after_its_package_names_second_segment_unless_the_host_picks_one() {
    let mut host = host();
    assert_eq!(
        host.install(package("acme/web"), None, Value::Null),
        Ok(name("web"))
    );
    assert_eq!(
        host.install(package("acme/web"), Some(name("browse")), Value::Null),
        Ok(name("browse"))
    );
}

#[test]
fn install_refuses_a_package_name_that_is_not_a_vendor_name_pair() {
    for bad in [
        "web",
        "acme/web/extra",
        "/web",
        "acme/",
        "Acme/web",
        "acme/Web",
    ] {
        assert_eq!(
            host().install(package(bad), None, Value::Null),
            Err(InstallError::InvalidPackage { package: bad }),
            "{bad}"
        );
    }
}

#[test]
fn install_refuses_a_name_already_installed_and_its_punctuation_twin() {
    let mut host = host();
    host.install(package("acme/user-input"), None, Value::Null)
        .unwrap();
    let taken = |name_: &str| InstallError::NameTaken {
        name: name(name_),
        existing: name("user-input"),
    };
    assert_eq!(
        host.install(package("acme/user-input"), None, Value::Null),
        Err(taken("user-input"))
    );
    assert_eq!(
        host.install(package("other/user_input"), None, Value::Null),
        Err(taken("user_input"))
    );
    assert_eq!(
        host.install(package("other/ask"), Some(name("user.input")), Value::Null),
        Err(taken("user.input"))
    );
    assert_eq!(
        taken("user_input").to_string(),
        "Plugin name user_input is taken by the installed user-input; install it under another name"
    );
}

#[test]
fn a_construct_failure_is_stored_and_reported_as_unavailable_with_its_reason() {
    let backed_package = Package {
        construct: backed,
        ..package("acme/backed")
    };
    let mut bare = host();
    assert_eq!(
        bare.install(backed_package, None, Value::Null),
        Ok(name("backed")),
        "a construct failure is not an install error"
    );
    let (_run, _env, requirements) =
        bare.begin_run(HostServices::new(), &prompt("plugins:\n  - backed\n"));
    assert_eq!(
        requirements.unavailable,
        [UnavailablePlugin::new(
            name("backed"),
            "backed needs tests/backend, and this host provides none"
        )]
    );
    assert!(requirements.missing_required.is_empty());

    let mut wide = HostServices::new();
    wide.provide(&BACKEND, Arc::from("up")).unwrap();
    let mut provided = HostContext::new(wide);
    provided.install(backed_package, None, Value::Null).unwrap();
    let (_run, _env, requirements) =
        provided.begin_run(HostServices::new(), &prompt("plugins:\n  - backed\n"));
    assert!(
        requirements.is_satisfied(),
        "construct reads the Host-wide services: {requirements:?}"
    );
}

#[test]
fn begin_run_reports_every_declared_or_slotted_plugin_it_cannot_serve() {
    let mut host = host();
    let needy = Package {
        needs: NEEDS_SESSION,
        ..package("acme/needy")
    };
    let broken = Package {
        construct: backed,
        ..package("acme/broken")
    };
    host.install(needy, None, Value::Null).unwrap();
    host.install(broken, None, Value::Null).unwrap();
    host.install(package("acme/fine"), None, Value::Null)
        .unwrap();

    let (_run, _env, requirements) = host.begin_run(
        HostServices::new(),
        &prompt("plugins:\n  - needy\n  - absent\n  - fine\ntools:\n  b: broken/use\n"),
    );
    assert_eq!(requirements.missing_required, [name("absent")]);
    assert_eq!(
        requirements.missing_services,
        [MissingService::new(name("needy"), "tests/session")]
    );
    assert_eq!(
        requirements.unavailable.len(),
        1,
        "the slotted Plugin that failed to build is reported: {requirements:?}"
    );
    assert_eq!(requirements.unavailable[0].plugin, name("broken"));
}

#[test]
fn begin_run_drops_a_tool_outside_its_plugin_a_repeated_id_and_an_illegal_wire_name() {
    let mut host = host();
    let config = json!([
        tool("kit/good", "good"),
        tool("other/escape", "escape"),
        tool("kit/good", "again"),
        tool("kit/bad", "kit/bad"),
    ]);
    host.install(package("acme/kit"), None, config).unwrap();
    assert_eq!(
        catalog_ids(&host, HostServices::new(), &prompt("")),
        ["kit/good"]
    );
}

#[test]
fn a_plugin_whose_needs_the_run_lacks_offers_it_no_tools() {
    let mut host = host();
    let needy = Package {
        needs: NEEDS_SESSION,
        ..package("acme/needy")
    };
    host.install(needy, None, json!([tool("needy/use", "use")]))
        .unwrap();
    assert!(catalog_ids(&host, HostServices::new(), &prompt("")).is_empty());

    let mut run = HostServices::new();
    run.provide(&SESSION, Arc::from("ada")).unwrap();
    assert_eq!(catalog_ids(&host, run, &prompt("")), ["needy/use"]);
}

#[test]
fn an_undeclared_plugins_tools_reach_the_catalog_but_its_prelude_does_not() {
    let mut host = host();
    let declared = Package {
        prelude: Some("declared = {}\n"),
        ..package("acme/declared")
    };
    let extra = Package {
        prelude: Some("extra = {}\n"),
        ..package("acme/extra")
    };
    host.install(declared, None, json!([tool("declared/a", "a")]))
        .unwrap();
    host.install(extra, None, json!([tool("extra/b", "b")]))
        .unwrap();
    let prompt = prompt("plugins:\n  - declared\n");
    assert_eq!(
        catalog_ids(&host, HostServices::new(), &prompt),
        ["declared/a", "extra/b"]
    );
    let (run, _env, _requirements) = host.begin_run(HostServices::new(), &prompt);
    let preludes: Vec<String> = run
        .preludes(&prompt)
        .iter()
        .map(|prelude| prelude.plugin().to_string())
        .collect();
    assert_eq!(preludes, ["declared"]);
}

#[test]
fn preludes_follow_the_prompts_declaration_order_and_carry_the_installed_name() {
    let mut host = host();
    for package_name in ["acme/first", "acme/second"] {
        let with_prelude = Package {
            prelude: Some("local plugin = ...\n"),
            ..package(package_name)
        };
        host.install(with_prelude, None, Value::Null).unwrap();
    }
    let renamed = Package {
        prelude: Some("local plugin = ...\n"),
        ..package("acme/first")
    };
    host.install(renamed, Some(name("third")), Value::Null)
        .unwrap();
    let prompt = prompt("plugins:\n  - third\n  - second\n  - first\n");
    let (run, _env, _requirements) = host.begin_run(HostServices::new(), &prompt);
    let order: Vec<String> = run
        .preludes(&prompt)
        .iter()
        .map(|prelude| prelude.plugin().to_string())
        .collect();
    assert_eq!(order, ["third", "second", "first"]);
}

/// Calls `tool` through `run` from a script and returns its result.
async fn call(run: &dyn ToolPerformer, tool: &str) -> Result<ToolOutput, ToolError> {
    let access = VfsRef::default().acquire(Origin::new("host test")).unwrap();
    let origin = ToolCallOrigin {
        execution: "host-test".to_owned(),
        section: "Only".to_owned(),
        caller: ToolCaller::Script,
    };
    let tool = ToolId::parse(tool).unwrap();
    run.call(
        tool,
        "alias".to_owned(),
        Arc::new(access),
        origin,
        json!({}),
    )
    .await
}

#[tokio::test]
async fn a_call_goes_to_the_plugin_its_tool_ids_first_segment_names() {
    let mut host = host();
    host.install(
        package("acme/one"),
        None,
        json!([tool("one/echo", "one_echo")]),
    )
    .unwrap();
    host.install(
        package("acme/two"),
        None,
        json!([tool("two/echo", "two_echo")]),
    )
    .unwrap();
    let (run, _env, _requirements) = host.begin_run(HostServices::new(), &prompt(""));

    let answer = |result: Result<ToolOutput, ToolError>| result.unwrap().text().to_owned();
    assert_eq!(answer(call(&run, "one/echo").await), "one:one/echo");
    assert_eq!(answer(call(&run, "two/echo").await), "two:two/echo");
    let error = call(&run, "three/echo")
        .await
        .expect_err("no Plugin is installed under three");
    assert!(error.to_string().contains("three/echo"), "{error}");
}

#[tokio::test]
async fn a_call_to_a_plugin_the_run_cannot_use_fails_naming_the_tool() {
    let mut host = host();
    let needy = Package {
        needs: NEEDS_SESSION,
        ..package("acme/needy")
    };
    host.install(needy, None, json!([tool("needy/use", "use")]))
        .unwrap();
    let (run, _env, _requirements) = host.begin_run(HostServices::new(), &prompt(""));
    let error = call(&run, "needy/use")
        .await
        .expect_err("the run lacks the Plugin's needs");
    assert!(error.to_string().contains("needy/use"), "{error}");
}

#[test]
fn survives_stop_answers_from_the_runs_snapshot_of_descriptors() {
    let mut host = host();
    let config = json!([
        { "id": "kit/ask", "wire": "ask", "survives_stop": true },
        tool("kit/fetch", "fetch"),
    ]);
    host.install(package("acme/kit"), None, config).unwrap();
    let (run, _env, _requirements) = host.begin_run(HostServices::new(), &prompt(""));
    let survives = |id: &str| run.survives_stop(&ToolId::parse(id).unwrap());
    assert!(survives("kit/ask"));
    assert!(!survives("kit/fetch"));
    assert!(
        !survives("other/ask"),
        "a tool the run lacks does not survive"
    );
}
