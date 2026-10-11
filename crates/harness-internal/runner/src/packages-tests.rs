//! Tests for the natives every registry holds: each builds a Plugin that
//! offers no tools, and carries no prelude and no needs.

use promptforge_plugin::HostServices;
use serde_json::Value;

use super::{NATIVE, package_name};

#[test]
fn natives_offer_no_tools_prelude_or_needs() {
    for package in NATIVE {
        assert!(package.prelude.is_none(), "{} has a prelude", package.name);
        assert!(package.needs.is_empty(), "{} has needs", package.name);
        let name = package_name(package.name).unwrap();
        let plugin = (package.construct)(&name, Value::Null, &HostServices::new()).unwrap();
        assert!(plugin.tools().is_empty(), "{} offers tools", package.name);
    }
}
