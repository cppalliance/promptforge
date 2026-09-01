//! Command execution seam: every external tool invocation goes through [`Probe`].

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::Context as _;

/// Maximum bytes retained from each of a child process's output streams.
pub const OUTPUT_LIMIT: usize = 64 * 1024;

/// One external command invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    /// Program path, or a name resolved through the child's `PATH`.
    pub program: PathBuf,
    /// Argument vector, excluding the program name.
    pub args: Vec<String>,
    /// Working directory for the child; `None` inherits the caller's.
    pub cwd: Option<PathBuf>,
    /// Directories prepended to the child's `PATH` for this invocation only.
    pub path_prefix: Vec<PathBuf>,
}

impl CommandRequest {
    /// Creates a request for `program` with no arguments.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            path_prefix: Vec::new(),
        }
    }

    /// Sets the argument vector.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the child's working directory.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Prepends `dir` to the child's `PATH`.
    #[must_use]
    pub fn path_prefix(mut self, dir: impl Into<PathBuf>) -> Self {
        self.path_prefix.push(dir.into());
        self
    }

    /// Renders the invocation as one display line, for errors and test fakes.
    #[must_use]
    pub fn display_line(&self) -> String {
        format!("{} {}", self.program.display(), self.args.join(" "))
    }
}

/// Bounded captured result of one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// Exit code; negative when the process did not exit on its own.
    pub code: i32,
    /// Captured standard output, truncated at [`OUTPUT_LIMIT`].
    pub stdout: String,
    /// Captured standard error, truncated at [`OUTPUT_LIMIT`].
    pub stderr: String,
}

impl CommandOutput {
    /// Returns true when the exit code is zero.
    #[must_use]
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// Runs external commands on behalf of the build pipeline.
pub trait Probe {
    /// Runs one command, capturing bounded output.
    ///
    /// # Errors
    /// Returns an error when the command cannot be spawned or awaited.
    fn run(&self, request: &CommandRequest) -> anyhow::Result<CommandOutput>;
}

/// Runs external commands against the real operating system.
#[derive(Debug, Default)]
pub struct SystemProbe;

impl Probe for SystemProbe {
    fn run(&self, request: &CommandRequest) -> anyhow::Result<CommandOutput> {
        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &request.cwd {
            command.current_dir(cwd);
        }
        if !request.path_prefix.is_empty() {
            let mut paths = request.path_prefix.clone();
            if let Some(existing) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&existing));
            }
            let joined = std::env::join_paths(&paths).context("join child PATH")?;
            command.env("PATH", joined);
        }
        let output = command
            .output()
            .with_context(|| format!("spawn `{}`", request.display_line()))?;
        Ok(CommandOutput {
            code: output.status.code().unwrap_or(-1),
            stdout: bounded(&output.stdout),
            stderr: bounded(&output.stderr),
        })
    }
}

/// Lossy-decodes `bytes` and truncates at [`OUTPUT_LIMIT`] with a marker.
pub(crate) fn bounded(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= OUTPUT_LIMIT {
        return text.into_owned();
    }
    let mut end = OUTPUT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n... [truncated, {} bytes total]",
        &text[..end],
        text.len()
    )
}

#[cfg(test)]
pub(crate) mod fake {
    use std::sync::Mutex;

    use super::{CommandOutput, CommandRequest, Probe};

    /// Scripted [`Probe`]: matches invocations by command-line substring.
    #[derive(Debug, Default)]
    pub(crate) struct FakeProbe {
        rules: Vec<(String, CommandOutput)>,
        invocations: Mutex<Vec<String>>,
    }

    impl FakeProbe {
        /// Adds a rule: invocations containing `needle` return `output`.
        pub(crate) fn on(mut self, needle: &str, output: CommandOutput) -> Self {
            self.rules.push((needle.to_string(), output));
            self
        }

        /// Returns every rendered invocation line, in order.
        pub(crate) fn invocations(&self) -> Vec<String> {
            self.invocations
                .lock()
                .expect("invocations mutex poisoned")
                .clone()
        }
    }

    impl Probe for FakeProbe {
        fn run(&self, request: &CommandRequest) -> anyhow::Result<CommandOutput> {
            let line = request.display_line();
            self.invocations
                .lock()
                .expect("invocations mutex poisoned")
                .push(line.clone());
            for (needle, output) in &self.rules {
                if line.contains(needle) {
                    return Ok(output.clone());
                }
            }
            anyhow::bail!("FakeProbe: no rule matched `{line}`")
        }
    }

    /// A successful output carrying `stdout`, bounded like the real probe.
    pub(crate) fn ok(stdout: &str) -> CommandOutput {
        CommandOutput {
            code: 0,
            stdout: super::bounded(stdout.as_bytes()),
            stderr: String::new(),
        }
    }

    /// A failed output carrying `stderr`, bounded like the real probe.
    pub(crate) fn fail(code: i32, stderr: &str) -> CommandOutput {
        CommandOutput {
            code,
            stdout: String::new(),
            stderr: super::bounded(stderr.as_bytes()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_passes_short_output_through() {
        assert_eq!(bounded(b"hello"), "hello");
    }

    #[test]
    fn bounded_truncates_with_marker() {
        let big = vec![b'x'; OUTPUT_LIMIT + 4096];
        let text = bounded(&big);
        assert!(text.len() < OUTPUT_LIMIT + 100);
        assert!(text.contains(&format!("[truncated, {} bytes total]", OUTPUT_LIMIT + 4096)));
    }

    #[test]
    fn fake_probe_reports_unmatched_invocations() {
        let probe = fake::FakeProbe::default();
        let err = probe
            .run(&CommandRequest::new("cmake").args(["--version"]))
            .unwrap_err();
        assert!(err.to_string().contains("no rule matched"));
    }

    #[test]
    fn fake_probe_matches_first_rule_and_records() {
        let probe = fake::FakeProbe::default()
            .on("--version", fake::ok("1.0"))
            .on("cmake", fake::ok("other"));
        let out = probe
            .run(&CommandRequest::new("cmake").args(["--version"]))
            .unwrap();
        assert_eq!(out.stdout, "1.0");
        assert_eq!(probe.invocations(), vec!["cmake --version".to_string()]);
    }
}
