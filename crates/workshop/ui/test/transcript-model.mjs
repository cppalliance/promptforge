// The agent transcript model (src/parts/agent/transcript/transcript-model.ts
// and tool-labels.ts), pure and DOM-free: session items in, turns, rows,
// and a tail status out. Covers the turn rules, every grouping case, the
// stable row keys, the group summary grammar and plurals, tool verbs and
// details, the loading rule, thought labels (including title-only
// thinking), and every tail-status rule. Run: node test/transcript-model.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import * as esbuild from "esbuild";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { buildTranscript, thoughtLabel } from "./src/parts/agent/transcript/transcript-model.ts";
      export { toolLabel, toolKind, isToolLoading, groupSummary } from "./src/parts/agent/transcript/tool-labels.ts";
    `,
    resolveDir: path.join(testDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});

const bundlePath = path.join(os.tmpdir(), "promptforge-transcript-model-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { buildTranscript, thoughtLabel, toolLabel, toolKind, isToolLoading, groupSummary } =
  await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}
function same(name, actual, expected) {
  if (!isDeepStrictEqual(actual, expected)) {
    failures.push(`${name}: got ${JSON.stringify(actual)}, wanted ${JSON.stringify(expected)}`);
  }
}

// --- Item builders ------------------------------------------------------------

const user = (text) => ({ kind: "user", text });
const reply = (text, n, pending = false) => ({ kind: "reply", text, model: null, pending, reply: n });
const thought = (text, n, { pending = false, startedAt = 0, endedAt = null } = {}) => ({
  kind: "reasoning",
  text,
  model: null,
  pending,
  reply: n,
  startedAt,
  endedAt,
});
// A thought that ran `ms` milliseconds and settled.
const thoughtFor = (text, n, ms) => thought(text, n, { startedAt: 1000, endedAt: 1000 + ms });
const call = (id, name, tool, args = {}) => ({ id, name, tool, args: JSON.stringify(args) });
const batch = (...calls) => ({ kind: "tool-call", calls, text: "", model: null });
const result = (id, text = "ok") => ({ kind: "tool-result", toolCallId: id, text });
const failure = (message) => ({ kind: "error", message, title: null });

const search = (id, query = "q") => call(id, "search", "web/search", { query });
const fetchCall = (id, url = "https://example.com/") => call(id, "fetch", "web/fetch", { url });
const generic = (id, name = "read", tool = "fs/read") => call(id, name, tool, { path: "a" });
const ask = (id) => call(id, "ask", "user-input/ask", { question: "?" });

const rowsOf = (model, turn = 0) => model.turns[turn].rows;
const kindsOf = (model, turn = 0) => rowsOf(model, turn).map((row) => row.kind);
const keysOf = (model, turn = 0) => rowsOf(model, turn).map((row) => row.key);

// --- Turns ------------------------------------------------------------------

{
  const none = buildTranscript([], false, false);
  check("no items give no turns and no tail", none.turns.length === 0 && none.tail === null);

  const model = buildTranscript(
    [user("one"), reply("a", 0), user("two"), reply("b", 1)],
    false,
    false,
  );
  check("each user item starts a turn", model.turns.length === 2);
  same("a turn's rows start with its human row", [kindsOf(model, 0), kindsOf(model, 1)], [
    ["human", "markdown"],
    ["human", "markdown"],
  ]);
  same("human rows key by item index, replies by reply id", [keysOf(model, 0), keysOf(model, 1)], [
    ["h:0", "r:0"],
    ["h:2", "r:1"],
  ]);
  check("a human row holds the text byte-exact", rowsOf(model, 0)[0].text === "one");

  const lead = buildTranscript([reply("hello", 0), user("x")], false, false);
  check(
    "items before the first user item form a turn with no human row",
    lead.turns.length === 2 && rowsOf(lead, 0)[0].kind === "markdown",
  );

  const frozen = Object.freeze([Object.freeze(user("x")), Object.freeze(reply("y", 0))]);
  check("the model never mutates its input", buildTranscript(frozen, true, false).turns.length === 1);
}

// --- Tool results attach by id within the turn and never become rows -----------

{
  const items = [
    user("go"),
    batch(generic("c1")),
    result("c1", "file body"),
    reply("done", 1),
    user("again"),
    batch(generic("c1")),
  ];
  const model = buildTranscript(items, true, false);
  const first = rowsOf(model, 0)[1];
  check("a lone tool call becomes a tool row", first.kind === "tool");
  check(
    "its result attached and stopped it loading",
    first.step.result === "file body" && first.step.loading === false,
  );
  check("a result never becomes a row", kindsOf(model, 0).join() === "human,tool,markdown");
  const second = rowsOf(model, 1)[1];
  check(
    "a recycled id in another turn does not take the earlier turn's result",
    second.step.result === null && second.step.loading === true,
  );
  check(
    "a result with no call, or no id, is ignored",
    buildTranscript([user("x"), result("zzz"), { kind: "tool-result", toolCallId: null, text: "t" }], false, false)
      .turns[0].rows.length === 1,
  );
}

// --- Grouping ---------------------------------------------------------------------

{
  // One tool call and nothing else: a tool row keyed by turn and call id.
  const one = buildTranscript([user("q"), batch(generic("c1")), result("c1")], false, false);
  same("one call becomes a tool row", kindsOf(one), ["human", "tool"]);
  same("a tool step keys by turn and call id", keysOf(one), ["h:0", "c:0:c1"]);

  // Thinking only becomes a thought row keyed by its first thought.
  const thinking = buildTranscript([user("q"), thoughtFor("hmm", 0, 2000)], false, false);
  same("thinking only becomes a thought row", kindsOf(thinking), ["human", "thought"]);
  same("a thought keys by reply id", keysOf(thinking), ["h:0", "t:0"]);
  check("a thought row holds each thought", rowsOf(thinking)[1].thoughts.length === 1);

  // Thinking plus a call: a group that takes its first step's key.
  const mixed = buildTranscript(
    [user("q"), thoughtFor("hmm", 0, 2000), batch(generic("c1")), result("c1")],
    false,
    false,
  );
  same("thinking plus a call becomes a group", kindsOf(mixed), ["human", "group"]);
  same("a group takes its first step's key", keysOf(mixed), ["h:0", "t:0"]);
  same(
    "group steps keep their order",
    rowsOf(mixed)[1].steps.map((step) => step.step),
    ["thought", "tool"],
  );

  // A batch splits into one step per call; two calls make a group.
  const two = buildTranscript(
    [user("q"), batch(search("c1"), fetchCall("c2")), result("c1"), result("c2")],
    false,
    false,
  );
  same("a two-call batch is one group", kindsOf(two), ["human", "group"]);
  same("the group takes the first call's key", keysOf(two), ["h:0", "c:0:c1"]);
  same("each call is its own step", rowsOf(two)[1].steps.map((step) => step.key), ["c:0:c1", "c:0:c2"]);

  // Rounds append to the open group.
  const rounds = buildTranscript(
    [
      user("q"),
      thoughtFor("a", 0, 1000),
      batch(generic("c1")),
      result("c1"),
      thoughtFor("b", 1, 1000),
      batch(generic("c2")),
      result("c2"),
    ],
    false,
    false,
  );
  same("rounds append to one open group", kindsOf(rounds), ["human", "group"]);
  check("the group holds all four steps", rowsOf(rounds)[1].steps.length === 4);

  // A reply closes the group; the next round opens another.
  const split = buildTranscript(
    [
      user("q"),
      batch(search("c1"), search("c2")),
      result("c1"),
      result("c2"),
      reply("so far", 1),
      batch(generic("c3")),
      result("c3"),
      batch(generic("c4")),
      result("c4"),
    ],
    false,
    false,
  );
  same("a reply closes the open group", kindsOf(split), ["human", "group", "markdown", "group"]);

  // An error closes the group and renders no row of its own.
  const errored = buildTranscript(
    [user("q"), batch(generic("c1")), result("c1"), failure("boom"), batch(generic("c2")), result("c2")],
    false,
    false,
  );
  same("an error closes the group and is not a row", kindsOf(errored), ["human", "tool", "tool"]);

  // A user item closes the group by starting the next turn.
  const turns = buildTranscript(
    [user("q"), batch(generic("c1")), result("c1"), user("r"), batch(generic("c2")), result("c2")],
    false,
    false,
  );
  same("a user item closes the group", [kindsOf(turns, 0), kindsOf(turns, 1)], [
    ["human", "tool"],
    ["human", "tool"],
  ]);
  same("a tool step keys by its turn number", [keysOf(turns, 0)[1], keysOf(turns, 1)[1]], [
    "c:0:c1",
    "c:1:c2",
  ]);
}

// --- Ask-tool calls are always standalone ------------------------------------------

{
  const model = buildTranscript(
    [
      user("q"),
      batch(search("c1"), fetchCall("c2"), ask("c3")),
      result("c1"),
      result("c2"),
      result("c3", "the answer"),
      batch(generic("c4")),
      result("c4"),
    ],
    false,
    false,
  );
  same("an ask call closes the group and stands alone", kindsOf(model), [
    "human",
    "group",
    "tool",
    "tool",
  ]);
  check(
    "the ask row holds the ask step",
    rowsOf(model)[2].step.label.action === "Asked questions" &&
      rowsOf(model)[2].step.result === "the answer",
  );

  const lone = buildTranscript([user("q"), batch(ask("c1"))], false, false);
  same("a lone ask is a tool row", kindsOf(lone), ["human", "tool"]);
  const withThought = buildTranscript(
    [user("q"), thoughtFor("hm", 0, 1000), batch(ask("c1"))],
    false,
    false,
  );
  same("thinking before an ask settles as its own thought row", kindsOf(withThought), [
    "human",
    "thought",
    "tool",
  ]);
}

// --- Keys: stable across pending and durable, unique within a turn ----------------

{
  const pending = buildTranscript(
    [user("q"), thought("hm", 3, { pending: true }), reply("par", 3, true)],
    true,
    false,
  );
  const settled = buildTranscript(
    [user("q"), thoughtFor("hmm", 3, 1500), reply("partial done", 3)],
    true,
    false,
  );
  same("pending and durable rows share keys", keysOf(pending), keysOf(settled));
  same("the keys are the reply ids", keysOf(settled), ["h:0", "t:3", "r:3"]);

  // A recycled call id inside one turn gets a distinct key.
  const recycled = buildTranscript(
    [user("q"), batch(generic("call_1")), result("call_1"), batch(generic("call_1")), result("call_1", "second")],
    false,
    false,
  );
  const keys = rowsOf(recycled)[1].steps.map((step) => step.key);
  check("recycled ids within a turn keep unique keys", keys.length === 2 && keys[0] !== keys[1]);
  check("the first occurrence keeps the plain key", keys[0] === "c:0:call_1");
  check(
    "each recycled call takes its own result",
    rowsOf(recycled)[1].steps[0].result === "ok" && rowsOf(recycled)[1].steps[1].result === "second",
  );

  // A call with no id falls back to the item index (and position in the batch).
  const noId = buildTranscript([user("q"), batch(generic(""), generic(""))], false, false);
  const noIdKeys = rowsOf(noId)[1].steps.map((step) => step.key);
  check(
    "calls with no id key by item index, distinct per call",
    noIdKeys[0] !== noIdKeys[1] && noIdKeys.every((key) => key.startsWith("c:0:") && key.includes("1")),
  );

  // Growth keeps every earlier key.
  const base = [user("q"), reply("a", 0), user("r")];
  const before = buildTranscript(base, true, false);
  const after = buildTranscript([...base, thought("hm", 1, { pending: true })], true, false);
  same("appending items keeps earlier keys", keysOf(after, 0), keysOf(before, 0));

  // Rows with nothing to show are dropped.
  const blank = buildTranscript([user("q"), reply("  \n", 0, true), thoughtFor("  ", 1, 1000)], true, false);
  same("blank replies and settled blank thoughts render no row", kindsOf(blank), ["human"]);
}

// --- Group summary: verb, parts, plurals, order -------------------------------------

{
  const summaryOf = (calls, { generating = false, tail = [] } = {}) => {
    const model = buildTranscript(
      [user("q"), batch(...calls), ...calls.map((c) => result(c.id)), ...tail],
      generating,
      false,
    );
    return rowsOf(model)[1].summary;
  };
  same("two searches", summaryOf([search("a"), search("b")]), { action: "Explored", details: "2 searches" });
  same("one search and one fetch", summaryOf([search("a"), fetchCall("b")]), {
    action: "Explored",
    details: "1 search, 1 fetch",
  });
  same("fetch plurals", summaryOf([fetchCall("a"), fetchCall("b"), fetchCall("c")]).details, "3 fetches");
  same("one tool", summaryOf([search("a"), generic("b")]).details, "1 search, 1 tool");
  same(
    "parts run search, fetch, tool whatever the call order",
    summaryOf([generic("a"), generic("b"), fetchCall("c"), search("d"), search("e"), generic("f")]).details,
    "2 searches, 1 fetch, 3 tools",
  );
  check("the parts have no 'and'", !summaryOf([search("a"), fetchCall("b"), generic("c")]).details.includes("and"));

  const withThinking = buildTranscript(
    [user("q"), thoughtFor("a", 0, 1000), batch(search("a"), generic("b")), result("a"), result("b")],
    false,
    false,
  );
  check(
    "thinking steps are not counted",
    rowsOf(withThinking)[1].summary.details === "1 search, 1 tool",
  );

  check(
    "the verb is Exploring while the group is the last row of a generating turn",
    summaryOf([search("a"), search("b")], { generating: true }).action === "Exploring",
  );
  check(
    "the verb is Explored once the turn stops generating",
    summaryOf([search("a"), search("b")], { generating: false }).action === "Explored",
  );
  check(
    "the verb is Explored when a row follows the group",
    summaryOf([search("a"), search("b")], { generating: true, tail: [reply("hi", 5, true)] }).action ===
      "Explored",
  );
  const earlier = buildTranscript(
    [user("q"), batch(search("a"), search("b")), result("a"), result("b"), user("next")],
    true,
    false,
  );
  check("a group in an earlier turn is Explored", rowsOf(earlier, 0)[1].summary.action === "Explored");

  // The exported summary helper alone.
  same("groupSummary builds its parts", groupSummary({ searches: 1, fetches: 2, tools: 1 }, true), {
    action: "Exploring",
    details: "1 search, 2 fetches, 1 tool",
  });
  same("groupSummary omits zero parts", groupSummary({ searches: 0, fetches: 0, tools: 2 }, false), {
    action: "Explored",
    details: "2 tools",
  });
}

// --- Tool verbs and details ------------------------------------------------------------

{
  same("tool kinds", [toolKind("web/search"), toolKind("web/fetch"), toolKind("user-input/ask"), toolKind("fs/read"), toolKind(null)], [
    "search",
    "fetch",
    "ask",
    "other",
    "other",
  ]);
  same("web/search running", toolLabel(search("a", "rust async"), true), {
    action: "Searching web",
    details: "rust async",
    callName: null,
  });
  same("web/search done", toolLabel(search("a", "rust async"), false), {
    action: "Searched web",
    details: "rust async",
    callName: null,
  });
  same("web/fetch running", toolLabel(fetchCall("a", "https://x.dev/p"), true), {
    action: "Fetching page",
    details: "https://x.dev/p",
    callName: null,
  });
  same("web/fetch done", toolLabel(fetchCall("a", "https://x.dev/p"), false).action, "Fetched page");
  same("ask running and done", [toolLabel(ask("a"), true), toolLabel(ask("a"), false)], [
    { action: "Asking questions", details: "", callName: null },
    { action: "Asked questions", details: "", callName: null },
  ]);
  same("a generic tool names the call and its namespace", toolLabel(generic("a", "read", "fs/read"), false), {
    action: "Ran",
    details: "read in fs",
    callName: "read",
  });
  same("a generic tool running", toolLabel(generic("a", "read", "fs/read"), true).action, "Running");
  same("a call with no tool id shows its name alone", toolLabel(call("a", "lua_helper", null), false), {
    action: "Ran",
    details: "lua_helper",
    callName: "lua_helper",
  });
  same(
    "malformed or missing arguments give empty details",
    [
      toolLabel({ id: "a", name: "search", tool: "web/search", args: "not json" }, false).details,
      toolLabel({ id: "a", name: "search", tool: "web/search", args: "" }, false).details,
      toolLabel({ id: "a", name: "search", tool: "web/search", args: '{"query": 5}' }, false).details,
      toolLabel({ id: "a", name: "fetch", tool: "web/fetch", args: "[]" }, false).details,
    ],
    ["", "", "", ""],
  );
}

// --- Loading ----------------------------------------------------------------------------

{
  same(
    "loading: no result and a generating last turn",
    [
      isToolLoading("other", false, { last: true, generating: true }),
      isToolLoading("other", true, { last: true, generating: true }),
      isToolLoading("other", false, { last: true, generating: false }),
      isToolLoading("other", false, { last: false, generating: false }),
    ],
    [true, false, false, false],
  );
  same(
    "loading: an ask without a result waits even while generating is off",
    [
      isToolLoading("ask", false, { last: true, generating: false }),
      isToolLoading("ask", true, { last: true, generating: false }),
      isToolLoading("ask", false, { last: false, generating: false }),
    ],
    [true, false, false],
  );

  const waiting = buildTranscript([user("q"), batch(ask("c1"))], false, false);
  check("an unanswered ask row is loading with generating off", rowsOf(waiting)[1].step.loading === true);
  const running = buildTranscript([user("q"), batch(search("c1"))], true, false);
  check("a running search row is loading", rowsOf(running)[1].step.loading === true);
  const stopped = buildTranscript([user("q"), batch(search("c1"))], false, false);
  check("a call with no result stops loading when generating stops", rowsOf(stopped)[1].step.loading === false);
}

// --- Thought labels ------------------------------------------------------------------------

{
  const label = (text, durationMs, streaming = false) =>
    thoughtLabel([{ text, durationMs, streaming }]);
  same("streaming thinking", label("hm", null, true), { action: "Thinking", details: null });
  same("unknown duration", label("hm", null), { action: "Thought", details: "briefly" });
  same("zero duration", label("hm", 0), { action: "Thought", details: "briefly" });
  same("under 500ms", label("hm", 499), { action: "Thought", details: "briefly" });
  same("4 seconds", label("hm", 4000), { action: "Thought", details: "4s" });
  same("300 seconds, never minutes", label("hm", 300000), { action: "Thought", details: "300s" });
  same("rounds to the nearest second", label("hm", 4400), { action: "Thought", details: "4s" });
  check("the label never says 'for'", !label("hm", 4000).details.includes("for"));

  // Title-only thinking: the last title replaces "Thought".
  same("a bold title replaces Thought", label("**Planning the fix**", 4000), {
    action: "Planning the fix",
    details: "4s",
  });
  same("a heading title replaces Thought", label("# Checking files\n\n", 2000), {
    action: "Checking files",
    details: "2s",
  });
  same("the last title wins", label("**First**\n\n## Second\n", 3000).action, "Second");
  same("title-only details are none under 500ms", label("**Quick**", 300), { action: "Quick", details: null });
  same("title-only details are none when the duration is unknown", label("**Quick**", null), {
    action: "Quick",
    details: null,
  });
  same("a title plus body text is not title-only", label("**Plan**\nsome real thinking", 4000), {
    action: "Thought",
    details: "4s",
  });
  same("two bold spans on one line are body text, not one title", label("**Option A** vs **Option B**", 4000), {
    action: "Thought",
    details: "4s",
  });
  same("streaming title-only thinking still says Thinking", label("**Plan**", null, true), {
    action: "Thinking",
    details: null,
  });

  // A title-only thought under a second shows one decimal, reached through the label.
  same("title-only at 600ms shows 0.6s", label("**Quick**", 600), { action: "Quick", details: "0.6s" });
  same("title-only at the 500ms floor shows 0.5s", label("**Quick**", 500), { action: "Quick", details: "0.5s" });
  same("title-only just under a second truncates, never 1.0s", label("**Quick**", 999).details, "0.9s");
  same("title-only at a second is whole seconds", label("**Quick**", 1000).details, "1s");
  same("title-only with whole seconds", label("**Quick**", 3200).details, "3s");
  same("plain thinking at 600ms rounds to whole seconds", label("hm", 600), { action: "Thought", details: "1s" });

  // A thought row sums its thoughts' durations.
  const summed = buildTranscript(
    [user("q"), thoughtFor("one", 0, 2000), thoughtFor("two", 1, 3000)],
    false,
    false,
  );
  const row = rowsOf(summed)[1];
  check("consecutive thinking is one thought row", row.kind === "thought" && row.thoughts.length === 2);
  same("a thought row's duration is the sum", row.label, { action: "Thought", details: "5s" });
  check("a thought row's text joins its thoughts", row.text === "one\n\ntwo");
  const titleRow = rowsOf(buildTranscript([user("q"), thoughtFor("**Reading**", 0, 2000)], false, false))[1];
  same("a title-only thought row's label carries the title", titleRow.label, { action: "Reading", details: "2s" });

  // Streaming follows endedAt, so a closed-but-pending thought has settled.
  const closed = buildTranscript(
    [user("q"), thought("hm", 0, { pending: true, startedAt: 100, endedAt: 4100 })],
    true,
    false,
  );
  check("a pending thought whose thinking ended is not streaming", rowsOf(closed)[1].streaming === false);
  same("and labels as settled", rowsOf(closed)[1].label, { action: "Thought", details: "4s" });
  const open = buildTranscript([user("q"), thought("hm", 0, { pending: true, startedAt: 100 })], true, false);
  check("a pending thought still thinking is streaming", rowsOf(open)[1].streaming === true);
  same("and labels as Thinking", rowsOf(open)[1].label, { action: "Thinking", details: null });

  // Inner thinking steps carry their own label.
  const inGroup = buildTranscript(
    [user("q"), thoughtFor("a", 0, 4000), batch(generic("c1")), result("c1")],
    false,
    false,
  );
  same("a group's thinking step labels itself", rowsOf(inGroup)[1].steps[0].label, {
    action: "Thought",
    details: "4s",
  });
}

// --- Tail status ----------------------------------------------------------------------------

{
  const tailOf = (items, generating = true, reconnecting = false) =>
    buildTranscript(items, generating, reconnecting).tail;

  check("no tail when not generating", tailOf([user("q")], false) === null);
  check("no tail with no items at all", tailOf([], true) === null);
  same("just after the user sends, the tail plans", tailOf([user("q")]), {
    kind: "planning",
    action: "Planning next moves",
    details: "",
    callName: null,
    inGroup: false,
  });
  check(
    "no tail while the last row is streaming assistant text",
    tailOf([user("q"), reply("Hel", 0, true)]) === null,
  );
  check(
    "no tail while streaming text follows a group",
    tailOf([user("q"), batch(search("c1")), result("c1"), reply("Hel", 1, true)]) === null,
  );
  check(
    "blank streaming text does not suppress the tail",
    tailOf([user("q"), reply(" ", 0, true)]) !== null,
  );

  // Reconnecting.
  same("reconnecting shows its own status", tailOf([user("q")], true, true), {
    kind: "reconnecting",
    action: "Reconnecting...",
    details: "",
    callName: null,
    inGroup: false,
  });
  check("the reconnecting text uses three ASCII dots", tailOf([user("q")], true, true).action === "Reconnecting...");
  check("reconnecting wins over a running tool", tailOf([user("q"), batch(search("c1")), batch(search("c2"))], true, true).kind === "reconnecting");
  check("reconnecting while idle shows nothing", tailOf([user("q")], false, true) === null);
  check(
    "streaming text suppresses even the reconnecting status",
    tailOf([user("q"), reply("Hel", 0, true)], true, true) === null,
  );

  // A running tool inside a group gives its loading verb and details.
  const insideGroup = tailOf([user("q"), batch(search("c1", "rust"), generic("c2")), result("c2")]);
  same("a running tool inside a group shows its verb and details", insideGroup, {
    kind: "tool",
    action: "Searching web",
    details: "rust",
    callName: null,
    inGroup: true,
  });
  const generic1 = tailOf([user("q"), batch(generic("c1", "read", "fs/read"), search("c2")), result("c2")]);
  check(
    "a generic running tool names its call",
    generic1.kind === "tool" && generic1.action === "Running" && generic1.details === "read in fs" && generic1.callName === "read",
  );

  // Suppressed: a standalone tool row already shows it.
  check(
    "a running standalone tool row suppresses the tail",
    tailOf([user("q"), batch(search("c1"))]) === null,
  );
  check(
    "a running ask row suppresses the tail",
    tailOf([user("q"), batch(ask("c1"))]) === null,
  );

  // Thinking.
  check(
    "a streaming thought row suppresses the tail",
    tailOf([user("q"), thought("hm", 0, { pending: true })]) === null,
  );
  same(
    "streaming thinking inside a group gives Thinking",
    tailOf([user("q"), batch(search("c1")), result("c1"), thought("hm", 1, { pending: true })]),
    {
      kind: "thinking",
      action: "Thinking",
      details: "",
      callName: null,
      inGroup: true,
    },
  );

  // Planning after finished work.
  const planning = tailOf([user("q"), batch(search("c1"), search("c2")), result("c1"), result("c2")]);
  check("finished tools in a group leave the tail at planning", planning.kind === "planning" && planning.inGroup === true);
  check(
    "settled thinking does not count as streaming",
    tailOf([user("q"), thoughtFor("hm", 0, 2000)]).kind === "planning",
  );
  check(
    "finished standalone work plans outside any group",
    tailOf([user("q"), batch(search("c1")), result("c1")]).inGroup === false,
  );
  check(
    "a settled reply row followed by nothing plans",
    tailOf([user("q"), reply("done", 0)]).kind === "planning",
  );

  // Only the last turn counts.
  check(
    "an unfinished tool in an earlier turn does not drive the tail",
    tailOf([user("a"), batch(search("c1")), user("b")]).kind === "planning",
  );
}

if (failures.length > 0) {
  console.error(`transcript-model: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("transcript-model: all assertions passed");
process.exit(0);
