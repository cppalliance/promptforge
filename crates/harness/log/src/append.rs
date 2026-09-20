//! The write path: open a log, begin a run, append records, end the run.

use std::fmt;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::LogError;
use crate::record::{Record, RunId, RunMeta, RunOutcome, Seq};
use crate::schema;

/// An open run log: one connection to one Turso database holding the
/// `runs` and `records` tables.
///
/// Every method takes `&mut self`: the effect loop is the log's one
/// writer, and exclusive access is what lets `append` read the next `seq`
/// and insert under it without a transaction. A host that needs the log
/// from several tasks wraps it in its own serialization.
pub struct RunLog {
    conn: turso::Connection,
}

impl fmt::Debug for RunLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunLog").finish_non_exhaustive()
    }
}

impl RunLog {
    /// Opens the log at `path`, creating the file and the schema when
    /// absent. The parent directory must already exist: this creates
    /// exactly the file, never a directory.
    ///
    /// # Errors
    /// Returns [`LogError::Io`] when `path` is not UTF-8 (Turso addresses
    /// databases by string) and [`LogError::Database`] when the engine
    /// cannot open the file or apply the schema.
    pub async fn open(path: &Path) -> Result<Self, LogError> {
        let path = path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "run log path must be utf-8")
        })?;
        Self::open_str(path).await
    }

    /// Opens a log that lives only as long as this value: for tests and
    /// for hosts that keep no history.
    ///
    /// # Errors
    /// Returns [`LogError::Database`] when the engine cannot build the
    /// database or apply the schema.
    pub async fn in_memory() -> Result<Self, LogError> {
        Self::open_str(":memory:").await
    }

    async fn open_str(path: &str) -> Result<Self, LogError> {
        let database = turso::Builder::new_local(path).build().await?;
        let conn = database.connect()?;
        conn.execute_batch(schema::SCHEMA).await?;
        Ok(Self { conn })
    }

    /// Opens a run and returns its identity. `meta.started_at` is stored as
    /// given; the log stamps nothing at begin.
    ///
    /// # Errors
    /// Returns [`LogError::Database`] when the row cannot be written.
    pub async fn begin_run(&mut self, meta: RunMeta) -> Result<RunId, LogError> {
        self.conn
            .execute(
                schema::INSERT_RUN,
                (
                    meta.session_id,
                    meta.agent,
                    meta.prompt_hash,
                    signed(meta.seed),
                    i64::from(meta.flags),
                    meta.started_at,
                ),
            )
            .await?;
        Ok(RunId::from_raw(self.conn.last_insert_rowid()))
    }

    /// Appends one record to `run` and returns the position the log
    /// assigned it: one past the run's last, `0` for the first. The record
    /// is stamped with the wall clock at append.
    ///
    /// # Errors
    /// Returns [`LogError::UnknownRun`] when `run` was never begun here,
    /// [`LogError::RunEnded`] when it has ended, [`LogError::Payload`]
    /// when the payload does not serialize, and [`LogError::Database`]
    /// when the engine refuses the write.
    pub async fn append(&mut self, run: RunId, record: Record) -> Result<Seq, LogError> {
        self.require_open(run).await?;
        let seq = self.next_seq(run).await?;
        let payload = serde_json::to_string(&record.payload)?;
        self.conn
            .execute(
                schema::INSERT_RECORD,
                (
                    run.get(),
                    signed(seq.get()),
                    record.task_id,
                    i64::from(record.task_seq),
                    record.kind.as_str(),
                    record.effect_id.map(signed),
                    payload,
                    now_ms(),
                ),
            )
            .await?;
        Ok(seq)
    }

    /// Closes `run` with `outcome`, stamping `ended_at` with the wall
    /// clock. A run closes exactly once.
    ///
    /// # Errors
    /// Returns [`LogError::UnknownRun`] when `run` was never begun here,
    /// [`LogError::RunEnded`] when it has already closed, and
    /// [`LogError::Database`] when the engine refuses the write.
    pub async fn end_run(&mut self, run: RunId, outcome: RunOutcome) -> Result<(), LogError> {
        self.require_open(run).await?;
        let kind = outcome.as_str();
        let (final_text, error_kind, error_message) = match outcome {
            RunOutcome::Completed { final_text } => (Some(final_text), None, None),
            RunOutcome::Failed { kind, message } => (None, Some(kind), Some(message)),
            RunOutcome::Cancelled => (None, None, None),
        };
        let changed = self
            .conn
            .execute(
                schema::UPDATE_RUN_ENDED,
                (
                    run.get(),
                    now_ms(),
                    kind,
                    final_text,
                    error_kind,
                    error_message,
                ),
            )
            .await?;
        // `require_open` saw the row open a moment ago and this value has
        // the only connection, so zero changes cannot happen; the check
        // keeps the exactly-once rule honest against a shared file.
        if changed == 0 {
            return Err(LogError::RunEnded(run));
        }
        Ok(())
    }

    /// The connection, for the read side.
    pub(crate) const fn conn(&self) -> &turso::Connection {
        &self.conn
    }

    /// Fails unless `run` exists and has not ended.
    async fn require_open(&self, run: RunId) -> Result<(), LogError> {
        let mut rows = self
            .conn
            .query(schema::SELECT_RUN_ENDED_AT, (run.get(),))
            .await?;
        match rows.next().await? {
            None => Err(LogError::UnknownRun(run)),
            Some(row) => match row.get::<Option<i64>>(0)? {
                None => Ok(()),
                Some(_) => Err(LogError::RunEnded(run)),
            },
        }
    }

    /// One past `run`'s last `seq`; `0` when it has no records.
    async fn next_seq(&self, run: RunId) -> Result<Seq, LogError> {
        let mut rows = self
            .conn
            .query(schema::SELECT_NEXT_SEQ, (run.get(),))
            .await?;
        let next = match rows.next().await? {
            Some(row) => row.get::<i64>(0)?,
            None => 0,
        };
        u64::try_from(next)
            .map(Seq::from_raw)
            .map_err(|_| LogError::Corrupt(format!("expected a non-negative seq, found {next}")))
    }
}

/// A `u64` as the `i64` SQLite stores: the same bits, so the round trip
/// through [`unsigned`] is lossless. Values past `i64::MAX` read as
/// negative in raw SQL, which nothing here does.
pub(crate) const fn signed(value: u64) -> i64 {
    i64::from_le_bytes(value.to_le_bytes())
}

/// The inverse of [`signed`].
pub(crate) const fn unsigned(value: i64) -> u64 {
    u64::from_le_bytes(value.to_le_bytes())
}

/// The wall clock as UTC milliseconds since the Unix epoch; `0` on a clock
/// set before the epoch, which is a display column's problem, not the log's.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}
