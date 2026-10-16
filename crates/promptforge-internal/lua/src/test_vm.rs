//! The test VM builder: a section VM set up the way section setup builds
//! one, through construction, the run's limits, Engine injection, and the
//! persistent Engine globals, with the control globals and coroutine shims
//! on request, handed back with named handles to what the VM shares. Also
//! the tool fixtures as data and the scope a VM's offers resolve to.

use std::ops::Deref;
use std::sync::{Arc, Mutex};

use mlua::FromLuaMulti;
use promptforge_model_client::model::ModelSet;
use promptforge_types::emitter::Emitter;
use promptforge_types::seed::RunSeed;
use promptforge_types::tools::{ToolDescriptor, ToolId};
use promptforge_types::untrusted::GuardNonce;
use promptforge_vfs::{Access, Origin, VfsRef};
use serde_json::{Value as Json, json};

use crate::tests::recording::null_emitter;
use crate::{Argv, MessageList, SectionText, SectionVm, ToolSet};

/// The one guard-nonce seed of every test VM.
pub(crate) const TEST_SEED: RunSeed = RunSeed::from_bytes([0x73; 32]);

/// The section a test VM reports under unless `section` names another.
pub(crate) const TEST_SECTION: &str = "Test";

/// The guard nonce [`TEST_SEED`] mints, for a test that drives a section
/// VM's construction itself.
#[track_caller]
pub(crate) fn test_nonce() -> GuardNonce {
    GuardNonce::from_seed(&TEST_SEED)
}

/// A fresh default handle's access capability: the store is declared at
/// the root and the vended identity is the test's own, so seeding through
/// the store view and the VM's store ops never meet a second live identity.
#[track_caller]
pub(crate) fn fresh_access() -> Arc<Access> {
    Arc::new(
        VfsRef::default()
            .acquire(Origin::new("lua test fixture"))
            .expect("the stock backend acquires"),
    )
}

/// A tool as data under `id`, described as `<id> tool`.
#[track_caller]
pub(crate) fn tool_at(id: &str) -> ToolDescriptor {
    ToolDescriptor::new(
        ToolId::parse(id).expect("valid id"),
        format!("{id} tool"),
        json!({ "type": "object" }),
    )
}

/// The `tools/echo` fixture tool as data. No implementation is ever
/// called: the VM binds descriptors and dispatch sees canned outputs.
#[track_caller]
pub(crate) fn echo_tool() -> ToolDescriptor {
    tool_at("tools/echo")
}

/// A test VM whose `tools` and `plugins` tables are installed over `set`.
#[track_caller]
pub(crate) fn lua_over(set: ToolSet) -> BuiltVm {
    TestVm::new().tools(set).build()
}

/// The wire names a round on `vm` advertises, in order.
#[track_caller]
pub(crate) fn scoped(vm: &SectionVm) -> Vec<String> {
    let (set, runtime) = vm.tool_bag_handles();
    crate::current_tool_bindings(&set, &runtime)
        .expect("the scope resolves")
        .iter()
        .map(|binding| binding.alias().to_owned())
        .collect()
}

/// A section VM's setup as data. Unset, the VM shares an empty tool and
/// model set, reports nowhere under [`TEST_SECTION`], gets a fresh store,
/// empty `args` and `sys`, a frozen nil `argv`, and an empty `var`.
pub(crate) struct TestVm<'a> {
    tools: ToolSet,
    models: ModelSet,
    access: Option<Arc<Access>>,
    emitter: Emitter,
    section: &'a str,
    argv: Argv<'a>,
    var: Option<&'a Json>,
    limits: Option<(usize, u32)>,
    raw_model_ids: bool,
    shims: bool,
    messages: bool,
}

impl<'a> TestVm<'a> {
    #[track_caller]
    pub(crate) fn new() -> Self {
        Self {
            tools: ToolSet::default(),
            models: ModelSet::default(),
            access: None,
            emitter: null_emitter(),
            section: TEST_SECTION,
            argv: Argv::Frozen(None),
            var: None,
            limits: None,
            raw_model_ids: false,
            shims: false,
            messages: false,
        }
    }

    /// The run's tool set, shared with the VM.
    #[track_caller]
    pub(crate) fn tools(mut self, tools: ToolSet) -> Self {
        self.tools = tools;
        self
    }

    /// The run's model set, shared with the VM.
    #[track_caller]
    pub(crate) fn models(mut self, models: ModelSet) -> Self {
        self.models = models;
        self
    }

    /// The VFS access the `store` table operates through.
    #[track_caller]
    pub(crate) fn access(mut self, access: Arc<Access>) -> Self {
        self.access = Some(access);
        self
    }

    /// The emitter every setup step and the teardown report through.
    #[track_caller]
    pub(crate) fn emitter(mut self, emitter: &Emitter) -> Self {
        self.emitter = emitter.clone();
        self
    }

    /// The section every report names.
    #[track_caller]
    pub(crate) fn section(mut self, section: &'a str) -> Self {
        self.section = section;
        self
    }

    /// How Engine injection installs `argv`.
    #[track_caller]
    pub(crate) fn argv(mut self, argv: Argv<'a>) -> Self {
        self.argv = argv;
        self
    }

    /// The value the guarded `var` starts from.
    #[track_caller]
    pub(crate) fn var(mut self, var: &'a Json) -> Self {
        self.var = Some(var);
        self
    }

    /// The run's Lua heap ceiling and `log()` event budget, applied before
    /// Engine injection.
    #[track_caller]
    pub(crate) fn limits(mut self, memory_bytes: usize, log_events: u32) -> Self {
        self.limits = Some((memory_bytes, log_events));
        self
    }

    /// The raw-model-id fallback `models.get` takes for an undeclared alias.
    #[track_caller]
    pub(crate) fn raw_model_ids(mut self) -> Self {
        self.raw_model_ids = true;
        self
    }

    /// The scheduler-mode control globals, whose prompt reads find no
    /// section, and every coroutine yield shim.
    #[track_caller]
    pub(crate) fn shims(mut self) -> Self {
        self.shims = true;
        self
    }

    /// A fresh `messages.new()` list as the global `msgs`.
    #[track_caller]
    pub(crate) fn messages(mut self) -> Self {
        self.messages = true;
        self
    }

    /// Builds the VM, failing the test at the first setup step that fails.
    #[track_caller]
    pub(crate) fn build(self) -> BuiltVm {
        let tools = Arc::new(self.tools);
        let models = Arc::new(Mutex::new(self.models));
        let access = self.access.unwrap_or_else(fresh_access);
        let mut vm =
            SectionVm::new_for_section(&test_nonce(), &tools, &models, &self.emitter, self.section)
                .expect("the test VM builds");
        if let Some((memory_bytes, log_events)) = self.limits {
            vm.apply_lua_limits(memory_bytes, log_events)
                .expect("the limits apply");
        }
        if self.raw_model_ids {
            vm.allow_raw_model_ids();
        }
        vm.inject_values_with_var("", &json!({}), &access, self.var, self.argv)
            .expect("the Engine values inject");
        vm.install_engine_globals(&self.emitter, self.section)
            .expect("the Engine globals install");
        if self.shims {
            vm.install_scheduler_control_globals(|_| {
                Ok::<Option<SectionText>, std::convert::Infallible>(None)
            })
            .expect("the control globals install");
            vm.install_coro_shims(1)
                .expect("the coroutine shims install");
        }
        let messages = self.messages.then(|| {
            let list = MessageList::default();
            let userdata = vm
                .lua()
                .create_userdata(list.clone())
                .expect("the list userdata builds");
            vm.lua()
                .globals()
                .raw_set("msgs", userdata)
                .expect("the global msgs installs");
            list
        });
        BuiltVm {
            vm,
            models,
            access,
            messages,
            emitter: self.emitter,
            section: self.section.to_owned(),
        }
    }
}

/// A built test VM: the section VM, which it derefs to, beside the model
/// set and access it shares and the list `msgs` holds.
pub(crate) struct BuiltVm {
    vm: SectionVm,
    /// The run's model set the VM shares.
    pub(crate) models: Arc<Mutex<ModelSet>>,
    /// The access the VM's `store` table operates through.
    pub(crate) access: Arc<Access>,
    messages: Option<MessageList>,
    emitter: Emitter,
    section: String,
}

impl BuiltVm {
    /// Evaluates `source` on the VM's main state under the chunk name
    /// `probe`, failing the test when it raises.
    #[track_caller]
    pub(crate) fn eval<T: FromLuaMulti>(&self, source: &str) -> T {
        self.vm
            .lua()
            .load(source)
            .set_name("probe")
            .eval()
            .expect("the test chunk runs")
    }

    /// Runs `source` on the VM's main state under the chunk name `probe`
    /// for its effects, failing the test when it raises.
    #[track_caller]
    pub(crate) fn exec(&self, source: &str) {
        self.vm
            .lua()
            .load(source)
            .set_name("probe")
            .exec()
            .expect("the test chunk runs");
    }

    /// Runs `source` on the VM's main state under the chunk name `probe`
    /// and returns what it raises, failing the test when it runs clean.
    #[track_caller]
    pub(crate) fn eval_err(&self, source: &str) -> mlua::Error {
        self.vm
            .lua()
            .load(source)
            .set_name("probe")
            .exec()
            .expect_err("the test chunk must raise")
    }

    /// The list the global `msgs` holds.
    #[track_caller]
    pub(crate) fn messages(&self) -> &MessageList {
        self.messages
            .as_ref()
            .expect("the test VM was built with messages()")
    }

    /// Tears the VM down through its emitter, under its section.
    #[track_caller]
    pub(crate) fn teardown(self) {
        self.vm.teardown(&self.emitter, &self.section);
    }
}

impl Deref for BuiltVm {
    type Target = SectionVm;

    fn deref(&self) -> &SectionVm {
        &self.vm
    }
}
