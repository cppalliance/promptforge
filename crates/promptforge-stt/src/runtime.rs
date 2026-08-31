//! Active-profile STT artifact provisioning and engine lifecycle.

use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use promptforge_gateway_config::{Config, SttRole, WorkshopSttConfig};
use promptforge_gateway_local::artifacts::ArtifactStore;
use promptforge_progress::ProgressHandle;
use promptforge_transcribe::{EngineConfig, SttEngine, SttSlot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadedModelRole {
    Interim,
    Final,
}

#[derive(Debug, Clone, Default)]
struct LoadedNames {
    interim: Option<String>,
    final_model: Option<String>,
}

/// Shared active STT state used by both gateway HTTP surfaces.
///
/// Clones observe the same engine and loaded-model names across profile
/// switches.
#[derive(Debug, Clone)]
pub struct SttState {
    slot: SttSlot,
    names: Arc<RwLock<LoadedNames>>,
    changes: tokio::sync::watch::Sender<u64>,
}

impl Default for SttState {
    fn default() -> Self {
        let (changes, _receiver) = tokio::sync::watch::channel(0);
        Self {
            slot: SttSlot::default(),
            names: Arc::new(RwLock::new(LoadedNames::default())),
            changes,
        }
    }
}

impl SttState {
    pub(crate) fn engine(&self) -> Option<Arc<SttEngine>> {
        self.slot.engine()
    }

    /// Returns whether an STT engine is active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.slot.is_active()
    }

    pub(crate) fn select(&self, name: &str) -> Option<(Arc<SttEngine>, LoadedModelRole)> {
        let role = {
            let names = self.names.read().unwrap_or_else(PoisonError::into_inner);
            if names.interim.as_deref() == Some(name) {
                Some(LoadedModelRole::Interim)
            } else if names.final_model.as_deref() == Some(name) {
                Some(LoadedModelRole::Final)
            } else {
                None
            }
        }?;
        self.slot.engine().map(|engine| (engine, role))
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn activate(&self, engine: SttEngine, interim: String, final_model: Option<String>) {
        self.slot.activate(engine);
        *self.names.write().unwrap_or_else(PoisonError::into_inner) = LoadedNames {
            interim: Some(interim),
            final_model,
        };
        self.changes.send_modify(|generation| *generation += 1);
    }

    fn take_engine(&self) -> Option<Arc<SttEngine>> {
        *self.names.write().unwrap_or_else(PoisonError::into_inner) = LoadedNames::default();
        let engine = self.slot.take();
        self.changes.send_modify(|generation| *generation += 1);
        engine
    }
}

/// Gateway-owned runtime for the selected profile's STT pair.
///
/// Dropping the runtime unloads its engine and releases the model memory.
#[derive(Debug)]
pub struct SttRuntime {
    state: SttState,
    active: bool,
}

impl SttRuntime {
    /// Creates an inactive runtime over `state`.
    #[must_use]
    pub fn empty(state: SttState) -> SttRuntime {
        unload_engine(&state);
        SttRuntime {
            state,
            active: false,
        }
    }

    /// Provisions the selected STT pair and loads its engine.
    ///
    /// A profile with no STT entries returns an inactive runtime. An
    /// interim-only profile loads one worker and preserves the streaming
    /// endpoint's degraded stop fallback.
    ///
    /// # Errors
    /// Returns [`SttRuntimeError::Artifact`] when download or digest
    /// verification fails, [`SttRuntimeError::MissingInterim`] when a final
    /// model has no interim partner, or [`SttRuntimeError::Engine`] when
    /// whisper cannot load the provisioned pair.
    pub fn start(
        config: &Config,
        state: SttState,
        progress: Option<&ProgressHandle>,
    ) -> Result<SttRuntime, SttRuntimeError> {
        if config.stt_models().is_empty() {
            return Ok(Self::empty(state));
        }
        let cache = promptforge_gateway_local::resolve_cache_root(config.local().cache_dir())
            .map_err(SttRuntimeError::Store)?;
        let store = ArtifactStore::new(cache).map_err(SttRuntimeError::Store)?;
        let models = provision_models(config, &store, progress)?;
        let Some((interim_name, interim_path)) = models.interim else {
            return Err(SttRuntimeError::MissingInterim);
        };
        let capture = config
            .workshop()
            .and_then(promptforge_gateway_config::WorkshopConfig::stt)
            .cloned()
            .unwrap_or_default();
        let engine_config = engine_config(&capture, interim_path, models.final_model.as_ref());
        let engine = SttEngine::new_with_progress(
            &engine_config,
            progress.map(|handle| handle.child("engine", 1.0)),
        )
        .map_err(SttRuntimeError::Engine)?;
        let final_name = models.final_model.map(|(name, _)| name);
        state.activate(engine, interim_name, final_name);
        Ok(SttRuntime {
            state,
            active: true,
        })
    }

    /// Returns shared state for HTTP routes.
    #[must_use]
    pub fn state(&self) -> SttState {
        self.state.clone()
    }

    /// Unloads the active engine immediately.
    pub fn shutdown(mut self) {
        self.clear();
    }

    fn clear(&mut self) {
        if self.active {
            unload_engine(&self.state);
            self.active = false;
        }
    }
}

impl Drop for SttRuntime {
    fn drop(&mut self) {
        self.clear();
    }
}

fn unload_engine(state: &SttState) {
    if let Some(engine) = state.take_engine() {
        while Arc::strong_count(&engine) > 1 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        drop(engine);
    }
}

#[derive(Debug, Default)]
struct ProvisionedModels {
    interim: Option<(String, PathBuf)>,
    final_model: Option<(String, PathBuf)>,
}

fn provision_models(
    config: &Config,
    store: &ArtifactStore,
    progress: Option<&ProgressHandle>,
) -> Result<ProvisionedModels, SttRuntimeError> {
    let mut provisioned = ProvisionedModels::default();
    for model in config.stt_models() {
        let model_progress = progress.map(|handle| handle.child(model.name(), 4.0));
        let path = store
            .ensure_model_with_progress(model.source(), model.sha256(), model_progress.as_ref())
            .map_err(|source| SttRuntimeError::Artifact {
                model: model.name().to_owned(),
                source,
            })?;
        match model.role() {
            SttRole::Interim => provisioned.interim = Some((model.name().to_owned(), path)),
            SttRole::Final => provisioned.final_model = Some((model.name().to_owned(), path)),
            _ => {
                return Err(SttRuntimeError::UnsupportedRole {
                    model: model.name().to_owned(),
                });
            }
        }
    }
    Ok(provisioned)
}

fn engine_config(
    capture: &WorkshopSttConfig,
    interim_model: PathBuf,
    final_model: Option<&(String, PathBuf)>,
) -> EngineConfig {
    EngineConfig {
        interim_model,
        final_model: final_model.map(|(_, path)| path.clone()),
        vocabulary: capture.vocabulary().to_vec(),
        window_seconds: capture.window_seconds(),
        interval_ms: capture.interval_ms(),
    }
}

/// An STT runtime startup failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SttRuntimeError {
    /// The artifact store could not be opened.
    #[non_exhaustive]
    #[error("open STT artifact store")]
    Store(#[source] promptforge_gateway_local::LocalError),

    /// One model could not be provisioned.
    #[non_exhaustive]
    #[error("provision STT model {model}")]
    Artifact {
        /// Catalog name of the model that failed.
        model: String,
        /// Artifact download, confinement, or verification failure.
        #[source]
        source: promptforge_gateway_local::LocalError,
    },

    /// A final model was selected without its required interim partner.
    #[error("final STT model requires an interim model")]
    MissingInterim,

    /// A future role reached a runtime that does not implement it.
    #[non_exhaustive]
    #[error("STT model {model} has an unsupported role")]
    UnsupportedRole {
        /// Catalog name carrying the unsupported role.
        model: String,
    },

    /// The provisioned whisper pair could not be loaded.
    #[non_exhaustive]
    #[error("load STT engine")]
    Engine(#[source] promptforge_transcribe::TranscribeError),
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use sha2::{Digest, Sha256};

    use super::*;

    fn selected(source: &str, sha256: Option<&str>) -> Config {
        let pin = sha256.map_or_else(String::new, |pin| format!("sha256 = \"{pin}\"\n"));
        let catalog = Config::from_toml_str(&format!(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
             [workshop]\n\
             [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = {source:?}\n\
             {pin}vram_gb = 1.0\n\
             [[profile]]\nname = \"work\"\nmodels = [\"speech\"]\n"
        ))
        .expect("catalog parses");
        catalog
            .select_profile(&promptforge_gateway_config::ProfileName::parse("work").expect("name"))
            .expect("profile selects")
    }

    #[test]
    fn a_pinned_model_rejects_the_wrong_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"model bytes").expect("fixture writes");
        let wrong = "0".repeat(64);
        let config = selected(&model.display().to_string(), Some(&wrong));
        let store = ArtifactStore::new(dir.path().join("cache")).expect("store builds");
        let error = provision_models(&config, &store, None).expect_err("bad pin must fail");
        assert!(matches!(error, SttRuntimeError::Artifact { .. }));
    }

    #[test]
    fn an_unpinned_local_model_provisions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"model bytes").expect("fixture writes");
        let config = selected(&model.display().to_string(), None);
        let store = ArtifactStore::new(dir.path().join("cache")).expect("store builds");
        let provisioned = provision_models(&config, &store, None).expect("unpinned path works");
        assert_eq!(
            provisioned.interim.as_ref().map(|(_, path)| path),
            Some(&model)
        );
    }

    #[test]
    fn a_pinned_model_accepts_the_matching_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"model bytes").expect("fixture writes");
        let mut pin = String::with_capacity(64);
        for byte in Sha256::digest(b"model bytes") {
            write!(&mut pin, "{byte:02x}").expect("writing to String is infallible");
        }
        let config = selected(&model.display().to_string(), Some(&pin));
        let store = ArtifactStore::new(dir.path().join("cache")).expect("store builds");
        let provisioned = provision_models(&config, &store, None).expect("matching pin works");
        assert_eq!(
            provisioned.interim.as_ref().map(|(_, path)| path),
            Some(&model)
        );
    }

    #[test]
    fn an_empty_runtime_clears_a_previously_loaded_name_table() {
        let state = SttState::default();
        *state.names.write().unwrap_or_else(PoisonError::into_inner) = LoadedNames {
            interim: Some("old".to_owned()),
            final_model: None,
        };
        let runtime = SttRuntime::empty(state.clone());
        assert!(state.select("old").is_none());
        runtime.shutdown();
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn switch_in_loads_and_switch_out_fully_unloads_the_engine() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = promptforge_transcribe::fixtures::require_model()
            .display()
            .to_string()
            .replace('\\', "/");
        let cache = dir.path().display().to_string().replace('\\', "/");
        let catalog = Config::from_toml_str(&format!(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
             [local]\ncache_dir = {cache:?}\n\
             [workshop]\n\
             [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = {source:?}\n\
             vram_gb = 1.0\n\
             [[profile]]\nname = \"work\"\nmodels = [\"speech\"]\n"
        ))
        .expect("catalog parses");
        let config = catalog
            .select_profile(&promptforge_gateway_config::ProfileName::parse("work").expect("name"))
            .expect("profile selects");
        let state = SttState::default();
        let runtime = SttRuntime::start(&config, state.clone(), None).expect("engine loads");
        assert!(state.is_active(), "switch-in activates the engine");
        assert!(state.select("speech").is_some(), "loaded name selects");
        runtime.shutdown();
        assert!(!state.is_active(), "switch-out drops the engine");
        assert!(state.select("speech").is_none(), "switch-out clears names");
    }
}
