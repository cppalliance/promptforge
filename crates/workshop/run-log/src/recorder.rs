//! The recorder that writes the Harness's runs to a run log.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use harness::record::{
    Record, RecorderError, RecorderFuture, RunId, RunMeta, RunOutcome, RunRecorder,
};
use tokio::sync::Mutex;

use crate::{LogError, RunLog};

/// The file under the state directory the run log is stored in.
const RUNS_FILE: &str = "runs.db";

/// The Turso [`RunLog`] a Host keeps every run in, shared by the
/// [`AgentRecorder`] of each launch.
///
/// The log is one file, `runs.db`, under the state directory the recorder
/// is built with. Building the recorder touches nothing. The first call
/// creates the directory and opens the file, and an open that fails leaves
/// the recorder unopened, so the next call tries again. Every call takes
/// the recorder's own lock, so runs that overlap write one at a time.
///
/// The Harness writes through the handle [`for_agent`](Self::for_agent)
/// returns, which names the launched agent in the row of each run it
/// begins. A failed write comes back as a [`RecorderError`] whose cause is
/// the log's [`LogError`]. The Harness stops the run that hit it.
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

    /// The recorder for one launch of `agent`: each run it begins is
    /// written to this log with `agent` in its row. Building it touches
    /// nothing.
    #[must_use]
    pub fn for_agent(self: &Arc<Self>, agent: impl Into<String>) -> AgentRecorder {
        AgentRecorder {
            recorder: Arc::clone(self),
            agent: agent.into(),
        }
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

/// A [`RunRecorder`] for one launch: it writes to its [`TursoRecorder`]'s
/// log and fills the `agent` column of each run it begins with the agent
/// the Host launched.
#[derive(Debug)]
pub struct AgentRecorder {
    /// The log every call writes to.
    recorder: Arc<TursoRecorder>,
    /// The launched agent, written at `begin_run`.
    agent: String,
}

impl RunRecorder for AgentRecorder {
    fn begin_run(&self, meta: RunMeta) -> RecorderFuture<'_, RunId> {
        Box::pin(async move {
            let mut slot = self.recorder.log.lock().await;
            let log = self
                .recorder
                .open(&mut slot)
                .await
                .map_err(RecorderError::new)?;
            log.begin_run(meta, &self.agent)
                .await
                .map_err(RecorderError::new)
        })
    }

    fn append(&self, run: RunId, record: Record) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            let mut slot = self.recorder.log.lock().await;
            let log = self
                .recorder
                .open(&mut slot)
                .await
                .map_err(RecorderError::new)?;
            log.append(run, record)
                .await
                .map(drop)
                .map_err(RecorderError::new)
        })
    }

    fn end_run(&self, run: RunId, outcome: RunOutcome) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            let mut slot = self.recorder.log.lock().await;
            let log = self
                .recorder
                .open(&mut slot)
                .await
                .map_err(RecorderError::new)?;
            log.end_run(run, outcome).await.map_err(RecorderError::new)
        })
    }
}
