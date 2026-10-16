//! Library prose: each library block reads its own lazy `prose`, filled
//! from the caller's environment, and the caller's `prose` comes back
//! exactly as it was after every load, a failed one included.

use super::*;

/// `library` with its blocks' prose templates taken from `templates` in
/// order.
fn with_prose(mut library: Library, templates: &[&str]) -> Library {
    for (block, template) in library.blocks.iter_mut().zip(templates) {
        (*template).clone_into(&mut block.prose);
    }
    library
}

/// `x` in a snapshot's `var`, or `unset`.
fn var_x(var: &Json) -> String {
    var.get("x")
        .and_then(Json::as_str)
        .unwrap_or("unset")
        .to_owned()
}

/// A library renderer that fills `{{ var.x }}` from the snapshot.
#[expect(
    clippy::unnecessary_wraps,
    reason = "a library prose renderer returns a Result"
)]
fn fill_var_x(template: &str, state: ProseState<'_>) -> mlua::Result<String> {
    let ProseState { var, .. } = state;
    Ok(template.replace("{{ var.x }}", &var_x(&var)))
}

#[test]
fn a_library_block_reads_its_own_prose_filled_from_the_callers_var() {
    let (vm, _) = require_vm_rendering(
        vec![(
            "## lib",
            None,
            with_prose(
                library(
                    "lib",
                    "## lib",
                    &["local ok = pcall(function() prose = 'x' end)\n\
                       return prose .. '|' .. tostring(ok)"],
                ),
                &["lib sees {{ var.x }}"],
            ),
        )],
        fill_var_x,
        &null_emitter(),
    );
    vm.install_lazy_prose(|_| Ok("caller text".to_owned()))
        .expect("the caller's prose must install");
    let value = returned(
        &vm,
        "var.x = 'seven'\nlocal seen = require '## lib'\nreturn seen .. '|' .. prose",
    );
    assert_eq!(value.as_deref(), Some("lib sees seven|false|caller text"));
}

#[test]
fn the_callers_read_prose_keeps_its_memo_across_a_require() {
    let renders = Arc::new(AtomicUsize::new(0));
    let (vm, _) = require_vm_rendering(
        vec![(
            "## lib",
            None,
            with_prose(library("lib", "## lib", &["return prose"]), &["lib text"]),
        )],
        fill_var_x,
        &null_emitter(),
    );
    let counted = Arc::clone(&renders);
    vm.install_lazy_prose(move |state| {
        counted.fetch_add(1, Ordering::Relaxed);
        Ok(format!("caller saw {}", var_x(&state.var)))
    })
    .expect("the caller's prose must install");
    let value = returned(
        &vm,
        "var.x = 'one'\nlocal before = prose\nvar.x = 'two'\n\
         local seen = require '## lib'\nreturn before .. '|' .. seen .. '|' .. prose",
    );
    assert_eq!(
        value.as_deref(),
        Some("caller saw one|lib text|caller saw one")
    );
    assert_eq!(
        renders.load(Ordering::Relaxed),
        1,
        "the caller's prose must render once, before the require"
    );
}

#[test]
fn a_load_that_raises_still_puts_the_callers_prose_back() {
    let (vm, _) = require_vm_rendering(
        vec![(
            "## bad",
            None,
            with_prose(
                library("bad", "## bad", &["error('fault: ' .. prose)"]),
                &["bad text"],
            ),
        )],
        fill_var_x,
        &null_emitter(),
    );
    vm.install_lazy_prose(|_| Ok("caller text".to_owned()))
        .expect("the caller's prose must install");
    let value = returned(
        &vm,
        "local ok, err = pcall(require, '## bad')\n\
         return tostring(ok) .. '|' .. tostring(err):match('fault: .*$') .. '|' .. prose",
    );
    assert_eq!(value.as_deref(), Some("false|fault: bad text|caller text"));
}
