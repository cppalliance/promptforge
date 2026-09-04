//! The relaunch handoff: a second `promptforge-gateway` launch while one is
//! running opens the running gateway's Settings page instead of booting a
//! duplicate server.
//!
//! The check runs before any bind attempt: the connection file in the run
//! directory is resolved through `shared-sidecar` (pid alive, image is a
//! gateway, health answers, key accepted), and a live file yields the
//! one-time `/auth` handoff URL for its bound port. A stale file is
//! deleted by the resolution - this process is the prospective owner and
//! rewrites the file after its own bind, so early cleanup cannot strand a
//! live gateway. On the desktop this is also the `.desktop` launcher's
//! relaunch behavior; on a server it keeps a second invocation from
//! shadowing `gateway.json` and hijacking discovery.

use crate::runner::ServeOptions;

/// What a launch does about an existing connection file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Relaunch {
    /// A live gateway owns the file: hand off with its Settings URL.
    OpenSettings(String),
    /// Nothing live owns the file: boot normally.
    Boot,
}

/// Maps a connection-file resolution to the relaunch decision. Only a
/// fully live file hands off; `Absent` and every stale reason boot.
pub(crate) fn decide(resolution: &shared_sidecar::Resolution) -> Relaunch {
    match resolution {
        shared_sidecar::Resolution::Attach(file) => {
            // The file carries the real port of the loopback bind; URLs
            // normalize to a literal 127.0.0.1, never localhost.
            Relaunch::OpenSettings(crate::handoff::auth_url(
                &format!("http://127.0.0.1:{}", file.port),
                &file.api_key,
            ))
        }
        _ => Relaunch::Boot,
    }
}

/// The running gateway's Settings handoff URL, when a live gateway owns
/// the connection file. The caller exits without binding: it opens the URL
/// in the browser, prints it, or - for a login-triggered launch - just
/// exits. Returns `None` when nothing live is found or the file cannot be
/// resolved (a resolution error boots normally; the gateway is the file's
/// owner and rewrites it after bind).
#[must_use]
pub fn running_gateway_settings_url(options: &ServeOptions) -> Option<String> {
    let run_dir = options
        .run_dir
        .clone()
        .or_else(shared_sidecar::default_run_dir)?;
    let resolution = match shared_sidecar::resolve(&run_dir) {
        Ok(resolution) => resolution,
        Err(error) => {
            tracing::warn!(
                "could not resolve the connection file in {}: {error}; booting normally",
                run_dir.display()
            );
            return None;
        }
    };
    match decide(&resolution) {
        Relaunch::OpenSettings(url) => Some(url),
        Relaunch::Boot => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection file as a live gateway would write it.
    fn live_file() -> shared_sidecar::ConnectionFile {
        shared_sidecar::ConnectionFile {
            port: 8081,
            api_key: "abc123".to_owned(),
            pid: std::process::id(),
            epoch: 1_757_000_000,
            version: "0.2.0".to_owned(),
            started_at: "2026-09-03T12:00:00Z".to_owned(),
        }
    }

    #[test]
    fn a_live_file_hands_off_its_settings_url() {
        let decision = decide(&shared_sidecar::Resolution::Attach(live_file()));
        assert_eq!(
            decision,
            Relaunch::OpenSettings("http://127.0.0.1:8081/auth?key=abc123".to_owned()),
            "the handoff URL targets the live gateway's one-time /auth redirect"
        );
    }

    #[test]
    fn a_live_file_with_a_query_special_key_encodes_the_url() {
        let file = shared_sidecar::ConnectionFile {
            api_key: "a&b=c d".to_owned(),
            ..live_file()
        };
        let decision = decide(&shared_sidecar::Resolution::Attach(file));
        assert_eq!(
            decision,
            Relaunch::OpenSettings("http://127.0.0.1:8081/auth?key=a%26b%3Dc+d".to_owned()),
            "a configured key survives the handoff"
        );
    }

    #[test]
    fn absent_and_stale_files_boot_normally() {
        assert_eq!(decide(&shared_sidecar::Resolution::Absent), Relaunch::Boot);
        for reason in [
            shared_sidecar::StaleReason::Invalid,
            shared_sidecar::StaleReason::ProcessDead,
            shared_sidecar::StaleReason::ImageMismatch,
            shared_sidecar::StaleReason::HealthFailed,
            shared_sidecar::StaleReason::KeyRejected,
        ] {
            assert_eq!(
                decide(&shared_sidecar::Resolution::Stale(reason)),
                Relaunch::Boot,
                "stale ({reason:?}) boots rather than handing off"
            );
        }
    }

    #[test]
    fn an_empty_run_dir_resolves_to_boot() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let options = ServeOptions::new(None, None::<crate::ProfileName>)
            .with_run_dir(temp.path().to_path_buf());
        assert_eq!(running_gateway_settings_url(&options), None);
    }

    #[test]
    fn a_stale_file_boots_and_is_cleaned() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        // A dead pid: a short-lived child, reaped and dropped.
        let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a short-lived child");
        let pid = child.id();
        child.wait().expect("the child exits");
        drop(child);
        let file = shared_sidecar::ConnectionFile { pid, ..live_file() };
        file.write_to(temp.path()).expect("write fixture");
        let options = ServeOptions::new(None, None::<crate::ProfileName>)
            .with_run_dir(temp.path().to_path_buf());

        assert_eq!(
            running_gateway_settings_url(&options),
            None,
            "a stale file never hands off"
        );
        assert!(
            !shared_sidecar::connection_file_path(temp.path()).exists(),
            "the stale file was deleted so the boot rewrites it cleanly"
        );
    }
}
