//! Tests for the headless gateway's refusal of an active STT model.

use super::*;

#[test]
fn a_headless_gateway_refuses_an_active_stt_model() {
    let catalog = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
         [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\n\
         source = \"missing.bin\"\nvram_gb = 1.0\n\
         [[profile]]\nname = \"work\"\nmodels = [\"speech\"]\n",
    )
    .expect("catalog parses");
    let config = catalog
        .select_profile(Some(&ProfileName::parse("work").expect("profile name")))
        .expect("profile selects");
    let error = Gateway::from_config(&config, ProfilesContext::default())
        .expect_err("STT without the runtime feature must be refused");
    let detail = std::error::Error::source(&error)
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        detail.contains("`stt` feature"),
        "the refusal names the missing feature: {error}: {detail}"
    );
}
