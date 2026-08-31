//! The `GET /admin/system` route: live host metrics for the config UI's
//! Settings > System cards - CPU, RAM, the artifact-cache drive, and the
//! NVIDIA GPU when a driver is present.
//!
//! CPU utilization is a delta between two readings, so the sampler keeps one
//! process-wide `sysinfo::System` and primes it on first use; every later
//! request reports the change since the previous poll (the UI polls every
//! 5s). Sampling reads OS counters and the first call sleeps one CPU-update
//! interval, so it runs inside `tokio::task::spawn_blocking` like every
//! store operation (Amendment D).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use nvml_wrapper::Nvml;
use serde::Serialize;
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};

use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// One `GET /admin/system` snapshot.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SystemSnapshot {
    /// Processor identity and load.
    cpu: CpuMetrics,
    /// Physical memory usage.
    ram: RamMetrics,
    /// Usage of the drive holding the artifact cache; absent when the cache
    /// root cannot be resolved or matched to a mounted disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    disk: Option<DiskMetrics>,
    /// The first NVIDIA GPU; absent - never an error - on machines without
    /// an NVML-capable driver.
    #[serde(skip_serializing_if = "Option::is_none")]
    gpu: Option<GpuMetrics>,
}

/// Processor identity and load.
#[derive(Debug, Clone, Serialize)]
struct CpuMetrics {
    /// The first logical core's reported frequency in MHz; 0 when unknown.
    frequency_mhz: u64,
    /// Logical core count.
    logical_cores: usize,
    /// Physical core count; null when the OS does not report it.
    physical_cores: Option<usize>,
    /// Whole-system CPU utilization, 0-100. Reads 0 only if sampled before
    /// the sampler is primed, which the priming pass prevents.
    utilization_percent: f32,
}

/// Physical memory usage in bytes.
#[derive(Debug, Clone, Serialize)]
struct RamMetrics {
    /// Bytes of RAM in use.
    used_bytes: u64,
    /// Bytes of RAM installed.
    total_bytes: u64,
}

/// Usage of the disk holding the artifact cache.
#[derive(Debug, Clone, Serialize)]
struct DiskMetrics {
    /// The resolved artifact cache root whose drive is reported.
    cache_dir: String,
    /// Bytes in use on the drive.
    used_bytes: u64,
    /// The drive's capacity in bytes.
    total_bytes: u64,
}

/// Name and VRAM of the first NVIDIA GPU.
#[derive(Debug, Clone, Serialize)]
struct GpuMetrics {
    /// The device name NVML reports (e.g. "NVIDIA GeForce RTX 4090").
    name: String,
    /// Bytes of VRAM in use across the device.
    vram_used_bytes: u64,
    /// Bytes of VRAM on the device.
    vram_total_bytes: u64,
}

/// Process-wide sampling state behind `AppState`: the retained
/// `sysinfo::System` whose successive refreshes yield CPU utilization
/// deltas, and the once-per-process NVML probe.
pub(crate) struct SystemSampler {
    system: System,
    /// Whether the double-refresh priming pass has run, after which a single
    /// refresh per request yields a meaningful utilization delta.
    primed: bool,
    /// Whether `Nvml::init` has been attempted; the probe is not retried
    /// because init loads the shared library and its symbols each call.
    nvml_probed: bool,
    nvml: Option<Nvml>,
}

impl SystemSampler {
    /// An unprimed sampler; the first sample takes the two CPU readings a
    /// utilization delta needs.
    pub(crate) fn new() -> SystemSampler {
        SystemSampler {
            system: System::new(),
            primed: false,
            nvml_probed: false,
            nvml: None,
        }
    }
}

impl Default for SystemSampler {
    fn default() -> SystemSampler {
        SystemSampler::new()
    }
}

// Manual because `Nvml` carries no `Debug` implementation.
impl fmt::Debug for SystemSampler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemSampler")
            .field("primed", &self.primed)
            .field("nvml_probed", &self.nvml_probed)
            .field("nvml", &self.nvml.is_some())
            .finish_non_exhaustive()
    }
}

/// The `GET /admin/system` route: bearer-authed, reports a live host-metrics
/// snapshot as JSON for the config UI's Settings > System cards.
///
/// `cpu` and `ram` are always present. `disk` reports the drive holding the
/// resolved artifact cache root and is absent when that root cannot be
/// resolved (no home directory) or no mounted disk holds it. `gpu` is absent
/// when no NVML-capable NVIDIA driver is present; NVML unavailability never
/// fails the request.
///
/// The route is compiled into every build: host metrics do not depend on the
/// `local` feature, and the NVML dependency probes its driver at runtime
/// rather than link time.
pub(crate) async fn admin_system(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SystemSnapshot>, GatewayError> {
    check_auth(&state, &headers).await?;
    let cache_dir = {
        let live = state.live.read().await;
        live.config.local().cache_dir().map(str::to_owned)
    };
    let metrics = Arc::clone(&state.metrics);
    let snapshot = tokio::task::spawn_blocking(move || {
        let cache_root = disk_target(cache_dir.as_deref());
        sample(&metrics, cache_root.as_deref())
    })
    .await
    .map_err(GatewayError::system_metrics)?;
    Ok(Json(snapshot))
}

/// Takes one snapshot, priming the sampler on the first call.
fn sample(sampler: &Mutex<SystemSampler>, cache_root: Option<&Path>) -> SystemSnapshot {
    // A poisoned lock only means a previous sample panicked mid-refresh; the
    // sampler holds no invariant a fresh refresh does not restore.
    let mut guard = sampler.lock().unwrap_or_else(PoisonError::into_inner);
    let refresh = RefreshKind::nothing()
        .with_cpu(CpuRefreshKind::nothing().with_cpu_usage().with_frequency())
        .with_memory(MemoryRefreshKind::nothing().with_ram());
    guard.system.refresh_specifics(refresh);
    if !guard.primed {
        // Utilization is the delta between two readings, so the very first
        // sample waits out sysinfo's minimum update interval and reads again;
        // later requests are spaced by the UI's poll interval.
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        guard.system.refresh_specifics(refresh);
        guard.primed = true;
    }
    if !guard.nvml_probed {
        // A failed init means no NVIDIA driver on this machine; the endpoint
        // degrades to an absent `gpu` field by design, so the error carries
        // no information worth returning.
        guard.nvml = Nvml::init().ok();
        guard.nvml_probed = true;
    }
    let cpu = CpuMetrics {
        frequency_mhz: guard
            .system
            .cpus()
            .first()
            .map_or(0, sysinfo::Cpu::frequency),
        logical_cores: guard.system.cpus().len(),
        physical_cores: System::physical_core_count(),
        utilization_percent: guard.system.global_cpu_usage(),
    };
    let ram = RamMetrics {
        used_bytes: guard.system.used_memory(),
        total_bytes: guard.system.total_memory(),
    };
    SystemSnapshot {
        cpu,
        ram,
        disk: cache_root.and_then(disk_metrics),
        gpu: guard.nvml.as_ref().and_then(gpu_metrics),
    }
}

/// The path whose drive the `disk` field reports: the resolved artifact
/// cache root, or `None` when no home directory makes resolution possible
/// (the card is then absent rather than an error).
fn disk_target(cache_dir: Option<&str>) -> Option<PathBuf> {
    #[cfg(feature = "local")]
    {
        // The local crate owns cache-root resolution, so the reported drive
        // is exactly the one the artifact store writes to.
        crate::local::resolve_cache_root(cache_dir).ok()
    }
    #[cfg(not(feature = "local"))]
    {
        // Headless builds compile without the local crate, so its defaulting
        // rule is mirrored here: the configured dir with `~` expanded, else
        // `~/.promptforge` under the platform home variable.
        let home = || {
            #[cfg(windows)]
            let value = std::env::var_os("USERPROFILE");
            #[cfg(not(windows))]
            let value = std::env::var_os("HOME");
            value.filter(|v| !v.is_empty()).map(PathBuf::from)
        };
        match cache_dir {
            Some("~") => home(),
            Some(dir) if !dir.is_empty() => {
                match dir.strip_prefix("~/").or_else(|| dir.strip_prefix("~\\")) {
                    Some(rest) => home().map(|h| h.join(rest)),
                    None => Some(PathBuf::from(dir)),
                }
            }
            _ => home().map(|h| h.join(".promptforge")),
        }
    }
}

/// Usage of the mounted disk whose mount point is the deepest prefix of
/// `cache_root`, or `None` when no mounted disk holds the path.
fn disk_metrics(cache_root: &Path) -> Option<DiskMetrics> {
    let disks = Disks::new_with_refreshed_list();
    let disk = disks
        .list()
        .iter()
        .filter(|disk| cache_root.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().components().count())?;
    Some(DiskMetrics {
        cache_dir: cache_root.display().to_string(),
        used_bytes: disk.total_space().saturating_sub(disk.available_space()),
        total_bytes: disk.total_space(),
    })
}

/// Name and VRAM of NVML device 0, or `None` when the device or any reading
/// is unavailable - the UI hides the GPU card rather than erroring, so a
/// partial reading carries no information worth returning.
fn gpu_metrics(nvml: &Nvml) -> Option<GpuMetrics> {
    let device = nvml.device_by_index(0).ok()?;
    let memory = device.memory_info().ok()?;
    Some(GpuMetrics {
        name: device.name().ok()?,
        vram_used_bytes: memory.used,
        vram_total_bytes: memory.total,
    })
}

#[cfg(test)]
mod tests {
    use promptforge_gateway_config::Config;

    use crate::test_support::serve;

    /// A minimal profile rooting the artifact cache at `cache_dir`.
    fn system_config(cache_dir: &std::path::Path) -> Config {
        Config::from_toml_str(&format!(
            r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[local]
cache_dir = '{cache_dir}'
"#,
            cache_dir = cache_dir.display(),
        ))
        .expect("the fixture profile parses")
    }

    #[tokio::test]
    async fn admin_system_reports_plausible_cpu_ram_and_disk() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let addr = serve(system_config(temp.path())).await;
        let response = reqwest::Client::new()
            .get(format!("http://{addr}/admin/system"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("a JSON body");

        let cpu = &body["cpu"];
        assert!(
            cpu["logical_cores"].as_u64().expect("logical_cores") > 0,
            "a running host has at least one logical core"
        );
        assert!(
            cpu["utilization_percent"].as_f64().is_some(),
            "utilization is always a number"
        );
        assert!(
            cpu["frequency_mhz"].is_u64(),
            "frequency is always a number, 0 when unknown"
        );

        let total_ram = body["ram"]["total_bytes"].as_u64().expect("ram total");
        let used_ram = body["ram"]["used_bytes"].as_u64().expect("ram used");
        assert!(total_ram > 0, "a running host has RAM installed");
        assert!(
            used_ram > 0 && used_ram <= total_ram,
            "used RAM is nonzero and within the installed total"
        );

        // The tempdir cache root sits on a mounted drive, so the disk card
        // resolves on every platform the suite runs on.
        let total_disk = body["disk"]["total_bytes"].as_u64().expect("disk total");
        let used_disk = body["disk"]["used_bytes"].as_u64().expect("disk used");
        assert!(total_disk > 0, "the cache drive has a capacity");
        assert!(used_disk <= total_disk, "usage cannot exceed capacity");

        // GPU is genuinely optional: absent on hosts without an NVIDIA
        // driver (CI), present with a name and a nonzero VRAM total where
        // NVML loads. The endpoint must succeed either way.
        if let Some(gpu) = body.get("gpu") {
            assert!(
                gpu["name"].as_str().is_some_and(|name| !name.is_empty()),
                "a reported GPU carries its device name"
            );
            assert!(
                gpu["vram_total_bytes"].as_u64().expect("vram total") > 0,
                "a reported GPU has VRAM"
            );
        }
    }

    #[tokio::test]
    async fn admin_system_requires_bearer_auth() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let addr = serve(system_config(temp.path())).await;
        let http = reqwest::Client::new();

        let unauthenticated = http
            .get(format!("http://{addr}/admin/system"))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            unauthenticated.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "a request without a bearer token is refused"
        );

        let wrong_key = http
            .get(format!("http://{addr}/admin/system"))
            .bearer_auth("wrong-token")
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            wrong_key.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "a request with the wrong bearer token is refused"
        );
    }
}
