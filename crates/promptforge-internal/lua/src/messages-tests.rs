//! Tests for the `messages.new()` list and its parse through the protocol.

use mlua::{AnyUserData, Lua, LuaSerdeExt, Value};
use promptforge_types::untrusted::GuardNonce;
use serde_json::json;

use super::{MessageList, install_messages};
use crate::protocol::{Answer, MessageContent, MessageRecord, MessageRole, Request, YieldParse};
use crate::{Error, SectionVm};

#[path = "messages-tests-list.rs"]
mod list;

#[path = "messages-tests-records.rs"]
mod records;

#[path = "messages-tests-view.rs"]
mod view;

/// A fresh default handle's access capability for a test VM.
fn fresh_access() -> std::sync::Arc<crate::Access> {
    std::sync::Arc::new(
        promptforge_vfs::VfsRef::default()
            .acquire(promptforge_vfs::Origin::new("messages test fixture"))
            .expect("the stock backend acquires"),
    )
}

fn lua_with_messages() -> Lua {
    let lua = Lua::new();
    let globals = lua.globals();
    install_messages(&lua, &globals).expect("messages install cannot fail on a fresh VM");
    lua
}

fn eval(lua: &Lua, source: &str) -> Value {
    lua.load(source).eval().expect("test source evaluates")
}

/// A section VM with its Engine values injected, `messages` included.
fn section_vm() -> SectionVm {
    let nonce = GuardNonce::from_seed(1);
    let observer = crate::tests::recording::null_emitter();
    let mut vm =
        SectionVm::new(&nonce, &observer, "Test").expect("section VM construction cannot fail");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("value injection cannot fail");
    vm
}

/// Parses `messages` through the chat protocol boundary, as a
/// `models.loop` round's yield would.
fn chat_parse(lua: &Lua, messages: Value) -> YieldParse {
    let request = lua.create_table().expect("table creation cannot fail");
    request.raw_set("op", "chat").expect("raw_set");
    request.raw_set("messages", messages).expect("raw_set");
    Request::from_yield(lua, &Value::Table(request))
}

/// The call error a chat parse answered with.
fn chat_error(parse: YieldParse) -> String {
    match parse {
        YieldParse::Call(Answer::Chat(Err(Error::Lua(message)))) => message,
        other => panic!("expected the chat call error, got {other:?}"),
    }
}

fn text(role: MessageRole, content: &str) -> MessageRecord {
    MessageRecord {
        role,
        content: MessageContent::Text(content.to_owned()),
        tool_calls: Vec::new(),
        tool_call_id: None,
    }
}

#[test]
fn new_returns_an_empty_list() {
    let lua = lua_with_messages();
    let (kind, len, first_nil): (String, i64, bool) = lua
        .load("local l = messages.new(); return type(l), #l, l[1] == nil")
        .eval()
        .expect("test source evaluates");
    assert_eq!(kind, "userdata");
    assert_eq!(len, 0);
    assert!(first_nil);
}

#[test]
fn each_new_call_returns_a_separate_list() {
    let lua = lua_with_messages();
    let (first, second): (usize, usize) = lua
        .load("local a, b = messages.new(), messages.new(); a:user('x'); return #a, #b")
        .eval()
        .expect("test source evaluates");
    assert_eq!((first, second), (1, 0));
}

#[test]
fn the_chat_parse_refuses_a_plain_table_with_the_list_error() {
    let lua = lua_with_messages();
    let refusal = "models.loop needs a messages.new() list; build one with \
                   messages.new() and :user, :append, or :replace";
    for source in [
        "{ { role = 'system', content = 'be terse' }, { role = 'user', content = 'hi' } }",
        "{}",
    ] {
        assert_eq!(
            chat_error(chat_parse(&lua, eval(&lua, source))),
            refusal,
            "{source}"
        );
    }
}

#[test]
fn a_non_empty_list_parses_to_a_chat_request_holding_that_list() {
    let lua = lua_with_messages();
    let built = eval(
        &lua,
        "msgs = messages.new():system('be terse'):user('hi') return msgs",
    );
    let list = match chat_parse(&lua, built) {
        YieldParse::Request(Request::Chat { list, binding }) => {
            assert!(binding.is_none(), "a round without a handle has no binding");
            list
        }
        other => panic!("expected a chat request, got {other:?}"),
    };
    let held = lua
        .globals()
        .get::<AnyUserData>("msgs")
        .expect("msgs is the list userdata")
        .borrow::<MessageList>()
        .expect("msgs is a MessageList")
        .records();
    assert_eq!(list.records(), held);
    assert_eq!(
        list.records(),
        vec![
            text(MessageRole::System, "be terse"),
            text(MessageRole::User, "hi")
        ]
    );
    lua.load("msgs:user('more')")
        .exec()
        .expect("test source runs");
    assert_eq!(
        list.records().len(),
        3,
        "the request holds the author's list, not a copy"
    );
}

#[test]
fn a_list_or_a_record_view_is_refused_by_the_var_and_prose_guards() {
    let vm = section_vm();
    vm.install_lazy_prose(|state| {
        let refusals = ["msgs", "record"].map(|name| match (state.globals)(name) {
            Err(Error::Lua(message)) => message,
            other => format!("{name} was not refused: {other:?}"),
        });
        Ok(refusals.join("|"))
    })
    .expect("the prose guard installs");
    let (writes, prose): (String, String) = vm
        .lua()
        .load(
            "msgs = messages.new():user('u')\n\
             record = msgs[1]\n\
             local out = {}\n\
             for _, write in ipairs({\n\
               function() var.list = msgs end,\n\
               function() var.record = record end,\n\
             }) do\n\
               local ok, err = pcall(write)\n\
               out[#out + 1] = ok and 'stored' or tostring(err):match('var%.%a+ must be JSON data, got %a+')\n\
             end\n\
             return table.concat(out, '|'), prose",
        )
        .eval()
        .expect("test source evaluates");
    assert_eq!(
        writes,
        "var.list must be JSON data, got userdata|var.record must be JSON data, got userdata"
    );
    assert_eq!(
        prose,
        "global `msgs` is a userdata; bare globals in prose must be JSON data|\
         global `record` is a userdata; bare globals in prose must be JSON data"
    );
}

#[test]
fn the_list_runs_under_the_hardened_section_sandbox() {
    let vm = section_vm();
    let json: serde_json::Value = vm
        .lua()
        .load(
            "local msgs = messages.new():system('s'):user('u')\n\
             return { msgs[1], msgs[2] }",
        )
        .eval::<Value>()
        .and_then(|value| vm.lua().from_value(value))
        .expect("record views convert under the hardened sandbox");
    assert_eq!(
        json,
        json!([{ "role": "system", "content": "s" }, { "role": "user", "content": "u" }])
    );
}
