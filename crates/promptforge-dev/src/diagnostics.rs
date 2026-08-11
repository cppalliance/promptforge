//! Human-facing diagnostics for the interactive runner: the stderr observer
//! and the failure formatter that leads a run error with `prompt.md:LINE:`.

use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use promptforge_core::observe::{Observation, Observer};

/// Formats a failed run so the first line leads with `prompt.md:LINE:` when
/// core mapped a Lua error to an absolute prompt line.
pub(crate) fn format_dev_failure(prompt_path: &Path, error: &anyhow::Error) -> String {
    let detail = format!("{error:#}");
    if let Some(line) = first_mapped_prompt_line(&detail) {
        format!(
            "dev run failed: {}:{}: {detail}",
            prompt_path.display(),
            line
        )
    } else {
        format!("dev run failed: {detail}")
    }
}

/// Pulls the innermost absolute prompt line from a core-mapped Lua error.
///
/// Core prefixes failures as `section \`Name\` epilog:51: ...`. Prefer the
/// last such tag so a fanout parent wrapper does not hide the arm that failed.
fn first_mapped_prompt_line(message: &str) -> Option<u32> {
    let mut found = None;
    let mut rest = message;
    while let Some(idx) = rest.find(':') {
        let after = &rest[idx + 1..];
        let digit_end = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if digit_end > 0
            && after.as_bytes().get(digit_end) == Some(&b':')
            && let Ok(line) = after[..digit_end].parse::<u32>()
        {
            // The surrounding tag (`epilog` / `prologue` / `library`) is the
            // real filter; small numeric noise elsewhere is ignored.
            let before = &rest[..idx];
            if before.ends_with("epilog")
                || before.ends_with("prologue")
                || before.ends_with("library")
            {
                found = Some(line);
            }
        }
        rest = &rest[idx + 1..];
    }
    found
}

/// An observer that writes every record as one line to its sink.
pub(crate) struct VerboseObserver<W> {
    sink: Mutex<W>,
}

impl<W> std::fmt::Debug for VerboseObserver<W> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("VerboseObserver").finish()
    }
}

impl<W: Write + Send> VerboseObserver<W> {
    pub(crate) fn new(sink: W) -> Self {
        Self {
            sink: Mutex::new(sink),
        }
    }
}

impl<W: Write + Send> Observer for VerboseObserver<W> {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        let mut sink = self.sink.lock().unwrap_or_else(PoisonError::into_inner);
        // Observers must not panic and reporting is a side channel, so a failed
        // write to stderr is deliberately dropped rather than surfaced.
        let _ignored = writeln!(sink, "{}", format_record(execution, section, &event));
    }
}

/// Formats one `(execution, section, event)` record as one trace line.
fn format_record(execution: &str, section: &str, event: &Observation) -> String {
    format!("[{execution}] {section}: {event}")
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;
    use std::sync::{Arc, Mutex, PoisonError};

    use promptforge_core::observe::Observation;

    use super::{VerboseObserver, first_mapped_prompt_line, format_dev_failure, format_record};

    #[derive(Clone, Debug, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn contents(&self) -> String {
            let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            String::from_utf8(bytes.clone()).expect("observer output must be UTF-8")
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn record_formats_as_one_bracketed_trace_line() {
        assert_eq!(
            format_record(
                "dev-00000000deadbeef",
                "Research",
                &Observation::Lua("checkpoint".to_owned())
            ),
            "[dev-00000000deadbeef] Research: Lua: checkpoint"
        );
    }

    #[test]
    fn mapped_lua_failure_leads_with_prompt_path_and_line() {
        let detail = "run briefer.md: lua error: lua error: section `Web Search` epilog:51: \
             [string \"section `Web Search` epilog\"]:51: assertion failed!";
        assert_eq!(first_mapped_prompt_line(detail), Some(51));
        let error = anyhow::anyhow!(detail);
        let formatted = format_dev_failure(Path::new("briefer.md"), &error);
        assert!(
            formatted.starts_with("dev run failed: briefer.md:51:"),
            "expected path:line prefix, got {formatted}"
        );
    }

    #[test]
    fn unmapped_failure_has_no_line_prefix() {
        let error = anyhow::anyhow!("some transport error with no prompt tag");
        let formatted = format_dev_failure(Path::new("briefer.md"), &error);
        assert!(
            formatted.starts_with("dev run failed: some transport error"),
            "unexpected formatting: {formatted}"
        );
    }

    #[test]
    fn verbose_observer_writes_every_record_as_its_own_line() {
        use promptforge_core::observe::Observer;

        let buffer = SharedBuffer::default();
        let observer = VerboseObserver::new(buffer.clone());

        observer.observe("dev-1", "Prompt", Observation::RunStarted);
        observer.observe("dev-1", "Section", Observation::Lua("step one".to_owned()));

        assert_eq!(
            buffer.contents(),
            "[dev-1] Prompt: Run started\n[dev-1] Section: Lua: step one\n"
        );
    }
}
