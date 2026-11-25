//! 工作区 = **一个真 agent 的对话**，多个工作区 = 同一个 app 会话里的多个 agent。
//!
//! 设计见 `docs/status/WORKSPACE_MODEL.md`：**工作区内部**是 niri 分裂树，
//! **工作区之间**是绝对空间坐标。这个模块负责把"世界层的工作区"接到
//! rig-ecs 的真实 agent 上，拖拽/整理/聚焦仍由 `rig_niri::workspaces` 提供。
//!
//! ```sh
//! RF_WORKSPACES=3 cargo run      # 竖直摆 3 个工作区，每个一个真 agent
//! ```
//!
//! 默认（不设或设 1）只跑单工作区，行为和以前一样：第 0 个工作区收编默认屏幕。
//!
//! # 一个"会话"在这个 app 里到底是什么
//!
//! 上游把会话分成两层，本项目只能照着这个事实接：
//!
//! 1. **磁盘会话 = [`rig_harness::prelude::SessionPaths`]，每个 app 一个。**
//!    `SessionPaths` 是 `Resource`，由 `rig-harness` 的 `SessionPlugin` 在启动时
//!    插入一次（`reference/rig-harness-src/src/host/session.rs`：`session_paths`
//!    + `app.insert_resource(SessionPaths(..))`）；会话日志 [`rig_ecs::session_log::SessionLog`]
//!    同样是**一个** `Resource`（`rig-ecs/src/lib.rs` 的 `init_resource::<SessionLog>()`），
//!    由 `open_session(world, store)` 打开**一个** store 后插入
//!    （`rig-ecs/src/session_log.rs`）。`SessionPaths` 指向哪个目录，这个 app 的
//!    `/resume`、日志、`RIG_HOME` 就都认哪一个；没有"一个 app 里再开第二个
//!    `SessionPaths`"的公开入口（会撞上同一个 `SessionLog`/store）。
//!
//! 2. **agent 会话 = [`rig_ecs::prelude::Agent`] + [`rig_ecs::prelude::Conversation`]，
//!    可以 N 个。** 两者都是组件：`spawn_root(world, AgentId(..))` 每调一次就多一个
//!    真 agent，`Agent` 的 required components 自带 `Conversation`
//!    （`rig-ecs/src/agent.rs`、`spawn.rs`）。同一个磁盘会话的日志按 `AgentId`
//!    给每个 agent 记一份，所以"一个磁盘会话里多个 agent"正是上游支持的形状。
//!
//! **结论**：工作区绑定的是**真 agent**（`Agent` + `Conversation`），不是假的
//! `u64` 会话号；`SessionPaths` 保持 app 级唯一，所有工作区的 agent 共享它。
//! `rig_niri::workspaces::Workspace::session` 仍然是 `u64` 占位（那个 crate 不归
//! 本模板改），真实身份由本文件的 [`WorkspaceAgents`] 记录：工作区实体 -> agent 实体。
//!
//! # 转写怎么分到每个工作区
//!
//! * 聚焦工作区：`ShownAgent` 指向它的 agent，`rig_niri::batch` 的真管线
//!   （`rebuild_batch -> run_looks -> collect_rows`）每帧读这个 agent 的
//!   `Conversation` 和内核里还没结算的效应，`apply_focus`/`commit_focused_panels`
//!   负责和全局工作副本同步；
//! * 非聚焦工作区：全局管线只服务聚焦工作区，所以本文件用
//!   `rig_niri::batch::dialogue_batch`（和真管线同一个 `conversation_frame`）
//!   把它自己的对话拼成 batch + 文字块，再用 `rig_niri::batch::live_additions`
//!   补上**实时内容**（在飞的调用、流式正文/推理、待回答的 Ask），按它自己的
//!   转写面板高度窗口化，写进 `WorkspacePanels.rows[PanelId::Transcript]`。
//!   切走的工作区不再"冻住"到回合结束。
//!
//! 这两半都在 `rig_niri::batch` 里，纯函数可测；本文件只负责"哪个工作区绑哪个
//! agent"和窗口化。
//!
//! # 按键
//!
//! | 键 | 作用 |
//! |---|---|
//! | `F1`..`F8` | 聚焦第 n 个工作区（其它隐藏） |
//! | `F9` | 取消聚焦（全部可见） |
//! | `F10` | 自动整理：按 niri 的垂直堆叠重算所有位姿 |

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use rig_ecs::prelude::{Agent, AgentId, Conversation, SpawnedBy};
use rig_ecs::spawn::spawn_root;
use rig_harness::prelude::{Answerer, SessionPaths, ShownAgent};
use rig_niri::batch;
use rig_niri::frame::FrameSize;
use rig_niri::layout;
use rig_niri::panels::{KeyedRow, PanelId};
use rig_niri::transcript::{Row, TextId, Transcript, row_hash};
use rig_niri::workspaces::{
    Workspace, WorkspacePanels, WorkspaceSet, arrange_workspaces, focus_workspace, spawn_workspace,
    workspace_step,
};
use rig_tui::ratatui::style::{Color as RColor, Style};
use rig_tui::ratatui::text::{Line, Span};

/// spawn 时的粗略间距（米）。
///
/// 真正的间距必须由**当前帧高**算（旧的 28 行常量在窗口变成 44 行后会让相邻条带
/// 竖直重叠 0.49m：下面那条的底部行会露进上面那条的可见范围）。所以这里只放一个
/// 粗略值，启动后由 [`restack_on_resize`] 按真实 `FrameSize` 重排。
const STRIP_SPACING_M: f32 = 1.47;

/// 工作区演示插件。
///
/// `count` 为 `None` 时读 `RF_WORKSPACES`（启动配置），测试/嵌入代码可以用
/// [`WorkspacesDemoPlugin::with_count`] 直接固定数量。
#[derive(Default)]
pub struct WorkspacesDemoPlugin {
    /// 固定工作区数；`None` = 读 `RF_WORKSPACES`。
    pub count: Option<usize>,
}

impl WorkspacesDemoPlugin {
    /// 固定工作区数（不读环境变量）。
    pub fn with_count(count: usize) -> Self {
        Self {
            count: Some(count),
        }
    }
}

impl Plugin for WorkspacesDemoPlugin {
    fn build(&self, app: &mut App) {
        let count = self.count.unwrap_or_else(workspace_count_from_env);
        app.insert_resource(DemoWorkspaces(count))
            .init_resource::<WorkspaceAgents>()
            .init_resource::<WorkspaceTranscripts>()
            .add_systems(Startup, spawn_extra_workspaces)
            // 绑定要等 `WorkspacesPlugin` 在 PostStartup 收编第 0 个工作区之后，
            // 所以放在第一帧的 `Update`；幂等，之后的帧只检查漏绑的。
            .add_systems(
                Update,
                bind_workspace_agents.in_set(rig_niri::FeathersSystems::Prepare),
            )
            // 焦点工作区 -> `ShownAgent`：`batch` 的真管线据此选对话。
            .add_systems(
                Update,
                show_focused_agent
                    .in_set(rig_niri::FeathersSystems::Prepare)
                    .after(bind_workspace_agents),
            )
            // 非聚焦工作区：自己的对话 -> 自己的面板行。排在真管线建行之后。
            .add_systems(
                Update,
                sync_workspace_transcripts
                    .in_set(rig_niri::FeathersSystems::Fit)
                    .after(batch::collect_rows),
            )
            .add_systems(Update, demo_keys)
            .add_systems(Update, (show_all_once, arrange_once))
            .add_systems(
                Update,
                restack_on_resize.after(rig_niri::FeathersSystems::Layout),
            );
    }
}

/// `RF_WORKSPACES` 解析；默认 1。
fn workspace_count_from_env() -> usize {
    std::env::var("RF_WORKSPACES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
}

#[derive(Resource)]
struct DemoWorkspaces(usize);

/// 工作区实体 -> 它绑定的真 agent 实体。
///
/// `rig_niri::workspaces::Workspace` 不能改，所以"这个工作区的会话是谁"记在
/// 应用侧这张表里；`Workspace::session` 只作为演示里的创建序号/排序键使用。
#[derive(Resource, Default, Clone, Debug)]
pub struct WorkspaceAgents {
    by_workspace: HashMap<Entity, Entity>,
    by_agent: HashMap<Entity, Entity>,
}

impl WorkspaceAgents {
    /// 这个工作区绑的 agent（还没绑定时 `None`）。
    pub fn agent_of(&self, workspace: Entity) -> Option<Entity> {
        self.by_workspace.get(&workspace).copied()
    }

    /// 这个 agent 属于哪个工作区。
    pub fn workspace_of(&self, agent: Entity) -> Option<Entity> {
        self.by_agent.get(&agent).copied()
    }

    /// 已经绑定的工作区数。
    pub fn len(&self) -> usize {
        self.by_workspace.len()
    }

    /// 还没有任何绑定。
    pub fn is_empty(&self) -> bool {
        self.by_workspace.is_empty()
    }

    /// 所有 `(工作区, agent)`。
    pub fn iter(&self) -> impl Iterator<Item = (Entity, Entity)> + '_ {
        self.by_workspace.iter().map(|(workspace, agent)| (*workspace, *agent))
    }
}

/// 每个工作区自己的对话引擎（只给非聚焦工作区用；聚焦工作区由全局引擎管）。
#[derive(Resource, Default)]
struct WorkspaceTranscripts(HashMap<Entity, Transcript>);

/// 第 0 个由默认屏幕收编，这里只补 1..count。
fn spawn_extra_workspaces(
    mut commands: Commands,
    count: Res<DemoWorkspaces>,
    mut set: ResMut<WorkspaceSet>,
) {
    for index in 1..count.0 {
        let pose = Transform::from_xyz(0.0, -(index as f32) * STRIP_SPACING_M, 0.0);
        spawn_workspace(&mut commands, &mut set, index as u64, pose, index);
    }
    if count.0 > 1 {
        info!(
            "[workspaces] 摆了 {} 个工作区（竖直间距 {:.2} m）",
            count.0,
            STRIP_SPACING_M
        );
    }
}

// ---------------------------------------------------------------------------
// 绑定：工作区 -> 真 agent
// ---------------------------------------------------------------------------

/// 给每个还没有 agent 的工作区绑一个**真** `Agent` + `Conversation`。
///
/// * 第 0 个工作区（默认屏幕）优先收编 app 已有的主 agent（`kernel.rs` 的
///   `main`）：这样默认工作区继续显示主对话，不会凭空多出一个"主 agent"；
///   和 rig-harness 的 `main_agent` 规则一致——还没有 `CurrentAgent` 时取
///   id 最小的顶层 agent（`Without<SpawnedBy>`）。
/// * 其它工作区各自 `spawn_root` 一个新 agent（id `workspace-<session>`）。
///
/// `Workspace::session` 是 rig_niri 的 `u64` 占位，这里只拿它拼真实 `AgentId`；
/// 绑定关系写进 [`WorkspaceAgents`]。
fn bind_workspace_agents(world: &mut World) {
    let Some(items) = world.get_resource::<WorkspaceSet>().map(|set| set.items.clone()) else {
        return;
    };
    if items.is_empty() {
        return;
    }
    let already: HashSet<Entity> = world
        .get_resource::<WorkspaceAgents>()
        .map(|agents| agents.by_workspace.keys().copied().collect())
        .unwrap_or_default();
    let pending: Vec<(Entity, u64, usize)> = items
        .iter()
        .copied()
        .filter(|workspace| !already.contains(workspace))
        .filter_map(|workspace| {
            world
                .get::<Workspace>(workspace)
                .map(|component| (workspace, component.session, component.order))
        })
        .collect();
    if pending.is_empty() {
        return;
    }

    // 第 0 个工作区如果能收编现有的主 agent，就不要再 spawn 一个。
    let mut reusable_main = pending
        .iter()
        .any(|(_, session, _)| *session == 0)
        .then(|| primary_root_agent(world))
        .flatten();

    for (workspace, session, order) in pending {
        let reused = session == 0 && reusable_main.is_some();
        let agent = if reused {
            reusable_main.take().expect("上面刚检查过")
        } else {
            let id = AgentId(format!("workspace-{session}"));
            let name = Name::new(format!("workspace {session} agent"));
            spawn_root(world, (id, name)).id()
        };
        // 一个前端负责显示这些工作区，也就负责回答它们的 Ask。重复 insert 会
        // 覆盖同名组件，不会叠出两个 Answerer。
        world.entity_mut(agent).insert(Answerer { agent: Some(agent) });

        let id = world
            .get::<AgentId>(agent)
            .map(|id| id.0.clone())
            .unwrap_or_else(|| format!("{agent:?}"));
        {
            let mut agents = world.resource_mut::<WorkspaceAgents>();
            agents.by_workspace.insert(workspace, agent);
            agents.by_agent.insert(agent, workspace);
        }
        info!(
            "[workspaces] workspace {order}（session {session}）-> {} agent {id} ({agent:?})",
            if reused { "收编主" } else { "新建" }
        );
    }

    let agents = world.resource::<WorkspaceAgents>();
    if agents.by_workspace.len() == items.len() {
        // `SessionPaths` 是 app 级唯一的磁盘会话（见模块文档）：所有工作区的
        // agent 都写进这一个日志，这里把它的真实路径一起打出来。
        let session = world
            .get_resource::<SessionPaths>()
            .map(|paths| paths.0.path().display().to_string())
            .unwrap_or_else(|| "<没有 SessionPaths>".to_owned());
        info!(
            "[workspaces] 每个工作区一个真会话：agents={} workspaces={} session={session}",
            agents.by_workspace.len(),
            items.len()
        );
    }
}

/// id 最小的顶层 agent（和 rig-harness `main_agent` 没有 `CurrentAgent` 时的规则一致）。
fn primary_root_agent(world: &mut World) -> Option<Entity> {
    let mut query =
        world.query_filtered::<(Entity, &AgentId), (With<Agent>, Without<SpawnedBy>)>();
    let mut roots: Vec<(String, Entity)> = query
        .iter(world)
        .map(|(entity, id)| (id.0.clone(), entity))
        .collect();
    roots.sort_by(|left, right| left.0.cmp(&right.0));
    roots.into_iter().next().map(|(_, entity)| entity)
}

/// 聚焦工作区的 agent 就是 `ShownAgent`（`batch` 真管线据此读它的对话）。
///
/// 只在**焦点变化**时改，用户手动看子 agent（`/agents`、picker）时不要每帧抢回来。
fn show_focused_agent(
    set: Res<WorkspaceSet>,
    agents: Res<WorkspaceAgents>,
    mut shown: Option<ResMut<ShownAgent>>,
    mut last_focused: Local<Option<Entity>>,
) {
    let Some(shown) = shown.as_deref_mut() else {
        return;
    };
    let Some(focused) = set.focused else {
        return;
    };
    let Some(agent) = agents.agent_of(focused) else {
        return;
    };
    if *last_focused == Some(focused) {
        return;
    }
    *last_focused = Some(focused);
    if shown.0 != Some(agent) {
        shown.0 = Some(agent);
        info!("[workspaces] 聚焦工作区 {focused:?} 显示 agent {agent:?}");
    }
}

/// 切焦点时把 `ShownAgent` 一起切过去，避免"这一帧的全局管线还在读旧 agent"。
fn focus_workspace_and_show(world: &mut World, workspace: Entity) {
    focus_workspace(world, workspace);
    let agent = world
        .get_resource::<WorkspaceAgents>()
        .and_then(|agents| agents.agent_of(workspace));
    let Some(agent) = agent else {
        return;
    };
    if let Some(mut shown) = world.get_resource_mut::<ShownAgent>() {
        shown.0 = Some(agent);
    }
}

// ---------------------------------------------------------------------------
// 非聚焦工作区的转写
// ---------------------------------------------------------------------------

/// 非聚焦工作区：把**它自己的 agent** 的对话 + 实时内容写进它自己的面板。
///
/// 对话历史由 [`batch::dialogue_batch`] 拼成 batch（和真管线同一个
/// `conversation_frame`），实时内容（在飞的调用、流式正文/推理、待回答的 Ask）
/// 由 [`batch::live_additions`] 直接读内核，两者接成一条行流，窗口化后写进
/// `WorkspacePanels.rows[PanelId::Transcript]`。切走的工作区因此不再"冻住"。
///
/// 是独占系统：`live_additions` 要在世界上跑查询，这段逻辑本来也要逐个工作区
/// 读写它自己的 `WorkspacePanels`；排在 `Fit` 里保证它在真管线建行之后跑。
///
/// 聚焦工作区跳过：它的行由 `rig_niri::batch` 的真管线写全局 `PanelRows`，
/// 再由 `commit_focused_panels` 提交进 `WorkspacePanels`（唯一写入口径）。
fn sync_workspace_transcripts(world: &mut World) {
    let Some((items, focused)) = world
        .get_resource::<WorkspaceSet>()
        .map(|set| (set.items.clone(), set.focused))
    else {
        return;
    };
    world
        .resource_mut::<WorkspaceTranscripts>()
        .0
        .retain(|workspace, _| items.contains(workspace));

    // 当前主题（`FeathersViewPlugin` 用 `Theme::from_env()` 插，`NO_COLOR`
    // 已折在 `plain` 里）；非聚焦条带和聚焦管线用同一个映射。
    let theme = world
        .get_resource::<rig_niri::theme::Theme>()
        .copied()
        .unwrap_or_default();

    for workspace_entity in items {
        if focused == Some(workspace_entity) {
            continue;
        }
        let Some(agent) = world
            .get_resource::<WorkspaceAgents>()
            .and_then(|agents| agents.agent_of(workspace_entity))
        else {
            continue;
        };
        // 这个工作区自己的转写面板区域：宽高都来自它自己的树。
        let Some((order, area)) = world
            .get::<Workspace>(workspace_entity)
            .zip(world.get::<WorkspacePanels>(workspace_entity))
            .and_then(|(workspace, panels)| {
                let root = panels.roots.entity(PanelId::Transcript)?;
                let area = panels.areas.get(&root).copied()?;
                Some((workspace.order, area))
            })
        else {
            continue;
        };
        if area.cols == 0 || area.rows == 0 {
            if let Some(mut panels) = world.get_mut::<WorkspacePanels>(workspace_entity) {
                panels.rows.set(PanelId::Transcript, Vec::new());
            }
            continue;
        }
        // 对话：和真管线同一个 `conversation_frame`（纯函数，产物是 owned 的）。
        let Some((batch, texts)) = world
            .get::<Conversation>(agent)
            .map(|conversation| batch::dialogue_batch(conversation, agent, area.cols, &theme))
        else {
            continue;
        };
        // 实时：在飞的调用、流式正文/推理、待回答的 Ask（只取本 agent 的）。
        let live = batch::live_additions(world, agent, area.cols);

        let rows = {
            let mut engines = world.resource_mut::<WorkspaceTranscripts>();
            let engine = engines.0.entry(workspace_entity).or_default();
            engine.set_width(area.cols);
            engine.apply(&batch);
            let mut kept: HashSet<TextId> = HashSet::new();
            for (id, lines) in texts {
                engine.set_text(id.clone(), lines);
                kept.insert(id);
            }
            // 对话里已经删掉的文字块不能留在面板上（请求块由 `apply` 每帧 finalize）。
            engine.retain_text(|id| matches!(id, TextId::Message { .. }) && kept.contains(id));
            let mut rows = engine.keyed_rows().to_vec();
            // 实时内容接在对话之后（和真管线里"流式块在消息之后"同一顺序）。
            rows.extend(live);
            rows
        };
        let height = usize::from(area.rows);
        let visible = if rows.is_empty() {
            // 还没有对话：显示**真实**绑定信息（不是假会话号），否则条带是白的。
            vec![workspace_label_row(order, agent, world.get::<AgentId>(agent))]
        } else {
            let max_top = rows.len().saturating_sub(height);
            rows[max_top..].to_vec()
        };
        if let Some(mut panels) = world.get_mut::<WorkspacePanels>(workspace_entity) {
            panels.rows.set(PanelId::Transcript, visible);
        }
    }
}

/// 空工作区的占位行：真实 agent 身份（`AgentId`），不是假的 `session N`。
fn workspace_label_row(order: usize, agent: Entity, id: Option<&AgentId>) -> KeyedRow {
    let text = match id {
        Some(id) => format!(" workspace {order} · agent {}", id.0),
        None => format!(" workspace {order} · agent {agent:?}"),
    };
    let row = Row::new(Line::from(Span::styled(
        text,
        Style::default().fg(RColor::Rgb(120, 128, 145)),
    )));
    KeyedRow {
        key: u64::MAX - 1,
        hash: row_hash(&row),
        row,
    }
}

// ---------------------------------------------------------------------------
// 演示用按键 / 整理
// ---------------------------------------------------------------------------

/// 演示用按键（真应用里这些是导航手势/命令）：
///
/// | 键 | 作用 |
/// |---|---|
/// | `F1`..`F8` | 聚焦第 n 个工作区（其它隐藏） |
/// | `F9` | 取消聚焦（全部可见） |
/// | `F10` | 自动整理：按 niri 的垂直堆叠重算所有位姿 |
fn demo_keys(world: &mut World) {
    let Some(keys) = world.get_resource::<ButtonInput<KeyCode>>() else {
        return;
    };
    let pressed: Vec<KeyCode> = keys.get_just_pressed().copied().collect();
    for key in pressed {
        match key {
            KeyCode::F1 | KeyCode::F2 | KeyCode::F3 | KeyCode::F4 | KeyCode::F5 | KeyCode::F6
            | KeyCode::F7 | KeyCode::F8 => {
                let index = match key {
                    KeyCode::F1 => 0,
                    KeyCode::F2 => 1,
                    KeyCode::F3 => 2,
                    KeyCode::F4 => 3,
                    KeyCode::F5 => 4,
                    KeyCode::F6 => 5,
                    KeyCode::F7 => 6,
                    _ => 7,
                };
                let target = world
                    .get_resource::<WorkspaceSet>()
                    .and_then(|set| set.get(index));
                if let Some(workspace) = target {
                    // 和焦点一起切 `ShownAgent`，这一帧的全局转写就是新会话的。
                    focus_workspace_and_show(world, workspace);
                    info!("[workspaces] 聚焦 F{}", index + 1);
                }
            }
            KeyCode::F9 => {
                if let Some(mut set) = world.get_resource_mut::<WorkspaceSet>() {
                    set.focused = None;
                    info!("[workspaces] 取消聚焦（内容不再写任何条带；所有条带仍然可见）");
                }
            }
            KeyCode::F10 => {
                arrange_workspaces(world);
                info!("[workspaces] 自动整理（niri 垂直堆叠）");
            }
            _ => {}
        }
    }
}

/// 首帧整理一次：让间距/朝向由 `arrange_workspaces` 统一算（和按 F10 的结果一致），
/// 避免"spawn 时的间距"和"整理后的间距"不一样。
fn arrange_once(mut done: Local<bool>, set: Res<WorkspaceSet>, mut commands: Commands) {
    if *done {
        return;
    }
    *done = true;
    if set.items.len() > 1 {
        commands.queue(|world: &mut World| {
            arrange_workspaces(world);
        });
    }
}

/// 帧高变化后，把**仍处于自动排列位置**的工作区按新帧高重新堆叠。
///
/// 为什么需要它：`FrameSize` 启动时先按默认值插入，窗口尺寸同步后才变成真实格子数；
/// `arrange_once` 可能跑在同步之前，于是条带间距按旧帧高算，窗口更大时相邻条带会
/// 竖直重叠（下面条带的底部行露进上面条带的可见范围）。这里在每次 `FrameSize` 变化
/// 时重排一次。
///
/// **逐个工作区判断**，不是全有或全无：把每个工作区当前 pose 和"按旧帧高自动堆叠"
/// 的期望值比较，只重排仍然相等的那些；被用户拖过/手放过的那个留在原地（niri 里
/// 只有用户的动作能移动窗口）。0 号工作区的自动位姿本来就是 `(0,0)`，不需要豁免——
/// 豁免会让"拖过 1 号"后的缩放大小时 0 号也留在旧高度，条带重叠又回来。
fn restack_on_resize(world: &mut World, mut last: Local<Option<FrameSize>>) {
    let Some(size) = world.get_resource::<FrameSize>().copied() else {
        return;
    };
    if *last == Some(size) {
        return;
    }
    let previous = last.replace(size);

    // 和 `arrange_workspaces` 同一排序口径：`order` 相同时按创建顺序。
    // 工作区的世界位姿来自 `Workspace::pose`；实体上没有 `Transform`（别把查询写成
    // `(&Workspace, &Transform)`，那样会一个都查不到、重排静默失效）。
    let mut workspaces: Vec<(usize, usize, Entity, Transform)> = {
        let mut query = world.query::<(Entity, &Workspace)>();
        query
            .iter(world)
            .enumerate()
            .map(|(created, (entity, workspace))| {
                (workspace.order, created, entity, workspace.pose)
            })
            .collect()
    };
    if workspaces.len() < 2 {
        return;
    }
    workspaces.sort_by_key(|(order, created, _, _)| (*order, *created));

    // 首次（还没有旧帧高）：直接按当前帧高整理。
    let Some(previous) = previous else {
        arrange_workspaces(world);
        return;
    };

    let old_step = workspace_step(previous);
    let new_step = workspace_step(size);
    for (index, (_, _, entity, pose)) in workspaces.iter().enumerate() {
        // 自动位姿：x = z = 0、朝向单位阵、y = -index × 旧帧高步长。
        let expected_y = -(index as f32) * old_step;
        let auto = pose.translation.x.abs() < 1e-4
            && pose.translation.z.abs() < 1e-4
            && (pose.translation.y - expected_y).abs() < 1e-3
            && pose.rotation.dot(Quat::IDENTITY).abs() > 1.0 - 1e-6;
        if !auto {
            continue;
        }
        if let Some(mut workspace) = world.get_mut::<Workspace>(*entity) {
            workspace.pose.translation = Vec3::new(0.0, -(index as f32) * new_step, 0.0);
            workspace.pose.rotation = Quat::IDENTITY;
        }
    }
}

/// `RF_SHOW_ALL=1`：启动后取消聚焦，一次看到所有条带。
fn show_all_once(mut shown: Local<bool>, set: Res<WorkspaceSet>) {
    if *shown || std::env::var("RF_SHOW_ALL").is_err() {
        return;
    }
    *shown = true;
    info!(
        "[workspaces] 共 {} 个工作区，竖直排列，全部可见",
        set.items.len()
    );
}

/// 默认树（单工作区时由 bootstrap 收编默认屏幕，这里只给测试用）。
pub fn demo_tree() -> layout::Node {
    layout::default_tree()
}

#[cfg(test)]
mod tests {
    //! 无窗口测试：真 agent + 真对话引擎，断言落在 `WorkspacePanels` 的行上。
    //!
    //! 不建窗口、不渲染、不截图：`FramePlugin` 只算布局，`BatchPlugin` 只建行。

    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::text::FontSource;
    use rig_core::message::{CallId, Message, ToolCall, ToolFunction, ToolName};
    use rig_core::tool::{Tool, ToolContext};
    use rig_ecs::prelude::{CallOf, Deliver, DeliveryMode, ForAgent, Partial, TurnOf};
    use rig_ecs::tools::AppToolsExt;
    use rig_harness::prelude::{Ask, ToolRequest};
    use rig_niri::FeathersSystems;
    use rig_niri::editor::SubmittedPrompts;
    use rig_niri::frame::{FramePlugin, FrameSize};
    use rig_niri::pane::{PanePlugin, TranscriptRows};
    use rig_niri::panels::{PanelRows, PanelsPlugin};
    use rig_niri::transcript::TranscriptFont;
    use rig_niri::workspaces::WorkspacesPlugin;
    use rig_tui::AppStatus;

    /// 条带间距必须按**当前帧高**算：窗口变大后如果还用旧帧高，相邻条带会竖直重叠，
    /// 下面条带底部的行会露进上面条带的可见范围（真实渲染里表现为"1 号的工作区标签
    /// 出现在 0 号条带里"）。
    #[test]
    fn resizing_the_frame_restacks_workspaces_so_strips_do_not_overlap() {
        let mut app = headless(3);
        app.update(); // 首帧：spawn + arrange_once（此时是 96×28）
        app.insert_resource(FrameSize {
            cols: 164,
            rows: 44,
        });
        app.update();

        let size = FrameSize {
            cols: 164,
            rows: 44,
        };
        let step = workspace_step(size);
        let strip = rig_niri::workspaces::workspace_strip_height(size);
        assert!(step > strip, "步长必须大于条带高度（否则重叠）：{step} vs {strip}");

        let mut poses: Vec<(usize, Vec3)> = app
            .world_mut()
            .query::<&Workspace>()
            .iter(app.world())
            .map(|workspace| (workspace.order, workspace.pose.translation))
            .collect();
        poses.sort_by_key(|(order, _)| *order);
        assert_eq!(poses.len(), 3);
        for pair in poses.windows(2) {
            let gap = pair[0].1.y - pair[1].1.y;
            assert!(
                (gap - step).abs() < 1e-3,
                "相邻条带按帧高堆叠：{:?} -> {:?} 间距 {gap}，期望 {step}",
                pair[0],
                pair[1]
            );
        }
    }

    /// 用户手放过的位置不能被"帧高变化"重排抢走（niri：只有用户的动作能移动窗口）。
    #[test]
    fn a_hand_placed_workspace_survives_a_frame_resize() {
        let mut app = headless(3);
        app.update();
        // 手动把 2 号放远（模拟用户拖拽）。
        let moved = app
            .world_mut()
            .query::<&Workspace>()
            .iter(app.world())
            .find(|workspace| workspace.order == 2)
            .map(|workspace| workspace.pose.translation)
            .expect("2 号工作区");
        app.world_mut()
            .query::<&mut Workspace>()
            .iter_mut(app.world_mut())
            .for_each(|mut workspace| {
                if workspace.order == 2 {
                    workspace.pose.translation = Vec3::new(0.4, moved.y - 2.5, 0.0);
                }
            });
        app.insert_resource(FrameSize {
            cols: 164,
            rows: 44,
        });
        app.update();

        let position = app
            .world_mut()
            .query::<&Workspace>()
            .iter(app.world())
            .find(|workspace| workspace.order == 2)
            .map(|workspace| workspace.pose.translation)
            .expect("2 号工作区");
        assert!((position.x - 0.4).abs() < 1e-4, "手放的位置要保住：{position:?}");
    }

    /// 拖过 1 号之后缩放窗口：1 号留在手放的位置，0/2 仍按**新帧高**重排，
    /// 不会再因为"有一个手放的就全都不排"而让 0 号留在旧高度、条带重叠。
    #[test]
    fn resizing_restacks_only_the_untouched_workspaces() {
        let mut app = headless(3);
        app.update();

        let old_size = FrameSize { cols: 96, rows: 28 };
        let old_step = workspace_step(old_size);
        let hand_placed = Vec3::new(3.0, -old_step, 0.0);
        app.world_mut()
            .query::<&mut Workspace>()
            .iter_mut(app.world_mut())
            .for_each(|mut workspace| {
                if workspace.order == 1 {
                    workspace.pose.translation = hand_placed;
                }
            });

        let new_size = FrameSize { cols: 164, rows: 44 };
        let new_step = workspace_step(new_size);
        let strip = rig_niri::workspaces::workspace_strip_height(new_size);
        assert!(new_step > old_step + 0.1, "新帧高必须让步长明显变大");
        app.insert_resource(new_size);
        app.update();

        let mut poses: Vec<(usize, Vec3)> = app
            .world_mut()
            .query::<&Workspace>()
            .iter(app.world())
            .map(|workspace| (workspace.order, workspace.pose.translation))
            .collect();
        poses.sort_by_key(|(order, _)| *order);
        let position = |order: usize| {
            poses
                .iter()
                .find(|(candidate, _)| *candidate == order)
                .map(|(_, position)| *position)
                .expect("工作区存在")
        };

        assert_eq!(position(1), hand_placed, "手放的 1 号留在原地");
        assert_eq!(position(0), Vec3::ZERO, "0 号本来就是自动位姿，照旧在原点");
        assert_eq!(
            position(2),
            Vec3::new(0.0, -2.0 * new_step, 0.0),
            "2 号按新帧高重排（不是旧帧高）"
        );
        // 0 号和 2 号之间隔了两个步长：条带高度 < 步长，所以互不重叠。
        assert!(2.0 * new_step > strip, "两个步长必须大于条带高度");
        let bottom0 = position(0).y;
        let top2 = position(2).y + strip;
        assert!(
            top2 < bottom0 - 1e-3,
            "重排后的 0/2 仍重叠：top2={top2}, bottom0={bottom0}"
        );
    }

    /// 一个永不返回的工具：调它一次就一直是"在飞"的，测试要断言的就是这个状态。
    struct NeverReturns;

    #[derive(serde::Deserialize)]
    struct NeverArgs {
        #[serde(default)]
        #[allow(dead_code)] // 只为让参数 schema 过得去：调用根本不看参数。
        q: String,
    }

    impl Tool for NeverReturns {
        const NAME: &'static str = "search_beta";
        type Args = NeverArgs;
        type Output = String;
        type Error = std::io::Error;

        fn description(&self) -> String {
            "a tool whose call never returns, so the test keeps it in flight".to_owned()
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "q": { "type": "string" } }
            })
        }

        fn call(
            &self,
            _context: &mut ToolContext,
            _args: Self::Args,
        ) -> impl std::future::Future<Output = Result<Self::Output, Self::Error>> + Send {
            async move {
                std::future::pending::<()>().await;
                Ok(String::new())
            }
        }
    }

    /// 装真内核 + 视图数据层 + 2 个工作区（和生产的插件顺序一致），不建窗口。
    fn headless(count: usize) -> App {
        let mut app = App::new();
        app.add_plugins(bevy::app::TaskPoolPlugin::default());
        app.add_plugins(rig_ecs::AgentPlugin);
        app.add_plugins(FramePlugin);
        app.add_plugins(PanelsPlugin);
        app.add_plugins(PanePlugin);
        app.add_plugins(WorkspacesPlugin);
        app.add_plugins(batch::BatchPlugin);
        app.insert_resource(FrameSize { cols: 96, rows: 28 })
            .insert_resource(TranscriptFont(FontSource::default()))
            // `RigHarnessPlugins` 里是 `CurrentAgentPlugin`；测试只装视图层，
            // 自己给一个空的 `ShownAgent`，让真管线有东西可指向。
            .insert_resource(ShownAgent(None))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<SubmittedPrompts>()
            .init_resource::<AppStatus>();
        app.configure_sets(
            Update,
            (
                FeathersSystems::Prepare,
                FeathersSystems::Layout,
                FeathersSystems::Draw,
                FeathersSystems::Fit,
                FeathersSystems::Render,
            )
                .chain(),
        );
        app.add_plugins(WorkspacesDemoPlugin::with_count(count));
        app
    }

    /// 面板行的纯文本。
    fn panel_text(rows: &[KeyedRow]) -> String {
        rows.iter()
            .flat_map(|keyed| keyed.row.line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 某个工作区自己的转写行。
    fn workspace_rows(app: &App, workspace: Entity) -> Vec<KeyedRow> {
        app.world()
            .get::<WorkspacePanels>(workspace)
            .expect("每个工作区都要有 WorkspacePanels")
            .rows
            .get(PanelId::Transcript)
            .to_vec()
    }

    /// 两个工作区 + 两个 agent（默认屏幕是第 0 个，第 1 个由插件 spawn）。
    fn two_workspaces(app: &mut App) -> (Entity, Entity, Entity, Entity) {
        app.update();
        let (ws0, ws1) = {
            let set = app.world().resource::<WorkspaceSet>();
            assert_eq!(set.items.len(), 2, "RF_WORKSPACES=2 应有两个工作区");
            (set.items[0], set.items[1])
        };
        let (agent0, agent1) = {
            let agents = app.world().resource::<WorkspaceAgents>();
            assert_eq!(agents.len(), 2, "两个工作区都要绑定 agent");
            (
                agents.agent_of(ws0).expect("workspace 0 的 agent"),
                agents.agent_of(ws1).expect("workspace 1 的 agent"),
            )
        };
        (ws0, ws1, agent0, agent1)
    }

    #[test]
    fn every_workspace_binds_its_own_real_agent_and_conversation() {
        let mut app = headless(2);
        let (ws0, ws1, agent0, agent1) = two_workspaces(&mut app);

        assert_ne!(agent0, agent1, "两个工作区不能绑同一个 agent");
        for agent in [agent0, agent1] {
            assert!(
                app.world().get::<Agent>(agent).is_some(),
                "工作区绑的必须是真 rig-ecs Agent"
            );
            assert!(
                app.world().get::<Conversation>(agent).is_some(),
                "每个真 agent 都要有自己的 Conversation"
            );
            let id = app
                .world()
                .get::<AgentId>(agent)
                .expect("真 agent 要有 AgentId");
            assert!(!id.0.is_empty(), "AgentId 不能是空串");
        }

        // 映射是双向的，测试/业务都从这张表查。
        let agents = app.world().resource::<WorkspaceAgents>();
        assert_eq!(agents.workspace_of(agent0), Some(ws0));
        assert_eq!(agents.workspace_of(agent1), Some(ws1));
    }

    #[test]
    fn each_workspace_transcript_shows_only_its_own_conversation() {
        let mut app = headless(2);
        let (ws0, ws1, agent0, agent1) = two_workspaces(&mut app);

        // 往各自的真 Conversation 里塞一条用户消息。
        app.world_mut()
            .get_mut::<Conversation>(agent0)
            .expect("agent 0 的对话")
            .0
            .push(Message::user("alpha 只属于 0 号工作区"));
        app.world_mut()
            .get_mut::<Conversation>(agent1)
            .expect("agent 1 的对话")
            .0
            .push(Message::user("beta 只属于 1 号工作区"));
        app.update();

        let text0 = panel_text(&workspace_rows(&app, ws0));
        let text1 = panel_text(&workspace_rows(&app, ws1));
        assert!(text0.contains("alpha"), "0 号工作区要显示自己的消息：{text0:?}");
        assert!(
            !text0.contains("beta"),
            "0 号工作区不能出现 1 号的消息：{text0:?}"
        );
        assert!(text1.contains("beta"), "1 号工作区要显示自己的消息：{text1:?}");
        assert!(
            !text1.contains("alpha"),
            "1 号工作区不能出现 0 号的消息：{text1:?}"
        );

        // 聚焦工作区的全局工作副本（真管线写的那份）也只显示它自己的对话。
        let global = panel_text(
            app.world()
                .resource::<PanelRows>()
                .get(PanelId::Transcript),
        );
        assert!(global.contains("alpha"), "聚焦工作区应是 0 号的对话：{global:?}");
        assert!(!global.contains("beta"), "聚焦工作区不能混入别的对话：{global:?}");

        // 真管线的整段行也来自这个 agent（不是工作区号拼出来的占位）。
        let full = app.world().resource::<TranscriptRows>();
        let full_text = panel_text(&full.0);
        assert!(full_text.contains("alpha"), "TranscriptRows 来自真对话：{full_text:?}");

        // 切换焦点：全局管线跟到 1 号 agent；0 号工作区改由本文件按自己的
        // Conversation 产行，两条条带仍然只显示自己那条消息。
        focus_workspace_and_show(app.world_mut(), ws1);
        app.update();

        let text0 = panel_text(&workspace_rows(&app, ws0));
        let text1 = panel_text(&workspace_rows(&app, ws1));
        assert!(text0.contains("alpha"), "非聚焦的 0 号仍显示自己：{text0:?}");
        assert!(!text0.contains("beta"), "0 号不能混入 1 号：{text0:?}");
        assert!(text1.contains("beta"), "聚焦的 1 号显示自己：{text1:?}");
        assert!(!text1.contains("alpha"), "1 号不能混入 0 号：{text1:?}");

        let global = panel_text(
            app.world()
                .resource::<PanelRows>()
                .get(PanelId::Transcript),
        );
        assert!(global.contains("beta"), "切焦点后全局应是 1 号对话：{global:?}");
        assert!(!global.contains("alpha"), "切焦点后不能还显示旧对话：{global:?}");
    }

    #[test]
    fn a_non_focused_workspace_shows_its_agents_live_content() {
        let mut app = headless(2);
        let (ws0, ws1, agent0, agent1) = two_workspaces(&mut app);

        // 对话里已经落地的消息（两边各一条）。
        app.world_mut()
            .get_mut::<Conversation>(agent0)
            .expect("agent 0 的对话")
            .0
            .push(Message::user("alpha 已完成"));
        app.world_mut()
            .get_mut::<Conversation>(agent1)
            .expect("agent 1 的对话")
            .0
            .push(Message::user("beta 已完成"));

        // 1 号工作区（非聚焦）：一个在飞的调用、一段流式正文、一个待回答的 Ask。
        // 工具永不返回，所以内核会把它留在 `Running` 里，不结算成结果。
        app.add_tool(NeverReturns);
        app.world_mut().spawn((
            ToolRequest {
                call: ToolCall::new(
                    CallId::from_wire("call_beta"),
                    ToolFunction::new(
                        ToolName::new("search_beta").expect("工具名非空"),
                        serde_json::json!({ "q": "beta" }),
                    ),
                ),
            },
            ForAgent(agent1),
        ));
        let turn = app.world_mut().spawn(TurnOf(agent1)).id();
        app.world_mut().spawn((
            CallOf(turn),
            Partial {
                text: "beta 正在流式输出".to_owned(),
                ..Default::default()
            },
        ));
        app.world_mut().spawn((
            Ask::new("keep beta?").choice("yes", "Yes"),
            ForAgent(agent1),
        ));
        app.update();

        // 非聚焦条带有自己的实时内容：以前这里只有已经落进对话的消息。
        let text1 = panel_text(&workspace_rows(&app, ws1));
        assert!(text1.contains("beta 已完成"), "非聚焦工作区要有自己的对话：{text1:?}");
        assert!(
            text1.contains("beta 正在流式输出"),
            "非聚焦工作区要显示流式正文：{text1:?}"
        );
        assert!(
            text1.contains("search_beta"),
            "非聚焦工作区要显示在飞调用的标题：{text1:?}"
        );
        assert!(
            text1.contains("running"),
            "在飞的调用要画成 running（和聚焦条带同一个默认 look）：{text1:?}"
        );
        assert!(
            text1.contains("keep beta?"),
            "非聚焦工作区要显示待回答的 Ask：{text1:?}"
        );

        // 聚焦工作区（0 号）走真管线，行为和改动前一样：它不读 1 号的流式
        // 正文/Ask，自己那条消息还在。
        let text0 = panel_text(&workspace_rows(&app, ws0));
        assert!(text0.contains("alpha 已完成"), "聚焦工作区显示自己的对话：{text0:?}");
        assert!(
            !text0.contains("beta 正在流式输出"),
            "聚焦工作区不能串入别的 agent 的流式正文：{text0:?}"
        );
        assert!(
            !text0.contains("keep beta?"),
            "聚焦工作区不能串入别的 agent 的 Ask：{text0:?}"
        );
        let full_text = panel_text(&app.world().resource::<TranscriptRows>().0);
        assert!(
            full_text.contains("alpha 已完成"),
            "真管线的整段行还是聚焦 agent 的：{full_text:?}"
        );
        assert!(
            !full_text.contains("beta 正在流式输出"),
            "真管线只读聚焦 agent 的流式正文：{full_text:?}"
        );
    }

    // ---- 输入框 -> 当前聚焦工作区的 agent ----

    /// 观察到的 `Deliver`。
    #[derive(Resource, Default)]
    struct Delivered(Vec<(Entity, String, DeliveryMode)>);

    fn record_deliver(deliver: On<Deliver>, mut delivered: ResMut<Delivered>) {
        delivered.0.push((
            deliver.entity,
            deliver.message.text().to_owned(),
            deliver.message.mode,
        ));
    }

    /// 视图输入层 + 真内核 + 工作区（和生产同插件顺序），不建窗口。
    fn typing_headless(count: usize) -> App {
        let mut app = headless(count);
        app.add_plugins(rig_niri::editor::EditorPlugin);
        // 输入行系统要的 `Theme` 在生产里由 `FeathersViewPlugin` 初始化。
        app.init_resource::<rig_niri::theme::Theme>()
            .add_message::<KeyboardInput>()
            .init_resource::<Delivered>()
            .add_observer(record_deliver);
        app
    }

    fn key(app: &mut App, code: KeyCode, key: Key, text: Option<&str>) {
        app.world_mut().write_message(KeyboardInput {
            key_code: code,
            logical_key: key,
            state: ButtonState::Pressed,
            text: text.map(|text| text.into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    fn type_prompt(app: &mut App, text: &str) {
        for character in text.chars() {
            let typed = character.to_string();
            key(
                app,
                KeyCode::KeyA, // 测试只关心字符，不关心物理键
                Key::Character(typed.clone().into()),
                Some(&typed),
            );
        }
        key(app, KeyCode::Enter, Key::Enter, None);
    }

    #[test]
    fn enter_sends_to_the_focused_workspaces_agent() {
        let mut app = typing_headless(2);
        let (ws0, ws1, agent0, agent1) = two_workspaces(&mut app);
        assert_eq!(
            app.world().resource::<ShownAgent>().0,
            Some(agent0),
            "默认聚焦 0 号工作区"
        );
        assert_eq!(
            app.world().resource::<WorkspaceSet>().focused,
            Some(ws0),
            "先确认 F2 之前焦点在 0 号"
        );

        // F2：走演示按键 -> `focus_workspace_and_show`（真实焦点路径）。
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F2);
        app.update();
        assert_eq!(
            app.world().resource::<WorkspaceSet>().focused,
            Some(ws1),
            "F2 应聚焦 1 号工作区"
        );
        assert_eq!(
            app.world().resource::<ShownAgent>().0,
            Some(agent1),
            "切焦点后 `ShownAgent` 跟到 1 号工作区的 agent"
        );

        type_prompt(&mut app, "hello 1");
        assert_eq!(
            app.world().resource::<Delivered>().0,
            [(agent1, "hello 1".to_owned(), DeliveryMode::Steer)],
            "Enter 应送给当前聚焦工作区的 agent，而不是 0 号"
        );
    }
}
