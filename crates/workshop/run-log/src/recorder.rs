//! The recorder that writes the Harness's runs to a run log.

use std::path::{Path, PathBuf};

use harness::record::{
    Record, RecorderError, RecorderFuture, RunId, RunMeta, RunOutcome, RunRecorder,
};
use tokio::sync::Mutex;

use crate::{LogError, RunLog};

/// The file under the state directory the run log is stored in.
const RUNS_FILE: &str = "runs.db";

/// A [`RunRecorder`] that keeps every run in a Turso [`RunLog`].
///
/// The log is one file, `runs.db`, under the state directory the recorder
/// is built with. Building the recorder touches nothing. The first call
/// creates the directory and opens the file, and an open that fails leaves
/// the recorder unopened, so the next call tries again. Every call takes
/// the recorder's own lock, so runs that overlap write one at a time.
///
/// A failed write comes back as a [`RecorderError`] whose cause is the
/// log's [`LogError`]. The Harness stops the run that hit it.
#[derive(Debug)]
pub struct TursoRecorder {
    /// The directory the database file sits in.
    dir: PathBuf,
    /// The database file.
    path: PathBuf,
    /// The open log, `None` until a call has opened it.
    log: Mutex<Option<RunLog>>,
}

impl TursoRecorder {
    /// A recorder that keeps its log in `runs.db` under `state_dir`. The
    /// directory and the file are created by the first call, not here.
    #[must_use]
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        let dir = state_dir.into();
        let path = dir.join(RUNS_FILE);
        Self {
            dir,
            path,
            log: Mutex::new(None),
        }
    }

    /// The database file the recorder writes. Open it with
    /// [`RunLog::open`] to read a run back once the recorder is gone.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The open log in `slot`, opening it first when no call has yet.
    async fn open<'a>(&self, slot: &'a mut Option<RunLog>) -> Result<&'a mut RunLog, LogError> {
        if let Some(log) = slot {
            Ok(log)
        } else {
            tokio::fs::create_dir_all(&self.dir).await?;
            let log = RunLog::open(&self.path).await?;
            Ok(slot.insert(log))
        }
    }
}

impl RunRecorder for TursoRecorder {
    fn begin_run(&self, meta: RunMeta) -> RecorderFuture<'_, RunId> {
        Box::pin(async move {
            let mut slot = self.log.lock().await;
            let log = self.open(&mut slot).await.map_err(RecorderError::new)?;
            log.begin_run(meta).await.map_err(RecorderError::new)
        })
    }

    fn append(&self, run: RunId, record: Record) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            let mut slot = self.log.lock().await;
            let log = self.open(&mut slot).await.map_err(RecorderError::new)?;
            log.append(run, record)
                .await
                .map(drop)
                .map_err(RecorderError::new)
        })
    }

    fn end_run(&self, run: RunId, outcome: RunOutcome) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            let mut slot = self.log.lock().await;
            let log = self.open(&mut slot).await.map_err(RecorderError::new)?;
            log.end_run(run, outcome).await.map_err(RecorderError::new)
        })
    }
}
