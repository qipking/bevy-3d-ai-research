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
//!    对话引擎（排序、折行、流式增补、finalize）。
//!
//! 还没回答的 `Ask` **不在转写里**：它们由 [`confirm_rows`] 产成确认行，写进
//! 独立的 [`PanelId::Confirm`] tile（见 [`confirm_panel_rows`]），和转写互不
//! 挤占。这样待确认的操作既不会跟着转写滚走，也不会因为转写滚动被裁掉。
//!
//! 没有自己管线的工作区（多工作区前端的非聚焦条带）用两个公开函数拿同样的内容：
//! [`dialogue_batch`] 把对话历史拼成批，[`live_additions`] 补上在飞调用、流式
//! 正文/推理和待回答的 Ask —— 后者把 Ask 当普通行画在同一块面板里（那些条带
//! 没有自己的确认 tile 在跑），并按 agent 过滤，两个条带的实时内容不会串台。
//!
//! 以前这里写死过一句中文"回复"，由每两帧吐一个字的假流式驱动 —— 已经删掉：
//! 正文现在只来自真实的 `Partial`（运行中的流）或对话里的 assistant 消息。

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::Duration;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use rig_core::conversation::{self as talk, CallRef, Note, TurnEnd, pairing::answered_by};
use rig_core::effect::EffectId;
use rig_core::message::{
    AssistantMessage, CallId, Message, ToolCall, ToolResult, ToolResultContent, UserContent,
};
use rig_core::tool::ToolResultContext;
use rig_ecs::prelude::*;
use rig_harness::prelude::{Ask, CallOutput, ShownAgent, ToolProgress};
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
use crate::panels::{KeyedRow, PanelId, PanelRows};
use crate::rich::upstream_theme;
use crate::theme::Theme;
use crate::transcript::{
    BlockId, Row, Slot, TextId, Transcript as TranscriptEngine, row_hash, wrap_lines,
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
    /// 它的所有者 agent **实体**（`ForAgent` 指的那个），聚焦条带按它过滤。
    ///
    /// `agent` 只是显示名；`owner` 让建批能判断"这条是不是 shown agent 的
    /// 根"，以及子 agent 的调用该挂在谁下面。`None` = 效果本来就没有
    /// `ForAgent`（app 自己的效应），或者更早的历史里没记 owner —— 建批时
    /// 按"没主的根"处理，任何条带都画（见 [`CallTree::scope`]）。
    pub owner: Option<Entity>,
    /// 它在"对话之外的调用"里的固定序号（`RenderKey::Streaming` 的位置）。
    pub ordinal: usize,
    pub call: ToolCall,
    pub result: ToolResult,
    /// 它发布过的事实（内核 `Published` 组件里的 `ToolResultContext`：
    /// `ExitStatus`、`OutputCut`、`FilesChanged` 之类），按会话保存时的
    /// 上限裁剪过（[`published_facts`]）。
    ///
    /// **来源**：效应答复时 `Published` 和 `Outcome` 一起插在效应实体上
    /// （rig-ecs `stage.rs:458`、`answer()`），而效应在 Commit 后会被回收，
    /// 所以必须在 [`record_calls`]（Review）抄下来。运行中的调用没有它
    /// —— 没有发布就没有退出码/截断标记，这是上游语义，不是缺口。
    pub facts: Option<ToolResultContext>,
    /// 从开始到结算的时长（真实时钟）：结算那一刻用 `Started.at.elapsed()`
    /// 算的，比真正完成晚一点点。上游用的是效应记录里的
    /// `EffectRecord::started_ms`/`finished_ms`（精确）；要那两列得消费
    /// `EffectRecorded`（TODO(past)，重启后的旧调用同理）。
    pub took: Option<Duration>,
}

/// 已结算调用的历史，按结算顺序。
///
/// 转写用它还原子 agent 的调用树、`took` 和"已经结算但还没落进对话"的卡片。
///
/// TODO(past): 恢复会话时更早的效应记录（上游 `PastEffects` / `EffectRecorded`）
/// 还没接：重启后旧的 `took` 与调用树是空的，`facts` 也只有已经落进对话的
/// 调用才拿得回来（`Entry::facts`），没提交的结算调用那份在 `KeptFacts`
/// 里 —— 那是 rig-ecs 的 `pub(crate)` 类型，本 crate 读不到（见
/// [`CallFact::facts`]）。
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

/// `[stopped]` 在 `TextId::Message` 里的块序号：`usize::MAX` 让它在排序上排到
/// 该消息所有真实内容块之后（`transcript::rank` 按块序号升序）。
///
/// 为什么复用 `TextId::Message` 而不加一个变体：应用层的非聚焦条带
/// （`rig_harness_in_3d::workspaces_demo`）清理旧文字块时只保留
/// `TextId::Message { .. }`；它不在本 crate 的改动范围里，新变体会在那条路径上
/// 被整块丢掉。block 取最大不会和真实内容块（0、1、……）撞车。
const STOPPED_BLOCK: usize = usize::MAX;

/// 调用之间的关系：谁启动了谁（子 agent 的调用树）。
///
/// 上游 `transcript::CallTree` 的对位，数据来自 `Effect.parent`（还没结算的）、
/// [`CallFact::parent`]（结算过的）和 [`EffectParents`]（已经结算、实体没了的
/// 非调用效应）。父子关系收**所有**效应，节点仍然只给工具调用：非调用的效应
/// （模型调用、turn、消息……）在 [`CallTree::below`] 里被穿过去，depth 不变，
/// 子 agent 的调用因此能接到发起它的调用下面（上游 `CallFacts::add_below`
/// 同样让没有节点的那层穿过）。
///
/// TODO(model-calls): 上游还会画模型调用节点
/// （`ChildCall { call: None, name: "vendor/model" }`，来自 `ModelRequest`/`SentTo`）。
/// 本前端没有这一层节点，所以子 agent 的调用比上游少缩进一层（upstream 的
/// depth 多 1），但相对层次和顺序一致；要补得先有模型调用的名字/状态来源。
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
        self.link(effect, parent);
        self.nodes.insert(effect, call);
    }

    /// 只记父子关系（[`live_additions`] 不画调用树，但要用同一套"根 + 后代"
    /// 过滤，见 [`CallTree::scope`]）。
    ///
    /// 幂等：同一效应先由 [`EffectParents`] 预链、再由 [`CallTree::add`] 链
    /// 一次，子列表里不会出现两份。
    fn link(&mut self, effect: EffectId, parent: Option<EffectId>) {
        let Some(parent) = parent.filter(|parent| *parent != effect) else {
            return;
        };
        if self.parents.contains_key(&effect) {
            return;
        }
        self.parents.insert(effect, parent);
        self.children.entry(parent).or_default().push(effect);
    }

    /// `roots` 连同它们在树里的全部后代（含根）。
    ///
    /// 聚焦条带的根 = owner 是 shown agent 的效应（或 owner 缺失的老记录）；
    /// 子 agent 的调用不在根里，但会作为发起它的调用的后代被收进来。
    fn scope(&self, roots: impl IntoIterator<Item = EffectId>) -> HashSet<EffectId> {
        let mut visible = HashSet::new();
        let mut pending: Vec<EffectId> = roots.into_iter().collect();
        while let Some(effect) = pending.pop() {
            if !visible.insert(effect) {
                continue;
            }
            if let Some(children) = self.children.get(&effect) {
                pending.extend(children.iter().copied());
            }
        }
        visible
    }

    /// `effect` 要不要在本条带单独占一张卡。
    ///
    /// 范围里的后代不占：它们的行已经由父卡片的 `children` 画了
    /// （[`child_calls_look`]），再插一张就是画两遍。
    /// `scope` 为 `None` = 没有聚焦视图（没有 `ShownAgent`），退回旧行为：
    /// 每个调用一张卡，不做树形去重。
    fn is_card(&self, scope: Option<&HashSet<EffectId>>, effect: EffectId) -> bool {
        let Some(scope) = scope else { return true };
        scope.contains(&effect)
            && self
                .parents
                .get(&effect)
                .is_none_or(|parent| !scope.contains(parent))
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

/// 每个效应挂在哪（`Effect::parent`），按效应 id 记 —— 结算后实体没了，
/// 关系还得留着，调用树才能跨过非调用的效应找到最近的调用祖先。
///
/// **来源**：`Effect` 组件（rig-ecs `stage.rs:251`，`parent` 是公开字段），
/// 在 Review 抄（[`record_effect_parents`]）：Commit 才 settle，所以这一
/// pass 里要结算的效应在 Review 还看得见。上游读的是每条
/// `EffectRecorded`（`Settled::note` 都记进 `CallTree`）；本前端没有消费
/// 那个消息，理由和 `Published` 一样 —— 组件比消息更直接。
///
/// 只增不删：一个效应 id 的父关系不会变（重试是新的效应 id），长会话里
/// 每个效应一条，和上游保留整份记录同一个量级。
#[derive(Resource, Default)]
pub struct EffectParents(HashMap<EffectId, Option<EffectId>>);

impl EffectParents {
    /// 已知的每条（效应, 父效应），给建树用。
    fn iter(&self) -> impl Iterator<Item = (EffectId, Option<EffectId>)> + '_ {
        self.0.iter().map(|(effect, parent)| (*effect, *parent))
    }
}

/// 把每个活效应的父关系抄进 [`EffectParents`]（Review，Commit 之前）。
///
/// 不跳过已抄过的：一个效应只有一条父关系，重复写是同值；这样不用给每个
/// 效应再挂一个记账组件，每 pass 的开销就是 effect 数（几十个）次哈希插入。
fn record_effect_parents(effects: Query<&Effect>, mut parents: ResMut<EffectParents>) {
    for effect in &effects {
        parents.0.insert(effect.id(), effect.parent);
    }
}

/// `owner`（`None` = 没有 owner）在 `shown` 的条带里可不可见。
///
/// 没有 `ShownAgent`（`shown == None`）= 不过滤：无视图的测试/导出路径保留
/// 全量。有视图时，shown 自己的和没有 owner 的（app 级效应 / 更早的历史）
/// 可见，别的 agent 的不可见 —— 聚焦建批、非聚焦条带、确认面板共用这一条口径。
fn focused_visible(shown: Option<Entity>, owner: Option<Entity>) -> bool {
    shown.is_none_or(|shown| owner.is_none_or(|owner| owner == shown))
}

/// 这条调用是不是 `shown` 的根：owner 就是它，或者更早的历史里没有 owner。
///
/// 没有 shown agent（`None`）时不做过滤，这个判断不会被用到。
fn is_root_of(shown: Option<Entity>, owner: Option<Entity>) -> bool {
    shown.is_some_and(|shown| focused_visible(Some(shown), owner))
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
            Has<Started>,
            Option<&'static Running>,
            Option<&'static Holds>,
            Option<&'static ForAgent>,
            Option<&'static ToolProgress>,
            Option<&'static CallOutput>,
            Option<&'static Published>,
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
/// 抄的东西：调用的结果（[`CompletedCalls`]）、效应 id/父效应/agent/时长/
/// 发布的事实（[`CallHistory`]）—— 效应实体结算后会被回收（`Published` 和
/// `Outcome` 都不在了），这些是转写画调用树、退出码/截断标记和"结算了但
/// 还没落进对话"卡片的唯一来源。
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
            Option<&Published>,
        ),
        Without<Recorded>,
    >,
    names: Query<&Name>,
    mut completed: ResMut<CompletedCalls>,
    mut history: ResMut<CallHistory>,
    mut ordinals: ResMut<LiveOrdinals>,
) {
    for (entity, request, outcome, effect, agent, started, published) in calls.iter() {
        let result = tool_result_of(outcome, &request.call);
        completed.0.push((request.call.clone(), result.clone()));
        let took = started
            .and_then(|started| started.at.elapsed().ok())
            .filter(|took| !took.is_zero());
        let owner = agent.map(|ForAgent(agent)| *agent);
        let agent = agent
            .and_then(|ForAgent(agent)| names.get(*agent).ok())
            .map(|name| name.as_str().to_owned());
        history.0.push(CallFact {
            effect: effect.id(),
            parent: effect.parent,
            agent,
            owner,
            ordinal: ordinals.of(effect.id()),
            call: request.call.clone(),
            result,
            facts: published_facts(published),
            took,
        });
        commands.entity(entity).insert(Recorded);
    }
}

/// 内核的 outcome -> look 读的 `ToolResult`。
///
/// 和内核把结果交给模型时同一套构造（`rig-core` 的 `ToolCall::answer`，
/// `completion/message.rs:601`）：保留输出里的 JSON/图片块，并且
/// 拒绝/跳过的结果也算错误（`!is_success()`，不只是 `is_error()`）。
fn tool_result_of(outcome: &Outcome, call: &ToolCall) -> ToolResult {
    let tool = outcome.tool_result();
    ToolResult {
        call: call.id.clone(),
        name: call.function.name.clone(),
        content: tool.output().clone().into_content(),
        is_error: !tool.is_success(),
    }
}

/// rig-ecs 保存在会话里的 facts 上限（`stage.rs:463` 的
/// `MAX_KEPT_FACTS_BYTES`，那边是私有的）：最多 256 KiB JSON，超出的大值
/// （比如一次重写整个文件的 diff）被丢掉，小的 `ExitStatus` 留下。
const KEPT_FACTS_BYTES: usize = 256 * 1024;

/// 内核 `Published` 组件 -> `ToolRender::facts` 读的 `ToolResultContext`。
///
/// **实时来源**：`Published` 是效应实体上的组件（rig-ecs `stage.rs:458`），
/// 只在 handler 答复时和 `Outcome` 一起插入（`stage.rs:answer()`），所以
/// 还在跑的调用没有 facts（退出码/截断标记/`FilesChanged` 都要等结果）。
/// 裁成会话会保存的形状（`ToolResultContext::kept`），这样结算到提交之间
/// 卡片上看到的事实，和之后从对话 entry 里读到的完全一致。
fn published_facts(published: Option<&Published>) -> Option<ToolResultContext> {
    published
        .map(|Published(facts)| facts.kept(KEPT_FACTS_BYTES))
        .filter(|facts| !facts.is_empty())
}

/// 一个还没结算的调用（这是建批时 `Streaming` 位置的真实来源）。
struct LiveCall {
    effect: EffectId,
    parent: Option<EffectId>,
    call: ToolCall,
    /// `ForAgent` 的显示名（子调用的 `agent · tool` 要用）。
    agent: Option<String>,
    /// `ForAgent` 的实体（聚焦条带按它判断谁是谁的根）。
    owner: Option<Entity>,
    state: CallState,
    result: Option<ToolResult>,
    /// 工具自己发布的事实，答复之前没有（见 [`published_facts`]）。
    facts: Option<ToolResultContext>,
    output: Option<String>,
    ordinal: usize,
}

/// 还没结算的调用的状态（和上游 `render::CallStates` 同一套判断顺序）。
///
/// `started` 是 `Has<Started>`（上游 `CallStates::state` 的判据）：`Running`
/// 是任务句柄，`Started` 是"内核已经启动它"，两者任一都算 started
/// —— 比如 `ToolStart::Open` 的调用有 `Started` 而没有 `Running`。
/// `hold_name` 给"在等谁"的那个名字：真管线从 `Name` 查询里取，
/// [`live_additions`] 直接读同一个 `Name` 组件 —— 两条路必须说同一句话。
fn live_state(
    outcome: Option<&Outcome>,
    started: bool,
    running: Option<&Running>,
    holds: Option<&Holds>,
    progress: Option<&ToolProgress>,
    hold_name: impl Fn(Entity) -> Option<String>,
) -> CallState {
    if let Some(holds) = holds.filter(|holds| holds.len() > 0) {
        let reasons = holds
            .iter()
            .map(|hold| hold_name(hold).unwrap_or_else(|| "unnamed hold".to_owned()))
            .collect();
        return CallState::Held(reasons);
    }
    if outcome.is_some() {
        return CallState::Done;
    }
    if started || running.is_some() {
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
    parents: Res<EffectParents>,
    stream: Res<ReplyStream>,
    mut dialogue: ResMut<Dialogue>,
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
    theme: Res<Theme>,
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
    // 聚焦条带只画 shown agent 自己那棵树；没有 shown agent（无视图的
    // 测试/导出路径）就退回全量，见下面的 `scope`。
    let shown = turn.agent();
    batch.agent = shown;
    // 生产路径用**我们的**语义主题（17 个 band 逐字段映射），不是上游
    // `RTheme::default()` 的终端默认色；`Theme::from_env` 的 `NO_COLOR`
    // 无色版也经这里下去。
    let rtheme = upstream_theme(&theme);
    dialogue.clear();

    // 1) 还没结算的调用：先收成自己的数据，后面既进 batch 也进调用树。
    let mut live: Vec<LiveCall> = Vec::new();
    for (
        request,
        effect,
        outcome,
        started,
        running,
        holds,
        agent,
        progress,
        output,
        published,
    ) in kernel.requests.iter()
    {
        let owner = agent.map(|ForAgent(agent)| *agent);
        let agent = agent
            .and_then(|ForAgent(agent)| kernel.names.get(*agent).ok())
            .map(|name| name.as_str().to_owned());
        live.push(LiveCall {
            effect: effect.id(),
            parent: effect.parent,
            call: request.call.clone(),
            agent,
            owner,
            state: live_state(outcome, started, running, holds, progress, |hold| {
                kernel
                    .names
                    .get(hold)
                    .ok()
                    .map(|name| name.as_str().to_owned())
            }),
            result: outcome.map(|outcome| tool_result_of(outcome, &request.call)),
            facts: published_facts(published),
            output: output.map(CallOutput::text).filter(|text| !text.is_empty()),
            ordinal: ordinals.of(effect.id()),
        });
    }
    live.sort_by_key(|call| call.effect);

    // 对话里已经出现、结果还没提交的调用（assistant 消息先提交、结果后提交），
    // 在飞时的状态/输出/子树仍然来自活效应 —— 上游 `ShownConversation` 的
    // `calls`/`outputs` 和 `ChildCalls` 的对位。`conversation_frame` 是纯函数，
    // 读不到内核，所以在这里收好传进去。
    let running_calls: HashMap<CallId, RunningCall> = live
        .iter()
        .map(|call| {
            (
                call.call.id.clone(),
                RunningCall {
                    effect: call.effect,
                    state: call.state.clone(),
                    output: call.output.clone(),
                },
            )
        })
        .collect();

    // 2) 调用树：结算过的 + 还在跑的（子 agent 的调用由此挂到发起它的调用下）。
    //    仍然收全量：父卡片要拿到自己的孩子；过滤留到 `scope`/`is_card`。
    let mut tree = CallTree::default();
    // 先接上所有已知的父子关系（含模型调用这类非调用效应）：子 agent 的
    // 调用挂在模型调用下，模型调用又挂在发起它的工具调用下（内核
    // `EffectParent`），中间那层没有卡片，但关系不能断。
    for (effect, parent) in parents.iter() {
        tree.link(effect, parent);
    }
    let mut roots: Vec<EffectId> = Vec::new();
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
        if is_root_of(shown, fact.owner) {
            roots.push(fact.effect);
        }
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
        if is_root_of(shown, call.owner) {
            roots.push(call.effect);
        }
    }
    // 可见范围：shown agent 的根 + 全部后代（含子 agent 的调用）。
    // `None` = 没有 shown agent：不过滤，退回旧行为。
    let scope = shown.map(|_| tree.scope(roots));

    // 3) 对话：assistant 正文/推理进文字块，调用/entry/内容块/aside 进 batch。
    if let Some(conversation) = turn.conversation() {
        let frame = conversation_frame(
            conversation,
            &history,
            &tree,
            &running_calls,
            &rtheme,
            batch.width,
        );
        for (key, input) in frame.requests {
            let output = RenderOutput {
                lines: default_lines(&input, &rtheme),
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
    //    聚焦条带里只画 shown agent 那棵树的根：后代的行由父卡片的
    //    `children` 画（`child_calls_look`），这里不再单独插一张卡。
    let mut rendered: HashSet<CallId> = dialogue.calls.clone();
    for fact in &history.0 {
        if !tree.is_card(scope.as_ref(), fact.effect) || !rendered.insert(fact.call.id.clone()) {
            continue;
        }
        let call = ToolRender {
            call: fact.call.clone(),
            result: Some(fact.result.clone()),
            state: CallState::Done,
            theme: rtheme,
            // 结算时从效应上的 `Published` 抄下来的事实：退出码、输出截断
            // 标记、`FilesChanged` 都从这里进 header/结果行（见
            // [`published_facts`] 与 [`record_calls`]）。
            facts: fact.facts.clone(),
            took: fact.took,
            // 答复后 `CallOutput` 不再是"运行中的输出"（上游同样只给
            // 在飞调用画），结果本身已经在上面的 `result` 里。
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
        if !tree.is_card(scope.as_ref(), call.effect) || !rendered.insert(call.call.id.clone()) {
            continue;
        }
        // TODO(streaming): 模型还在流参数、调用效应还没出现时，上游会先画一个
        // `CallState::Streaming` 的 `ToolRender`（位置取 `Partial::calls` 的下标）。
        // 这里等效应出现才画，免得用一个会变的合成位置。
        let render = ToolRender {
            call: call.call.clone(),
            result: call.result.clone(),
            state: call.state.clone(),
            theme: rtheme,
            // 已经答复但还没提交进对话的调用有 `Published`（和 `Outcome` 一起
            // 插上的）；还在跑的没有 —— 那时确实没有任何退出码/截断事实。
            facts: call.facts.clone(),
            // 上游 `ToolRender::took` 只在结算后才有（EffectRecord 的
            // started/finished）；这里等 [`record_calls`] 记进历史再填。
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
            request.output.lines = default_lines(&request.input, &rtheme);
        }
    }
}

/// 一个请求在没有 look 时画什么（上游 `RenderInput::default_lines` 的公开部分）。
fn default_lines(input: &RenderInput, theme: &RTheme) -> Vec<LookLine> {
    let lines: Vec<LookLine> = match input {
        // 默认 look 已经带了调用状态色带（来自 `ToolRender::theme`）。
        RenderInput::Tool(call) => call.default_lines(),
        RenderInput::Entry(entry) => on_band(
            entry.default_lines(),
            theme_band(theme, theme.tool),
        )
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
        RenderInput::Aside(aside) => on_band(
            aside_fallback_lines(aside),
            theme_band(theme, theme.tool),
        )
        .into_iter()
        .map(LookLine::from)
        .collect(),
    };
    // `NO_COLOR`：上游终端会在 buffer 上统一清色，我们的渲染层没有那一步，
    // 默认 look 自带的 band/代码色也要在落行前清掉（rich 的 plain 路径同理）。
    match theme.plain {
        true => lines.into_iter().map(plain_look).collect(),
        false => lines,
    }
}

// ---------------------------------------------------------------------------
// 对话 -> 请求 / 文字块（纯函数，测试直接喂构造的 `Conversation`）
// ---------------------------------------------------------------------------

/// 对话里还没结果的调用在飞时的样子（按调用 id）。
///
/// 上游 `transcript::Look` 从 `ShownConversation` 的 `calls`/`outputs` 和
/// `ChildCalls` 拿同一批数据：assistant 消息先提交、工具结果后提交，所以
/// 已经出现在对话里的调用仍然可能在跑，它的状态（含 [`ToolProgress`] 的
/// 进度行）、输出和子树都要从活效应来。
struct RunningCall {
    /// 它的效应 id：`CallTree::children_of` 用（还在跑的调用也能有子调用）。
    effect: EffectId,
    /// `render::CallStates::state`（含进度行）。
    state: CallState,
    /// 运行中工具报告的输出（`CallOutput::text`）；没有报告就是 `None`。
    output: Option<String>,
}

/// 一个 agent 的**对话历史** -> 这一帧的 `RenderBatch` + 没有 `RenderKey` 的文字块。
///
/// 这是 [`conversation_frame`] 的公开出口，给**没有自己管线**的工作区用（见
/// `rig_harness_in_3d::workspaces_demo` 的非聚焦条带）：对话里已经落地的部分
/// （用户请求、assistant 正文/推理、tool call、送达的 entry、内容块、aside）
/// 和真管线画得一模一样 —— 请求先填上没 look 时的默认行（`default_lines`），
/// look 调度不跑（非聚焦条带不该有两套 look 状态），调用树也没有。
///
/// 还没落进对话的东西（在飞的调用、流式正文/推理、待回答的 Ask）不在这里，
/// 由 [`live_additions`] 补在行流后面。`width` 是文字列数；`theme` 是当前
/// 应用主题（和聚焦管线同一个 `upstream_theme` 映射，`NO_COLOR` 也走它）。
pub fn dialogue_batch(
    conversation: &Conversation,
    agent: Entity,
    width: u16,
    theme: &Theme,
) -> (RenderBatch, Vec<(TextId, Vec<Line<'static>>)>) {
    let rtheme = upstream_theme(theme);
    let frame = conversation_frame(
        conversation,
        &CallHistory::default(),
        &CallTree::default(),
        // 没有自己管线的工作区拿不到内核的活效应（正是"没有管线"的定义），
        // 所以没有在飞状态可传：对话里没结果的调用按 Running(None) 画。
        &HashMap::new(),
        &rtheme,
        width,
    );
    let mut batch = RenderBatch {
        pane: None,
        agent: Some(agent),
        width,
        requests: HashMap::new(),
    };
    for (key, input) in frame.requests {
        let output = RenderOutput {
            lines: default_lines(&input, &rtheme),
            decorations: Vec::new(),
        };
        batch.requests.insert(key, RenderRequest { input, output });
    }
    (batch, frame.text)
}

/// 从对话建出转写要的请求、文字块和 aside 落点。
///
/// 对位上游 `transcript::message_lines` + `renderers::{entry, content, aside}`：
/// 消息里的文字进文字块，tool call / 送达的 entry / 内容块 / aside 进 `RenderBatch`。
/// 一条调用结果画在它的调用下面（`next` 那条消息里的 `ToolResult`），不单独成块。
fn conversation_frame(
    conversation: &talk::Conversation,
    history: &CallHistory,
    tree: &CallTree,
    running: &HashMap<CallId, RunningCall>,
    theme: &RTheme,
    width: u16,
) -> DialogueFrame {
    let mut frame = DialogueFrame::default();
    let entries: Vec<_> = conversation.entries().collect();
    // 被停掉的 turn：对话的 turn 记录（`TurnEnd::Stopped`）是停止的**持久**
    // 来源。live 的 `TurnOutcome` 只在 turn 结束前挂在 turn 实体上，Commit 之后
    // 实体就没了；上游 `transcript::stop_lines` 读的也是 turn 记录。
    //
    // `at` 是停止时这一轮最后一条消息；早于该轮开始、或者已被压缩移走的
    // 记录不画（上游同样 `entries.of_id(at).is_none() -> continue`）。
    let mut stopped_after: Vec<usize> = conversation
        .turns()
        .filter_map(|(_, record)| match record.end {
            Some(TurnEnd::Stopped { at: Some(at), .. }) => conversation.index_of(at),
            _ => None,
        })
        .collect();
    stopped_after.sort_unstable();
    stopped_after.dedup();
    for (at, entry) in entries.iter().enumerate() {
        // 下一条 entry 既是结果消息（`answers`），也是调用 facts 的来源
        // （`Entry::facts`，见 assistant 分支）。
        let next_entry = entries.get(at + 1);
        let next = next_entry.map(|entry| entry.message);
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
                                    on_band(lines, theme_band(theme, theme.tool)),
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
                                            theme_band(theme, theme.tool),
                                        ),
                                    ));
                                }
                                None => {
                                    let mut lines = vec![Line::default()];
                                    lines.extend(user_lines(&text.text));
                                    frame.text.push((
                                        TextId::Message { at, block },
                                        on_band(lines, theme_band(theme, theme.user)),
                                    ));
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
                // `ToolRender::facts` 的**真来源**（上游
                // `transcript::message_lines` 同一段）：结果那条 entry 里
                // 保存的 `ResultFacts`，按 `CallRef`（回复 id + 调用位置）
                // 配给每个调用。退出码/截断标记/`FilesChanged` 都在里面，
                // 恢复会话后也照样有 —— 它跟着对话存，不跟着效应走。
                let facts = next_entry.map(|next| {
                    let calls = CallRef::calls(entry.id, assistant).map(|(at, call)| (at, &call.id));
                    next.facts.of(calls)
                });
                let facts = facts.into_iter().flatten().chain(std::iter::repeat(None));
                let mut calls = (0..).zip(results).zip(facts);
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
                            let ((index, result), facts) = calls.next().unwrap_or_default();
                            let at_call = CallRef {
                                reply: entry.id,
                                index,
                            };
                            let fact = history.by_call(&call.id);
                            // 已经出现在对话里的调用仍然可能在跑：状态、输出、
                            // 子树都从活效应来（上游 `Look::state` /
                            // `ShownConversation::outputs` / `ChildCalls`）。
                            let running = running.get(&call.id);
                            let state = running.map_or_else(
                                || match result {
                                    Some(_) => CallState::Done,
                                    None => CallState::Running(None),
                                },
                                |running| running.state.clone(),
                            );
                            let output = match result {
                                Some(_) => None,
                                None => running.and_then(|running| running.output.clone()),
                            };
                            let effect = fact
                                .map(|fact| fact.effect)
                                .or_else(|| running.map(|running| running.effect));
                            frame.calls.insert(call.id.clone());
                            let render = ToolRender {
                                call: call.clone(),
                                result: result.cloned(),
                                state,
                                theme: *theme,
                                facts: facts.cloned(),
                                took: fact.and_then(|fact| fact.took),
                                output,
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
    // 停止标记：dim 的 `[stopped]`，排在那一轮最后一条消息之后（上游
    // `transcript::stop_lines` 的 assistant 文案；这里统一一行，不猜停止原因）。
    for at in stopped_after {
        frame.text.push((
            TextId::Message {
                at,
                block: STOPPED_BLOCK,
            },
            vec![Line::styled("[stopped]", Style::new().dim())],
        ));
    }
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
    let mut lines: Vec<Line<'static>> = rig_tui::markdown::render(text, theme, Some(width))
        .into_iter()
        .map(|line| line.line().clone())
        .collect();
    // 上游终端在 buffer 上统一清色；我们的渲染层没有那一步，markdown 的
    // 标题/引用/代码块颜色要在 `NO_COLOR` 时自己清掉。
    if theme.plain {
        for line in &mut lines {
            clear_colors(line);
        }
    }
    lines
}

/// 推理：dim + italic 的 markdown（上游 `renderers::reasoning_lines`）。
fn reasoning_lines(text: &str, width: u16, theme: &RTheme) -> Vec<Line<'static>> {
    let style = Style::new().dim().italic();
    reply_lines(text, width, theme)
        .into_iter()
        .map(|line| line.patch_style(style))
        .collect()
}

/// 把行放到 `band` 上（上游 `transcript::on_band`：行自己已有底色的不覆盖）；
/// `None`（`NO_COLOR`）时不画底色。
fn on_band(
    lines: impl IntoIterator<Item = Line<'static>>,
    band: Option<RColor>,
) -> Vec<Line<'static>> {
    let Some(band) = band else {
        return lines.into_iter().collect();
    };
    lines
        .into_iter()
        .map(|mut line| {
            line.style.bg = line.style.bg.or(Some(band));
            line
        })
        .collect()
}

/// `NO_COLOR` 时的 band：不画。
fn theme_band(theme: &RTheme, band: RColor) -> Option<RColor> {
    (!theme.plain).then_some(band)
}

/// 清掉一行（含 span）的前景/底色，保留修饰符（`NO_COLOR`）。
fn clear_colors(line: &mut Line<'static>) {
    line.style.fg = None;
    line.style.bg = None;
    for span in &mut line.spans {
        span.style.fg = None;
        span.style.bg = None;
    }
}

/// `NO_COLOR` 下的 look 行：先按正常方式展开代码色，再清掉所有颜色
/// （上游终端在 buffer 上统一 `strip_colors`，我们在这里做）。
fn plain_look(line: LookLine) -> LookLine {
    let mut line = line.colored();
    clear_colors(&mut line);
    LookLine::from(line)
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

/// 一次调用的状态色带，判据和上游 `ToolRender::header_at` 同一顺序：
/// 停止/被按住/排队用 `call_waiting`，运行中 `call_running`，已答复
/// `call_done`，失败（含非 0 退出码）`call_failed`。
fn call_band(call: &ToolRender) -> RColor {
    match (&call.result, &call.state) {
        (_, CallState::Stopped) => call.theme.call_waiting,
        (Some(_), _) if call.failed() => call.theme.call_failed,
        (_, CallState::Held(_)) => call.theme.call_waiting,
        (Some(_), _) => call.theme.call_done,
        (None, CallState::Running(_)) => call.theme.call_running,
        (None, _) => call.theme.call_waiting,
    }
}

/// 子 agent 调用树的行：缩进 + 名字（`agent · tool`）+ 状态，失败用
/// `call_failed` 色带、结果一行。
///
/// 这是上游 rig-basics `SubagentLooks` 的语义（那张 look 不在本仓库的依赖里，
/// 数据来自 [`ChildCall`]：`ToolRender::children`）。
fn child_call_lines(children: &[ChildCall], width: u16, theme: &RTheme) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let failed_band = theme_band(theme, theme.call_failed);
    // 名字只加粗，不再写死 Cyan；状态/失败由 band 表达。
    let name_style = Style::new().add_modifier(Modifier::BOLD);
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
            Span::styled(format!("{agent}{}", child.name), name_style),
        ]);
        let state = state_words(&child.state);
        if !state.is_empty() {
            line.push_span(Span::styled(format!("  {state}"), Style::new().dim()));
        }
        if child.failed {
            // 失败用主题的 `call_failed` band，而不是写死的红前景。
            let style = match failed_band {
                Some(band) => Style::new().bg(band),
                None => Style::new(),
            };
            line.push_span(Span::styled("  failed", style));
        }
        lines.push(line);
        if let Some(answer) = &child.answer {
            let first = answer.lines().next().unwrap_or("").trim_end();
            // TODO(width): 按字符数截断（没用 unicode 宽度；差几个全角字符，够用）。
            let room = width.saturating_sub(indent.len() + 2).max(8);
            let shown: String = first.chars().take(room).collect();
            if !shown.is_empty() {
                let style = match (child.failed, failed_band) {
                    (true, Some(band)) => Style::new().bg(band).dim(),
                    (true, None) => Style::new().dim(),
                    (false, _) => Style::new().dim(),
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
        out.decorate(
            "children",
            child_call_lines(&call.children, width, &call.theme),
        );
    }
}

/// 跑一遍 rig-tui 的 look 调度（独占：它要整个 world）。
pub fn run_looks(world: &mut World) {
    world.run_schedule(TranscriptRender);
    // `NO_COLOR`：look 可能自己写了颜色（业务 look、默认 look 的状态带），
    // 上游终端在 buffer 上统一清掉，我们的渲染层没有那一步，这里清。
    if world
        .get_resource::<Theme>()
        .is_some_and(|theme| theme.plain)
        && let Some(mut batch) = world.get_resource_mut::<RenderBatch>()
    {
        for request in batch.requests.values_mut() {
            for line in &mut request.output.lines {
                *line = plain_look(std::mem::take(line));
            }
            for (_, lines) in &mut request.output.decorations {
                for line in lines {
                    clear_colors(line);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RenderBatch -> 行
// ---------------------------------------------------------------------------

/// 把 look 的产出交给对话引擎（排序、折行、流式增补、finalize）。
///
/// 待确认的 `Ask` 不在这里拼：它们走确认面板（[`confirm_panel_rows`]）。
pub fn collect_rows(
    batch: Res<RenderBatch>,
    theme: Res<Theme>,
    mut transcript: ResMut<TranscriptEngine>,
    dialogue: Res<Dialogue>,
    stream: Res<ReplyStream>,
    prompts: Res<SubmittedPrompts>,
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
    let rtheme = upstream_theme(&theme);
    if stream.finished || dialogue.has_reply || stream.reasoning.is_empty() {
        transcript.remove(&BlockId::Text(TextId::StreamingReasoning));
    } else {
        transcript.set_text(
            TextId::StreamingReasoning,
            reasoning_lines(&stream.reasoning, width, &rtheme),
        );
    }
    if stream.finished || dialogue.has_reply || stream.text.is_empty() {
        transcript.remove(&BlockId::Text(TextId::StreamingReply));
    } else {
        // 推理和正文之间空一行（上游 `lay_out_transcript` 的最后一段）。
        let mut lines = vec![Line::default()];
        lines.extend(reply_lines(&stream.text, width, &rtheme));
        transcript.set_text(TextId::StreamingReply, lines);
    }

    // aside 落在哪条消息之后（引擎排序时要）。
    for (id, slot) in &dialogue.slots {
        transcript.place(BlockId::Request(RenderKey::Aside(*id)), *slot);
    }

    rows.0 = transcript.keyed_rows().to_vec();
}

// ---------------------------------------------------------------------------
// 确认面板（还没回答的 Ask）
// ---------------------------------------------------------------------------

/// 一条待确认的 `Ask`：实体 + 内核效应 id（FIFO 顺序的来源）。
///
/// 内核给每个效应发的 id 严格递增，所以按它升序就是"先来先显示"；上游
/// `rig-harness` 的 `answer_with_text` 也用同一个键挑"最老的问题"来回答，
/// 这里沿用同一条规则，不另造时序。
#[derive(Clone, Copy, Debug)]
pub struct PendingAsk<'a> {
    /// Ask 实体：`Row.choice` 里带的就是它，点击回答时用。
    pub entity: Entity,
    /// 内核效应 id：越小越先来。
    pub effect: EffectId,
    /// 问题本身。
    pub ask: &'a Ask,
}

/// 确认行：把所有待确认的 `Ask` 按 FIFO 排成可显示的行。
///
/// * 没有待确认的 Ask → 空 vec（确认 tile 就是 0 行）。
/// * 每个 Ask 一块：标题（`? 标题`）、正文（缩进、按 `width` 折行）、选项
///   （`    [n] 标签`，带 [`Row::choice`]，点该选项折出来的任一行都回答它）。
/// * 多个 Ask 时每块**标题行**前加 `(第几个/共几个) `；只有一个时不加，免得
///   给唯一的问题添噪音。序号只标在标题上而不标每一行：块以标题开头，每行
///   重复会和选项自己的 `[n]` 编号、正文缩进混在一起。
/// * `width` 是文字列数（和 `wrap_lines` 一致）；0 按 1 列折，不 panic。
pub fn confirm_rows(asks: &[PendingAsk<'_>], width: u16) -> Vec<Row> {
    confirm_keyed_rows(asks, width)
        .into_iter()
        .map(|keyed| keyed.row)
        .collect()
}

/// [`confirm_rows`] 的带 key/hash 版本：直接写进 `PanelRows` 的形状。
///
/// key 每帧稳定（Ask 实体 + 块内位置）；选项行的 `Row.choice` 原样保留，
/// 面板生成行实体时会据此挂上 `transcript::AskChoice`，点击语义和拆出来
/// 之前完全一样。
pub fn confirm_keyed_rows(asks: &[PendingAsk<'_>], width: u16) -> Vec<KeyedRow> {
    let mut ordered: Vec<PendingAsk<'_>> = asks.to_vec();
    ordered.sort_by_key(|pending| pending.effect);
    let total = ordered.len();
    let mut rows = Vec::new();
    for (index, pending) in ordered.iter().enumerate() {
        push_ask_block(
            &mut rows,
            pending.entity,
            pending.ask,
            (total > 1).then_some((index + 1, total)),
            width,
        );
    }
    rows
}

/// 确认 tile 的高度（UI 像素）：行数 × 行高，没有行时 0。
///
/// 树里的建议（树不归本模块改）：右列输入框**正上方**（补全 tile 和输入
/// 之间），高度随 [`PanelRows`] 里 `Confirm` 的行数改 —— 0 个 Ask 时 0 行，
/// 和补全 tile 的空闲态一样不占位置；多个 Ask 时按 [`confirm_tile_height`]
/// 取 `n * ROW_HEIGHT`。放在输入上方是因为它是"回答完才能继续"的确认，
/// 眼睛从转写落到底部时先看到它，再是输入框。
pub fn confirm_tile_height(rows: usize) -> f32 {
    rows as f32 * crate::transcript::ROW_HEIGHT
}

/// 把一个 Ask 按"标题 + 正文 + 选项"折成确认行（FIFO 已在调用方排好）。
fn push_ask_block(
    rows: &mut Vec<KeyedRow>,
    ask: Entity,
    question: &Ask,
    ordinal: Option<(usize, usize)>,
    width: u16,
) {
    let mut title = Vec::new();
    if let Some((index, total)) = ordinal {
        // 次要信息用主题的 dim 色（和 `Theme::default().dim` 同一色值）。
        title.push(Span::styled(
            format!("({index}/{total}) "),
            Style::new().fg(RColor::Rgb(120, 128, 145)),
        ));
    }
    title.push(Span::styled("? ", Style::new().fg(RColor::Magenta)));
    title.push(Span::styled(
        question.title.clone(),
        Style::new()
            .fg(RColor::LightMagenta)
            .add_modifier(Modifier::BOLD),
    ));
    push_ask_rows(rows, ask, "title", vec![Line::from(title)], width, None);
    for (index, body_line) in question.body.lines().enumerate() {
        let line = Line::from(Span::styled(
            format!("    {body_line}"),
            Style::new().fg(RColor::Rgb(200, 206, 220)),
        ));
        push_ask_rows(
            rows,
            ask,
            &format!("body{index}"),
            vec![line],
            width,
            None,
        );
    }
    for (index, choice) in question.choices.iter().enumerate() {
        let line = Line::from(Span::styled(
            format!("    [{}] {}", index + 1, choice.label),
            Style::new()
                .fg(RColor::Green)
                .add_modifier(Modifier::BOLD),
        ));
        push_ask_rows(
            rows,
            ask,
            &format!("choice{index}"),
            vec![line],
            width,
            Some(&choice.id),
        );
    }
}

/// 把当前还没回答的 `Ask` 写进确认面板（[`PanelId::Confirm`]）。
///
/// 数据来源和原来的 `collect_rows` 相同（世界里没有 `Outcome` 的 Ask），
/// 多取一个 `Effect` 作为 FIFO 序号；宽度取树给 Confirm 叶子的文字列数，
/// 根还不存在时用 97 兜底（和 `editor::editor_rows` 一致）。只装
/// `BatchPlugin` 的测试 App 没有 `PanelRows`，这时什么也不做。
///
/// **按 shown agent 过滤**（[`focused_visible`]）：多会话时别的 agent 的
/// 问题不能出现在本工作区的确认 tile 里 —— 点选项就是替它作答。没有
/// `ShownAgent` 时退回全量（无视图的测试/导出路径）。
pub fn confirm_panel_rows(
    asks: Query<(Entity, &Ask, Option<&Effect>, Option<&ForAgent>), Without<Outcome>>,
    roots: Option<Res<PanelRoots>>,
    areas: Option<Res<PanelAreas>>,
    turn: ShownTurn,
    mut panels: Option<ResMut<PanelRows>>,
) {
    let Some(panels) = panels.as_deref_mut() else {
        return;
    };
    let width = roots
        .as_deref()
        .and_then(|roots| roots.entity(PanelId::Confirm))
        .and_then(|root| areas.as_deref().and_then(|areas| areas.get(root)))
        .map_or(97, |area| area.cols.max(1));
    let shown = turn.agent();
    let pending: Vec<PendingAsk<'_>> = asks
        .iter()
        .filter(|(_, _, _, for_agent)| {
            focused_visible(shown, for_agent.map(|ForAgent(owner)| *owner))
        })
        .map(|(entity, ask, effect, _)| PendingAsk {
            entity,
            // 还没登记成效应的 Ask（正常一帧内就有）排最后，但不丢掉：
            // 确认面板显示的是"这个 agent 的、所有没回答的 Ask"，不只是有
            // 效应 id 的。
            effect: effect.map(Effect::id).unwrap_or(EffectId::from_raw(u64::MAX)),
            ask,
        })
        .collect();
    panels.set(PanelId::Confirm, confirm_keyed_rows(&pending, width));
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
// 实时内容（没有自己管线的工作区）
// ---------------------------------------------------------------------------

/// 一个 agent 的**实时**内容：在飞的工具调用、正在流的正文/推理、待回答的 Ask。
///
/// 聚焦工作区从 [`rebuild_batch`] 的整条管线拿这些（活效应 + 调用树 + look 调度
/// + 流式 `Partial`）；没有自己管线的工作区（`rig_harness_in_3d::workspaces_demo`
/// 的非聚焦条带）就调这个函数直接读内核，把结果**接在** [`dialogue_batch`] 的
/// 行流后面。顺序和真管线一致：调用（按内核效应 id，先来先显示）→ 流式推理
/// → 流式正文 → 待回答的 Ask。
///
/// * 调用：`ToolRender::default_lines`（对话之外的调用在真管线里也走这个默认
///   画法），状态用 [`live_state`] 的同一套判断 —— running / held / failed /
///   done 的说法和聚焦条带一字不差；不跑 look 调度，也不画调用树（那要 look），
///   所以只画本 agent 树的**根**：子 agent 的调用挂在父卡片下的那些不在这里
///   单独占一张卡（真管线由 `child_calls_look` 画）。已结算但还没落进对话的
///   调用（[`CallHistory`]）也画，条件和 [`rebuild_batch`] 第 4 步一样：不在
///   对话里、是这棵树的根；行 key 和它在飞时相同（`tool:<effect>`），结算时
///   原地换状态，不会先消失再出现。owner 的口径和聚焦管线一致
///   （[`focused_visible`]）：没有 `ForAgent` 的调用在两条条带里都可见；
/// * 正文/推理：直接复用 [`reply_lines`] / [`reasoning_lines`]，按 `width` 折行
///   （[`wrap_lines`]，和转写同一个折行器）；
/// * Ask：复用 [`confirm_keyed_rows`]（FIFO 仍按内核效应 id），只取本 agent
///   的（没有 `ForAgent` 的按无主处理，和确认面板同一口径）；`Row::choice`
///   原样保留，点击语义和确认面板一样。
///
/// 已经落进对话的调用不会重复画（真管线里的 `rendered` 集合做同一件事）。
pub fn live_additions(world: &mut World, agent: Entity, width: u16) -> Vec<KeyedRow> {
    // 主题资源可能不存在（没有装 `BatchPlugin`/`FeathersViewPlugin` 的裸 world）：
    // 那时退回默认主题。存在时和聚焦管线一样逐字段映射到上游 `RTheme`。
    let our_theme = world.get_resource::<Theme>().copied().unwrap_or_default();
    let rtheme = upstream_theme(&our_theme);
    let mut rows: Vec<KeyedRow> = Vec::new();

    /// 一个还没结算的调用：建树只用到效应 id / 父 / owner，画卡用其余字段。
    struct Live {
        effect: EffectId,
        parent: Option<EffectId>,
        owner: Option<Entity>,
        call: ToolCall,
        result: Option<ToolResult>,
        state: CallState,
        /// 答复过才有（见 [`published_facts`]）。
        facts: Option<ToolResultContext>,
        output: Option<String>,
    }

    // 1) 在飞的调用：还没结算的 `ToolRequest`。收全量（别的 agent 的也收）才能
    //    把父子关系接起来；画的时候只画本 agent 那棵树的根。
    let known = conversation_calls(world, agent);
    let mut live: Vec<Live> = Vec::new();
    {
        let mut requests = world.query_filtered::<
            (
                &ToolRequest,
                &Effect,
                Option<&Outcome>,
                Has<Started>,
                Option<&Running>,
                Option<&Holds>,
                Option<&ToolProgress>,
                Option<&ForAgent>,
                Option<&Published>,
                Option<&CallOutput>,
            ),
            Without<Settled>,
        >();
        // 同一个 world 的两次共享借用：查询请求 + 在闭包里读持有者的名字。
        let shared: &World = world;
        for (
            request,
            effect,
            outcome,
            started,
            running,
            holds,
            progress,
            for_agent,
            published,
            output,
        ) in requests.iter(shared)
        {
            live.push(Live {
                effect: effect.id(),
                parent: effect.parent,
                owner: for_agent.map(|ForAgent(owner)| *owner),
                call: request.call.clone(),
                result: outcome.map(|outcome| tool_result_of(outcome, &request.call)),
                state: live_state(outcome, started, running, holds, progress, |hold| {
                    shared
                        .get::<Name>(hold)
                        .map(|name| name.as_str().to_owned())
                }),
                facts: published_facts(published),
                output: output.map(CallOutput::text).filter(|text| !text.is_empty()),
            });
        }
    }
    live.sort_by_key(|call| call.effect);

    // 2) 调用树：实时 + 已结算（`CallHistory`）都接进来，才认得出谁是根、
    //    谁挂在父卡片下。仍然收全量（别的 agent 的也收），过滤靠 `is_card`。
    //    历史只借两次、不整段克隆（每帧每个非聚焦条带都会调这个函数）。
    let mut tree = CallTree::default();
    // 已知的父子关系全接上，包括模型调用这类没有卡片的效应（和
    // `rebuild_batch` 第 2 步同一口径）：子 agent 的调用要穿过它们挂到
    // 发起它的调用下面。
    if let Some(parents) = world.get_resource::<EffectParents>() {
        for (effect, parent) in parents.iter() {
            tree.link(effect, parent);
        }
    }
    let mut roots: Vec<EffectId> = Vec::new();
    for call in &live {
        // 对话里已经有的调用也接进树：它下面的实时子调用不能因此升成根。
        tree.link(call.effect, call.parent);
        if focused_visible(Some(agent), call.owner) {
            roots.push(call.effect);
        }
    }
    if let Some(history) = world.get_resource::<CallHistory>() {
        for fact in &history.0 {
            tree.link(fact.effect, fact.parent);
            if focused_visible(Some(agent), fact.owner) {
                roots.push(fact.effect);
            }
        }
    }
    let scope = tree.scope(roots);

    // 3) 根卡片：已结算的在前（和 `rebuild_batch` 第 4 步的插入顺序一致）再按
    //    效应 id 排，先来先显示。已结算但还没落进对话的也画：不画的话，调用
    //    结算到对话提交之间会在条带上先消失再出现（闪一下）；行 key 在飞时和
    //    结算后都是 `tool:<effect>`，结算只原地换状态。子调用只出现在父卡片的
    //    children 里（非聚焦条带没有 look 画它，不重复占卡）。
    let mut cards: Vec<(EffectId, ToolRender)> = Vec::new();
    let mut rendered: HashSet<CallId> = known;
    if let Some(history) = world.get_resource::<CallHistory>() {
        for fact in &history.0 {
            if !tree.is_card(Some(&scope), fact.effect)
                || !rendered.insert(fact.call.id.clone())
            {
                continue;
            }
            let mut render =
                ToolRender::new(fact.call.clone(), Some(fact.result.clone()), CallState::Done);
            // `ToolRender::new` 内部写死 `RTheme::default()`，这里换成映射后的
            // 主题（状态带/结果色都跟我们的 `Theme` 走）。
            render.theme = rtheme;
            render.took = fact.took;
            // 结算时从 `Published` 抄下的事实（退出码/截断标记/…）。
            render.facts = fact.facts.clone();
            render.item = Some(CallItem::Streaming(fact.ordinal));
            cards.push((fact.effect, render));
        }
    }
    for call in &live {
        if !tree.is_card(Some(&scope), call.effect) || !rendered.insert(call.call.id.clone()) {
            continue;
        }
        let mut render =
            ToolRender::new(call.call.clone(), call.result.clone(), call.state.clone());
        render.theme = rtheme;
        render.facts = call.facts.clone();
        render.output = call.output.clone();
        cards.push((call.effect, render));
    }
    cards.sort_by_key(|(effect, _)| *effect);
    for (effect, render) in cards {
        let mut lines: Vec<Line<'static>> = render
            .default_lines()
            .iter()
            .map(LookLine::colored)
            .collect();
        if rtheme.plain {
            for line in &mut lines {
                clear_colors(line);
            }
        }
        push_live_block(
            &mut rows,
            &format!("tool:{}", effect.as_u64()),
            lines,
            width,
        );
    }

    // 4) 正在流的推理/正文：运行中 turn 的 `Partial`，和真管线同一条路径。
    if let Some(partial) = agent_partial(world, agent) {
        // 回复已经落进对话的那一帧，流式块让位，别画两份（真管线用
        // `dialogue.has_reply` 做同一件事）。
        let in_dialogue = !partial.text.is_empty()
            && world.get::<Conversation>(agent).is_some_and(|conversation| {
                conversation_has_text(conversation, &partial.text)
            });
        if !in_dialogue && !partial.reasoning.is_empty() {
            push_live_block(
                &mut rows,
                "streaming-reasoning",
                reasoning_lines(&partial.reasoning, width, &rtheme),
                width,
            );
        }
        if !in_dialogue && !partial.text.is_empty() {
            // 推理和正文之间空一行（和 `collect_rows` 的流式块同形）。
            let mut lines = vec![Line::default()];
            lines.extend(reply_lines(&partial.text, width, &rtheme));
            push_live_block(&mut rows, "streaming-reply", lines, width);
        }
    }

    // 5) 待回答的 Ask：只取本 agent 的（无主 = 可见，和确认面板同一口径），
    //    FIFO 仍由内核效应 id 决定。
    let mut asks: Vec<(Entity, EffectId, Ask)> = Vec::new();
    {
        let mut pending = world.query_filtered::<
            (Entity, &Ask, Option<&Effect>, Option<&ForAgent>),
            Without<Outcome>,
        >();
        for (entity, ask, effect, for_agent) in pending.iter(world) {
            if !focused_visible(Some(agent), for_agent.map(|ForAgent(owner)| *owner)) {
                continue;
            }
            asks.push((
                entity,
                // 还没登记成效应的 Ask（正常一帧内就有）排最后，但不丢掉。
                effect.map(Effect::id).unwrap_or(EffectId::from_raw(u64::MAX)),
                ask.clone(),
            ));
        }
    }
    let pending: Vec<PendingAsk<'_>> = asks
        .iter()
        .map(|(entity, effect, ask)| PendingAsk {
            entity: *entity,
            effect: *effect,
            ask,
        })
        .collect();
    rows.extend(confirm_keyed_rows(&pending, width));
    rows
}

/// 对话里已经有的调用 id：实时块不再重复画它们（真管线里的 `rendered`）。
fn conversation_calls(world: &World, agent: Entity) -> HashSet<CallId> {
    let Some(conversation) = world.get::<Conversation>(agent) else {
        return HashSet::new();
    };
    let mut known = HashSet::new();
    for entry in conversation.entries() {
        if let Message::Assistant(assistant) = entry.message {
            known.extend(assistant.tool_calls().map(|call| call.id.clone()));
        }
    }
    known
}

/// agent 正在流式输出的回复：运行中 turn 的 `Partial`。
///
/// 和 [`ShownTurn::partial`] 同一条路径（`ActiveTurn` → `Calls` → 最后一个
/// 带 `Partial` 的调用），两条条带对"正在流的是哪段"看法一致。
fn agent_partial<'w>(world: &'w World, agent: Entity) -> Option<&'w Partial> {
    let turn = world.get::<ActiveTurn>(agent)?.turn();
    let calls = world.get::<Calls>(turn)?;
    calls
        .iter()
        .filter_map(|call| world.get::<Partial>(call))
        .next_back()
}

/// 把一块行按 `width` 折好，追加成带稳定 key 的 [`KeyedRow`]。
fn push_live_block(
    rows: &mut Vec<KeyedRow>,
    part: &str,
    lines: Vec<Line<'static>>,
    width: u16,
) {
    for (index, row) in wrap_lines(&lines, width).into_iter().enumerate() {
        rows.push(KeyedRow {
            key: live_key(part, index),
            hash: row_hash(&row),
            row,
        });
    }
}

/// 实时行的 key：段标记在 bit62，`part` 的哈希在 bit61..32，**块内行号**在
/// bit31..0。
///
/// ```text
/// bit 63..62  01        LIVE 段：引擎按位置发的 key 最高位是 0、Ask 行是 0x8000 段
/// bit 61..32            key_of(&[part]) 的低 30 位
/// bit 31..0             这一块里的行号（push_live_block 按折行顺序从 0 编）
/// ```
///
/// 这和引擎的 `transcript::row_key`（`ordinal << 32 | index`）是**同一个约定**：
/// 高 32 位是"块 / 条目"身份、低 32 位是块内行号。`pane::item_rows` 只看
/// `key >> 32`，所以同一 `part`（一次调用 / 一段流式回复）的所有行会被分成
/// **一个条目**，不同调用各自一个条目；Alt+↑/↓ 一下一条，不会逐行走。
///
/// 30 位哈希：同一个 `part` 每帧稳定（哈希只吃 `part`，不吃行号），
/// 不同 `part` 的碰撞概率约 2^-30 —— 面板 diff 用 key 对齐，撞了最多两张卡
/// 被当成同一块（内容 hash 仍逐行更新），可以接受。
///
/// `part` 带效应 id / 种类，同一块内容每帧是同一个 key，面板的增量 diff 才能
/// 原地改行。`Ask` 行不是这里发的（走 `confirm` 面板的 `ask_key`，0x8000 段）；
/// `BELOW_MARKER_KEY = u64::MAX` 也不在这个段里，两者都不受影响。
fn live_key(part: &str, index: usize) -> u64 {
    /// 段标记：bit62 = 1，bit63 = 0（避开引擎的 0x0_ 段和 Ask 的 0x8000 段）。
    const LIVE: u64 = 0x4000_0000_0000_0000;
    // 最高两位留给段标记：part 只取哈希低 30 位，放在 bit61..32。
    let block = key_of(&[part]) & 0x3FFF_FFFF;
    LIVE | (block << 32) | (index as u64 & 0xFFFF_FFFF)
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
    // header 的底色来自调用的状态 band，不再写死状态词的前景色。
    let band = theme_band(&call.theme, call_band(call));
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

    let mut header = Line::from(vec![
        Span::styled("* ", Style::default()),
        Span::styled("search_products", Style::default().bold()),
        Span::styled(format!("  {state}"), Style::default().italic()),
        Span::styled(
            format!("  query={}", call.argument("query").unwrap_or("-")),
            Style::default().dim(),
        ),
    ]);
    if let Some(band) = band {
        header = header.bg(band);
    }
    let mut lines: Vec<LookLine> = vec![header.into()];

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
        out.lines = order_lines(call);
    }
}

fn order_lines(call: &ToolRender) -> Vec<LookLine> {
    let held = matches!(call.state, CallState::Held(_));
    let done = matches!(call.state, CallState::Done);
    let tail = if held {
        "  waiting for your confirmation (gate)"
    } else if done {
        "  order placed"
    } else {
        "  queued"
    };
    // 和 search 一样：header 底色来自调用的状态 band，状态词不再写死颜色。
    let mut header = Line::from(vec![
        Span::styled("* ", Style::default()),
        Span::styled("create_order", Style::default().bold()),
        Span::styled(tail, Style::default().dim()),
    ]);
    if let Some(band) = theme_band(&call.theme, call_band(call)) {
        header = header.bg(band);
    }
    let mut lines = vec![header.into()];
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
                    Style::default().dim(),
                ))
                .into(),
            );
        }
    }
    lines
}

pub struct BatchPlugin;

impl Plugin for BatchPlugin {
    fn build(&self, app: &mut App) {
        // `Theme` 是绘制系统（`rebuild_batch`/`collect_rows`/`run_looks`）的
        // 必需资源；应用层通常由 `FeathersViewPlugin` 用 `Theme::from_env()`
        // 插入（`NO_COLOR`）。这里只做兜底：已存在时不覆盖。
        app.init_resource::<Theme>()
            .init_resource::<RenderBatch>()
            .init_resource::<ReplyStream>()
            .init_resource::<CompletedCalls>()
            .init_resource::<CallHistory>()
            .init_resource::<LiveOrdinals>()
            .init_resource::<Dialogue>()
            .init_resource::<TranscriptEngine>()
            .init_resource::<EffectParents>()
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
            .add_systems(
                AgentPass,
                (record_effect_parents, record_calls).in_set(Stage::Review),
            )
            .add_systems(
                Update,
                (
                    track_reply_stream,
                    rebuild_batch,
                    run_looks,
                    collect_rows,
                    confirm_panel_rows,
                )
                    .chain()
                    .in_set(crate::FeathersSystems::Draw),
            );
    }
}

#[cfg(test)]
mod tests {
    //! 建批的测试：不跑模型、不连网，用构造的 `Conversation` / `CallFact` 断言。

    use super::*;
    use rig_core::message::{
        AssistantContent, Image, Text, ToolFunction, ToolName, ToolResultContent,
    };
    use rig_core::tool::ToolContext;

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
            &HashMap::new(),
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
            &HashMap::new(),
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

    /// 一段用户请求 + assistant 回复的对话。
    fn answered_conversation() -> (talk::Conversation, talk::MessageId) {
        let mut conversation = talk::Conversation::new();
        conversation.push(Message::user("go"));
        let reply = conversation.push(Message::assistant("half an answer"));
        (conversation, reply)
    }

    fn frame_of(conversation: &talk::Conversation) -> DialogueFrame {
        conversation_frame(
            conversation,
            &CallHistory::default(),
            &CallTree::default(),
            &HashMap::new(),
            &RTheme::default(),
            80,
        )
    }

    /// 这个文字块是不是 `[stopped]`（`TextId::Message` 的 STOPPED_BLOCK）。
    fn is_stopped(id: &TextId) -> bool {
        matches!(id, TextId::Message { block, .. } if *block == STOPPED_BLOCK)
    }

    /// 被停掉的 turn：在它最后一条消息之后画一条 dim 的 `[stopped]`。
    #[test]
    fn a_stopped_turn_draws_a_dim_stopped_line_after_its_last_message() {
        let (mut conversation, reply) = answered_conversation();
        conversation.open_turn();
        // 用户按 Esc 停掉：`Stopper::stop` 在 turn 记录里写 `at` = 最后一条消息。
        conversation.end_turn(TurnEnd::Stopped {
            by: Some(talk::StopCause::User),
            at: Some(reply),
            calls: Vec::new(),
        });
        let frame = frame_of(&conversation);
        let (_, lines) = frame
            .text
            .iter()
            .find(|(id, _)| {
                *id == TextId::Message {
                    at: 1,
                    block: STOPPED_BLOCK,
                }
            })
            .expect("assistant 那一轮（第 1 条消息）之后应有停止标记");
        assert_eq!(lines_text(lines), "[stopped]");
        assert!(
            lines[0].style.add_modifier.contains(Modifier::DIM),
            "停止标记是 dim 行：{:?}",
            lines[0].style
        );
    }

    /// 正常结束（Answered）的 turn 不画停止标记。
    #[test]
    fn an_answered_turn_has_no_stopped_line() {
        let (mut conversation, reply) = answered_conversation();
        conversation.open_turn();
        conversation.end_turn(TurnEnd::Answered {
            answer: Some(reply),
        });
        let frame = frame_of(&conversation);
        assert!(
            frame.text.iter().all(|(id, _)| !is_stopped(id)),
            "Answered 记录不该出现 [stopped]"
        );
        assert!(!frame
            .text
            .iter()
            .any(|(_, lines)| lines_text(lines).contains("[stopped]")));
    }

    /// 没有 `at` 的停止记录（例如压缩在它自己的进程里结束的 turn）没有可判定的
    /// 落点：不画，别凭空在末尾造一条。
    #[test]
    fn a_stop_record_without_a_last_message_draws_nothing() {
        let (mut conversation, _) = answered_conversation();
        conversation.open_turn();
        conversation.end_turn(TurnEnd::Stopped {
            by: Some(talk::StopCause::User),
            at: None,
            calls: Vec::new(),
        });
        let frame = frame_of(&conversation);
        assert!(
            frame.text.iter().all(|(id, _)| !is_stopped(id)),
            "没有 at 的停止记录不画"
        );
    }

    /// 压缩摘要（`Author::plugin("compaction")` 的 user 条目）本身照常画成
    /// entry；但 live 的 `Conversation` 不保存"它替换了多少条消息"，所以不能
    /// 凭空画 `N messages above were compacted`（那需要上游的 `Replaced`/会话
    /// 日志，本前端都还没有，见 REMAINING §8）。
    #[test]
    fn a_compaction_summary_gets_no_invented_message_count() {
        let mut conversation = talk::Conversation::new();
        conversation.push(Message::user("old question"));
        conversation.push(Message::assistant("old answer"));
        // `rig_memory::apply_summary` 写进对话的形状：一条 user 条目，作者是
        // `Author::plugin("compaction")`，正文带这层包裹。
        let summary = talk::Entry::new(Message::user(
            "The conversation history before this point was compacted into the following summary:\n\n\
             <summary>\nold exchange\n</summary>",
        ))
        .by(Author::plugin("compaction"));
        conversation.splice(..2, [summary], Author::plugin("compaction"));

        let frame = frame_of(&conversation);
        // 摘要本身仍然作为 entry 画出来（不是用户敲的那段文字）。
        assert!(
            frame.requests.iter().any(|(key, input)| {
                matches!(key, RenderKey::Entry(0, 0))
                    && matches!(input, RenderInput::Entry(entry)
                        if entry.plugin == "compaction" && entry.text.contains("old exchange"))
            }),
            "压缩摘要应照常画成 entry"
        );
        // 计数不在 conversation 里：任何一行都不能声称压缩了多少条消息。
        let all_text = frame
            .text
            .iter()
            .map(|(_, lines)| lines_text(lines))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!all_text.contains("messages above were compacted"));
        assert!(!all_text.contains("message above was compacted"));
    }

    #[test]
    fn a_tool_result_is_drawn_under_its_call_and_not_again_as_text() {
        let (conversation, _, _) = conversation();
        let frame = conversation_frame(
            &conversation,
            &CallHistory::default(),
            &CallTree::default(),
            &HashMap::new(),
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
            &HashMap::new(),
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
        let theme = RTheme::default();
        let lines = child_call_lines(&children, 40, &theme);
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
        // 失败用 `call_failed` 色带（不是写死的红前景）。
        assert_eq!(
            lines[2].spans.last().map(|span| span.style.bg),
            Some(Some(theme.call_failed))
        );
        assert_eq!(
            lines[2].spans.last().map(|span| span.style.fg),
            Some(None)
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

    /// 一条聚焦条带的 App：`shown` 有对话，`ShownAgent` 指向它。
    fn focused_app() -> (App, Entity) {
        let mut app = headless_app();
        let shown = app
            .world_mut()
            .spawn(Conversation(talk::Conversation::new()))
            .id();
        app.insert_resource(ShownAgent(Some(shown)));
        (app, shown)
    }

    /// 直接跑一次 [`rebuild_batch`]（不跑内核的其它系统）。
    fn rebuild_now(app: &mut App) {
        use bevy::ecs::system::RunSystemOnce;

        app.world_mut()
            .run_system_once(rebuild_batch)
            .expect("建批系统要能跑");
    }

    /// 一条结算过的调用事实（直接喂 `CallHistory`，绕开内核的结算）。
    fn settled_fact(
        effect: u64,
        parent: Option<u64>,
        owner: Option<Entity>,
        ordinal: usize,
        call: ToolCall,
    ) -> CallFact {
        let result = ToolResult {
            call: call.id.clone(),
            name: call.function.name.clone(),
            content: vec![ToolResultContent::Text(Text::new(format!(
                "{} result",
                call.function.name
            )))],
            is_error: false,
        };
        CallFact {
            effect: EffectId::from_raw(effect),
            parent: parent.map(EffectId::from_raw),
            // `agent` 只是显示名；过滤看 `owner`。
            agent: owner.map(|_| "owner".to_owned()),
            owner,
            ordinal,
            call,
            result,
            facts: None,
            took: None,
        }
    }

    /// 批里每张顶层 `Streaming` 卡：序号、工具名、父卡片上的子调用名。
    fn streaming_cards(batch: &RenderBatch) -> Vec<(usize, String, Vec<String>)> {
        let mut cards: Vec<_> = batch
            .requests
            .iter()
            .filter_map(|(key, request)| match (key, &request.input) {
                (RenderKey::Streaming(at), RenderInput::Tool(tool)) => Some((
                    *at,
                    tool.call.function.name.as_str().to_owned(),
                    tool.children.iter().map(|child| child.name.clone()).collect(),
                )),
                _ => None,
            })
            .collect();
        cards.sort_by_key(|(at, ..)| *at);
        cards
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

    // -----------------------------------------------------------------------
    // 聚焦条带只画 shown agent 自己的调用树
    // -----------------------------------------------------------------------

    #[test]
    fn a_focused_batch_draws_only_the_shown_agents_calls() {
        let (mut app, shown) = focused_app();
        let other = app.world_mut().spawn_empty().id();
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_shown_live", "shown_live"),
            },
            ForAgent(shown),
        ));
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_other_live", "other_live"),
            },
            ForAgent(other),
        ));
        app.world_mut()
            .resource_mut::<CallHistory>()
            .0
            .extend([
                settled_fact(
                    100,
                    None,
                    Some(shown),
                    50,
                    named_call("call_shown_done", "shown_done"),
                ),
                settled_fact(
                    101,
                    None,
                    Some(other),
                    51,
                    named_call("call_other_done", "other_done"),
                ),
            ]);
        rebuild_now(&mut app);

        let cards = streaming_cards(app.world().resource::<RenderBatch>());
        let names: Vec<&str> = cards.iter().map(|(_, name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec!["shown_live", "shown_done"],
            "聚焦条带只画 shown agent 的在飞 + 结算调用：{cards:?}"
        );
    }

    #[test]
    fn a_subagent_call_hangs_under_its_parent_and_gets_no_card_of_its_own() {
        let (mut app, shown) = focused_app();
        let helper = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<CallHistory>()
            .0
            .extend([
                settled_fact(
                    10,
                    None,
                    Some(shown),
                    50,
                    named_call("call_settled_parent", "settled_parent"),
                ),
                settled_fact(
                    11,
                    Some(10),
                    Some(helper),
                    51,
                    named_call("call_settled_child", "settled_child"),
                ),
            ]);
        let live_parent = app
            .world_mut()
            .spawn((
                ToolRequest {
                    call: named_call("call_live_parent", "live_parent"),
                },
                ForAgent(shown),
            ))
            .id();
        let parent_effect = app
            .world()
            .get::<Effect>(live_parent)
            .expect("活调用要有自己的效应")
            .id();
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_live_child", "live_child"),
            },
            Effect::with_parent_id(Some(parent_effect)),
            ForAgent(helper),
        ));
        rebuild_now(&mut app);

        let cards = streaming_cards(app.world().resource::<RenderBatch>());
        let names: Vec<&str> = cards.iter().map(|(_, name, _)| name.as_str()).collect();
        assert_eq!(names.len(), 2, "只该有父调用两张卡：{cards:?}");
        assert!(names.contains(&"settled_parent"), "{cards:?}");
        assert!(names.contains(&"live_parent"), "{cards:?}");
        assert!(!names.contains(&"settled_child"), "子调用不能再占一张卡：{cards:?}");
        assert!(!names.contains(&"live_child"), "活的子调用也不能再占一张卡：{cards:?}");

        let children_of = |name: &str| {
            cards
                .iter()
                .find(|(_, card, _)| card == name)
                .map(|(_, _, children)| children.clone())
                .unwrap_or_else(|| panic!("批里没有 {name} 的卡：{cards:?}"))
        };
        assert_eq!(
            children_of("settled_parent"),
            vec!["settled_child".to_owned()],
            "结算过的子 agent 调用挂在父卡片的 children 里"
        );
        assert_eq!(
            children_of("live_parent"),
            vec!["live_child".to_owned()],
            "还在跑的子 agent 调用也挂在父卡片的 children 里"
        );
    }

    #[test]
    fn another_agents_call_tree_does_not_leak_into_the_focused_strip() {
        let (mut app, _shown) = focused_app();
        let other = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<CallHistory>()
            .0
            .extend([
                settled_fact(
                    20,
                    None,
                    Some(other),
                    50,
                    named_call("call_other_parent", "other_parent"),
                ),
                settled_fact(
                    21,
                    Some(20),
                    Some(other),
                    51,
                    named_call("call_other_child", "other_child"),
                ),
            ]);
        let live_parent = app
            .world_mut()
            .spawn((
                ToolRequest {
                    call: named_call("call_other_live_parent", "other_live_parent"),
                },
                ForAgent(other),
            ))
            .id();
        let parent_effect = app
            .world()
            .get::<Effect>(live_parent)
            .expect("活调用要有自己的效应")
            .id();
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_other_live_child", "other_live_child"),
            },
            Effect::with_parent_id(Some(parent_effect)),
            ForAgent(other),
        ));
        rebuild_now(&mut app);

        let batch = app.world().resource::<RenderBatch>();
        assert!(
            batch.requests.is_empty(),
            "别的 agent 的父调用连同子调用都不该出现：{:?}",
            streaming_cards(batch)
        );
    }

    #[test]
    fn without_a_shown_agent_the_batch_falls_back_to_every_call() {
        // 没有视图/导出的路径：没有 `ShownAgent` 就不过滤，免得丢掉调用。
        let mut app = headless_app();
        let alpha = app.world_mut().spawn_empty().id();
        let beta = app.world_mut().spawn_empty().id();
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_alpha_live", "alpha_live"),
            },
            ForAgent(alpha),
        ));
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_beta_live", "beta_live"),
            },
            ForAgent(beta),
        ));
        app.world_mut()
            .resource_mut::<CallHistory>()
            .0
            .extend([
                settled_fact(
                    30,
                    None,
                    Some(alpha),
                    50,
                    named_call("call_alpha_done", "alpha_done"),
                ),
                settled_fact(
                    31,
                    None,
                    Some(beta),
                    51,
                    named_call("call_beta_done", "beta_done"),
                ),
                // owner 缺失的老历史：没有 shown agent 时照旧全画。
                settled_fact(
                    32,
                    None,
                    None,
                    52,
                    named_call("call_legacy_done", "legacy_done"),
                ),
            ]);
        rebuild_now(&mut app);

        let cards = streaming_cards(app.world().resource::<RenderBatch>());
        let names: Vec<&str> = cards.iter().map(|(_, name, _)| name.as_str()).collect();
        for expected in [
            "alpha_live",
            "beta_live",
            "alpha_done",
            "beta_done",
            "legacy_done",
        ] {
            assert!(
                names.contains(&expected),
                "没有 ShownAgent 时退回全量，缺 {expected}：{names:?}"
            );
        }
        assert_eq!(names.len(), 5, "每张调用卡都在：{names:?}");
    }

    #[test]
    fn a_settled_call_keeps_the_agent_entity_as_its_owner() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<CompletedCalls>();
        world.init_resource::<CallHistory>();
        world.init_resource::<LiveOrdinals>();
        let alpha = world.spawn_empty().id();
        let beta = world.spawn_empty().id();
        for (owner, id) in [(alpha, "call_alpha"), (beta, "call_beta")] {
            let call = named_call(id, "search");
            let outcome = Outcome::from(rig_core::tool::ToolResult::success(
                rig_core::tool::ToolOutput::text("done"),
            ));
            world.spawn((ToolRequest { call }, outcome, ForAgent(owner)));
        }
        world.run_system_once(record_calls).unwrap();

        let history = world.resource::<CallHistory>();
        assert_eq!(history.0.len(), 2, "两个结算调用都要进历史");
        let owner_of = |id: &str| {
            history
                .0
                .iter()
                .find(|fact| fact.call.id == CallId::from_wire(id))
                .and_then(|fact| fact.owner)
        };
        assert_eq!(
            owner_of("call_alpha"),
            Some(alpha),
            "owner 直接从 ForAgent 的实体来"
        );
        assert_eq!(owner_of("call_beta"), Some(beta));
    }

    // -----------------------------------------------------------------------
    // 确认面板（Ask 从转写里拆出来）
    // -----------------------------------------------------------------------

    /// 一个构造好的 Ask：标题 + 正文 + 若干 `(id, label)` 选项。
    fn question(title: &str, body: &str, choices: &[(&str, &str)]) -> Ask {
        let mut ask = Ask::new(title).body(body);
        for (id, label) in choices {
            ask = ask.choice(*id, *label);
        }
        ask
    }

    /// 一条待确认的 Ask（测试用假实体 + 手写的 FIFO 序号）。
    fn pending<'a>(ask: &'a Ask, entity: Entity, effect: u64) -> PendingAsk<'a> {
        PendingAsk {
            entity,
            effect: EffectId::from_raw(effect),
            ask,
        }
    }

    fn confirm_texts(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|row| text(&row.line)).collect()
    }

    /// 一个只有确认面板的世界：行写进 `PanelRows`，根和区域都就位。
    fn confirm_world(width: u16) -> (World, Entity) {
        let mut world = World::new();
        world.insert_resource(rig_ecs::effects::Effects::default());
        world.init_resource::<PanelRows>();
        world.init_resource::<PanelRoots>();
        world.init_resource::<PanelAreas>();
        let root = world.spawn_empty().id();
        world
            .resource_mut::<PanelRoots>()
            .0
            .insert(PanelId::Confirm, root);
        world.resource_mut::<PanelAreas>().0.insert(
            root,
            crate::frame::PanelArea {
                rect: crate::frame::CellRect::new(0, 0, width, 8),
                size: Vec2::ZERO,
                cols: width,
                rows: 8,
                center: None,
            },
        );
        (world, root)
    }

    #[test]
    fn no_pending_ask_means_the_confirm_tile_is_empty() {
        assert!(confirm_rows(&[], 80).is_empty());
        assert!(confirm_keyed_rows(&[], 80).is_empty());
        assert_eq!(confirm_tile_height(0), 0.0);
    }

    #[test]
    fn one_ask_lays_out_its_title_body_and_choices_as_clickable_rows() {
        let entity = Entity::from_raw_u32(42).expect("合法实体");
        let ask = question(
            "Keep the changes?",
            "3 files changed",
            &[("yes", "Yes"), ("no", "No")],
        );
        let rows = confirm_rows(&[pending(&ask, entity, 1)], 80);

        // 标题 + 1 行正文 + 2 个选项。
        assert_eq!(rows.len(), 4);
        assert_eq!(
            confirm_texts(&rows),
            vec![
                "? Keep the changes?".to_owned(),
                "    3 files changed".to_owned(),
                "    [1] Yes".to_owned(),
                "    [2] No".to_owned(),
            ]
        );

        // 只有选项行可点，且带的是这个 Ask 和它自己的选项 id。
        let choices: Vec<(Entity, String)> = rows
            .iter()
            .filter_map(|row| row.choice.clone())
            .collect();
        assert_eq!(choices.len(), ask.choices.len(), "几个选项就几行可点");
        assert_eq!(
            choices,
            vec![
                (entity, "yes".to_owned()),
                (entity, "no".to_owned()),
            ]
        );
    }

    #[test]
    fn two_asks_show_first_come_first_served_with_their_ordinal() {
        let first = question("first", "", &[("ok", "OK")]);
        let second = question("second", "", &[("ok", "OK")]);
        let first_entity = Entity::from_raw_u32(11).expect("合法实体");
        let second_entity = Entity::from_raw_u32(22).expect("合法实体");

        // 故意把后来的 Ask 排在查询前面：FIFO 应由 EffectId 决定，不看查询顺序。
        let rows = confirm_rows(
            &[
                pending(&second, second_entity, 20),
                pending(&first, first_entity, 10),
            ],
            80,
        );
        let texts = confirm_texts(&rows);
        // 每个 Ask 一块：标题 + 一个选项。
        assert_eq!(texts[0], "(1/2) ? first");
        assert_eq!(texts[1], "    [1] OK");
        assert_eq!(texts[2], "(2/2) ? second");
        assert_eq!(texts[3], "    [1] OK");
        // 选项仍然各归各的 Ask。
        let ask_of: Vec<Entity> = rows
            .iter()
            .filter_map(|row| row.choice.as_ref().map(|(ask, _)| *ask))
            .collect();
        assert_eq!(ask_of, vec![first_entity, second_entity]);

        // 只有一个 Ask 时不加序号（不给唯一的问题添噪音）。
        let single = confirm_texts(&confirm_rows(&[pending(&first, first_entity, 10)], 80));
        assert_eq!(single[0], "? first");
    }

    #[test]
    fn a_long_choice_wraps_and_every_wrapped_row_still_answers_it() {
        let entity = Entity::from_raw_u32(7).expect("合法实体");
        let ask = question(
            "A question title long enough to need wrapping",
            "",
            &[("yes", "an extremely long label that also needs wrapping")],
        );
        let rows = confirm_rows(&[pending(&ask, entity, 1)], 20);
        let choice_rows: Vec<&Row> = rows.iter().filter(|row| row.choice.is_some()).collect();
        assert!(choice_rows.len() > 1, "长选项应折成多行：{:?}", confirm_texts(&rows));
        for row in choice_rows {
            assert_eq!(
                row.choice,
                Some((entity, "yes".to_owned())),
                "折出来的每一段都要回答同一个选项"
            );
        }
    }

    #[test]
    fn a_zero_width_confirm_layout_does_not_panic() {
        let entity = Entity::from_raw_u32(1).expect("合法实体");
        let ask = question("Q", "B", &[("ok", "OK")]);
        let rows = confirm_rows(&[pending(&ask, entity, 1)], 0);
        assert!(!rows.is_empty(), "0 列也至少产出一行");
        assert_eq!(confirm_tile_height(rows.len()), rows.len() as f32 * 18.0);
    }

    #[test]
    fn pending_asks_in_the_world_fill_the_confirm_panel_in_fifo_order() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut world, _root) = confirm_world(80);
        // 没有待确认的 Ask：写空 vec（tile 0 行）。
        world.run_system_once(confirm_panel_rows).unwrap();
        assert!(world
            .resource::<PanelRows>()
            .get(PanelId::Confirm)
            .is_empty());

        let first = world.spawn(question("first", "", &[("ok", "OK")])).id();
        let second = world.spawn(question("second", "", &[("ok", "OK")])).id();
        world.flush();
        assert!(
            world.get::<Effect>(first).is_some(),
            "Ask 的效应应由内核登记（FIFO 序号从它来）"
        );

        world.run_system_once(confirm_panel_rows).unwrap();
        let rows = world.resource::<PanelRows>().get(PanelId::Confirm);
        let texts: Vec<String> = rows.iter().map(|keyed| text(&keyed.row.line)).collect();
        assert_eq!(texts[0], "(1/2) ? first");
        assert_eq!(texts[2], "(2/2) ? second");
        let asks: Vec<Entity> = rows
            .iter()
            .filter_map(|keyed| keyed.row.choice.as_ref().map(|(ask, _)| *ask))
            .collect();
        assert_eq!(asks, vec![first, second]);
    }

    #[test]
    fn the_confirm_tile_lists_only_the_shown_agents_asks() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut world, _root) = confirm_world(80);
        let shown = world
            .spawn(Conversation(talk::Conversation::new()))
            .id();
        let other = world.spawn_empty().id();
        world.insert_resource(ShownAgent(Some(shown)));
        let mine = world
            .spawn((question("keep mine?", "", &[("ok", "OK")]), ForAgent(shown)))
            .id();
        world.spawn((
            question("keep theirs?", "", &[("ok", "OK")]),
            ForAgent(other),
        ));
        // 无主的 Ask（app 级）和调用同一口径：没有 owner 也可见。
        let app_level = world
            .spawn(question("keep app?", "", &[("ok", "OK")]))
            .id();
        world.flush();
        assert!(
            world.get::<Effect>(mine).is_some(),
            "Ask 的效应应由内核登记"
        );

        world.run_system_once(confirm_panel_rows).unwrap();
        let rows = world.resource::<PanelRows>().get(PanelId::Confirm);
        let texts: Vec<String> = rows.iter().map(|keyed| text(&keyed.row.line)).collect();
        assert_eq!(
            texts,
            vec![
                "(1/2) ? keep mine?".to_owned(),
                "    [1] OK".to_owned(),
                "(2/2) ? keep app?".to_owned(),
                "    [1] OK".to_owned(),
            ],
            "确认 tile 只列 shown agent 的（加无主的）Ask：{texts:?}"
        );
        let asks: Vec<Entity> = rows
            .iter()
            .filter_map(|keyed| keyed.row.choice.as_ref().map(|(ask, _)| *ask))
            .collect();
        assert_eq!(asks, vec![mine, app_level], "答案不能落到别的 agent 身上");
    }

    /// 点击语义不变：确认行经面板的同一套生成路径后，行实体上挂的就是
    /// `transcript::AskChoice`（前端 `answer_ask` 观察者查的就是它），
    /// 而且从行内文字子节点往上爬也能找到——和点击处理的做法一致。
    #[test]
    fn confirm_choice_rows_carry_the_ask_choice_the_click_observer_reads() {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::text::FontSource;

        use crate::panels::PanelsPlugin;
        use crate::transcript::{AskChoice, TranscriptFont};

        let mut app = App::new();
        app.insert_resource(rig_ecs::effects::Effects::default())
            .init_resource::<PanelRows>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelAreas>()
            .add_plugins(PanelsPlugin)
            .insert_resource(TranscriptFont(FontSource::default()));
        let root = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<PanelRoots>()
            .0
            .insert(PanelId::Confirm, root);
        app.world_mut().resource_mut::<PanelAreas>().0.insert(
            root,
            crate::frame::PanelArea {
                rect: crate::frame::CellRect::new(0, 0, 80, 4),
                size: Vec2::ZERO,
                cols: 80,
                rows: 4,
                center: None,
            },
        );
        let ask = app
            .world_mut()
            .spawn(question("Keep?", "", &[("yes", "Yes")]))
            .id();
        app.world_mut().flush();
        app.world_mut().run_system_once(confirm_panel_rows).unwrap();
        app.update();

        let rows: Vec<Entity> = app
            .world()
            .get::<Children>(root)
            .map(|children| children.iter().collect())
            .unwrap_or_default();
        assert_eq!(rows.len(), 2, "一行标题 + 一行选项");
        assert!(
            app.world().get::<AskChoice>(rows[0]).is_none(),
            "标题不可点"
        );
        let choice = app
            .world()
            .get::<AskChoice>(rows[1])
            .expect("选项行必须挂 AskChoice");
        assert_eq!(choice.ask, ask);
        assert_eq!(choice.choice, "yes");

        // 点击落在行内文字上时，观察者从命中的实体沿 ChildOf 往上爬。
        let text_child = app
            .world()
            .get::<Children>(rows[1])
            .and_then(|children| children.iter().next())
            .expect("选项行有文字子节点");
        let mut hit = text_child;
        let mut found = None;
        loop {
            if let Some(choice) = app.world().get::<AskChoice>(hit) {
                found = Some((choice.ask, choice.choice.clone()));
                break;
            }
            match app.world().get::<ChildOf>(hit) {
                Some(child_of) => hit = child_of.parent(),
                None => break,
            }
        }
        assert_eq!(found, Some((ask, "yes".to_owned())));
    }

    // -----------------------------------------------------------------------
    // 实时内容（没有自己管线的工作区）
    // -----------------------------------------------------------------------

    /// 一行显示宽度（全角算 2），断言折行用。
    fn line_width(line: &Line<'static>) -> usize {
        use unicode_width::UnicodeWidthStr;
        line.spans
            .iter()
            .map(|span| span.content.as_ref().width())
            .sum()
    }

    /// 一组面板行的纯文本。
    fn rows_text(rows: &[KeyedRow]) -> String {
        rows.iter()
            .map(|keyed| text(&keyed.row.line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 一个只有内核数据的 world：效果 id 递增（调用/Ask 的 FIFO 要用）。
    fn live_world() -> World {
        let mut world = World::new();
        world.insert_resource(rig_ecs::effects::Effects::default());
        world
    }

    /// 一个带自己调用 id 的工具调用（两个 agent 各一个，不能共用一个 id）。
    fn named_call(id: &str, name: &str) -> ToolCall {
        let name = ToolName::new(name).expect("工具名非空");
        ToolCall::new(
            CallId::from_wire(id),
            ToolFunction::new(name, serde_json::json!({ "q": id })),
        )
    }

    /// 给 `agent` 开一个真 turn，把流式正文/推理挂在它的模型调用上。
    ///
    /// 走的是内核的真实形状：`TurnOf(agent)` 带出 `ActiveTurn`，
    /// `CallOf(turn)` 带出 `Calls`，`Partial` 在调用上。
    fn stream_into(world: &mut World, agent: Entity, text: &str, reasoning: &str) {
        let turn = world.spawn(TurnOf(agent)).id();
        world.spawn((
            CallOf(turn),
            Partial {
                text: text.to_owned(),
                reasoning: reasoning.to_owned(),
                calls: Vec::new(),
            },
        ));
        world.flush();
    }

    #[test]
    fn dialogue_batch_draws_the_whole_conversation_from_one_pure_function() {
        let (conversation, tool, note) = conversation();
        let conversation = Conversation(conversation);
        let agent = Entity::from_raw_u32(77).expect("合法实体");
        let (batch, texts) = dialogue_batch(&conversation, agent, 80, &Theme::default());

        assert_eq!(batch.agent, Some(agent), "批要指认它属于哪个 agent");
        assert_eq!(batch.width, 80, "宽度原样交给 look");

        // 用户请求 + assistant 正文都进文字块（和聚焦条线同一个帧函数）。
        let user = texts
            .iter()
            .find(|(id, _)| *id == TextId::Message { at: 0, block: 0 })
            .map(|(_, lines)| lines_text(lines))
            .expect("用户消息应进文字块");
        assert!(user.contains("› Why tabs?"), "用户文字带 › 前缀：{user}");
        let reply = texts
            .iter()
            .find(|(id, _)| *id == TextId::Message { at: 1, block: 0 })
            .map(|(_, lines)| lines_text(lines))
            .expect("assistant 正文应进文字块");
        assert!(reply.contains("Tabs are the style."), "正文来自对话：{reply}");

        // tool call 走 `Tool(m, b)`，默认行里有工具名。
        let request = batch
            .requests
            .get(&RenderKey::Tool(1, 1))
            .expect("对话里的调用要有请求");
        let drawn = request
            .output
            .lines
            .iter()
            .map(|line| line.line().to_string())
            .collect::<String>();
        assert!(drawn.contains("search"), "默认行里要有工具名：{drawn}");
        assert_eq!(tool.id, CallId::from_wire("call_test"));

        // note（aside）也在：落点和文字块都对得上真管线。
        assert!(
            batch.requests.contains_key(&RenderKey::Aside(note)),
            "note 要画成 aside 请求"
        );
    }

    #[test]
    fn live_additions_keep_each_agents_calls_and_stream_to_itself() {
        let mut world = live_world();
        let alpha = world.spawn(AgentId("alpha".into())).id();
        let beta = world.spawn(AgentId("beta".into())).id();
        // 真 `Running`（挂一个永不结束的任务）：在飞调用画成 running。
        let pool = bevy::tasks::TaskPool::new();
        let wake = Wake::new(|| {});
        let running = Running::spawn_into::<Done<()>, ()>(&pool, &wake, std::future::pending());
        world.spawn((
            ToolRequest {
                call: named_call("call_alpha", "search_alpha"),
            },
            ForAgent(alpha),
            running,
        ));
        world.spawn((
            ToolRequest {
                call: named_call("call_beta", "search_beta"),
            },
            ForAgent(beta),
        ));
        // 正文故意比 24 列宽：要按宽度折行。
        stream_into(
            &mut world,
            alpha,
            "alpha streams a reply much wider than the strip",
            "",
        );
        stream_into(&mut world, beta, "beta streams its own reply", "");
        world.flush();

        let rows = live_additions(&mut world, alpha, 24);
        let text = rows_text(&rows);
        assert!(text.contains("search_alpha"), "本 agent 的在飞调用要出现：{text:?}");
        assert!(!text.contains("search_beta"), "别的 agent 的调用不能串台：{text:?}");
        assert!(text.contains("alpha streams"), "本 agent 的流式正文要出现：{text:?}");
        assert!(!text.contains("beta streams"), "别的 agent 的正文不能串台：{text:?}");
        assert!(text.contains("running"), "在飞调用应画成 running：{text:?}");
        let widest = rows
            .iter()
            .map(|keyed| line_width(&keyed.row.line))
            .max()
            .unwrap_or(0);
        assert!(
            widest <= 24,
            "流式正文要按宽度折行（最宽 {widest} 列）：{text:?}"
        );
    }

    #[test]
    fn live_additions_add_reasoning_and_only_this_agents_asks() {
        let mut world = live_world();
        let alpha = world.spawn(AgentId("alpha".into())).id();
        let beta = world.spawn(AgentId("beta".into())).id();
        stream_into(&mut world, alpha, "alpha body", "alpha thinks");
        let alpha_ask = world
            .spawn((
                question("keep alpha?", "", &[("yes", "Yes")]),
                ForAgent(alpha),
            ))
            .id();
        world.spawn((question("keep beta?", "", &[("yes", "Yes")]), ForAgent(beta)));
        world.flush();

        let rows = live_additions(&mut world, alpha, 60);
        let text = rows_text(&rows);
        assert!(text.contains("alpha thinks"), "推理要出现：{text:?}");
        assert!(text.contains("alpha body"), "正文要出现：{text:?}");
        assert!(text.contains("keep alpha?"), "本 agent 的 Ask 要出现：{text:?}");
        assert!(
            !text.contains("keep beta?"),
            "别的 agent 的 Ask 不能串台：{text:?}"
        );
        // 选项行原样带 `Row::choice`：点击回答的语义和确认面板一致。
        let choices: Vec<(Entity, String)> = rows
            .iter()
            .filter_map(|keyed| keyed.row.choice.clone())
            .collect();
        assert_eq!(choices, vec![(alpha_ask, "yes".to_owned())]);
    }

    #[test]
    fn live_additions_do_not_draw_a_call_the_conversation_already_has() {
        let mut world = live_world();
        let agent = world.spawn(AgentId("a".into())).id();
        let known = named_call("call_known", "search_known");
        let mut conversation = talk::Conversation::new();
        conversation.push(Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::ToolCall(known.clone()),
        ])));
        world.entity_mut(agent).insert(Conversation(conversation));
        world.spawn((
            ToolRequest {
                call: known.clone(),
            },
            ForAgent(agent),
        ));
        world.flush();

        let text = rows_text(&live_additions(&mut world, agent, 80));
        assert!(
            !text.contains("search_known"),
            "对话里已经有的调用不重复画一份：{text:?}"
        );
    }

    #[test]
    fn live_additions_let_go_of_a_reply_the_conversation_already_has() {
        let mut world = live_world();
        let agent = world.spawn(AgentId("a".into())).id();
        // 回复已经提交进对话，`Partial` 还没消失（提交和消失之间的一帧）。
        let mut conversation = talk::Conversation::new();
        conversation.push(Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::text("already in the dialogue"),
        ])));
        world.entity_mut(agent).insert(Conversation(conversation));
        stream_into(&mut world, agent, "already in the dialogue", "some thinking");

        let text = rows_text(&live_additions(&mut world, agent, 80));
        assert!(
            !text.contains("already in the dialogue"),
            "回复落进对话后流式块要让位（别画两份）：{text:?}"
        );
    }

    #[test]
    fn live_additions_only_draw_the_root_so_a_nested_call_is_not_duplicated() {
        let mut world = live_world();
        let alpha = world.spawn(AgentId("alpha".into())).id();
        let beta = world.spawn(AgentId("beta".into())).id();
        let parent = world
            .spawn((
                ToolRequest {
                    call: named_call("call_parent", "parent_tool"),
                },
                ForAgent(alpha),
            ))
            .id();
        world.flush();
        let parent_effect = world
            .get::<Effect>(parent)
            .expect("活调用要有自己的效应")
            .id();
        // 两个子调用：一个还挂在 alpha 名下，一个是子 agent 的；都挂在同一个
        // 父调用下面。它们不该在条带里各自再占一张卡。
        for (id, name, owner) in [
            ("call_child", "child_tool", alpha),
            ("call_beta_child", "beta_child_tool", beta),
        ] {
            world.spawn((
                ToolRequest {
                    call: named_call(id, name),
                },
                Effect::with_parent_id(Some(parent_effect)),
                ForAgent(owner),
            ));
        }
        // 另一个 agent 自己的根调用：不能串台。
        world.spawn((
            ToolRequest {
                call: named_call("call_beta", "beta_tool"),
            },
            ForAgent(beta),
        ));
        world.flush();

        let rows = live_additions(&mut world, alpha, 80);
        let text = rows_text(&rows);
        assert!(text.contains("parent_tool"), "本 agent 的根要画：{text:?}");
        assert!(
            !text.contains("child_tool"),
            "父卡片下面的调用不单独占一张卡：{text:?}"
        );
        assert!(
            !text.contains("beta_child_tool"),
            "子 agent 的调用也不单独占一张卡：{text:?}"
        );
        assert!(!text.contains("beta_tool"), "别的 agent 的调用不能串台：{text:?}");
    }

    #[test]
    fn live_additions_treat_a_call_without_an_owner_like_the_focused_pipeline() {
        // 没有 `ForAgent` 的调用（app 级效应 / 老记录）：聚焦建批按"无主的根"
        // 处理、可见；非聚焦条带也必须看得见，不能只有一条条带显示。
        let (mut app, shown) = focused_app();
        let other = app.world_mut().spawn_empty().id();
        app.world_mut().spawn(ToolRequest {
            call: named_call("call_unowned", "unowned_tool"),
        });
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_other", "other_tool"),
            },
            ForAgent(other),
        ));
        rebuild_now(&mut app);
        let cards = streaming_cards(app.world().resource::<RenderBatch>());
        let names: Vec<&str> = cards.iter().map(|(_, name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec!["unowned_tool"],
            "聚焦条带里无主调用可见：{cards:?}"
        );

        let text = rows_text(&live_additions(app.world_mut(), shown, 80));
        assert!(
            text.contains("unowned_tool"),
            "非聚焦条带也要看到同一条无主调用：{text:?}"
        );
        assert!(
            !text.contains("other_tool"),
            "别的 agent 的调用仍然不可见：{text:?}"
        );
    }

    #[test]
    fn live_additions_keep_a_settled_call_until_the_conversation_has_it() {
        let mut world = live_world();
        world.init_resource::<CallHistory>();
        let agent = world.spawn(AgentId("a".into())).id();
        let call = named_call("call_settling", "settling_tool");
        let entity = world
            .spawn((ToolRequest { call: call.clone() }, ForAgent(agent)))
            .id();
        world.flush();
        let effect = world.get::<Effect>(entity).expect("效应").id();

        // 在飞：一张卡。
        let live_rows = live_additions(&mut world, agent, 80);
        let live_text = rows_text(&live_rows);
        assert!(live_text.contains("settling_tool"), "在飞时要画：{live_text:?}");
        let live_first = live_rows.first().expect("在飞时至少一行").key;

        // 结算了、对话还没提交：卡不能消失，key 也不能变（原地换状态，不闪）。
        world.entity_mut(entity).insert(Settled);
        world.resource_mut::<CallHistory>().0.push(settled_fact(
            effect.as_u64(),
            None,
            Some(agent),
            7,
            call.clone(),
        ));
        let settled_rows = live_additions(&mut world, agent, 80);
        let settled_text = rows_text(&settled_rows);
        assert!(
            settled_text.contains("settling_tool"),
            "结算到提交之间不能先消失：{settled_text:?}"
        );
        assert!(
            settled_text.contains("settling_tool result"),
            "结算后要带上结果（状态是 done）：{settled_text:?}"
        );
        assert_eq!(
            settled_rows.first().map(|keyed| keyed.key),
            Some(live_first),
            "同一张卡结算前后第一行 key 不变"
        );

        // 对话提交后：由对话那份画，实时块让位。
        let mut conversation = talk::Conversation::new();
        conversation.push(Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::ToolCall(call.clone()),
        ])));
        world.entity_mut(agent).insert(Conversation(conversation));
        let text = rows_text(&live_additions(&mut world, agent, 80));
        assert!(
            !text.contains("settling_tool"),
            "落进对话后实时块不再重复画：{text:?}"
        );
    }

    // -----------------------------------------------------------------------
    // 工具卡的事实（`ToolRender::facts`）：退出码 / 截断 / 进度 / 调用树
    // -----------------------------------------------------------------------

    /// 结果那条 entry 里的 `ResultFacts`：`ToolRender::facts` 在对话路径上的
    /// 真来源（上游 `transcript::message_lines` 的 `next.facts.of(calls)`）。
    fn result_facts(
        reply: talk::MessageId,
        call: &CallId,
        context: &ToolContext,
    ) -> talk::ResultFacts {
        let facts = context
            .result_context()
            .with_at(CallRef { reply, index: 0 });
        [(call.clone(), facts)].into_iter().collect()
    }

    /// 批里某张工具卡的 `ToolRender`。
    fn tool_render<'a>(batch: &'a RenderBatch, key: &RenderKey) -> &'a ToolRender {
        match &batch
            .requests
            .get(key)
            .unwrap_or_else(|| panic!("批里没有 {key:?}"))
            .input
        {
            RenderInput::Tool(render) => render,
            _ => panic!("{key:?} 不是工具卡"),
        }
    }

    /// 一个带主题的 `ToolRender`（业务 look 的测试用）。
    fn render_of(
        name: &str,
        state: CallState,
        result: Option<ToolResult>,
        theme: RTheme,
    ) -> ToolRender {
        ToolRender {
            call: call(name, serde_json::json!({ "query": "tabs" })),
            result,
            state,
            theme,
            facts: None,
            took: None,
            output: None,
            children: Vec::new(),
            item: None,
        }
    }

    /// 一条构造好的工具结果（`message::ToolResult`，look 读的形状）。
    fn message_result(name: &str, text: &str, is_error: bool) -> ToolResult {
        ToolResult {
            call: CallId::from_wire("call_test"),
            name: ToolName::new(name).expect("工具名非空"),
            content: vec![ToolResultContent::Text(Text::new(text))],
            is_error,
        }
    }

    /// 一行（含 span）没有前景/底色，只有修饰符（`NO_COLOR`）。
    fn assert_no_colors(line: &Line<'static>, what: &str) {
        assert_eq!(line.style.fg, None, "{what} 还有前景色：{line:?}");
        assert_eq!(line.style.bg, None, "{what} 还有底色：{line:?}");
        for span in &line.spans {
            assert_eq!(
                span.style.fg, None,
                "{what} 的 span {:?} 还有前景色",
                span.content
            );
            assert_eq!(
                span.style.bg, None,
                "{what} 的 span {:?} 还有底色",
                span.content
            );
        }
    }

    #[test]
    fn tool_card_headers_take_the_call_bands_from_the_app_theme() {
        // 三个 band 换成一眼能认出的值：header 必须原样拿到它们，说明走的是
        // `upstream_theme(我们的 Theme)`，不是上游 `RTheme::default()`。
        let mut theme = Theme::default();
        let running = RColor::Rgb(11, 22, 33);
        let done = RColor::Rgb(21, 22, 23);
        let failed = RColor::Rgb(31, 32, 33);
        theme.call_running = Style::new().bg(running);
        theme.call_done = Style::new().bg(done);
        theme.call_failed = Style::new().bg(failed);

        let (mut app, shown) = focused_app();
        app.insert_resource(theme);
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_running", "search_running"),
            },
            ForAgent(shown),
            // `Has<Started>` 就够：还没交任务句柄也算 running。
            Started {
                at: std::time::SystemTime::now(),
            },
        ));
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_done", "search_done"),
            },
            Outcome::from(rig_core::tool::ToolResult::success(
                rig_core::tool::ToolOutput::text("ok"),
            )),
            ForAgent(shown),
        ));
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_failed", "search_failed"),
            },
            // 错误结果：header 走 `call_failed` 色带（不是 Done 的绿）。
            Outcome::refused("not allowed"),
            ForAgent(shown),
        ));
        rebuild_now(&mut app);

        let batch = app.world().resource::<RenderBatch>();
        let band_of = |name: &str| {
            batch.requests.values().find_map(|request| match &request.input {
                RenderInput::Tool(render)
                    if render.call.function.name.as_str() == name =>
                {
                    request
                        .output
                        .lines
                        .first()
                        .map(|line| line.line().style.bg)
                }
                _ => None,
            })
        };
        assert_eq!(band_of("search_running"), Some(Some(running)));
        assert_eq!(band_of("search_done"), Some(Some(done)));
        assert_eq!(band_of("search_failed"), Some(Some(failed)));
    }

    #[test]
    fn custom_look_headers_take_their_state_band_from_the_theme() {
        let theme = upstream_theme(&Theme::default());

        // search：running 的 header 用 call_running；状态词不再有写死的 dim 色，
        // 底色由整行 band 提供。
        let running = render_of("search_products", CallState::Running(None), None, theme);
        let lines = search_product_lines(&running);
        assert_eq!(lines[0].line().style.bg, Some(theme.call_running));
        let state = lines[0]
            .line()
            .spans
            .iter()
            .find(|span| span.content.contains("(running)"))
            .expect("状态词");
        assert_eq!(state.style.fg, None, "状态词不再写死颜色");
        assert!(state.style.add_modifier.contains(Modifier::ITALIC));

        // order：done → call_done；held → call_waiting；failed → call_failed。
        let done = render_of(
            "create_order",
            CallState::Done,
            Some(message_result(
                "create_order",
                r#"{"order_id":"o-1","total":9,"status":"placed"}"#,
                false,
            )),
            theme,
        );
        let lines = order_lines(&done);
        assert_eq!(lines[0].line().style.bg, Some(theme.call_done));
        let tail = lines[0]
            .line()
            .spans
            .iter()
            .find(|span| span.content.contains("order placed"))
            .expect("done 的状态词");
        assert_eq!(tail.style.fg, None, "状态词不再写死颜色");

        let held = render_of(
            "create_order",
            CallState::Held(vec!["gate".to_owned()]),
            None,
            theme,
        );
        assert_eq!(
            order_lines(&held)[0].line().style.bg,
            Some(theme.call_waiting)
        );

        let failed = render_of(
            "search_products",
            CallState::Done,
            Some(message_result("search_products", "boom", true)),
            theme,
        );
        assert_eq!(
            search_product_lines(&failed)[0].line().style.bg,
            Some(theme.call_failed)
        );
    }

    #[test]
    fn the_default_theme_keeps_the_same_key_colors() {
        // 前景 token（旧可见色）不变；17 个 band 的默认值也不变（theme.rs 钉过，
        // 这里再钉一遍 batch 真正消费的那几个）。
        let ours = Theme::default();
        assert_eq!(ours.fg.fg, Some(RColor::Rgb(210, 216, 228)));
        assert_eq!(ours.dim.fg, Some(RColor::Rgb(120, 128, 145)));
        let theme = upstream_theme(&ours);
        assert_eq!(theme.user, RColor::Rgb(18, 38, 49));
        assert_eq!(theme.tool, RColor::Rgb(31, 37, 49));
        assert_eq!(theme.call_running, RColor::Rgb(53, 58, 33));
        assert_eq!(theme.call_done, RColor::Rgb(18, 36, 34));
        assert_eq!(theme.call_failed, RColor::Rgb(76, 20, 29));
        // 默认主题下自定义 look 的 header 底色 = 上面这些默认 band。
        let running = render_of("search_products", CallState::Running(None), None, theme);
        assert_eq!(
            search_product_lines(&running)[0].line().style.bg,
            Some(RColor::Rgb(53, 58, 33))
        );
    }

    #[test]
    fn from_env_only_reads_the_no_color_variable() {
        // 不改进程环境（并行测试会串）：只读当前值，断言 `from_env` 的判定
        // 和它一致。`NO_COLOR` 的解析本身在 `theme::tests` 里钉过。
        let expected = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        assert_eq!(Theme::from_env().plain, expected);
    }

    #[test]
    fn a_plain_theme_draws_no_foreground_or_background_colors() {
        // `NO_COLOR` 的等价入口：`Theme::from_env` 设了变量时返回的就是它
        // （`theme::tests::no_color_is_set_only_when_nonempty` 钉了判定）。
        let plain = Theme::default().without_color();
        assert!(plain.plain);

        // rich 的 plain 路径（markdown 的标题/引用/代码块底色也清掉）。
        let rows = crate::rich::markdown_rows("## 标题\n\n正文 `inline`\n\n```rust\nfn main() {}\n```\n", 40, &plain);
        assert!(!rows.is_empty());
        for row in &rows {
            assert_no_colors(&row.line, "rich 行");
        }

        // batch 自己的路径：对话（用户/assistant/tool/note）出来的行也不能有颜色。
        let (conversation, _, _) = conversation();
        let conversation = Conversation(conversation);
        let (batch, texts) = dialogue_batch(&conversation, Entity::PLACEHOLDER, 80, &plain);
        let mut lines: Vec<Line<'static>> = Vec::new();
        for request in batch.requests.values() {
            lines.extend(request.output.lines.iter().map(|line| line.line().clone()));
            for (_, decoration) in &request.output.decorations {
                lines.extend(decoration.iter().cloned());
            }
        }
        for (_, text) in &texts {
            lines.extend(text.iter().cloned());
        }
        assert!(!lines.is_empty());
        for line in &lines {
            assert_no_colors(line, "batch 行");
        }
    }

    #[test]
    fn a_plain_theme_strips_colors_after_the_real_looks() {
        // 生产链路：rebuild -> looks -> collect；look 自己写的颜色（状态带）
        // 要在 `run_looks` 的 plain 清理里被清掉。
        let (mut app, shown) = focused_app();
        app.insert_resource(Theme::default().without_color());
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_plain", "search"),
            },
            ForAgent(shown),
            Started {
                at: std::time::SystemTime::now(),
            },
        ));
        app.update();
        app.update();
        let rows = app.world().resource::<TranscriptRows>();
        assert!(!rows.0.is_empty(), "活调用应该产出至少一行");
        for keyed in &rows.0 {
            assert_no_colors(&keyed.row.line, "真管线行");
        }
    }

    #[test]
    fn a_nonzero_exit_in_the_result_facts_reaches_the_header() {
        use rig_core::tool::ExitStatus;

        let mut conversation = talk::Conversation::new();
        let tool = named_call("call_exit", "shell");
        let reply = conversation.push(Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::ToolCall(tool.clone()),
        ])));
        let mut context = ToolContext::new();
        context
            .insert_result(ExitStatus::Exited(3))
            .expect("退出码要能发布");
        conversation.push(
            talk::Entry::new(Message::User {
                content: vec![UserContent::ToolResult(ToolResult {
                    call: tool.id.clone(),
                    name: tool.function.name.clone(),
                    content: vec![ToolResultContent::Text(Text::new("boom"))],
                    is_error: false,
                })],
            })
            .with_facts(result_facts(reply, &tool.id, &context)),
        );

        let frame = conversation_frame(
            &conversation,
            &CallHistory::default(),
            &CallTree::default(),
            &HashMap::new(),
            &RTheme::default(),
            80,
        );
        let render = frame
            .requests
            .iter()
            .find_map(|(key, input)| match (key, input) {
                (RenderKey::Tool(0, 0), RenderInput::Tool(render)) => Some(render),
                _ => None,
            })
            .expect("调用卡");
        assert_eq!(
            render
                .facts
                .as_ref()
                .and_then(|facts| facts.get::<ExitStatus>().ok().flatten()),
            Some(ExitStatus::Exited(3)),
            "facts 来自结果那条 entry 的 ResultFacts：{:?}",
            render.facts
        );
        let lines = render.default_lines();
        let header = text(lines[0].line());
        assert!(header.contains("exit 3"), "header 要说出退出码：{header:?}");
        assert_eq!(
            lines[0].line().style.bg,
            Some(RTheme::default().call_failed),
            "非 0 退出码要走失败色带：{header:?}"
        );
    }

    #[test]
    fn a_settled_call_keeps_the_facts_it_published_for_its_card() {
        use bevy::ecs::system::RunSystemOnce;
        use rig_core::tool::ExitStatus;

        let (mut app, shown) = focused_app();
        let call = named_call("call_published", "shell");
        let mut context = ToolContext::new();
        context
            .insert_result(ExitStatus::Exited(2))
            .expect("退出码要能发布");
        app.world_mut().spawn((
            ToolRequest { call: call.clone() },
            // 结果本身不是错误：失败色带只能来自 facts 里的退出码。
            Outcome::from(rig_core::tool::ToolResult::success(
                rig_core::tool::ToolOutput::text("printed"),
            )),
            Published(context.result_context()),
            ForAgent(shown),
        ));
        app.world_mut()
            .run_system_once(record_calls)
            .expect("record_calls 要能跑");

        let facts = app
            .world()
            .resource::<CallHistory>()
            .0
            .iter()
            .find(|fact| fact.call.id == call.id)
            .expect("结算调用进了历史")
            .facts
            .clone();
        assert_eq!(
            facts
                .as_ref()
                .and_then(|facts| facts.get::<ExitStatus>().ok().flatten()),
            Some(ExitStatus::Exited(2)),
            "效应上的 `Published` 要抄进 CallFact：{facts:?}"
        );

        rebuild_now(&mut app);
        let batch = app.world().resource::<RenderBatch>();
        let key = batch
            .requests
            .keys()
            .find(|key| match &batch.requests[*key].input {
                RenderInput::Tool(render) => render.call.id == call.id,
                _ => false,
            })
            .expect("结算到提交之间的卡还在批里")
            .clone();
        let render = tool_render(batch, &key);
        assert_eq!(render.state, CallState::Done);
        let lines = &batch
            .requests
            .get(&key)
            .expect("请求在批里")
            .output
            .lines;
        let header = text(lines[0].line());
        assert!(header.contains("exit 2"), "header 要说出退出码：{header:?}");
        assert_eq!(
            lines[0].line().style.bg,
            Some(upstream_theme(&Theme::default()).call_failed),
            "非 0 退出码要走失败色带：{header:?}"
        );
    }

    #[test]
    fn a_running_call_in_the_conversation_shows_its_progress_from_tool_progress() {
        let (mut app, shown) = focused_app();
        let call = named_call("call_progress", "build");
        let mut conversation = talk::Conversation::new();
        conversation.push(Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::ToolCall(call.clone()),
        ])));
        app.world_mut()
            .entity_mut(shown)
            .insert(Conversation(conversation));
        app.world_mut().spawn((
            ToolRequest { call: call.clone() },
            ForAgent(shown),
            // 上游 `CallStates::state` 的判据是 `Has<Started>`：任务句柄还没
            // 交出去也算 started。进度行本身来自 `ToolProgress`，不是编的字。
            Started {
                at: std::time::SystemTime::now(),
            },
            ToolProgress("12 of 40 files".to_owned()),
        ));
        rebuild_now(&mut app);

        let batch = app.world().resource::<RenderBatch>();
        let key = RenderKey::Tool(0, 0);
        let render = tool_render(batch, &key);
        assert_eq!(
            render.state,
            CallState::Running(Some("12 of 40 files".to_owned())),
            "对话里还在跑的调用要拿活效应的状态（含进度）"
        );
        assert!(
            render.facts.is_none(),
            "答复之前没有 `Published`，facts 没有来源，只能是 None"
        );
        assert!(
            render.took.is_none(),
            "还没结算，took 没有来源（上游也是结算后才有）"
        );
        let lines = &batch.requests.get(&key).expect("调用卡在批里").output.lines;
        let progress = lines
            .iter()
            .find(|line| text(line.line()).contains("12 of 40 files"))
            .expect("进度行要出现");
        assert_eq!(
            progress.line().style.fg,
            Some(RColor::Yellow),
            "进度行是 `progress_lines` 的黄色"
        );
        assert!(
            !batch
                .requests
                .keys()
                .any(|key| matches!(key, RenderKey::Streaming(_))),
            "对话里已经有的调用不该再占一张 Streaming 卡：{:?}",
            batch.requests.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_cut_result_draws_its_marker_from_the_result_facts() {
        use rig_core::tool::{KeptEnd, OutputCut};

        let marker = "[10 earlier lines cut; all of it in /tmp/full.log]";
        let mut conversation = talk::Conversation::new();
        let tool = named_call("call_cut", "shell");
        let reply = conversation.push(Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::ToolCall(tool.clone()),
        ])));
        let mut context = ToolContext::new();
        context
            .insert_result(OutputCut {
                kept: KeptEnd::Start,
                kept_lines: 2,
                total_lines: 12,
                spill: Some("/tmp/full.log".to_owned()),
            })
            .expect("截断事实要能发布");
        conversation.push(
            talk::Entry::new(Message::User {
                content: vec![UserContent::ToolResult(ToolResult {
                    call: tool.id.clone(),
                    name: tool.function.name.clone(),
                    content: vec![ToolResultContent::Text(Text::new(format!(
                        "first\nsecond\n{marker}"
                    )))],
                    is_error: false,
                })],
            })
            .with_facts(result_facts(reply, &tool.id, &context)),
        );

        let frame = conversation_frame(
            &conversation,
            &CallHistory::default(),
            &CallTree::default(),
            &HashMap::new(),
            &RTheme::default(),
            80,
        );
        let render = frame
            .requests
            .iter()
            .find_map(|(key, input)| match (key, input) {
                (RenderKey::Tool(0, 0), RenderInput::Tool(render)) => Some(render),
                _ => None,
            })
            .expect("调用卡");
        let cut = render
            .facts
            .as_ref()
            .and_then(|facts| facts.get::<OutputCut>().ok().flatten())
            .expect("OutputCut 从 ResultFacts 来");
        assert_eq!(cut.kept, KeptEnd::Start);
        assert_eq!(cut.cut_lines(), 10);
        let lines: Vec<String> = render
            .default_lines()
            .iter()
            .map(|line| text(line.line()))
            .collect();
        let at = lines
            .iter()
            .position(|line| line.contains(marker))
            .unwrap_or_else(|| panic!("截断标记要出现：{lines:?}"));
        assert_eq!(
            at,
            lines.len() - 1,
            "`KeptEnd::Start` 的标记排在保留下来的行之后：{lines:?}"
        );
        assert_eq!(
            lines.iter().filter(|line| line.contains(marker)).count(),
            1,
            "标记只画一次（facts 认得它，不会重复/吞掉）：{lines:?}"
        );
    }

    #[test]
    fn a_call_tree_keeps_upstream_depth_through_non_call_effects() {
        let child = |name: &str, state: CallState| ChildCall {
            depth: 0,
            call: Some(ToolFunction::new(
                ToolName::new(name).expect("工具名非空"),
                serde_json::json!({}),
            )),
            name: name.to_owned(),
            agent: None,
            state,
            failed: false,
            answer: None,
        };
        let mut tree = CallTree::default();
        tree.add(
            EffectId::from_raw(10),
            None,
            child("parent", CallState::Done),
        );
        tree.add(
            EffectId::from_raw(11),
            Some(EffectId::from_raw(10)),
            child("child", CallState::Done),
        );
        tree.add(
            EffectId::from_raw(12),
            Some(EffectId::from_raw(11)),
            child("grandchild", CallState::Done),
        );
        // 不是调用的效应 13（比如模型调用）挂在 10 下面、14 挂在 13 下面：
        // 上游 `CallFacts::add_below` 让调用从它下面穿过，depth 不变。
        tree.link(EffectId::from_raw(13), Some(EffectId::from_raw(10)));
        tree.add(
            EffectId::from_raw(14),
            Some(EffectId::from_raw(13)),
            child(
                "through",
                CallState::Running(Some("half way".to_owned())),
            ),
        );

        let found: Vec<(String, usize, CallState)> = tree
            .children_of(EffectId::from_raw(10))
            .into_iter()
            .map(|child| (child.name, child.depth, child.state))
            .collect();
        assert_eq!(
            found,
            vec![
                ("child".to_owned(), 0, CallState::Done),
                ("grandchild".to_owned(), 1, CallState::Done),
                (
                    "through".to_owned(),
                    0,
                    CallState::Running(Some("half way".to_owned()))
                ),
            ],
            "直接子调用 depth 0、孙调用 1，穿过的非调用效应不改 depth"
        );
    }

    #[test]
    fn a_non_call_effect_between_the_spawn_and_a_subagent_call_keeps_the_chain() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut app, shown) = focused_app();
        let helper = app.world_mut().spawn_empty().id();
        // 父卡片：发起子 agent 的调用（结算过、还没进对话）。
        app.world_mut()
            .resource_mut::<CallHistory>()
            .0
            .push(settled_fact(
                10,
                None,
                Some(shown),
                50,
                named_call("call_spawn", "spawn_agent"),
            ));
        // 中间那层：模型调用（不是工具调用，没有卡片）。真实内核里它的
        // `Effect::parent` 来自 agent 的 `EffectParent`，也就是 spawn 调用。
        let model = app
            .world_mut()
            .spawn(Effect::with_parent_id(Some(EffectId::from_raw(10))))
            .id();
        let model_effect = app
            .world()
            .get::<Effect>(model)
            .expect("模型调用要有自己的效应")
            .id();
        // 子 agent 的调用挂在模型调用下面（`spawn_reply_calls` 的真实形状：
        // `Effect::with_parent_id(Some(reply))`，`reply` 就是模型调用效应）。
        app.world_mut().spawn((
            ToolRequest {
                call: named_call("call_child", "subagent_tool"),
            },
            Effect::with_parent_id(Some(model_effect)),
            ForAgent(helper),
        ));
        app.world_mut()
            .run_system_once(record_effect_parents)
            .expect("父关系要能抄下来");
        assert_eq!(
            app.world()
                .resource::<EffectParents>()
                .0
                .get(&model_effect)
                .copied()
                .flatten(),
            Some(EffectId::from_raw(10)),
            "模型调用要挂在 spawn 调用下（Review 抄的 `Effect::parent`）"
        );

        rebuild_now(&mut app);
        let batch = app.world().resource::<RenderBatch>();
        let parent = batch
            .requests
            .values()
            .find_map(|request| match &request.input {
                RenderInput::Tool(render) if render.name() == "spawn_agent" => Some(render),
                _ => None,
            })
            .expect("父卡片");
        let children: Vec<(String, usize, CallState)> = parent
            .children
            .iter()
            .map(|child| (child.name.clone(), child.depth, child.state.clone()))
            .collect();
        assert_eq!(
            children,
            vec![("subagent_tool".to_owned(), 0, CallState::Waiting)],
            "子 agent 的调用要穿过没有卡片的模型调用，挂到 spawn 卡片下（depth 0）"
        );
        assert_eq!(
            streaming_cards(batch).len(),
            1,
            "子 agent 的调用只出现在父卡片的 children 里，不单独占一张卡：{:?}",
            streaming_cards(batch)
        );
    }

    #[test]
    fn a_started_call_is_running_before_its_task_handle_exists() {
        let progress = ToolProgress("half way".to_owned());
        assert_eq!(
            live_state(None, true, None, None, None, |_| None),
            CallState::Running(None),
            "`Has<Started>` 就够（上游 `CallStates::state` 的判据）"
        );
        assert_eq!(
            live_state(None, true, None, None, Some(&progress), |_| None),
            CallState::Running(Some("half way".to_owned())),
            "进度行来自 `ToolProgress`"
        );
        assert_eq!(
            live_state(None, false, None, None, None, |_| None),
            CallState::Waiting,
            "没开始、没答复：queued"
        );
    }
}
