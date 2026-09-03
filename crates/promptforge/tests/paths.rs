//! Compile-level proof that the facade entry paths resolve.

#[test]
fn pipeline_and_agent_paths_resolve() {
    use promptforge::agent::{AgentConfig, AgentError, run as agent_run};
    use promptforge::pipeline::{RunConfig, RunError, run as pipeline_run};

    // Function items and types must resolve; nothing here is executed for effect.
    let _ = std::mem::size_of_val(&pipeline_run);
    let _ = std::mem::size_of_val(&agent_run);
    let _: Option<RunConfig> = None;
    let _: Option<RunError> = None;
    let _: Option<AgentConfig> = None;
    let _: Option<AgentError> = None;
}
