//! The workshop's run event log: the [`Observer`] write side, the
//! [`EventLog`] read side, live broadcast fan-out, and versioned JSONL
//! persistence in one append-only type.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use promptforge_core_support::events::{
    CallMetrics, EventLog, RuntimeEvent, RuntimeEventKind, ToolCallEvent,
};
use promptforge_core_support::observe::{Observation, Observer};
use serde::{Deserialize, Serialize};
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
    /// use promptforge_core_support::events::EventLog;
    /// use promptforge_core_support::observe::Observer;
    /// use promptforge_workshop_server::WorkshopObserver;
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
    /// use promptforge_core_support::events::EventLog;
    /// use promptforge_core_support::observe::Observer;
    /// use promptforge_workshop_server::WorkshopObserver;
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
    /// use promptforge_core_support::observe::Observer;
    /// use promptforge_workshop_server::WorkshopObserver;
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
mod tests {
    use std::sync::Arc;

    use promptforge_core_support::events::{ClientTiming, LlamaTimings, Usage, VllmMetrics};
    use serde_json::json;

    use super::*;

    fn full_metrics() -> CallMetrics {
        CallMetrics {
            usage: Some(Usage {
                prompt_tokens: 7,
                completion_tokens: 3,
                total_tokens: 10,
                cached_tokens: Some(2),
                reasoning_tokens: Some(1),
            }),
            llama: Some(LlamaTimings {
                prompt_n: 7,
                prompt_ms: 12.5,
                prompt_per_second: 560.0,
                predicted_n: 3,
                predicted_ms: 30.5,
                predicted_per_second: 98.5,
                draft_n: 4,
                draft_n_accepted: 2,
            }),
            vllm: Some(VllmMetrics {
                time_to_first_token_ms: Some(8.5),
                generation_time_ms: Some(22.5),
                queue_time_ms: Some(1.5),
                mean_itl_ms: Some(7.5),
                tokens_per_second: Some(133.5),
            }),
            client: Some(ClientTiming {
                ttft_ms: Some(9.5),
                mean_itl_ms: Some(8.25),
                e2e_ms: 41.5,
            }),
        }
    }

    /// Emits one event of every kind through the Observer hooks.
    fn emit_one_of_each(log: &WorkshopObserver) {
        log.on_user_input("run", "chat", "hi");
        log.on_thinking("run", "chat", 0, 0, 1, "llama-3", "pondering");
        log.on_assistant_tool_calls(
            "run",
            "chat",
            0,
            0,
            1,
            "llama-3",
            &[ToolCallEvent {
                id: "call_1".to_owned(),
                name: "read_file".to_owned(),
                arguments: json!({ "path": "notes.txt" }),
            }],
        );
        log.on_tool_result(
            "run",
            "chat",
            0,
            0,
            1,
            "call_1",
            "read_file",
            "file contents",
            false,
        );
        log.on_assistant_reply(
            "run",
            "chat",
            1,
            0,
            2,
            "hello",
            Some("stop"),
            "llama-3",
            Some(&full_metrics()),
        );
    }

    fn collect(log: &WorkshopObserver) -> Vec<RuntimeEvent> {
        (0..log.len())
            .map(|index| log.get(index).expect("every index below len() reads"))
            .collect()
    }

    #[test]
    fn concurrent_appends_lose_nothing_and_preserve_per_producer_order() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let log = Arc::new(WorkshopObserver::new(Some(&path)).expect("open a fresh log"));

        let mut producers = Vec::new();
        for producer in 0..4 {
            let log = Arc::clone(&log);
            producers.push(std::thread::spawn(move || {
                let section = format!("producer-{producer}");
                for sequence in 0..25 {
                    log.on_user_input("run", &section, &sequence.to_string());
                }
            }));
        }
        for producer in producers {
            producer.join().expect("producer threads finish");
        }

        assert_eq!(log.len(), 100, "no append may be lost");
        let events = collect(&log);
        let expected: Vec<String> = (0..25).map(|sequence| sequence.to_string()).collect();
        for producer in 0..4 {
            let section = format!("producer-{producer}");
            let sequence: Vec<&str> = events
                .iter()
                .filter(|event| event.section == section)
                .map(|event| event.content.as_str())
                .collect();
            assert_eq!(
                sequence, expected,
                "{section} must keep its own append order through the interleaving"
            );
        }

        // The file's order is the in-memory order: the two advance under
        // one guard, and the replay proves it.
        let replayed = WorkshopObserver::load_from(&path).expect("replay the concurrent log");
        assert_eq!(collect(&replayed), events);
    }

    #[test]
    fn event_log_reads_see_a_consistent_prefix() {
        let log = Arc::new(WorkshopObserver::new(None).expect("open a memory log"));
        let writer = Arc::clone(&log);
        let producer = std::thread::spawn(move || {
            for sequence in 0..200 {
                writer.on_user_input("run", "chat", &sequence.to_string());
            }
        });

        // Every observed length is a fully readable prefix, and an entry
        // once appended never changes.
        loop {
            let len = log.len();
            for index in 0..len {
                let event = log
                    .get(index)
                    .expect("every index below an observed len() must read");
                assert_eq!(
                    event.content,
                    index.to_string(),
                    "entry {index} must be the entry that was appended there"
                );
            }
            if len == 200 {
                break;
            }
            std::thread::yield_now();
        }
        producer.join().expect("the producer thread finishes");
    }

    #[test]
    fn subscribe_receives_every_entry_in_log_order() {
        let log = WorkshopObserver::new(None).expect("open a memory log");
        let mut entries = log.subscribe();
        emit_one_of_each(&log);

        let expected = [
            (RuntimeEventKind::UserInput, "hi".to_owned()),
            (RuntimeEventKind::Thinking, "pondering".to_owned()),
            (
                RuntimeEventKind::AssistantToolCalls,
                r#"[{"id":"call_1","name":"read_file","arguments":{"path":"notes.txt"}}]"#
                    .to_owned(),
            ),
            (RuntimeEventKind::ToolResult, "file contents".to_owned()),
            (RuntimeEventKind::AssistantReply, "hello".to_owned()),
        ];
        for (kind, content) in expected {
            let received = entries.try_recv().expect("every appended entry broadcasts");
            assert_eq!((received.kind, received.content), (kind, content));
        }
        assert!(
            matches!(
                entries.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ),
            "no entry may broadcast that was not appended"
        );
    }

    #[test]
    fn append_and_load_round_trip_byte_for_byte() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let log = WorkshopObserver::new(Some(&path)).expect("open a fresh log");
        emit_one_of_each(&log);
        let in_memory = collect(&log);
        drop(log);

        let original = std::fs::read_to_string(&path).expect("read the persisted log");
        let restored = WorkshopObserver::load_from(&path).expect("replay the log");
        assert_eq!(collect(&restored), in_memory, "replay restores every entry");

        // Re-serializing the replayed log reproduces the file byte for
        // byte: nothing was lost, reordered, or reshaped in either
        // direction.
        let mut rebuilt = header_line().expect("the header line renders");
        for event in collect(&restored) {
            rebuilt.push_str(&serde_json::to_string(&event).expect("events serialize"));
            rebuilt.push('\n');
        }
        assert_eq!(rebuilt, original);

        // A loaded log keeps appending to the same file, behind the same
        // header.
        restored.on_user_input("run", "chat", "again");
        drop(restored);
        let reloaded = WorkshopObserver::load_from(&path).expect("replay the appended log");
        assert_eq!(reloaded.len(), 6);
        assert_eq!(
            reloaded.get(5).map(|event| event.content),
            Some("again".to_owned())
        );
    }

    #[test]
    fn load_from_tolerates_crlf_line_endings() {
        // An autocrlf checkout of the committed canary, or a log touched
        // by a CRLF editor, materializes \r\n endings; replay must keep
        // reading such a file.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let log = WorkshopObserver::new(Some(&path)).expect("open a fresh log");
        emit_one_of_each(&log);
        let events = collect(&log);
        drop(log);

        let text = std::fs::read_to_string(&path).expect("read the persisted log");
        std::fs::write(&path, text.replace('\n', "\r\n")).expect("rewrite with CRLF endings");

        let replayed = WorkshopObserver::load_from(&path).expect("a CRLF log must still load");
        assert_eq!(collect(&replayed), events);
    }

    #[test]
    fn new_truncates_to_a_fresh_headed_log() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        std::fs::write(&path, "stale junk from an earlier life\n").expect("seed stale bytes");

        let log = WorkshopObserver::new(Some(&path)).expect("open over the stale file");
        drop(log);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read the fresh log"),
            header_line().expect("the header line renders"),
            "new() must truncate to a bare versioned header"
        );
        let empty = WorkshopObserver::load_from(&path).expect("replay the fresh log");
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn load_from_rejects_missing_and_alien_headers_and_torn_lines() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let header = header_line().expect("the header line renders");

        let empty = dir.path().join("empty.jsonl");
        std::fs::write(&empty, "").expect("write the empty file");
        let error = WorkshopObserver::load_from(&empty).expect_err("an empty file must not load");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("missing event log header"));

        let alien = dir.path().join("alien.jsonl");
        std::fs::write(
            &alien,
            "{\"format\":\"workshop-event-log\",\"version\":999}\n",
        )
        .expect("write the alien file");
        let error =
            WorkshopObserver::load_from(&alien).expect_err("an alien version must not load");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("unsupported event log"));

        let torn = dir.path().join("torn.jsonl");
        std::fs::write(&torn, format!("{header}{{\"kind\":\"user_message\""))
            .expect("write the torn file");
        let error = WorkshopObserver::load_from(&torn).expect_err("a torn line must not load");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("line 2"),
            "the error must name the offending line: {error}"
        );

        // The version discipline leans on this: a kind outside the
        // version-1 vocabulary (the reserved `plan`, for one) must refuse
        // to load rather than replay as something else.
        let unknown = dir.path().join("unknown.jsonl");
        std::fs::write(
            &unknown,
            format!(
                "{header}{{\"kind\":\"plan\",\"section\":\"chat\",\"chain_id\":0,\"depth\":0,\"turn\":0,\"content\":\"\"}}\n"
            ),
        )
        .expect("write the unknown-kind file");
        let error = WorkshopObserver::load_from(&unknown)
            .expect_err("a kind this version does not speak must not load");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("malformed event on line 2"),
            "the error must name the offending line: {error}"
        );

        let missing = dir.path().join("missing.jsonl");
        let error =
            WorkshopObserver::load_from(&missing).expect_err("a missing file must not load");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_poisoned_lock_recovers_for_appends_and_reads() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let log = Arc::new(WorkshopObserver::new(Some(&path)).expect("open a fresh log"));

        let poisoner = Arc::clone(&log);
        let panicked = std::thread::spawn(move || {
            let _guard = poisoner
                .inner
                .write()
                .expect("the lock is not yet poisoned");
            panic!("poisoning the event log lock on purpose");
        })
        .join();
        assert!(panicked.is_err(), "the poisoning thread must panic");
        assert!(log.inner.is_poisoned(), "the lock must be poisoned");

        // Zone two: the poison is recovered, not propagated - appends,
        // reads, broadcast, and persistence all keep working.
        let mut entries = log.subscribe();
        log.on_user_input("run", "chat", "after the poison");
        assert_eq!(log.len(), 1);
        assert_eq!(
            log.get(0).map(|event| event.content),
            Some("after the poison".to_owned())
        );
        assert_eq!(
            entries
                .try_recv()
                .expect("the broadcast survives the poison")
                .content,
            "after the poison"
        );
        drop(entries);
        drop(log);
        let replayed = WorkshopObserver::load_from(&path).expect("replay the poisoned-era log");
        assert_eq!(replayed.len(), 1, "persistence survives the poison");
    }

    #[test]
    fn a_failing_writer_degrades_to_the_in_memory_log() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("injected append failure"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let log = WorkshopObserver::with_writer_for_test(FailingWriter);
        let mut entries = log.subscribe();
        emit_one_of_each(&log);
        assert_eq!(
            log.len(),
            5,
            "a failed file append never loses the in-memory entry"
        );
        assert_eq!(
            entries
                .try_recv()
                .expect("the broadcast survives the failing writer")
                .content,
            "hi"
        );
    }
}
