//! 转写桥：内核的效应 + agent 的对话 -> rig-tui `RenderBatch` -> look -> 行。
//!
//! rig-tui 的 `renderers::add` 是 `pub(crate)`，所以建批由我们自己驱动，一帧三步：
//!
//! 1. [`rebuild_batch`]：从 **对话**（assistant 正文、tool call、送达的 entry、
//!    内容块、aside）和 **还没结算的效应**（排队/运行/被闸门按住的调用）建出
//!    `RenderBatch`。每个请求先填上游的默认行（`ToolRender::default_lines` 等），
//!    业务 look 之后可以整块覆盖。
//! 2. [`run_looks`]：跑一遍 rig-tui 的 `TranscriptRender`，业务 look 改写
//!    `RenderOutput::lines`；`RenderSystems::Decorate` 里补子 agent 的调用树。
//! 3. [`collect_rows`]：把 look 的产出交给 [`crate::transcript::Transcript`] 这个
//!    对话引擎（排序、折行、流式增补、finalize），再拼上还没回答的 `Ask`。
//!
//! 以前这里写死过一句中文"回复"，由每两帧吐一个字的假流式驱动 —— 已经删掉：
//! 正文现在只来自真实的 `Partial`（运行中的流）或对话里的 assistant 消息。

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::Duration;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use rig_core::conversation::{self as talk, Note, pairing::answered_by};
use rig_core::effect::EffectId;
use rig_core::message::{
    AssistantMessage, CallId, Message, Text, ToolCall, ToolResult, ToolResultContent, UserContent,
};
use rig_ecs::prelude::*;
use rig_harness::prelude::{CallOutput, ShownAgent, ToolProgress};
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style, Stylize};
use rig_tui::ratatui::text::{Line, Span};
use rig_tui::{
    AsideRender, CallItem, CallState, ChildCall, ContentRender, EntryRender, LookLine, RenderBatch,
    RenderContent, RenderInput, RenderKey, RenderOutput, RenderRequest, RenderSystems, Theme as RTheme,
    ToolRender, TranscriptRender,
};

use crate::editor::SubmittedPrompts;
use crate::frame::{PanelAreas, PanelRoots};
use crate::pane::TranscriptRows;
use crate::panels::{KeyedRow, PanelId};
use crate::transcript::{
    BlockId, Slot, TextId, Transcript as TranscriptEngine, row_hash, wrap_lines,
};

/// 已结算的调用（效应实体结算后会被回收，转写需要自己记历史）。
///
/// 公开形状保持不变（别的模块 `init_resource` / 读它）。
#[derive(Resource, Default)]
pub struct CompletedCalls(pub Vec<(ToolCall, ToolResult)>);

/// 一个已结算调用的事实：结算后效应实体就没了，转写要自己留。
#[derive(Clone, Debug)]
pub struct CallFact {
    /// 调用自己的效应 id（结算后 `Effect` 已回收，靠它还原调用树）。
    pub effect: EffectId,
    /// 启动它的效应。
    pub parent: Option<EffectId>,
    /// 它属于哪个 agent（结算时的名字，子 agent 可能已经没了）。
    pub agent: Option<String>,
    /// 它在"对话之外的调用"里的固定序号（`RenderKey::Streaming` 的位置）。
    pub ordinal: usize,
    pub call: ToolCall,
    pub result: ToolResult,
    /// 从开始到结算的时长（真实时钟）。
    pub took: Option<Duration>,
}

/// 已结算调用的历史，按结算顺序。
///
/// 转写用它还原子 agent 的调用树、`took` 和"已经结算但还没落进对话"的卡片。
///
/// TODO(past): 恢复会话时更早的效应记录（上游 `PastEffects` / `EffectRecorded`）
/// 还没接：重启后旧的 `took` 与调用树是空的。
#[derive(Resource, Default)]
pub struct CallHistory(pub Vec<CallFact>);

impl CallHistory {
    /// `id` 这个调用的记录。
    fn by_call(&self, id: &CallId) -> Option<&CallFact> {
        self.0.iter().find(|fact| &fact.call.id == id)
    }
}

/// 已记入历史的效应。
#[derive(Component)]
pub struct Recorded;

/// 对话之外的调用在 `RenderKey::Streaming` 里的位置。
///
/// 按效应 id 第一次出现的顺序发号；结算后由 [`CallFact::ordinal`] 沿用，
/// 这样一整轮里同一张卡片不会换位置（`panels` 的前缀 diff 才能原地更新）。
#[derive(Resource, Default)]
pub struct LiveOrdinals {
    next: usize,
    by_effect: HashMap<EffectId, usize>,
}

impl LiveOrdinals {
    /// 效应 `effect` 在 `RenderKey::Streaming` 里的位置（第一次见到时发号）。
    pub fn of(&mut self, effect: EffectId) -> usize {
        if let Some(at) = self.by_effect.get(&effect) {
            return *at;
        }
        let at = self.next;
        self.next += 1;
        self.by_effect.insert(effect, at);
        at
    }
}

/// 正在流式输出的回复。
///
/// **真实来源**：运行中 turn 的 `Partial` 组件（正文、推理、还在流参数的调用）。
/// 没有模型时它一直是空的；以前那句写死的中文回复和"每两帧一个字"的假流式已删。
#[derive(Resource, Default)]
pub struct ReplyStream {
    /// 到目前为止的正文。
    pub text: String,
    /// 到目前为止的推理。
    pub reasoning: String,
    /// 正文真实变长的次数（增量帧数；不是伪造的节奏）。
    pub ticks: u32,
    /// 这一轮是否已经结束（没有 `Partial` 了）。
    pub finished: bool,
}

/// 这一帧对话里没有 `RenderKey` 的文字块和 aside 落点。
///
/// 内部记账资源：由 [`rebuild_batch`] 写、[`collect_rows`] 读；放在这里只是
/// 因为两个系统都是公开的。
///
/// [`rebuild_batch`] 从对话里读出来，[`collect_rows`] 交给对话引擎 —— 上游也是
/// 在 `transcript::message_lines` 里直接画这些（它们不进 `RenderBatch`）。
#[derive(Resource, Default)]
pub struct Dialogue {
    /// 文字块：用户请求、assistant 正文/推理。
    text: Vec<(TextId, Vec<Line<'static>>)>,
    /// aside 的落点（`RenderKey::Aside` 里只有 id，落点来自 `Aside.after`）。
    slots: Vec<(AsideId, Slot)>,
    /// 没有可读对话时，用本进程提交的提示词兜底。
    from_prompts: bool,
    /// 对话里已经画过的调用：结算历史里不再重复画它们。
    calls: HashSet<CallId>,
    /// 对话里已经有正在流的那段正文：这一帧不再画流式块（提交和 `Partial`
    /// 消失之间会有一帧两份）。
    has_reply: bool,
}

impl Dialogue {
    fn clear(&mut self) {
        self.text.clear();
        self.slots.clear();
        self.calls.clear();
        self.has_reply = false;
        self.from_prompts = true;
    }
}

/// 一帧对话建出来的东西（纯函数产物，测试直接断言）。
#[derive(Default)]
struct DialogueFrame {
    /// tool call / 送达的 entry / 内容块 / aside 的请求。
    requests: Vec<(RenderKey, RenderInput)>,
    /// 没有 `RenderKey` 的文字块。
    text: Vec<(TextId, Vec<Line<'static>>)>,
    /// aside 的落点。
    slots: Vec<(AsideId, Slot)>,
    /// 对话里已有的调用 id。
    calls: HashSet<CallId>,
}

/// 调用之间的关系：谁启动了谁（子 agent 的调用树）。
///
/// 上游 `transcript::CallTree` 的对位，数据来自 `Effect.parent`（还没结算的）
/// 和 [`CallFact::parent`]（结算过的）。
///
/// TODO(model-calls): 只把工具调用当节点；上游还会画模型调用节点
/// （`ChildCall { call: None, name: "vendor/model" }`，来自 `ModelRequest`/`SentTo`）。
/// 非调用的效应现在是从它下面穿过去（深度不变），所以子 agent 的调用链是通的，
/// 只是少了"模型调用"那一层。
#[derive(Default)]
struct CallTree {
    /// 每个效应作为调用时给 look 的信息（`depth` 在取子树时填）。
    nodes: HashMap<EffectId, ChildCall>,
    /// 每个效应的父效应。
    parents: HashMap<EffectId, EffectId>,
    /// 父效应 -> 子效应。
    children: HashMap<EffectId, Vec<EffectId>>,
}

impl CallTree {
    /// 记下效应 `effect`（父是 `parent`）作为调用长什么样。
    fn add(&mut self, effect: EffectId, parent: Option<EffectId>, call: ChildCall) {
        if let Some(parent) = parent.filter(|parent| *parent != effect) {
            self.parents.insert(effect, parent);
            self.children.entry(parent).or_default().push(effect);
        }
        self.nodes.insert(effect, call);
    }

    /// `parent` 下面的调用，按效应 id 的顺序、各带自己的深度（上游
    /// `CallFacts::below`：不是调用的效应穿过它，depth 不变）。
    fn children_of(&self, parent: EffectId) -> Vec<ChildCall> {
        let mut out = Vec::new();
        self.below(parent, 0, &mut HashSet::new(), &mut out);
        out
    }

    fn below(
        &self,
        parent: EffectId,
        depth: usize,
        seen: &mut HashSet<EffectId>,
        out: &mut Vec<ChildCall>,
    ) {
        let mut children = self.children.get(&parent).cloned().unwrap_or_default();
        children.sort_unstable();
        for child in children {
            if !seen.insert(child) {
                continue;
            }
            match self.nodes.get(&child) {
                Some(node) => {
                    out.push(ChildCall {
                        depth,
                        ..node.clone()
                    });
                    self.below(child, depth + 1, seen, out);
                }
                // 不是调用（比如一次模型调用之外的效应）：调用从它下面穿过。
                None => self.below(child, depth, seen, out),
            }
        }
    }
}

/// 建批要读的内核数据（分组，免得系统参数太多）。
#[derive(SystemParam)]
pub struct KernelData<'w, 's> {
    /// 还没结算的调用效应。
    requests: Query<
        'w,
        's,
        (
            &'static ToolRequest,
            &'static Effect,
            Option<&'static Outcome>,
            Option<&'static Running>,
            Option<&'static Holds>,
            Option<&'static ForAgent>,
            Option<&'static ToolProgress>,
            Option<&'static CallOutput>,
        ),
        Without<Settled>,
    >,
    /// 持有/agent 的名字（`Held(reasons)` 里那句"等谁"、子调用的 agent 名）。
    names: Query<'w, 's, &'static Name>,
}

/// 用户正在看的 agent，和它运行中的流式回复。
#[derive(SystemParam)]
pub struct ShownTurn<'w, 's> {
    shown: Option<Res<'w, ShownAgent>>,
    conversations: Query<'w, 's, &'static Conversation>,
    active: Query<'w, 's, &'static ActiveTurn>,
    calls: Query<'w, 's, &'static Calls>,
    partials: Query<'w, 's, &'static Partial>,
}

impl ShownTurn<'_, '_> {
    /// 用户正在看的 agent：`ShownAgent` 指向、而且真的有对话的那个。
    fn agent(&self) -> Option<Entity> {
        let shown = self.shown.as_ref()?.0?;
        self.conversations.contains(shown).then_some(shown)
    }

    /// 它的对话；`None` = 没有可读的会话（比如只跑工具的 demo）。
    fn conversation(&self) -> Option<&Conversation> {
        self.conversations.get(self.agent()?).ok()
    }

    /// 它运行中 turn 的最新 `Partial`（正文/推理/流式调用）。
    fn partial(&self) -> Option<&Partial> {
        let turn = self.active.get(self.agent()?).ok()?;
        let calls = self.calls.get(turn.turn()).ok()?;
        calls
            .iter()
            .filter_map(|call| self.partials.get(call).ok())
            .next_back()
    }
}

fn key_of(parts: &[&str]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut hasher);
    hasher.finish()
}

/// 把运行中 turn 的 `Partial` 读进 [`ReplyStream`] —— 真实的流式，不造字。
fn track_reply_stream(mut stream: ResMut<ReplyStream>, turn: ShownTurn) {
    match turn.partial() {
        Some(partial) => {
            // `ticks` 只数真实的增量帧（正文变长），不是伪造的节奏。
            if partial.text.len() > stream.text.len() {
                stream.ticks = stream.ticks.wrapping_add(1);
            }
            stream.text.clone_from(&partial.text);
            stream.reasoning.clone_from(&partial.reasoning);
            stream.finished = false;
        }
        None => {
            // 这一轮没有 `Partial` 了：回复已经落进对话（或者被停掉），
            // 流式块不再画，正文从对话里读。
            stream.finished = true;
        }
    }
}

/// 在 Review 阶段（Commit 之前）把已答复的调用抄进历史。
///
/// 抄的东西：调用的结果（[`CompletedCalls`]）、效应 id/父效应/agent/时长
/// （[`CallHistory`]）—— 效应实体结算后会被回收，这些是转写画调用树和
/// "结算了但还没落进对话"卡片的唯一来源。
fn record_calls(
    mut commands: Commands,
    calls: Query<
        (
            Entity,
            &ToolRequest,
            &Outcome,
            &Effect,
            Option<&ForAgent>,
            Option<&Started>,
        ),
        Without<Recorded>,
    >,
    names: Query<&Name>,
    mut completed: ResMut<CompletedCalls>,
    mut history: ResMut<CallHistory>,
    mut ordinals: ResMut<LiveOrdinals>,
) {
    for (entity, request, outcome, effect, agent, started) in calls.iter() {
        let result = tool_result_of(outcome, &request.call);
        completed.0.push((request.call.clone(), result.clone()));
        let took = started
            .and_then(|started| started.at.elapsed().ok())
            .filter(|took| !took.is_zero());
        let agent = agent
            .and_then(|ForAgent(agent)| names.get(*agent).ok())
            .map(|name| name.as_str().to_owned());
        history.0.push(CallFact {
            effect: effect.id(),
            parent: effect.parent,
            agent,
            ordinal: ordinals.of(effect.id()),
            call: request.call.clone(),
            result,
            took,
        });
        commands.entity(entity).insert(Recorded);
    }
}

/// 内核的 outcome -> look 读的 `ToolResult`。
fn tool_result_of(outcome: &Outcome, call: &ToolCall) -> ToolResult {
    let tool = outcome.tool_result();
    ToolResult {
        call: call.id.clone(),
        name: call.function.name.clone(),
        content: vec![ToolResultContent::Text(Text::new(tool.output().render()))],
        is_error: tool.is_error(),
    }
}

/// 一个还没结算的调用（这是建批时 `Streaming` 位置的真实来源）。
struct LiveCall {
    effect: EffectId,
    parent: Option<EffectId>,
    call: ToolCall,
    agent: Option<String>,
    state: CallState,
    result: Option<ToolResult>,
    output: Option<String>,
    ordinal: usize,
}

/// 还没结算的调用的状态（和上游 `render::CallStates` 同一套判断顺序）。
fn live_state(
    outcome: Option<&Outcome>,
    running: Option<&Running>,
    holds: Option<&Holds>,
    progress: Option<&ToolProgress>,
    names: &Query<&Name>,
) -> CallState {
    if let Some(holds) = holds.filter(|holds| holds.len() > 0) {
        let reasons = holds
            .iter()
            .map(|hold| {
                names
                    .get(hold)
                    .map(|name| name.as_str().to_owned())
                    .unwrap_or_else(|_| "unnamed hold".to_owned())
            })
            .collect();
        return CallState::Held(reasons);
    }
    if outcome.is_some() {
        return CallState::Done;
    }
    if running.is_some() {
        return CallState::Running(progress.map(|ToolProgress(line)| line.clone()));
    }
    CallState::Waiting
}

/// 读内核的活效应，建出这一帧的 `RenderBatch`（对话 + 对话之外的调用）。
///
/// `width` 取上一帧 Layout 给转写面板的文字列数；Layout 还没跑过就沿用
/// `RenderBatch` 里上一次的宽度（不写死 78，也绝不拿 1 列去折一段假的）。
pub fn rebuild_batch(
    mut batch: ResMut<RenderBatch>,
    mut ordinals: ResMut<LiveOrdinals>,
    history: Res<CallHistory>,
    stream: Res<ReplyStream>,
    mut dialogue: ResMut<Dialogue>,
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
    turn: ShownTurn,
    kernel: KernelData,
) {
    batch.requests.clear();
    // 宽度：这一帧 Layout 给的列数；没给就沿用上一次（`batch` 是常驻资源）。
    let layout = roots
        .entity(PanelId::Transcript)
        .and_then(|root| areas.get(root))
        .map(|area| area.cols)
        .filter(|cols| *cols > 0);
    if let Some(cols) = layout {
        batch.width = cols;
    }
    batch.pane = roots.entity(PanelId::Transcript);
    batch.agent = turn.agent();
    let theme = RTheme::default();
    dialogue.clear();

    // 1) 还没结算的调用：先收成自己的数据，后面既进 batch 也进调用树。
    let mut live: Vec<LiveCall> = Vec::new();
    for (request, effect, outcome, running, holds, agent, progress, output) in
        kernel.requests.iter()
    {
        let agent = agent
            .and_then(|ForAgent(agent)| kernel.names.get(*agent).ok())
            .map(|name| name.as_str().to_owned());
        live.push(LiveCall {
            effect: effect.id(),
            parent: effect.parent,
            call: request.call.clone(),
            agent,
            state: live_state(outcome, running, holds, progress, &kernel.names),
            result: outcome.map(|outcome| tool_result_of(outcome, &request.call)),
            output: output.map(CallOutput::text).filter(|text| !text.is_empty()),
            ordinal: ordinals.of(effect.id()),
        });
    }
    live.sort_by_key(|call| call.effect);

    // 2) 调用树：结算过的 + 还在跑的（子 agent 的调用由此挂到发起它的调用下）。
    let mut tree = CallTree::default();
    for fact in &history.0 {
        tree.add(
            fact.effect,
            fact.parent,
            ChildCall {
                depth: 0,
                call: Some(fact.call.function.clone()),
                name: fact.call.function.name.as_str().to_owned(),
                agent: fact.agent.clone(),
                state: CallState::Done,
                failed: fact.result.is_error,
                answer: Some(tool_result_text(&fact.result)),
            },
        );
    }
    for call in &live {
        tree.add(
            call.effect,
            call.parent,
            ChildCall {
                depth: 0,
                call: Some(call.call.function.clone()),
                name: call.call.function.name.as_str().to_owned(),
                agent: call.agent.clone(),
                state: call.state.clone(),
                failed: call.result.as_ref().is_some_and(|result| result.is_error),
                answer: call.result.as_ref().map(tool_result_text),
            },
        );
    }

    // 3) 对话：assistant 正文/推理进文字块，调用/entry/内容块/aside 进 batch。
    if let Some(conversation) = turn.conversation() {
        let frame = conversation_frame(conversation, &history, &tree, &theme, batch.width);
        for (key, input) in frame.requests {
            let output = RenderOutput {
                lines: default_lines(&input, &theme),
                decorations: Vec::new(),
            };
            batch.requests.insert(
                key,
                RenderRequest {
                    input,
                    output,
                },
            );
        }
        dialogue.text = frame.text;
        dialogue.slots = frame.slots;
        dialogue.calls = frame.calls;
        // 对话里已经有内容就不再拿本进程的提示词兜底（那是同一句话的另一份）。
        dialogue.from_prompts = conversation.messages().is_empty();
        // 回复已经提交进对话：流式块让位，别在提交和 `Partial` 消失之间画两份。
        dialogue.has_reply =
            !stream.text.is_empty() && conversation_has_text(conversation, &stream.text);
    }

    // 4) 对话之外的调用：结算历史（结算了但还没落进对话）+ 还在跑的效应。
    //    上游用 `RenderKey::Streaming(n)` 表示"还没进任何消息的调用"。
    let mut rendered: HashSet<CallId> = dialogue.calls.clone();
    for fact in &history.0 {
        if !rendered.insert(fact.call.id.clone()) {
            continue;
        }
        let call = ToolRender {
            call: fact.call.clone(),
            result: Some(fact.result.clone()),
            state: CallState::Done,
            theme,
            // TODO(facts): `ToolRender::facts`（`ToolResultContext`，退出码/进度/
            // 截断标记）还没有来源：内核的 `Outcome` 只给了输出文本。
            facts: None,
            took: fact.took,
            output: None,
            children: tree.children_of(fact.effect),
            item: Some(CallItem::Streaming(fact.ordinal)),
        };
        batch.requests.insert(
            RenderKey::Streaming(fact.ordinal),
            RenderRequest {
                input: RenderInput::Tool(call),
                output: RenderOutput::default(),
            },
        );
    }
    for call in &live {
        if !rendered.insert(call.call.id.clone()) {
            continue;
        }
        // TODO(streaming): 模型还在流参数、调用效应还没出现时，上游会先画一个
        // `CallState::Streaming` 的 `ToolRender`（位置取 `Partial::calls` 的下标）。
        // 这里等效应出现才画，免得用一个会变的合成位置。
        let render = ToolRender {
            call: call.call.clone(),
            result: call.result.clone(),
            state: call.state.clone(),
            theme,
            facts: None,
            took: None,
            output: call.output.clone(),
            children: tree.children_of(call.effect),
            item: Some(CallItem::Streaming(call.ordinal)),
        };
        batch.requests.insert(
            RenderKey::Streaming(call.ordinal),
            RenderRequest {
                input: RenderInput::Tool(render),
                output: RenderOutput::default(),
            },
        );
    }
    // 默认行在 look 跑之前填好（上游 `Renderers::output` 的 Collect 遍也这样）；
    // 业务 look 在 `run_looks` 里整块覆盖，`Decorate` 再追加具名贡献。
    for request in batch.requests.values_mut() {
        if request.output.lines.is_empty() && request.output.decorations.is_empty() {
            request.output.lines = default_lines(&request.input, &theme);
        }
    }
}

/// 一个请求在没有 look 时画什么（上游 `RenderInput::default_lines` 的公开部分）。
fn default_lines(input: &RenderInput, theme: &RTheme) -> Vec<LookLine> {
    match input {
        // 默认 look 已经带了调用状态色带。
        RenderInput::Tool(call) => call.default_lines(),
        RenderInput::Entry(entry) => on_band(entry.default_lines(), theme.tool)
            .into_iter()
            .map(LookLine::from)
            .collect(),
        RenderInput::Content(content) => content
            .default_lines()
            .into_iter()
            .map(LookLine::from)
            .collect(),
        // 上游把非 note aside 交给画它的插件（默认不画）；本前端没有这种插件，
        // 全不画就等于"aside 全丢"，所以给一个兜底：种类 + 来源 + 内容。
        RenderInput::Aside(aside) => on_band(aside_fallback_lines(aside), theme.tool)
            .into_iter()
            .map(LookLine::from)
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// 对话 -> 请求 / 文字块（纯函数，测试直接喂构造的 `Conversation`）
// ---------------------------------------------------------------------------

/// 从对话建出转写要的请求、文字块和 aside 落点。
///
/// 对位上游 `transcript::message_lines` + `renderers::{entry, content, aside}`：
/// 消息里的文字进文字块，tool call / 送达的 entry / 内容块 / aside 进 `RenderBatch`。
/// 一条调用结果画在它的调用下面（`next` 那条消息里的 `ToolResult`），不单独成块。
fn conversation_frame(
    conversation: &talk::Conversation,
    history: &CallHistory,
    tree: &CallTree,
    theme: &RTheme,
    width: u16,
) -> DialogueFrame {
    let mut frame = DialogueFrame::default();
    let entries: Vec<_> = conversation.entries().collect();
    for (at, entry) in entries.iter().enumerate() {
        let next = entries.get(at + 1).map(|entry| entry.message);
        match entry.message {
            Message::System { .. } => {}
            Message::User { content } => {
                // 这条消息装着工具结果时，它旁边的文字是压缩/清除的说明，
                // 不是用户敲的（上游同样按 dim 缩进画）。
                let results = content
                    .iter()
                    .any(|item| matches!(item, UserContent::ToolResult(_)));
                for (block, item) in content.iter().enumerate() {
                    match item {
                        UserContent::Text(text) => {
                            if let Some((label, body)) = talk::attached_file(&text.text) {
                                let header = Line::styled(
                                    format!("  [{label}, {} lines]", body.lines().count()),
                                    Style::new().dim(),
                                );
                                let mut lines = vec![Line::default(), header];
                                lines.extend(rig_tui::text_rows(body, Style::new().dim()));
                                frame.text.push((
                                    TextId::Message { at, block },
                                    on_band(lines, theme.tool),
                                ));
                                continue;
                            }
                            // rig 自己写的停止标记、以及"没挂上的附件"的副本都不画。
                            if entry.is_stop_marker()
                                || talk::unattached(&text.text).is_some()
                            {
                                continue;
                            }
                            // 有作者、而且不是用户自己：这是 agent/插件送达的文字。
                            let other = entry.author.is_some_and(|author| {
                                author.kind != AuthorKind::User || author.title.is_some()
                            });
                            match entry.author.filter(|_| other) {
                                Some(author) => {
                                    let body = author
                                        .header()
                                        .and_then(|header| {
                                            text.text.strip_prefix(&header)
                                        })
                                        .map_or(text.text.as_str(), |body| {
                                            body.trim_start_matches('\n')
                                        });
                                    frame.requests.push((
                                        RenderKey::Entry(at, block),
                                        RenderInput::Entry(EntryRender {
                                            id: Some(entry.id),
                                            plugin: plugin_name(author),
                                            title: author.label(),
                                            text: body.to_owned(),
                                        }),
                                    ));
                                }
                                None if results => {
                                    frame.text.push((
                                        TextId::Message { at, block },
                                        on_band(
                                            rig_tui::text_rows(
                                                &text.text,
                                                Style::new().dim(),
                                            ),
                                            theme.tool,
                                        ),
                                    ));
                                }
                                None => {
                                    let mut lines = vec![Line::default()];
                                    lines.extend(user_lines(&text.text));
                                    frame
                                        .text
                                        .push((TextId::Message { at, block }, on_band(lines, theme.user)));
                                }
                            }
                        }
                        // 已经画在它的调用下面了。
                        UserContent::ToolResult(_) => {}
                        other => {
                            if let Some(content) = content_of_user(other) {
                                frame.requests.push((
                                    RenderKey::Content(at, block),
                                    RenderInput::Content(ContentRender {
                                        message: Some(entry.id),
                                        index: block,
                                        content,
                                        assistant: false,
                                    }),
                                ));
                            }
                        }
                    }
                }
            }
            Message::Assistant(assistant) => {
                let results = answers(assistant, next);
                let mut calls = (0..).zip(results);
                for (block, item) in assistant.content.iter().enumerate() {
                    match item {
                        AssistantContent::Text(text) => {
                            let mut lines = vec![Line::default()];
                            lines.extend(reply_lines(&text.text, width, theme));
                            frame.text.push((TextId::Message { at, block }, lines));
                        }
                        AssistantContent::Reasoning(reasoning) => {
                            frame.text.push((
                                TextId::Message { at, block },
                                reasoning_lines(&reasoning.text, width, theme),
                            ));
                        }
                        AssistantContent::ToolCall(call) => {
                            let (index, result) = calls.next().unwrap_or((0, None));
                            let at_call = CallRef {
                                reply: entry.id,
                                index,
                            };
                            let fact = history.by_call(&call.id);
                            let effect = fact.map(|fact| fact.effect);
                            frame.calls.insert(call.id.clone());
                            let render = ToolRender {
                                call: call.clone(),
                                result: result.cloned(),
                                state: match result {
                                    Some(_) => CallState::Done,
                                    None => CallState::Running(None),
                                },
                                theme: *theme,
                                // TODO(facts): 同 `rebuild_batch`：`ToolResultContext` 没有来源。
                                facts: None,
                                took: fact.and_then(|fact| fact.took),
                                output: None,
                                children: effect
                                    .map(|effect| tree.children_of(effect))
                                    .unwrap_or_default(),
                                item: Some(CallItem::Reply(at_call)),
                            };
                            frame.requests.push((
                                RenderKey::Tool(at, block),
                                RenderInput::Tool(render),
                            ));
                        }
                        AssistantContent::Image(image) => {
                            frame.requests.push((
                                RenderKey::Content(at, block),
                                RenderInput::Content(ContentRender {
                                    message: Some(entry.id),
                                    index: block,
                                    content: RenderContent::Image(image.clone()),
                                    assistant: true,
                                }),
                            ));
                        }
                        // provider 自己的一项，这里没有画法（上游跳过）。
                        AssistantContent::Opaque(_) => {}
                        // `AssistantContent` 是 non_exhaustive：以后的新块这里先不画。
                        _ => {}
                    }
                }
            }
        }
    }
    // aside（note 画成 entry，别的画成 aside）：落在它跟着的那条消息之后。
    for (id, aside) in conversation.asides() {
        let slot = match aside.after.and_then(|after| conversation.index_of(after)) {
            Some(at) => Slot::After(at),
            None => Slot::Top,
        };
        frame.slots.push((id, slot));
        let input = match Note::of(aside) {
            Some(note) => RenderInput::Entry(EntryRender {
                id: None,
                plugin: plugin_name(&aside.author),
                title: note.title,
                text: note.text,
            }),
            None => RenderInput::Aside(AsideRender {
                id,
                aside: aside.clone(),
                theme: *theme,
            }),
        };
        frame.requests.push((RenderKey::Aside(id), input));
    }
    // TODO(stops): 被停掉的 turn 下画的 `[stopped]`（上游 `transcript::stop_lines`，
    // 读 turn 记录的 `TurnEnd::Stopped`）还没接。
    frame
}

/// 对话里的 assistant 正文是否已经包含 `text`（流式正文通常只是它的前缀）。
fn conversation_has_text(conversation: &talk::Conversation, text: &str) -> bool {
    conversation.messages().iter().any(|message| match &**message {
        Message::Assistant(assistant) => assistant.content.iter().any(|item| {
            matches!(item, AssistantContent::Text(block) if block.text.contains(text))
        }),
        _ => false,
    })
}

/// 回复的每个调用后面那条消息里的结果，按调用顺序（上游 `transcript::answers`）。
fn answers<'a>(
    reply: &AssistantMessage,
    results: Option<&'a Message>,
) -> Vec<Option<&'a ToolResult>> {
    let calls = reply.tool_calls().map(|call| &call.id);
    let results = results.into_iter().flat_map(Message::results);
    answered_by(calls, results.map(|result| (&result.call, result)))
}

/// 用户自己敲的一段：`› ` + 粗体（上游 `renderers::user_lines` 的同形）。
fn user_lines(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 { "› " } else { "  " };
            Line::from(vec![
                Span::from(prefix).dim(),
                Span::from(line.replace('\t', "    ")).bold(),
            ])
        })
        .collect()
}

/// 回复正文按 markdown 排版成**未折**的行（上游 `renderers::reply_lines`：
/// `markdown::render`，表格按宽度收，折行交给对话引擎）。
fn reply_lines(text: &str, width: u16, theme: &RTheme) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    rig_tui::markdown::render(text, theme, Some(width))
        .into_iter()
        .map(|line| line.line().clone())
        .collect()
}

/// 推理：dim + italic 的 markdown（上游 `renderers::reasoning_lines`）。
fn reasoning_lines(text: &str, width: u16, theme: &RTheme) -> Vec<Line<'static>> {
    let style = Style::new().dim().italic();
    reply_lines(text, width, theme)
        .into_iter()
        .map(|line| line.patch_style(style))
        .collect()
}

/// 把行放到 `band` 上（上游 `transcript::on_band`：行自己已有底色的不覆盖）。
fn on_band(lines: impl IntoIterator<Item = Line<'static>>, band: RColor) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|mut line| {
            line.style.bg = line.style.bg.or(Some(band));
            line
        })
        .collect()
}

/// 非 note aside 的兜底 look：`⤶ aside · 种类 · 来源`，然后是它的值。
fn aside_fallback_lines(aside: &AsideRender) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        format!(
            "⤶ aside · {} · {}",
            aside.aside.kind,
            aside.aside.author.label()
        ),
        Style::new().magenta().bold(),
    )];
    let value = match &aside.aside.value {
        serde_json::Value::Null => String::new(),
        value => value.to_string(),
    };
    if !value.is_empty() {
        lines.extend(rig_tui::text_rows(&value, Style::new().dim()));
    }
    lines
}

/// 用户请求的兜底行（没有可读对话时，本进程提交的提示词）。
fn prompt_lines(text: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    lines.extend(user_lines(text));
    lines
}

/// 写这条的插件名（上游 `renderers::plugin_name`）。
fn plugin_name(author: &Author) -> String {
    match &author.kind {
        AuthorKind::Plugin(name) => name.clone(),
        _ => String::new(),
    }
}

/// 用户内容里的非文字块（上游 `RenderContent::of_user`，那边是 `pub(crate)`）。
fn content_of_user(content: &UserContent) -> Option<RenderContent> {
    match content {
        UserContent::Image(image) => Some(RenderContent::Image(image.clone())),
        UserContent::Audio(audio) => Some(RenderContent::Audio(audio.clone())),
        UserContent::Video(video) => Some(RenderContent::Video(video.clone())),
        UserContent::Document(document) => Some(RenderContent::Document(document.clone())),
        UserContent::Text(_) | UserContent::ToolResult(_) => None,
    }
}

/// 一个工具结果的文本（上游 `renderers::result_text`，那边是 `pub(crate)`）。
fn tool_result_text(result: &ToolResult) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut images = 0;
    for content in &result.content {
        match content {
            ToolResultContent::Text(text) => parts.push(text.text.clone()),
            ToolResultContent::Json { value } => parts.push(value.to_string()),
            ToolResultContent::Image(_) => images += 1,
        }
    }
    match images {
        0 => {}
        1 => parts.push("[1 image]".to_owned()),
        images => parts.push(format!("[{images} images]")),
    }
    parts.join("\n")
}

// ---------------------------------------------------------------------------
// 子 agent 的调用树（`RenderSystems::Decorate`）
// ---------------------------------------------------------------------------

/// 调用状态的说法（和上游默认 look 的用词一致）。
fn state_words(state: &CallState) -> String {
    match state {
        CallState::Streaming => "streaming".to_owned(),
        CallState::Waiting => "queued".to_owned(),
        CallState::Held(holds) => format!("held: {}", holds.join(", ")),
        CallState::Running(_) => "running".to_owned(),
        CallState::Done => "done".to_owned(),
        CallState::Stopped => "stopped".to_owned(),
    }
}

/// 子 agent 调用树的行：缩进 + 名字（`agent · tool`）+ 状态，失败红、结果一行。
///
/// 这是上游 rig-basics `SubagentLooks` 的语义（那张 look 不在本仓库的依赖里，
/// 数据来自 [`ChildCall`]：`ToolRender::children`）。
fn child_call_lines(children: &[ChildCall], width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    for child in children {
        let indent = "  ".repeat(child.depth + 1);
        let mark = if child.call.is_some() { "↳" } else { "…" };
        let agent = child
            .agent
            .as_deref()
            .map(|agent| format!("{agent} · "))
            .unwrap_or_default();
        let mut line = Line::from(vec![
            Span::styled(format!("{indent}{mark} "), Style::new().dim()),
            Span::styled(
                format!("{agent}{}", child.name),
                Style::new().fg(RColor::Cyan),
            ),
        ]);
        let state = state_words(&child.state);
        if !state.is_empty() {
            line.push_span(Span::styled(format!("  {state}"), Style::new().dim()));
        }
        if child.failed {
            line.push_span(Span::styled("  failed", Style::new().red()));
        }
        lines.push(line);
        if let Some(answer) = &child.answer {
            let first = answer.lines().next().unwrap_or("").trim_end();
            // TODO(width): 按字符数截断（没用 unicode 宽度；差几个全角字符，够用）。
            let room = width.saturating_sub(indent.len() + 2).max(8);
            let shown: String = first.chars().take(room).collect();
            if !shown.is_empty() {
                let style = if child.failed {
                    Style::new().red().dim()
                } else {
                    Style::new().dim()
                };
                lines.push(Line::from(Span::styled(format!("{indent}  {shown}"), style)));
            }
        }
    }
    lines
}

/// 给每个有子调用的工具卡片补上调用树（上游在 `RenderSystems::Decorate` 里
/// 由 `SubagentLooks` 做同样的事）。
fn child_calls_look(mut batch: ResMut<RenderBatch>) {
    let width = batch.width;
    for (call, out) in batch.calls_mut() {
        if call.children.is_empty() {
            continue;
        }
        out.decorate("children", child_call_lines(&call.children, width));
    }
}

/// 跑一遍 rig-tui 的 look 调度（独占：它要整个 world）。
pub fn run_looks(world: &mut World) {
    world.run_schedule(TranscriptRender);
}

// ---------------------------------------------------------------------------
// RenderBatch -> 行
// ---------------------------------------------------------------------------

/// 把 look 的产出交给对话引擎（排序、折行、流式增补、finalize），再拼上 `Ask`。
pub fn collect_rows(
    batch: Res<RenderBatch>,
    mut transcript: ResMut<TranscriptEngine>,
    dialogue: Res<Dialogue>,
    stream: Res<ReplyStream>,
    prompts: Res<SubmittedPrompts>,
    asks: Query<(Entity, &rig_harness::prelude::Ask), Without<Outcome>>,
    mut rows: ResMut<TranscriptRows>,
) {
    let width = batch.width;
    transcript.set_width(width);
    transcript.apply(&batch);

    // 没有 `RenderKey` 的文字块：有对话就用对话里的，没有就用本进程提交的提示词。
    //
    // TODO(notice): `Notice` 消息现在由装配层（`rig_harness_in_3d` 的 `NoticeLog`）
    // 在 `Fit` 里接到 `TranscriptRows` 末尾；要交给引擎（`TextId::Notice` +
    // `place`）得先把两处统一，否则会画两遍。
    if dialogue.from_prompts {
        for (index, prompt) in prompts.0.iter().enumerate() {
            transcript.set_text(TextId::Prompt(index), prompt_lines(prompt));
        }
        transcript.retain_text(|id| matches!(id, TextId::Prompt(_)));
    } else {
        for (id, lines) in &dialogue.text {
            transcript.set_text(id.clone(), lines.clone());
        }
        // 对话可读之后提示词兜底就撤掉（那份文字已经在对话里了）。
        let kept: HashSet<&TextId> = dialogue.text.iter().map(|(id, _)| id).collect();
        transcript.retain_text(|id| {
            matches!(id, TextId::StreamingReply | TextId::StreamingReasoning)
                || kept.contains(id)
        });
    }

    // 流式回复：真实 `Partial` 的正文/推理；流结束、没有流、或者回复已经落进
    // 对话，就收掉这一块。
    let theme = RTheme::default();
    if stream.finished || dialogue.has_reply || stream.reasoning.is_empty() {
        transcript.remove(&BlockId::Text(TextId::StreamingReasoning));
    } else {
        transcript.set_text(
            TextId::StreamingReasoning,
            reasoning_lines(&stream.reasoning, width, &theme),
        );
    }
    if stream.finished || dialogue.has_reply || stream.text.is_empty() {
        transcript.remove(&BlockId::Text(TextId::StreamingReply));
    } else {
        // 推理和正文之间空一行（上游 `lay_out_transcript` 的最后一段）。
        let mut lines = vec![Line::default()];
        lines.extend(reply_lines(&stream.text, width, &theme));
        transcript.set_text(TextId::StreamingReply, lines);
    }

    // aside 落在哪条消息之后（引擎排序时要）。
    for (id, slot) in &dialogue.slots {
        transcript.place(BlockId::Request(RenderKey::Aside(*id)), *slot);
    }

    let mut keyed: Vec<KeyedRow> = transcript.keyed_rows().to_vec();

    // 还没回答的 Ask：真实组件，追加在最后（正文与选项也按同一宽度折行）。
    for (ask_entity, ask) in asks.iter() {
        let title = Line::from(vec![
            Span::styled("? ", Style::new().fg(RColor::Magenta)),
            Span::styled(
                ask.title.clone(),
                Style::new()
                    .fg(RColor::LightMagenta)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        push_ask_rows(&mut keyed, ask_entity, "title", vec![title], width, None);
        for (index, body_line) in ask.body.lines().enumerate() {
            let line = Line::from(Span::styled(
                format!("    {body_line}"),
                Style::new().fg(RColor::Rgb(200, 206, 220)),
            ));
            push_ask_rows(
                &mut keyed,
                ask_entity,
                &format!("body{index}"),
                vec![line],
                width,
                None,
            );
        }
        for (index, choice) in ask.choices.iter().enumerate() {
            let line = Line::from(Span::styled(
                format!("    [{}] {}", index + 1, choice.label),
                Style::new()
                    .fg(RColor::Green)
                    .add_modifier(Modifier::BOLD),
            ));
            push_ask_rows(
                &mut keyed,
                ask_entity,
                &format!("choice{index}"),
                vec![line],
                width,
                Some(&choice.id),
            );
        }
    }

    rows.0 = keyed;
}

/// Ask 行的 key：高位段（引擎按位置发的 key 最高位是 0，不会撞）。
fn ask_key(ask: Entity, part: &str, index: usize) -> u64 {
    let hashed = key_of(&[&format!("{ask:?}"), part, &index.to_string()]);
    0x8000_0000_0000_0000 | (hashed >> 1)
}

/// 把一段 Ask 的文字按宽度折成行；`choice` 给了就每一行都可点。
fn push_ask_rows(
    rows: &mut Vec<KeyedRow>,
    ask: Entity,
    part: &str,
    lines: Vec<Line<'static>>,
    width: u16,
    choice: Option<&str>,
) {
    for (index, mut row) in wrap_lines(&lines, width).into_iter().enumerate() {
        if let Some(choice) = choice {
            row = row.ask_choice(ask, choice.to_owned());
        }
        rows.push(KeyedRow {
            key: ask_key(ask, part, index),
            hash: row_hash(&row),
            row,
        });
    }
}

// ---------------------------------------------------------------------------
// 业务 look（demo 的两个工具卡片）
// ---------------------------------------------------------------------------

/// The two looks this demo ships, registered on rig-tui's own render schedule.
fn search_product_look(mut batch: ResMut<RenderBatch>) {
    for (call, out) in batch.tools_mut("search_products") {
        out.lines = search_product_lines(call);
    }
}

fn search_product_lines(call: &ToolRender) -> Vec<LookLine> {
    let dim = Style::default().fg(RColor::Rgb(120, 128, 145));
    let secondary = Style::default().fg(RColor::Rgb(165, 175, 195));
    let price_style = Style::default().fg(RColor::Green);
    let rate = Style::default().fg(RColor::LightYellow);
    let state = match &call.state {
        CallState::Streaming => "(streaming)".to_string(),
        CallState::Running(_) => "(running)".to_string(),
        CallState::Held(reasons) => format!("(held: {})", reasons.join(", ")),
        CallState::Waiting => "(queued)".to_string(),
        CallState::Done => "(done)".to_string(),
        CallState::Stopped => "(stopped)".to_string(),
    };

    let mut lines: Vec<LookLine> = Vec::new();
    lines.push(
        Line::from(vec![
            Span::styled("* ", Style::default().fg(RColor::Green)),
            Span::styled(
                "search_products",
                Style::default()
                    .fg(RColor::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {state}"), dim),
            Span::styled(
                format!("  query={}", call.argument("query").unwrap_or("-")),
                secondary,
            ),
        ])
        .into(),
    );

    if let Some(text) = call.result_text()
        && let Ok(products) = serde_json::from_str::<serde_json::Value>(&text)
    {
        lines.push(
            Line::from(Span::styled(
                "  #  name                          price  shop             rate",
                dim,
            ))
            .into(),
        );
        for (index, product) in products.as_array().into_iter().flatten().enumerate() {
            let name = product["name"].as_str().unwrap_or("?");
            let shop = product["shop"].as_str().unwrap_or("?");
            let price = product["price"].as_i64().unwrap_or(0);
            let rating = product["rating"].as_f64().unwrap_or(0.0);
            lines.push(
                Line::from(vec![
                    Span::styled(format!("  {}  ", index + 1), dim),
                    Span::styled(format!("{name:<29}"), secondary),
                    Span::styled(format!("{price:<6}"), price_style),
                    Span::styled(format!("{shop:<17}"), secondary),
                    Span::styled(format!("{rating:.1}"), rate),
                ])
                .into(),
            );
        }
    }
    lines
}

/// `create_order` 的 look：等待确认 / 已下单。
fn order_look(mut batch: ResMut<RenderBatch>) {
    for (call, out) in batch.tools_mut("create_order") {
        let held = matches!(call.state, CallState::Held(_));
        let done = matches!(call.state, CallState::Done);
        let tail = if held {
            "  waiting for your confirmation (gate)"
        } else if done {
            "  order placed"
        } else {
            "  queued"
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled("* ", Style::default().fg(RColor::Magenta)),
                Span::styled("create_order", Style::default().fg(RColor::LightMagenta)),
                Span::styled(tail, Style::default().fg(RColor::Rgb(180, 186, 200))),
            ])
            .into(),
        ];
        if done && let Some(text) = call.result_text() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                lines.push(
                    Line::from(Span::styled(
                        format!(
                            "  order {} · total {} · status {}",
                            value["order_id"].as_str().unwrap_or("-"),
                            value["total"].as_i64().unwrap_or(0),
                            value["status"].as_str().unwrap_or("-")
                        ),
                        Style::default().fg(RColor::Green),
                    ))
                    .into(),
                );
            }
        }
        out.lines = lines;
    }
}

pub struct BatchPlugin;

impl Plugin for BatchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderBatch>()
            .init_resource::<ReplyStream>()
            .init_resource::<CompletedCalls>()
            .init_resource::<CallHistory>()
            .init_resource::<LiveOrdinals>()
            .init_resource::<Dialogue>()
            .init_resource::<TranscriptEngine>()
            .configure_sets(
                TranscriptRender,
                (
                    RenderSystems::Render,
                    RenderSystems::Decorate,
                    RenderSystems::Edit,
                )
                    .chain(),
            )
            .add_systems(
                TranscriptRender,
                (
                    search_product_look.in_set(RenderSystems::Render),
                    order_look.in_set(RenderSystems::Render),
                    child_calls_look.in_set(RenderSystems::Decorate),
                ),
            )
            .add_systems(AgentPass, record_calls.in_set(Stage::Review))
            .add_systems(
                Update,
                (track_reply_stream, rebuild_batch, run_looks, collect_rows)
                    .chain()
                    .in_set(crate::FeathersSystems::Draw),
            );
    }
}

#[cfg(test)]
mod tests {
    //! 建批的测试：不跑模型、不连网，用构造的 `Conversation` / `CallFact` 断言。

    use super::*;
    use rig_core::message::{AssistantContent, Image, ToolFunction, ToolName, ToolResultContent};

    /// 一个工具调用。
    fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
        let name = ToolName::new(name).expect("工具名非空");
        ToolCall::new(
            CallId::from_wire("call_test"),
            ToolFunction::new(name, arguments),
        )
    }

    /// 行里的纯文本。
    fn text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    /// 一段文字块的全部文本。
    fn lines_text(lines: &[Line<'static>]) -> String {
        lines.iter().map(text).collect::<Vec<_>>().join("\n")
    }

    /// 一条用户消息 + 一条 assistant 回复（正文 + 调用）+ 结果 + 一条 note。
    fn conversation() -> (talk::Conversation, ToolCall, AsideId) {
        let mut conversation = talk::Conversation::new();
        let user = conversation.push(Message::user("Why tabs?"));
        let tool = call("search", serde_json::json!({ "q": "tabs" }));
        conversation.push(Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::text("Tabs are the style."),
            AssistantContent::ToolCall(tool.clone()),
        ])));
        conversation.push(Message::User {
            content: vec![
                UserContent::ToolResult(ToolResult {
                call: tool.id.clone(),
                name: tool.function.name.clone(),
                    content: vec![ToolResultContent::Text(Text::new("3 products"))],
                    is_error: false,
                }),
                UserContent::Image(Image::default()),
            ],
        });
        let note = conversation.add_aside(
            Note::new("Why tabs?", "The project's style says so.")
                .aside(Author::plugin("btw"), Some(user)),
        );
        (conversation, tool, note)
    }

    #[test]
    fn a_conversation_becomes_real_requests_and_text_blocks() {
        let (conversation, tool, note) = conversation();
        let frame = conversation_frame(
            &conversation,
            &CallHistory::default(),
            &CallTree::default(),
            &RTheme::default(),
            80,
        );

        // 用户自己敲的那条：文字块，`› ` 前缀。
        let user = frame
            .text
            .iter()
            .find(|(id, _)| *id == TextId::Message { at: 0, block: 0 })
            .map(|(_, lines)| lines.clone())
            .expect("用户消息应进文字块");
        assert!(
            lines_text(&user).contains("› Why tabs?"),
            "用户文字应带 › 前缀：{}",
            lines_text(&user)
        );

        // assistant 正文：文字块，内容就是对话里的正文。
        let reply = frame
            .text
            .iter()
            .find(|(id, _)| *id == TextId::Message { at: 1, block: 0 })
            .map(|(_, lines)| lines.clone())
            .expect("assistant 正文应进文字块");
        assert!(
            lines_text(&reply).contains("Tabs are the style."),
            "assistant 正文来自对话：{}",
            lines_text(&reply)
        );

        // tool call：`Tool(消息序号, 块序号)`，结果挂在它下面。
        let (key, render) = frame
            .requests
            .iter()
            .find_map(|(key, input)| match (key, input) {
                (RenderKey::Tool(1, 1), RenderInput::Tool(render)) => Some((key, render)),
                _ => None,
            })
            .expect("tool call 应是 Tool(1, 1)");
        assert_eq!(*key, RenderKey::Tool(1, 1), "块序号来自 assistant 的 content");
        assert!(render.result.is_some(), "结果来自下一条消息");
        assert_eq!(render.call.id, tool.id);
        assert_eq!(
            render.item,
            Some(CallItem::Reply(CallRef {
                reply: conversation.id(1).unwrap(),
                index: 0,
            })),
            "调用位置是回复里的第 0 个调用"
        );

        // 图片内容块：`Content(消息序号, 块序号)`。
        let content = frame
            .requests
            .iter()
            .find_map(|(key, input)| match (key, input) {
                (RenderKey::Content(2, 1), RenderInput::Content(content)) => Some(content),
                _ => None,
            })
            .expect("图片应是 Content(2, 1)");
        assert_eq!(content.content.kind(), rig_tui::ContentKind::Image);
        assert!(!content.assistant);

        // note：`Aside(id)` 里装的是 `EntryRender`（上游同样这么画）。
        let (_, aside) = frame
            .requests
            .iter()
            .find(|(key, _)| *key == RenderKey::Aside(note))
            .expect("note 应有 Aside 请求");
        match aside {
            RenderInput::Entry(entry) => {
                assert_eq!(entry.title, "Why tabs?");
                assert_eq!(entry.plugin, "btw");
                assert!(entry.text.contains("style says so"));
            }
            _ => panic!("note 应画成 EntryRender"),
        }
        // note 跟着用户那条消息（第 0 条）。
        assert_eq!(
            frame.slots.iter().find(|(id, _)| *id == note).map(|(_, slot)| *slot),
            Some(Slot::After(0))
        );
        // 对话里已有的调用 id 记下来，结算历史里不再重复画。
        assert!(frame.calls.contains(&tool.id));
    }

    #[test]
    fn delivered_text_by_another_author_becomes_an_entry() {
        // 插件/agent 送达的文字（有作者）：`RenderKey::Entry`，不是用户自己敲的那段。
        let mut conversation = talk::Conversation::new();
        conversation.push(Message::user("typed"));
        conversation.push(
            talk::Entry::new(Message::user("summary of the last compaction"))
                .by(Author::plugin("compactor").titled("summary")),
        );
        let frame = conversation_frame(
            &conversation,
            &CallHistory::default(),
            &CallTree::default(),
            &RTheme::default(),
            80,
        );
        let entry = frame
            .requests
            .iter()
            .find_map(|(key, input)| match (key, input) {
                (RenderKey::Entry(1, 0), RenderInput::Entry(entry)) => Some(entry),
                _ => None,
            })
            .expect("送达的文字应是 Entry(1, 0)");
        assert_eq!(entry.plugin, "compactor");
        assert!(entry.title.contains("summary"), "标题来自作者：{}", entry.title);
        assert!(entry.text.contains("summary of the last compaction"));
        // 它不该同时出现成用户文字块。
        assert!(frame
            .text
            .iter()
            .all(|(_, lines)| !lines_text(lines).contains("summary of the last compaction")));
    }

    #[test]
    fn a_tool_result_is_drawn_under_its_call_and_not_again_as_text() {
        let (conversation, _, _) = conversation();
        let frame = conversation_frame(
            &conversation,
            &CallHistory::default(),
            &CallTree::default(),
            &RTheme::default(),
            80,
        );
        let all_text = frame
            .text
            .iter()
            .map(|(_, lines)| lines_text(lines))
            .collect::<String>();
        assert!(
            !all_text.contains("3 products"),
            "工具结果应画在调用下面，不该再单独成一段：{all_text}"
        );
        let rendered = frame.requests.iter().any(|(key, input)| {
            matches!(key, RenderKey::Tool(1, 1))
                && matches!(input, RenderInput::Tool(render)
                    if render.result.as_ref().is_some_and(|result| {
                        tool_result_text(result).contains("3 products")
                    }))
        });
        assert!(rendered, "结果应挂在那次调用上");
    }

    #[test]
    fn a_note_is_an_entry_and_other_asides_are_asides() {
        let mut conversation = talk::Conversation::new();
        conversation.push(Message::user("hello"));
        let note = conversation.add_aside(
            Note::new("t", "n").aside(Author::plugin("btw"), conversation.id(0)),
        );
        let plain = conversation.add_aside(talk::Aside::new(
            "rig.call",
            Author::plugin("host"),
            serde_json::json!({ "call": "c1" }),
        ));
        let frame = conversation_frame(
            &conversation,
            &CallHistory::default(),
            &CallTree::default(),
            &RTheme::default(),
            80,
        );
        assert!(frame.requests.iter().any(|(key, input)| {
            *key == RenderKey::Aside(note) && matches!(input, RenderInput::Entry(_))
        }));
        assert!(frame.requests.iter().any(|(key, input)| {
            *key == RenderKey::Aside(plain) && matches!(input, RenderInput::Aside(_))
        }));

        // 没有 look 时，非 note 的 aside 也有兜底正文（不是悄悄丢掉）。
        let lines = default_lines(
            &RenderInput::Aside(AsideRender {
                id: plain,
                aside: conversation.aside(plain).unwrap().clone(),
                theme: RTheme::default(),
            }),
            &RTheme::default(),
        );
        let joined: String = lines
            .iter()
            .map(|line| line.line().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("⤶ aside"), "{joined}");
        assert!(joined.contains("rig.call"), "要写种类：{joined}");
        assert!(joined.contains("plugin host"), "要写来源：{joined}");
        assert!(joined.contains("c1"), "要写出值：{joined}");
    }

    #[test]
    fn the_call_tree_hangs_children_under_their_parent_by_effect_id() {
        let parent = EffectId::from_raw(10);
        let first = EffectId::from_raw(12);
        let second = EffectId::from_raw(11);
        let grandchild = EffectId::from_raw(13);
        let mut tree = CallTree::default();
        let node = |name: &str, state: CallState| ChildCall {
            depth: 0,
            call: Some(ToolFunction::new(
                ToolName::new(name).unwrap(),
                serde_json::json!({}),
            )),
            name: name.to_owned(),
            agent: Some("helper".to_owned()),
            state,
            failed: false,
            answer: None,
        };
        tree.add(parent, None, node("spawn", CallState::Done));
        tree.add(first, Some(parent), node("first", CallState::Done));
        tree.add(second, Some(parent), node("second", CallState::Running(None)));
        tree.add(grandchild, Some(first), node("deep", CallState::Waiting));

        let children = tree.children_of(parent);
        let names: Vec<&str> = children.iter().map(|child| child.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["second", "first", "deep"],
            "按效应 id 排，且孙调用挂在发起它的调用下面"
        );
        assert_eq!(children[0].depth, 0);
        assert_eq!(children[2].depth, 1);
        assert_eq!(tree.children_of(EffectId::from_raw(99)), Vec::new());
    }

    #[test]
    fn child_calls_draw_as_an_indented_tree_with_state_and_answer() {
        let children = vec![
            ChildCall {
                depth: 0,
                call: Some(ToolFunction::new(
                    ToolName::new("search").unwrap(),
                    serde_json::json!({}),
                )),
                name: "search".to_owned(),
                agent: Some("helper".to_owned()),
                state: CallState::Running(None),
                failed: false,
                answer: None,
            },
            ChildCall {
                depth: 1,
                call: None,
                name: "model".to_owned(),
                agent: Some("helper".to_owned()),
                state: CallState::Done,
                failed: true,
                answer: Some("boom\nsecond line".to_owned()),
            },
        ];
        let lines = child_call_lines(&children, 40);
        let drawn: Vec<String> = lines.iter().map(text).collect();
        assert!(
            drawn[0].starts_with("  ↳ helper · search"),
            "缩进 + agent · 工具：{:?}",
            drawn[0]
        );
        assert!(drawn[0].contains("running"), "{:?}", drawn[0]);
        assert!(
            drawn[1].starts_with("    … helper · model"),
            "深度 1 缩进更深：{:?}",
            drawn[1]
        );
        assert!(drawn[1].contains("failed"), "{:?}", drawn[1]);
        assert!(drawn[2].contains("boom"), "结果只取第一行：{:?}", drawn[2]);
        assert!(!drawn[2].contains("second line"));
        // 失败的结果是红的。
        assert_eq!(
            lines[2].spans.last().map(|span| span.style.fg),
            Some(Some(RColor::Red))
        );
    }

    #[test]
    fn lines_carry_their_role_and_a_reply_is_markdown() {
        // 用户：`› ` 前缀 + 粗体。
        let user = user_lines("hello");
        assert_eq!(user[0].spans[0].content.as_ref(), "› ");
        assert!(user[0].spans[1].style.add_modifier.contains(Modifier::BOLD));
        // assistant：markdown 排版（`*x*` 的星号不出现），推理 dim + italic。
        let theme = RTheme::default();
        let reply = reply_lines("*short* answer", 80, &theme);
        let reply_text: String = reply.iter().map(text).collect::<Vec<_>>().join("\n");
        assert!(reply_text.contains("short answer"), "{reply_text}");
        assert!(!reply_text.contains('*'), "markdown 的强调符号不该画出来");
        let reasoning = reasoning_lines("thinking", 80, &theme);
        assert!(
            reasoning
                .iter()
                .all(|line| line.style.add_modifier.contains(Modifier::ITALIC)),
            "推理是斜体"
        );
    }

    #[test]
    fn the_prompt_fallback_is_a_user_line() {
        let lines = prompt_lines("typed by me");
        assert!(lines_text(&lines).contains("› typed by me"));
        assert_eq!(lines[0].spans.len(), 0, "前面空一行");
    }

    /// 一个没有窗口的 App：真内核 + 建批 + 对话引擎（不跑模型、不连网）。
    ///
    /// 资源补的是视图层其它插件平时会 init 的那几个；帧布局不跑，所以直接把
    /// `RenderBatch::width` 设成 60 列（等于上一次 Layout 给的值）。
    fn headless_app() -> App {
        use bevy::app::TaskPoolPlugin;

        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.add_plugins(rig_ecs::AgentPlugin);
        app.init_resource::<PanelAreas>()
            .init_resource::<PanelRoots>()
            .init_resource::<TranscriptRows>()
            .init_resource::<SubmittedPrompts>();
        app.add_plugins(BatchPlugin);
        app.world_mut().resource_mut::<RenderBatch>().width = 60;
        app
    }

    /// 转写面板上的全部文本。
    fn transcript_text(app: &App) -> String {
        let rows = app.world().resource::<TranscriptRows>();
        rows.0
            .iter()
            .map(|keyed| {
                keyed
                    .row
                    .line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_headless_app_draws_a_live_tool_call() {
        use rig_ecs::spawn::spawn_root;

        let mut app = headless_app();
        let agent = spawn_root(app.world_mut(), AgentId("test".into())).id();
        app.world_mut().spawn((
            ToolRequest {
                call: call("search", serde_json::json!({ "q": "tabs" })),
            },
            ForAgent(agent),
        ));
        app.update();
        app.update();
        let text = transcript_text(&app);
        assert!(text.contains("search"), "活调用应成行，实际：{text:?}");
        assert!(
            !text.contains("推荐第"),
            "写死的假回复不能再出现：{text:?}"
        );
    }

    #[test]
    fn a_headless_app_draws_the_conversations_reply_and_its_note() {
        use rig_ecs::spawn::spawn_root;

        let mut app = headless_app();
        let agent = spawn_root(app.world_mut(), AgentId("test".into())).id();
        let mut conversation = talk::Conversation::new();
        let asked = conversation.push(Message::user("Why tabs?"));
        conversation.push(Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::text("Tabs are the style."),
        ])));
        conversation.add_aside(
            Note::new("Why tabs?", "The project's style says so.")
                .aside(Author::plugin("btw"), Some(asked)),
        );
        app.world_mut()
            .entity_mut(agent)
            .insert(Conversation(conversation));
        app.insert_resource(ShownAgent(Some(agent)));
        app.update();
        app.update();
        let text = transcript_text(&app);
        assert!(text.contains("› Why tabs?"), "用户请求应成行：{text:?}");
        assert!(
            text.contains("Tabs are the style."),
            "assistant 正文应成行（以前根本没有）：{text:?}"
        );
        assert!(text.contains("Why tabs?"), "note 的标题应成行：{text:?}");
        assert!(
            text.contains("The project's style says so."),
            "note 的正文应成行：{text:?}"
        );
        // note 在它跟着的那条消息之后、assistant 回复之前。
        let note_at = text.find("The project's style").unwrap();
        let reply_at = text.find("Tabs are the style.").unwrap();
        assert!(note_at < reply_at, "note 应排在它跟的那条消息之后：{text:?}");
    }

    #[test]
    fn a_live_call_gets_a_stable_streaming_ordinal() {
        let mut ordinals = LiveOrdinals::default();
        let first = EffectId::from_raw(7);
        let second = EffectId::from_raw(9);
        assert_eq!(ordinals.of(first), 0);
        assert_eq!(ordinals.of(second), 1);
        assert_eq!(ordinals.of(first), 0, "同一个效应每次都要同一个位置");
        assert_eq!(ordinals.of(EffectId::from_raw(8)), 2);
    }
}
