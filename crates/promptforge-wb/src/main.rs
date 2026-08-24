//! The `promptforge-wb` binary: the PromptForge Workbench desktop window
//! shell.
//!
//! Empty for now. The wry/tao window arrives at the shell step of the
//! workbench build; this skeleton exists so the workspace builds fast while
//! the server crate takes shape.

fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_is_named_promptforge_wb() {
        assert_eq!(env!("CARGO_PKG_NAME"), "promptforge-wb");
    }
}
