//! Download progress reporters: the callback trait, the no-op reporter, and activity percent text.

use std::sync::atomic::{AtomicU64, Ordering};

use gateway_progress::Activity;

/// Progress updates for a single HTTP blob download.
pub trait DownloadProgress: Send {
    /// Records the total length in bytes, when the server sent one.
    fn set_len(&self, total: Option<u64>);
    /// Adds `n` downloaded bytes to the running total.
    fn inc(&self, n: u64);
}

/// A [`DownloadProgress`] that discards every callback, for callers with no
/// activity to report into.
pub(in super::super) struct NoopProgress;

impl DownloadProgress for NoopProgress {
    fn set_len(&self, _total: Option<u64>) {}

    fn inc(&self, _n: u64) {}
}

/// A `"{verb} {name} {pct}%"` line republished into an activity on each
/// whole-percent change only, so a byte-granular loop never floods the
/// hub's subscribers.
#[derive(Debug)]
pub struct PercentText {
    verb: &'static str,
    name: String,
    /// The last percent published, or `u64::MAX` before the first.
    percent: AtomicU64,
}

impl PercentText {
    /// A reporter for `"{verb} {name} ..."`, e.g. `("Downloading", "qwen.gguf")`.
    #[must_use]
    pub fn new(verb: &'static str, name: impl Into<String>) -> Self {
        Self {
            verb,
            name: name.into(),
            percent: AtomicU64::new(u64::MAX),
        }
    }

    /// The bare `"{verb} {name}"` line, for a phase whose length is unknown.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{} {}", self.verb, self.name)
    }

    /// Publishes `done` of `total` as a whole percent when it changed. A
    /// zero total publishes the bare label once.
    pub fn report(&self, activity: &Activity, done: u64, total: u64) {
        if total == 0 {
            if self.percent.swap(0, Ordering::Relaxed) != 0 {
                activity.set_text(self.label());
            }
            return;
        }
        let percent = done.saturating_mul(100) / total;
        let percent = percent.min(100);
        if self.percent.swap(percent, Ordering::Relaxed) != percent {
            activity.set_text(format!("{} {} {percent}%", self.verb, self.name));
        }
    }
}

/// A [`DownloadProgress`] that formats `"Downloading {name} {pct}%"` into an
/// activity's text on each whole-percent change. Without a Content-Length
/// the text stays at the bare `"Downloading {name}"`.
pub(in super::super) struct ActivityProgress<'a> {
    activity: &'a Activity,
    text: PercentText,
    total: AtomicU64,
    downloaded: AtomicU64,
}

impl<'a> ActivityProgress<'a> {
    pub(in super::super) fn new(activity: &'a Activity, name: &str) -> Self {
        let text = PercentText::new("Downloading", name);
        activity.set_text(text.label());
        Self {
            activity,
            text,
            total: AtomicU64::new(0),
            downloaded: AtomicU64::new(0),
        }
    }
}

impl DownloadProgress for ActivityProgress<'_> {
    fn set_len(&self, total: Option<u64>) {
        self.total.store(total.unwrap_or(0), Ordering::Relaxed);
    }

    fn inc(&self, n: u64) {
        let downloaded = self.downloaded.fetch_add(n, Ordering::Relaxed) + n;
        let total = self.total.load(Ordering::Relaxed);
        if total > 0 {
            self.text.report(self.activity, downloaded, total);
        }
    }
}
