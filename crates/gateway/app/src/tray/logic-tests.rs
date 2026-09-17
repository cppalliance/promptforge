//! The tray status readout: routed-model count and VRAM totals.
use gateway_config::Config;

use crate::test_support::app_state;

/// Two remote models, one local model at 3.5 GB, one STT model at 1 GB,
/// and a profile selecting both local entries.
const CONFIG: &str = "config-version = 0\n\
     [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
     [[endpoint]]\nid = \"fake\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
     [[model]]\nname = \"alpha\"\ndescription = \"a\"\ncontext = 1024\nupstream = \"a\"\nendpoints = [\"fake\"]\n\
     [[model]]\nname = \"beta\"\ndescription = \"b\"\ncontext = 1024\nupstream = \"b\"\nendpoints = [\"fake\"]\n\
     [[local_model]]\nname = \"gamma\"\ndescription = \"g\"\nsource = \"/models/gamma.gguf\"\ncontext = 4096\nvram_gb = 3.5\n\
     [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = \"/speech.bin\"\nvram_gb = 1.0\n\
     [[profile]]\nname = \"work\"\nmodels = [\"gamma\", \"speech\"]\n";

fn selected_config() -> Config {
    Config::from_toml_str(CONFIG)
        .expect("config parses")
        .select_profile(Some(
            &gateway_config::ProfileName::parse("work").expect("profile name"),
        ))
        .expect("the work profile selects")
}

/// With no local child running, only the boot STT selection counts.
#[test]
#[expect(clippy::float_cmp, reason = "1.0 is exact in binary floating point")]
fn the_tray_status_counts_routed_models_and_the_boot_stt_selection() {
    let state = app_state(selected_config(), None);
    let (models, vram_gb) = state
        .tray_model_status()
        .expect("an uncontended state reads");
    assert_eq!(models, 2, "the harness routes the remote catalog");
    assert_eq!(
        vram_gb, 1.0,
        "no local child runs; the STT declaration counts"
    );
}

/// A running local child's declared VRAM counts, and keeps counting
/// after an apply swaps the live config for one with no selection.
#[cfg(feature = "test-fixtures")]
#[tokio::test]
#[expect(
    clippy::float_cmp,
    reason = "3.5 + 1.0 is exact in binary floating point"
)]
async fn the_status_sums_running_children_after_an_apply_swaps_the_config() {
    let state = app_state(selected_config(), None);
    let child = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [[endpoint]]\nid = \"local\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
         [[model]]\nname = \"gamma\"\ndescription = \"running child\"\ncontext = 4096\nupstream = \"gamma\"\nendpoints = [\"local\"]\n",
    )
    .expect("child config parses");
    let routing = crate::routing::Routing::from_config(&child).expect("child routes");
    state.live.write().await.local =
        crate::local::LocalRuntime::from_test_models(routing.models().to_vec());
    assert_eq!(state.live.read().await.model_status().1, 4.5);

    // What an apply publishes: the same document, parsed with no
    // profile selected, so `local_models()` and `stt_models()` are empty.
    let applied = Config::from_toml_str(CONFIG)
        .expect("config parses")
        .select_profile(None)
        .expect("no selection");
    assert!(applied.local_models().is_empty() && applied.stt_models().is_empty());
    state.live.write().await.config = std::sync::Arc::new(applied);

    let (models, vram_gb) = state.live.read().await.model_status();
    assert_eq!(models, 2);
    assert_eq!(
        vram_gb, 4.5,
        "the running child and the boot STT selection still count"
    );
}
