//! Section execution and fall-through.
//!
//! The run walks the top-level sections in file order, each in a fresh context.
//! For each section: run its Lua block; if the chunk returns a plain value that
//! value is the run's result and the run ends immediately (this doubles as the
//! return fence - sections after it are not reached by fall-through). Otherwise
//! the section's prose is `{{ }}`-substituted (over `args`, the `var` the block
//! wrote, and the runtime `sys`) and, if non-empty, sent to the gateway for one
//! round trip; then control falls through to the next section.
//!
//! Running off the last section ends the run: the result is `default_return`
//! from the frontmatter, else the last model reply, else a generic completion.
//!
//! Still to come: the other exit cases (a descriptor = goto/task/fanout), the
//! tool-call loop, and durable state to carry a non-terminal section's work
//! forward (today an intermediate section's model reply is not retained).

use serde_json::json;

use crate::client::{CompletionResult, GatewayClient, Message, ToolSchema};
use crate::lua;
use crate::parser::Prompt;
use crate::subst;
use crate::tools::Tool;
use crate::{Error, Result};

/// The default maximum number of model round trips a single section's
/// tool-call loop will take before giving up, applied when a prompt's
/// frontmatter does not declare its own `max_tool_iterations`.
const DEFAULT_MAX_TOOL_ITERATIONS: usize = 24;

/// Execute a prompt and return the run's result.
///
/// `args` is the single raw input string, exposed to Lua and to `{{ args }}`.
///
/// `tools` is the run's full pool of available tools. Tool scoping is opt-in
/// per section: a section advertises only the tools its Lua block named with
/// `tools.add(...)`, and a section with no Lua block (or one that never calls
/// `tools.add`) advertises none. Only a section's scoped subset is shown to and
/// dispatchable by the model for that section. When the model asks to call one,
/// the executor dispatches it, appends the result to the conversation, and
/// re-sends, looping until the model returns a final text reply. The cap on
/// those round trips is the prompt's `max_tool_iterations` when set, otherwise
/// a raised runtime default. Pass an empty slice to disable tools entirely.
///
/// # Errors
/// Returns [`crate::Error::Lua`] if a Lua block fails,
/// [`crate::Error::Substitution`] if a `{{ }}` path cannot be resolved,
/// [`crate::Error::MissingEnv`] if the gateway client cannot be built when a
/// model call is needed, [`crate::Error::UnknownScopedTool`] if a section
/// scopes a tool name absent from `tools`, [`crate::Error::UnknownTool`] if the
/// model calls a tool that was not provided, [`crate::Error::ToolLoopExhausted`]
/// if a section's tool-call loop does not converge within its iteration cap, or
/// any transport/backend error from a model call.
pub async fn run(prompt: &Prompt, args: &str, tools: &[&dyn Tool]) -> Result<String> {
    let when = now_rfc3339();
    let mut client: Option<GatewayClient> = None;
    let mut last_reply: Option<String> = None;

    // Resolve the tool-loop cap once: the prompt's declared budget, or the
    // runtime default when it declares none.
    let max_tool_iterations = prompt
        .frontmatter
        .max_tool_iterations
        .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS);

    for (index, section) in prompt.sections.iter().enumerate() {
        let sys = json!({ "when": when, "now": now_rfc3339(), "id": index + 1 });

        // Run the section's Lua block. A returned value ends the whole run.
        // The block's `tools.add(...)` names (empty without a Lua block) scope
        // which tools this section may advertise and dispatch.
        let (var, scoped_names) = if let Some(source) = &section.lua {
            let outcome = lua::run_chunk(source, args, &sys)?;
            if let Some(value) = outcome.returned {
                return Ok(value);
            }
            (outcome.var, outcome.scoped_tools)
        } else {
            (json!({}), Vec::new())
        };

        // Resolve the scoped names against the run's tool pool. A name absent
        // from the pool is a hard error, never a silent drop. The model can
        // only be shown, and only dispatch, this filtered subset.
        let section_tools = scoped_tools(tools, &scoped_names)?;
        let schemas: Vec<ToolSchema> = section_tools
            .iter()
            .map(|t| ToolSchema {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters_schema(),
            })
            .collect();

        // Substitute the prose; if there is any, take one model round trip.
        let prose = subst::substitute(&section.prose, args, &var, &sys)?;
        if !prose.trim().is_empty() {
            if client.is_none() {
                client = Some(GatewayClient::from_env()?);
            }
            if let Some(client) = &client {
                let text =
                    run_tool_loop(client, &schemas, &section_tools, prose, max_tool_iterations)
                        .await?;
                last_reply = Some(text);
            }
        }
        // Fall through to the next section (context clears - nothing is carried).
    }

    // Ran off the end.
    Ok(prompt
        .frontmatter
        .default_return
        .clone()
        .or(last_reply)
        .unwrap_or_else(|| "done".to_string()))
}

/// Resolve a section's scoped tool names against the run's tool pool, in
/// first-named order, returning the matching subset the section may advertise
/// and dispatch.
///
/// # Errors
/// Returns [`Error::UnknownScopedTool`] if a scoped name has no matching tool
/// in `tools`, so a typo or an undeclared tool fails loudly rather than being
/// silently dropped.
fn scoped_tools<'a>(tools: &[&'a dyn Tool], names: &[String]) -> Result<Vec<&'a dyn Tool>> {
    let mut selected: Vec<&'a dyn Tool> = Vec::with_capacity(names.len());
    for name in names {
        let tool = tools
            .iter()
            .copied()
            .find(|t| t.name() == name)
            .ok_or_else(|| Error::UnknownScopedTool(name.clone()))?;
        selected.push(tool);
    }
    Ok(selected)
}

/// Drive one section's model call to a final text reply, dispatching any tool
/// calls the model requests along the way.
///
/// The conversation starts with the section's prose as a `user` turn. Each
/// round trip either yields text (returned immediately) or a batch of tool
/// calls; for the latter, the assistant turn is echoed back verbatim, each tool
/// is dispatched and its result appended as a `tool` turn, and the conversation
/// is re-sent. The loop is capped at `max_tool_iterations` round trips.
///
/// # Errors
/// Returns [`Error::UnknownTool`] if the model calls a tool not in `tools`,
/// [`Error::ToolLoopExhausted`] if the cap is hit without a text reply, or any
/// transport/backend error from a model call or a tool's own failure.
async fn run_tool_loop(
    client: &GatewayClient,
    schemas: &[ToolSchema],
    tools: &[&dyn Tool],
    prose: String,
    max_tool_iterations: usize,
) -> Result<String> {
    let mut conversation = vec![Message::user(prose)];
    let tool_arg = if schemas.is_empty() {
        None
    } else {
        Some(schemas)
    };

    for _ in 0..max_tool_iterations {
        match client.complete(&conversation, tool_arg).await? {
            CompletionResult::Text(text) => return Ok(text),
            CompletionResult::ToolCalls(calls) => {
                // Echo the assistant's tool-call turn back into the history. The
                // parsed `ToolCall`s are reconstructed into the raw OpenAI wire
                // shape (`function.arguments` re-encoded as a JSON string).
                let raw_calls = calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments.to_string(),
                            },
                        })
                    })
                    .collect();
                conversation.push(Message::assistant_tool_calls(raw_calls));

                // Dispatch each requested tool and append its result.
                for call in &calls {
                    let tool = tools
                        .iter()
                        .find(|t| t.name() == call.name)
                        .ok_or_else(|| Error::UnknownTool(call.name.clone()))?;
                    let result = tool.call(call.arguments.clone()).await?;
                    conversation.push(Message::tool(call.id.clone(), result));
                }
            }
        }
    }

    Err(Error::ToolLoopExhausted)
}

/// The current UTC time as an RFC 3339 string, or empty on a formatting error.
fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lua-only prompts never build the gateway client, so these run offline.
    fn parse(md: &str) -> Prompt {
        Prompt::parse(md).unwrap()
    }

    #[tokio::test]
    async fn falls_through_to_next_section() {
        let md = "---\nname: t\ndescription: d\nversion: 1\n---\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn \"second\"\n```\n";
        let out = run(&parse(md), "", &[]).await.unwrap();
        assert_eq!(out, "second");
    }

    #[tokio::test]
    async fn explicit_return_stops_fall_through() {
        let md = "---\nname: t\ndescription: d\nversion: 1\n---\n\n\
## First\n\n```lua\nreturn \"first\"\n```\n\n\
## Second\n\n```lua\nreturn \"unreached\"\n```\n";
        let out = run(&parse(md), "", &[]).await.unwrap();
        assert_eq!(out, "first");
    }

    #[tokio::test]
    async fn runs_off_end_to_default_return() {
        let md = "---\nname: t\ndescription: d\nversion: 1\ndefault_return: \"fell off\"\n---\n\n\
## Only\n\n```lua\nlocal x = 1\n```\n";
        let out = run(&parse(md), "", &[]).await.unwrap();
        assert_eq!(out, "fell off");
    }

    #[tokio::test]
    async fn generic_result_when_nothing_produced() {
        let md = "---\nname: t\ndescription: d\nversion: 1\n---\n\n\
## Only\n\n```lua\nlocal x = 1\n```\n";
        let out = run(&parse(md), "", &[]).await.unwrap();
        assert_eq!(out, "done");
    }

    #[tokio::test]
    async fn sys_id_increments_per_section() {
        // First section files nothing and falls through; second returns its id.
        let md = "---\nname: t\ndescription: d\nversion: 1\n---\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn tostring(sys.id)\n```\n";
        let out = run(&parse(md), "", &[]).await.unwrap();
        assert_eq!(out, "2");
    }

    // --- Tool-call loop test (exercises the model round trip via a mock) ---

    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Json;
    use axum::Router;
    use axum::extract::State;
    use axum::routing::post;
    use serde_json::Value;

    use crate::tools::Tool;

    /// A trivial tool that echoes back the `value` argument it is given.
    struct EchoTool;

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
        )]
        fn name(&self) -> &str {
            "echo"
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
        )]
        fn description(&self) -> &str {
            "Echo the value argument back to the caller."
        }

        fn parameters_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"]
            })
        }

        async fn call(&self, args: Value) -> Result<String> {
            let value = args.get("value").and_then(Value::as_str).unwrap_or("");
            Ok(format!("echoed: {value}"))
        }
    }

    /// A second trivial tool, so scoping tests can distinguish which tools a
    /// section selects from a pool of more than one.
    struct NoopTool;

    #[async_trait::async_trait]
    impl Tool for NoopTool {
        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
        )]
        fn name(&self) -> &str {
            "noop"
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
        )]
        fn description(&self) -> &str {
            "Do nothing."
        }

        fn parameters_schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }

        async fn call(&self, _args: Value) -> Result<String> {
            Ok(String::new())
        }
    }

    #[test]
    fn section_scoped_to_one_tool_selects_only_that_one() {
        let echo = EchoTool;
        let noop = NoopTool;
        let pool: &[&dyn Tool] = &[&echo, &noop];
        let selected = scoped_tools(pool, &["echo".to_string()]).unwrap();
        let names: Vec<&str> = selected.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["echo"]);
    }

    #[test]
    fn section_with_no_scope_selects_no_tools() {
        let echo = EchoTool;
        let noop = NoopTool;
        let pool: &[&dyn Tool] = &[&echo, &noop];
        let selected = scoped_tools(pool, &[]).unwrap();
        assert!(selected.is_empty());
    }

    #[test]
    fn scoped_name_absent_from_pool_is_an_error() {
        let echo = EchoTool;
        let pool: &[&dyn Tool] = &[&echo];
        match scoped_tools(pool, &["web_search".to_string()]) {
            Err(Error::UnknownScopedTool(name)) => assert_eq!(name, "web_search"),
            Err(other) => panic!("expected UnknownScopedTool, got {other:?}"),
            Ok(_) => panic!("a scoped name absent from the pool must error"),
        }
    }

    /// Spawn a mock gateway that returns a tool call on its first request and a
    /// final text reply on its second. The call counter is shared so the two
    /// responses are distinguishable.
    async fn spawn_mock_gateway() -> SocketAddr {
        async fn completions(
            State(calls): State<Arc<AtomicUsize>>,
            Json(_body): Json<Value>,
        ) -> Json<Value> {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // First round trip: ask to call the echo tool.
                Json(json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "echo",
                                    "arguments": "{\"value\":\"hi\"}"
                                }
                            }]
                        }
                    }]
                }))
            } else {
                // Second round trip: return the final answer.
                Json(json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "final answer" }
                    }]
                }))
            }
        }

        let state = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route("/v1/chat/completions", post(completions))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        addr
    }

    /// Build the tool schemas the loop advertises, mirroring what `run` does.
    fn schemas_for(tools: &[&dyn Tool]) -> Vec<ToolSchema> {
        tools
            .iter()
            .map(|t| ToolSchema {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters_schema(),
            })
            .collect()
    }

    #[tokio::test]
    async fn tool_loop_dispatches_then_returns_text() {
        // The loop is tested against a real client pointed at the mock gateway.
        // `run_tool_loop` takes the client explicitly, so no process-global env
        // is needed (the crate forbids `unsafe`, which `env::set_var` requires).
        let addr = spawn_mock_gateway().await;
        let client = GatewayClient::new(&format!("http://{addr}/v1"), "test", "test-model");

        let echo = EchoTool;
        let tools: &[&dyn Tool] = &[&echo];
        let schemas = schemas_for(tools);

        let out = run_tool_loop(
            &client,
            &schemas,
            tools,
            "ask the model".to_string(),
            DEFAULT_MAX_TOOL_ITERATIONS,
        )
        .await
        .unwrap();
        assert_eq!(out, "final answer");
    }

    /// A mock gateway that always asks for a tool call, never converging. The
    /// returned counter records how many completion requests it served, so a
    /// test can assert the loop stopped after exactly its cap of round trips.
    async fn spawn_always_tool_call() -> (SocketAddr, Arc<AtomicUsize>) {
        async fn completions(
            State(calls): State<Arc<AtomicUsize>>,
            Json(_body): Json<Value>,
        ) -> Json<Value> {
            calls.fetch_add(1, Ordering::SeqCst);
            Json(json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_x",
                            "type": "function",
                            "function": { "name": "echo", "arguments": "{\"value\":\"x\"}" }
                        }]
                    }
                }]
            }))
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route("/v1/chat/completions", post(completions))
            .with_state(Arc::clone(&calls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (addr, calls)
    }

    #[tokio::test]
    async fn tool_loop_gives_up_after_exactly_the_configured_cap() {
        // A small explicit cap: the loop must make exactly that many round
        // trips against a never-converging model, then exhaust.
        let cap = 3;
        let (addr, calls) = spawn_always_tool_call().await;
        let client = GatewayClient::new(&format!("http://{addr}/v1"), "test", "test-model");

        let echo = EchoTool;
        let tools: &[&dyn Tool] = &[&echo];
        let schemas = schemas_for(tools);

        let err = run_tool_loop(&client, &schemas, tools, "loop forever".to_string(), cap)
            .await
            .expect_err("a never-converging model should exhaust the loop");
        assert!(matches!(err, Error::ToolLoopExhausted));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            cap,
            "the loop must make exactly `cap` round trips before giving up"
        );
    }

    #[tokio::test]
    async fn tool_loop_uses_the_default_cap_when_unspecified() {
        // Threading `DEFAULT_MAX_TOOL_ITERATIONS` (what `run` passes when a
        // prompt declares no budget) makes exactly that many round trips.
        let (addr, calls) = spawn_always_tool_call().await;
        let client = GatewayClient::new(&format!("http://{addr}/v1"), "test", "test-model");

        let echo = EchoTool;
        let tools: &[&dyn Tool] = &[&echo];
        let schemas = schemas_for(tools);

        let err = run_tool_loop(
            &client,
            &schemas,
            tools,
            "loop forever".to_string(),
            DEFAULT_MAX_TOOL_ITERATIONS,
        )
        .await
        .expect_err("a never-converging model should exhaust the loop");
        assert!(matches!(err, Error::ToolLoopExhausted));
        assert_eq!(calls.load(Ordering::SeqCst), DEFAULT_MAX_TOOL_ITERATIONS);
        assert_eq!(DEFAULT_MAX_TOOL_ITERATIONS, 24);
    }

    #[test]
    fn run_resolves_cap_from_frontmatter_else_default() {
        // Mirrors the resolution in `run`: a declared budget wins, an absent
        // one falls back to the raised default.
        let declared =
            "---\nname: t\ndescription: d\nversion: 1\nmax_tool_iterations: 5\n---\n\n## S\n\np\n";
        let p = Prompt::parse(declared).unwrap();
        assert_eq!(
            p.frontmatter
                .max_tool_iterations
                .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS),
            5
        );

        let absent = "---\nname: t\ndescription: d\nversion: 1\n---\n\n## S\n\np\n";
        let p = Prompt::parse(absent).unwrap();
        assert_eq!(
            p.frontmatter
                .max_tool_iterations
                .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS),
            DEFAULT_MAX_TOOL_ITERATIONS
        );
    }

    #[tokio::test]
    async fn tool_loop_errors_on_unknown_tool() {
        // The model asks for "echo" but no tools are provided to the loop.
        let (addr, _calls) = spawn_always_tool_call().await;
        let client = GatewayClient::new(&format!("http://{addr}/v1"), "test", "test-model");

        // Advertise schemas so the request carries tools, but pass no dispatch
        // targets, so the returned call resolves to no tool.
        let echo = EchoTool;
        let schemas = schemas_for(&[&echo]);

        let err = run_tool_loop(
            &client,
            &schemas,
            &[],
            "call unknown".to_string(),
            DEFAULT_MAX_TOOL_ITERATIONS,
        )
        .await
        .expect_err("an unprovided tool should be rejected");
        match err {
            Error::UnknownTool(name) => assert_eq!(name, "echo"),
            other => panic!("expected UnknownTool, got {other:?}"),
        }
    }
}
