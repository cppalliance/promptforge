//! Offline prompt-fixture suite: parses fixtures written the way a prompt
//! author writes them and drives their execution on the serial test driver
//! without a live gateway. Split by domain - parsing contracts, section
//! execution, fanout, control flow, the args/argv and lazy-prose surfaces,
//! an author's own `_G` metatable, `require`, and the prepare pass - over
//! the run-level [`Scenario`](crate::test_support::scenario::Scenario). Its
//! cases reach engine-only items (the test drivers and recorders, the store
//! facade, the parser's Lua programs), so they run here rather than against
//! the `promptforge` facade, whose own suite holds the rest.

use serde_json::json;

use crate::test_support::FnTool;
use crate::tools::{ToolError, ToolOutput};

mod args_surface;
mod exec_flow;
mod execution;
mod fanout;
mod global_metatable;
mod lazy_prose;
mod parsing;
mod prepare;
mod require;
mod vfs;

/// The fixture Plugin's `tools/echo`, which echoes its `value` argument.
fn echo_tool() -> FnTool {
    let schema = json!({
        "type": "object",
        "properties": { "value": { "type": "string" } },
        "required": ["value"]
    });
    FnTool::new("tools/echo", schema, |args| {
        let value = args["value"]
            .as_str()
            .ok_or_else(|| ToolError::message("echo requires a string `value` argument"))?;
        Ok(ToolOutput::trusted(format!("echoed: {value}")))
    })
    .description("Echo the value argument back to the caller.")
}
