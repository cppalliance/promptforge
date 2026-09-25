The one crate a host program depends on to parse PromptForge prompt files and drive their runs.

PromptForge prompts are Markdown files that mix prose with Lua, and they reach models and tools only through the host that runs them. This crate runs them as a sans-I/O state machine. A run never opens a socket, touches a file, reads the clock, or starts a thread. It hands your program each piece of outside work as an *effect*, and it reports what happened as *events*. Your program performs the work however it likes, hands back the answers, and logs the events. That puts every model call, tool call, and timer under the host's control, which makes a run easy to test, easy to cancel, and deterministic.

By the end of this page you can parse a prompt, drive its run to the end, cancel it cleanly, prepare it against your deployment's tools and models, and read every possible result and error.

# What this crate is

Three ideas cover the whole crate.

**Parsing.** One call, [`Prompt::parse`], turns a prompt file's full source text into a reusable [`Prompt`]. It returns a pair. The first half is a [`Result`] that holds either the [`Prompt`] or a [`ParseError`]. The second half is a [`Vec`] of the parse-time [`Event`](crate::event::Event) values, which come back whether the parse succeeds or fails.

**Running.** A [`Run`] is a state machine that your program drives. You call [`Run::step`], perform each effect in the step, answer each effect through [`Run::resume`], and step again until the step is [`Step::Done`]. Each effect arrives inside [`Step::Pending`] as a tuple of an [`EffectId`](crate::effect::EffectId), a [`Provenance`](crate::ids::Provenance), and an [`Effect`](crate::effect::Effect). You hand the answer back under that same [`EffectId`](crate::effect::EffectId). The run performs no I/O itself, so every model call, tool call, store operation, user-input wait, and timer reaches you as an effect.

**Reporting.** Every event from a run arrives in the [`Step::Pending::events`](Step#variant.Pending.field.events) or [`Step::Done::events`](Step#variant.Done.field.events) vector of a step. You append them to your log in order. [`Step::Done`] holds the last events, and the run's own end boundary is among them. Events are for your log. The run only sees your log when an effect asks for part of it.

The crate is a facade. The root holds the run-facing types on this page, and fourteen topical modules hold the rest, each with its own page. Everything happens through calls from your program. There is nothing to configure outside it, and it needs no async runtime.

# PromptForge prompts in brief

A host developer rarely writes prompts, but it helps to know what the run is walking. This is the smallest complete working prompt:

````markdown
---
name: greeter
description: says hi
promptforge: 0
---

# Greeter

## Say hi

Say hello.
````

**Frontmatter.** A prompt file opens with a `---` line, then YAML, then a second `---` line. Every prompt sets `name:` and `description:`, and a runnable prompt also sets `promptforge:`, the format version. This build supports major version 0, so an author writes `promptforge: 0`. The parser rejects unknown keys. Four optional keys declare the prompt's contract with the host: `capabilities:`, `tools:`, `models:`, and `args:`. The host satisfies them before the run starts.

**The H1 and sections.** After the frontmatter comes exactly one non-empty level-1 heading, the prompt's title. Level-2 headings divide the body into named sections. Sections nest one level at a time, down to H6, and sibling names must be unique. An H4 directly under an H2 is rejected as an orphan.

**Prose blocks and Lua blocks.** A section body alternates between prose blocks and Lua fences. Only two fence forms are valid, tagged `lua` and `lua shared`. Prose on its own never calls a model. Prose written before a Lua block builds up in a pending buffer, and the next Lua block reads it as the read-only `prose` global. Nothing reaches a model until Lua sends it, for example with `models.infer(prose)`. Prose that no Lua block reads is commentary, and prose after a section's last Lua block is discarded. That is why the greeter prompt above never calls a model.

**The store.** Sections keep bulk state in the run-scoped `store`, a set of virtual files addressed by string paths and shared by every section of the run. Lua uses calls such as `store.write(path, text)` and `store.read(path)`. Each store operation reaches the host as an effect.

**`jump` and `call`.** `jump(heading)` transfers control to another section outright. `call(heading, input?)` runs another section as a subroutine and returns its return value. Both name the target with a heading reference such as `'## Help'`.

**Fanout.** `fanout(worker, collection)` runs a worker section once per member of a collection, concurrently. The collection is usually a list section read with `list_from_section`.

**Tools and models.** Tools come from capabilities. A prompt lists capability ids such as `promptforge/web` under `capabilities:`, and binds prompt-local aliases to exact tool paths such as `promptforge/web/fetch` under `tools:`. A prompt never names a concrete model. It declares roles under `models:` with keywords such as `thinking` or `fast`, and the host binds each role before the run.

This is orientation only. The full prompt language is in the separate language guide.

# Terms

The rest of the page uses four words freely.

- **Section**: a named heading in the prompt body. Each section runs in its own Lua state.
- **Chain**: one line of execution through the prompt. The main walk of a run is the root chain, whose id is `0`, and [`ChainId::root`](crate::ids::ChainId::root) renders as `"0"`. A `call` child or a spawned task gets a chain id that extends its parent's with a local child index, for example `0.2.1`.
- **Effect**: a piece of work for the host to perform, requested by the run. It arrives as an [`Effect`](crate::effect::Effect) paired with two ids. The [`EffectId`](crate::effect::EffectId) is an opaque run-wide handle that you pass back to [`Run::resume`]. The [`Provenance`](crate::ids::Provenance) identifies the task that built the effect.
- **Event**: a value reported by the run for the host to log, in order. Every event holds a [`Provenance`](crate::ids::Provenance). A host can fill a log record's columns from any event without matching on its variant: [`Event::execution`](crate::event::Event::execution) and [`Event::section`](crate::event::Event::section) name where it happened, and [`Event::provenance`](crate::event::Event::provenance) alone supplies the task id and sequence number.

# A first run

This program parses a one-section prompt, runs it with the argument `"world"`, performs the section's two store effects, and reads the result.

````
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::timestamp::Timestamp;
use promptforge::vfs::perform_store_op;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

let source = concat!(
    "---\n",
    "name: greeter\n",
    "description: says hi\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Greeter\n",
    "\n",
    "## Say hi\n",
    "\n",
    "```lua\n",
    "store.write('greeting.md', 'hello ' .. argv.prose)\n",
    "return store.read('greeting.md')\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "greeter");
let prompt = Arc::new(parsed?);

let started_at = Timestamp::from_unix_millis(951_782_400_000);
let ctx = RunContext::new("greeter", 7, started_at);
let mut run = Run::new(Arc::clone(&prompt), "world", ctx);

let mut log = Vec::new();
let result = loop {
    match run.step() {
        Step::Pending { effects, events } => {
            log.extend(events);
            for (id, _provenance, effect) in effects {
                let answer = match effect {
                    Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
                    _ => EffectAnswer::Dropped,
                };
                run.resume(id, answer);
            }
        }
        Step::Done { result, events } => {
            log.extend(events);
            break result;
        }
    }
};

match result {
    RunResult::Ok(text) => assert_eq!(text, "hello world"),
    other => panic!("the run should succeed: {other:?}"),
}
assert!(!log.is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **Build the source.** The prompt declares `promptforge: 0`. [`Prompt::parse`] accepts a prompt without it, but the run then ends on its first step with a failure. The section has one Lua block, which writes a store file and returns its contents. The example builds the source with [`concat!`] so each prompt line stays readable.
2. **Parse once.** [`Prompt::parse`] takes the source and an execution label for the parse events. The example ignores the events here. A parsed [`Prompt`] goes into an [`Arc`](std::sync::Arc), because [`Run::new`] takes an [`Arc`](std::sync::Arc) of a [`Prompt`]. One parse can back many runs, each with its own clone of the [`Arc`](std::sync::Arc).
3. **Build the context.** [`RunContext::new`] takes the run's name, a seed, and a start instant, all supplied by the host. The engine reads neither the OS clock nor the OS random number generator, so neither value has a default. A real host draws the seed from its own CSPRNG and stamps the start instant from its own clock. The start instant is a [`Timestamp`](crate::timestamp::Timestamp), here built with [`Timestamp::from_unix_millis`](crate::timestamp::Timestamp::from_unix_millis).
4. **Create the run.** [`Run::new`] takes the prompt, the argument string, and the context. It consumes the context, and [`RunContext`] is not [`Clone`], so each run needs its own. The argument string is one string, passed as is. The prompt reads it raw as `args` and parsed as `argv`. This prompt has no `args:` declaration, so `argv.prose` holds the whole string. The engine never validates the argument string. Pass `""` for no arguments.
5. **Drive the loop.** Each [`Run::step`] returns a [`Step`]. On [`Step::Pending`] the host logs the events, performs each effect, and answers it through [`Run::resume`]. This prompt issues only store effects, which the host performs with [`perform_store_op`](crate::vfs::perform_store_op). The catch-all arm gives up on any other effect with [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped).
6. **Read the result.** [`Step::Done`] holds the run's [`RunResult`]. A successful run ends with [`RunResult::Ok`], whose text is the last scalar Lua return, or `"done"` when no section returned one.

This run is capability-free. Its context came straight from [`RunContext::new`] and never went through [`Environment::prepare`], so the run has no tools and no models. That is fine for a prompt that never calls a model. A section that sends prose to a model with no model bound fails the run with [`RunErrorKind::Binding`]. The [Reference](#reference) section shows how to prepare a context with tools and models.

# The host loop

Every host drives a run with the same cycle.

1. Call [`Run::step`].
2. On [`Step::Pending`], commit the step's events to your log before performing any of its effects. A task that reads its own history then sees everything reported before the read.
3. Perform each effect on any executor or on the calling thread. Call [`Run::resume`] once per answer as each answer arrives, in any order.
4. Go back to step 1. On [`Step::Done`], log the events, read the [`Step::Done::result`](Step#variant.Done.field.result), and stop.

**Exactly one answer per effect.** Every issued effect receives exactly one answer, and [`Step::Done`] is withheld while any issued effect is unanswered. So the end of a run is also the end of every effect. An empty [`Step::Pending::effects`](Step#variant.Pending.field.effects) list means there is nothing new to perform, because every chain is waiting on an effect already issued. Answer what is still out, then step. The run catches answer bugs instead of panicking. An answer of the wrong kind, an answer for an id that the run never issued, or a second answer for one effect ends the run with [`RunErrorKind::Internal`]. The messages say "an effect's answer must be of the effect's own kind" and "an answer arrived for an effect the run did not issue or already answered". Calling [`Run::step`] again after [`Step::Done`] is also a host error, reported the same way. After [`Step::Done`], every answer is ignored.

**Giving up on an effect.** [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped) answers an effect without performing it. If a chain still waits on that effect, it resumes with a cancelled error. A drop counts as that effect's one answer. Dropping a `user_input()` wait ends the run as [`RunResult::Cancelled`].

**Cancelling.** [`Run::cancel`] sets the run's cancel flag. Running Lua stops from its instruction hook, even inside an endless loop, and the next [`Run::step`] tears every chain down. Cancelling doesn't end the run on the spot. That next step is [`Step::Pending`] with no new effects and the run's end boundary in its events. The host answers each effect still out with [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped) and steps again, and that step is [`Step::Done`] with [`RunResult::Cancelled`]. A host that already performed an effect before it learned of the cancel may deliver the real answer instead. The run discards it and counts it as that effect's one answer. To cancel from another thread, take a [`CancelHandle`](crate::cancel::CancelHandle) from [`Run::cancel_handle`] and call [`CancelHandle::cancel`](crate::cancel::CancelHandle::cancel) on it.

**Knowing when to stop.** [`Run::decided`] returns `true` once the outcome is settled, even while [`Step::Done`] still waits on outstanding answers. It is `false` for a fresh run and for a run waiting on an answer it still needs. Read it after each [`Step::Pending`]. Once it is `true`, stop performing effects and answer each held effect with [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped). Base this decision on [`Run::decided`], not on watching the events for an end event.

This example cancels a run while its store effect is still out:

````
use std::sync::Arc;

use promptforge::effect::EffectAnswer;
use promptforge::timestamp::Timestamp;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

let source = concat!(
    "---\n",
    "name: notes\n",
    "description: keeps a note\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Notes\n",
    "\n",
    "## Save\n",
    "\n",
    "```lua\n",
    "store.write('todo.md', 'ship it')\n",
    "return 'saved'\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "notes");
let ctx = RunContext::new("notes", 7, Timestamp::UNIX_EPOCH);
let mut run = Run::new(Arc::new(parsed?), "", ctx);

let Step::Pending { effects, .. } = run.step() else {
    panic!("the section waits on its store write");
};
let (held, _provenance, _effect) = effects.into_iter().next().ok_or("one effect")?;
assert!(!run.decided());

run.cancel();
let Step::Pending { effects, .. } = run.step() else {
    panic!("the held effect still needs its answer");
};
assert!(effects.is_empty());
assert!(run.decided());

run.resume(held, EffectAnswer::Dropped);
assert!(matches!(run.step(), Step::Done { result: RunResult::Cancelled, .. }));
assert!(run.decided());
# Ok::<(), Box<dyn std::error::Error>>(())
````

**Nothing to catch.** [`Run::step`], [`Run::resume`], and [`Run::cancel`] are infallible. A run's failures are values in [`RunResult::Failure`], so the host drives the loop without catching panics or errors from it, and the host owns the retry policy. The engine retries nothing. Startup failures follow the same rule. [`Run::new`] always returns a run, and a prompt that cannot start ends on its first step with [`Step::Done`]. The startup failures are an unsupported `promptforge:` version, which is [`RunErrorKind::Version`], a missing version, which is [`RunErrorKind::Parse`], and a failing store backend, which is [`RunErrorKind::Store`].

# How a run walks a prompt

The first [`Run::step`] starts the walk. The H1 body runs first as the preamble, a live pass with full host access. Then the top-level H2 sections run in file order. The first H2 is the entry point, and control falls through to the next section when one finishes. The preamble is where a prompt sets `models.default` and `tools.always`, and it is the only place where `argv` is writable. `call`, `jump`, `fanout`, and `list_from_section` fail in the preamble with "only available in sections". A failing Lua chunk in the H1 acts as a hard gate. It ends the run with [`RunErrorKind::RequirementsUnmet`], and the Lua error text becomes the notice.

**One Lua state per section.** Each section runs in its own sandboxed Lua state, created when the section starts and torn down when it ends. The state has only the `string`, `table`, and `math` libraries plus the safe base functions, so one section's Lua cannot leak into the next. `pairs` and `next` visit keys in a fixed sorted order. At most one `lua shared` fence is allowed, in the H1 body. It defines a shared library that runs as every section's first chunk, so its functions and globals are available in every section and every fanout arm. A fatal Lua error ends the run with [`RunErrorKind::Lua`], and exhausting a Lua host quota such as log events or instructions ends it with [`RunErrorKind::Quota`].

**What section Lua sees.** Besides `args` and `argv`, every section gets the `sys` runtime metadata table, the `log(...)` checkpoint function, and the scratch table `var`, which rolls forward from section to section. `sys.when` is the start instant given to [`RunContext::new`], rendered as RFC 3339 in every section and in the H1 pass. It is not a live clock. When a prompt declares structured `args:`, the argument string is parsed as JSON, so `{"query": "papers", "limit": 5}` arrives as `argv.query` and `argv.limit`. Unparseable input or a JSON `null` makes `argv` nil.

**The prose template.** When a Lua block reads `prose`, `{{ }}` placeholders are filled in one pass with values from the run, such as `{{ args }}`, `{{ argv.key }}`, `{{ var.key }}`, and `{{ sys.key }}`. With the argument `Acme Corp`, the prose `hi {{ args }}!` becomes `hi Acme Corp!`. No arithmetic is performed. A failing substitution ends the run with [`RunErrorKind::Substitution`]. A `---` line inside a section resets the pending buffer, so text above it never reaches `prose`. The `---` needs a blank line before it, or the line above becomes a heading.

**Model rounds.** `models.infer(prompt)` runs one tool-free model round. `models.loop(messages, compactor?)` runs the full model and tool loop over a message list built with `messages.new()`. Each model round and each tool call reaches the host as an effect.

**Returns.** A scalar `return` from any section's Lua block ends the whole run, and its value becomes the text of [`RunResult::Ok`]. If the first section returns `"first"`, a later `return "unreached"` never runs. A scalar return from the H1 pass skips every section.

**Moving control.** A section can move control only within its visible set: its sibling sections at the same level, and its own direct children. A heading reference with zero matches is a not-found error, and one with two matches is an ambiguity error. After a `jump`, the jumping section's remaining blocks never run, and only `var` crosses to the target. A `call` runs its target in a fresh Lua state. The child gets a clone of `var`, and its writes to the clone are discarded. `call('## Research', topic)` replaces `args` for the child chain. Nested `call` and `fanout` are capped at 8 levels. A failed `call` arrives in Lua as an ordinary error that `pcall` can catch.

**The store survives.** Section Lua state never outlives a section, but the store does. Every section of a run shares one store, so bulk state persists from section to section. The store also supports line-numbered reads, wildcard listing with `store.glob`, and an `untrusted(text)` wrapper that puts store content going back to a model inside a guard envelope.

# Determinism

A run is deterministic. The same run inputs with the same answers, replayed in order, produce the same effects and events. The run inputs are the seed and the start instant given to [`RunContext::new`], and the flags given to [`RunContext::flags`]. The run reproduces its nonces, `sys.when`, its effects, and its events. To make a run reproducible, a host records those inputs plus the [`EffectRecord`](crate::effect::EffectRecord) and [`AnswerRecord`](crate::effect::AnswerRecord) of every effect it performs. Replay itself is not built yet. The crate defines what to record, but nothing re-executes a log today.

**The host supplies all nondeterminism.** A live run draws its seed from a CSPRNG, because a predictable seed is a guessable nonce. The seed feeds the nonce of the untrusted envelope and any future random choice inside the run. The start instant comes from the host's own clock. The two inputs are independent. Changing the start instant leaves the nonce unchanged, and changing the seed leaves `sys.when` unchanged. No behavior flags are defined yet. Every run records [`Flags::EMPTY`](crate::replay::Flags::EMPTY), and the flags are a recorded input reserved for future use.

**Stable ids.** Two runs of the same prompt with the same inputs allocate the same chain, task, and entry ids however their chains interleave, because every counter is local to the chain that advances it.

**Provenance and effect ids.** Two runs with the same inputs and answers stamp the same [`Provenance`](crate::ids::Provenance) on the same effects and events. That makes [`Provenance`](crate::ids::Provenance) the replay key for matching a re-executed run against its recorded log. The [`EffectId`](crate::effect::EffectId) is an in-flight handle for [`Run::resume`] and need not reproduce across runs, so logs should match effects by [`Provenance`](crate::ids::Provenance).

**Events never steer.** Recording every event or dropping them all leaves a run's outputs, errors, and ordering unchanged.

# Concurrency

A run needs no async runtime. Because the run does no I/O itself, concurrency is whatever the host does with the effects in each step. A host may perform one step's effects in parallel and resume them in any order, on any executor or on the calling thread.

**Chains interleave at effect boundaries.** A chain runs until it needs an effect answered. Then it parks until [`Run::resume`] delivers the answer and a later [`Run::step`] queues it again. That is how one step can hand out several effects at once. Fanout arms are the usual source, because each arm's model round or tool call is its own effect.

**Fanout.** A list section is Lua-free and holds only items that start with `- `, `* `, `N. `, or `N) `. Fanout over an empty collection is an error. Inside each arm, the member is the `item` global, and `sys.index` is its 1-based position. Results come back in collection order, each with `.ok`, `.text`, `.item`, and `.exhausted`. At most 8 arms run at once by default, a limit set with [`RunLimits::max_fanout_concurrency`]. Each arm gets a fresh clone of the caller's `var`, while the store is shared. Two arms writing the same store path fail, but `store.append` to one path stays legal. A fatal error in one arm aborts its siblings. An arm whose tool loop ran out of iterations reports `.ok == false` and `.exhausted == true` instead. Within one run, two live execution identities claiming one store path end the run with [`RunErrorKind::Determinism`], which Lua cannot catch.

**Threads.** [`Run`] is [`Send`], so a run can move to another thread between calls. It is not [`Clone`], and one caller drives it at a time, because [`Run::step`] and [`Run::resume`] take `&mut self`. [`RunContext`], [`RunLimits`], [`Environment`], [`RunResult`], [`RunError`], and [`RunErrorKind`] are all [`Send`], [`Sync`], and `'static`, and so is the [`CancelHandle`](crate::cancel::CancelHandle) from [`Run::cancel_handle`]. The run shares one cancel flag with every section's Lua state and never creates per-task child handles.

# Reference

This part covers every item at the crate root. A typical host calls them in this order:

1. [`Prompt::parse`] the source.
2. [`RunContext::new`] plus its builders. Set [`RunContext::model`] before preparing, and also [`RunContext::vfs`] when the run shares a store with capabilities.
3. [`Environment::prepare`] the context against the prompt.
4. [`Requirements::merge`] the host's own capability-activation report into the report from prepare.
5. [`Requirements::refusal`], and fail the run here if it returns [`Some`].
6. [`Run::new`], then the host loop.

Three conventions hold across the root. [`RunContext`], [`RunLimits`], and [`Environment`] have builder methods that take `self` and return the updated value, so calls chain. Error and record enums are `#[non_exhaustive]`, so a `match` on them needs a wildcard arm. Each error type has a matchable kind: [`ParseError::kind`] returns a [`ParseErrorKind`], and [`RunError::kind`] returns a [`RunErrorKind`].

## Prompt

[`Prompt`] is a fully parsed prompt file: its frontmatter, its H1 title, its compiled Lua, and its section tree. The host reads the title and the frontmatter, and a [`Run`] uses the rest. The only way to get one is [`Prompt::parse`]. [`Prompt`] is [`Clone`].

[`Prompt::parse`] takes two arguments.

- `input`, a [`&str`](str), is the prompt file's full source text, passed as read. It must begin with a `---` line. A leading UTF-8 byte order mark is stripped, and both `\n` and `\r\n` line endings work. The frontmatter requires `name:` and `description:` and rejects unknown keys. Besides the four contract keys, it accepts `promptforge:`, `input:`, `output:`, and `max_tool_iterations:`, which must be positive and at most `1000`. The body must hold exactly one H1 with a non-empty title. A prompt with an H1 and no `##` sections parses and runs.
- `execution`, a [`&str`](str), is a label of the host's choosing. The parse stamps it on every parse event, so the parse events can be filed in the same log as the run. Passing the run's name is a common choice, but nothing requires it.

It returns a pair. The first half is a [`Result`] of a [`Prompt`] or a [`ParseError`]. The second half is a [`Vec`] of [`Event`](crate::event::Event) values, always returned, in order: [`Event::ParseStarted`](crate::event::Event::ParseStarted), the Lua compilation events for each compiled block, then [`Event::ParseSucceeded`](crate::event::Event::ParseSucceeded) or [`Event::ParseFailed`](crate::event::Event::ParseFailed). They are reported under task `0` with sequence numbers from zero, because no run exists yet. A host that logs them ahead of the run in one stream passes their count to [`RunContext::provenance_start`]. [`Prompt::parse`] performs no I/O and does not check `promptforge:`. A missing or unsupported version surfaces when the run starts.

The other methods read or adjust a parsed prompt.

- [`Prompt::frontmatter`] returns a reference to the parsed [`Frontmatter`](crate::prompt::Frontmatter), where the host reads the prompt's name, description, declared version, args, tools, capabilities, and model roles. The [`prompt`] module page covers it.
- [`Prompt::title`] returns the H1 title as a [`&str`](str), for example `"Greeter"` for `# Greeter`. It is never empty.
- [`Prompt::strip_h1_prose`] drops every prose block from the H1 and clears the description text, leaving the compiled H1 Lua blocks and the sections untouched. Use it to run a prompt's live H1 Lua without sending any H1 prose to a model. It takes `&mut self`, so call it before wrapping the prompt in an [`Arc`](std::sync::Arc), or call it on a clone.

````
use promptforge::event::Event;
use promptforge::{ParseErrorKind, Prompt};

let source = concat!(
    "---\n",
    "name: greeter\n",
    "description: says hi\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Greeter\n",
    "\n",
    "## Say hi\n",
    "\n",
    "Say hello.\n",
);
let (parsed, events) = Prompt::parse(source, "docs");
let prompt = parsed?;
assert_eq!(prompt.frontmatter().name(), "greeter");
assert_eq!(prompt.title(), "Greeter");
assert!(matches!(events.first(), Some(Event::ParseStarted { .. })));
assert!(matches!(events.last(), Some(Event::ParseSucceeded { .. })));

let (failed, events) = Prompt::parse("no frontmatter here", "docs");
let error = failed.err().ok_or("the parse fails")?;
assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
assert_eq!(error.name(), None);
assert!(matches!(events.last(), Some(Event::ParseFailed { .. })));
# Ok::<(), Box<dyn std::error::Error>>(())
````

## ParseError

[`ParseError`] explains why a prompt failed to parse. [`Prompt::parse`] returns it inside its [`Result`], and hosts never build one. Each accessor below takes no arguments and cannot fail.

- [`ParseError::kind`] returns the stable [`ParseErrorKind`]. Branch on it instead of matching message text.
- [`ParseError::line`] returns the 1-based file line as an [`Option`] of [`u32`], when known. A frontmatter YAML failure reports the YAML decoder's position converted to a file line. A Lua compile failure returns [`None`] and puts its position in the message.
- [`ParseError::column`] returns the 1-based column as an [`Option`] of [`u32`], when known.
- [`ParseError::span`] returns an [`Option`] of a `(start, end)` pair of [`usize`] byte offsets that mark the offending region in the source, for example a duplicate sibling section. It is always [`None`] for [`ParseErrorKind::Frontmatter`] and [`ParseErrorKind::Lua`].
- [`ParseError::name`] returns the prompt's frontmatter name as an [`Option`] of [`&str`](str). It is [`None`] for [`ParseErrorKind::Frontmatter`], because the name is not known yet, and for [`ParseErrorKind::Lua`]. When it is [`None`], the host uses its own label for the source.

[`ParseError`] implements [`Display`](std::fmt::Display) with the underlying diagnostic, such as "prompt requires an H1 title". It implements [`std::error::Error`], and its [`source`](std::error::Error::source) is the underlying cause, such as the YAML decode failure. There is no conversion from [`ParseError`] into [`RunError`], so a host reports parse failures separately from run failures.

## ParseErrorKind

[`ParseErrorKind`] is the matchable classification of a [`ParseError`], returned by [`ParseError::kind`]. It is `#[non_exhaustive]`. In every case the prompt author fixes the file, so the host reports the error with whatever location it has.

- [`ParseErrorKind::Frontmatter`]: the file does not start with a `---` line, never closes the frontmatter, has invalid YAML, has an unknown key, or lacks `name:` or `description:`.
- [`ParseErrorKind::Structure`]: the H1 is missing, there is more than one H1, or the H1 title is empty. It is also the fallback kind for parser-internal failures.
- [`ParseErrorKind::Fence`]: a Lua fence is misplaced or unclosed. That covers the removed `lua prompt` fence form, which fails with a message naming the two valid forms, a second `lua shared` fence, and a `lua shared` fence outside the H1.
- [`ParseErrorKind::List`]: a list-only section holds a non-list item or an empty item.
- [`ParseErrorKind::Lua`]: the shared library, an H1 block, or a section block is not valid Lua. The message names the section and block and includes the compiler diagnostic.

## SourceLocation

[`SourceLocation`] says where a run failed, as a prompt source position or a Rust code position. [`RunError::location`] returns one, and hosts never build one. All four fields are public.

- [`SourceLocation::path`], a [`String`], is the prompt's frontmatter name when the parse got that far, or the Rust source file for an internal fault. A frontmatter YAML failure happens before the name is known, so its path is the placeholder `"<prompt>"`, which the host replaces with its own label.
- [`SourceLocation::line`], an [`Option`] of [`u32`], is the 1-based line, when known. It is always [`Some`] for internal faults.
- [`SourceLocation::column`], an [`Option`] of [`u32`], is the 1-based column, when known. It is [`None`] for internal faults.
- [`SourceLocation::span`], an [`Option`] of a [`Range`](std::ops::Range) of [`usize`], is the byte span of the offending region in the prompt source. Only structured parse failures have one.

## RunContext

[`RunContext`] holds everything one run takes as input: its name, seed, start instant, limits, cancel flag, debug mode, UI snapshot, flags, current model, and filesystem handle. After [`Environment::prepare`], it also holds the run's tool catalog and its tool and model bindings. One context serves one run. It is neither [`Clone`] nor [`Default`], and [`Run::new`] consumes it.

[`RunContext::new`] takes three arguments.

- `name`, anything that converts [`Into`] a [`String`], is the run's identity. The run stamps it as the execution label on every event, and effects that name the execution use it too. Any label that helps you find the run in your logs works.
- `seed`, a [`u64`], is the run's source of randomness. The nonce of the untrusted envelope is derived from it. A live host draws it from a CSPRNG, and a replay passes the recorded value.
- `started_at`, a [`Timestamp`](crate::timestamp::Timestamp), is the instant the run began, which section Lua sees as `sys.when`. Build it from your own clock with [`Timestamp::from_unix_millis`](crate::timestamp::Timestamp::from_unix_millis), or use [`Timestamp::UNIX_EPOCH`](crate::timestamp::Timestamp::UNIX_EPOCH) in tests. The value `951_782_400_000` renders as `"2000-02-29T00:00:00Z"`.

The new context starts with a fresh cancel flag, no UI snapshot, [`DebugMode::Off`](crate::event::DebugMode::Off), [`RunLimits::new`], [`Flags::EMPTY`](crate::replay::Flags::EMPTY), a provenance start of `0`, no current model, an empty tool catalog, empty tool and model bindings, and a filesystem handle with a fresh in-memory store.

Each builder method takes the context by value plus one argument and returns the updated context. None of them can fail.

- [`RunContext::report_debug`] takes a [`DebugMode`](crate::event::DebugMode). With [`DebugMode::On`](crate::event::DebugMode::On), each model round's raw request and response bodies are reported as request and response events. [`DebugMode::Off`](crate::event::DebugMode::Off), the default, reports neither. The bodies already travel in the chat effect and its answer, so turn this on only when you want them in the event stream too.
- [`RunContext::cancel`] takes a [`CancelHandle`](crate::cancel::CancelHandle) and replaces the flag that [`RunContext::new`] minted. Pass a handle that the host keeps, such as a new one from [`CancelHandle::new`](crate::cancel::CancelHandle::new) or a child from [`CancelHandle::child`](crate::cancel::CancelHandle::child), so that cancelling the parent reaches this run. Without this call, the context's own flag is still reachable through [`RunContext::cancel_handle`].
- [`RunContext::limits`] takes the run's [`RunLimits`]. The default is [`RunLimits::new`].
- [`RunContext::ui`] takes a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html) snapshot of host state, taken at run start. Section Lua reads it through a `ui()` global. With a snapshot set, `models.get` also resolves an undeclared alias as a raw gateway catalog model id, so `models.loop(models.get(ui().selected_model), ...)` works without declaring the model. Without this call there is no `ui()` global and only declared aliases resolve. A change in host state takes effect on the next run.
- [`RunContext::flags`] takes the [`Flags`](crate::replay::Flags) for the run to record. A live run keeps the default [`Flags::EMPTY`](crate::replay::Flags::EMPTY). A replay passes the recorded set, built with [`Flags::from_bits`](crate::replay::Flags::from_bits).
- [`RunContext::provenance_start`] takes a [`u32`] that sets where the root task's provenance sequence starts. The default is `0`, for a run logged on its own. A host that logs the parse events ahead of the run in one stream passes the number of parse events, so every task and sequence pair in the stream is unique. Only the root task's counter moves, and spawned tasks count from zero.
- [`RunContext::model`] takes a [`ModelDescriptor`](crate::model::ModelDescriptor), the host's current model. Set it before [`Environment::prepare`], which binds every declared model role to it and checks each role against it. Without a current model, declared roles stay unbound and selecting one at run time fails. The [`model`] module page shows how to build a descriptor.
- [`RunContext::vfs`] takes a [`VfsRef`](crate::vfs::VfsRef), the run's filesystem handle, whose store mount backs every section's `store` table. Normally this is the handle from [`Environment::run_vfs`], which the host also hands to its capability activation so the capabilities and the run share one store. [`Environment::prepare`] keeps a handle set this way instead of building a new one. If the handle has no store mount, [`Run::new`] adds a fresh in-memory store there. If the mounted store backend fails its probe, the run's first step is [`Step::Done`] with [`RunErrorKind::Store`].

The remaining methods read the context back. Each takes `&self`, has no arguments, and cannot fail.

- [`RunContext::vfs_handle`] returns a reference to the run's [`VfsRef`](crate::vfs::VfsRef). Use it to seed files before the run and to extract output after it. [`Run::new`] consumes the context, so clone the handle first if you need it after the run.
- [`RunContext::current_model`] returns the [`ModelDescriptor`](crate::model::ModelDescriptor) set with [`RunContext::model`] as an [`Option`] of a reference, or [`None`].
- [`RunContext::cancel_handle`] returns a clone of the context's [`CancelHandle`](crate::cancel::CancelHandle). [`Run::new`] keeps the same flag, so this handle and the one from [`Run::cancel_handle`] are the same flag. A host hands it to its activated capabilities, so one cancel reaches them and the run.
- [`RunContext::model_bindings`] returns a reference to the [`ModelBindings`](crate::model::ModelBindings), which say which model each declared role is bound to. They are empty until the context is prepared with a current model.
- [`RunContext::tools`] returns a reference to the run's [`ToolCatalog`](crate::tools::ToolCatalog), a copy of the environment's catalog after [`Environment::prepare`]. It is empty on a context that was never prepared.
- [`RunContext::tool_bindings`] returns a reference to the [`ToolBindings`](crate::tools::ToolBindings), which say which tool descriptor each declared alias is bound to. They are empty on a context that was never prepared.
- [`RunContext::name`], [`RunContext::seed`], [`RunContext::run_flags`], and [`RunContext::started_at`] return the name as a [`&str`](str), the seed as a [`u64`], the [`Flags`](crate::replay::Flags), and the start [`Timestamp`](crate::timestamp::Timestamp), so the host can record them.
- [`RunContext::depth`] returns the prompt-tool nesting depth as a [`u32`]. It is always `0` today.

````
use std::num::NonZeroU32;

use promptforge::cancel::CancelHandle;
use promptforge::timestamp::Timestamp;
use promptforge::{RunContext, RunLimits};

let eight = NonZeroU32::new(8).ok_or("8 is non-zero")?;
let parent = CancelHandle::new();
let ctx = RunContext::new("example-run", 7, Timestamp::from_unix_millis(951_782_400_000))
    .limits(RunLimits::new().max_tool_iterations(eight))
    .cancel(parent.child());
assert_eq!(ctx.name(), "example-run");
assert_eq!(ctx.seed(), 7);
assert_eq!(ctx.started_at().to_rfc3339(), "2000-02-29T00:00:00Z");
assert_eq!(ctx.depth(), 0);

parent.cancel();
assert!(ctx.cancel_handle().is_cancelled());
# Ok::<(), Box<dyn std::error::Error>>(())
````

## Environment

[`Environment`] describes one deployment: the host roots mounted for every run, a nesting cap, and the catalog of tools available to runs. Build it once and share it across concurrent runs. It is [`Clone`], [`Send`], [`Sync`], and `'static`, and everything that changes per run sits on the [`RunContext`]. It holds tool descriptors only, and the tool implementations stay with the host.

[`Environment::new`] returns an environment with no host roots, a nesting cap of `3`, and an empty tool catalog. [`Environment::default`] returns the same thing. Three builder methods adjust it. Each takes the environment by value plus one argument, returns the updated environment, and cannot fail.

- [`Environment::base_vfs`] takes a [`VfsRef`](crate::vfs::VfsRef) of host roots, which every per-run filesystem mounts at `/`. It must hold host roots only, never the store mount. The default is an empty router. The base is shared by every run, so when two concurrent runs write the same host file, the second write fails with [`VfsError::Conflict`](crate::vfs::VfsError::Conflict).
- [`Environment::max_depth`] takes a [`u32`] cap on model-orchestrated prompt-tool nesting. The default is `3`. The cap is inert today. It is stored, but nothing reads it until the sub-run adapter lands, and [`RunContext::depth`] stays `0`. It is a different limit from the enforced cap of 8 on nested `call` and `fanout`.
- [`Environment::tools`] takes the [`ToolCatalog`](crate::tools::ToolCatalog) that runs bind against, assembled from the host's activated capabilities. The [`tools`] module page shows how to build one. The default is an empty catalog, and with it every exact tool slot's capability is reported missing.

[`Environment::run_vfs`] takes `&self` and returns a fresh per-run [`VfsRef`](crate::vfs::VfsRef): the base at `/` plus a fresh in-memory store. Each call returns a different store. A host that activates capabilities calls it first, hands the result to its activation, and sets it on the context with [`RunContext::vfs`].

[`Environment::prepare`] takes `&self` and two arguments, and returns a pair of a [`RunContext`] and a [`Requirements`]. It never fails, because problems are reported in the [`Requirements`].

- `prompt`, a reference to a [`Prompt`], is the prompt for the run to execute. Pass the same prompt that later goes to [`Run::new`].
- `ctx`, a [`RunContext`], is consumed. Build it with [`RunContext::new`], and set [`RunContext::model`] and, when needed, [`RunContext::vfs`] first. Other builders may be called before or after.

The returned context is enriched. Its filesystem handle is replaced by [`Environment::run_vfs`] unless the host set one. Its tool catalog is a copy of the environment's. Its tool bindings hold every exact tool slot filled from the catalog, where the first two segments of a tool path name its capability, so `promptforge/web/fetch` belongs to `promptforge/web`. A slot whose capability contributed no tools lands in [`Requirements::missing_required`]. A slot whose capability is in the catalog but did not contribute that tool is not reported and stays unbound, and advertising that alias fails at run time. With no current model, nothing is bound or checked. With one, every declared role is bound to it, even when a check fails. A role's `min_context` above the model's context window adds a [`RequirementCheck::ContextMinimum`] entry to [`Requirements::unmet_requirements`], and a mismatched hard keyword adds a [`RequirementCheck::HardKeyword`] entry. Soft keywords are never checked. [`Environment::prepare`] never reports conflicts. Because it takes `&self`, one environment prepares many runs.

## Requirements

[`Requirements`] is the preflight report of what the deployment still cannot satisfy for a prompt. [`Environment::prepare`] returns one. A host that builds its own capability-activation report starts from [`Requirements::default`], which has three empty lists, and pushes into the public fields. The struct is `#[non_exhaustive]`, so it cannot be built with a struct literal.

- [`Requirements::unmet_requirements`], a [`Vec`] of [`UnmetRequirement`], lists the model requirements that the bound model does not satisfy. Only [`Environment::prepare`] fills it, and it stays empty when no current model was set.
- [`Requirements::missing_required`], a [`Vec`] of [`CapabilityId`](crate::capabilities::CapabilityId), lists the required capabilities that the run cannot have. The host's activation adds a capability that is absent or failed to activate, and [`Environment::prepare`] adds the capability of an exact tool slot that contributed nothing to the catalog.
- [`Requirements::conflicts`], a [`Vec`] of [`CapabilityConflict`], lists pairs of present capabilities that cannot activate in one run. Neither member of a pair activates. Only the host's activation reports these.

The methods read and combine reports.

- [`Requirements::is_satisfied`] returns `true` when all three lists are empty.
- [`Requirements::merge`] takes `&mut self` and another [`Requirements`] by value, and folds it in. A host merges its activation report into the report from prepare, so a single refusal names every gap. A capability already in [`Requirements::missing_required`] is not repeated. Conflicts and unmet requirements are appended as they are.
- [`Requirements::refusal`] returns [`None`] when the report is satisfied. Otherwise it returns a [`RunError`] of kind [`RunErrorKind::RequirementsUnmet`], whose [`Display`](std::fmt::Display) text is exactly [`Requirements::notice`]. [`Run::new`] does not check requirements, so the host checks this before building the run and fails the run with the error instead. The error has no location, and it is neither cancelled nor retryable.
- [`Requirements::notice`] returns a [`String`] written for a model to read. It starts with `the environment cannot satisfy this prompt:` and adds one line per gap, each after a newline and `- `. Missing capabilities come first as `missing required capability: {id}`. Conflicts follow as `conflicting capabilities: {first} and {second} cannot be activated together; declare one or the other`. Unmet requirements come last, as `role '{role}': requires a context of at least {required} tokens; the current model provides {actual}` or `role '{role}': requires '{required}'; the current model's thinking capability is {actual}`. A satisfied report gives only the first line.

This prompt binds a tool slot, but the environment's catalog is empty:

````
use promptforge::capabilities::CapabilityId;
use promptforge::timestamp::Timestamp;
use promptforge::{CapabilityConflict, Environment, Prompt, Requirements, RunContext, RunErrorKind};

let source = concat!(
    "---\n",
    "name: fetcher\n",
    "description: fetches a page\n",
    "promptforge: 0\n",
    "tools:\n",
    "  fetch: promptforge/web/fetch\n",
    "---\n",
    "\n",
    "# Fetcher\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "fetcher");
let prompt = parsed?;
let ctx = RunContext::new("fetcher", 7, Timestamp::UNIX_EPOCH);
let (ctx, mut requirements) = Environment::new().prepare(&prompt, ctx);

let web = CapabilityId::parse("promptforge/web")?;
assert_eq!(requirements.missing_required, [web.clone()]);
assert!(!requirements.is_satisfied());
assert!(ctx.tool_bindings().is_empty());
assert_eq!(
    requirements.notice(),
    "the environment cannot satisfy this prompt:\n- missing required capability: promptforge/web",
);

let mut activation = Requirements::default();
activation.missing_required.push(web);
activation.conflicts.push(CapabilityConflict::new(
    CapabilityId::parse("acme/bashkit")?,
    CapabilityId::parse("acme/terminal")?,
));
requirements.merge(activation);
assert_eq!(requirements.missing_required.len(), 1);
assert_eq!(requirements.conflicts.len(), 1);

let refusal = requirements.refusal().ok_or("the report is unsatisfied")?;
assert_eq!(refusal.kind(), RunErrorKind::RequirementsUnmet);
assert_eq!(refusal.to_string(), requirements.notice());
# Ok::<(), Box<dyn std::error::Error>>(())
````

## RequirementCheck

[`RequirementCheck`] names which model check an [`UnmetRequirement`] failed. It is `#[non_exhaustive]`, and hosts only compare against its variants.

- [`RequirementCheck::ContextMinimum`]: the role's `min_context` exceeds the current model's context window. Pick a model with a larger context and prepare again, or refuse the run.
- [`RequirementCheck::HardKeyword`]: the current model does not satisfy a hard keyword. That is `thinking` against a model whose thinking mode is [`ThinkingMode::Never`](crate::model::ThinkingMode::Never), or `no-thinking` against one whose mode is [`ThinkingMode::Always`](crate::model::ThinkingMode::Always). Pick a model whose thinking mode fits and prepare again, or refuse the run.

## UnmetRequirement

[`UnmetRequirement`] describes one failed model requirement, with the required and actual values side by side. It arrives in [`Requirements::unmet_requirements`], and hosts never build one.

- [`UnmetRequirement::role`], a [`String`], is the role label declared under `models:`, for example `"analyst"`.
- [`UnmetRequirement::check`], a [`RequirementCheck`], says which check failed.
- [`UnmetRequirement::required`], a [`String`], is what the prompt required: the decimal context minimum, such as `"200000"`, or the hard keyword `"thinking"` or `"no-thinking"`.
- [`UnmetRequirement::actual`], a [`String`], is what the current model provides: its decimal context window, such as `"32000"`, or its thinking mode as `"Never"`, `"Always"`, `"Switchable"`, or `"unknown"`.

## CapabilityConflict

[`CapabilityConflict`] records two present capabilities that cannot activate in one run. The host's activation builds these and pushes them into [`Requirements::conflicts`]. The struct is `#[non_exhaustive]`, so the host builds one with [`CapabilityConflict::new`].

[`CapabilityConflict::new`] takes two [`CapabilityId`](crate::capabilities::CapabilityId) values and cannot fail. `first` is the capability declared earlier, and `second` is the one declared later, so the order matters. Build each with [`CapabilityId::parse`](crate::capabilities::CapabilityId::parse), or take them from the prompt's declarations.

- [`CapabilityConflict::first`], a [`CapabilityId`](crate::capabilities::CapabilityId), is the earlier-declared capability.
- [`CapabilityConflict::second`], a [`CapabilityId`](crate::capabilities::CapabilityId), is the later-declared capability.

[`Requirements::notice`] renders both with their [`Display`](std::fmt::Display) form.

## RunLimits

[`RunLimits`] sets a run's resource ceilings. The defaults are safe as they are, and [`RunContext::new`] installs them, so a host only builds [`RunLimits`] to change one. Start from [`RunLimits::new`], or from [`RunLimits::default`], which is the same, then call setters and install the result with [`RunContext::limits`].

Each setter takes the limits by value plus one value, returns the updated limits, and cannot fail. Most take a non-zero integer type, so zero cannot be expressed.

- [`RunLimits::max_tool_iterations`] takes a [`NonZeroU32`](std::num::NonZeroU32) cap on the model rounds in one section's tool-call loop. The default is 24. A prompt's frontmatter `max_tool_iterations:` overrides it for that prompt.
- [`RunLimits::max_fanout_concurrency`] takes a [`NonZeroUsize`](std::num::NonZeroUsize) cap on the fanout arms that run at once. The default is 8.
- [`RunLimits::max_response_bytes`] takes a [`NonZeroU64`](std::num::NonZeroU64) cap on the size of a model response body, in bytes. The default is 16 MiB.
- [`RunLimits::lua_memory_bytes`] takes a [`NonZeroUsize`](std::num::NonZeroUsize) cap on the memory of each section's Lua state, in bytes. The default is 64 MiB.
- [`RunLimits::lua_log_events`] takes a [`NonZeroU32`](std::num::NonZeroU32) cap on the `log` checkpoints in each section's Lua state. The default is 1024. Running out ends the run with [`RunErrorKind::Quota`].
- [`RunLimits::request_timeout`] takes a [`Duration`](std::time::Duration), the longest a model request waits for its next receive: first the response headers, then each body chunk. Every receive restarts the wait, so a stream that keeps arriving is never cut off. The default is 120 seconds. This setter takes a plain [`Duration`](std::time::Duration), so the type does not rule out zero, and the effect of a zero duration is unknown.

Each getter takes `&self` and returns the matching value: [`RunLimits::tool_iterations`], [`RunLimits::fanout_concurrency`], [`RunLimits::response_bytes`], [`RunLimits::lua_memory`], [`RunLimits::lua_logs`], and [`RunLimits::timeout`].

## Run

[`Run`] is one run of one prompt, the state machine at the center of the host loop. [`Run::new`] is its only constructor. It is [`Send`] but not [`Clone`].

[`Run::new`] takes three arguments and always returns a run that has not started. Nothing executes until the first [`Run::step`].

- `prompt`, an [`Arc`](std::sync::Arc) of a [`Prompt`], is the parsed prompt. Pass [`Arc::new`](std::sync::Arc::new) of the parse result, or a clone of an existing [`Arc`](std::sync::Arc) to run the same parse again. The prompt must declare `promptforge: 0` to start.
- `args`, a [`&str`](str), is the argument string, or `""` for none. [A first run](#a-first-run) and [How a run walks a prompt](#how-a-run-walks-a-prompt) describe how the prompt reads it.
- `ctx`, a [`RunContext`], is consumed. Pass the context from [`Environment::prepare`], or one straight from [`RunContext::new`] for a capability-free run.

A prompt that cannot start ends on the first step, as [`Step::Done`] with [`RunResult::Failure`] and no events. A version other than `0` gives [`RunErrorKind::Version`]. A missing version gives [`RunErrorKind::Parse`] with the message "not a promptforge prompt: no promptforge version". A failing store backend gives [`RunErrorKind::Store`]. [`Run::new`] does not check [`Requirements`].

The other methods drive and observe the run. None of them can fail.

- [`Run::step`] takes `&mut self` and returns a [`Step`]. The first call runs the H1 pass when the H1 has Lua blocks, and otherwise starts the section walk. It returns [`Step::Pending`] while any chain waits on an answer, and [`Step::Done`] once the run is over and every issued effect is answered. Stepping after [`Step::Done`] returns another [`Step::Done`] with [`RunErrorKind::Internal`] and the message "a finished run cannot be stepped again", or "a run that failed to start cannot be stepped again". If nothing is ready and nothing is pending, the run fails with an internal error instead of hanging.
- [`Run::resume`] takes `&mut self`, an [`EffectId`](crate::effect::EffectId), and an [`EffectAnswer`](crate::effect::EffectAnswer), and returns nothing. The id comes from the effect's tuple. The answer must be of the same kind as the effect, such as an [`EffectAnswer::Store`](crate::effect::EffectAnswer::Store) for an [`Effect::Store`](crate::effect::Effect::Store), or it can be [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped). The waiting chain is queued for the next step, and the answer's events are reported on that step. A bad answer ends the run with [`RunErrorKind::Internal`] on a later step. A fatal outcome of the answer itself also ends the run on a later step, with its own kind: a store claims conflict, for example, ends it with [`RunErrorKind::Determinism`]. When an unknown id ends the run, the effects still out become orphans, and the host still owes their answers before [`Step::Done`]. On a run that failed to start, [`Run::resume`] does nothing.
- [`Run::cancel`] takes `&mut self` and sets the run's cancel flag. [The host loop](#the-host-loop) describes the shutdown that follows. The flag is the context's flag, so capabilities that hold a handle from [`RunContext::cancel_handle`] see it too.
- [`Run::cancel_handle`] takes `&self` and returns a clone of the run's [`CancelHandle`](crate::cancel::CancelHandle). Another thread can call [`CancelHandle::cancel`](crate::cancel::CancelHandle::cancel) on it, or check [`CancelHandle::is_cancelled`](crate::cancel::CancelHandle::is_cancelled).
- [`Run::decided`] takes `&self` and returns a [`bool`]. It is `true` once the run's end boundary has been reported, or the run never started, and every effect still out is an orphan whose answer only [`Step::Done`] waits on. It stays `true` after [`Step::Done`].

## Step

[`Step`] is the outcome of one [`Run::step`]. The host receives it and never builds one.

- [`Step::Pending`]: the run continues. The host sees it whenever a chain still waits on an answer, including after [`Run::cancel`] while effects are still out. The host logs the events, performs the effects, answers each one, and steps again.
  - [`Step::Pending::effects`](Step#variant.Pending.field.effects) is a [`Vec`] of tuples, each an [`EffectId`](crate::effect::EffectId), a [`Provenance`](crate::ids::Provenance), and an [`Effect`](crate::effect::Effect), in issue order. The list may be empty.
  - [`Step::Pending::events`](Step#variant.Pending.field.events) is a [`Vec`] of [`Event`](crate::event::Event) values reported by this step, in order.
- [`Step::Done`]: the run is over. The host sees it only once every issued effect has been answered. The host logs the events, reads the result, and stops stepping.
  - [`Step::Done::result`](Step#variant.Done.field.result) is the [`RunResult`].
  - [`Step::Done::events`](Step#variant.Done.field.events) is a [`Vec`] of the [`Event`](crate::event::Event) values reported since the previous step. It includes the run's end boundary, [`Event::RunSucceeded`](crate::event::Event::RunSucceeded) or [`Event::RunFailed`](crate::event::Event::RunFailed), unless an earlier [`Step::Pending`] already reported it. It is empty for a run that failed to start.

## RunResult

[`RunResult`] is what a run produced, read from [`Step::Done::result`](Step#variant.Done.field.result). Every outcome is a value, including a prompt that declines the request, which is ordinary result text.

- [`RunResult::Ok`] holds a [`String`], the run's final text. It is the last scalar Lua return, or `"done"` when no section returned one. The variant shares its name with [`Result`]'s, so write [`RunResult::Ok`] in full where [`Result`] is also in scope.
- [`RunResult::Cancelled`] means the host cancelled the run, either through the cancel flag or by answering a waiting chain's effect with [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped). Treat it as a clean stop. It has no payload.
- [`RunResult::Failure`] holds a [`RunError`]. Match on [`RunError::kind`], show the [`Display`](std::fmt::Display) text to a person, use [`RunError::location`] to navigate, and check [`RunError::is_retryable`] before retrying.

## RunError

[`RunError`] explains why a run failed. It arrives in [`RunResult::Failure`] or from [`Requirements::refusal`], and hosts never build one. Each method takes `&self`, has no arguments, and cannot fail.

- [`RunError::kind`] returns the stable [`RunErrorKind`]. Match on it with a wildcard arm instead of matching message text.
- [`RunError::is_cancelled`] returns `true` only for a [`RunErrorKind::Cancelled`] error. The [`Run`] interface reports cancellation as [`RunResult::Cancelled`] instead, so an error from [`Step::Done`] normally returns `false`.
- [`RunError::is_retryable`] returns `true` when a retry may succeed: an HTTP failure, a malformed model response, a failure reading the backend body, or a backend status of 500 or above. It returns `false` for everything else, including statuses below 500.
- [`RunError::location`] returns an [`Option`] of a [`SourceLocation`]. A frontmatter YAML failure gives the path `"<prompt>"` with the YAML line and column. A structured parse failure, including the missing-version failure, gives the frontmatter name when known, with the line, column, and span when known. An internal fault gives the Rust file and line. Every other failure returns [`None`].

[`RunError`] implements [`Display`](std::fmt::Display) with the underlying message, which for [`RunErrorKind::RequirementsUnmet`] is exactly the refusal notice. It implements [`std::error::Error`], and the cause chain is reachable through [`source`](std::error::Error::source).

This prompt declares a version this build does not support:

````
use std::sync::Arc;

use promptforge::timestamp::Timestamp;
use promptforge::{Prompt, Run, RunContext, RunErrorKind, RunResult, Step};

let source = concat!(
    "---\n",
    "name: future\n",
    "description: needs a newer engine\n",
    "promptforge: 7\n",
    "---\n",
    "\n",
    "# Future\n",
    "\n",
    "## Only\n",
    "\n",
    "```lua\n",
    "return 'unreached'\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "future");
let ctx = RunContext::new("future", 7, Timestamp::UNIX_EPOCH);
let mut run = Run::new(Arc::new(parsed?), "", ctx);

let Step::Done { result: RunResult::Failure(error), events } = run.step() else {
    panic!("an unsupported version ends the first step");
};
assert_eq!(error.kind(), RunErrorKind::Version);
assert!(!error.is_retryable());
assert!(events.is_empty());

let Step::Done { result: RunResult::Failure(again), .. } = run.step() else {
    panic!("a run that failed to start stays done");
};
assert_eq!(again.kind(), RunErrorKind::Internal);
# Ok::<(), Box<dyn std::error::Error>>(())
````

## RunErrorKind

[`RunErrorKind`] is the matchable classification of a [`RunError`], returned by [`RunError::kind`]. It is `#[non_exhaustive]`, so new kinds can appear without breaking a `match` that has a wildcard arm.

- [`RunErrorKind::Parse`]: the prompt could not be parsed, or it declares no `promptforge:` version. [`RunError::location`] gives the source position. Report it to the author.
- [`RunErrorKind::Version`]: the prompt declares a `promptforge:` major other than `0`. The prompt needs a supported version or a newer engine.
- [`RunErrorKind::Binding`]: a tool or model could not be bound. A common cause is a section that sends prose to a model with neither `models.use` nor a prompt-wide `models.default`, reported as "model binding required for section ...". Fix the environment or the prompt's declarations.
- [`RunErrorKind::Completion`]: a model completion failed at the transport, backend, or decode layer. Check [`RunError::is_retryable`], because transient failures may succeed on retry.
- [`RunErrorKind::Tool`]: a tool failed, the model called a tool outside the section's advertised set, Lua called an alias that is not bound in the run, or the tool-call loop hit its iteration cap without a final reply. Raise [`RunLimits::max_tool_iterations`] if the cap was the cause, or fix the tool.
- [`RunErrorKind::Store`]: a store operation failed, including a store backend that fails its probe when the run starts. Inspect the backend or the operation.
- [`RunErrorKind::Determinism`]: two live execution identities claimed one store path, and the run ended at once to keep interleaving deterministic. Avoid concurrent runs or capabilities writing the same path.
- [`RunErrorKind::Lua`]: a section's Lua failed to run or to return a usable value, for example a runtime error or a misused task. Report it to the author.
- [`RunErrorKind::Quota`]: a Lua host quota ran out, such as log events, log bytes, or instructions. Raise the matching [`RunLimits`] value or fix the prompt.
- [`RunErrorKind::ContextExhausted`]: the compactor ran out of room in the model's context window. Use a model with a larger context or shorten the prompt's history.
- [`RunErrorKind::Input`]: the host's input handling failed a user-input wait. Inspect that handling.
- [`RunErrorKind::Substitution`]: a `{{ }}` substitution in prose failed. Report it to the author.
- [`RunErrorKind::Cancelled`]: the host cancelled the run. This kind exists only inside a run, and the [`Run`] interface reports cancellation as [`RunResult::Cancelled`], so a host normally never sees it.
- [`RunErrorKind::Internal`]: an internal invariant failed. The usual cause is a host loop error: stepping after [`Step::Done`], answering an unknown id, answering an effect twice, or answering with the wrong kind. [`RunError::location`] gives the Rust file and line. Fix the host loop, and otherwise report an engine bug.
- [`RunErrorKind::RequirementsUnmet`]: the environment cannot satisfy the prompt. The host gets it from [`Requirements::refusal`], or during a run when the prompt's H1 Lua fails. Satisfy the listed requirements or show the notice.

# Where to go next

The module pages, in reading order:

- [`prompt`]: what a parsed prompt declares in its frontmatter, from its name and description to its store files, capabilities, tool slots, typed args, and model roles.
- [`timestamp`]: the start instant of a run, built from signed Unix milliseconds and rendered as RFC 3339.
- [`cancel`]: the cancel flag, shared by cloning, arranged into parent and child handles, and set from any thread.
- [`effect`]: every kind of effect a run can hand out, and the answer for each.
- [`model`]: model identities and descriptors, binding a prompt's roles, building messages, and completion errors.
- [`transport`]: the chat-completions codec that builds a request body and reads the response stream through any HTTP client.
- [`tools`]: tool descriptors and ids, building a validated catalog, and answering tool calls from your own implementations.
- [`input`]: answering a user-input wait with the operator's text, or reporting that no operator is present.
- [`vfs`]: the virtual filesystem behind the store, its backends and mounts, and seeding and extracting run files.
- [`event`]: the events a parse and a run report, how to persist them, and which ones mark section and run boundaries.
- [`ids`]: chain ids, how `call` children and spawned tasks extend them, and provenance for ordering a log by task.
- [`metrics`]: token usage and timing for each model reply.
- [`replay`]: the behavior flags a host records beside the seed and start instant.
- [`capabilities`]: parsing a capability id and checking whether a tool id belongs to a capability.

*Claude Opus 5.5*
