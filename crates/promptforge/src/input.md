Answers to a user-input wait: the operator's text, a report that the host has no input, or a failure of the host's input source.

A prompt section can pause and ask a person for text by calling `user_input()`. The run has no way to reach a person, so it hands the question to your program as an effect, and your program replies with one of the values in this module. You decide where the question goes: a terminal, a chat window, a web form, or nowhere at all. By the end of this page you can answer an input wait with the operator's text, answer it when no operator exists, report a broken input source, cancel a wait, and log each wait with its answer.

# Where this fits

A section asks for input with one Lua call, which returns two values:

````lua
local text, available = user_input()
````

When a section makes that call, the run reports [`Event::UserInputWaitStarted`](crate::event::Event::UserInputWaitStarted) and then hands the host an [`Effect::UserInput`](crate::effect::Effect::UserInput) inside a [`Step::Pending`](crate::Step::Pending) returned by [`Run::step`](crate::Run::step). A host can use the event to show that the run is waiting on the operator.

The effect's two fields say who is asking. [`Effect::UserInput::execution`](crate::effect::Effect#variant.UserInput.field.execution) is the run's execution identifier, which is the `name` argument given to [`RunContext::new`](crate::RunContext::new). [`Effect::UserInput::section`](crate::effect::Effect#variant.UserInput.field.section) is the name of the section asking. A host that serves several runs or several operators uses the two fields to route the question to the right person.

The host answers through [`Run::resume`](crate::Run::resume) under the effect's [`EffectId`](crate::effect::EffectId). The answer is an [`EffectAnswer::UserInput`](crate::effect::EffectAnswer::UserInput), which holds a [`Result`] of an [`InputOutcome`] or an [`InputError`], or it is [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped). That gives four possible answers, and each one resumes the Lua call differently:

- [`InputOutcome::Text`] in [`Ok`] carries the operator's text. The call returns that text and `true`.
- [`InputOutcome::Unavailable`] in [`Ok`] says the host has no input to give. The call returns a fixed fallback sentence and `false`.
- An [`InputError`] in [`Err`] says the host's input source failed. The call raises an error.
- [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped) gives up on the wait. The call raises a cancelled error.

Any other answer kind, such as [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat), ends the run with [`RunErrorKind::Internal`](crate::RunErrorKind::Internal). The sections below take the four answers in turn.

# Answering with operator text

This host drives a prompt whose one section asks the operator a question and returns what it got back. The host routes the question by execution and section, and answers with the operator's text.

````
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::event::Event;
use promptforge::input::InputOutcome;
use promptforge::timestamp::Timestamp;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

fn ask_operator(execution: &str, section: &str) -> String {
    println!("run {execution} is waiting in section {section}");
    "hello operator".to_owned()
}

let source = concat!(
    "---\n",
    "name: asker\n",
    "description: asks the operator\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Asker\n",
    "\n",
    "## Ask\n",
    "\n",
    "```lua\n",
    "local before = 41\n",
    "local text, available = user_input()\n",
    "return text .. ' ' .. tostring(available) .. ' ' .. (before + 1)\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "asker");
let ctx = RunContext::new("asker", 7, Timestamp::UNIX_EPOCH);
let mut run = Run::new(Arc::new(parsed?), "", ctx);

let mut log: Vec<Event> = Vec::new();
let result = loop {
    match run.step() {
        Step::Pending { effects, events } => {
            log.extend(events);
            for (id, _provenance, effect) in effects {
                let answer = match effect {
                    Effect::UserInput { execution, section } => {
                        let text = ask_operator(&execution, &section);
                        EffectAnswer::UserInput(Ok(InputOutcome::Text(text)))
                    }
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
    RunResult::Ok(text) => assert_eq!(text, "hello operator true 42"),
    other => panic!("the run should succeed: {other:?}"),
}
assert!(log.iter().any(|event| matches!(event, Event::UserInputWaitStarted { .. })));
assert!(log.iter().any(|event| matches!(event, Event::UserInput { text, .. } if text == "hello operator")));
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **The prompt.** The section sets a local, asks for input, and returns the text, the flag, and the local plus one. It never touches the store, so the host sees only the one input effect.
2. **Routing.** The host destructures [`Effect::UserInput`](crate::effect::Effect::UserInput) and passes both fields to `ask_operator`, which stands in for the host's own way of reaching a person.
3. **The answer.** The host wraps the operator's [`String`] in [`InputOutcome::Text`], then in [`Ok`], then in [`EffectAnswer::UserInput`](crate::effect::EffectAnswer::UserInput), and hands it to [`Run::resume`](crate::Run::resume). The text can hold anything.
4. **The result.** The section receives `"hello operator"` byte-exact, with `available` set to `true`. The local `before` still holds `41` after the wait, so the section returns `"hello operator true 42"` as the text of [`RunResult::Ok`](crate::RunResult::Ok).
5. **The events.** The log holds [`Event::UserInputWaitStarted`](crate::event::Event::UserInputWaitStarted) from the wait opening and [`Event::UserInput`](crate::event::Event::UserInput), whose [`Event::UserInput::text`](crate::event::Event#variant.UserInput.field.text) is the operator's text.

**Waiting as long as needed.** A blocking host can hold the effect for as long as the operator takes. The asking section's Lua state and message history stay intact across the wait, which is why `before` survives in the example. The wait does not block the rest of the run, so the host keeps stepping the run and answering other chains' effects while the input effect stays out.

# Answering without an operator

The run issues an [`Effect::UserInput`](crate::effect::Effect::UserInput) for every `user_input()` call, whether or not the host has anyone to ask. So a host with no operator still answers every one, with [`InputOutcome::Unavailable`], and the prompt continues without input.

````
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::event::Event;
use promptforge::input::InputOutcome;
use promptforge::timestamp::Timestamp;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

let source = concat!(
    "---\n",
    "name: asker\n",
    "description: asks the operator\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Asker\n",
    "\n",
    "## Ask\n",
    "\n",
    "```lua\n",
    "local text, available = user_input()\n",
    "return tostring(available) .. '|' .. text\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "asker");
let ctx = RunContext::new("asker", 7, Timestamp::UNIX_EPOCH);
let mut run = Run::new(Arc::new(parsed?), "", ctx);

let mut log: Vec<Event> = Vec::new();
let result = loop {
    match run.step() {
        Step::Pending { effects, events } => {
            log.extend(events);
            for (id, _provenance, effect) in effects {
                let answer = match effect {
                    Effect::UserInput { .. } => EffectAnswer::UserInput(Ok(InputOutcome::Unavailable)),
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
    RunResult::Ok(text) => {
        assert_eq!(text, "false|User input is unavailable in this host; continue without it.");
    }
    other => panic!("the run should succeed: {other:?}"),
}
assert!(log.iter().any(|event| matches!(event, Event::UserInputWaitStarted { .. })));
assert!(!log.iter().any(|event| matches!(event, Event::UserInput { .. })));
# Ok::<(), Box<dyn std::error::Error>>(())
````

The section receives the fixed sentence "User input is unavailable in this host; continue without it." with `available` set to `false`. The call does not raise. The run still reports [`Event::UserInputWaitStarted`](crate::event::Event::UserInputWaitStarted) when the wait opens, but it reports no [`Event::UserInput`](crate::event::Event::UserInput) for this answer.

**Branch on the flag.** A prompt tells real input from the fallback by `available`, not by the text. The fallback sentence is not exported. An operator who types exactly that sentence still reports `available` as `true`, so an operator cannot fake the unavailable state.

# Failed and cancelled waits

An [`InputError`] answer or a dropped wait raises an error at the prompt's `user_input()` call. This example runs two small prompts. One calls `user_input` through `pcall`, and the other calls it directly. The helper answers the one input effect with whatever answer it is given.

````
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::input::InputError;
use promptforge::timestamp::Timestamp;
use promptforge::{Prompt, Run, RunContext, RunErrorKind, RunResult, Step};

fn run_with(source: &str, answer: EffectAnswer) -> Result<RunResult, Box<dyn std::error::Error>> {
    let (parsed, _parse_events) = Prompt::parse(source, "asker");
    let ctx = RunContext::new("asker", 7, Timestamp::UNIX_EPOCH);
    let mut run = Run::new(Arc::new(parsed?), "", ctx);
    let mut answer = Some(answer);
    loop {
        match run.step() {
            Step::Pending { effects, .. } => {
                for (id, _provenance, effect) in effects {
                    let reply = match effect {
                        Effect::UserInput { .. } => answer.take().unwrap_or(EffectAnswer::Dropped),
                        _ => EffectAnswer::Dropped,
                    };
                    run.resume(id, reply);
                }
            }
            Step::Done { result, .. } => return Ok(result),
        }
    }
}

let catches = concat!(
    "---\n",
    "name: catcher\n",
    "description: catches an input failure\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Catcher\n",
    "\n",
    "## Ask\n",
    "\n",
    "```lua\n",
    "local ok, err = pcall(user_input)\n",
    "return tostring(ok) .. ': ' .. tostring(err)\n",
    "```\n",
);
let raises = concat!(
    "---\n",
    "name: asker\n",
    "description: asks the operator\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Asker\n",
    "\n",
    "## Ask\n",
    "\n",
    "```lua\n",
    "local text = user_input()\n",
    "return text\n",
    "```\n",
);

let cause = std::io::Error::other("socket reset");
let failure = InputError::with_source("the input device is gone", cause);
let RunResult::Ok(text) = run_with(catches, EffectAnswer::UserInput(Err(failure)))? else {
    panic!("pcall catches the input failure");
};
assert!(text.starts_with("false: "));
assert!(text.contains("the input device is gone"));
assert!(!text.contains("socket reset"));

let failure = InputError::message("the input device is gone");
let RunResult::Failure(error) = run_with(raises, EffectAnswer::UserInput(Err(failure)))? else {
    panic!("an uncaught input failure fails the run");
};
assert_eq!(error.kind(), RunErrorKind::Input);

let result = run_with(raises, EffectAnswer::Dropped)?;
assert!(matches!(result, RunResult::Cancelled));
# Ok::<(), Box<dyn std::error::Error>>(())
````

The three runs show each path.

1. **A caught failure.** The host answers an [`InputError`] in [`Err`]. The section calls `user_input` through `pcall`, which returns `false` and the error. Lua sees the error as a table whose `kind` is `"internal"` and whose message is `"user input request was not answered: {message}"`, where `{message}` is the host's message. The cause given to [`InputError::with_source`] stays on the Rust side and never reaches the prompt, so `"socket reset"` is absent from the result.
2. **An uncaught failure.** The section calls `user_input()` directly, so nothing catches the error and the run fails. It ends with [`RunResult::Failure`](crate::RunResult::Failure), and [`RunError::kind`](crate::RunError::kind) returns [`RunErrorKind::Input`](crate::RunErrorKind::Input). That classification applies only when the failure goes uncaught. A caught failure is an ordinary Lua error of kind `"internal"`.
3. **A dropped wait.** The host answers [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped). The waiting call resumes with a cancelled error. Nothing catches it here, so the run ends as [`RunResult::Cancelled`](crate::RunResult::Cancelled).

**A message the prompt may see.** The message in an [`InputError`] is written by the host and shown inside the prompt at the call site, where the model can read it too. Write it for that audience, and put the underlying cause in [`InputError::with_source`].

**Cancelling the run.** [`Run::cancel`](crate::Run::cancel) also ends a pending wait. The waiting `user_input()` call resumes with a cancelled error, and the run reports the interruption promptly. The host then answers the held input effect with [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped), like every other effect still out after a cancel.

# Prompt-side rules

Three rules limit where and how a prompt asks.

- `user_input()` is a global in every prompt section, and the host must answer each [`Effect::UserInput`](crate::effect::Effect::UserInput) it issues.
- `user_input()` takes no arguments. Calling it with any argument raises a Lua error with the message `user_input takes no arguments`.
- The model has no direct way to ask the operator. The run advertises no `user_input` tool to the model, and a model loop offers only the tools the prompt adds. The [`tools`](crate::tools) module page covers how a prompt chooses those tools.

# Logging an input wait

A logging host records each wait with [`Effect::record`](crate::effect::Effect::record) and its answer with [`EffectAnswer::record`](crate::effect::EffectAnswer::record). The effect becomes an [`EffectRecord::UserInput`](crate::effect::EffectRecord::UserInput) with both fields, and the answer becomes an [`AnswerRecord::UserInput`](crate::effect::AnswerRecord::UserInput). Inside the answer record, [`InputOutcome::Text`] becomes [`InputAnswerRecord::Text`](crate::effect::InputAnswerRecord::Text), [`InputOutcome::Unavailable`] becomes [`InputAnswerRecord::Unavailable`](crate::effect::InputAnswerRecord::Unavailable), and an [`InputError`] becomes its [`Display`](std::fmt::Display) text in [`Err`]. The [`effect`](crate::effect) module page covers both records.

````
use promptforge::effect::{AnswerRecord, Effect, EffectAnswer, EffectRecord, InputAnswerRecord};
use promptforge::input::{InputError, InputOutcome};

let effect = Effect::UserInput { execution: "asker".to_owned(), section: "Ask".to_owned() };
let record = effect.record();
assert_eq!(
    record,
    EffectRecord::UserInput { execution: "asker".to_owned(), section: "Ask".to_owned() },
);
assert_eq!(
    serde_json::to_string(&record)?,
    r#"{"UserInput":{"execution":"asker","section":"Ask"}}"#,
);

let text = EffectAnswer::UserInput(Ok(InputOutcome::Text("hi".to_owned()))).record();
assert_eq!(serde_json::to_string(&text)?, r#"{"UserInput":{"Ok":{"Text":"hi"}}}"#);

let unavailable = EffectAnswer::UserInput(Ok(InputOutcome::Unavailable)).record();
assert_eq!(unavailable, AnswerRecord::UserInput(Ok(InputAnswerRecord::Unavailable)));

let failed = EffectAnswer::UserInput(Err(InputError::message("the input device is gone"))).record();
assert_eq!(failed, AnswerRecord::UserInput(Err("the input device is gone".to_owned())));
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Reference

This part covers the two items in the module. The host builds both and passes them to [`Run::resume`](crate::Run::resume) inside an [`EffectAnswer::UserInput`](crate::effect::EffectAnswer::UserInput).

## InputOutcome

[`InputOutcome`] is what the host produced for one input wait: the operator's text, or a statement that the host has no input to give. Building a variant directly is the only way to get one. The host answers with it in [`Ok`] inside an [`EffectAnswer::UserInput`](crate::effect::EffectAnswer::UserInput).

- [`InputOutcome::Text`] holds a [`String`], the operator's text. Use it when the host has real operator input for the wait, for example after a blocking wait that the operator answered. Any content is valid. [Answering with operator text](#answering-with-operator-text) shows what the prompt receives and which event the run reports.
- [`InputOutcome::Unavailable`] carries no data. Use it when the host has no operator or no input source for the wait. [Answering without an operator](#answering-without-an-operator) shows what the prompt receives.

[`InputOutcome`] is `#[non_exhaustive]`, so a `match` on it outside this crate needs a wildcard arm. A future variant, such as a deferred wait, can then arrive without breaking the host. [`InputOutcome`] has no serde form. Its log form is [`InputAnswerRecord`](crate::effect::InputAnswerRecord), which serializes.

````
use promptforge::input::InputOutcome;

fn describe(outcome: &InputOutcome) -> String {
    match outcome {
        InputOutcome::Text(text) => format!("the operator said {text}"),
        InputOutcome::Unavailable => "no operator".to_owned(),
        _ => "an outcome this host does not know".to_owned(),
    }
}

assert_eq!(describe(&InputOutcome::Text("yes".to_owned())), "the operator said yes");
assert_eq!(describe(&InputOutcome::Unavailable), "no operator");
````

## InputError

[`InputError`] reports that the host failed to produce input for one wait because its input source broke. That differs from [`InputOutcome::Unavailable`], which says the host has nothing to give. The host builds one with a constructor and answers with it in [`Err`] inside an [`EffectAnswer::UserInput`](crate::effect::EffectAnswer::UserInput). The struct is `#[non_exhaustive]` with private fields, so its two constructors are the only way to build one. Both are `#[must_use]` and cannot fail.

[`InputError::message`] takes one argument.

- `text`, anything that converts [`Into`] a [`String`], such as a [`&str`](str) or a [`String`], is the failure message. The prompt sees it at the Lua call site, as [Failed and cancelled waits](#failed-and-cancelled-waits) describes, so it must be safe for the prompt and the model to read. No length or content rule is enforced.

It returns an error with that message and no cause, so its [`source`](std::error::Error::source) returns [`None`].

[`InputError::with_source`] takes two arguments.

- `text`, anything that converts [`Into`] a [`String`], is the message shown at the Lua call site, under the same rules as for [`InputError::message`].
- `source`, any type that implements [`std::error::Error`], [`Send`], and [`Sync`] and is `'static`, such as a [`std::io::Error`], is the underlying cause. The error boxes it and returns it from [`source`](std::error::Error::source). The prompt never sees it.

It returns an error with that message and that cause.

[`InputError`] implements [`Display`](std::fmt::Display), which writes only the message text, and [`std::error::Error`], whose [`source`](std::error::Error::source) returns the cause given to [`InputError::with_source`], or [`None`]. It has no serde form. A run log records it as its [`Display`](std::fmt::Display) text in [`AnswerRecord::UserInput`](crate::effect::AnswerRecord::UserInput).

````
use std::error::Error;

use promptforge::input::InputError;

let plain = InputError::message("the input device is gone");
assert_eq!(plain.to_string(), "the input device is gone");
assert!(plain.source().is_none());

let cause = std::io::Error::other("socket reset");
let wrapped = InputError::with_source("the input device is gone", cause);
assert_eq!(wrapped.to_string(), "the input device is gone");
assert!(wrapped.source().is_some());
````



