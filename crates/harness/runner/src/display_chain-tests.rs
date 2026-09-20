use super::display_chain;

/// A leaf cause with its own text.
#[derive(Debug, thiserror::Error)]
#[error("disk gone")]
struct Leaf;

/// An outer error whose text does not mention its cause.
#[derive(Debug, thiserror::Error)]
#[error("the prompt could not be read")]
struct Outer(#[source] Leaf);

/// An outer error that copies its cause's text into its own message, the
/// shape of a variant such as `LuaRuntime { message, source }`.
#[derive(Debug, thiserror::Error)]
#[error("lua runtime error: {message}")]
struct Copying {
    message: String,
    #[source]
    source: Leaf,
}

#[test]
fn a_two_level_chain_renders_the_cause_after_the_outer_text() {
    let rendered = display_chain(&Outer(Leaf));
    assert_eq!(
        rendered, "the prompt could not be read: disk gone",
        "the cause follows the outer text after a colon"
    );
}

#[test]
fn a_cause_already_quoted_by_the_outer_text_is_not_appended_twice() {
    let error = Copying {
        message: "disk gone".to_owned(),
        source: Leaf,
    };
    let rendered = display_chain(&error);
    assert_eq!(
        rendered, "lua runtime error: disk gone",
        "a cause whose text the outer message already carries is skipped"
    );
    assert_eq!(
        rendered.matches("disk gone").count(),
        1,
        "the cause text appears exactly once"
    );
}

#[test]
fn a_leaf_renders_as_its_own_text() {
    assert_eq!(display_chain(&Leaf), "disk gone");
}
