This crate lets your program run PromptForge agents as long-running sessions that a person talks to.

Your program tells the [`Harness`] where models live, launches agents by name, and relays what each running agent says and asks. That is the whole job of a Host. The Harness owns the running work, and your program owns everything around it: the settings, the person at the screen, and when anything changes.

By the end of this page you will have built `desk`, a Host that runs the built-in `chat` agent for one person. Each tour adds one idea: launch an agent and read its result, stream its replies, answer its questions, stop a turn, and reattach after a disconnect. A stub model server on localhost stands in for a real one. It answers each model round with `You said: ` and the last message. It also lists `stub-model` in its model list, with a context window of at least 32768 tokens, because `chat` declares that minimum and a smaller window refuses the run.

# Before you start

A PromptForge agent is a Markdown prompt file. Its Lua code holds the logic, and its prose holds text for a model. The [PromptForge language guide](https://cppalliance.github.io/promptforge/language/) teaches how to write one. This page uses a few words for the pieces a Host deals with:

- A prompt file your program can launch by name is an *agent*.
- One launched agent, which keeps running until it finishes or you close it, is a *session*.
- One execution of the agent file, from its start to its end, is a *run*. A session makes its first run once a catalog with at least one model is bound, which is at launch when you pushed one first, and a new run each time it restarts, as later tours show. Every run adds to the same history.
- The model server that answers every model call a session makes is the *gateway*.
- The person your program puts in front of a session to answer its questions is the *operator*.
- An open question that a session has asked the operator, and is waiting on, is a *wait*.
- One small piece of a reply, sent while the model is still writing, is a *delta*.

An agent never reaches the outside world by itself. When it needs outside work done, it asks the Harness your program drives. That Harness work is a model round, a tool call (a question to the operator included), or a read or write of a file in the agent's [store](vfs). An agent that starts background tasks can also ask for a timer and a read of a task's history. Every event, reply, and question a session sends you comes from that Harness work.

Here is the smallest agent a Host can launch, placed in `desk`'s agents folder:

````
use harness::{Harness, HarnessConfig};
use std::fs;

// 1. Make desk's agents folder.
let desk = std::env::temp_dir().join("desk-before-you-start");
let agents = desk.join("agents");
fs::create_dir_all(&agents)?;

// 2. Write `hello.md`: frontmatter, one H1 title, and one section whose Lua returns a text.
let hello = concat!(
    "---\n",
    "name: hello\n",
    "description: Says hello without a model\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Hello\n",
    "\n",
    "## Greet\n",
    "\n",
    "```lua\n",
    "return 'Hello from desk.'\n",
    "```\n",
);
fs::write(agents.join("hello.md"), hello)?;

// 3. Build a harness over the folder.
let harness = Harness::new(HarnessConfig {
    agents_path: agents,
    state_dir: desk.join("state"),
});

// 4. The harness offers `hello` next to the built-in `chat`.
assert_eq!(harness.discover(), ["chat", "hello"]);
Ok::<(), std::io::Error>(())
````

1. Step 1 makes a folder for `desk`'s agents. An agent is just a file in a folder that the Harness reads by path.
2. Step 2 writes `hello.md`. Its frontmatter holds the three keys every agent needs: `name`, `description`, and `promptforge: 0`. Then come one H1 title and one section whose Lua returns a fixed text. The source is built with `concat!` so that rustdoc keeps its `# Hello` line. The smallest agent needs no model, no tool, and no operator.
3. Step 3 builds a Harness over the folder with [`Harness::new`] and a [`HarnessConfig`]. Building a Harness touches no folder, so this step cannot fail.
4. Step 4 asserts that [`Harness::discover`] lists `chat` and `hello`, sorted. The file stem is the name a launch asks for, and it sits next to the built-in `chat`.

# Launch an agent

You have an agent and a model server, and you want your program to run the agent and read its answer. Your program tells the Harness where the model server is, then launches agents by name. Each launch becomes a session that runs on its own.

Launching feels like [`tokio::spawn`](https://docs.rs/tokio/latest/tokio/fn.spawn.html): you get a handle back at once, and the work is already running. Unlike a spawned task, you watch its state and then read its output.

````
use harness::{display_chain, CatalogBinding, GatewayBinding, HostSnapshot, LaunchRequest, SessionState};
# use harness::{Harness, HarnessConfig};
# use std::error::Error;
# use std::sync::Arc;
# let desk = std::env::temp_dir().join("desk-launch-an-agent");
# let agents = desk.join("agents");
# std::fs::create_dir_all(&agents)?;

// 1. Add desk's `greet` agent: it answers the line in `line.txt` into `reply.txt`.
let greet = concat!(
    "---\nname: greet\ndescription: Answers one line\npromptforge: 0\n",
    "models: { writer: {} }\n",
    "input: { path: line.txt, description: The line to answer }\n",
    "output: { path: reply.txt, description: The answer }\n",
    "---\n\n# Greet\n\n## Answer\n\n```lua\n",
    "store.write('reply.txt', models.infer(writer, store.read('line.txt')))\n",
    "```\n",
);
std::fs::write(agents.join("greet.md"), greet)?;

// 2. Build one harness, and share it behind an `Arc`.
let harness = Arc::new(Harness::new(HarnessConfig {
    agents_path: agents,
    state_dir: desk.join("state"),
}));

// 3. Push the stub model server at generation 1, without `/v1`.
let stub = "http://127.0.0.1:8080";
harness.set_gateway(GatewayBinding { base_url: stub.into(), key: "desk-key".into(), generation: 1 });

// 4. Push the stub's model list at generation 1, and select its model.
harness.set_catalog(CatalogBinding { generation: 1, models: vec![[("id", "stub-model")].into_iter().collect()] });
harness.set_host(HostSnapshot { selected_model: Some("stub-model".into()), ..HostSnapshot::default() });

// 5. Launch by name with the operator's line, wait for `Closed`, then read the output.
async fn ask(harness: &Harness, line: &str) -> Result<String, Box<dyn Error>> {
    let request = LaunchRequest { agent: "greet".into(), args: String::new(), input_text: Some(line.into()) };
    let session = harness.launch(request).await.map_err(|refusal| display_chain(&refusal))?;
    session.subscribe_state().wait_for(|state| *state == SessionState::Closed).await?;
    let reply = session.output_text()?;
    assert_eq!(reply, format!("You said: {line}"));
    Ok(reply)
}

// 6. `greet` is launchable, the push took, and a logged binding hides its key.
assert_eq!(harness.discover(), ["chat", "greet"]);
assert_eq!(harness.gateway().map(|gateway| gateway.generation), Some(1));
assert!(format!("{:?}", harness.gateway()).contains("<redacted>"));
# Ok::<(), std::io::Error>(())
````

1. Step 1 writes `greet.md`. `models: { writer: {} }` declares a model role labelled `writer`, and every declared role is also a Lua global of that name. `input:` and `output:` name the files the agent reads and writes. `models.infer(writer, text)` sends one model round with that text and returns the reply as a string. `store.read` and `store.write` use the session's store, where the Harness puts the `input:` file and looks for the `output:` file.
2. Step 2 builds one [`Harness`] from a [`HarnessConfig`] and shares it behind an [`Arc`](std::sync::Arc), because one Harness serves every session your program launches.
3. Step 3 pushes the stub model server with [`Harness::set_gateway`], as a [`GatewayBinding`]. Leave `/v1` off its `base_url`, because the Harness appends it. Give every new binding a higher `generation` than the last.
4. Step 4 pushes a [`CatalogBinding`], your program's list of chat-capable models, through [`Harness::set_catalog`], and selects `stub-model` with [`Harness::set_host`] and a [`HostSnapshot`]. Each `models` entry is one raw JSON object, a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), here `{"id": "stub-model"}` built from a one-pair array; the Harness takes the model's name from its `"id"` string.
5. Step 5 defines `ask`. [`Harness::launch`] takes a [`LaunchRequest`] naming an agent from [`Harness::discover`], and writes its `input_text` to the agent's declared input file. `ask` waits for [`SessionState::Closed`] on [`Session::subscribe_state`] before it calls [`Session::output_text`], which returns [`OutputError::Unfinished`] until a run has completed. A completed or failed run closes the session by itself. `ask` needs the live stub, so the example never calls it.
6. Step 6 asserts that `greet` is launchable, that [`Harness::gateway`] holds the generation 1 binding, and that `{:?}` prints the key as `"<redacted>"`, so a binding is safe to log.

What a gateway push does depends on its generation:

- A push with the same generation is ignored entirely, so a rotated key never takes effect.
- A lower one is stored and new launches use it, but running sessions ignore it and keep the old gateway.
- A higher one reaches every running session, and [The complete program](#the-complete-program) shows what that does.

The Harness checks the name before the gateway. A name that `discover` does not list, a path included, is refused with [`LaunchError::UnknownAgent`]. No gateway, or one whose URL does not parse or whose key is empty, fails with [`LaunchError::GatewayUnusable`].

You might expect [`Harness::new`] to check your folders and connect to the model server. Instead, it touches nothing, so a bad name or an unusable gateway arrives as a [`LaunchError`] from `launch`.

A missing or empty catalog raises no error: the session stays `Alive` and waits until a catalog with at least one model arrives.

Start catalog generations at 1. A session launched before any catalog counts generation 0 as already seen, so a push at 0 never reaches it.

Each run reads the Host snapshot as it starts, so a new selection reaches a running session only when its run restarts, never in the middle of a reply.

Push the gateway and the catalog, launch by name, wait for `Closed`, then read the output. `chat` loops on `input.ask()` forever, so it never closes by itself, and later tours close it. Next, [Stream a reply](#stream-a-reply) shows a reply while the model writes it.

# Stream a reply

You want the operator to watch a reply appear as the model writes it, not all at once when it finishes. While the model writes, the session sends small pieces of the reply, the deltas. When the reply is done, the session records one event holding the finished text. Every event a session has recorded, in order, is its [transcript](log). The pieces and the finished event carry the same reply number.

Each model round records up to three kinds of event that hold a reply number. A thinking event holds the model's finished reasoning, and replaces the reasoning pieces. The `assistant_reply` event holds the finished answer, and replaces the text pieces. A tool-call event lists the tools the reply asked to run.

Deltas feel like an [`mpsc`](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html) stream of chunks. Unlike an `mpsc` channel, they are a broadcast that keeps nothing for a receiver that is not there, and a saved event replaces them when the reply is done.

````
use harness::DeltaKind;
# use harness::{CatalogBinding, GatewayBinding, Harness, HarnessConfig, HostSnapshot, LaunchRequest, Session, WaitFrame};
# use std::collections::HashMap;
# use std::error::Error;
# use std::future::{poll_fn, Future};
# use std::pin::pin;
# use std::task::Poll;
# fn desk() -> Harness {
#     let config = HarnessConfig { agents_path: "desk/agents".into(), state_dir: "desk/state".into() };
#     let harness = Harness::new(config);
#     harness.set_gateway(GatewayBinding { base_url: "http://127.0.0.1:8080".into(), key: "desk-key".into(), generation: 1 });
#     harness.set_catalog(CatalogBinding { generation: 1, models: vec![[("id", "stub-model")].into_iter().collect()] });
#     harness.set_host(HostSnapshot { selected_model: Some("stub-model".into()), ..HostSnapshot::default() });
#     harness
# }
# fn chat() -> LaunchRequest {
#     LaunchRequest { agent: "chat".into(), args: String::new(), input_text: None }
# }
# async fn say(session: &Session, text: &str) -> Result<(), Box<dyn Error>> {
#     let mut waits = session.subscribe_waits();
#     session.resend_waits();
#     loop {
#         if let WaitFrame::Required { token } = waits.recv().await? {
#             return Ok(session.send_input(&token, text.into(), || {})?);
#         }
#     }
# }
# enum Next<D, E> {
#     Delta(D),
#     Event(E),
# }
# async fn first<D, E>(delta: impl Future<Output = D>, event: impl Future<Output = E>) -> Next<D, E> {
#     let (mut delta, mut event) = (pin!(delta), pin!(event));
#     poll_fn(|cx| match delta.as_mut().poll(cx) {
#         Poll::Ready(delta) => Poll::Ready(Next::Delta(delta)),
#         Poll::Pending => event.as_mut().poll(cx).map(Next::Event),
#     })
#     .await
# }

async fn stream() -> Result<(), Box<dyn Error>> {
#     let harness = desk();
    // 1. Subscribe to deltas and events right after launch, before any reply starts.
    let session = harness.launch(chat()).await?;
    let mut deltas = session.subscribe_deltas();
    let mut events = session.subscribe_events();
#   say(&session, "Hello, desk.").await?;

    // 2. Print each text piece as it arrives, grouped by its reply number.
    let mut pieces: HashMap<u64, String> = HashMap::new();
    let (reply, text) = loop {
        match first(deltas.recv(), events.recv()).await {
            Next::Delta(Ok(delta)) => match delta.kind {
                DeltaKind::Text => {
                    print!("{}", delta.content);
                    pieces.entry(delta.reply).or_default().push_str(&delta.content);
                }
                DeltaKind::Reasoning => eprint!("{}", delta.content),
                _ => {}
            },
            // 3. A receiver that fell behind lost pieces; the finished event repairs them.
            Next::Delta(Err(_)) => {}
            // 4. The finished reply event carries the reply number of its pieces.
            Next::Event(event) => {
                let event = event?;
                if event.event["kind"] == "assistant_reply" {
                    break (event.reply, event.event["text"].as_str().map(str::to_owned));
                }
            }
        }
    };

    // 5. Swap the pieces for the finished text: they join to the same reply.
    let shown = reply.and_then(|reply| pieces.remove(&reply));
    assert_eq!(shown, text);
    Ok(())
}
````

1. Read the hidden `say(&session, "Hello, desk.")` as the operator typing `Hello, desk.`. You do not need its body to follow this tour. Step 1 builds `desk`'s Harness through the hidden `desk` function, which pushes the stub gateway, the generation 1 catalog, and the selected model, as [Launch an agent](#launch-an-agent) taught. It launches `chat`, then calls [`Session::subscribe_deltas`] and [`Session::subscribe_events`] before the hidden `say` answers `chat`'s first question. `chat` starts by asking the operator a question and pauses until it gets an answer. The hidden `say` answers it the way the next tour teaches, and that answer starts the first model round. Each receiver gets only what is sent after it subscribes, and the session is already running when `launch` returns. Subscribing right after launch catches every piece of the first reply.
2. Step 2 prints each [`DeltaKind::Text`] piece, the answer, and collects it under its `reply` number. It sends each [`DeltaKind::Reasoning`] piece, the model's reasoning, to stderr, because the operator usually sees these in different places. A wildcard arm ignores kinds added later, because [`DeltaKind`] is `#[non_exhaustive]`. The hidden `first` stands in for [`tokio::select!`](https://docs.rs/tokio/latest/tokio/macro.select.html) over the two receivers.
3. Step 3 ignores the error a lagging delta receiver gets for the pieces it lost, and does not retry. A missed piece costs the operator a moment of streaming, never text, because the finished event holds the whole reply.
4. Step 4 stops at the `assistant_reply` event, and takes its `reply` number and its finished `text`. Only thinking, reply, and tool-call events carry a `reply` number, so a [`SessionEvent`] whose `reply` is `None` is something other than model text. You can route events without parsing every one.
5. Step 5 removes the pieces collected under that number and asserts that they equal the finished text. The streamed preview and the finished reply agree, so `desk` can swap one for the other. Each piece carries the number of the event that replaces it, which is how `desk` knows which pieces to swap out.

What happens when you subscribe after the session has recorded some events? Those events are still there: read them with [`Session::transcript`]. Deltas never enter it, so a late subscriber can rebuild every finished reply, but none of the pieces.

You might expect deltas to be saved like events, so that a late subscriber can replay them. Instead, deltas are live only, and the finished event with the same `reply` number is the lasting copy.

Show pieces as they arrive, and let the finished event have the last word. Next, [Answer the operator](#answer-the-operator) sends the operator's text back to the agent.

# Answer the operator

The agent asks the operator a question, and your program must show it and send back what they type. When an agent asks, the session opens a wait with a single-use token, and pauses that part of the agent until you answer.

A wait feels like a [`oneshot`](https://docs.rs/tokio/latest/tokio/sync/oneshot/index.html) channel whose sender you hold. Unlike a oneshot, you answer it by token through the session, and it outlives any receiver you watched it on.

An agent can ask in two ways. Its own Lua can call `input.ask()`, which needs only `promptforge/user-input` in its `capabilities:` frontmatter. `capabilities:` lists the tool sets the Harness provides, and `promptforge/user-input` is the set for asking the operator. `chat` declares it and calls `input.ask()` directly, so this tour takes that path.

Or the model can decide to ask. Then the agent's `tools:` frontmatter maps an alias, the name the model calls, to a tool's full id, as in `ask: promptforge/user-input/ask`, and [`USER_INPUT_ASK_TOOL`] is that id. The agent still declares `promptforge/user-input` under `capabilities:`, because a tool slot whose capability is not declared refuses the run with `RequirementsUnmet`.

Binding a tool is not the same as offering it: the agent's Lua offers the alias with `tools.add` for one section or `tools.always` for the whole run, and the capability alone gives the model nothing to call. Both paths call the same ask tool, and open a wait the same way. One pass from the operator's message to the agent's reply is a *turn*. Answering a wait starts the next turn.

````
use harness::{WaitError, WaitFrame};
# use harness::{CatalogBinding, GatewayBinding, Harness, HarnessConfig, HostSnapshot, LaunchRequest, Session};
# use std::error::Error;
# fn desk() -> Harness {
#     let config = HarnessConfig { agents_path: "desk/agents".into(), state_dir: "desk/state".into() };
#     let harness = Harness::new(config);
#     harness.set_gateway(GatewayBinding { base_url: "http://127.0.0.1:8080".into(), key: "desk-key".into(), generation: 1 });
#     harness.set_catalog(CatalogBinding { generation: 1, models: vec![[("id", "stub-model")].into_iter().collect()] });
#     harness.set_host(HostSnapshot { selected_model: Some("stub-model".into()), ..HostSnapshot::default() });
#     harness
# }
# fn chat() -> LaunchRequest {
#     LaunchRequest { agent: "chat".into(), args: String::new(), input_text: None }
# }

// 1. Subscribe to questions, then have the session re-announce the ones already open.
async fn question(session: &Session) -> Result<String, Box<dyn Error>> {
    let mut waits = session.subscribe_waits();
    session.resend_waits();

    // 2. Keep the token of the first open question, and drop prompts that were cancelled.
    loop {
        match waits.recv().await? {
            WaitFrame::Required { token } => return Ok(token),
            WaitFrame::Cancelled { token } => println!("desk drops the prompt for {token}"),
        }
    }
}

async fn answer() -> Result<(), Box<dyn Error>> {
#     let harness = desk();
    let session = harness.launch(chat()).await?;
    let mut events = session.subscribe_events();

    // 3. Answer by token with the operator's text, exactly as typed.
    let token = question(&session).await?;
    session.send_input(&token, "Hello, desk.".into(), || {})?;

    // 4. The token is spent: desk clears its prompt, and a second answer is refused.
    let again = session.send_input(&token, "Hello again.".into(), || {});
    assert_eq!(again, Err(WaitError::UnknownToken));

    // 5. The next reply uses the operator's text.
    let reply = loop {
        let event = events.recv().await?;
        if event.event["kind"] == "assistant_reply" {
            break event;
        }
    };
    assert_eq!(reply.event["text"], "You said: Hello, desk.");
    Ok(())
}
````

1. Step 1 defines `question`, which calls [`Session::subscribe_waits`] and then [`Session::resend_waits`]. The receiver shows only frames sent after you subscribe. But the agent is paused on its question, so the question has to outlive any one receiver. The session keeps each open question in its own list, not in the channel, and `resend_waits` announces every one still open.
2. Step 2 returns the token of the first [`WaitFrame::Required`]. That single-use token ties an answer to the question that asked for it. For each [`WaitFrame::Cancelled`], `question` drops the prompt, because that question ended unanswered and an answer to it would be refused.
3. Step 3 launches `chat` from the hidden `desk`, subscribes to events, and answers `chat`'s first question by token with [`Session::send_input`]. Pass the operator's text as typed: the agent receives it byte for byte, with no trimming, so trim it yourself if the agent expects that. `send_input` calls its closure once before it hands over the text, and the closure runs even when the call goes on to fail. Pass `|| {}` unless you track turns, and do not take a call to the closure as proof that the answer was accepted.
4. Step 4 answers the same token again, and gets [`WaitError::UnknownToken`], with the text discarded. Treat that error as a normal race: the token was already answered, cancelled, or never issued. An answered wait sends no `Cancelled`, so `desk` clears its prompt itself once `send_input` returns `Ok`.
5. Step 5 waits for the next `assistant_reply` event and asserts that its text is `You said: Hello, desk.`, the stub's echo of the answer. The operator's answer reached the agent and drove its next model turn.

Here is the whole question loop, from the agent asking to the agent resuming:

````text
   agent                 session                        desk                  operator
     │                      │                             │                       │
     │  input.ask()         │                             │                       │
     ├─────────────────────>│  WaitFrame::Required        │                       │
     │  (this part pauses)  │  { token }                  │                       │
     │                      ├────────────────────────────>│  show the question    │
     │                      │                             ├──────────────────────>│
     │                      │                             │                       │
     │                      │                             │  the operator types   │
     │                      │  send_input(&token, text)   │<──────────────────────┤
     │                      │<────────────────────────────┤                       │
     │  text, as typed      │                             │                       │
     │<─────────────────────┤                             │                       │
     │  (the agent resumes) │                             │                       │
````

You might expect a question to be lost when nobody was subscribed as it was asked, like a broadcast with no receiver. Instead, the session holds every open question until it is answered or cancelled, and `resend_waits` announces each one again.

Subscribe, resend, and answer by token. Next, [Stop a turn](#stop-a-turn) ends a turn that is going nowhere.

# Stop a turn

The model is stuck or heading the wrong way, and the operator wants to stop this answer but keep the conversation. Stopping the turn is different from stopping the session: [`Session::cancel`] stops the turn, and [`Session::close`] stops the session. This tour's example assumes a different stub, one that never answers the round the first answer starts, so the turn hangs until you cancel it. The example compiles but never runs, so that stub's setup is not shown.

Cancelling a turn feels like aborting a tokio task. Unlike an aborted task, the session is not gone: the agent starts again over the conversation so far.

````
use harness::{FailureKind, SessionFailure, SessionState};
# use harness::{CatalogBinding, GatewayBinding, Harness, HarnessConfig, HostSnapshot, LaunchRequest, Session, WaitError, WaitFrame};
# use std::error::Error;
# fn desk() -> Harness {
#     let config = HarnessConfig { agents_path: "desk/agents".into(), state_dir: "desk/state".into() };
#     let harness = Harness::new(config);
#     harness.set_gateway(GatewayBinding { base_url: "http://127.0.0.1:8080".into(), key: "desk-key".into(), generation: 1 });
#     harness.set_catalog(CatalogBinding { generation: 1, models: vec![[("id", "stub-model")].into_iter().collect()] });
#     harness.set_host(HostSnapshot { selected_model: Some("stub-model".into()), ..HostSnapshot::default() });
#     harness
# }
# fn chat() -> LaunchRequest {
#     LaunchRequest { agent: "chat".into(), args: String::new(), input_text: None }
# }
# async fn question(session: &Session) -> Result<String, Box<dyn Error>> {
#     let mut waits = session.subscribe_waits();
#     session.resend_waits();
#     loop {
#         match waits.recv().await? {
#             WaitFrame::Required { token } => return Ok(token),
#             WaitFrame::Cancelled { token } => println!("desk drops the prompt for {token}"),
#         }
#     }
# }

// 1. desk keeps the session after a failed turn; a failed run has already closed it.
fn keeps_session(failure: &SessionFailure) -> bool {
    match failure.kind {
        FailureKind::ModelTurnFailed | FailureKind::ToolCallFailed => true,
        FailureKind::RunFailed | FailureKind::Interrupted => false,
    }
}

async fn stop() -> Result<(), Box<dyn Error>> {
#     let harness = desk();
    // 2. Subscribe to failure reports before the first turn.
    let session = harness.launch(chat()).await?;
    let mut failures = session.subscribe_errors();

    // 3. Answer the first question; in this tour the stub model server hangs on this reply.
    let first = question(&session).await?;
    session.send_input(&first, "Take your time.".into(), || {})?;

    // 4. Cancel the turn: `chat` starts again and asks anew, with no failure report.
    session.cancel();
    let second = question(&session).await?;
    assert_ne!(first, second);
    assert_eq!(session.send_input(&first, "Too late.".into(), || {}), Err(WaitError::UnknownToken));
    assert!(failures.try_recv().is_err());

    // 5. Close the session for good, and wait until nothing is left running.
    let mut state = session.subscribe_state();
    session.close();
    state.wait_for(|now| *now == SessionState::Closed).await?;
    Ok(())
}

// 6. A failed model turn keeps the session; an interrupted run does not.
let message = "Model turn failed in agent `Conversation`".to_string();
let turn = SessionFailure { kind: FailureKind::ModelTurnFailed, message };
let run = SessionFailure { kind: FailureKind::Interrupted, message: String::new() };
assert!(keeps_session(&turn) && !keeps_session(&run));
````

1. Step 1 defines `keeps_session`, which branches on each [`SessionFailure`]'s `kind`, never its `message`, because `message` is display text for the operator and the model. [`FailureKind::ModelTurnFailed`] and [`FailureKind::ToolCallFailed`] mean the turn failed and the agent is waiting again, so `desk` keeps the session. [`FailureKind::RunFailed`] means the run ended in error, and the session closes by itself. [`FailureKind::Interrupted`] is reported only after a close you asked for has ended the run. Either way the session is already closing, so `desk` does not close it again. Launch a new session if the operator wants to go on. The match lists every kind, because [`FailureKind`] is exhaustive.
2. Step 2 calls [`Session::subscribe_errors`] before the first turn, because a failure report sent with no receiver attached is dropped. The transcript would still show that turn, but only as a turn without a reply.
3. Step 3 answers `chat`'s first question, which starts the model turn that hangs.
4. Step 4 calls `cancel`, which stops only the current turn. `chat`'s run starts again over its transcript and asks a new question, and no failure is reported. The assertions check that the new token differs, that the answered first token is refused with [`WaitError::UnknownToken`], and that no failure report arrived. A question still open at the cancel would get [`WaitFrame::Cancelled`], so remove those prompts. Files the stopped run wrote are gone after the restart, because each run starts with an empty [store](vfs) unless you launch with your own filesystem.
5. Step 5 subscribes to the state, calls `close`, and waits for [`SessionState::Closed`]. The state moves to `Closing` at once, and `Closed` means nothing of the session is left running. [`Harness::close`] with the session's id ends it the same way.
6. Step 6 builds a `ModelTurnFailed` report and an `Interrupted` report, and asserts that `keeps_session` keeps the first and not the second. The `message` is display text for the operator. Here it names `Conversation`, the one `##` section of `chat`, which holds its question loop. `keeps_session` never reads it. The routing in step 1 works on real report values, decided by kind alone.

You might expect `cancel` to end the session, the way aborting a task ends it. Instead, it stops only the current turn and starts the agent again over what it already said, so the conversation survives and the operator can go on. `close` is what ends the session.

Cancel stops a turn; close stops the session. Next, [Reattach after a disconnect](#reattach-after-a-disconnect) picks a session back up after your client drops it.

# Reattach after a disconnect

The operator's client dropped, and when they come back you want the same conversation, not a new one. A session outlives its client, so your program keeps the session's id and looks the session up again.

A session id feels like a database key. Unlike a row, what it names keeps running while you are away.

````
use harness::SessionId;
# use harness::{CatalogBinding, GatewayBinding, Harness, HarnessConfig, HostSnapshot, LaunchRequest, Session, WaitFrame};
# use std::error::Error;
# fn desk() -> Harness {
#     let config = HarnessConfig { agents_path: "desk/agents".into(), state_dir: "desk/state".into() };
#     let harness = Harness::new(config);
#     harness.set_gateway(GatewayBinding { base_url: "http://127.0.0.1:8080".into(), key: "desk-key".into(), generation: 1 });
#     harness.set_catalog(CatalogBinding { generation: 1, models: vec![[("id", "stub-model")].into_iter().collect()] });
#     harness.set_host(HostSnapshot { selected_model: Some("stub-model".into()), ..HostSnapshot::default() });
#     harness
# }
# fn chat() -> LaunchRequest {
#     LaunchRequest { agent: "chat".into(), args: String::new(), input_text: None }
# }
# async fn question(session: &Session) -> Result<String, Box<dyn Error>> {
#     let mut waits = session.subscribe_waits();
#     session.resend_waits();
#     loop {
#         match waits.recv().await? {
#             WaitFrame::Required { token } => return Ok(token),
#             WaitFrame::Cancelled { token } => println!("desk drops the prompt for {token}"),
#         }
#     }
# }

async fn reattach() -> Result<(), Box<dyn Error>> {
#     let harness = desk();
#     let session = harness.launch(chat()).await?;
    // 1. The client shows one event, keeps the id as text, and disconnects.
    let mut client = session.subscribe_events();
#     let token = question(&session).await?;
#     session.send_input(&token, "Hello, desk.".into(), || {})?;
    let shown = client.recv().await?.index;
    let saved = session.id().as_str().to_owned();
    drop((client, session));

    // 2. Look the session up by its id; `None` would mean start a new one.
    let session = harness.session(&SessionId::new(saved)).ok_or("the session was closed")?;

    // 3. Subscribe first, then replay the history from one past the last shown index.
    let mut live = session.subscribe_events();
    let mut indexes: Vec<u64> = Vec::new();
    for event in session.transcript(shown + 1).await? {
        indexes.push(event.index);
    }

    // 4. Re-announce the open question, answer it, and skip live events already replayed.
    let token = question(&session).await?;
    session.send_input(&token, "I'm back.".into(), || {})?;
    loop {
        let event = live.recv().await?;
        if indexes.last().is_some_and(|last| event.index <= *last) {
            continue;
        }
        indexes.push(event.index);
        if event.event["kind"] == "assistant_reply" {
            break;
        }
    }

    // 5. Replayed and live events join with no gap and no repeat.
    let expected: Vec<u64> = (shown + 1..).take(indexes.len()).collect();
    assert_eq!(indexes, expected);
    Ok(())
}

// 6. An id the harness never issued finds no session.
# let harness = desk();
assert!(harness.session(&SessionId::new("never-issued")).is_none());
````

1. Step 1 launches `chat` from the hidden `desk`, and the hidden lines answer its first question. The client keeps the `index` of one live event, saves the id from [`Session::id`] as text with [`SessionId::as_str`], and drops its receiver and its handle. Dropping the handle is a disconnect, not a close: the session keeps running, with its next question open.
2. Step 2 rebuilds the id with [`SessionId::new`] and finds the same running session with [`Harness::session`]. A `None` would mean the session was closed, with nothing to rejoin, so `desk` would start a new one. The saved text is all a client needs to rejoin.
3. Step 3 calls [`Session::subscribe_events`] before it reads [`Session::transcript`] from one past the last shown index. An event recorded between the two reads is caught live rather than lost. The other order, history first, would leave a gap.
4. Step 4 re-announces the question asked while the client was away through `question`, which calls `subscribe_waits` and then `resend_waits`. It answers the question, and skips live events whose `index` the replay already gave. Open questions survive a disconnect, and the skip removes repeats.
5. Step 5 asserts that the replayed and live indexes run one by one from one past the last shown index. `index` numbers every event from zero across every run of the session, restarts included, so one saved number is all the client state `desk` needs.
6. Step 6 asserts that `Harness::session` finds nothing for an id the Harness never issued, just as it finds nothing once a session is closed. If you need to watch a close finish, keep a [`Session`] handle, because [`Harness::close`] removes the session at once, while it is still `Closing`. A lookup right after a close already returns `None`.

You might expect a disconnect to end the session, the way dropping a receiver ends a channel. Instead, the session keeps running and keeps its open questions, and only a close ends it.

Keep the id, subscribe first, then replay and re-announce. Next, [The complete program](#the-complete-program) puts every tour into one Host.

# The complete program

Here is the whole `desk` Host, every line visible: your program owns the settings and the operator, the Harness owns the sessions, and the two meet through pushes, launches, and subscriptions.

````
use harness::{
    display_chain, CatalogBinding, DeltaKind, GatewayBinding, Harness, HarnessConfig,
    HostSnapshot, LaunchRequest, Session, SessionState, WaitError, WaitFrame,
};
use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// desk's stub model server on localhost answers every round with "You said: " and the last message.
const STUB: &str = "http://127.0.0.1:8080";

// A task desk hands to its runtime, such as `tokio::spawn`.
type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

// 1. desk owns every setting and pushes each one into the harness, each at generation 1.
fn build_harness() -> Arc<Harness> {
    let harness = Arc::new(Harness::new(HarnessConfig {
        agents_path: "desk/agents".into(),
        state_dir: "desk/state".into(),
    }));
    harness.set_host(HostSnapshot {
        selected_model: Some("stub-model".into()),
        ..HostSnapshot::default()
    });
    harness.set_catalog(CatalogBinding {
        generation: 1,
        models: vec![[("id", "stub-model")].into_iter().collect()],
    });
    harness.set_gateway(GatewayBinding {
        base_url: STUB.into(),
        key: "desk-key".into(),
        generation: 1,
    });
    harness
}

// 2. Print each text piece as it arrives; a receiver that falls behind only loses pieces.
fn stream(session: &Session) -> Task {
    let mut deltas = session.subscribe_deltas();
    let state = session.subscribe_state();
    Box::pin(async move {
        loop {
            match deltas.recv().await {
                Ok(delta) if delta.kind == DeltaKind::Text => print!("{}", delta.content),
                Ok(_) => {}
                Err(_) if *state.borrow() == SessionState::Closed => break,
                Err(_) => {}
            }
        }
    })
}

// 3. Print each finished reply, which has the last word over its pieces.
fn show_replies(session: &Session) -> Task {
    let mut events = session.subscribe_events();
    let state = session.subscribe_state();
    Box::pin(async move {
        loop {
            match events.recv().await {
                Ok(event) if event.event["kind"] == "assistant_reply" => {
                    println!("\n[reply {:?}] {}", event.reply, event.event["text"]);
                }
                Ok(_) => {}
                Err(_) if *state.borrow() == SessionState::Closed => break,
                Err(_) => {}
            }
        }
    })
}

// 4. A task holding its own clone of the session reports when nothing is left running.
fn report_close(session: Session) -> Task {
    Box::pin(async move {
        let mut state = session.subscribe_state();
        let closed = state.wait_for(|now| *now == SessionState::Closed).await.is_ok();
        println!("session {} closed: {closed}", session.id());
    })
}

// 5. Answer the open question, and ask again when a restart cancelled it first.
async fn answer(session: &Session, text: &str) -> Result<(), Box<dyn Error>> {
    loop {
        let mut waits = session.subscribe_waits();
        session.resend_waits();
        let token = loop {
            match waits.recv().await? {
                WaitFrame::Required { token } => break token,
                WaitFrame::Cancelled { token } => println!("desk drops the prompt for {token}"),
            }
        };
        match session.send_input(&token, text.into(), || {}) {
            Ok(()) => return Ok(()),
            Err(WaitError::UnknownToken) => continue,
        }
    }
}

async fn desk(spawn: impl Fn(Task)) -> Result<(), Box<dyn Error>> {
    let harness = build_harness();

    // 6. Launch `chat` by name, and start the tasks before its first turn.
    let request = LaunchRequest { agent: "chat".into(), args: String::new(), input_text: None };
    let session = harness.launch(request).await.map_err(|refusal| display_chain(&refusal))?;
    spawn(stream(&session));
    spawn(report_close(session.clone()));

    // 7. The operator talks, stops a slow turn, and answers the question `chat` asks next.
    answer(&session, "Hello, desk.").await?;
    session.cancel();
    answer(&session, "Shorter, please.").await?;

    // 8. The model server and the model list change while the session runs, each at a higher generation.
    harness.set_gateway(GatewayBinding {
        base_url: "http://127.0.0.1:8081".into(),
        key: "desk-key-2".into(),
        generation: 2,
    });
    harness.set_catalog(CatalogBinding {
        generation: 2,
        models: vec![
            [("id", "stub-model")].into_iter().collect(),
            [("id", "stub-large")].into_iter().collect(),
        ],
    });

    // 9. A client that comes back looks the session up by id, subscribes, then replays.
    let id = session.id().clone();
    drop(session);
    let session = harness.session(&id).ok_or("desk's session is gone")?;
    let replies = show_replies(&session);
    let history = session.transcript(0).await?;
    assert!(history.iter().zip(0..).all(|(event, index)| event.index == index));
    spawn(replies);
    answer(&session, "Still there?").await?;

    // 10. Nothing answers for the operator: desk closes, even with a question open.
    println!("open questions: {:?}", session.unresolved_waits());
    let mut state = session.subscribe_state();
    assert!(harness.close(&id));
    assert!(harness.session(&id).is_none());
    state.wait_for(|now| *now == SessionState::Closed).await?;
    Ok(())
}
````

1. Step 1 is `build_harness`, from [Launch an agent](#launch-an-agent). You might expect the Harness to find its model server and model in the environment or a config file. Instead, it holds only what your program pushes, and it starts with no gateway at all. So `desk` pushes each setting as a value. Your program already owns the operator's settings and knows when they change. Only a gateway push, or a catalog push whose model list changed, restarts a session's run, and only when its generation is above the last one that session saw. A `set_host` push restarts nothing, and a running session picks it up at its next restart. Taking values you push leaves your program in control of when a change lands.
2. Step 2 is `stream`, from [Stream a reply](#stream-a-reply). It owns its own delta receiver and a state watch, prints `Text` pieces, and shrugs off a lag error while the session runs. It ends once the state is `Closed`.
3. Step 3 is `show_replies`, which prints each `assistant_reply` event. That finished text replaces the pieces `stream` printed under the same reply number.
4. Step 4 is `report_close`, which holds its own clone of the [`Session`]. `report_close` keeps a handle because [`Harness::close`] removes the session from the Harness while it is still `Closing`.
5. Step 5 is `answer`, from [Answer the operator](#answer-the-operator). It subscribes, re-announces, and answers by token. When a restart cancelled the question under it, [`WaitError::UnknownToken`] sends it around again for the new question.
6. Step 6 launches `chat` by name, shows any refusal through [`display_chain`], and spawns `stream` and `report_close` at once, because the session is already running when `launch` returns.
7. Step 7 answers `chat`'s first question, cancels the slow turn as [Stop a turn](#stop-a-turn) taught, and answers the new question `chat` asks after its run starts again.
8. Step 8 pushes a new gateway and a changed model list, each at generation 2, above what the session has seen. A gateway push with a higher generation cancels every running session's current turn at once. Its open questions get [`WaitFrame::Cancelled`], and its run restarts over its transcript on the new gateway.
9. Step 9 drops the handle and looks the session up by id, as [Reattach after a disconnect](#reattach-after-a-disconnect) taught. It subscribes through `show_replies` before it reads `transcript(0)`, and asserts that the history's indexes run from zero with no gap.
10. Step 10 lists the open tokens with [`Session::unresolved_waits`], closes the session with `Harness::close`, asserts that it is gone from the Harness at once, and waits for `Closed`. An open question keeps its agent waiting until `desk` answers it, cancels the turn, or closes the session, so `desk` decides when to give up.

A catalog push with a changed model list, under a higher generation, restarts the run, and with no answer in progress it cancels the current turn at once, as a gateway push does. When the operator's answer has already been accepted, the restart waits for that turn to settle at the first `assistant_reply` event or the first `ModelTurnFailed` or `ToolCallFailed` report, without waiting for the agent to ask again. Every catalog push replaces the stored catalog, but a session acts only on a generation above the last one it saw.

The Host loop, from launch to streaming, answering, and closing:

````text
   desk                                      harness and session
   ────                                      ───────────────────
   set_host, set_catalog, set_gateway ─────> holds the settings desk pushed
   launch("chat") ─────────────────────────> a session starts running
   stream task       <─────────────────────  Delta pieces, live only
   answer()          <─────────────────────  WaitFrame::Required { token }
                     ─────────────────────>  send_input(&token, text)
   show_replies task <─────────────────────  SessionEvent, the finished reply
   cancel() ───────────────────────────────> the turn stops, chat asks again
   session(&id), transcript(0) ────────────> the same session, its whole history
   close(&id) ─────────────────────────────> Closing, then Closed
````

Push settings, launch by name, relay what each session says and asks, and close it when you are done. [Where to go next](#where-to-go-next) lists the module pages.

# Reference

## CatalogBinding

[`CatalogBinding`] carries one generation of your program's chat-capable model list into the Harness. Push it through [`Harness::set_catalog`] whenever that list changes; every push replaces the stored catalog. When no chat-capable model exists, push an empty `models` list under a new, higher generation rather than skipping the push. A running session keeps its current run, but its next restart waits until a later push brings models back under a higher generation. [Launch an agent](#launch-an-agent) shows the first push.

- `generation`: nothing rejects a repeat, but sessions ignore a catalog not above the last one they saw, so start at 1 and raise it.
- `models`: raw JSON model entries; with no selected model, a launch binds the `"id"` string of the first entry.
- [`CatalogBinding::default()`](CatalogBinding::default): generation 0 with an empty `models` list.

## Delta

A [`Delta`] is one live piece of a model round's reply, sent through [`Session::subscribe_deltas`] and never stored. Use it to show a reply while the model writes it. A receiver that falls behind loses pieces, and a piece sent while no receiver is attached is dropped. Collect pieces by `reply`, and swap them for the [`SessionEvent`] with the same `reply`, which is the final version. [Stream a reply](#stream-a-reply) teaches this.

- `reply`: the reply number of the event that will replace this piece.
- `kind`: whether the piece is answer text or reasoning.

## DeltaKind

[`DeltaKind`] tells an answer piece from a reasoning piece in a [`Delta`], so you can show them apart. `Text` is answer content, which the round's `assistant_reply` event replaces, and `Reasoning` is the model's reasoning, which the round's thinking event replaces. It serializes in lowercase, as `"text"` and `"reasoning"`, so a client that forwards it over its own wire sees those strings. Match it with a wildcard arm for kinds added later. [Stream a reply](#stream-a-reply) teaches this.

## FailureKind

[`FailureKind`] classifies one failure report from [`Session::subscribe_errors`], so you can label it or decide what to do next. The first two kinds are turn failures the agent survives, and each settles the current turn. After the last two the session is already closing, so do not close it again: `RunFailed` closes it by itself, and `Interrupted` comes only after a close you asked for, never after a run that already completed or failed. [Stop a turn](#stop-a-turn) teaches this.

| Variant | Meaning |
|---|---|
| `ModelTurnFailed` | A model round failed, and the agent is waiting again. |
| `ToolCallFailed` | A tool call failed, and the agent is waiting again. |
| `RunFailed` | The run itself ended in error. |
| `Interrupted` | A close you asked for ended the run before it finished on its own. |

## GatewayBinding

[`GatewayBinding`] tells the Harness which model server to use and the key that goes with it. Push it through [`Harness::set_gateway`] at startup and whenever the server or key changes. A binding whose URL does not parse or whose key is empty still installs, and launches under it fail with [`LaunchError::GatewayUnusable`]; pushed under a higher generation, it also ends every running session, which reports [`FailureKind::RunFailed`] and closes. Push a corrected binding under a higher generation. [Launch an agent](#launch-an-agent) teaches this.

- `generation`: a push with the current one is ignored. A different one replaces the stored binding, but running sessions switch only to a higher one.
- [`GatewayBinding::api_root`]: `base_url` with trailing slashes trimmed and `/v1` always appended, so leave `/v1` off `base_url`.
- `key`: `Debug` prints it as `"<redacted>"`, so you can log a binding safely.

## Harness

[`Harness`] runs every session your program launches. Build one per program, share it behind an [`Arc`](std::sync::Arc), push the gateway, catalog, and Host settings, then launch agents by name. [`Harness::launch`] refuses with a [`LaunchError`] in this order: an unknown name, even with no gateway bound, then an unusable gateway, an unreadable agent source, or a run log that cannot open. Fix them in the order reported; the next launch retries the run log. [Launch an agent](#launch-an-agent) teaches this.

- [`Harness::new`]: touches no filesystem; the run log is opened under `state_dir` on the first launch.
- [`Harness::discover`]: the `.md` file stems in `agents_path` plus the built-in `chat`, sorted.
- `launch`: returns a session already registered and running, with [`LaunchOptions::default()`](LaunchOptions::default), so each run works in a fresh memory store.
- [`Harness::set_gateway`]: a push with the current generation does nothing, so [`Harness::gateway`] keeps returning the earlier URL and key.
- [`Harness::close`]: removes the session at once, while it is still `Closing`, and returns whether a session was ended.

## HarnessConfig

[`HarnessConfig`] tells the Harness where the agents live and where to keep its state. Write it as a struct literal and pass it to [`Harness::new`]. Neither path is checked then, so a bad path shows up at launch, not at construction. A missing `state_dir` is created on the first launch. [Launch an agent](#launch-an-agent) teaches this.

- `agents_path`: the folder whose `.md` files are the launchable agents; launching `name` reads `<agents_path>/<name>.md`.
- `state_dir`: the folder the Harness keeps its run log under.

## HostSnapshot

[`HostSnapshot`] carries your program's selected model and workspace roots into the Harness. Push it through [`Harness::set_host`] when the operator changes either one. Each run reads it as it starts, so a new selection reaches a running session at its next restart, never a turn in progress; with no selection, a launch binds the first model of the latest [`CatalogBinding`]. The Harness starts with [`HostSnapshot::default()`](HostSnapshot::default): no selection and no roots. [Launch an agent](#launch-an-agent) teaches this.

- `selected_model`: never swapped for another; a model the gateway lacks, or an unfetchable list, fails the run with [`FailureKind::RunFailed`], closing the session; `launch` succeeds.

- [`HostSnapshot::ui`]: returns `{ "selected_model", "workspace_root" }`, each `null` when absent.
- `workspace_roots`: only the first root reaches the prompt's `ui()` global.

## LaunchError

[`LaunchError`] says why [`Harness::launch`] or [`Harness::launch_with`] refused a launch. Show it to a person through [`display_chain`], so that its cause is in the text. `UnknownAgent` also refuses any name that looks like a path, so pass a bare agent name. [Launch an agent](#launch-an-agent) teaches this.

| Variant | Meaning |
|---|---|
| `UnknownAgent` | The name is not a discovered agent; `name` holds what you asked for. Pick a name from [`Harness::discover`]. |
| `GatewayUnusable` | No gateway is bound, or the bound one could not make a model client. Push a valid URL and a non-empty key under a higher generation. |
| [`SessionState`](LaunchError::SessionState) | This variant shares its name with the [`SessionState`] enum but has nothing to do with it: the agent's source file could not be read. `source` holds the filesystem failure, also reachable through [`Error::source`](std::error::Error::source). |
| `Log` | The run log could not be opened, which includes a state directory that cannot be created; it converts from [`LogError`](log::LogError). A launch returns it only then. Launch again, since a failed open is retried. |

## LaunchOptions

[`LaunchOptions`] gives a session its environment, such as a filesystem of its own. Pass it to [`Harness::launch_with`] to stage a session's files or keep them across relaunches. Build it with `..LaunchOptions::default()`, so a field added later does not break your code. The [`vfs`] page shows how.

- `vfs`: the filesystem every run of the session works in, relaunches included; when unset, each run gets a fresh memory store at `/`. Every clone of the handle shares the same files, so a clone you keep can seed them before launch and collect them after the run. While the run is live, an access through your clone conflicts with the run's on any file both touch. An op sink built into the filesystem sees only operations its policy admits, so a denied read or write never reaches it.

## LaunchRequest

[`LaunchRequest`] names the agent to launch and what to hand it, for [`Harness::launch`] or [`Harness::launch_with`]. When `input_text` is `None` and the session's filesystem lacks the prompt's declared input file, `launch` still returns the session, but the run fails as it prepares: [`Session::subscribe_errors`] gets a [`FailureKind::RunFailed`] report, and the session closes. Set `input_text`, or stage the file through [`LaunchOptions`]. [Launch an agent](#launch-an-agent) teaches this.

- `agent`: a bare agent name as [`Harness::discover`] returns it; anything else, such as a path, is refused.
- `args`: the run's argument text, handed to the prompt as `args`, and empty when absent from serialized input.
- `input_text`: staged at the prompt's declared `input:` file before each run; left out of serialized output when `None`.

## OutputError

[`OutputError`] says why [`Session::output_text`] has no text for you. Wait for `Closed` through [`Session::subscribe_state`] before you read, because a new session's output starts as `Unfinished`, and reading too early looks like a failed run. A missing output never fails the run, so check the output apart from the run's outcome. [Launch an agent](#launch-an-agent) teaches this.

| Variant | Meaning |
|---|---|
| `Unfinished` | No run has completed: it is still running, or it failed, was cancelled, or was closed first. |
| `Undeclared` | The prompt declares no `output:` file; add one. |
| `Missing` | The run completed without writing its declared output file; `path` holds that path. |
| `Store` | The store refused the read of the output file at `path`; `source` holds the store's failure, also reachable through [`Error::source`](std::error::Error::source). |

## Session

A [`Session`] is the handle to one running agent. Every clone names the same session, and the session outlives your client's connection. [`Session::send_input`] returns [`WaitError::UnknownToken`] when no open wait holds the token; treat that as a normal race and call [`Session::resend_waits`]. [`Session::transcript`] returns the log's error when a run cannot be read or a stored event no longer parses. [Launch an agent](#launch-an-agent) teaches this.

- Every `subscribe_` receiver sees only what is sent after it subscribes; subscribe to events before you read `transcript`.
- `send_input`: delivers `text` byte-exact; when it fails, the turn it accepted is settled again, so you need no cleanup.
- [`Session::cancel`]: stops the current turn with no failure report; open waits get `Cancelled`, and the agent restarts over its transcript.
- [`Session::close`]: ends the session for good; outstanding work is dropped, and once the run is done the state is `Closed`.
- `cancel`, `close`, and a restart forced by a gateway or model list push all set `Closing`; the state alone cannot tell them apart.

## SessionEvent

A [`SessionEvent`] is one durable entry of a session's event log. You get it live from [`Session::subscribe_events`], or replay it from [`Session::transcript`] after a reconnect. Live events and transcript reads stamp `reply` by the same rule, so you can merge the two by `reply` without special cases. [Stream a reply](#stream-a-reply) teaches this.

- `index`: the entry's position in the whole transcript, from zero and continuing across relaunches; resume past the last one you saw.
- `reply`: the reply number whose [`Delta`] pieces this event replaces; only thinking, reply, and tool-call events have one.
- `event`: the logged event in its stored JSON shape.

## SessionFailure

A [`SessionFailure`] is one failure report for the operator, from [`Session::subscribe_errors`]. A report sent while no receiver is attached is dropped, and a receiver that falls behind misses reports. Then read the transcript, where a missed report shows as a turn without a reply. [Stop a turn](#stop-a-turn) teaches this.

- `kind`: which failure this is, as a [`FailureKind`]; it is the contract, and the field to act on.
- `message`: display text for the operator and the model; it can change, so never derive meaning from it.

## SessionId

A [`SessionId`] names a session so that you can find it again through [`Harness::session`] after your client disconnects. It serializes as a bare string, and `Display` writes the raw id, so you can store it and send it to a client as plain text. [Reattach after a disconnect](#reattach-after-a-disconnect) teaches this.

- [`SessionId::new`]: wraps any string without checking it, an empty one included; an id the Harness never minted finds no session.
- [`SessionId::fresh`]: mints 128 random bits from the OS-seeded cryptographic RNG, hex-encoded.

## SessionState

[`SessionState`] says where a session's run stands; to wait for a close to finish, await `Closed` through [`Session::subscribe_state`]. A new session starts `Alive`, and stays `Alive` while it waits for its first catalog with models, so `Alive` means the session is not closing, not that a model is at work. `Closing` means outstanding work is being answered or dropped. `Closed` means the run reported done, and nothing is outstanding. [Launch an agent](#launch-an-agent) teaches this.

- [`SessionState::interrupted`]: maps `Alive` and `Closing` to `Closing`, and leaves `Closed` as `Closed`.
- [`SessionState::done`]: maps every state to `Closed`.

## USER_INPUT_ASK_TOOL

[`USER_INPUT_ASK_TOOL`] is the id of the tool that asks the operator. Bind it under an alias in `tools:`, such as `ask: promptforge/user-input/ask`, and offer the alias with `tools.add` or `tools.always`. A script's `input.ask()` shows up as this id and returns `available`, which `input.connected()` reports without asking. `available` is `false` only for an agent declaring `promptforge/user-input` with `optional: true` on a Host without an input handler; the Harness gives every run one. [Answer the operator](#answer-the-operator) teaches this.

## WaitError

[`WaitError`] says why [`Session::send_input`] refused an answer. Its one variant, `UnknownToken`, means no open wait holds the token: it was never issued, already answered, or cancelled. Tokens are single use, so a second answer is refused and its text discarded, and every other open wait is untouched. Treat it as a normal race, and call [`Session::resend_waits`] to see which questions are still open. [Answer the operator](#answer-the-operator) teaches this.

## WaitFrame

A [`WaitFrame`] tells you that a session opened a question for the operator, or dropped one. `Required` means a wait opened, and the session wants operator input. `Cancelled` means a wait ended unanswered, as when its turn is cancelled, so its prompt is stale; an answered wait sends no `Cancelled`, so clear that prompt yourself. A frame lost to a dead connection is not sent again; call [`Session::resend_waits`], and every live wait reappears as `Required`. [Answer the operator](#answer-the-operator) teaches this.

- `Required.token`: the single-use token your answer must echo, unique per wait.
- `Cancelled.token`: the token whose wait is gone.

## display_chain

[`display_chain`] renders an error and every cause in its `source()` chain as one line, joined with `: `. Use it to show a Harness error to a person or a model, because an error's `Display` holds only its own message. A cause whose text already appears in the line is left out, but its own causes are still visited, so the root cause survives. An empty cause is left out with no `: `. [Launch an agent](#launch-an-agent) shows it in use.

# Where to go next

- [`cancel`]: stop async work safely, at its next safe point, instead of dropping it mid-step.
- [`log`]: read the saved history every session writes, and tell its failures apart.
- [`vfs`]: give a session files of your own instead of the empty store each run starts with.

