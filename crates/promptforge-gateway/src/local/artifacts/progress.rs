//! Download progress reporting: interactive TTY bar or non-TTY tracing lines.

use std::sync::atomic::{AtomicU64, Ordering};

use indicatif::{ProgressBar, ProgressStyle};

/// Non-TTY log cadence: every 64 MiB or 5% of Content-Length, whichever fires first.
const LOG_PROGRESS_BYTES: u64 = 64 * 1024 * 1024;

/// Progress updates for a single HTTP blob download.
pub(super) trait DownloadProgress: Send {
    fn set_len(&self, total: Option<u64>);
    fn inc(&self, n: u64);
    fn finish(&self);
    fn abandon(&self);
}

/// Basename (or short fallback) shown on the progress bar / log lines.
pub(super) fn download_label(url: &str) -> String {
    url.rsplit('/')
        .next()
        .and_then(|part| {
            let name = part.split('?').next().unwrap_or(part);
            (!name.is_empty()).then(|| name.to_owned())
        })
        .unwrap_or_else(|| "download".to_owned())
}

/// Chooses a TTY progress bar or non-TTY tracing progress for `label`.
pub(super) fn progress_for_download(label: &str, is_tty: bool) -> Box<dyn DownloadProgress> {
    if is_tty {
        Box::new(IndicatifProgress::new(label))
    } else {
        Box::new(TracingProgress::new(label))
    }
}

/// Interactive stderr progress bar via indicatif.
struct IndicatifProgress {
    bar: ProgressBar,
}

impl IndicatifProgress {
    fn new(label: &str) -> Self {
        let bar = ProgressBar::new_spinner();
        bar.set_message(label.to_owned());
        if let Some(style) = bar_style() {
            bar.set_style(style);
        }
        Self { bar }
    }
}

fn bar_style() -> Option<ProgressStyle> {
    ProgressStyle::with_template(
        "{spinner:.green} {msg} [{bar:40.cyan/blue}] {percent:>3}% {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta})",
    )
    .ok()
    .map(|style| style.progress_chars("=>-"))
}

fn spinner_style() -> Option<ProgressStyle> {
    ProgressStyle::with_template("{spinner:.green} {msg} {bytes} ({bytes_per_sec})").ok()
}

impl DownloadProgress for IndicatifProgress {
    fn set_len(&self, total: Option<u64>) {
        match total {
            Some(len) if len > 0 => {
                self.bar.set_length(len);
                if let Some(style) = bar_style() {
                    self.bar.set_style(style);
                }
            }
            _ => {
                if let Some(style) = spinner_style() {
                    self.bar.set_style(style);
                }
            }
        }
    }

    fn inc(&self, n: u64) {
        self.bar.inc(n);
    }

    fn finish(&self) {
        self.bar.finish_and_clear();
    }

    fn abandon(&self) {
        self.bar.abandon();
    }
}

/// Non-TTY progress: periodic `tracing::info!` lines.
struct TracingProgress {
    label: String,
    total: AtomicU64,
    downloaded: AtomicU64,
    last_logged: AtomicU64,
}

impl TracingProgress {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            total: AtomicU64::new(0),
            downloaded: AtomicU64::new(0),
            last_logged: AtomicU64::new(0),
        }
    }

    fn maybe_log(&self, force: bool) {
        let downloaded = self.downloaded.load(Ordering::Relaxed);
        let total = self.total.load(Ordering::Relaxed);
        let last = self.last_logged.load(Ordering::Relaxed);
        let step = if total > 0 {
            (total / 20).max(LOG_PROGRESS_BYTES)
        } else {
            LOG_PROGRESS_BYTES
        };
        if !force && downloaded.saturating_sub(last) < step && downloaded != 0 {
            return;
        }
        self.last_logged.store(downloaded, Ordering::Relaxed);
        if let Some(percent) = downloaded.saturating_mul(100).checked_div(total) {
            tracing::info!(
                file = %self.label,
                downloaded,
                total,
                percent,
                "download progress"
            );
        } else {
            tracing::info!(
                file = %self.label,
                downloaded,
                "download progress"
            );
        }
    }
}

impl DownloadProgress for TracingProgress {
    fn set_len(&self, total: Option<u64>) {
        if let Some(len) = total {
            self.total.store(len, Ordering::Relaxed);
        }
        tracing::info!(
            file = %self.label,
            total = total.unwrap_or(0),
            "download started"
        );
    }

    fn inc(&self, n: u64) {
        self.downloaded.fetch_add(n, Ordering::Relaxed);
        self.maybe_log(false);
    }

    fn finish(&self) {
        self.maybe_log(true);
        tracing::info!(
            file = %self.label,
            downloaded = self.downloaded.load(Ordering::Relaxed),
            "download finished"
        );
    }

    fn abandon(&self) {
        tracing::warn!(
            file = %self.label,
            downloaded = self.downloaded.load(Ordering::Relaxed),
            "download abandoned"
        );
    }
}
