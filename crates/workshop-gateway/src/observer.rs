//! The workshop's run event log: the [`Observer`] write side, the
//! [`EventLog`] read side, live broadcast fan-out, and versioned JSONL
//! persistence in one append-only type.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use serde::{Deserialize, Serialize};
use shared_promptforge_api::events::{
    CallMetrics, EventLog, RuntimeEvent, RuntimeEventKind, ToolCallEvent,
};
use shared_promptforge_api::observe::{Observation, Observer};
use tokio::sync::broadcast;

/// The `format` field of the header line that opens every persisted log.
const LOG_FORMAT: &str = "workshop-event-log";

/// The persisted-log version this module writes and reads.
///
/// Version 1 is one serde-compact [`RuntimeEvent`] JSON object per line,
/// behind the header line. Two event kinds are reserved for future
/// producers and stay out of the vocabulary until one exists: `plan`
/// (snapshot-replace semantics with a required `planId`) and the
/// five-status tool state (`pending` / `in_progress` / `completed` /
/// `failed` / `cancelled`). A version-1 reader rejects a line whose kind
/// it does not know, so shipping those kinds revisits this version.
const LOG_VERSION: u32 = 1;

/// Capacity of the broadcast channel behind
/// [`WorkshopObserver::subscribe`]. A receiver that falls further behind
/// misses the overwritten entries and recovers them by index through the
/// log itself, which retains every entry.
const BROADCAST_CAPACITY: usize = 256;

/// The versioned header line every persisted log begins with, so a reader
/// refuses a file this module does not speak instead of misparsing it.
#[derive(Debug, Serialize, Deserialize)]
struct Header {
    /// The format name; always [`LOG_FORMAT`] in files this module writes.
    format: String,
    /// The format version; this build speaks [`LOG_VERSION`].
    version: u32,
}

/// Everything behind the one lock. Entry order, file order, and broadcast
/// order agree because all three advance under the same write guard.
struct Inner {
    /// The append-only in-memory log; an index once valid stays valid.
    events: Vec<RuntimeEvent>,
    /// The persistence half, absent for a memory-only log.
    persist: Option<Persist>,
}

/// An open append handle to the persisted JSONL file, with its path
/// retained for failure messages.
struct Persist {
    /// Where the log persists, named in degradation warnings.
    path: PathBuf,
    /// The append handle; a boxed trait object so tests can inject a
    /// failing writer.
    writer: Box<dyn Write + Send + Sync>,
}

impl Persist {
    /// Creates the log file at `path` - truncating whatever was there -
    /// and writes the versioned header line.
    fn create(path: &Path) -> io::Result<Self> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(header_line()?.as_bytes())?;
        Ok(Self {
            path: path.to_path_buf(),
            writer: Box::new(file),
        })
    }
}

/// The workshop's append-only run event log.
///
/// One instance records one run's [`RuntimeEvent`]s. The [`Observer`]
/// content methods append (the write side), [`EventLog`] serves indexed
/// reads (the read side), and [`subscribe`](Self::subscribe) fans every
/// appended entry out live. Operational lifecycle observations
/// ([`Observer::observe`]) are deliberately not recorded: the runtime-event
/// vocabulary carries content events alone.
///
/// With a persist path, every entry also appends to a JSONL file as it
/// lands - one serde-compact event per line behind a versioned header
/// line - and [`load_from`](Self::load_from) replays such a file and
/// continues appending to it. Failures follow the crate's zone-two
/// posture: a persistence error is logged degradation that never loses
/// the in-memory entry and never panics, and a lock poisoned by a
/// panicking peer recovers the value rather than wedging the process.
///
/// Reports are synchronous and briefly hold the log's write lock across
/// one file append; callers on an async runtime reach a persisting log
/// through `spawn_blocking`.
pub struct WorkshopObserver {
    /// The log and its optional persistence, under one lock.
    inner: RwLock<Inner>,
    /// The live fan-out; entries are sent under the write guard, so
    /// receivers observe log order.
    sender: broadcast::Sender<RuntimeEvent>,
}

impl WorkshopObserver {
    /// Opens a fresh, empty log.
    ///
    /// With `Some(path)`, the file at `path` is created - truncating
    /// whatever was there - and receives the versioned header line at
    /// once; every event then appends one JSONL line as it lands.
    /// Resuming an existing file is [`load_from`](Self::load_from)'s job.
    /// With `None` the log is memory-only.
    ///
    /// # Errors
    /// Returns the underlying I/O error when the file cannot be created
    /// or the header line cannot be written.
    ///
    /// # Examples
    /// ```
    /// use shared_promptforge_api::events::EventLog;
    /// use shared_promptforge_api::observe::Observer;
    /// use workshop_gateway::WorkshopObserver;
    ///
    /// let log = WorkshopObserver::new(None)?;
    /// log.on_user_input("run", "chat", "hello");
    /// assert_eq!(log.len(), 1);
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn new(persist_path: Option<&Path>) -> io::Result<Self> {
        let persist = persist_path.map(Persist::create).transpose()?;
        Ok(Self::assemble(Vec::new(), persist))
    }

    /// Replays the persisted log at `path` and continues appending to it.
    ///
    /// The whole file is validated up front: the header line must carry
    /// this module's format and version, and every following line must
    /// parse as one [`RuntimeEvent`]. Strict on purpose - a line that
    /// does not parse is schema drift or corruption, and surfacing it
    /// beats replaying a lie. The replayed entries become the in-memory
    /// log, indexes matching the original run, and the file reopens for
    /// append behind the same header.
    ///
    /// # Errors
    /// Returns the underlying I/O error when the file cannot be read or
    /// reopened, and an [`io::ErrorKind::InvalidData`] error naming the
    /// offending line when the header is missing or alien or an event
    /// line does not parse.
    ///
    /// # Examples
    /// ```
    /// use shared_promptforge_api::events::EventLog;
    /// use shared_promptforge_api::observe::Observer;
    /// use workshop_gateway::WorkshopObserver;
    ///
    /// let dir = tempfile::TempDir::new()?;
    /// let path = dir.path().join("events.jsonl");
    /// let live = WorkshopObserver::new(Some(&path))?;
    /// live.on_user_input("run", "chat", "hello");
    /// drop(live);
    ///
    /// let restored = WorkshopObserver::load_from(&path)?;
    /// assert_eq!(restored.len(), 1);
    /// assert_eq!(restored.get(0).map(|event| event.content), Some("hello".to_owned()));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn load_from(path: &Path) -> io::Result<Self> {
        let events = replay(path)?;
        let writer = std::fs::OpenOptions::new().append(true).open(path)?;
        Ok(Self::assemble(
            events,
            Some(Persist {
                path: path.to_path_buf(),
                writer: Box::new(writer),
            }),
        ))
    }

    /// Subscribes to every entry appended from this call on.
    ///
    /// Entries arrive in log order, each sent after it is readable
    /// through [`EventLog`]. Earlier entries never replay here - read
    /// them by index instead - and a receiver that lags past the channel
    /// capacity misses the overwritten entries and recovers them the
    /// same way.
    ///
    /// # Examples
    /// ```
    /// use shared_promptforge_api::observe::Observer;
    /// use workshop_gateway::WorkshopObserver;
    ///
    /// let log = WorkshopObserver::new(None)?;
    /// let mut entries = log.subscribe();
    /// log.on_user_input("run", "chat", "hello");
    /// assert_eq!(entries.try_recv()?.content, "hello");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.sender.subscribe()
    }

    /// Assembles the shared state around replayed or empty `events`.
    fn assemble(events: Vec<RuntimeEvent>, persist: Option<Persist>) -> Self {
        Self {
            inner: RwLock::new(Inner { events, persist }),
            sender: broadcast::channel(BROADCAST_CAPACITY).0,
        }
    }

    /// Appends one event to memory, to the file when persisting, and to
    /// the broadcast, all under the one write guard so the three orders
    /// agree. A persistence failure is logged degradation (zone two): the
    /// in-memory entry lands regardless, and later appends keep trying.
    fn append(&self, event: RuntimeEvent) {
        // One serde-compact event is one JSONL line, the vocabulary's
        // documented persisted shape.
        let line = match serde_json::to_string(&event) {
            Ok(mut line) => {
                line.push('\n');
                Some(line)
            }
            Err(source) => {
                tracing::warn!(%source, "run event not persisted: serialization failed");
                None
            }
        };
        let mut inner = self.write();
        if let Some(persist) = inner.persist.as_mut()
            && let Some(line) = line.as_deref()
            && let Err(source) = persist.writer.write_all(line.as_bytes())
        {
            tracing::warn!(
                path = %persist.path.display(),
                %source,
                "run event not persisted: append failed"
            );
        }
        inner.events.push(event.clone());
        // A send without receivers is the channel's resting state, not a
        // fault; entries stay readable by index regardless.
        let _ = self.sender.send(event);
    }

    /// The read guard, recovering a lock poisoned by a panicking peer
    /// rather than wedging the process (the crate's zone-two policy).
    fn read(&self) -> RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The write guard; the same poison recovery as [`Self::read`].
    fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Builds a log around an arbitrary writer, for failure-injection
    /// tests.
    #[cfg(test)]
    fn with_writer_for_test(writer: impl Write + Send + Sync + 'static) -> Self {
        Self::assemble(
            Vec::new(),
            Some(Persist {
                path: PathBuf::from("<test>"),
                writer: Box::new(writer),
            }),
        )
    }
}

impl fmt::Debug for WorkshopObserver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.read();
        f.debug_struct("WorkshopObserver")
            .field("len", &inner.events.len())
            .field(
                "persist_path",
                &inner.persist.as_ref().map(|persist| persist.path.as_path()),
            )
            .finish_non_exhaustive()
    }
}

impl Observer for WorkshopObserver {
    /// Discards the operational lifecycle report: the run event log
    /// records content events alone, and lifecycle vocabulary
    /// deliberately has no [`RuntimeEventKind`].
    fn observe(&self, _execution: &str, _section: &str, _event: Observation) {}

    fn on_assistant_reply(
        &self,
        _execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        text: &str,
        finish_reason: Option<&str>,
        model: &str,
        metrics: Option<&CallMetrics>,
    ) {
        self.append(RuntimeEvent {
            kind: RuntimeEventKind::AssistantReply,
            section: section.to_owned(),
            chain_id,
            depth,
            turn,
            content: text.to_owned(),
            model: Some(model.to_owned()),
            tool_call_id: None,
            finish_reason: finish_reason.map(str::to_owned),
            metrics: metrics.cloned(),
        });
    }

    /// Records the batch with its content rendered as the JSON array of
    /// the calls, so a reader can parse the ids, names, and arguments
    /// back out of one string field.
    fn on_assistant_tool_calls(
        &self,
        _execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        model: &str,
        calls: &[ToolCallEvent],
    ) {
        let content = match serde_json::to_string(calls) {
            Ok(content) => content,
            Err(source) => {
                tracing::warn!(%source, "tool-call batch not recorded: serialization failed");
                return;
            }
        };
        self.append(RuntimeEvent {
            kind: RuntimeEventKind::AssistantToolCalls,
            section: section.to_owned(),
            chain_id,
            depth,
            turn,
            content,
            model: Some(model.to_owned()),
            tool_call_id: None,
            finish_reason: None,
            metrics: None,
        });
    }

    /// Records the result content keyed by its provider call id. The
    /// alias and the trust marking have no field in the event vocabulary
    /// and are deliberately dropped.
    fn on_tool_result(
        &self,
        _execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        tool_call_id: &str,
        _alias: &str,
        content: &str,
        _trusted: bool,
    ) {
        self.append(RuntimeEvent {
            kind: RuntimeEventKind::ToolResult,
            section: section.to_owned(),
            chain_id,
            depth,
            turn,
            content: content.to_owned(),
            model: None,
            tool_call_id: Some(tool_call_id.to_owned()),
            finish_reason: None,
            metrics: None,
        });
    }

    fn on_thinking(
        &self,
        _execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        model: &str,
        text: &str,
    ) {
        self.append(RuntimeEvent {
            kind: RuntimeEventKind::Thinking,
            section: section.to_owned(),
            chain_id,
            depth,
            turn,
            content: text.to_owned(),
            model: Some(model.to_owned()),
            tool_call_id: None,
            finish_reason: None,
            metrics: None,
        });
    }

    fn on_user_input(&self, _execution: &str, section: &str, text: &str) {
        self.append(RuntimeEvent {
            kind: RuntimeEventKind::UserInput,
            section: section.to_owned(),
            chain_id: 0,
            depth: 0,
            turn: 0,
            content: text.to_owned(),
            model: None,
            tool_call_id: None,
            finish_reason: None,
            metrics: None,
        });
    }
}

impl EventLog for WorkshopObserver {
    fn len(&self) -> u64 {
        self.read().events.len() as u64
    }

    fn get(&self, index: u64) -> Option<RuntimeEvent> {
        let inner = self.read();
        usize::try_from(index)
            .ok()
            .and_then(|index| inner.events.get(index).cloned())
    }
}

/// The header line, newline included, that opens every persisted log.
fn header_line() -> io::Result<String> {
    let header = Header {
        format: LOG_FORMAT.to_owned(),
        version: LOG_VERSION,
    };
    let mut line = serde_json::to_string(&header).map_err(io::Error::other)?;
    line.push('\n');
    Ok(line)
}

/// Reads and validates a persisted log: the versioned header line, then
/// one event per line.
fn replay(path: &Path) -> io::Result<Vec<RuntimeEvent>> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return Err(invalid_data(format!(
            "missing event log header in {}",
            path.display()
        )));
    };
    let header: Header = serde_json::from_str(first).map_err(|source| {
        invalid_data(format!(
            "malformed event log header in {}: {source}",
            path.display()
        ))
    })?;
    if header.format != LOG_FORMAT || header.version != LOG_VERSION {
        return Err(invalid_data(format!(
            "unsupported event log {} version {} in {}; this build reads {LOG_FORMAT} version {LOG_VERSION}",
            header.format,
            header.version,
            path.display()
        )));
    }
    lines
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|source| {
                invalid_data(format!(
                    "malformed event on line {} of {}: {source}",
                    index + 2,
                    path.display()
                ))
            })
        })
        .collect()
}

/// An [`io::ErrorKind::InvalidData`] error carrying `message`.
fn invalid_data(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
#[path = "observer-tests.rs"]
mod tests;
