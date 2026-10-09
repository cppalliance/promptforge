---
name: Messages userdata refactor
overview: Replace the pure-Lua messages.new() builder with a Rust-backed list that remembers its last issued request, so each model round's Chat record names the round it extends (after) and how many leading messages it keeps (keep), and logs only the messages after those, with no list events and no new public types.
todos:
  - id: step-1-sandbox-pairs
    content: "Step 1: the sandbox pairs in iteration.rs honors a userdata __pairs, with tests"
    status: pending
  - id: step-2-list-core
    content: "Step 2: MessageList core in messages.rs (builders, replace, system-first, extra fields dropped, __len, __newindex, commit) plus parse_record, with tests"
    status: pending
  - id: step-3-record-views
    content: "Step 3: read-only RecordView, MessageRecord::to_json, list __index (integral floats too) and __pairs, with tests"
    status: pending
  - id: step-4-cutover
    content: "Step 4: messages.new() returns the Rust list; shim append and handle rule; parse_chat takes the list; engine imports swap MessageRecord for MessageList; test migrations, argument and author-shape tests; full test commands"
    status: pending
  - id: step-5-chat-record
    content: "Step 5: after and keep on Effect::Chat and EffectRecord::Chat, commit after the precheck, listing re-blessed, rebuild and refused-round tests; full gates"
    status: pending
  - id: docs-follow-up
    content: "Follow-up outside the run: rewrite promptforge-docs chapters 11 and 17 and rebuild the books"
    status: pending
isProject: false
---

# Messages userdata refactor

<product-contract>

## Product Requirements

Every model round in a `models.loop` conversation records the entire message list again in its Chat effect record, so the run log grows with the square of the number of turns. This plan makes `messages.new()` return a Rust-backed list that remembers the request it last sent. When a model round uses the list, that round's Chat record names the earlier round whose request it extends (`after`), says how many leading messages it keeps from that request (`keep`), and logs only the messages after those. Authors keep the same chainable builders, gain `replace` for explicit edits, and lose hand-written arrays, index assignment, and writes to records read from the list. Every fact below was checked against `master` at `bae2a59ac`. The promptforge repository is at `c:\Users\Vinnie\cursor\promptforge`, and paths are relative to it unless they name `promptforge-docs/`, the separate guide repository at `c:\Users\Vinnie\cursor\promptforge-docs`, or `mlua-0.12.0/`, mlua's sources at `C:\Users\Vinnie\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\mlua-0.12.0`.

- Problem and users:
  - Harness operators and anyone reading the run log.
    - `Effect::record` maps every wire message into `EffectRecord::Chat.messages` on every round (`crates/promptforge-internal/engine/src/execute/run/effect.rs`, line 178; the record type is at lines 218 to 234).
    - The harness serializes the whole record into the log (`crates/harness-internal/runner/src/effect_loop.rs`, line 389).
    - So a k-turn conversation writes on the order of k squared message bodies.
  - Prompt authors. Today the list is a plain Lua table (`crates/promptforge-internal/lua/src/__impl_messages.lua`, lines 12 to 52). The Engine sees each list only as a snapshot when a round runs, so it cannot tell what changed since the previous round.
- Goals:
  - `messages.new()` returns an engine-owned list that remembers its last issued request.
  - A Chat record names the round its request extends and how many leading messages it keeps from that request, and logs only the messages after those.
  - Edits that cancel out between two sends, and records put back unchanged, add nothing to the log.
  - System records are allowed only in the leading block, refused at the line that breaks the rule.
  - Any change to earlier records is an explicit `replace`, and a record read from the list cannot be changed in place.
- Non-goals:
  - Anything outside the message list: the compactor framework, self-restore, a JSONL log backend, tool-offering interning, and log text deduplication.
  - Porting the `models.loop` shim's logic to Rust. The separate loop plan (`C:\Users\Vinnie\.cursor\plans\models_loop_in_rust_5e81c0d4.plan.md`) does that after this plan lands.
  - Changing the projection in `crates/promptforge-internal/lua/src/projection.rs`, which already enforces the leading-system rule and cross-record pairing at dispatch.
  - Bumping the run log's `LAYOUT_VERSION`. The rule is to bump only on table-definition changes (`crates/workshop/run-log/src/schema.rs`, lines 17 to 20), and each record's payload is one JSON text column (line 47). This plan changes only that JSON. Nothing but engine tests reads the recorded Chat messages; the facade suite only round-trips whole records (`crates/promptforge/tests/suite/effect.rs`, lines 187 to 190).
- Success criteria:
  - A list's first send logs its whole request with no `after` and `keep` 0. Each later send names the list's previous issued round as `after` and logs only the messages after the shared leading ones: within one `models.loop` call that is the shim's appends, and nothing on a send with no changes.
  - For every Chat record, taking the first `keep` messages of `after`'s rebuilt request and adding the record's messages yields exactly the wire messages on that round's live `Effect::Chat`.
  - A record added and then removed between two sends, and a record replaced by an identical one, add nothing to the log.
  - Writing a field of a record read from a list raises an error that points to `replace`.
  - `prompts/research-person.md` and `crates/workshop/agents/agents/chat.md` run unchanged.
  - The guide chapter on conversations describes the new list.
- Constraints:
  - The Engine stays sans-IO: no new effect kind and no new suspension point.
  - Each change is the smallest that satisfies its goal. A new public item, a changed contract, or anything beyond the decisions below needs the user's decision first. User's words: "make sure we are not building out too much ... without direct input from me."
  - No new public type. The public API changes only by the `after` and `keep` fields on the two Chat variants.
  - The 500-line ceiling is enforced on the lua and engine crates, so new or edited files stay at or under it.
  - The facade firewall and the API listing gate stay in force.
- Open questions: None.

## Functional Specification

Authors build conversations with `messages.new()` and the same chainable methods as today, plus `replace` for editing earlier records. The list can be read by index, length, and iteration but not assigned into, records read from it are read-only, and `models.loop` accepts only such a list. Edits leave no record of their own: when a round is issued, its Chat record names the round it extends and logs only what follows the shared leading messages.

- Actors and workflows:
  - Prompt author, typical shape: `local msgs = messages.new()`, `msgs:user(prose)`, `models.loop(msgs)`, `return msgs[#msgs].content`.
  - List methods, each returning the list so calls chain:
    - `system(content)`, `user(content)`, `assistant(content, tool_calls?)`, `tool(content, tool_call_id)`, and `append(record)`: unchanged surface. `append` also accepts a record view read from any list. Fields beyond `role`, `content`, `tool_calls`, and `tool_call_id` are dropped when the record is added, so they cannot be read back from the list. Today they stay on the table; nothing in-tree reads them.
    - `replace(first, last, records...)`:
      - takes a 1-based inclusive range;
      - with no records, it deletes the range;
      - `first == last + 1` inserts before `first`;
      - bounds are `1 <= first <= #list + 1` and `first - 1 <= last <= #list`, and anything else is an error naming the bounds;
      - `first` and `last` accept integers and integral floats, and anything else is an error.
    - Removing the terminal reply is `msgs:replace(#msgs, #msgs)`.
  - `models.loop(handle?, messages, compactor?)`:
    - It still appends assistant, tool-call, tool-result, and task-notice records into the list.
    - The leading handle form is recognized when the first argument is userdata and the second is neither nil nor a function. Otherwise the first argument is the list. So `(msgs)` and `(msgs, compactor)` are the list form, and `(handle, msgs)`, `(handle, msgs, compactor)`, and `(handle, {plain array})` are the handle form, where the plain array then gets the plain-table error below. This is a stopgap that the loop plan's Phase 1 replaces with exact type checks.
- Inputs and outputs:
  - `#msgs` is the record count. `msgs[i]` returns a read-only view of record `i`, or nil out of range. `i` may be an integer or an integral float, so `msgs[4 / 2]` reads record 2 as it does on a plain table; any other key reads nil. `pairs(msgs)` and `ipairs(msgs)` both visit records 1 to `#msgs` in order, each as a read-only view, so loops written for plain arrays keep working.
  - A record view:
    - reads like the record table: `role`, `content`, `tool_calls` when present, and `tool_call_id` when present, with `content` as a string or a fresh content-parts table and `tool_calls` as a fresh table;
    - raises an error pointing to `replace` when any field is assigned;
    - visits its present fields under `pairs`, in the order `role`, `content`, `tool_calls`, `tool_call_id`;
    - converts to JSON exactly like the record table it stands for, so `other:append(msgs[i])`, tool arguments, and every other JSON conversion see the full record;
    - has `type()` `userdata`.
  - A Chat effect record holds `after`, `keep`, and `messages`:
    - `after` is the id of the earlier round whose request this one extends, or none;
    - `keep` is how many leading messages this request shares with `after`'s request, 0 when `after` is none;
    - `messages` is this request's wire messages after the first `keep`, in wire form as today.
  - A `models.infer` round has no list: `after` is none, `keep` is 0, and `messages` is its one user message, exactly as recorded today.
- States and validation:
  - A list that has never been sent has no previous round, and its first send records no `after` and `keep` 0.
  - Each later send records the list's previous issued round as `after`. `keep` counts the leading wire messages of the new request that equal that round's request, stopping at the first difference.
  - A send is a round issued as an `Effect::Chat`. Every refusal in `prepare_chat` before the effect is issued, such as a missing model, a tool-scope failure, a projection error, or the context precheck (`crates/promptforge-internal/engine/src/execute/scheduler/chat.rs`, lines 115 to 164), writes no Chat record and leaves the list's last issued round and request where they were. Otherwise the next record would name a round that no record holds.
  - Because `keep` compares only the last issued request with the new one, a record added and removed between sends never appears, and a record replaced by an identical one adds nothing.
  - An edit that changes an earlier message, such as a compaction that rewrites the prefix, stops `keep` at that message, so the messages after it are logged again once, on the next send.
  - Each record's fields are validated when it is added, with the existing per-record rules of the chat parse (`crates/promptforge-internal/lua/src/protocol/parse/chat.rs`, `parse_message` and its helpers, lines 88 to 269), so a malformed record fails at the builder call instead of at `models.loop`.
  - The leading-system rule is checked against the list's state after every edit, including `replace`.
  - Cross-record rules, such as tool-call batch pairing and unique tool-call ids, stay in `project_messages` at dispatch.
- Errors and recovery:
  - A plain Lua table passed to `models.loop`: `models.loop needs a messages.new() list; build one with messages.new() and :user, :append, or :replace`.
  - An empty list passed to `models.loop`: `messages must not be empty`, as today.
  - `msgs[i] = x`: an error pointing to `append` and `replace`.
  - `msgs[i].field = x`: `records read from a messages.new() list are read-only; change the list with replace(first, last, records...)`.
  - A system record after any non-system record: an error naming the position and pointing to `replace` on the leading block.
  - A list or a record view stored in `var` or substituted with `{{ }}` fails through the existing userdata guards in `var` (`crates/promptforge-internal/lua/src/sys.rs`, line 86) and prose substitution (`prose.rs`, line 33).
- Security and privacy behavior:
  - The projection's metadata strip is unchanged, and Chat records log the projected wire messages, as today.
- Acceptance criteria:
  - Every Success criterion holds, and every Testing Plan item passes.

</product-contract>
<implementation-contract>

## Technical Design

The list becomes a Rust userdata in `promptforge-lua` whose state is shared through an `Arc`, so the chat request can hold a handle to it. When a `models.loop` round is issued, `prepare_chat` reads the list's records and projects them. Right after the precheck, it asks the list to record the new request, which returns the previous issued round and the number of leading messages the two requests share. Those two numbers ride on the Chat effect beside the projected wire messages, and the effect's log record keeps them plus only the messages after the shared ones. No event variants and no public types are added, and the harness changes in one pattern only. The harness log, the workshop, and the run-log layout need no change.

- Architecture:
  - The chat parse, its dispatch, and the issue of the effect run in one chain step with no Lua in between (`crates/promptforge-internal/lua/src/protocol/parse.rs`, line 215; `crates/promptforge-internal/engine/src/execute/scheduler/dispatch.rs`, lines 202 to 205; `crates/promptforge-internal/engine/src/execute/scheduler.rs`, lines 348 to 355). So dispatch reads the list's records and records the send once nothing can refuse the round, with no edit possible in between.
  - Every issued effect is returned from the step that issued it (`crates/promptforge-internal/engine/src/execute/scheduler/drive.rs`, line 56), and the harness records every effect a step returns, even moot ones it then drops (`crates/harness-internal/runner/src/effect_loop.rs`, lines 175 to 182 and 190 to 191). So a list's Chat records in log order are exactly its sends, and every `after` names a recorded round.
  - Sends are recorded at the send, which happens at the same program point in every run, not per Engine step. A replay compares each re-issued effect against its record (`crates/promptforge-internal/engine/src/execute/run/effect.rs`, lines 206 to 207), so a record must come out the same in every recording of the same program.
  - `Effect::record` sees only the effect, so `after` and `keep` ride on `Effect::Chat`, and the record takes the messages after `keep` from the effect's own wire messages.
  - `keep` counts wire messages, not records, because the projection merges runs of records (`crates/promptforge-internal/lua/src/projection.rs`, lines 15 to 19) and the record logs wire messages. Comparing the new request with the last one costs no more than the projection that already runs every round.
  - `after` is a `RoundId`, which is already public (`crates/promptforge-internal/types/src/ids.rs`, lines 222 to 225, serialized as a bare number), already in the record as `round`, and already what the round's content events carry.
  - References are logical, not physical file positions, so a future JSONL backend would hold the same records. User's words: "some people will want Turso, others JSONL".
- Modules and interfaces:
  - `promptforge-lua` (`crates/promptforge-internal/lua/src/`):
    - `messages.rs` (56 lines today) holds `MessageList` and the record view in place of the pure-Lua chunk. `__impl_messages.lua` is deleted.
    - The list stores its records as the existing message record type, plus its last issued round and wire request.
    - `MessageRecord` has no serde form today (`protocol/request.rs`, lines 377 to 390). A crate-private `to_json` renders a record in the JSON form the chat parse accepts, and record views serialize through it.
    - A record view is a serializable userdata, made with mlua's `create_ser_userdata` (`mlua-0.12.0/src/state.rs`, line 1630). mlua's deserializer converts such a userdata through its `Serialize` implementation (`mlua-0.12.0/src/serde/de.rs`, line 195), so a view converts like the record table it stands for. A read-only proxy table would not work: mlua converts a table by its raw fields, so a proxy would convert as an empty table.
    - `install_messages` keeps its place in `inject_values_with_var` (`vm/install.rs`, line 149) and its signature, because the list needs nothing from the emitter or from `sys`.
    - The loop shim is `models_loop` in `__impl_coro.lua` (lines 236 to 316), the Lua that implements `models.loop` by yielding `chat` and `tool_call` requests to the Engine and appending records to the author's list. In it, `append_record` (lines 197 to 199) calls `messages:append(record)` instead of storing by index, and the argument handling of `models_loop` (lines 259 to 271) picks the handle form only when the first argument is userdata and the second is neither nil nor a function. Both are stopgaps: the loop plan's Phase 1 replaces `append_record` and the handle rule when it ports the loop to Rust, so they need to work, not to be polished. The task notices the shim drains before each round go through `append_record` too (line 210).
    - In `protocol/parse/chat.rs`, `parse_chat` (lines 34 to 53) requires a `MessageList` userdata for `messages`, refuses a plain table with the error in Functional Specification and an empty list with `messages must not be empty`, and puts a clone of the list handle on `Request::Chat` (`protocol/request.rs`, lines 173 to 190) in place of the records.
    - The methodless-handle rule (`crates/promptforge-internal/lua/AGENTS.md`, line 5) says Engine globals are namespace functions and handles are frozen userdata with no methods, with `messages.new()` builders as the one deliberate exception. Its text in that file, in `lib.rs` (lines 51 to 55), and in `messages.rs` (lines 8 to 9) changes to say the exception is the Rust-backed list, whose builders and `replace` are colon methods. A record view has metamethods only, so it follows the rule.
    - The sandbox's `pairs` replacement (`iteration.rs`, lines 117 to 138) reads `__pairs` only from a table's metatable and refuses every other value with `bad argument #1 to 'pairs' (table expected, got ...)`. It also honors a `__pairs` metamethod on a userdata, as stock Lua does, and still refuses a userdata without one with the same message. That makes the module doc's "exactly as in stock Lua" (line 8) true for userdata too. `ipairs` is the stock function and reads through `__index`, so it needs no change.
  - `promptforge-engine` (`crates/promptforge-internal/engine/src/execute/`):
    - `run/effect.rs`: `Effect::Chat` (lines 87 to 101) gains `after` and `keep`, which are `None` and 0 on a `models.infer` round. Its `messages` stay the full projected wire messages, so the host can still perform the round.
    - `run/effect.rs`: `EffectRecord::Chat` (lines 218 to 234) gains `after` and `keep`, and its `messages` hold the wire messages after `keep`. `Effect::record` (lines 167 to 187) copies `after` and `keep` and skips the first `keep` messages.
    - `scheduler/chat.rs`: `dispatch_chat` (line 80) and `prepare_chat` (line 102) take the list handle in place of the records slice. `prepare_chat` reads `list.records()` before projecting. Right after the precheck (lines 159 to 164), the last point that can refuse the round, it numbers the round and calls `list.commit(round.id, &conversation)`, which records the request and returns `after` and `keep` for the effect. Neither `number_round`, nor `commit`, nor `issue` (line 176) can fail, so the send is recorded exactly when the round is issued.
    - The `models.infer` round sets `after` to `None` and `keep` to 0 (`scheduler/dispatch.rs`, lines 247 to 253).
  - Facade: no new re-exports. `crates/promptforge/public-api.txt` gains the `after` and `keep` lines beside lines 773 to 778 and 798 to 805, and it is re-blessed in the same change.
  - Harness: `crates/harness-internal/runner/src/effect_loop.rs` destructures `Effect::Chat` field by field (lines 325 to 331) and adds `..`. The other host-side matches already end in `..`.
- File and public API changes:
  - Deleted: `crates/promptforge-internal/lua/src/__impl_messages.lua`.
  - Public:
    - the `after` and `keep` fields on `Effect::Chat` and `EffectRecord::Chat`, and the narrower meaning of `EffectRecord::Chat.messages`;
    - author-facing: the list surface in Functional Specification. Hand-written arrays, index assignment, and field writes on record views are refused.
  - Docs, in the separate `promptforge-docs` repository: `src/language/11-conversations.md` and `src/language/17-quick-reference.md`.
    - Line 36 and line 52 say the list is a plain Lua table.
    - Line 118 says hand-written arrays work.
    - Line 217 removes the terminal record with `msgs[#msgs] = nil`, which becomes `msgs:replace(#msgs, #msgs)`.
    - Line 307 describes the late-system error, which is now raised by the append itself.
    - The Checking the list section (lines 315 to 382) says the builders check nothing and every check runs at the `models.loop` call. Per-record checks and system placement now run at the edit, and the rows for the list as a whole (lines 336 to 342) change, since a plain table is now refused outright.
    - The chapter says that a record read from a list is a read-only view.
    - Line 118 and line 173 say extra fields on a record are kept or accepted; they are now dropped when the record is added and cannot be read back.
    - `src/language/17-quick-reference.md`, the `messages` table (lines 83 to 97): add `list:replace`, `#list`, and `list[i]` rows, change `list:append` from "appended unchanged" to validated with extra fields dropped, and say the `record.*` rows read a read-only view.

    These are rewritten to match Functional Specification, and the books are rebuilt with `cargo xtask site --books-only` per `promptforge-docs/CONTRIBUTING.md` (lines 5 to 12).
- Data, persistence, failure, security, and privacy constraints:
  - The run-log table layout is unchanged, so `LAYOUT_VERSION` stays at 1.
  - No run logs exist yet, so the new Chat record shape needs no migration and no version marker. User's words: "old log files don't matter, there aren't any."
  - The messages a Chat record logs repeat text that other records already hold, once per conversation rather than once per round. A reply is also in its `AssistantReply` event (`crates/promptforge-internal/types/src/event.rs`, line 360) and in its answer record (`ChatAnswerRecord.reply`, `run/effect.rs`, line 340). A tool's output, operator input included, is also in its `ToolResult` event (`event.rs`, line 400) and in its answer record (`ToolAnswerRecord.text`, `run/effect.rs`, line 371). A task notice is also in its `TaskNotice` event (`event.rs`, line 416). Answer records are logged like effects (`effect_loop.rs`, line 406), so reply and tool text is stored up to three times and notice text twice. The repetition is accepted in this plan. The separate run-log dedup plan (`C:\Users\Vinnie\.cursor\plans\run_log_blob_dedup_7c3e91a2.plan.md`) stores each long string once per `runs.db` inside `workshop-run-log`, with no change here.
  - A compaction that rewrites the prefix logs the kept suffix again once, on the next send. The user accepts that cost: compression or the dedup plan can reduce it later.

### Exact Rust declarations

Every new or changed declaration, checked against `master` at `bae2a59ac`. Function bodies are elided with `;` except where the body is the spec. Declarations not listed keep their current form.

#### `promptforge-lua`, `messages.rs`

These replace the chunk loader (lines 18 to 52), and the module doc (lines 1 to 16) is rewritten for the Rust list. `mlua` is built with the `send` feature (root `Cargo.toml`, line 120), so userdata must be `Send + Sync`: `Arc` and `Mutex`, never `Rc` or `RefCell`.

```rust
/// A `messages.new()` list. The same value is the Lua userdata and the
/// handle a model round's request holds; clones share one list.
#[derive(Clone, Debug, Default)]
pub struct MessageList(Arc<Mutex<ListState>>);

/// What a list holds.
#[derive(Debug, Default)]
struct ListState {
    /// The records, in order, shared with the views read from them.
    records: Vec<Arc<MessageRecord>>,
    /// The last issued send: its round and the wire request it sent.
    last: Option<(RoundId, Vec<Message>)>,
}

impl MessageList {
    /// The records in order, for a round's projection.
    #[must_use]
    pub fn records(&self) -> Vec<MessageRecord>;

    /// Whether the list holds no records.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool;

    /// Appends `record`. A system record after any non-system record is
    /// refused with a message naming its position.
    pub(crate) fn push(&self, record: MessageRecord) -> std::result::Result<(), String>;

    /// Replaces the 1-based inclusive range `first..=last` with
    /// `records`, under the bounds in Functional Specification, then
    /// checks the leading-system rule. A refused edit leaves the list
    /// unchanged.
    fn replace(
        &self,
        first: usize,
        last: usize,
        records: Vec<MessageRecord>,
    ) -> std::result::Result<(), String>;

    /// Records `request` as the list's send in `round`, and returns the
    /// previous send's round and how many leading messages `request`
    /// shares with that send's request; `(None, 0)` on the first send.
    #[must_use]
    pub fn commit(&self, round: RoundId, request: &[Message]) -> (Option<RoundId>, u64);
}

impl UserData for MessageList {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M);
}

/// A read-only view of one record, which `msgs[i]` and iteration return.
/// It serializes as the record's JSON form, so a view converts like the
/// record table it stands for.
#[derive(Clone, Debug)]
struct RecordView(Arc<MessageRecord>);

impl serde::Serialize for RecordView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>;
}

impl UserData for RecordView {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M);
}

/// Installs the `messages` global holding `new`, which returns a new
/// empty [`MessageList`].
///
/// # Errors
/// Returns [`Error::Lua`] if the global install fails.
pub(crate) fn install_messages(lua: &Lua, globals: &Table) -> Result<()>;
```

Every method locks the state briefly and never calls Lua while the lock is held: Lua values are converted before locking, and views are built after unlocking. The lock ignores poisoning (`lock().unwrap_or_else(PoisonError::into_inner)`), so the methods that return no `Result` cannot fail.

`MessageList`'s Lua methods are registered with `add_function`, so each takes the list as `AnyUserData` and returns it, which is what makes calls chain. Each returns `mlua::Result<AnyUserData>`, builds its record as JSON, validates it with `parse_record` at the position it will take, and raises a refusal as `mlua::Error::external(Error::Lua(message))`, the crate's convention (`compactors.rs`, line 259; `prose.rs`, line 87), so an author `pcall` catches it. Validation runs before any change, so a refused call leaves the list as it was.

- `system(content)` and `user(content)`: `(AnyUserData, Value)`.
- `assistant(content, tool_calls?)`: `(AnyUserData, Value, Option<Value>)`.
- `tool(content, tool_call_id)`: `(AnyUserData, Value, Value)`.
- `append(record)`: `(AnyUserData, Value)`, where a record view converts through its `Serialize` implementation like a record table.
- `replace(first, last, records...)`: `(AnyUserData, Value, Value, Variadic<Value>)`, where `first` and `last` accept integers and integral floats.
- `MetaMethod::Len`, through `add_meta_method`: returns `usize`.
- `MetaMethod::Index`: an integer key, or an integral float key, returns a new `RecordView` made with `lua.create_ser_userdata`, or nil out of range; any other key returns nil. Method names stay with mlua's method lookup, which runs before a user `__index`.
- `MetaMethod::NewIndex`: always the error pointing to `append` and `replace`.
- `MetaMethod::Pairs`: returns an iterator function, the list, and nil. Each call reads the live list and returns the next index with a new view of that record, then nil after `#list`, so `pairs` and `ipairs` visit the same records in the same order.

`RecordView`'s metamethods:

- `MetaMethod::Index`: `role` returns the role string; `content` the text, or a fresh content-parts table; `tool_calls` a fresh table of `{ id, name, arguments }` tables, or nil when the record has none; `tool_call_id` the id, or nil; any other key nil.
- `MetaMethod::NewIndex`: always `records read from a messages.new() list are read-only; change the list with replace(first, last, records...)`.
- `MetaMethod::Pairs`: visits the present fields in the order `role`, `content`, `tool_calls`, `tool_call_id`.

`lib.rs` re-exports `MessageList` beside `project_messages` (line 167).

#### `promptforge-lua`, `protocol/request.rs`

```rust
impl MessageRecord {
    /// The record's JSON form: `role`, `content`, and `tool_calls` and
    /// `tool_call_id` when present, in the shapes the chat parse accepts,
    /// so the parse turns it back into an equal record.
    #[must_use]
    pub(crate) fn to_json(&self) -> serde_json::Value;
}
```

`Request::Chat` (lines 173 to 190) becomes:

```rust
    /// One stateless tool-capable model round over a `messages.new()`
    /// list, yielded by the `models.loop` shim once per round. The round
    /// advertises the section's current tool scope, local Lua tools
    /// included, resolved in the dispatch arm where the tool scope sits.
    Chat {
        /// The list the round sends. Its records were validated as the
        /// list added them.
        list: MessageList,
        /// The loop shim's leading handle, as its frozen binding cloned
        /// out of the userdata while the VM handle is live; `None` when
        /// the round names no handle, and the driver resolves the
        /// section's current model.
        binding: Option<ModelBinding>,
    },
```

The `Request` doc (lines 17 to 22) stays true: the list handle is shared Rust data, not a VM handle.

#### `promptforge-lua`, `protocol/parse/chat.rs`

```rust
/// Parses a `chat` request: the loop shim's optional leading `handle`
/// and the `messages` list, which must be a non-empty `messages.new()`
/// list.
pub(super) fn parse_chat(table: &mlua::Table) -> std::result::Result<Request, FieldFailure>;

/// Validates one record in its JSON form, as a list builder adds it.
/// `index` is the 1-based position the record takes, named in the
/// error.
pub(crate) fn parse_record(
    index: usize,
    entry: &serde_json::Value,
) -> std::result::Result<MessageRecord, String>;
```

- `parse_chat` no longer converts a table, so it drops its `lua` parameter, and the call in `protocol/parse.rs` (line 215) becomes `parse_chat(table)`. It borrows the `messages` userdata as a `MessageList` and clones the handle.
- `parse_messages` (lines 55 to 86) loses its only caller and is deleted. `parse_message` (line 89) and its helpers stay, behind `parse_record`.

#### `promptforge-engine`, `run/effect.rs`

`Effect::Chat` (lines 87 to 101) gains two fields after `messages`:

```rust
        /// The earlier round whose request this one extends: the previous
        /// send of the same `messages.new()` list. `None` on a list's first
        /// send and on a nested `models.infer` round.
        after: Option<RoundId>,
        /// How many leading messages this request shares with `after`'s
        /// request; 0 when `after` is `None`.
        keep: u64,
```

`EffectRecord::Chat` (lines 218 to 234) gains the same two fields, and `messages` keeps its type and its wire form:

```rust
        /// The earlier round whose request this one extends, or `None`
        /// when the request starts here.
        after: Option<RoundId>,
        /// How many leading messages of `after`'s request this request
        /// repeats; 0 when `after` is `None`.
        keep: u64,
        /// The request's messages after the first `keep`, one wire-form
        /// message per entry.
        messages: Vec<Value>,
```

The Chat arm of `Effect::record` (lines 167 to 187) becomes:

```rust
            Effect::Chat {
                binding,
                messages,
                after,
                keep,
                tools,
                round,
                ..
            } => {
                let invocation = binding.invocation();
                let kept = usize::try_from(*keep).unwrap_or(usize::MAX);
                EffectRecord::Chat {
                    round: round.id,
                    alias: binding.alias().to_owned(),
                    after: *after,
                    keep: *keep,
                    messages: messages.iter().skip(kept).map(wire_value).collect(),
                    tools: tools
                        .iter()
                        .map(|schema| tool_schema_name(schema).to_owned())
                        .collect(),
                    temperature: invocation.temperature.map(Temperature::get),
                    max_tokens: invocation.max_tokens.map(std::num::NonZeroU32::get),
                    thinking: invocation.thinking,
                }
            }
```

In the `EffectRecord` doc (lines 211 to 212), "It stores the messages in their wire form." becomes "It stores the request's messages in wire form after the first `keep`, which repeat round `after`'s request."

#### `promptforge-engine`, scheduler

`dispatch_chat` (`scheduler/chat.rs`, line 80) and `prepare_chat` (line 102) take the list handle in place of the records slice:

```rust
    pub(super) fn dispatch_chat(
        &mut self,
        id: ChainIndex,
        list: MessageList,
        binding: Option<ModelBinding>,
    );

    fn prepare_chat(
        &mut self,
        id: ChainIndex,
        list: MessageList,
        binding: Option<ModelBinding>,
    ) -> Result<ChatDispatch>;
```

`prepare_chat` starts with `let messages = list.records();` and projects `&messages`. Its tail (lines 165 to 177), right after the precheck, becomes:

```rust
        chain
            .anchor
            .sending(binding.id().clone(), conversation.clone());
        let round = self.number_round(ReplyOrigin::Chat);
        let (after, keep) = list.commit(round.id, &conversation);
        let effect = Effect::Chat {
            options: binding.completion_options(),
            binding,
            messages: conversation,
            after,
            keep,
            tools: schemas,
            round,
        };
        self.issue(id, effect, Continuation::Chat(round.id));
        Ok(ChatDispatch::Issued)
```

In `scheduler/dispatch.rs`, the `Request::Chat` arm (lines 202 to 205) becomes:

```rust
            Request::Chat { list, binding } => {
                self.dispatch_chat(id, list, binding);
                Ok(())
            }
```

The infer effect (lines 247 to 253) adds `after: None, keep: 0,` after `messages: vec![Message::user(prompt)],`.

#### Re-exports, the harness, and tests

- `MessageList` takes `MessageRecord`'s place in the explicit `crate::lua` re-export list in `crates/promptforge-internal/engine/src/lua.rs` (line 18), and in the `crate::lua` import of `scheduler/chat.rs` (line 33). Nothing else in the engine names `MessageRecord`, so leaving it would fail the unused-import lint under `CARGO_BUILD_WARNINGS=deny`.
- No facade re-export changes. The listing gains the four field lines.
- Harness, `crates/harness-internal/runner/src/effect_loop.rs` (lines 325 to 331): the pattern becomes `Effect::Chat { binding, messages, tools, options, round, .. }`.
- `run/tests.rs` (lines 56 to 111) builds `Effect::Chat` with `after: None, keep: 0`.
- `protocol/tests/parse_chat.rs` matches `Request::Chat { messages, binding }` exhaustively (line 43, and the pattern at line 280), so those switch to `list`. `projection-tests.rs` (around line 88) parses plain-array fixtures through `Request::from_yield`; those become lists, and the records come from `list.records()`.
- `error.rs` (line 24) drops "and the messages library" from its list of compiled-program statics.

Choices made in these declarations, open to override:

- `u64` for `keep`, matching `EffectId`, so no count needs a narrowing conversion.
- Records are shared by `Arc` between the list and the views read from it, so a view costs no copy of its record.
- The send is recorded right after the precheck instead of after `issue`. That is equivalent, because nothing after the precheck can refuse the round and neither `number_round`, `commit`, nor `issue` can fail.
- `keep` uses `Message`'s existing `PartialEq` (`model-client/src/client/wire.rs`, line 19).
- If `messages.rs` would pass the 500-line ceiling, `RecordView` moves into a private child module, `messages-view.rs`, loaded with `#[path]` the way `messages-tests.rs` is.
- New record tests go in `run/effect-tests.rs` (206 lines), not `run/tests.rs`, which is at 475.

</implementation-contract>
<verification-contract>

## Testing Plan

Unit tests pin the list's surface, its edit rules, the read-only views, and what each send records. A rebuild test proves that following each Chat record's `after` and `keep` recovers exactly what the model saw on every round. Existing engine tests that call `models.loop` or `models.infer` guard against regressions, and the exit gates are the repository's full set.

- Unit:
  - `crates/promptforge-internal/lua/src/messages-tests.rs`, rewritten for the userdata:
    - each builder chains and appends;
    - `replace` edges: delete, insert at `first == last + 1`, full replace, out-of-bounds, and a non-integral float;
    - system-first is refused after a builder call, after `append`, and after `replace`;
    - `msgs[i] = x` is refused;
    - record views: each field reads like the record table; assigning any field raises the read-only error; `pairs` over a view visits its present fields in order; a view appended to another list, and a view converted with `lua.from_value`, give the same record as the table form;
    - `pairs(msgs)`, `ipairs(msgs)`, and `for i = 1, #msgs` each visit every record in order as views, and visit nothing on an empty list;
    - commit, with requests made by projecting the records through `project_messages`: the first commit returns `(None, 0)`; a resend with no changes keeps every message; appends keep the whole previous request; a `replace` in the middle keeps the messages before the first changed one; a system edit keeps 0; removing the terminal reply keeps all but the last; each commit records its round, so the next commit names it.

    The raw-array parity test (lines 145 to 181) becomes a test that a plain table is refused by the chat parse.
  - Plain-array fixtures in the chat-parse tests (`crates/promptforge-internal/lua/src/protocol/tests/parse_chat.rs`) are converted to lists or become rejection cases. The per-record error tests there (lines 143 to 264: malformed tool calls, content-part payloads, a non-string `tool_call_id`, and the index each error names) move to builder tests through `append`, keeping their expected messages, because the list now refuses those records when they are added.
  - `crates/promptforge-internal/lua/src/iteration-tests.rs`: the sandbox `pairs` honors a userdata's `__pairs`, and still refuses a userdata without one with `bad argument #1 to 'pairs' (table expected, got userdata)`.
  - `models.loop` argument handling: `(msgs)`, `(msgs, compactor)`, `(handle, msgs)`, and `(handle, msgs, compactor)` each resolve the list, and `(handle, {plain array})` fails with the plain-table error.
- Integration and end-to-end:
  - A rebuild test. Run each case with a driver that keeps every live `Effect::Chat` beside its record. For each Chat record in log order, take the first `keep` messages of the request rebuilt for `after` (an empty request when `after` is none), add the record's messages, and assert that the result equals the wire messages on that round's live `Effect::Chat`. The cases:
    - a tool round;
    - a `replace` that compacts earlier records, where `keep` stops at the first changed message;
    - an add then a remove between two sends, which adds nothing to the log;
    - a resend with no changes, which logs no messages and keeps the whole previous request;
    - a re-insert of an identical record, which adds nothing;
    - removing the terminal reply and resending.
  - A refused round: a send refused by the context precheck, followed by an edit and a resend. The resend's `after` names the last issued round, and its `keep` is counted against that round's request. `crates/promptforge-internal/engine/src/execute/tests/precheck_anchor.rs` (lines 165 to 181) already drives a precheck refusal.
  - An engine test that runs the `prompts/research-person.md` Lua shape against a canned model.
  - Updated for the record shape:
    - `crates/promptforge-internal/engine/src/execute/tests/effects.rs` (lines 78 to 86, 113 to 114, and 129): the infer record has no `after`, `keep` 0, and the same messages as today; the first loop round has no `after`, `keep` 0, and one message; the second names the first round as `after`, keeps 1, and logs two messages, the assistant tool-call record and the tool record;
    - `crates/promptforge-internal/engine/src/execute/run/tests.rs` (lines 56 to 111) builds `Effect::Chat` with `after: None, keep: 0` and adds a record with an `after` to its serde round-trip.
  - `crates/promptforge-internal/engine/src/execute/tests/serial_driver.rs` (lines 44 to 52) needs no change, because an infer record's messages are unchanged.
  - `msgs[#msgs] = nil` becomes `msgs:replace(#msgs, #msgs)` in `crates/promptforge-internal/engine/src/execute/tests/models_loop.rs` (line 131) and `precheck_anchor.rs` (line 173).
- Regression, security, and performance:
  - Every existing engine test that calls `models.loop` or `models.infer` passes unchanged. That covers about 109 `messages.new()` sites and 60 indexed reads in `crates/`. A Rust test that reads `msgs[i]` back as a `Table` must read the view through `lua.from_value` instead; the regression run finds any.
- Exit criteria, the repository's gates (`AGENTS.md`, lines 52 to 58), run from `c:\Users\Vinnie\cursor\promptforge`:
  - Tests: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
  - Clippy with `CARGO_BUILD_WARNINGS=deny`: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, then `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`.
  - Formatting: `cargo fmt --all --check`.
  - Docs with `RUSTDOCFLAGS="-D warnings"`: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then `cargo doc -p promptforge --no-deps`.
  - Facade surface: `cargo +<pinned nightly> xtask api --check`, with the nightly named in `crates/build-xtask/src/api/toolchain.rs`, passing against the listing re-blessed with `--bless`. The listing's diff is the four new field lines only.
  - Structure: `cargo test -p build-xtask`.
  - The `promptforge-docs` books rebuild cleanly with `cargo xtask site --books-only`, checked by the docs follow-up in that repository.

</verification-contract>
<decision-record>

## Decision Record

The conversation list moves into the Engine, which records each round's request as an extension of the list's previous request instead of photographing the whole list every round. Each Chat record names the round it extends and how many leading messages it keeps, with no digest, no list events, and no new public types, because following `after` back and taking `keep` messages at each step rebuilds every request a model saw. The plan covers the message refactor and nothing else.

- Decisions:
  - The message list is a Rust-backed userdata, so the Engine sees every append and compaction. User's words: "my thinking now is that messages should be a special built-in table which is a userdata, and the Rust code backs it, so it sees through appends and compactions".
  - Scope is the message refactor alone. User's words: "create a plan for the message refactor specifically and nothing else".
  - `models.loop` requires a `messages.new()` list. User's choice: "Require a messages.new() list; a plain array is an error naming the fix (one code path, every Chat record is list-backed)". No in-tree prompt or test passes a hand-written array to `models.loop`.
  - The edit surface is the builders plus `replace`, with no `truncate`. User's choice: "Only replace(first, last, records...): it deletes, inserts, and compacts; the two tests become msgs:replace(#msgs, #msgs) (Recommended)".
  - The net change since the list's last send is recorded inside the round's Chat record, and there are no list events. Only the result matters, at the point the list is used. User's choice: "Record the net change inside the model call's record, with no list events."
  - The change is recorded as `after` and `keep`: the round the request extends, and how many leading messages it shares with that round's request, with only the rest logged. User's words: "do the 'after plus keep' if it reduces complexity. we can pay a little extra per compaction. maybe in the future we compress it with zlib or something." It needs no list ids, versions, per-entry marks, stored copy of sent records, trimming, pending-send handle, or new public type, and the format can grow exact deltas or compression later without breaking what it already records.
  - Sends are recorded at the send, not per Engine step. When several tasks run at once, which edits land in which step depends on when answers arrive, so per-step grouping could differ between recordings of the same program. A send happens at the same program point every time, so what it records is deterministic.
  - `keep` compares the new request with the list's last issued request, message by message in wire form, stopping at the first difference. Equal messages anywhere before that point cost nothing, which keeps the user's rule "if it's the same thing, we just need to see the result" for edits that leave the prefix unchanged.
  - A Chat record names its base by round id, with no content digest. User's words: "if we dont need the sha then drop it".
  - System records stay in the leading block, and prefix changes are explicit edits. User's words: "I think adding system messages later in the stack is a bad idea. Because of prefix cache invalidation, changes to the prefix of the messages should be visible and explicit". `keep` makes such a change visible in the log, because it stops at the first changed message.
  - The messages after `keep` are logged in full, and the repeated text is accepted for now. The user accepted the same repetition earlier: "Accept the 2x for now; list events carry full records, so a list rebuilds from list events alone (Recommended)". The count is up to three copies, not two, because answer records also keep reply and tool text.
  - Records read from a list are read-only views. User's choice: "make the returned copy read-only yes". A view is a serializable userdata, not a proxy table, because mlua converts a table by its raw fields, so a proxy table would convert as empty when passed to a tool or appended to another list.
  - Lua keeps control of compaction policy; `replace` is the mechanism.
  - Lists iterate like arrays: `pairs`, `ipairs`, and numeric loops over `#msgs` all visit the records in order. User's words: "of course lists should support pairs, how else would the Lua iterate the list?"
  - The shim edits in this plan, `append_record` through `append` and the handle rule, are stopgaps that the loop plan's Phase 1 replaces. They must work, and need no polish beyond that.
  - The stopgap handle rule picks the handle form when the first argument is userdata and the second is neither nil nor a function, so a handle followed by a plain array gets the designed plain-table error. User's choice: "All five fixes plus the polish items, including the new handle rule". The two-userdata rule it replaces would have answered `(handle, {plain array})` with "compactor must be a function, got table".
  - Fields beyond the four a record holds are dropped when the record is added, because the list stores parsed records. Nothing in-tree reads them back, and the guide changes in the docs follow-up.
  - `msgs[i]` accepts integral floats as well as integers, matching a plain table's key normalization and `replace`'s bounds.
  - `install_messages` stays where it is, because the list needs nothing from the emitter or from `sys`.
  - No `LAYOUT_VERSION` bump, per the repository's rule that only table-definition changes bump it.
  - The conversations chapter rewrite is a follow-up outside the vibe run, because it lands in the separate `promptforge-docs` repository and the run's commit cycle works in one repository. User's choice: "Remove Step 7 from this run and do the docs as a separate follow-up in promptforge-docs".
  - Each behavior commit carries its own proofs, with no tests-only step: the author-shape tests land with the cutover, and the rebuild and refused-round tests land with the Chat record change. User's choice: "Move the author-shape tests into Step 4 and the rebuild and refused-round tests into Step 5, then delete Step 6".
- Rejected alternatives:
  - One event per edit (`MessagesCreated`, `MessagesAppended`, `MessagesReplaced`). Reason: a prompt that manipulates its list heavily writes a long stream of events, an add that is later removed is logged anyway, and only the list at the point of use matters. Revisit: if something other than a model round needs a list's edit history.
  - A separate list event at each send, with the Chat record pointing at it. Reason: it splits one round's input across two records and adds a public event variant that every event consumer has to skip. Revisit: if something other than a model round needs to read the list.
  - Splices against the previous send, with list ids and versions. Reason: exact deltas need per-entry marks, a stored copy of the sent records, trimming by value, a pending-send handle in the chat request, and three new public types, while their only gain is not logging a compaction's kept suffix again. Revisit: if compaction logs measurably too much, with or without compression.
  - Accepting plain arrays by adopting them, or keeping both paths. Reason: two code paths, and an adopted table silently stops receiving appends. Revisit: if authors need hand-built arrays.
  - `truncate(len)`. Reason: tail removal has no production caller, and `replace` covers it. Revisit: never.
  - A SHA digest in each reference. Reason: redundant with `after` and `keep`. Revisit: when replay needs a cheap comparison value.
  - Pointing logged messages at the event or answer record that already holds their text. Reason: it would need origin tracking across the Lua boundary, and the separate run-log dedup plan removes the repeated text in storage with no Engine change. Revisit: if that plan is dropped.
  - Allowing system messages anywhere and combining them at the front. Reason: it rewrites the prefix implicitly and discards the model's prefix cache. Revisit: never.
  - A read-only proxy table as the record view. Reason: it converts to JSON as an empty table. Revisit: never.
  - Keeping the list in the Harness. Reason: a hidden side channel, and the host does not know which list a round used. Revisit: never.
- Assumptions, risks, and notes:
  - The log shows each list only as of its last send. Edits after the last send and before a crash are not logged, and a list that is never sent never appears in the log. Replay does not mind, because it re-runs the program. A self-restoring prompt would see its history as of the last model call; the reply, tool results, and operator input after that are already logged as their own answer and event records, and operator input arrives as a tool output (`crates/plugin-user-input/src/ask.rs`, line 60).
  - Debug capture, when a caller turns it on, still logs each round's full request body as a `Request` event (`crates/promptforge-internal/engine/src/execute/support.rs`, lines 103 to 111). It is opt-in and outside this plan.
  - A list, and now a record view, cannot be stored in `var` or substituted into prose; storing a field works. Nothing in-tree stores a list, and the regression run will show whether anything stores a whole record.
  - `type(msgs[i])` is now `userdata`, not `table`.
  - A view's nested tables, content parts and `tool_calls`, are fresh copies, so writing into them changes nothing. Only field writes on the view itself raise.
  - The userdata and the loop shim's switch to `append` must land together. The userdata alone breaks the shim's index store.
  - A plain table passed to `models.loop` while task notices are pending fails at the shim's `messages:append` with Lua's own nil-call error, before the parse can raise the designed one. Accepted: it needs an earlier loop in the same chain to have left undelivered notices, and the loop plan's Phase 1 removes it.

### Deferred and Out of Scope

- Deferred: an author compactor framework that edits the list with `replace`. Revisit when compaction work starts.
- Deferred: compressing logged messages. Revisit when log size is measured.
- Out of scope: self-restore, a JSONL log backend, tool-offering interning, the loop port (the separate loop plan), and any change outside the message list.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway`. The workspace `default-members` is only `crates/gateway/app`, so a plain `cargo build` builds just the gateway. The headless gateway is `cargo build --locked -p gateway --no-default-features`, the desktop app is `cargo build --locked -p workshop`, and any one crate is `cargo build --locked -p <crate>`. The UIs build with `npm ci --prefix crates/workshop` then `npm run build --workspace ui` inside `crates/workshop`, and `npm ci` then `npm run build` inside `crates/gateway/config-ui/ui` (both run `node build.mjs`).
- Focused test command pattern: `cargo nextest run --locked -p <crate> <test-name-substring>`, adding `--test <target>` when the test lives in an integration binary. CI also runs single named tests as `cargo test --locked -p <crate> --test it <test_name>` (with `--no-default-features --features test-fixtures` for the gateway lease tests). One JS file runs with `node --test <path>.test.mjs` from its package directory.
- Component test command pattern: `cargo nextest run --locked -p <crate>` runs a crate's unit and integration tests; `--lib` runs unit tests only; `--test <target>` runs one integration binary, where the target is `suite` for `promptforge` and `harness` and `it` for every other crate with a `tests/it/main.rs`. Crates with flat integration files (`gateway-stt-engine`, `gateway-stt-backend-whisper`, `gateway-cloud-providers`, `build-workshop`) use the file stem as the target. Test-only features are `test-support` (`promptforge-engine`, `promptforge-lua`, `promptforge-parser`, `promptforge-plugin`) and `test-fixtures` (gateway and Workshop crates). A JS package runs with `npm test --workspace <ui|look|platform>` inside `crates/workshop`, or `npm test` inside `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then the Workshop crates separately with `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. CI adds `cargo nextest run --locked -p workshop-workspace --all-features` and `cargo nextest run --locked -p workshop-server --features headless`. Structural and boundary checks are `cargo test -p build-xtask`, and the nightly job adds `cargo nextest run --locked -p build-xtask --run-ignored only`. The JS suites are `npm test --workspaces --if-present` inside `crates/workshop` and `npm test` inside `crates/gateway/config-ui/ui`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, and for Workshop `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, both gated with `CARGO_BUILD_WARNINGS=deny`. The one standalone check is the headless build shape `cargo check -p gateway --no-default-features`. CI also runs `cargo deny check`, `cargo audit`, and `cargo hakari verify`, and checks that `ring` stays out of the gateway's normal dependency closure. TypeScript is checked with `npm run typecheck --workspaces --if-present` inside `crates/workshop` and `npm run typecheck` inside `crates/gateway/config-ui/ui`. The root `Cargo.toml` `[workspace.lints]` deny `clippy::all`, `pedantic`, `unwrap_used`, `expect_used`, `allow_attributes`, and `unsafe_code`; `clippy.toml` allows unwrap and expect in tests and bans process-global installers such as `std::panic::set_hook` and `tracing::subscriber::set_global_default`.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` sets `style_edition = "2024"`, and `.githooks/pre-commit` runs it). No JS formatter check is configured in CI.
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, and for the facade with default features `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`. CI also builds `cargo doc -p harness --no-deps`, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, and `cargo doc --locked --no-deps -p workshop-server --document-private-items`. The facade surface check is `cargo +<pinned nightly> xtask api --check`, with the nightly named in `crates/build-xtask/src/api/toolchain.rs`, compared against `crates/promptforge/public-api.txt`.
- Test placement and naming conventions:
  - Unit tests sit in a sibling file `<stem>-tests.rs` wired from `<stem>.rs` with `#[cfg(test)]`, `#[path = "<stem>-tests.rs"]`, and `mod tests;` (about 177 such files), for example `crates/promptforge-internal/lua/src/messages-tests.rs` beside `messages.rs`. Extra test files for one stem take a dash label, such as `prelude-tests-environment.rs`. Larger groups use a `tests/` subdirectory or a `tests.rs` with topic submodules, such as `crates/promptforge-internal/engine/src/execute/tests/` and `crates/promptforge-internal/lua/src/tests.rs`.
  - Integration tests are one binary per crate at `tests/it/main.rs` (or `tests/suite/main.rs` for `promptforge` and `harness`) with sibling modules and `fixtures/` or `common/` directories. A few gateway crates and `build-workshop` use flat files under `tests/` instead.
  - Integration binary roots carry `#![expect(clippy::expect_used, reason = "...")]`; unit tests rely on `clippy.toml`'s `allow-expect-in-tests` and `allow-unwrap-in-tests`.
  - Test names are full descriptive sentences in snake case, such as `a_well_formed_conversation_projects_unchanged` and `pairs_visits_string_keys_in_byte_order`.
  - No doctests: the `build-xtask` `no_doctests` check fails any doc comment holding a code block rustdoc would compile, so examples go in `text`, `json`, or `toml` fences.
  - Plugin tests lend a Plugin its context through `promptforge_plugin::testing::TestCall`, enabled by the `test-support` feature. The two benches (`models_loop` in `promptforge-engine`, `surface` in `promptforge-lua`) are criterion targets with `harness = false` and `required-features = ["test-support"]`.
  - JS tests are `*.test.mjs` under `src/` or `*.mjs` under `test/`, run with `node --test`. `.config/nextest.toml` defines `default` and `ci` profiles with a 60 second slow timeout and a `heavy` test group for the STT packages.
- Directory map:
  - `crates/` holds every Rust crate and the TypeScript packages. Flat crates: `promptforge` (Engine facade), `promptforge-plugin` (Plugin contract), `plugin-mcp`, `plugin-web`, `plugin-user-input` (Plugins), `harness`, `harness-gateway-client`, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `workspace-hack` (cargo-hakari), and the `build-*` tooling crates (`build-xtask`, `build-workshop`, `build-ui`, `build-user-guide`, `build-ceiling`, `build-llama-cuda`).
  - Manifestless containers hold each family's private crates: `crates/promptforge-internal/` (types, vfs, model-client, lua, parser, engine), `crates/harness-internal/runner`, `crates/gateway/` (app, config, config-ui, local, logging, progress, protocol, routing, cloud-providers, web-search, and `stt/` with api, engine, backend-whisper, whisper-ffi), and `crates/workshop/` (desktop, server, server-api, gateway, menu, protocol, registry, status, support, user-state, workspace, run-log, agents, plus the npm workspaces `ui`, `look`, `platform`). `crates/shared-ui` is a TypeScript and CSS package, not a crate.
  - `crates/promptforge-internal/lua/src/` holds the Lua VM layer: Rust modules plus pure-Lua shims loaded with `include_str!` (`__impl_messages.lua`, `__impl_globals.lua`, `__impl_store.lua`, `__impl_tasks.lua`, `__impl_fanout.lua`, `__impl_coro.lua`). `messages.rs` builds the `messages` namespace and `models-userdata.rs` sits beside `models.rs`. Lua userdata code is concentrated in `promptforge-lua` (24 files), with a few uses in `promptforge-engine`.
  - `guide/` holds the user documentation sources (`books/` for gateway, language, and workshop, plus `chrome/` and `landing/`); `prompts/` holds example prompts; `tools/` holds `.mjs` sidecar staging and live-test scripts; `vibe/` holds tracked plan files; `images/` holds README art.
  - `.github/workflows/` holds CI and release workflows, `.githooks/` holds `pre-commit` (fmt) and `pre-push` (headless check, clippy, `cargo deny`), `.config/` holds `nextest.toml` and `hakari.toml`, `.cursor/rules/` holds `workshop-architecture.mdc` and `workshop-spa.mdc`, and `.cargo/config.toml` sets the Windows `rust-lld` linker with static CRT and defines the `xtask` and `workshop` aliases.
  - Root config: `Cargo.toml`, `clippy.toml`, `deny.toml`, `rustfmt.toml`, `rust-toolchain.toml` (stable), `dist-workspace.toml` (cargo-dist), and `gateway.local.example.toml`. Ignored: `local/` (developer configs, prompts, and STT fixtures), `target/`, and `target-msrv/`; `cabinet/` is untracked.
- Component boundaries (normal dependency directions from `cargo metadata`, ignoring `workspace-hack`):
  - Engine: `promptforge-types` and `promptforge-vfs` depend on nothing in the workspace; `promptforge-model-client` depends on types; `promptforge-lua` on model-client, types, and vfs; `promptforge-parser` on lua and types; `promptforge-engine` on lua, model-client, parser, types, and vfs; the `promptforge` facade re-exports all six.
  - Plugin contract: `promptforge-plugin` depends only on `promptforge-types` and `promptforge-vfs`. `plugin-mcp`, `plugin-web`, and `plugin-user-input` each depend only on `promptforge-plugin`, never on each other, the Engine internals, or the Harness.
  - Harness: `harness-runner` depends on `promptforge` and `promptforge-plugin`; `harness` on `harness-runner`, `promptforge`, and `promptforge-plugin`; `harness-gateway-client` on `harness`, `plugin-web`, and `promptforge`.
  - Gateway: `gateway-api-types` is the leaf; `gateway-config` sits on it, `gateway-protocol` on api-types and config, `gateway-routing` on config and protocol, `gateway-local` on config, progress, protocol, and routing, and `gateway-stt` on config, local, progress, and the speech engine crates. The `gateway` app depends on its family (local, stt, web-search, and config-ui optional), `gateway-api-discovery`, and `shared-loopback`. No gateway crate depends on the Engine or the Harness.
  - Workshop (a Host): `workshop-protocol` and `workshop-support` are leaves and `workshop-registry` sits on protocol; `workshop-gateway`, `-menu`, `-status`, `-user-state`, and `-workspace` sit on that base; `workshop-agents` depends on `harness`, `plugin-user-input`, and `promptforge`; `workshop-run-log` on `harness`. `workshop-server` is the top, depending on the Harness, the three Plugins, `promptforge`, `promptforge-plugin`, and the `workshop-*` crates; `workshop-server-api` depends on `workshop-server`, and `workshop` on `workshop-server-api` and `gateway-api-discovery`.
  - `cargo test -p build-xtask` enforces the product and container boundaries, the Workshop tier graph, the `## Invariants` marker, and lint inheritance.
- Conventions summary:
  - Rust 2024 edition on the stable toolchain; every crate inherits the workspace lints with `[lints] workspace = true`, and `missing_docs` and `unreachable_pub` warn, so crates keep a minimal public surface.
  - Every crate's `lib.rs` opens with `//!` crate docs ending in a `## Invariants` list. The defined terms Engine, Harness, Host, and Plugin are capitalized and mean one thing each; Engine crates call their driver "the caller" and never mention the Host. `crates/workshop/ui/test/docs-claims.mjs` scans `AGENTS.md`, `## Invariants` docs, and `.cursor/rules` for these rules.
  - Source directories are flat: a subdirectory needs at least three files, otherwise siblings are named `foo-bar.rs` and wired with `#[path = "foo-bar.rs"] mod bar;`.
  - Dependencies are declared once in `[workspace.dependencies]` with a comment explaining each pin or feature set, and crates use `workspace = true` plus `workspace-hack`.
  - Comments state a non-obvious constraint or cite an upstream issue URL for a workaround. Error messages are concise and self-contained for model consumption, naming required versus actual.
  - JSON that reaches a recorder or replay comparison round-trips exactly (`float_roundtrip`, sorted keys, finite numbers).
  - Behavior changes ship with tests in the same change; types and compiler checks come first, then behavior tests and deterministic fault injection.
  - Workshop UI CSS uses only custom properties from `@workshop/look` and `ui/src/tokens/component.css`, and every persisted UI value goes through the `ui-storage` adapter to an allow-listed server key.
  - Commits use short imperative summaries, and a finished plan lands as `Close plan: <slug>`.

</project-survey>
<execution-plan>

## Execution Instructions

- Before starting:
  - Read `AGENTS.md` at the repository root and `crates/promptforge-internal/lua/AGENTS.md`.
  - Check that `git -C c:\Users\Vinnie\cursor\promptforge rev-parse --short HEAD` prints `bae2a59ac`. If it does not, re-check the cited lines of every file named in File and public API changes, in Exact Rust declarations, and in the steps below before editing, and correct the plan where they moved.
  - Two related plans exist. Do not implement either here:
    - the loop plan (`C:\Users\Vinnie\.cursor\plans\models_loop_in_rust_5e81c0d4.plan.md`) starts after this plan lands and replaces this plan's two shim stopgaps;
    - the dedup plan (`C:\Users\Vinnie\.cursor\plans\run_log_blob_dedup_7c3e91a2.plan.md`) is independent.
  - Every decision is settled, and Open questions is empty. Anything this plan does not decide goes to the user before it is built. User's words: "make sure we are not building out too much ... without direct input from me."
  - Run commands from `c:\Users\Vinnie\cursor\promptforge`. The docs follow-up after the last step runs from `c:\Users\Vinnie\cursor\promptforge-docs` and is not part of this run.
- Gates:
  - Scoped gates for a set of crates: `cargo nextest run --locked -p <crate>...`; `cargo clippy -p <crate>... --all-targets --all-features` with `CARGO_BUILD_WARNINGS=deny`; `cargo fmt --all --check`; `cargo doc -p <crate>... --no-deps --all-features` with `RUSTDOCFLAGS="-D warnings"`; and `cargo test -p build-xtask`, which enforces the crate boundaries and the 500-line ceiling on every source and test file.
  - Full gates: the Exit criteria list in Testing Plan.
  - `<pinned nightly>` is the toolchain named in `crates/build-xtask/src/api/toolchain.rs`.
- Components, in dependency order:
  1. Sandbox iteration (Step 1). First because the list's `pairs` support needs the sandbox `pairs` to honor a userdata `__pairs`, and the change stands alone.
  2. Message list (Steps 2 to 4). Before the Chat record because `prepare_chat` commits through the list handle that the cutover puts on `Request::Chat`. Its pieces are sequential: the list core, then the record views that the list's `__index` and `__pairs` return, then the cutover, which needs both because existing prompts read `msgs[i]`. The cutover lands the userdata, the loop shim switch, and the parse change in one commit, because the userdata alone breaks the shim's index store.
  3. Chat record (Step 5). After the list because each send is committed through the list. Its one piece is the record shape together with the end-to-end proofs that read `after` and `keep`, so the commit that adds the fields also proves them.
- The conversations chapter is a follow-up outside this run, described after Step 5, because it lands in the separate `promptforge-docs` repository.
- Each step is one commit holding its code and its tests, with a short imperative summary, and passes its gates before the next step starts.

<step-1>

### Step 1: Honor userdata __pairs in the sandbox pairs [completed]

- Component: Sandbox iteration

- Piece: sandbox `pairs`, the component's only piece.
- Changes, in `crates/promptforge-internal/lua/src/iteration.rs`:
  - the `pairs` replacement (lines 117 to 138) also calls a `__pairs` metamethod found on a userdata's metatable, as stock Lua does. A userdata without one is still refused with `bad argument #1 to 'pairs' (table expected, got userdata)`, and tables behave as today;
  - the module doc's "exactly as in stock Lua" (line 8) is reworded so it plainly covers userdata too.
- Tests, in `crates/promptforge-internal/lua/src/iteration-tests.rs` (292 lines), using a test-only `UserData` type:
  - a userdata with `__pairs` is iterated through it;
  - a userdata without one is refused with the message above.
- Verify: scoped gates on `promptforge-lua`.
- Commit: `Honor userdata __pairs in sandbox pairs`.

</step-1>

<step-2>

### Step 2: Build the MessageList core [completed]

- Component: Message list

- Piece: list core. Step 3's views read these records, so it comes first.
- Changes:
  - `crates/promptforge-internal/lua/src/protocol/parse/chat.rs`: add `parse_record(index, entry)` as declared in Exact Rust declarations, the entry point to `parse_message` (line 89) and its helpers. `parse_messages` keeps working until Step 4 deletes it.
  - `crates/promptforge-internal/lua/src/messages.rs`: add `MessageList`, `ListState`, and the methods `records`, `push`, `replace`, and `commit` as declared, beside the existing chunk loader, which still backs `messages.new()` until Step 4.
    - State is `Arc<Mutex<ListState>>`. Every method locks briefly, ignores poisoning with `lock().unwrap_or_else(PoisonError::into_inner)`, and never calls Lua under the lock.
    - `UserData for MessageList` registers `system`, `user`, `assistant`, `tool`, `append`, and `replace` with `add_function`, using the argument shapes in Exact Rust declarations. Each takes and returns the list as `AnyUserData`, builds its record as JSON, validates it with `parse_record` at the position it will take, raises a refusal as `mlua::Error::external(Error::Lua(message))`, and leaves the list unchanged when refused.
    - `replace` applies the bounds and the integer rule in Functional Specification. The leading-system rule is checked on the resulting list after every edit, and a refusal names the position and points to `replace` on the leading block.
    - `MetaMethod::Len` returns the record count, and `MetaMethod::NewIndex` always raises an error pointing to `append` and `replace`. `__index` and `__pairs` come in Step 3.
    - `commit(round, request)` returns `(None, 0)` on the first send; otherwise it returns the previous round and the count of leading messages equal under `Message`'s existing `PartialEq`, stopping at the first difference. Either way it then stores `round` and a copy of `request` as the last send.
  - `crates/promptforge-internal/lua/src/lib.rs`: re-export `MessageList` beside `project_messages` (line 167) in this step, so the type is reachable and passes the `dead_code` and `unreachable_pub` lints under `CARGO_BUILD_WARNINGS=deny` before anything installs it. `is_empty` waits for Step 4, where its first caller lands.
- Tests, in a new `crates/promptforge-internal/lua/src/messages-tests-list.rs`, wired from `messages-tests.rs` with `#[path = "messages-tests-list.rs"] mod list;` the way `prelude-tests.rs` wires `prelude-tests-environment.rs`. Until Step 4, a test makes its list with `lua.create_userdata(MessageList::default())` and sets it as a global.
  - each builder chains and appends;
  - `replace` edges: delete, insert at `first == last + 1`, full replace, out-of-bounds with the error naming the bounds, and a non-integral float;
  - a system record is refused after a builder call, after `append`, and after `replace`, and the list is unchanged afterward;
  - a record that breaks a per-record parse rule is refused by the builder;
  - `append` of a record with a field beyond the four stores the record without it, as `list.records()` shows;
  - `msgs[i] = x` is refused, and `#msgs` counts the records;
  - commit, with each request made by running `project_messages` over `list.records()`: the first commit returns `(None, 0)`; a resend with no changes keeps every message; appends keep the whole previous request; a `replace` in the middle keeps the messages before the first changed one; a system edit keeps 0; removing the terminal reply keeps all but the last; each commit records its round, so the next commit names it.
- Verify: scoped gates on `promptforge-lua`.
- Commit: `Add the Rust-backed MessageList core`.

</step-2>

<step-3>

### Step 3: Add read-only record views and list iteration

- Component: Message list

- Piece: record views. They read Step 2's records, and Step 4 needs them because existing prompts read `msgs[i]`. `pairs(msgs)` depends on Step 1.
- Changes:
  - `crates/promptforge-internal/lua/src/protocol/request.rs`: add `MessageRecord::to_json` as declared, rendering the JSON form the chat parse accepts, so `parse_record` turns it back into an equal record.
  - `crates/promptforge-internal/lua/src/messages.rs`, or the private child module `messages-view.rs` loaded with `#[path]` if `messages.rs` would pass 500 lines: add `RecordView(Arc<MessageRecord>)` with `Serialize` through `to_json`, created with `lua.create_ser_userdata`, and its metamethods:
    - `__index`: `role` returns the role string; `content` the text, or a fresh content-parts table; `tool_calls` a fresh table of `{ id, name, arguments }` tables, or nil when absent; `tool_call_id` the id, or nil; any other key nil;
    - `__newindex`: always `records read from a messages.new() list are read-only; change the list with replace(first, last, records...)`;
    - `__pairs`: the present fields in the order `role`, `content`, `tool_calls`, `tool_call_id`.
  - `MessageList` gains `MetaMethod::Index`, where an integer or integral float key returns a new view or nil out of range, any other key returns nil, and method names stay with mlua's method lookup, and `MetaMethod::Pairs`, which returns an iterator function, the list, and nil. Each iterator call reads the live list and returns the next index with a new view, then nil after `#list`.
  - `append` now accepts a view from any list, because a view converts through its `Serialize` implementation like a record table.
- Tests, in a new `crates/promptforge-internal/lua/src/messages-tests-view.rs`, wired from `messages-tests.rs` with `#[path = "messages-tests-view.rs"] mod view;`:
  - `to_json` round-trips every record shape (text, content parts, tool calls, tool result) through `parse_record`;
  - each view field reads like the record table, and `type()` of a view is `userdata`;
  - `msgs[2.0]` reads the same record as `msgs[2]`, while `msgs[1.5]` and `msgs.nope` read nil, and a dropped extra field reads nil on its view;
  - assigning any view field raises the read-only error;
  - `pairs` over a view visits its present fields in order;
  - a view appended to another list, and a view converted with `lua.from_value`, give the same record as the table form;
  - writing into a view's nested tables leaves the record unchanged;
  - with the sandbox `pairs` from Step 1 installed, `pairs(msgs)`, `ipairs(msgs)`, and `for i = 1, #msgs` each visit every record in order as views, and visit nothing on an empty list.
- Verify: scoped gates on `promptforge-lua`.
- Commit: `Add read-only record views to MessageList`.

</step-3>

<step-4>

### Step 4: Cut messages.new() over to the Rust list

- Component: Message list

- Piece: cutover. It needs Steps 2 and 3, and it lands the userdata, the loop shim switch, and the parse change together.
- Changes in `promptforge-lua` (`crates/promptforge-internal/lua/src/`):
  - `messages.rs`: `install_messages` keeps its place in `inject_values_with_var` (`vm/install.rs`, line 149) and its signature, and installs `messages.new` as a Rust function returning a new empty `MessageList` userdata. Delete `MESSAGES_CHUNK_NAME`, `MESSAGES_SOURCE`, and `MESSAGES_PROGRAM`, and rewrite the module doc (lines 1 to 16) for the Rust list. Add `MessageList::is_empty` as declared.
  - Remove `__impl_messages.lua` from the repository with `git rm`; git history keeps it.
  - `__impl_coro.lua`: `append_record` (lines 197 to 199) calls `messages:append(record)`, which also covers the task notices drained at line 210. The argument handling of `models_loop` (lines 259 to 271) takes the leading handle form only when the first argument is userdata and the second is neither nil nor a function; otherwise the first argument is the list. Both are stopgaps the loop plan replaces, so they need to work and nothing more.
  - `protocol/request.rs`: `Request::Chat` becomes `{ list: MessageList, binding }` with the doc in Exact Rust declarations.
  - `protocol/parse/chat.rs`: `parse_chat(table)` drops its `lua` parameter and borrows `messages` as a `MessageList`. It refuses a plain table with `models.loop needs a messages.new() list; build one with messages.new() and :user, :append, or :replace`, refuses an empty list with `messages must not be empty`, and clones the handle onto `Request::Chat`. Delete `parse_messages` (lines 55 to 86). The call in `protocol/parse.rs` (line 215) becomes `parse_chat(table)`.
  - The methodless-handle text in `crates/promptforge-internal/lua/AGENTS.md` (line 5), `lib.rs` (lines 51 to 55), and the new `messages.rs` module doc says the one exception is the Rust-backed list, whose builders and `replace` are colon methods, and that a record view has metamethods only.
  - `error.rs` (line 24) drops "and the messages library" from its list of compiled-program statics.
  - `crates/promptforge-internal/lua/benches/surface.rs` (lines 62 to 63): the `message_building` doc comment describes the Rust-backed list instead of "the pure-Lua `messages.new()` builders ... over a plain numeric table". The bench body is unchanged.
- Changes in `promptforge-engine` (`crates/promptforge-internal/engine/src/`), the minimum to compile against the new request:
  - `lua.rs` (line 18) and the `crate::lua` import in `execute/scheduler/chat.rs` (line 33): replace `MessageRecord` with `MessageList`. Nothing else in the engine names `MessageRecord`, so keeping it fails the unused-import lint under `CARGO_BUILD_WARNINGS=deny`.
  - `execute/scheduler/dispatch.rs`: the `Request::Chat` arm (lines 202 to 205) becomes the form in Exact Rust declarations.
  - `execute/scheduler/chat.rs`: `dispatch_chat` (line 80) and `prepare_chat` (line 102) take `list: MessageList` in place of the records slice, and `prepare_chat` starts with `let messages = list.records();` and projects `&messages`. The commit comes in Step 5.
- Tests:
  - `crates/promptforge-internal/lua/src/messages-tests.rs`: drop the chunk-builder tests that Steps 2 and 3 replace, including `append_adds_a_raw_record_unchanged` (line 132), whose extra-field behavior Step 2's drop test reverses, and build lists with `messages.new()` from `lua_with_messages()`. The raw-array parity test (lines 145 to 181) becomes a test that the chat parse refuses a plain table with the designed error. Add: an empty list is refused with `messages must not be empty`; a non-empty list parses to a `Request::Chat` whose `list.records()` equal the list's records; a list and a record view stored with `var` or substituted with `{{ }}` fail through the existing userdata guards (`sys.rs`, line 86; `prose.rs`, line 33).
  - `crates/promptforge-internal/lua/src/protocol/tests/parse_chat.rs` (313 lines):
    - the valid-shape tests (lines 25 to 141) and the handle tests (lines 270 to 313) build their fixtures as lists, and the patterns at lines 43 and 280 switch from `messages` to `list`;
    - the per-record error tests (lines 143 to 264: malformed tool calls, content-part payloads, a non-string `tool_call_id`, and the index each error names) cannot become lists, because the list refuses those records when they are added. They move to builder tests that add each bad record through `append` and keep their expected messages, in a new `crates/promptforge-internal/lua/src/messages-tests-records.rs` wired from `messages-tests.rs` with `#[path = "messages-tests-records.rs"] mod records;`;
    - only the whole-list shape cases, such as a non-table `messages`, become plain-table rejection cases.
  - `crates/promptforge-internal/lua/src/projection-tests.rs` (around line 88): the `lua_parse` helper builds a list from its Lua source, and the records come from `list.records()`. The file is at 496 lines, and the 500-line ceiling fails the build through `build-ceiling`, so keep the helper change line-neutral or move the helper into a dash-labeled sibling file.
  - `crates/promptforge-internal/engine/src/execute/tests/models_loop.rs` (line 131) and `precheck_anchor.rs` (line 173): `msgs[#msgs] = nil` becomes `msgs:replace(#msgs, #msgs)`. This lands here, not with the Chat record, because index assignment is refused from this commit on.
  - A new `crates/promptforge-internal/engine/src/execute/tests/models_loop-arguments.rs`, wired from `models_loop.rs` with `#[path = "models_loop-arguments.rs"] mod arguments;`: `(msgs)`, `(msgs, compactor)`, `(handle, msgs)`, and `(handle, msgs, compactor)` each resolve the list, and `(handle, {plain array})` fails with the plain-table error.
  - A new `crates/promptforge-internal/engine/src/execute/tests/models_loop-author-shapes.rs`, wired from `models_loop.rs` with `#[path = "models_loop-author-shapes.rs"] mod author_shapes;`, running against a canned model. These are regression guards that pass before and after the cutover:
    - the `prompts/research-person.md` shape (`messages.new()`, `msgs:user(prose)`, `models.loop(msgs)`, `return msgs[#msgs].content`) returns the canned reply;
    - the `crates/workshop/agents/agents/chat.md` shape (`history:user(text)` then `pcall(function() return models.loop(history) end)`, twice, with fixed strings in place of `input.ask()`) completes both turns.
  - Any Rust test that reads `msgs[i]` back as a `Table` reads the view through `lua.from_value` instead; the full test run finds them.
  - Run as written, because every prompt in the tree now builds its list through the userdata: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
- Verify: the two test commands above; scoped clippy, fmt, and docs on `promptforge-lua` and `promptforge-engine`; `cargo test -p build-xtask`; and `cargo +<pinned nightly> xtask api --check`, which passes against the unchanged listing because neither `Request` nor `MessageList` is on the facade.
- Commit: `Back messages.new() with the Rust MessageList`.

</step-4>

<step-5>

### Step 5: Record after and keep on Chat effects

- Component: Chat record

- Piece: record shape and its end-to-end proofs, the component's only piece. It needs Step 4's list handle in `prepare_chat`.
- Changes:
  - `crates/promptforge-internal/engine/src/execute/run/effect.rs`:
    - `Effect::Chat` (lines 87 to 101) gains `after: Option<RoundId>` and `keep: u64` after `messages`, with the docs in Exact Rust declarations. Its `messages` stay the full projected request, so the host can still perform the round.
    - `EffectRecord::Chat` (lines 218 to 234) gains the same two fields, and its `messages` hold the wire messages after the first `keep`. `None` serializes as `null`, like the record's other `Option` fields.
    - The Chat arm of `Effect::record` (lines 167 to 187) becomes the body in Exact Rust declarations.
    - In the `EffectRecord` doc (lines 211 to 212), "It stores the messages in their wire form." becomes "It stores the request's messages in wire form after the first `keep`, which repeat round `after`'s request."
  - `execute/scheduler/chat.rs`: the tail of `prepare_chat` (lines 165 to 177), right after the precheck, becomes the form in Exact Rust declarations: `number_round`, then `list.commit(round.id, &conversation)`, then the effect with `after` and `keep`, then `issue`. A refusal before that point leaves the list's last send untouched.
  - `execute/scheduler/dispatch.rs`: the infer effect (lines 247 to 253) adds `after: None, keep: 0,` after `messages: vec![Message::user(prompt)],`.
  - `crates/harness-internal/runner/src/effect_loop.rs` (lines 325 to 331): the pattern becomes `Effect::Chat { binding, messages, tools, options, round, .. }`. Every other `Effect::Chat` pattern in the tree already ends in `..`.
  - `crates/promptforge/public-api.txt`: re-bless with `cargo +<pinned nightly> xtask api --bless`, then confirm with `--check`. The listing's diff is the four new field lines only.
- Tests:
  - `crates/promptforge-internal/engine/src/execute/run/tests.rs` (lines 56 to 111): build `Effect::Chat` with `after: None, keep: 0`, and add a record with an `after` to the serde round-trip. The file is at 475 lines; if the change would pass 500, put the new round-trip case in `run/effect-tests.rs`.
  - `crates/promptforge-internal/engine/src/execute/run/effect-tests.rs` (206 lines): `Effect::record` copies `after` and `keep` and logs only the messages after `keep`, and a `keep` equal to the message count logs none.
  - `crates/promptforge-internal/engine/src/execute/tests/effects.rs` (lines 78 to 86, 113 to 114, and 129): the infer record has no `after`, `keep` 0, and the same messages as today; the first loop round has no `after`, `keep` 0, and one message; the second names the first round as `after`, keeps 1, and logs two messages, the assistant tool-call record and the tool record.
  - `crates/promptforge-internal/engine/src/execute/tests/serial_driver.rs` needs no change, because an infer record's messages are unchanged.
  - A new `crates/promptforge-internal/engine/src/execute/tests/chat_record_rebuild.rs`, registered with `mod chat_record_rebuild;` in `execute/tests.rs`:
    - a driver that keeps every live `Effect::Chat` beside its `Effect::record()`, and a rebuild check that walks the Chat records in log order, takes the first `keep` messages of the request rebuilt for `after` (an empty request when `after` is none), adds the record's messages, and asserts the result equals the wire messages on that round's live `Effect::Chat`;
    - the cases: a tool round; a `replace` that compacts earlier records, where `keep` stops at the first changed message; an add then a remove between two sends, which adds nothing to the log; a resend with no changes, which logs no messages and keeps the whole previous request; a re-insert of an identical record, which adds nothing; removing the terminal reply and resending.
  - A new `crates/promptforge-internal/engine/src/execute/tests/precheck_anchor-resend.rs`, wired from `precheck_anchor.rs` with `#[path = "precheck_anchor-resend.rs"] mod resend;` so it reuses that file's precheck setup (lines 165 to 181): a send refused by the context precheck writes no Chat record; after an edit, the resend's `after` names the last issued round, and its `keep` is counted against that round's request.
  - Each new file stays at or under 500 lines; split one with a dash label if it would not.
- Verify: the full gates in Exit criteria except the books rebuild, which belongs to the docs follow-up. This is the last step, so the whole set must pass here.
- Commit: `Record after and keep on Chat effects`.

</step-5>

### Follow-up outside this run: rewrite the conversations chapter

This is not a step, and the vibe run does not execute it. It lands as its own change in the separate `promptforge-docs` repository after Step 5 lands, and it meets the success criterion that the guide chapter on conversations describes the new list.

- Changes, in `c:\Users\Vinnie\cursor\promptforge-docs`, file `src/language/11-conversations.md`, matching Functional Specification:
  - lines 36 and 52: the list is a `messages.new()` list owned by the Engine, not a plain Lua table;
  - line 118: `models.loop` refuses a hand-written array, with its error text, and `list:append` validates the record and drops fields beyond the four instead of appending it unchanged;
  - line 173: extra fields are dropped when the record is added, so they cannot be read back from the list;
  - line 217: `msgs[#msgs] = nil` becomes `msgs:replace(#msgs, #msgs)`;
  - line 307: the late-system error is raised by the edit itself;
  - the Checking the list section (lines 315 to 382): per-record checks and system placement run at the edit, the rows for the list as a whole (lines 336 to 342) change because a plain table is refused outright, and cross-record checks such as tool-call pairing still run at the `models.loop` call;
  - a passage on `replace(first, last, records...)` and its bounds, and on record views: a record read from a list is read-only, its `type()` is `userdata`, and changes go through `replace`.
- Changes, in the same repository, file `src/language/17-quick-reference.md`, the `messages` table (lines 83 to 97): add `list:replace`, `#list`, and `list[i]` rows; `list:append` validates the record and drops extra fields instead of appending it unchanged; the `record.*` rows read a read-only view.
- Verify: from `c:\Users\Vinnie\cursor\promptforge-docs`, `cargo xtask site --books-only` builds cleanly, per `CONTRIBUTING.md` (lines 5 to 12).
- Commit, in `promptforge-docs`: `Describe the Rust-backed messages list`.

</execution-plan>
