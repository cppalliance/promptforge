//! The control globals from H1: `call`, `jump`, `fanout`, and
//! `list_from_section` naming an unknown section fail the run.

use super::*;

/// The control globals work in H1 (section 0): a `call` naming no
/// top-level section fails the run with the resolution error naming the
/// target.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_from_h1_to_an_unknown_section_fails_the_run() {
    let md = flow_prompt!(
        "\
# Test prompt\n\n\
```lua\n\
call('## Nope')\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("call from H1 to an unknown section must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("## Nope"),
        "the resolution error names the missing section: {rendered}"
    );
}

/// `jump` out of H1 names a top-level section; an unknown target fails the
/// run with the resolution error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_from_h1_to_an_unknown_section_fails_the_run() {
    let md = flow_prompt!(
        "\
# Test prompt\n\n\
```lua\n\
jump('## Nope')\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("jump from H1 to an unknown section must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("## Nope"),
        "the resolution error names the missing section: {rendered}"
    );
}

/// `fanout` from H1 resolves its worker against the top-level sections; an
/// unknown worker fails the run with the resolution error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_from_h1_to_an_unknown_section_fails_the_run() {
    let md = flow_prompt!(
        "\
# Test prompt\n\n\
```lua\n\
fanout('## Nope', {'a'})\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("fanout from H1 to an unknown section must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("## Nope"),
        "the resolution error names the missing section: {rendered}"
    );
}

/// `list_from_section` from H1 resolves over the whole top-level slice; an
/// unknown target fails the run with the resolution error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_from_section_from_h1_to_an_unknown_section_fails_the_run() {
    let md = flow_prompt!(
        "\
# Test prompt\n\n\
```lua\n\
list_from_section('## Nope')\n\
```\n"
    );
    let error = run_offline(md)
        .await
        .expect_err("list_from_section from H1 to an unknown section must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("## Nope"),
        "the resolution error names the missing section: {rendered}"
    );
}
