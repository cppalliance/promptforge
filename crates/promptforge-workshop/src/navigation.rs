//! The navigation policy: the webview may load only the server's own
//! origin; every other http(s) target opens in the system browser.
//!
//! A clicked link never navigates the app away from itself, and no other
//! loopback server - same machine, different port, or a different
//! loopback spelling - gets the desktop app's Tauri API surface, folder
//! picker, or file-drop bridge. Other schemes (`about:blank`, `data:`)
//! and unparseable values are left to the webview.

/// What the app does with a URL the webview wants to navigate to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Navigation {
    /// The webview loads the URL itself.
    Allow,
    /// The URL opens in the system browser; the webview stays put.
    OpenExternally,
}

/// Classifies a navigation target against the server's origin: a
/// same-origin http(s) URL (the in-process server) loads in the webview,
/// while any other absolute http(s) URL opens in the system browser.
#[must_use]
pub(crate) fn classify_navigation(server: &url::Origin, target: &str) -> Navigation {
    let Ok(url) = url::Url::parse(target) else {
        return Navigation::Allow;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return Navigation::Allow;
    }
    if url.origin() == *server {
        Navigation::Allow
    } else {
        Navigation::OpenExternally
    }
}

#[cfg(test)]
mod tests {
    use super::{Navigation, classify_navigation};

    /// The origin of the in-process server the test window was opened on.
    fn server_origin(url: &str) -> url::Origin {
        match url::Url::parse(url) {
            Ok(parsed) => parsed.origin(),
            Err(error) => panic!("the test server URL must parse: {error}"),
        }
    }

    #[test]
    fn same_origin_urls_load_in_the_webview() {
        let server = server_origin("http://127.0.0.1:7910/");
        for url in [
            "http://127.0.0.1:7910/",
            "http://127.0.0.1:7910/ws",
            "http://127.0.0.1:7910/settings?tab=keys#top",
        ] {
            assert_eq!(
                classify_navigation(&server, url),
                Navigation::Allow,
                "{url}"
            );
        }
    }

    #[test]
    fn a_default_port_normalizes_into_the_origin() {
        let server = server_origin("https://127.0.0.1/");
        assert_eq!(
            classify_navigation(&server, "https://127.0.0.1:443/"),
            Navigation::Allow
        );
    }

    #[test]
    fn other_loopback_origins_open_in_the_system_browser() {
        let server = server_origin("http://127.0.0.1:7910/");
        for url in [
            // Another loopback server is not the workshop, whatever the
            // port, scheme, or loopback spelling.
            "http://127.0.0.1:4000/",
            "https://127.0.0.1:7910/",
            "http://localhost:7910/",
            "http://[::1]:7910/",
        ] {
            assert_eq!(
                classify_navigation(&server, url),
                Navigation::OpenExternally,
                "{url}"
            );
        }
    }

    #[test]
    fn external_urls_open_in_the_system_browser() {
        let server = server_origin("http://127.0.0.1:7910/");
        for url in [
            "https://example.com/",
            "http://192.168.1.10/",
            "https://localhost.evil.example/",
        ] {
            assert_eq!(
                classify_navigation(&server, url),
                Navigation::OpenExternally,
                "{url}"
            );
        }
    }

    #[test]
    fn non_http_and_unparseable_targets_are_left_to_the_webview() {
        let server = server_origin("http://127.0.0.1:7910/");
        for url in [
            "about:blank",
            "data:text/html,<p>hi</p>",
            "not a url",
            "/relative/path",
        ] {
            assert_eq!(
                classify_navigation(&server, url),
                Navigation::Allow,
                "{url}"
            );
        }
    }
}
