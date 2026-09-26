# Quick Reference

This page lists every frontmatter key, Lua global and member, substitution form, default and limit, and error kind in the prompt language, each linked to the chapter that teaches it. Use it to look up an exact form or value, and follow the link when you need to know how that piece behaves.

## Frontmatter keys

Every frontmatter key and value rule, with top-level keys first and nested keys grouped under their parent.

| Key | Value | Default | Taught in |
|---|---|---|---|
| Top-level key set | `name`, `description`, `promptforge`, `max_tool_iterations`, `input`, `output`, `capabilities`, `tools`, `args`, `models` | only `name` and `description` required | [Prompt File Structure](02-file-structure.md#frontmatter-rules-and-errors) |
| `name` | string, kept as written | none, required | [Prompt File Structure](02-file-structure.md#name-and-description) |
| `description` | one-line string, kept as written | none, required | [Prompt File Structure](02-file-structure.md#name-and-description) |
| `promptforge` | `0` | none, needed to run | [Prompt File Structure](02-file-structure.md#the-promptforge-version) |
| `max_tool_iterations` | whole number `1` to `1000` | `24` per `models.loop` call, or the host's default | [Conversations](11-conversations.md#the-round-cap) |
| `input` | map of `path` and `description` | no input file | [Prompt File Structure](02-file-structure.md#input-and-output-files) |
| `output` | map of `path` and `description` | no output file | [Prompt File Structure](02-file-structure.md#input-and-output-files) |
| `capabilities` | list of capability entries | no capabilities | [Tools](12-tools.md#declaring-capabilities) |
| `tools` | map of alias to tool path | no tool slots | [Tools](12-tools.md#tool-slots-and-tool-objects) |
| `args` | map of arg name to arg declaration | implicit `prose` arg | [Arguments](06-arguments.md#arg-declarations) |
| `models` | map of role label to role declaration | no model roles | [Models](10-models.md#declaring-roles) |
| `args.{name}` | arg declaration map | none | [Arguments](06-arguments.md#arg-declarations) |
| `args.{name}.default` | value matching `type` | no default | [Arguments](06-arguments.md#arg-declarations) |
| `args.{name}.description` | string | no description | [Arguments](06-arguments.md#arg-declarations) |
| `args.{name}.optional` | boolean | `false` | [Arguments](06-arguments.md#arg-declarations) |
| `args.{name}.type` | `string`, `boolean`, `integer`, or `number` | none, required | [Arguments](06-arguments.md#arg-declarations) |
| `capabilities` entry as a string | capability id `namespace/pack`, such as `promptforge/web` | required, no config | [Tools](12-tools.md#capability-ids-and-tool-paths) |
| `capabilities` entry `config` | any YAML value | no config | [Tools](12-tools.md#declaring-capabilities) |
| `capabilities` entry `optional` | boolean | `false` | [Tools](12-tools.md#declaring-capabilities) |
| `capabilities` entry `ref` | capability id | none, required in the map form | [Tools](12-tools.md#declaring-capabilities) |
| Implicit `prose` arg | optional `string` arg `prose`, described as `Freeform input for this prompt` | used when `args` is absent | [Arguments](06-arguments.md#prose-input-and-structured-input) |
| `input.description` | string | none, required | [Prompt File Structure](02-file-structure.md#input-and-output-files) |
| `input.path` | store filename, such as `paper.md` | none, required | [Prompt File Structure](02-file-structure.md#input-and-output-files) |
| `models.{label}` | role declaration map, `{}` for none | none | [Models](10-models.md#declaring-roles) |
| `models.{label}.description` | string | the bound model's catalog description | [Models](10-models.md#declaring-roles) |
| `models.{label}.keywords` | list drawn from the seven keywords, kept in order | no keywords | [Models](10-models.md#keywords-and-the-thinking-switch) |
| `models.{label}.keywords` entry `chat` | soft keyword | not set | [Models](10-models.md#keywords-and-the-thinking-switch) |
| `models.{label}.keywords` entry `creative` | soft keyword | not set | [Models](10-models.md#keywords-and-the-thinking-switch) |
| `models.{label}.keywords` entry `fast` | soft keyword | not set | [Models](10-models.md#keywords-and-the-thinking-switch) |
| `models.{label}.keywords` entry `frontier` | soft keyword | not set | [Models](10-models.md#keywords-and-the-thinking-switch) |
| `models.{label}.keywords` entry `no-thinking` | hard keyword, thinking off | not set | [Models](10-models.md#keywords-and-the-thinking-switch) |
| `models.{label}.keywords` entry `small` | soft keyword | not set | [Models](10-models.md#keywords-and-the-thinking-switch) |
| `models.{label}.keywords` entry `thinking` | hard keyword, thinking on | not set | [Models](10-models.md#keywords-and-the-thinking-switch) |
| `models.{label}.min_context` | whole number of tokens, `1` to `4294967295` | no minimum | [Models](10-models.md#declaring-roles) |
| Name grammar for aliases, role labels, and arg names | `[A-Za-z][A-Za-z0-9_-]{0,63}` | none | [Prompt File Structure](02-file-structure.md#names-for-aliases-roles-and-args) |
| `output.description` | string | none, required | [Prompt File Structure](02-file-structure.md#input-and-output-files) |
| `output.path` | store filename, such as `report.md` | none, required | [Prompt File Structure](02-file-structure.md#input-and-output-files) |
| Tool path in `tools.{alias}` | `namespace/pack/name`, such as `promptforge/web/fetch` | none | [Tools](12-tools.md#capability-ids-and-tool-paths) |
| `tools.{alias}` | tool path string | none | [Tools](12-tools.md#tool-slots-and-tool-objects) |

## Lua globals and members

Every global, function, field, and record shape a prompt's Lua code can use, with its form, what it gives back, and the chapter that teaches it.

### models

| Name | Form | Returns | Taught in |
|---|---|---|---|
| `handle.capabilities` | `h.capabilities` | The role's keywords, a sequence of kebab-case strings in declaration order | [Models](10-models.md#handle-fields) |
| `handle.context` | `h.context` | The bound model's context window in tokens, at least 1 | [Models](10-models.md#handle-fields) |
| `handle.description` | `h.description` | The role's `description:`, or the model's catalog description, which can be empty | [Models](10-models.md#handle-fields) |
| `handle.label` | `h.label` | The role label, the same string as `h.name` | [Models](10-models.md#handle-fields) |
| `handle.max_tokens` | `h.max_tokens` | The `max_tokens` option from `models.use`, or `nil` | [Models](10-models.md#handle-fields) |
| `handle.model_id` | `h.model_id` | The bound model's catalog id, such as `claude-sonnet-4-6` | [Models](10-models.md#handle-fields) |
| `handle.name` | `h.name` | The role label | [Models](10-models.md#handle-fields) |
| `handle.temperature` | `h.temperature` | The `temperature` option from `models.use`, or `nil` | [Models](10-models.md#handle-fields) |
| `handle.thinking` | `h.thinking` | `true` for `thinking`, `false` for `no-thinking`, `nil` for neither | [Models](10-models.md#handle-fields) |
| model handle | `local h = models.get('writer')` | Frozen userdata with nine read-only fields and no methods | [Models](10-models.md#model-handles) |
| `models.default` | `models.default(label)` | The default role's model handle; sets the prompt-wide default | [Models](10-models.md#choosing-a-sections-model) |
| `models.get` | `models.get(label)` | The role's model handle; the selection is unchanged | [Models](10-models.md#model-handles) |
| `models.get` | `models.get(ui().selected_model)` | A model handle for a host catalog model id, with a host-state snapshot only | [Models](10-models.md#model-handles) |
| `models.infer` | `models.infer(prompt)` | The reply as a string, from one round on the section's model | [Models](10-models.md#running-a-round-with-modelsinfer) |
| `models.infer` | `models.infer(handle, prompt)` | The reply as a string, from one round on the handle's model | [Models](10-models.md#running-a-round-with-modelsinfer) |
| `models.loop` | `models.loop(messages, compactor?)` | `nil`; appends every record to `messages` | [Conversations](11-conversations.md#a-first-conversation) |
| `models.loop` | `models.loop(handle, messages, compactor?)` | `nil`; every round runs on the handle's model | [Conversations](11-conversations.md#model-and-tool-scope) |
| `models.loop` compactor argument | `models.loop(msgs, function(reason) ... end)` | Called with `"precheck"` or `"provider"` on overflow | [Conversations](11-conversations.md#compactors-and-context-exhaustion) |
| `models.use` | `models.use(label)` | The role's model handle; selects the role for the section | [Models](10-models.md#choosing-a-sections-model) |
| `models.use` option `max_tokens` | `{ max_tokens = n }` | Whole number from 1 to 4294967295; read back as `h.max_tokens` | [Models](10-models.md#sampling-options) |
| `models.use` option `temperature` | `{ temperature = n }` | Finite number from 0.0 to 2.0; read back as `h.temperature` | [Models](10-models.md#sampling-options) |
| `models.use` options | `models.use(label, { temperature = 0.3, max_tokens = 256 })` | A model handle carrying the options; an omitted option reads `nil` | [Models](10-models.md#sampling-options) |

### messages

| Name | Form | Returns | Taught in |
|---|---|---|---|
| builder chaining | `messages.new():system(s):user(u)` | The same list, one record appended per call in call order | [Conversations](11-conversations.md#building-message-lists) |
| content part `image_url` | `{ type = "image_url", image_url = { url = "data:image/png;base64,..." } }` | An image part in a record's `content` array | [Conversations](11-conversations.md#message-records) |
| content part `text` | `{ type = "text", text = "..." }` | A text part in a record's `content` array | [Conversations](11-conversations.md#message-records) |
| `list:append` | `list:append(record)` | The same list, with `record` appended unchanged | [Conversations](11-conversations.md#building-message-lists) |
| `list:assistant` | `list:assistant(content, tool_calls?)` | The same list, with an `assistant` record appended | [Conversations](11-conversations.md#building-message-lists) |
| `list:system` | `list:system(content)` | The same list, with `{ role = "system", content = content }` appended | [Conversations](11-conversations.md#building-message-lists) |
| `list:tool` | `list:tool(content, tool_call_id)` | The same list, with a `tool` record appended | [Conversations](11-conversations.md#building-message-lists) |
| `list:user` | `list:user(content)` | The same list, with `{ role = "user", content = content }` appended | [Conversations](11-conversations.md#building-message-lists) |
| `messages.new` | `messages.new()` | An empty message list, length 0 | [Conversations](11-conversations.md#building-message-lists) |
| `record.content` | `record.content` | A string, or a non-empty array of content parts | [Conversations](11-conversations.md#message-records) |
| `record.role` | `record.role` | `system`, `user`, `assistant`, or `tool` | [Conversations](11-conversations.md#message-records) |
| `record.tool_call_id` | `record.tool_call_id` | On a `tool` record, the string `id` of the call it answers | [Conversations](11-conversations.md#message-records) |
| `record.tool_calls` | `record.tool_calls` | On an `assistant` record, an array of `{ id, name, arguments? }` | [Conversations](11-conversations.md#message-records) |
| tool call entry | `call.id`, `call.name`, `call.arguments` | The call id, the alias the model called, and the parsed arguments table | [Conversations](11-conversations.md#what-the-loop-appends) |

### compactors

| Name | Form | Returns | Taught in |
|---|---|---|---|
| `compactors.fail` | `models.loop(msgs, compactors.fail)` | The default policy; an overflowing round raises `context_exhausted` | [Conversations](11-conversations.md#compactors-and-context-exhaustion) |
| `compactors.fail(tag)` | `compactors.fail('precheck')` | Never returns; raises `context_exhausted` with `reason` set to the tag | [Conversations](11-conversations.md#compactors-and-context-exhaustion) |
| overflow reason `"precheck"` | `err.reason == 'precheck'` | The estimated request exceeded the context window; nothing was sent | [Conversations](11-conversations.md#compactors-and-context-exhaustion) |
| overflow reason `"provider"` | `err.reason == 'provider'` | The provider rejected the request as too large for the context window | [Conversations](11-conversations.md#compactors-and-context-exhaustion) |

### tools

| Name | Form | Returns | Taught in |
|---|---|---|---|
| Tool object | the alias global, such as `search` | A frozen Tool object with five fields and no methods | [Tools](12-tools.md#tool-slots-and-tool-objects) |
| `Tool.description` | `search.description` | The tool's catalog description, never an override | [Tools](12-tools.md#tool-slots-and-tool-objects) |
| `Tool.name` | `search.name` | The alias the slot is bound under | [Tools](12-tools.md#tool-slots-and-tool-objects) |
| `Tool.parameters` | `search.parameters` | An empty table | [Tools](12-tools.md#tool-slots-and-tool-objects) |
| `Tool.untrusted` | `search.untrusted` | `false` | [Tools](12-tools.md#tool-slots-and-tool-objects) |
| `Tool.wire_name` | `search.wire_name` | The last segment of the tool path, such as `fetch` | [Tools](12-tools.md#tool-slots-and-tool-objects) |
| `tools.add` | `tools.add(alias_or_tool)` | Nothing; scopes the tool into the current section | [Tools](12-tools.md#advertising-tools-to-the-model) |
| `tools.add` array form | `tools.add({ 'search', fetch })` | Nothing; scopes every listed tool | [Tools](12-tools.md#advertising-tools-to-the-model) |
| `tools.add` description override | `tools.add(alias, description)` | Nothing; sets the model's description for this section | [Tools](12-tools.md#advertising-tools-to-the-model) |
| `tools.add_local` | `tools.add_local(alias, description, params, handler)` | Nothing; registers a local tool in the section | [Tools](12-tools.md#local-tools) |
| `tools.add_local` handler args | `function(a) return a.text end` | One fresh table of the call's arguments, by declared name | [Tools](12-tools.md#local-tools) |
| `tools.add_local` handler return | `return 'saved'` | A scalar, as the call's text; `nil` gives the empty string | [Tools](12-tools.md#local-tools) |
| `tools.add_local` param types | `"string"`, `"integer"`, `"number"`, `"boolean"` | The parameter's JSON Schema type | [Tools](12-tools.md#local-tools) |
| `tools.add_local` params table | `{ query = 'string', limit = { 'integer', 'maximum hits' } }` | JSON Schema `object` parameters, every one required | [Tools](12-tools.md#local-tools) |
| `tools.allow_tasks` | `tools.allow_tasks()` | Nothing; scopes the task built-ins, allowing any section the chain can resolve | [Tasks](15-tasks.md#letting-the-model-start-tasks) |
| `tools.allow_tasks` targets list | `tools.allow_tasks({ '## Summarize', '## Review' })` | Nothing; scopes the task built-ins and limits the model's `task` calls to the listed headings | [Tasks](15-tasks.md#letting-the-model-start-tasks) |
| `tools.always` | `tools.always(alias)` | Nothing; scopes the tool into every section | [Tools](12-tools.md#advertising-tools-to-the-model) |
| `tools.always` description override | `tools.always(alias, description)` | Nothing; sets the model's description for the whole run | [Tools](12-tools.md#advertising-tools-to-the-model) |
| `tools.call` | `tools.call(alias_or_tool, args?)` | The tool's output: a string, or a table from a structured tool | [Tools](12-tools.md#calling-tools-from-lua) |
| `tools.calls` | `tools.calls[alias]` or `tools.calls.alias` | The section's call count for that alias, an integer | [Tools](12-tools.md#counting-calls) |

### store

| Name | Form | Returns | Taught in |
|---|---|---|---|
| `store.append` | `store.append(path, contents)` | `nil`; adds `contents` to the end, creating the file when absent | [The Store](09-the-store.md#writing-and-reading-files) |
| `store.delete` | `store.delete(path)` | `nil`; removes the file or an empty directory, and succeeds when absent | [The Store](09-the-store.md#changing-and-checking-files) |
| `store.exists` | `store.exists(path)` | `true` or `false`, for a file or a directory | [The Store](09-the-store.md#changing-and-checking-files) |
| `store.glob` | `store.glob(pattern)` | A sorted array of matching store file paths, never directories | [The Store](09-the-store.md#listing-files-with-glob) |
| `store.read` | `store.read(path)` | The whole file verbatim as a string | [The Store](09-the-store.md#writing-and-reading-files) |
| `store.read` with a range | `store.read(path, start, end?)` | Lines `start` to `end`, 1-based and inclusive, joined with `"\n"` | [The Store](09-the-store.md#line-ranges-and-numbered-reads) |
| `store.read_numbered` | `store.read_numbered(path)` | The whole file as `N\| text` lines numbered from 1 | [The Store](09-the-store.md#line-ranges-and-numbered-reads) |
| `store.read_numbered` with a range | `store.read_numbered(path, start, end?)` | The selected lines with their absolute line numbers | [The Store](09-the-store.md#line-ranges-and-numbered-reads) |
| `store.str_replace` | `store.str_replace(path, old, new)` | `nil`; replaces the single occurrence of `old` with `new` | [The Store](09-the-store.md#changing-and-checking-files) |
| `store.write` | `store.write(path, contents)` | `nil`; creates the file or replaces its text | [The Store](09-the-store.md#writing-and-reading-files) |

### tasks

| Name | Form | Returns | Taught in |
|---|---|---|---|
| Task handle | `{ task = id }` | A plain methodless table; the bare id string also works | [Tasks](15-tasks.md#task-handles-and-ids) |
| `tasks.cancel` | `tasks.cancel(task)` | Nothing; cancelling an ended task does nothing | [Tasks](15-tasks.md#cancellation-and-task-lifetimes) |
| `tasks.events` | `tasks.events(task, opts?)` | A 1-based sequence of event tables, each with a `kind` | [Task Events](16-task-events.md#reading-a-tasks-history) |
| `tasks.events` option `last` | `tasks.events(t, { last = seq })` | Only events after `seq`, a whole number from 0 to 4294967295 | [Task Events](16-task-events.md#reading-a-tasks-history) |
| `tasks.note` | `tasks.note(text)` | Nothing; sets the `note` field of `tasks.status` | [Tasks](15-tasks.md#checking-on-tasks) |
| `tasks.pending` | `tasks.pending(filter?)` | The chain's live Task handles in spawn order, empty when none | [Tasks](15-tasks.md#checking-on-tasks) |
| `tasks.pending` filter `origin` | `tasks.pending({ origin = 'model' })` | Only live tasks of that origin, `author` or `model` | [Tasks](15-tasks.md#checking-on-tasks) |
| `tasks.ready` | `tasks.ready(task)` | `true` once the task has ended, else `false` | [Tasks](15-tasks.md#checking-on-tasks) |
| `tasks.spawn` | `tasks.spawn(target, opts?)` | A Task handle, at once | [Tasks](15-tasks.md#starting-a-task) |
| `tasks.spawn` option `index` | `{ index = n }` | The task's `sys.index`, an integer of 0 or more | [Tasks](15-tasks.md#starting-a-task) |
| `tasks.spawn` option `input` | `{ input = s }` | A string that replaces the task's `args` | [Tasks](15-tasks.md#starting-a-task) |
| `tasks.spawn` option `item` | `{ item = v }` | JSON data that becomes the task's `item` | [Tasks](15-tasks.md#starting-a-task) |
| `tasks.status` | `tasks.status(task)` | The status table of an owned task or of `sys.taskid` | [Tasks](15-tasks.md#checking-on-tasks) |
| `tasks.status` fields | `tasks.status(t).state` | `target`, `origin`, `state`, `ok`, `section`, `blocked`, `turns`, `tasks`, `depth`, `note` | [Tasks](15-tasks.md#checking-on-tasks) |
| `tasks.when_all` | `tasks.when_all(set, opts?)` | A results sequence in set order, then `timed_out` | [Tasks](15-tasks.md#waiting-for-results) |
| `tasks.when_all` result entry | `results[i]` | `{ task, ok, result }`, itself a Task handle | [Tasks](15-tasks.md#waiting-for-results) |
| `tasks.when_all` `timed_out` | `local results, timed_out = tasks.when_all(set, { timeout = 5 })` | `true` when the timeout expired first, else `false` | [Tasks](15-tasks.md#time-limits-on-waits) |
| `tasks.when_any` | `tasks.when_any(set, opts?)` | The ended member's Task handle, `ok`, and its result text or error value | [Tasks](15-tasks.md#waiting-for-results) |
| `tasks.when_any` with a timeout | `tasks.when_any(set, { timeout = 5 })` | `nil` when no member ended in time | [Tasks](15-tasks.md#time-limits-on-waits) |
| wait option `timeout` | `{ timeout = seconds }` | A whole, fractional, or zero number of seconds | [Tasks](15-tasks.md#time-limits-on-waits) |

### sys

| Name | Form | Returns | Taught in |
|---|---|---|---|
| `sys.execution` | `sys.execution` | The run's name, assigned by the host | [The Lua Environment](05-lua-environment.md#run-metadata-in-sys) |
| `sys.id` | `sys.id` | The current section entry's id, such as `0.3.0` | [The Lua Environment](05-lua-environment.md#run-metadata-in-sys) |
| `sys.index` | `sys.index` | An arm's 1-based position in its collection, or a task's `index` option | [Fanout](14-fanout.md#inside-an-arm) |
| `sys.model` | `sys.model` | The section's catalog model id, readable only after the section's first tool call | [Models](10-models.md#the-bound-model-in-sysmodel) |
| `sys.section_count` | `sys.section_count` | The number of top-level sections in the prompt | [The Lua Environment](05-lua-environment.md#run-metadata-in-sys) |
| `sys.section_name` | `sys.section_name` | The running section's heading name, or the title in the H1 pass | [The Lua Environment](05-lua-environment.md#run-metadata-in-sys) |
| `sys.taskid` | `sys.taskid` | The nearest enclosing task's id, `0` on the main walk | [The Lua Environment](05-lua-environment.md#run-metadata-in-sys) |
| `sys.when` | `sys.when` | The run's start instant as an RFC 3339 string | [The Lua Environment](05-lua-environment.md#run-metadata-in-sys) |

### Model-facing task tools

For each tool and argument, Form is what the model sends and Returns is the text the model gets back; the `tools.allow_tasks` row is the Lua call that puts these tools in scope.

| Name | Form | Returns | Taught in |
|---|---|---|---|
| `await_tasks` | `{}` | Every task notice that has arrived, one per line, once one of the model's tasks ends; `nothing to wait for` when none is running | [Tasks](15-tasks.md#the-models-wait) |
| `await_tasks.timeout` | `{"timeout": 0.1}` | Any notices, then `timed out; tasks {ids} still running` when no task ended in time; `slept {seconds} seconds` when none is running | [Tasks](15-tasks.md#the-models-wait) |
| `task` | `{"target": "## Research"}` | `Task id={id} started`, at once | [Tasks](15-tasks.md#letting-the-model-start-tasks) |
| `task.input` | `"input": "text"`, optional | Replaces the task's argument string | [Tasks](15-tasks.md#letting-the-model-start-tasks) |
| `task.target` | `"target": "## Research"`, required | Names the section the task runs | [Tasks](15-tasks.md#letting-the-model-start-tasks) |
| `task_cancel` | `{"id": "0.0"}` | `Task id={task} cancelled` | [Tasks](15-tasks.md#the-models-status-cancel-and-history-tools) |
| `task_cancel.id` | `"id": "0.0"`, required | Names a task the model started, exactly as `task` returned it | [Tasks](15-tasks.md#the-models-status-cancel-and-history-tools) |
| `task_events` | `{"id": "0.0"}` | One JSON event per line in the untrusted envelope, or `no new events` | [Tasks](15-tasks.md#the-models-status-cancel-and-history-tools) |
| `task_events.id` | `"id": "0.0"`, required | Names a task the model started, exactly as `task` returned it | [Tasks](15-tasks.md#the-models-status-cancel-and-history-tools) |
| `task_events.last` | `"last": 12`, optional | Only events after that `provenance.seq` | [Tasks](15-tasks.md#the-models-status-cancel-and-history-tools) |
| `task_status` | `{"id": "0.0"}` | One line starting `Task id={task} (## {target}): {state}` | [Tasks](15-tasks.md#the-models-status-cancel-and-history-tools) |
| `task_status.id` | `"id": "0.0"`, required | Names a task the model started, exactly as `task` returned it | [Tasks](15-tasks.md#the-models-status-cancel-and-history-tools) |
| `tools.allow_tasks` | `tools.allow_tasks(targets?)` in Lua | `task`, `task_cancel`, `task_status`, `await_tasks`, and `task_events` in scope for every round in the section | [Tasks](15-tasks.md#letting-the-model-start-tasks) |

### Web tools

For each tool and argument, Form is what the model sends and Returns is the text the model gets back; the `promptforge/web` row is the capability line that makes the tools available.

| Name | Form | Returns | Taught in |
|---|---|---|---|
| `promptforge/web` | `capabilities: [promptforge/web]` | The tool paths `promptforge/web/fetch` and `promptforge/web/search` | [Web Fetch and Search](13-web-fetch-and-search.md#the-web-capability) |
| `promptforge/web/fetch` | `{"url": "https://example.com/"}` | A `url:`, `truncated:`, `extraction:` header, a blank line, then the content, in the untrusted envelope | [Web Fetch and Search](13-web-fetch-and-search.md#calling-the-fetch-tool) |
| `promptforge/web/fetch` `max_chars` | `"max_chars": 5000`, optional | At most that many characters, 1 to the policy's limit (40,000 by default); the limit when omitted | [Web Fetch and Search](13-web-fetch-and-search.md#length-and-size-limits) |
| `promptforge/web/fetch` `raw` | `"raw": true`, optional | The whole HTML page as markdown, with `extraction: raw-html`; `false` when omitted | [Web Fetch and Search](13-web-fetch-and-search.md#what-a-fetch-returns) |
| `promptforge/web/fetch` `url` | `"url": "https://example.com/"`, required | The page at that address | [Web Fetch and Search](13-web-fetch-and-search.md#calling-the-fetch-tool) |
| `promptforge/web/search` | `{"query": "rust async runtime"}` | JSON text with a `results` array, in the untrusted envelope | [Web Fetch and Search](13-web-fetch-and-search.md#searching-the-web) |
| `promptforge/web/search` `count` | `"count": 5`, optional | At most that many results, 1 to 20; the gateway's default when omitted | [Web Fetch and Search](13-web-fetch-and-search.md#search-options) |
| `promptforge/web/search` `country` | `"country": "us"`, optional | One country's results; 1 to 128 characters, not blank | [Web Fetch and Search](13-web-fetch-and-search.md#search-options) |
| `promptforge/web/search` `exclude_domains` | `"exclude_domains": ["example.com"]`, optional | Drops results from those sites; at most 20 bare hostnames | [Web Fetch and Search](13-web-fetch-and-search.md#search-options) |
| `promptforge/web/search` `freshness` | `"freshness": "pw"`, optional | Results from the past day, week, month, or year: `pd`, `pw`, `pm`, or `py` | [Web Fetch and Search](13-web-fetch-and-search.md#search-options) |
| `promptforge/web/search` `include_domains` | `"include_domains": ["example.com"]`, optional | Only results from those sites; at most 20 bare hostnames | [Web Fetch and Search](13-web-fetch-and-search.md#search-options) |
| `promptforge/web/search` `query` | `"query": "rust async runtime"`, required | The search text; 1 to 400 characters, not blank | [Web Fetch and Search](13-web-fetch-and-search.md#searching-the-web) |
| `promptforge/web/search` `safesearch` | `"safesearch": "strict"`, optional | The SafeSearch level: `off`, `moderate`, or `strict` | [Web Fetch and Search](13-web-fetch-and-search.md#search-options) |
| `promptforge/web/search` `search_lang` | `"search_lang": "en"`, optional | The search language; 1 to 128 characters, not blank | [Web Fetch and Search](13-web-fetch-and-search.md#search-options) |

### Lua standard library

Every section VM runs Lua 5.5 with these libraries and base functions.

| Name | Form | Returns | Taught in |
|---|---|---|---|
| `_G` | `_G` | the global table | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `_VERSION` | `_VERSION` | the Lua version string | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `assert` | `assert(condition, message)` | raises `message` when `condition` is false | [The Lua Environment](05-lua-environment.md#calls-that-wait-and-errors-that-raise) |
| `error` | `error(message)` | raises `message` | [The Lua Environment](05-lua-environment.md#calls-that-wait-and-errors-that-raise) |
| `getmetatable` | `getmetatable(v)` | as in standard Lua 5.5; `var` and `sys` give a guard string | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `ipairs` | `ipairs(t)` | as in standard Lua 5.5 | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `math` library | `math.{name}(...)` | as in standard Lua 5.5 | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `select` | `select(n, ...)` | as in standard Lua 5.5 | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `setmetatable` | `setmetatable(t, mt)` | `t` | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `string` library | `string.upper(s)`, `s:match(pattern)` | as in standard Lua 5.5 | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `table` library | `table.{name}(...)` | as in standard Lua 5.5 | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `table.concat` | `table.concat(list, sep, i, j)` | joined string; `__tostring` values render with `tostring` | [The Lua Environment](05-lua-environment.md#standard-lua-and-host-calls) |
| `tonumber` | `tonumber(v)` | as in standard Lua 5.5 | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `tostring` | `tostring(v)` | string; an error value gives its message | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `type` | `type(v)` | type name; an error value gives `'table'` | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |

### Other globals

These globals, fanout result fields, and error value fields need no declaration.

| Name | Form | Returns | Taught in |
|---|---|---|---|
| `{alias}` | `{alias}` | Tool object for that bound tool slot | [Tools](12-tools.md#tool-slots-and-tool-objects) |
| `args` | `args` | the raw argument string | [Arguments](06-arguments.md#input-basics) |
| `argv` | `argv` | the parsed argument string; `{ prose = args }` without `args:`, nil when structured input is not JSON | [Arguments](06-arguments.md#prose-input-and-structured-input) |
| `call` | `call(target, input?)` | the called chain's result as a string | [Jump and Call](08-jump-and-call.md#jump-and-call-at-a-glance) |
| `compactors` | `compactors.fail` | the `compactors` namespace | [Conversations](11-conversations.md#compactors-and-context-exhaustion) |
| `err .. s` and `s .. err` | `'prefix: ' .. err` | concatenation with the error's message | [The Lua Environment](05-lua-environment.md#catching-and-inspecting-errors) |
| `err.finish_reason` | `err.finish_reason` | provider finish reason on `empty_model_reply`, when sent | [Conversations](11-conversations.md#empty-and-truncated-replies) |
| `err.kind` and `err.message` | `local ok, err = pcall(f, ...)` | error kind tag; message string | [The Lua Environment](05-lua-environment.md#catching-and-inspecting-errors) |
| `err.kind` tags | `err.kind == '{tag}'` | one of exactly twelve tags | [The Lua Environment](05-lua-environment.md#catching-and-inspecting-errors) |
| `err.name` | `err.name` | requested tool name on `unbound_tool` and `out_of_scope_tool` | [Tools](12-tools.md#tool-failures) |
| `err.reason` | `err.reason` | `precheck` or `provider` on `context_exhausted` | [Conversations](11-conversations.md#compactors-and-context-exhaustion) |
| `err.task` | `err.task` | task id on `task_not_owned`, `task_consumed`, and a cancelled task | [Tasks](15-tasks.md#task-errors) |
| `err.tasks` | `err.tasks` | leaked task ids joined with `, ` in spawn order, on `tasks_live` | [Tasks](15-tasks.md#cancellation-and-task-lifetimes) |
| `fanout` | `fanout(worker, collection)` | array of fanout results, one per member in collection order | [Fanout](14-fanout.md#the-fanout-call) |
| heading reference | `'## Name'` | the section with that level and name | [Jump and Call](08-jump-and-call.md#heading-addresses) |
| host globals | no import | installed in every section VM | [The Lua Environment](05-lua-environment.md#the-sandbox-and-its-globals) |
| `item` | `item` | the arm's member inside a fanout arm, or a task's `item` option | [Fanout](14-fanout.md#inside-an-arm) |
| `item.key` and `item.value` | `item.key`, `item.value` | a keyed member's key and value | [Fanout](14-fanout.md#collections-and-member-order) |
| `jump` | `jump(target)` | nothing; ends the block and the walk continues at `target` | [Jump and Call](08-jump-and-call.md#jump-and-call-at-a-glance) |
| `{label}` | `{label}` | model handle for that bound role | [Models](10-models.md#model-handles) |
| `list_from_section` | `list_from_section(heading)` | 1-based array of the list section's item strings | [Blocks and Prose](03-blocks-and-prose.md#reading-list-items-from-lua) |
| `log` | `log(message)` | nothing; records a `lua` checkpoint event | [The Lua Environment](05-lua-environment.md#checkpoints-with-log) |
| `messages` | `messages.new()` | the `messages` namespace | [Conversations](11-conversations.md#building-message-lists) |
| `models` | `models.{name}(...)` | the `models` namespace | [Models](10-models.md#model-roles-at-a-glance) |
| `next` | `next(t, k?)` | next key and value in `pairs` order; `nil, nil` past the last key | [The Lua Environment](05-lua-environment.md#deterministic-table-iteration) |
| `pairs` | `pairs(t)` | iteration in the same fixed key order on every run | [The Lua Environment](05-lua-environment.md#deterministic-table-iteration) |
| `pcall` | `pcall(f, ...)` | `true` and results, or `false` and an error value or the value you raised | [The Lua Environment](05-lua-environment.md#catching-and-inspecting-errors) |
| `prose` | `prose` | the block's pending prose, rendered, as a string | [Blocks and Prose](03-blocks-and-prose.md#the-prose-global) |
| `result.exhausted` | `r[i].exhausted` | `true` only when the arm's `models.loop` hit the round cap | [Fanout](14-fanout.md#results) |
| `result.item` | `r[i].item` | the member the arm processed | [Fanout](14-fanout.md#results) |
| `result.ok` | `r[i].ok` | `true` when the arm completed normally, `false` at the round cap | [Fanout](14-fanout.md#results) |
| `result.text` | `r[i].text` | the arm's result text, `''` when it returned none | [Fanout](14-fanout.md#results) |
| `store` | `store.{name}(...)` | the `store` namespace | [The Store](09-the-store.md#what-the-store-is) |
| `sys` | `sys.{field}` | the `sys` namespace | [The Lua Environment](05-lua-environment.md#run-metadata-in-sys) |
| `tasks` | `tasks.{name}(...)` | the `tasks` namespace | [Tasks](15-tasks.md#tasks-at-a-glance) |
| `tools` | `tools.{name}(...)` | the `tools` namespace | [Tools](12-tools.md#tools-at-a-glance) |
| `tostring(err)` | `tostring(err)` | the error's message, with no traceback | [The Lua Environment](05-lua-environment.md#catching-and-inspecting-errors) |
| `tostring(result)` and `table.concat(results)` | `tostring(r[i])`, `table.concat(r, sep)` | the result's text; the joined texts | [Fanout](14-fanout.md#the-fanout-call) |
| `ui` | `ui()` | host-state snapshot table; present only when the host supplies one | [The Lua Environment](05-lua-environment.md#host-state-with-ui) |
| `untrusted` | `untrusted(s)` | `s` inside an untrusted envelope | [The Store](09-the-store.md#wrapping-untrusted-text) |
| `user_input` | `local text, available = user_input()` | the operator's text and `true`, or a fixed sentence and `false` | [The Lua Environment](05-lua-environment.md#asking-the-operator-with-user_input) |
| `var` | `var.key = value` | your own values, carried along the walk | [The Lua Environment](05-lua-environment.md#keeping-values-in-var) |
| `xpcall` | `xpcall(f, handler, ...)` | like `pcall`; `handler` receives the error value | [The Lua Environment](05-lua-environment.md#catching-and-inspecting-errors) |

## Substitution forms

Each row links to the section that teaches the form.

| Form | Renders | Taught in |
|---|---|---|
| Inserted text | Verbatim, never scanned again | [Substitution](07-substitution.md#literal-braces-and-one-pass-output) |
| Resolved value | Strings as is, numbers and booleans in natural form, tables and arrays as compact JSON with sorted keys | [Substitution](07-substitution.md#dotted-paths-and-rendering) |
| `\{{`, `\}}`, `\\` | `{{`, `}}`, `\` | [Substitution](07-substitution.md#literal-braces-and-one-pass-output) |
| `{{ args }}` | The argument string exactly as passed | [Substitution](07-substitution.md#run-input-with-args-and-argv) |
| `{{ argv }}` | The whole parsed arguments | [Substitution](07-substitution.md#run-input-with-args-and-argv) |
| `{{ argv.key }}` | One field of the parsed arguments, at any depth | [Substitution](07-substitution.md#run-input-with-args-and-argv) |
| `{{ item }}` | The seeded item of a fanout arm or a task, by its type, JSON null as `null` | [Substitution](07-substitution.md#fanout-items) |
| `{{ name }}` | A section-local Lua global, whole | [Substitution](07-substitution.md#values-from-var-sys-and-lua-globals) |
| `{{ name.key }}` | A field of a table held in a Lua global | [Substitution](07-substitution.md#values-from-var-sys-and-lua-globals) |
| `{{ path }}` | The value its first segment names: `args`, `argv`, `item`, `var`, `sys`, or a Lua global | [Substitution](07-substitution.md#what-substitution-does) |
| `{{ sys.key }}` | A runtime-provided `sys` field | [Substitution](07-substitution.md#values-from-var-sys-and-lua-globals) |
| `{{ var.key }}` | A value in `var`, with dotted paths for nested fields | [Substitution](07-substitution.md#values-from-var-sys-and-lua-globals) |

## Defaults and limits

Set by names the frontmatter key that sets a value, or says whether the host sets it or it is fixed.

| Name | Default | Range or rule | Set by | Taught in |
|---|---|---|---|---|
| Call depth cap | 8 levels | Nested `call`, fanout arms, and tasks share it, first call included | fixed | [Jump and Call](08-jump-and-call.md#call-failures-and-the-depth-cap) |
| Cancel poll interval | 10,000 instructions | A host cancel stops a running block within this many instructions | fixed | [Limits and Errors](17-limits-and-errors.md#cancelling-a-run) |
| Fanout concurrency cap | 8 live arms | Per `fanout` call | host | [Fanout](14-fanout.md#concurrency) |
| Generic completion text | `done` | The run result when no block returns a scalar | fixed | [How a Prompt Runs](04-how-a-prompt-runs.md#what-a-run-does) |
| Host-set limits | Listed in the chapter | A prompt changes only the round cap | host | [Limits and Errors](17-limits-and-errors.md#limits-at-a-glance) |
| Instruction count | No cap | Only the cancel poll counts instructions | fixed | [Limits and Errors](17-limits-and-errors.md#lua-block-budgets) |
| Log byte quota | 262,144 bytes per section VM | 256 UTF-8 bytes per allowed log event, so it follows the log event quota | host | [Limits and Errors](17-limits-and-errors.md#lua-block-budgets) |
| Log event quota | 1024 `log` calls per section VM | Every one-argument `log` call spends one | host | [Limits and Errors](17-limits-and-errors.md#lua-block-budgets) |
| Log message length | 256 characters | Counted as Unicode characters | fixed | [The Lua Environment](05-lua-environment.md#checkpoints-with-log) |
| Lua memory | 64 MiB per section VM | Running out is an ordinary `lua` error | host | [Limits and Errors](17-limits-and-errors.md#lua-block-budgets) |
| Model receive timeout | 120 seconds | Applies to the headers and to each next body chunk | host | [Limits and Errors](17-limits-and-errors.md#model-reply-size-and-wait-time) |
| Model response cap | 16 MiB | A larger reply fails the call | host | [Limits and Errors](17-limits-and-errors.md#model-reply-size-and-wait-time) |
| Round cap default | 24 rounds | Per `models.loop` call, when `max_tool_iterations` is absent | host | [Conversations](11-conversations.md#the-round-cap) |
| Round cap from frontmatter | The host default | Whole number 1 to 1000, per `models.loop` call | `max_tool_iterations` | [Conversations](11-conversations.md#the-round-cap) |
| Run limit defaults | Listed in the chapter | One set applies to the whole run | host | [Limits and Errors](17-limits-and-errors.md#limits-at-a-glance) |
| `user_input` fallback sentence | `User input is unavailable in this host; continue without it.` | Returned with `available` set to `false` | fixed | [The Lua Environment](05-lua-environment.md#asking-the-operator-with-user_input) |

## Error kinds

Parse error kinds classify a file that fails to parse, run error kinds classify how a failed run ended, and error kinds on error values are the `kind` tags a `pcall` sees.

### Parse error kinds

| Kind | Raised when | Message names | Taught in |
|---|---|---|---|
| `Fence` | A second `lua shared` fence, a `lua shared` fence outside the H1, or an unclosed fence | The unclosed fence's label or section name, else nothing | [Limits and Errors](17-limits-and-errors.md#parse-error-kinds) |
| `Frontmatter` | The frontmatter block is missing, unclosed, or not valid YAML, or a frontmatter key or value is rejected | The YAML diagnostic, in `invalid frontmatter: {message}`, or the rejected key's own message, with the line and column beside it | [Limits and Errors](17-limits-and-errors.md#parse-error-kinds) |
| `List` | A list section holds non-list content, an empty item, or no items | The section name, and the offending line for non-list content | [Limits and Errors](17-limits-and-errors.md#parse-error-kinds) |
| `Lua` | A Lua region (the shared library, an H1 block, or a section block) does not compile | The section and block, plus the compiler diagnostic | [Limits and Errors](17-limits-and-errors.md#parse-error-kinds) |
| `Structure` | The H1 title is missing, repeated, or empty, a heading is empty or has no parent one level up, two siblings share a name, or the file has no `promptforge:` key when run | The section name, the heading level, and both lines of a duplicate sibling | [Limits and Errors](17-limits-and-errors.md#parse-error-kinds) |

### Run error kinds

| Kind | Raised when | Message names | Taught in |
|---|---|---|---|
| `Binding` | A section sends prose to a model or calls `models.infer` without a handle while no `models.use` or `models.default` is in effect | The section, in `model binding required for section {section}` | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| Cancelled outcome | The host cancels the run, or a caught `cancelled` error value is raised again after another suspending call; a clean stop with no run error kind, not a failure | Nothing; the outcome carries no message | [Limits and Errors](17-limits-and-errors.md#cancelling-a-run) |
| `Completion` | A model call fails at the transport, backend, or decode layer (a missing or invalid environment variable, invalid client configuration, or a disabled gateway included), or an empty reply, and the error goes uncaught | The backend status, the variable name, or the reply's detail phrase, depending on the failure | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| `ContextExhausted` | A round overflows the model's context window under the selected compactor and goes uncaught | The reason, in `context exhausted: {reason}` | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| `Determinism` | Two live chains claim one store path in conflicting ways in block code; the call never returns, so no `pcall` catches it | The store path, both chains, and both claim kinds, in `store determinism violation: {detail}` | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| `Input` | The host's input source fails a `user_input` request and the failure goes uncaught | The host's failure text, in `user input request was not answered: {message}` | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| `Internal` | An engine invariant breaks, a fault in the engine rather than the prompt | The invariant, in `internal invariant violated: {message}` | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| `Lua` | An uncaught Lua failure in a walked section, `call` chain, task, fanout arm, or the shared library load, including a failed substitution, running out of memory, a failed `store` call, and a block that returns a table; a task error in any chain; a caught `lua`, `internal`, `out_of_scope_tool`, `unbound_tool`, or task error value raised again after another suspending call | The Lua error's own text | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| `Parse` | The file fails with any parse error kind, or has no `promptforge:` key | The parse error's own message, with its location beside it when known | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| `Quota` | The log event quota or the log byte quota runs out and the error goes uncaught | Nothing, as in `lua log event quota exceeded` or `lua log byte quota exceeded` | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| `RequirementsUnmet` | Prepare finds a required capability missing, two declared capabilities in conflict, or a model role requirement unmet, or an ordinary Lua error goes uncaught in the H1 pass | Each unmet requirement on its own line, or the Lua error text | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| Retryable failures | `Completion` failures from a transport failure (a receive timeout included), a malformed or oversized reply, an unreadable backend body, or a backend status of 500 or higher; nothing reruns a failed run automatically | The backend status, when there is one | [Limits and Errors](17-limits-and-errors.md#model-call-and-environment-failures) |
| `Store` | The host's store backend fails as the run starts or as the store opens for the H1 pass, the walk, or a task | Nothing, as in `store operation failed` | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| `Tool` | A tool fails, the model calls a tool outside the round's scope, a script calls an alias not bound in the run, or `models.loop` reaches its round cap, and the error goes uncaught | The tool's failure text, the requested name and the aliases in scope or bound, or nothing, as in `tool-call loop did not converge` | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |
| `Version` | The `promptforge:` key declares a major version other than `0` | The declared version, in `unsupported promptforge version: {n} (this build supports major 0)` | [Limits and Errors](17-limits-and-errors.md#how-a-failed-run-is-classified) |

### Error kinds on error values

| Kind | Raised when | Message names | Taught in |
|---|---|---|---|
| `cancelled` | A host cancel reaches running Lua or a waiting call, or a wait returns it, unraised, for a cancelled task | Nothing, as in `interrupted by Ctrl-C`, or the task, in `` task `{task}` was cancelled ``; field `task` | [Limits and Errors](17-limits-and-errors.md#errors-caught-in-lua) |
| `context_exhausted` | A `models.loop` round overflows the context window under `compactors.fail`, or a script calls `compactors.fail(tag)` | The reason in words, in `context exhausted: {reason}`; field `reason` is `"precheck"` or `"provider"` | [Conversations](11-conversations.md#compactors-and-context-exhaustion) |
| `empty_model_reply` | A `models.loop` reply is empty and is not the clean exit | The reply's detail phrase, or plain `empty model reply`; field `finish_reason` when the provider sent one | [Conversations](11-conversations.md#empty-and-truncated-replies) |
| `internal` | A host-side failure the prompt cannot fix: a model call's transport, backend, or decode failure, a missing or invalid environment variable, invalid client configuration, a disabled gateway, the missing-model error, a failed `user_input` source, or an engine fault | The backend status, the variable name, the section, or the host's input failure text, depending on the failure | [Limits and Errors](17-limits-and-errors.md#errors-caught-in-lua) |
| `lua` | A runtime error, a host call's argument or misuse error, running out of memory, a spent log quota, a failed substitution, or a failed `store` call | The error's own text, such as the unknown field or the store path | [Limits and Errors](17-limits-and-errors.md#errors-caught-in-lua) |
| `out_of_scope_tool` | The model calls a name outside the round's scope | The requested name and the aliases in scope, in `tool "{name}" is not in this section's scope; in-scope aliases: [...]`; field `name` | [Tools](12-tools.md#model-tool-calls) |
| `task_consumed` | A wait names a task already delivered | The task, in `` task `{task}` was already delivered: a task's result is taken by one wait ``; field `task` | [Tasks](15-tasks.md#waiting-for-results) |
| `task_not_owned` | A wait, status read, history read, or cancel names a task the chain does not own, or an id that names no task | The task, in `` task `{task}` is not a task this chain owns ``; field `task` | [Tasks](15-tasks.md#task-errors) |
| `tasks_live` | A chain ends normally with tasks it spawned still live | The leaked ids in spawn order; field `tasks`, joined with `, ` | [Tasks](15-tasks.md#cancellation-and-task-lifetimes) |
| `tool` | A bound tool fails in a script `tools.call` | The tool's model-safe failure text, in `tool call failure: {message}` | [Tools](12-tools.md#tool-failures) |
| `tool_loop_exhausted` | `models.loop` makes its round cap of rounds without a final reply | Nothing, as in `tool-call loop did not converge` | [Conversations](11-conversations.md#the-round-cap) |
| `unbound_tool` | A script `tools.call` names neither a local tool nor an alias bound in the run, or names a task built-in | The name and every bound alias, in `tool "{name}" is not bound in this run; bound aliases: [...]`; field `name` | [Tools](12-tools.md#tool-failures) |
