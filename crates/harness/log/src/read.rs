//! The read side: one run's row and its records, whole or sliced by kind
//! and task.

use crate::append::{RunLog, unsigned};
use crate::error::LogError;
use crate::record::{
    Record, RecordFilter, RecordKind, RunId, RunMeta, RunOutcome, RunRow, Seq, StoredRecord,
};
use crate::schema;

impl RunLog {
    /// Reads `run`'s row.
    ///
    /// # Errors
    /// Returns [`LogError::UnknownRun`] when `run` was never begun here,
    /// [`LogError::Corrupt`] when the row does not fit the schema, and
    /// [`LogError::Database`] when the engine cannot read it.
    pub async fn run(&self, run: RunId) -> Result<RunRow, LogError> {
        let mut rows = self.conn().query(schema::SELECT_RUN, (run.get(),)).await?;
        let Some(row) = rows.next().await? else {
            return Err(LogError::UnknownRun(run));
        };
        let meta = RunMeta {
            session_id: row.get(0)?,
            agent: row.get(1)?,
            prompt_hash: row.get(2)?,
            seed: unsigned(row.get(3)?),
            flags: to_u32(row.get(4)?, "flags")?,
            started_at: row.get(5)?,
        };
        let ended_at: Option<i64> = row.get(6)?;
        let outcome = match row.get::<Option<String>>(7)? {
            None => None,
            Some(kind) => Some(parse_outcome(
                &kind,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
            )?),
        };
        Ok(RunRow {
            id: run,
            meta,
            ended_at,
            outcome,
        })
    }

    /// Reads the records of `run` that `filter` selects, in the order
    /// [`RecordFilter`] documents. A run with nothing selected reads as
    /// empty; an unknown run is refused.
    ///
    /// # Errors
    /// Returns [`LogError::UnknownRun`] when `run` was never begun here,
    /// [`LogError::Corrupt`] when a row does not fit the schema,
    /// [`LogError::Payload`] when a payload does not parse, and
    /// [`LogError::Database`] when the engine cannot read.
    pub async fn records(
        &self,
        run: RunId,
        filter: RecordFilter,
    ) -> Result<Vec<StoredRecord>, LogError> {
        self.run(run).await?;
        let kind = filter.kind.map(RecordKind::as_str);
        let limit = filter.last.map_or(-1, i64::from);
        let mut rows = match filter.task {
            None => {
                self.conn()
                    .query(schema::SELECT_RECORDS, (run.get(), kind, limit))
                    .await?
            }
            Some(task) => {
                self.conn()
                    .query(schema::SELECT_TASK_RECORDS, (run.get(), task, kind, limit))
                    .await?
            }
        };
        // The queries read newest first so `LIMIT` keeps the final
        // records; oldest first is the order callers expect.
        let mut records = Vec::new();
        while let Some(row) = rows.next().await? {
            let kind: String = row.get(3)?;
            let kind = RecordKind::parse(&kind).ok_or_else(|| {
                LogError::Corrupt(format!(
                    "expected kind effect, answer, or event, found {kind:?}"
                ))
            })?;
            let payload: String = row.get(5)?;
            records.push(StoredRecord {
                seq: Seq::from_raw(unsigned(row.get(0)?)),
                at: row.get(6)?,
                record: Record {
                    task_id: row.get(1)?,
                    task_seq: to_u32(row.get(2)?, "task_seq")?,
                    kind,
                    effect_id: row.get::<Option<i64>>(4)?.map(unsigned),
                    payload: serde_json::from_str(&payload)?,
                },
            });
        }
        records.reverse();
        Ok(records)
    }

    /// The `Event` payloads of one task (named by its rendered path), in
    /// `task_seq` order: what the `TaskEvents` performer hands back to the
    /// engine. `last` keeps only the final `n`. A task that never logged
    /// reads as empty.
    ///
    /// # Errors
    /// Returns [`LogError::UnknownRun`] when `run` was never begun here,
    /// [`LogError::Corrupt`] when a row does not fit the schema,
    /// [`LogError::Payload`] when a payload does not parse, and
    /// [`LogError::Database`] when the engine cannot read.
    pub async fn events_for_task(
        &self,
        run: RunId,
        task: &str,
        last: Option<u32>,
    ) -> Result<Vec<serde_json::Value>, LogError> {
        let filter = RecordFilter {
            kind: Some(RecordKind::Event),
            task: Some(task.to_owned()),
            last,
        };
        let records = self.records(run, filter).await?;
        Ok(records
            .into_iter()
            .map(|stored| stored.record.payload)
            .collect())
    }

    /// Every `event` record of `run` in `seq` order, the loop's order:
    /// what a session view renders and what a reconnecting client replays.
    ///
    /// # Errors
    /// Returns [`LogError::UnknownRun`] when `run` was never begun here,
    /// [`LogError::Corrupt`] when a row does not fit the schema,
    /// [`LogError::Payload`] when a payload does not parse, and
    /// [`LogError::Database`] when the engine cannot read.
    pub async fn transcript(&self, run: RunId) -> Result<Vec<StoredRecord>, LogError> {
        let filter = RecordFilter {
            kind: Some(RecordKind::Event),
            task: None,
            last: None,
        };
        self.records(run, filter).await
    }
}

/// A stored `u32` column, refusing anything outside the type.
fn to_u32(value: i64, column: &str) -> Result<u32, LogError> {
    u32::try_from(value)
        .map_err(|_| LogError::Corrupt(format!("expected {column} to fit u32, found {value}")))
}

/// Rebuilds a [`RunOutcome`] from its four columns.
fn parse_outcome(
    kind: &str,
    final_text: Option<String>,
    error_kind: Option<String>,
    error_message: Option<String>,
) -> Result<RunOutcome, LogError> {
    match (kind, final_text, error_kind, error_message) {
        ("completed", Some(final_text), None, None) => Ok(RunOutcome::Completed { final_text }),
        ("failed", None, Some(kind), Some(message)) => Ok(RunOutcome::Failed { kind, message }),
        ("cancelled", None, None, None) => Ok(RunOutcome::Cancelled),
        (kind, final_text, error_kind, error_message) => Err(LogError::Corrupt(format!(
            "expected outcome columns to match {kind:?}, found final_text={final_text:?} \
             error_kind={error_kind:?} error_message={error_message:?}"
        ))),
    }
}
