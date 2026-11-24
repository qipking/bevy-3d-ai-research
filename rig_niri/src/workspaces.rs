//! 工作区：一个会话的完整界面 + 它在世界里的绝对位姿。
//!
//! 模型见 `docs/status/WORKSPACE_MODEL.md`：世界层可以有多个工作区，每个工作区
//! 内部是一条 niri 无限条带。第 2 批先把**面板状态**按工作区拆开：
//!
//! * [`Workspace`] 是工作区实体：会话 id（先用 `u64` 占位）、内部分裂树
//!   [`Workspace::tree`]、世界位姿 [`Workspace::pose`]、整理顺序 [`Workspace::order`]；
//! * [`WorkspacePanels`] 挂在同一个实体上，装这个工作区自己的 [`PanelRows`] /
//!   [`PanelRoots`] / [`PanelSpawned`] / 本帧区域。这是每个工作区的**权威状态**，
//!   两个工作区的行、根实体、已生成行互不相干；
//! * 放置发生在 [`crate::frame`]：`pose × tree.rects()` 得到每块面板的世界位姿。
//!
//! ## 旧全局资源怎么处理
//!
//! [`PanelRows`]/[`PanelRoots`]/[`PanelSpawned`]/[`FrameTree`] 仍是 `Resource`，
//! 因为还有不少模块（`status`/`editor`/`picker`/`complete`/`niri`/`selection`…）
//! 只认识它们。它们现在**只代表当前聚焦工作区**，是聚焦工作区的工作副本：
//!
//! ```text
//! 内容系统（Draw/Fit） ──写──> 全局 PanelRows / FrameTree / PanelRoots / PanelSpawned
//!                                      │
//!                        Render 里由 commit_focused_panels 写回组件（唯一写入口径）
//!                                      ▼
//!                         Workspace 实体上的 WorkspacePanels（权威快照）
//!                                      │
//!                     apply_focus（聚焦切换时）单向装载回全局
//!                                      ▼
//!                               下一帧的全局工作副本
//! ```
//!
//! 所以全局资源不是第二份真相：非聚焦工作区的状态只存在组件里，内容系统只写
//! 聚焦工作区（全局工作副本）。没有 [`WorkspaceSet`] 的 App（只装
//! [`crate::panels::PanelsPlugin`]/[`crate::frame::FramePlugin`] 的旧测试）继续
//! 走原来的全局单例路径，行为不变。
//!
//! ## 世界层的四个操作（第 3 批）
//!
//! 用户原话："让工作区可以在空间内拖拽 放置 自动整理（niri 排列工作区的方式
//! 重置工作区空间 pos 坐标）聚焦（无特殊动作只是隐藏其他工作区）"。
//!
//! | 操作 | 入口 | 触发 |
//! |---|---|---|
//! | 自动整理 | [`arrange_workspaces`] | 应用层按键（`rig_harness_in_3d` 接线） |
//! | 放置 | [`place_workspace`] | 命令 / 拖拽落点 |
//! | 拖拽 | `drag_workspaces` 系统（内部 [`drag_world_delta`]） | **中键**拖动，或 **Alt+左键**拖动 |
//! | 聚焦 | [`focus_workspace`]（只决定内容写哪个工作区，**不隐藏任何东西**） | 应用层按键 |
//!
//! 四者的共同约束（`docs/status/WORKSPACE_MODEL.md` §5）：世界层操作只改
//! `pose` 或 `Visibility`，**不碰**工作区内部的树、行、面板尺寸。自动整理是
//! niri 的竖直堆叠（窗口往右排、工作区竖着排），聚焦只是把其它工作区的面板
//! 根设成 `Visibility::Hidden`——不动相机、不做动画、不改变任何几何。
//!
//! ## 默认工作区
//!
//! [`WorkspacesPlugin`] 在 `PostStartup` 检查 [`WorkspaceSet`]：
//!
//! * 全局资源里没有默认屏幕（只装了 WorkpacesPlugin / 纯 headless）→ 建一个
//!   空工作区并聚焦它；
//! * 有默认屏幕（`spawn_default_screen` / `spawn_niri_screen`）→ 把面板收编成
//!   `session = 0`、位姿单位阵、`order = 0` 的默认工作区。即使应用已经在
//!   Startup 建了显式工作区，默认屏幕也要收编（放在 `items[0]`）——它就是
//!   第 0 个会话的界面；
//! * 没有默认屏幕、但已有显式工作区 → 聚焦第一个并把它的状态装进全局代理。

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};

use crate::frame::{
    DefaultScreen, FramePanel, FrameSize, FrameTree, PanelArea, PanelAreas, UNITS_TO_WORLD,
};
use crate::layout;
use crate::panels::{PanelId, PanelRoots, PanelRows, PanelSpawned};
use crate::FeathersSystems;

/// 一个工作区 = 一个会话的完整界面 + 它在世界里的绝对位姿。
#[derive(Component, Clone, Debug, PartialEq)]
pub struct Workspace {
    /// 会话 id。这里先用 `u64` 占位，接真会话时换成 rig-harness 的类型。
    pub session: u64,
    /// 工作区内部的分裂树（niri 条带）。叶子是 [`PanelId`]。
    pub tree: layout::Node,
    /// 绝对世界位姿（不是相对相机）。面板世界位姿 = `pose × 帧内矩形`。
    pub pose: Transform,
    /// 整理顺序（niri 自动整理工作区时按它排）。
    pub order: usize,
}

/// 所有工作区。
///
/// `focused` 现在决定内容写入口径（内容系统只写它）；第 3 批再让它决定可见性
/// ——聚焦只隐藏其它工作区，不改任何几何。
#[derive(Resource, Default, Clone, Debug)]
pub struct WorkspaceSet {
    /// 工作区实体，按创建顺序（整理看 [`Workspace::order`]）。
    pub items: Vec<Entity>,
    /// 当前聚焦的工作区：内容系统写它，别的只显示不接收新内容。
    pub focused: Option<Entity>,
}

impl WorkspaceSet {
    /// 聚焦工作区是否就是 `workspace`。
    pub fn is_focused(&self, workspace: Entity) -> bool {
        self.focused == Some(workspace)
    }

    /// 创建顺序里的第 `index` 个工作区。
    pub fn get(&self, index: usize) -> Option<Entity> {
        self.items.get(index).copied()
    }
}

/// 一个工作区的面板状态（挂在 [`Workspace`] 实体上）。
///
/// 这是每个工作区的**权威快照**；聚焦工作区每帧由 [`commit_focused_panels`]
/// 从全局工作副本写回，切换聚焦时由 [`apply_focus`] 装回全局。
#[derive(Component, Clone, Default)]
pub struct WorkspacePanels {
    /// 这个工作区每块面板的行。
    pub rows: PanelRows,
    /// 这个工作区的面板根实体（[`PanelId`] → 实体）。
    pub roots: PanelRoots,
    /// 这个工作区每块面板已生成的行（增量 diff 用）。
    pub spawned: PanelSpawned,
    /// 这个工作区这一帧的面板区域（帧内坐标，尚未乘 [`Workspace::pose`]）。
    pub areas: HashMap<Entity, PanelArea>,
}

impl WorkspacePanels {
    /// 这个工作区的行。
    pub fn rows(&self) -> &PanelRows {
        &self.rows
    }

    /// 这个工作区的面板根实体。
    pub fn roots(&self) -> &PanelRoots {
        &self.roots
    }

    /// 这个工作区这一帧算出来的区域。
    pub fn area(&self, root: Entity) -> Option<PanelArea> {
        self.areas.get(&root).copied()
    }
}

/// 一块 UI 面板属于哪个工作区（值是工作区实体）。
///
/// 每块面板的 [`crate::frame::FramePanel`] 旁边挂它：放置系统据此拿工作区的
/// `pose`，行同步据此找这个工作区自己的根表。面板仍然是最上层 `bevy_ui` 根
/// （不挂 `ChildOf`：bevy_ui 的根节点必须自己就是根，否则尺寸算不出来）。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspacePanel(pub Entity);

/// 已经装载进全局代理的工作区，避免每帧重复装载。
#[derive(Resource, Default)]
struct LoadedFocus(Option<Entity>);

// ---------------------------------------------------------------------------
// 创建
// ---------------------------------------------------------------------------

/// 新建一个工作区（默认屏幕的树 + 四块树面板）。
///
/// 面板实体直接挂 [`WorkspacePanel`] 标签，登记进这个工作区自己的
/// [`WorkspacePanels`]；不会写全局代理。`set.focused` 为空时把新工作区设为
/// 聚焦，全局代理会在下一次 `Update` 的 `Prepare` 装载。
pub fn spawn_workspace(
    commands: &mut Commands,
    set: &mut WorkspaceSet,
    session: u64,
    pose: Transform,
    order: usize,
) -> Entity {
    spawn_workspace_with_tree(
        commands,
        set,
        session,
        layout::default_tree(),
        pose,
        order,
    )
}

/// 同 [`spawn_workspace`]，但用给定的分裂树（`tree` 的叶子必须都能在默认屏幕
/// 的四块面板里找到；多出来的叶子不会放置）。
pub fn spawn_workspace_with_tree(
    commands: &mut Commands,
    set: &mut WorkspaceSet,
    session: u64,
    tree: layout::Node,
    pose: Transform,
    order: usize,
) -> Entity {
    let workspace = commands
        .spawn((
            Name::new(format!("workspace:{session}")),
            Workspace {
                session,
                tree,
                pose,
                order,
            },
        ))
        .id();
    let (roots, rows) = crate::frame::default_screen_panels(commands, Some(workspace));
    commands.entity(workspace).insert(WorkspacePanels {
        rows,
        roots,
        spawned: PanelSpawned::default(),
        areas: HashMap::new(),
    });
    set.items.push(workspace);
    if set.focused.is_none() {
        set.focused = Some(workspace);
    }
    workspace
}

// ---------------------------------------------------------------------------
// 聚焦：全局代理 ↔ 工作区组件
// ---------------------------------------------------------------------------

/// 把 `workspace` 设为聚焦工作区，并立即把它的状态装进全局代理。
///
/// 返回之前的聚焦工作区（没有返回 `None`）。切换前会先把旧聚焦的全局工作副本
/// 提交回它的组件，所以切换不会丢还没走到 `Render` 的行。
///
/// 第 3 批的 `focus_workspace` 操作（只改可见性）在这里先把"装载"这一半做好。
pub fn focus_workspace(world: &mut World, workspace: Entity) -> Option<Entity> {
    if world.get::<Workspace>(workspace).is_none() {
        return world
            .get_resource::<WorkspaceSet>()
            .and_then(|set| set.focused);
    }
    let previous = {
        let Some(mut set) = world.get_resource_mut::<WorkspaceSet>() else {
            return None;
        };
        let previous = set.focused;
        if previous == Some(workspace) {
            return previous;
        }
        set.focused = Some(workspace);
        previous
    };
    if let Some(previous) = previous {
        commit_into_world(world, previous);
    }
    install_into_world(world, workspace);
    if let Some(mut loaded) = world.get_resource_mut::<LoadedFocus>() {
        loaded.0 = Some(workspace);
    }
    previous
}

/// 把全局工作副本写回 `workspace` 的组件（聚焦时每帧 `Render` 调一次）。
pub(crate) fn commit_focused_panels(
    set: Option<Res<WorkspaceSet>>,
    rows: Option<Res<PanelRows>>,
    roots: Option<Res<PanelRoots>>,
    spawned: Option<Res<PanelSpawned>>,
    tree: Option<Res<FrameTree>>,
    mut workspaces: Query<(&mut Workspace, &mut WorkspacePanels)>,
) {
    let Some(set) = set else {
        return;
    };
    let Some(focused) = set.focused else {
        return;
    };
    let Ok((mut workspace, mut panels)) = workspaces.get_mut(focused) else {
        return;
    };
    if let Some(rows) = rows {
        panels.rows.0 = rows.0.clone();
    }
    if let Some(roots) = roots {
        panels.roots.0 = roots.0.clone();
    }
    if let Some(spawned) = spawned
        && panels.spawned != *spawned
    {
        panels.spawned.0 = spawned.0.clone();
    }
    if let Some(tree) = tree
        && workspace.tree != tree.0
    {
        workspace.tree = tree.0.clone();
    }
}

/// 把聚焦工作区的行实体表发回全局 `PanelSpawned`。
///
/// `PanelsPlugin` 的行同步会改 `WorkspacePanels::spawned`（生成/回收行实体），
/// 下一帧 `picker` 之类的模块还要从全局 `PanelSpawned` 里 `forget`，所以同步完
/// 要把这张表发回去。
pub(crate) fn publish_focused_spawned(
    set: Option<Res<WorkspaceSet>>,
    mut spawned: Option<ResMut<PanelSpawned>>,
    workspaces: Query<&WorkspacePanels>,
) {
    let (Some(set), Some(spawned)) = (set, spawned.as_deref_mut()) else {
        return;
    };
    let Some(focused) = set.focused else {
        return;
    };
    let Ok(panels) = workspaces.get(focused) else {
        return;
    };
    if *spawned != panels.spawned {
        *spawned = panels.spawned.clone();
    }
}

/// 聚焦变化时，把新工作区的组件状态装进全局工作副本。
///
/// 直接改 [`WorkspaceSet::focused`]（不必走 [`focus_workspace`]）也能生效：
/// 这个系统在 `Prepare` 检测 `LoadedFocus`，下一帧 `Layout` 之前完成装载。
fn apply_focus(
    set: Res<WorkspaceSet>,
    mut loaded: ResMut<LoadedFocus>,
    rows: Option<ResMut<PanelRows>>,
    roots: Option<ResMut<PanelRoots>>,
    spawned: Option<ResMut<PanelSpawned>>,
    tree: Option<ResMut<FrameTree>>,
    areas: Option<ResMut<PanelAreas>>,
    mut screen: Option<ResMut<DefaultScreen>>,
    workspaces: Query<(&Workspace, &WorkspacePanels)>,
) {
    let Some(focused) = set.focused else {
        loaded.0 = None;
        return;
    };
    if loaded.0 == Some(focused) {
        return;
    }
    let Ok((workspace, panels)) = workspaces.get(focused) else {
        return;
    };
    if let Some(mut rows) = rows {
        rows.0 = panels.rows.0.clone();
    }
    if let Some(mut roots) = roots {
        roots.0 = panels.roots.0.clone();
    }
    if let Some(mut spawned) = spawned {
        spawned.0 = panels.spawned.0.clone();
    }
    if let Some(mut tree) = tree {
        tree.0 = workspace.tree.clone();
    }
    if let Some(mut areas) = areas {
        areas.0 = panels.areas.clone();
    }
    if let Some(screen) = screen.as_deref_mut() {
        screen.transcript = panels.roots.entity(PanelId::Transcript);
    }
    loaded.0 = Some(focused);
}

// ---------------------------------------------------------------------------
// 启动收编
// ---------------------------------------------------------------------------

/// 第一个 `Update` 之前把现有面板状态收编成一个默认工作区（或装载显式工作区）。
///
/// 注意"显式工作区"和"默认屏幕"可以同时存在：`FramePlugin` 的
/// `spawn_default_screen` 总会在 Startup 先 spawn 一套面板，应用自己的
/// Startup 系统再 spawn 额外工作区（`RF_WORKSPACES=3` 就是这样）。这时默认
/// 屏幕仍然是 **session 0 的工作区**，必须一起收编，否则：
///
/// * 它的面板成了没有工作区的孤儿（布局/行同步都不会碰，屏幕中间一片空）；
/// * 全局代理会被装载到第一个显式工作区，真会话的内容写进了错误的条带。
fn bootstrap_workspaces(
    mut commands: Commands,
    mut set: ResMut<WorkspaceSet>,
    mut loaded: ResMut<LoadedFocus>,
    mut rows: Option<ResMut<PanelRows>>,
    mut roots: Option<ResMut<PanelRoots>>,
    mut spawned: Option<ResMut<PanelSpawned>>,
    mut tree: Option<ResMut<FrameTree>>,
    mut areas: Option<ResMut<PanelAreas>>,
    mut screen: Option<ResMut<DefaultScreen>>,
    panels: Query<Entity, (With<FramePanel>, Without<WorkspacePanel>)>,
    workspaces: Query<(&Workspace, &WorkspacePanels)>,
) {
    // 全局根表非空 = `spawn_default_screen` / `spawn_niri_screen` 确实建了
    // 一套默认屏幕（显式工作区的根表在组件里，不写全局）。
    let legacy_screen = roots.as_deref().is_some_and(|roots| !roots.0.is_empty());
    if set.items.is_empty() || legacy_screen {
        // 把 `spawn_default_screen` / `spawn_niri_screen` 留在全局资源里的那套
        // 面板收编成默认工作区（session 0、位姿单位阵、order 0）。全局资源本来
        // 就是这套面板的状态，直接克隆进组件即可（下一帧起组件是权威快照）。
        let workspace = commands
            .spawn((
                Name::new("workspace:0"),
                Workspace {
                    session: 0,
                    tree: tree
                        .as_deref()
                        .map(|tree| tree.0.clone())
                        .unwrap_or_else(layout::default_tree),
                    pose: Transform::IDENTITY,
                    order: 0,
                },
                WorkspacePanels {
                    rows: rows.as_deref().cloned().unwrap_or_default(),
                    roots: roots.as_deref().cloned().unwrap_or_default(),
                    spawned: spawned.as_deref().cloned().unwrap_or_default(),
                    areas: areas.as_deref().map(|areas| areas.0.clone()).unwrap_or_default(),
                },
            ))
            .id();
        // 启动时还**没挂工作区标签**的面板（默认屏幕 / niri 屏幕 /
        // `RF_OVERLAY` 浮层）归默认工作区；显式工作区自己的面板已经有标签，
        // 绝不能再改（否则它们会被并进 session 0）。
        for entity in &panels {
            commands.entity(entity).insert(WorkspacePanel(workspace));
        }
        // 显式工作区存在时，默认屏幕仍是最前面的第 0 个会话。
        if set.items.is_empty() {
            set.items.push(workspace);
        } else {
            set.items.insert(0, workspace);
        }
        set.focused = Some(workspace);
        loaded.0 = Some(workspace);
        return;
    }

    // 显式创建的工作区：聚焦第一个，并把它的状态装进全局代理。
    if set.focused.is_none() {
        set.focused = set.items.first().copied();
    }
    let Some(focused) = set.focused else {
        return;
    };
    let Ok((workspace, panels)) = workspaces.get(focused) else {
        return;
    };
    if let Some(rows) = rows.as_deref_mut() {
        rows.0 = panels.rows.0.clone();
    }
    if let Some(roots) = roots.as_deref_mut() {
        roots.0 = panels.roots.0.clone();
    }
    if let Some(spawned) = spawned.as_deref_mut() {
        spawned.0 = panels.spawned.0.clone();
    }
    if let Some(tree) = tree.as_deref_mut() {
        tree.0 = workspace.tree.clone();
    }
    if let Some(areas) = areas.as_deref_mut() {
        areas.0 = panels.areas.clone();
    }
    if let Some(screen) = screen.as_deref_mut() {
        screen.transcript = panels.roots.entity(PanelId::Transcript);
    }
    loaded.0 = Some(focused);
}

/// 新 spawn 的 [`FramePanel`]（picker / 补全之类的浮层）归当前聚焦工作区。
///
/// 它们不是工作区创建时那套面板，没有机会在 spawn 处挂标签；在 `Layout`
/// 统一补上（排在 `lay_out_frame` 之前），同一帧就能按聚焦工作区的位姿放它们。
fn tag_new_panels(
    mut commands: Commands,
    set: Res<WorkspaceSet>,
    new_panels: Query<Entity, (With<FramePanel>, Without<WorkspacePanel>)>,
) {
    let Some(focused) = set.focused else {
        return;
    };
    for entity in &new_panels {
        commands.entity(entity).insert(WorkspacePanel(focused));
    }
}

// ---------------------------------------------------------------------------
// `&mut World` 版的装载/提交（给 [`focus_workspace`] 用）
// ---------------------------------------------------------------------------

fn install_into_world(world: &mut World, workspace: Entity) {
    let (tree, rows, roots, spawned, areas, transcript) = {
        let Some(workspace_component) = world.get::<Workspace>(workspace) else {
            return;
        };
        let Some(panels) = world.get::<WorkspacePanels>(workspace) else {
            return;
        };
        (
            workspace_component.tree.clone(),
            panels.rows.clone(),
            panels.roots.clone(),
            panels.spawned.clone(),
            panels.areas.clone(),
            panels.roots.entity(PanelId::Transcript),
        )
    };
    if let Some(mut resource) = world.get_resource_mut::<PanelRows>() {
        *resource = rows;
    }
    if let Some(mut resource) = world.get_resource_mut::<PanelRoots>() {
        *resource = roots;
    }
    if let Some(mut resource) = world.get_resource_mut::<PanelSpawned>() {
        *resource = spawned;
    }
    if let Some(mut resource) = world.get_resource_mut::<FrameTree>() {
        *resource = FrameTree(tree);
    }
    if let Some(mut resource) = world.get_resource_mut::<PanelAreas>() {
        *resource = PanelAreas(areas);
    }
    if let Some(mut screen) = world.get_resource_mut::<DefaultScreen>() {
        screen.transcript = transcript;
    }
}

fn commit_into_world(world: &mut World, workspace: Entity) {
    let tree = world.get_resource::<FrameTree>().map(|tree| tree.0.clone());
    let rows = world.get_resource::<PanelRows>().cloned();
    let roots = world.get_resource::<PanelRoots>().cloned();
    let spawned = world.get_resource::<PanelSpawned>().cloned();
    let Some(mut panels) = world.get_mut::<WorkspacePanels>(workspace) else {
        return;
    };
    if let Some(rows) = rows {
        panels.rows = rows;
    }
    if let Some(roots) = roots {
        panels.roots = roots;
    }
    if let Some(spawned) = spawned {
        panels.spawned = spawned;
    }
    drop(panels);
    if let (Some(tree), Some(mut component)) =
        (tree, world.get_mut::<Workspace>(workspace))
    {
        component.tree = tree;
    }
}

// ---------------------------------------------------------------------------
// 世界层的四个操作：自动整理 / 放置 / 拖拽 / 可见性
// ---------------------------------------------------------------------------
//
// 入口表见模块文档。这里只碰世界层：`pose`（位置/朝向）与 `Visibility`；
// 工作区内部的分裂树、行、面板尺寸一个字节都不动。

/// 工作区之间的竖直间距：niri 默认 gap 16 px 折算成世界单位（0.0288 m）。
pub const WORKSPACE_GAP: f32 = 16.0 * UNITS_TO_WORLD;

/// 一个工作区条带在世界里的高度：帧高（格）× 行高 × 单位换算。
///
/// 帧高用 [`FrameSize::rows`]（和 `frame::lay_out` 铺条带用的高度是同一个数），
/// 不是"输入 + 状态 + 最小转写"那种估算：条带内部怎么分是分裂树的事，
/// 自动整理只负责世界层的间距。
pub fn workspace_strip_height(frame: FrameSize) -> f32 {
    f32::from(frame.rows) * crate::transcript::ROW_HEIGHT * UNITS_TO_WORLD
}

/// niri 排列工作区时相邻两个的竖直步长 = 条带高度 + 间距。
pub fn workspace_step(frame: FrameSize) -> f32 {
    workspace_strip_height(frame) + WORKSPACE_GAP
}

/// 自动整理：按 [`Workspace::order`] 把所有工作区**竖直堆叠**，覆盖手放的坐标。
///
/// niri 的工作区是动态的、竖着排的（窗口往右排到无限条带，工作区往下叠），
/// 所以按 `order` 排序后的第 `i` 个工作区放在：
///
/// ```text
/// x = 0, z = 0, y = -(i as f32) × step
/// step = workspace_strip_height(frame) + WORKSPACE_GAP
/// rotation = Quat::IDENTITY（朝向统一）
/// ```
///
/// 这正是"重置工作区空间 pos 坐标"：手放/拖拽过的位置全部被覆盖，但
/// **不动树的内部布局**（树、行、面板尺寸都不在这条路径上）。
///
/// 返回整理后的顺序（实体列表，下标就是竖直位置）；`order` 相同的工作区
/// 按创建顺序排，保证结果稳定。
pub fn arrange_workspaces(world: &mut World) -> Vec<Entity> {
    let frame = world.get_resource::<FrameSize>().copied().unwrap_or_default();
    let step = workspace_step(frame);
    let mut items: Vec<(usize, usize, Entity)> = {
        let mut query = world.query::<(Entity, &Workspace)>();
        query
            .iter(world)
            .enumerate()
            .map(|(created, (entity, workspace))| (workspace.order, created, entity))
            .collect()
    };
    items.sort_by_key(|item| (item.0, item.1));
    let arranged: Vec<Entity> = items.iter().map(|(_, _, entity)| *entity).collect();
    for (index, entity) in arranged.iter().enumerate() {
        let Some(mut workspace) = world.get_mut::<Workspace>(*entity) else {
            continue;
        };
        workspace.pose.translation = Vec3::new(0.0, -(index as f32) * step, 0.0);
        workspace.pose.rotation = Quat::IDENTITY;
    }
    arranged
}

/// 放置：显式设置某个工作区的绝对位姿（拖拽落点 / 命令用它）。
///
/// 只改这一个工作区；其它工作区的位姿、尺寸、内部布局都不受影响。返回是否
/// 找到该工作区（实体不存在时是 `false`，不 panic）。
pub fn place_workspace(world: &mut World, workspace: Entity, pose: Transform) -> bool {
    let Some(mut component) = world.get_mut::<Workspace>(workspace) else {
        return false;
    };
    if component.pose != pose {
        component.pose = pose;
    }
    true
}

/// 拖拽的一步：给某个工作区叠加一个世界位移（只改平移，不动朝向/缩放）。
///
/// 这是 [`drag_world_delta`] 算完位移之后的落点；返回是否找到该工作区。
pub fn drag_workspace(world: &mut World, workspace: Entity, delta: Vec3) -> bool {
    if !delta.is_finite() {
        return false;
    }
    let Some(mut component) = world.get_mut::<Workspace>(workspace) else {
        return false;
    };
    component.pose.translation += delta;
    true
}

/// 一条世界射线（原点 + 单位方向）。
///
/// 用局部类型而不是 `bevy_shape::Ray3d`，免得把渲染侧的拾取类型漏进这个
/// crate 的公开 API；数学和 `Camera::viewport_to_world` 是同一条。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldRay {
    pub origin: Vec3,
    pub direction: Vec3,
}

/// 把 NDC 位置反投影成世界射线（相机 view-projection 的逆变换）。
///
/// `clip_from_world` = `Camera::clip_from_view() × camera_transform.affine().inverse()`
/// （相机在世界里）。取裁剪空间 `z = 1`（近平面；bevy 用 reverse-Z）与 `z = 0`
/// （远平面）两点，两者连线就是这条像素对应的视线。原点放在近平面：这样
/// [`ray_plane`] 的 `距离 >= 0` 就等于"交点在相机前方"。
pub fn ray_from_ndc(clip_from_world: Mat4, ndc: Vec2) -> Option<WorldRay> {
    let inverse = clip_from_world.inverse();
    if !inverse.is_finite() {
        return None;
    }
    let unproject = |z: f32| -> Option<Vec3> {
        let world = inverse * Vec4::new(ndc.x, ndc.y, z, 1.0);
        if !world.is_finite() || world.w.abs() < 1e-6 {
            return None;
        }
        Some(world.truncate() / world.w)
    };
    let near = unproject(1.0)?;
    let far = unproject(0.0)?;
    let direction = (far - near).normalize_or_zero();
    (direction != Vec3::ZERO).then_some(WorldRay {
        origin: near,
        direction,
    })
}

/// 射线与平面求交（`plane_point` 过点，`plane_normal` 法向）；平面在相机
/// 背后（交点在射线反方向）时返回 `None`。
pub fn ray_plane(ray: WorldRay, plane_point: Vec3, plane_normal: Vec3) -> Option<Vec3> {
    let denominator = ray.direction.dot(plane_normal);
    if denominator.abs() < 1e-6 {
        return None;
    }
    let distance = (plane_point - ray.origin).dot(plane_normal) / denominator;
    (distance.is_finite() && distance >= 0.0)
        .then_some(ray.origin + ray.direction * distance)
}

/// 指针从 `from_ndc` 拖到 `to_ndc` 时，工作区应该走的**世界位移**。
///
/// 两端各反投影成一条视线，投到**过工作区中心**的平面上（法向 = 工作区的
/// 面板朝向，见 `workspace_plane`），交点之差就是位移：
///
/// ```text
/// ray(ndc)  = 反投影的视线（ray_from_ndc）
/// hit(ndc)  = ray(ndc) ∩ { p : (p − plane_point) · plane_normal = 0 }
/// Δworld    = hit(to_ndc) − hit(from_ndc)
/// ```
///
/// 这是"屏幕像素 → 世界米"的唯一换算；**不是**把像素差直接加到世界上
/// （相机一斜，像素和米就不是线性关系）。
pub fn drag_world_delta(
    clip_from_world: Mat4,
    from_ndc: Vec2,
    to_ndc: Vec2,
    plane_point: Vec3,
    plane_normal: Vec3,
) -> Option<Vec3> {
    let start = ray_plane(
        ray_from_ndc(clip_from_world, from_ndc)?,
        plane_point,
        plane_normal,
    )?;
    let end = ray_plane(
        ray_from_ndc(clip_from_world, to_ndc)?,
        plane_point,
        plane_normal,
    )?;
    Some(end - start)
}

/// 正在进行的拖拽（[`drag_workspaces`] 用）。
///
/// 只记按下时的快照：每帧拿"按下 NDC → 当前 NDC"重新算世界位移，不在上一帧
/// 结果上叠加，所以拖久了也不会漂移。结束拖拽位置保留（不自动弹回）。
#[derive(Resource, Default, Debug)]
pub struct WorkspaceDrag {
    active: Option<ActiveDrag>,
}

impl WorkspaceDrag {
    /// 正在拖的工作区（没有在拖时 `None`）。
    pub fn workspace(&self) -> Option<Entity> {
        self.active.map(|active| active.workspace)
    }

    /// 有没有正在拖。
    pub fn is_dragging(&self) -> bool {
        self.active.is_some()
    }
}

/// 一次拖拽的按下快照。
#[derive(Clone, Copy, Debug)]
struct ActiveDrag {
    /// 被拖的工作区（**只**改它一个）。
    workspace: Entity,
    /// 按下时的位姿：拖拽结果 = `start_pose.translation + Δworld`。
    start_pose: Transform,
    /// 开始拖拽时按下的鼠标键（松开它就结束）。
    button: MouseButton,
    /// 拖拽平面：过工作区中心，法向 = 工作区面板朝向。
    plane_point: Vec3,
    /// 拖拽平面的法向（`pose.rotation * Vec3::Z`）。
    plane_normal: Vec3,
    /// 按下时的指针 NDC。
    start_ndc: Vec2,
}

/// 拖拽平面：过工作区中心、和它的面板平行（法向 = 位姿的 Z 轴）。
///
/// 面板就在这个平面上，所以把指针射线投到这里，"按住哪一点就拖哪一点"
/// 的手感最自然；相机俯仰不影响平面的定义。
fn workspace_plane(pose: &Transform) -> (Vec3, Vec3) {
    let normal = (pose.rotation * Vec3::Z).normalize_or_zero();
    let normal = if normal == Vec3::ZERO { Vec3::Z } else { normal };
    (pose.translation, normal)
}

/// 这个工作区现在可见吗？聚焦 = 只显示它；没有聚焦时全部可见
/// （所有工作区都可见：niri 的工作区是竖着摆的，靠视口移动去看，不靠隐藏）。
fn visible_workspace(set: Option<&WorkspaceSet>, workspace: Entity) -> bool {
    set.is_none_or(|set| set.focused.is_none_or(|focused| focused == workspace))
}

/// 拖拽键位：**中键**，或 **Alt+左键**。
///
/// 普通左键留给选字（`selection::pointer_selection` 会忽略 Alt+左键），所以
/// 两者不打架。返回值是"这次按下用的是哪个键"，松开它拖拽结束。
fn drag_button(
    buttons: &ButtonInput<MouseButton>,
    keys: Option<&ButtonInput<KeyCode>>,
) -> Option<MouseButton> {
    if buttons.just_pressed(MouseButton::Middle) {
        return Some(MouseButton::Middle);
    }
    let alt = keys.is_some_and(|keys| {
        keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight)
    });
    (alt && buttons.just_pressed(MouseButton::Left)).then_some(MouseButton::Left)
}

/// 主窗口的指针位置（逻辑像素，原点左上）。
fn cursor_position(windows: &Query<&Window, With<PrimaryWindow>>) -> Option<Vec2> {
    windows.iter().next().and_then(Window::cursor_position)
}

/// 指针位置 → `(clip_from_world, ndc)`；没有可用相机/视口时给 `None`。
fn pointer_ndc(
    cameras: &Query<(Entity, &Camera, &GlobalTransform)>,
    cursor: Vec2,
) -> Option<(Mat4, Vec2)> {
    for (_, camera, transform) in cameras.iter() {
        if !camera.is_active {
            continue;
        }
        let Ok(ndc) = camera.viewport_to_ndc(cursor) else {
            continue;
        };
        let clip_from_world = camera.clip_from_view() * Mat4::from(transform.affine().inverse());
        if clip_from_world.is_finite() {
            return Some((clip_from_world, ndc));
        }
    }
    None
}

/// 指针 NDC 的视线落在哪个**可见**工作区的条带里（取最近的那个）。
///
/// 条带 = 帧矩形（`FrameSize × CELL/ROW_HEIGHT`）在 `pose` 平面上的位置。
/// 这是工作区自己的"抓取区域"：不用拾取某个具体 UI 节点，所以面板空白处
/// 也能按住拖。
fn workspace_under_pointer(
    clip_from_world: Mat4,
    ndc: Vec2,
    frame: FrameSize,
    set: Option<&WorkspaceSet>,
    workspaces: &mut Query<(Entity, &mut Workspace)>,
) -> Option<Entity> {
    let ray = ray_from_ndc(clip_from_world, ndc)?;
    let half = Vec2::new(
        f32::from(frame.cols) * crate::frame::CELL.x,
        f32::from(frame.rows) * crate::transcript::ROW_HEIGHT,
    ) * UNITS_TO_WORLD
        * 0.5;
    let mut nearest: Option<(f32, Entity)> = None;
    for (entity, workspace) in workspaces.iter_mut() {
        if !visible_workspace(set, entity) {
            continue;
        }
        let (plane_point, plane_normal) = workspace_plane(&workspace.pose);
        let Some(hit) = ray_plane(ray, plane_point, plane_normal) else {
            continue;
        };
        let local = workspace.pose.to_matrix().inverse().transform_point3(hit);
        if local.x.abs() > half.x || local.y.abs() > half.y {
            continue;
        }
        let distance = ray.origin.distance_squared(hit);
        if nearest.is_none_or(|(best, _)| distance < best) {
            nearest = Some((distance, entity));
        }
    }
    nearest.map(|(_, entity)| entity)
}

/// 拖拽系统：中键 / Alt+左键按住某个工作区的条带，拖动改它的 `pose.translation`。
///
/// * 位移用 [`drag_world_delta`] 从指针的屏幕位置反算（投到过工作区中心的
///   平面上），**不是**直接加屏幕像素；
/// * 只改被拖的那一个工作区，其它工作区的位姿/锚点像素级不动；
/// * 松开后位置保留（不会自动弹回；只有 [`arrange_workspaces`] 会覆盖它）；
/// * 所有工作区都可见（niri 不隐藏工作区，只靠位置区分）。
///
/// 没有窗口/相机/鼠标资源（headless 测试、只装工作区插件的 App）时是 no-op。
#[allow(clippy::too_many_arguments)]
fn drag_workspaces(
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(Entity, &Camera, &GlobalTransform)>,
    frame: Option<Res<FrameSize>>,
    set: Option<Res<WorkspaceSet>>,
    mut drag: ResMut<WorkspaceDrag>,
    mut workspaces: Query<(Entity, &mut Workspace)>,
) {
    let Some(buttons) = buttons.as_deref() else {
        return;
    };

    if let Some(active) = drag.active {
        // 松开按下的那个键 = 结束拖拽；位置留在松手处（不弹回）。
        if !buttons.pressed(active.button) {
            drag.active = None;
            return;
        }
        let Some(cursor) = cursor_position(&windows) else {
            return;
        };
        let Some((clip_from_world, ndc)) = pointer_ndc(&cameras, cursor) else {
            return;
        };
        let Some(delta) = drag_world_delta(
            clip_from_world,
            active.start_ndc,
            ndc,
            active.plane_point,
            active.plane_normal,
        ) else {
            return;
        };
        let Ok((_, mut workspace)) = workspaces.get_mut(active.workspace) else {
            drag.active = None;
            return;
        };
        let translation = active.start_pose.translation + delta;
        if workspace.pose.translation != translation {
            workspace.pose.translation = translation;
        }
        return;
    }

    let Some(button) = drag_button(buttons, keys.as_deref()) else {
        return;
    };
    let Some(cursor) = cursor_position(&windows) else {
        return;
    };
    let Some((clip_from_world, ndc)) = pointer_ndc(&cameras, cursor) else {
        return;
    };
    let frame = frame.as_deref().copied().unwrap_or_default();
    let Some(workspace) =
        workspace_under_pointer(clip_from_world, ndc, frame, set.as_deref(), &mut workspaces)
    else {
        return;
    };
    let Ok((_, component)) = workspaces.get(workspace) else {
        return;
    };
    let start_pose = component.pose;
    let (plane_point, plane_normal) = workspace_plane(&start_pose);
    drag.active = Some(ActiveDrag {
        workspace,
        start_pose,
        button,
        plane_point,
        plane_normal,
        start_ndc: ndc,
    });
}


/// 工作区插件：启动收编 + 聚焦装载 + 新面板打标签。
///
/// 放置（`frame::lay_out_frame` / `frame::place_panels`）和行同步
/// （`panels::sync_panels`）在自己的插件里按 [`WorkspaceSet`] 分派，不在这里。
pub struct WorkspacesPlugin;

impl Plugin for WorkspacesPlugin {
    fn build(&self, app: &mut App) {
        // 单独装本插件时也要有确定的阶段顺序；`FeathersViewPlugin` 重复配置
        // 同一组边是幂等的。
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
        )
        .init_resource::<WorkspaceSet>()
        .init_resource::<LoadedFocus>()
        // 第 3 批：拖拽状态（中键 / Alt+左键，见 `drag_workspaces`）。
        .init_resource::<WorkspaceDrag>()
        // niri 模式的屏幕在 `PostStartup` 生成：先让它写全局资源，再收编。
        .add_systems(
            PostStartup,
            bootstrap_workspaces.after(crate::niri::spawn_niri_screen),
        )
        .add_systems(
            Update,
            apply_focus.in_set(FeathersSystems::Prepare),
        )
        // 第 3 批：拖动工作区（世界坐标由 `drag_world_delta` 从屏幕位置反算）。
        .add_systems(
            Update,
            drag_workspaces.in_set(FeathersSystems::Prepare),
        )
        // 新 spawn 的面板（picker / 补全 / 拆窗）在 `Prepare` 之后统一补标签：
        // 放到 `Layout` 且排在 `lay_out_frame` 之前，命令缓冲已经刷新，同一帧
        // 就能按聚焦工作区的位姿放好。
        .add_systems(
            Update,
            tag_new_panels
                .in_set(FeathersSystems::Layout)
                .before(crate::frame::lay_out_frame),
        );
    }
}

#[cfg(test)]
mod tests {
    //! 每个工作区一套面板状态的 headless 测试：不建窗口、不跑渲染。
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::text::FontSource;
    use rig_tui::ratatui::text::Line;

    use crate::frame::{FrameGeometry, FramePlugin, FrameSize, CELL, UNITS_TO_WORLD};
    use crate::layout::{Child, Node as LayoutNode, Split, NO_GAP};
    use crate::panels::{KeyedRow, PanelsPlugin, RowLayout};
    use crate::selection::Selection;
    use crate::transcript::{Row, RowText, TranscriptFont};

    fn keyed(key: u64, text: &str) -> KeyedRow {
        KeyedRow {
            key,
            hash: key,
            row: Row::new(Line::from(text.to_owned())),
        }
    }

    fn plain(row: &KeyedRow) -> String {
        row.row
            .line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    /// 只装面板/工作区/布局几个系统和资源的最小 App；工作区由测试自己 spawn。
    fn workspace_app() -> App {
        let mut app = App::new();
        app.init_resource::<LoadedFocus>()
            .init_resource::<FrameTree>()
            .init_resource::<PanelAreas>()
            .init_resource::<FrameGeometry>()
            .init_resource::<Selection>()
            .init_resource::<DefaultScreen>()
            .insert_resource(FrameSize { cols: 96, rows: 28 })
            .insert_resource(TranscriptFont(FontSource::default()))
            .add_plugins((PanelsPlugin, WorkspacesPlugin))
            .add_systems(
                Update,
                (crate::frame::lay_out_frame, crate::frame::place_panels)
                    .chain()
                    .in_set(FeathersSystems::Layout),
            );
        app
    }

    /// 两个位姿不同、内部树相同的工作区（锚点差才正好等于 pose 差）。
    fn two_workspaces(
        mut commands: Commands,
        mut set: ResMut<WorkspaceSet>,
    ) {
        spawn_workspace(
            &mut commands,
            &mut set,
            1,
            Transform::from_xyz(1.0, 2.0, 3.0),
            0,
        );
        spawn_workspace(
            &mut commands,
            &mut set,
            2,
            Transform::from_xyz(-4.0, 5.0, 0.5),
            1,
        );
    }

    fn app_with_two_workspaces() -> App {
        let mut app = workspace_app();
        app.add_systems(Startup, two_workspaces);
        app.update();
        app
    }

    fn workspaces(app: &App) -> (Entity, Entity) {
        let set = app.world().resource::<WorkspaceSet>();
        assert_eq!(set.items.len(), 2, "两个工作区");
        (set.items[0], set.items[1])
    }

    fn rows_of(app: &App, workspace: Entity) -> &PanelRows {
        &app.world()
            .get::<WorkspacePanels>(workspace)
            .expect("工作区要有面板状态")
            .rows
    }

    fn root_of(app: &App, workspace: Entity, panel: PanelId) -> Entity {
        app.world()
            .get::<WorkspacePanels>(workspace)
            .expect("工作区要有面板状态")
            .roots
            .entity(panel)
            .unwrap_or_else(|| panic!("{panel:?} 要有根实体"))
    }

    fn row_entities(app: &App, root: Entity) -> Vec<Entity> {
        app.world()
            .get::<Children>(root)
            .map(|children| children.iter().collect())
            .unwrap_or_default()
    }

    // ---- 第 3 批（世界层四个操作）的测试辅助 ----

    fn pose_of(app: &App, workspace: Entity) -> Transform {
        app.world()
            .get::<Workspace>(workspace)
            .expect("Workspace")
            .pose
    }

    /// 每块面板根的 3D 锚点（世界坐标，米）。
    fn panel_anchors(app: &App, workspace: Entity) -> Vec<(PanelId, Vec3)> {
        let panels = app
            .world()
            .get::<WorkspacePanels>(workspace)
            .expect("WorkspacePanels");
        [
            PanelId::Transcript,
            PanelId::Input,
            PanelId::Status,
            PanelId::Complete,
        ]
        .into_iter()
        .filter_map(|id| {
            let root = panels.roots.entity(id)?;
            let anchor = app
                .world()
                .get::<feathers_in_3d::WorldPanel>(root)?
                .anchor
                .translation;
            Some((id, anchor))
        })
        .collect()
    }

    /// 每块面板根的 `Node` 像素尺寸。
    fn panel_sizes(app: &App, workspace: Entity) -> Vec<(PanelId, Vec2)> {
        fn px(value: bevy::ui::Val) -> f32 {
            match value {
                bevy::ui::Val::Px(value) => value,
                _ => 0.0,
            }
        }
        let panels = app
            .world()
            .get::<WorkspacePanels>(workspace)
            .expect("WorkspacePanels");
        [
            PanelId::Transcript,
            PanelId::Input,
            PanelId::Status,
            PanelId::Complete,
        ]
        .into_iter()
        .filter_map(|id| {
            let root = panels.roots.entity(id)?;
            let node = app.world().get::<Node>(root)?;
            Some((id, Vec2::new(px(node.width), px(node.height))))
        })
        .collect()
    }

    fn visibility_of(app: &App, workspace: Entity, panel: PanelId) -> Visibility {
        *app.world()
            .get::<Visibility>(root_of(app, workspace, panel))
            .expect("面板根要有 Visibility")
    }

    /// 假窗口 + 正交相机：相机在 `(0,0,5)` 看 -Z，视口 x/y ∈ [-2, 2]。
    ///
    /// 正交投影下 `world = ndc × 2`（z 不影响 x/y），所以测试里的期望位移
    /// 可以直接算出来；拖拽代码走的是和真实相机同一条 view-projection 逆变换。
    fn add_fake_camera_and_window(app: &mut App) -> (Entity, Entity) {
        let camera = app.world_mut().spawn((
            Camera {
                computed: bevy::camera::ComputedCameraValues {
                    clip_from_view: bevy::math::proj::orthographic(-2.0, 2.0, -2.0, 2.0, 0.1, 100.0),
                    target_info: Some(bevy::camera::RenderTargetInfo {
                        physical_size: UVec2::new(800, 600),
                        scale_factor: 1.0,
                    }),
                    ..default()
                },
                ..default()
            },
            GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 5.0)),
        )).id();
        let mut window = Window::default();
        window.resolution = bevy::window::WindowResolution::new(800, 600);
        window.set_cursor_position(Some(Vec2::new(400.0, 300.0)));
        let window = app.world_mut().spawn((window, PrimaryWindow)).id();
        (camera, window)
    }

    struct DragFixture {
        app: App,
        camera: Entity,
        window: Entity,
    }

    impl DragFixture {
        fn update(&mut self) {
            self.app.update();
            // bevy 的 `InputPlugin` 每帧清 `just_pressed`；测试 App 没装输入
            // 插件，手动清，否则"松开后再走几帧"会被当成又按了一次。
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
        }

        /// NDC (x, y) → 窗口逻辑像素（原点左上，y 朝下）。
        fn set_cursor_ndc(&mut self, ndc: Vec2) {
            let position = Vec2::new((ndc.x + 1.0) * 0.5 * 800.0, (1.0 - ndc.y) * 0.5 * 600.0);
            self.app
                .world_mut()
                .get_mut::<Window>(self.window)
                .expect("假窗口")
                .set_cursor_position(Some(position));
        }

        fn press(&mut self, button: MouseButton) {
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .press(button);
        }

        fn release(&mut self, button: MouseButton) {
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .release(button);
        }

        fn dragging(&self) -> Option<Entity> {
            self.app.world().resource::<WorkspaceDrag>().workspace()
        }
    }

    /// 两个都在相机画面里的工作区：A 在原点（聚焦），B 在 y = -1（隐藏）。
    fn two_visible_workspaces(mut commands: Commands, mut set: ResMut<WorkspaceSet>) {
        spawn_workspace(&mut commands, &mut set, 1, Transform::IDENTITY, 0);
        spawn_workspace(
            &mut commands,
            &mut set,
            2,
            Transform::from_xyz(0.0, -1.0, 0.0),
            1,
        );
    }

    fn drag_app() -> DragFixture {
        let mut app = workspace_app();
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.add_systems(Startup, two_visible_workspaces);
        let (camera, window) = add_fake_camera_and_window(&mut app);
        let mut fixture = DragFixture { app, camera, window };
        fixture.update();
        fixture
    }

    // 1. 不显式创建时插件自动建一个默认工作区，默认屏幕照旧。
    #[test]
    fn the_plugin_creates_one_default_workspace() {
        if crate::niri::niri_layout() {
            return; // RF_NIRI_LAYOUT 打开时默认屏幕由 niri 生成。
        }
        let mut app = App::new();
        app.init_resource::<Selection>()
            .insert_resource(TranscriptFont(FontSource::default()))
            .add_plugins((FramePlugin, PanelsPlugin, WorkspacesPlugin));
        // 固定成默认帧尺寸，免得环境里的 `RF_FRAME` 影响锚点回归。
        app.insert_resource(FrameSize { cols: 96, rows: 28 });
        app.update();

        let set = app.world().resource::<WorkspaceSet>();
        assert_eq!(set.items.len(), 1, "没有显式创建时只有一个默认工作区");
        let focused = set.focused.expect("默认工作区聚焦");
        assert_eq!(focused, set.items[0]);
        let workspace = app.world().get::<Workspace>(focused).expect("Workspace");
        assert_eq!(workspace.session, 0);
        assert_eq!(workspace.pose, Transform::IDENTITY);
        assert_eq!(workspace.order, 0);
        assert_eq!(workspace.tree, layout::default_tree());

        // 四块面板都登记进这个工作区自己的根表，并打了标签。
        let panels = app.world().get::<WorkspacePanels>(focused).unwrap();
        for id in [
            PanelId::Transcript,
            PanelId::Input,
            PanelId::Status,
            PanelId::Complete,
        ] {
            let root = panels.roots.entity(id).expect("默认屏幕的根实体");
            assert_eq!(
                app.world().get::<WorkspacePanel>(root).map(|tag| tag.0),
                Some(focused),
                "{id:?} 挂在自己的工作区根下"
            );
        }

        // 默认工作区位姿是单位阵：锚点与第 2 批之前的单屏布局逐像素一致。
        let transcript = panels.roots.entity(PanelId::Transcript).unwrap();
        let anchor = app
            .world()
            .get::<feathers_in_3d::WorldPanel>(transcript)
            .expect("WorldPanel")
            .anchor
            .translation;
        let want_x = -16.0 * CELL.x * UNITS_TO_WORLD;
        assert!(
            (anchor.x - want_x).abs() < 1e-5,
            "默认工作区的转写锚点 x = {}",
            anchor.x
        );
        assert_eq!(anchor.y, 0.0);
    }

    // 2. 默认工作区的转写能拿到行：写进全局代理的行提交到组件并生成行实体。
    #[test]
    fn the_default_workspace_transcript_gets_rows() {
        if crate::niri::niri_layout() {
            return;
        }
        let mut app = App::new();
        app.init_resource::<Selection>()
            .insert_resource(TranscriptFont(FontSource::default()))
            .add_plugins((FramePlugin, PanelsPlugin, WorkspacesPlugin));
        app.update();

        let focused = app.world().resource::<WorkspaceSet>().focused.unwrap();
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(7, "hello workspace")]);
        app.update();

        let rows = rows_of(&app, focused);
        assert_eq!(rows.get(PanelId::Transcript).len(), 1);
        assert_eq!(plain(&rows.get(PanelId::Transcript)[0]), "hello workspace");

        let root = root_of(&app, focused, PanelId::Transcript);
        let entities = row_entities(&app, root);
        assert_eq!(entities.len(), 1, "行实体画在默认工作区的转写根下");
        assert_eq!(
            app.world().get::<RowText>(entities[0]).map(|text| text.0.as_str()),
            Some("hello workspace")
        );
        assert_eq!(
            app.world().get::<RowLayout>(entities[0]).map(|layout| layout.panel),
            Some(PanelId::Transcript)
        );
    }

    // 3. 两个工作区各自的 PanelRoots 实体不同，且都挂对了标签。
    #[test]
    fn two_workspaces_keep_different_panel_roots() {
        let app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        for id in [
            PanelId::Transcript,
            PanelId::Input,
            PanelId::Status,
            PanelId::Complete,
        ] {
            let root_a = root_of(&app, a, id);
            let root_b = root_of(&app, b, id);
            assert_ne!(root_a, root_b, "{id:?} 两个工作区的根不能是同一实体");
            assert_eq!(
                app.world().get::<WorkspacePanel>(root_a).map(|tag| tag.0),
                Some(a)
            );
            assert_eq!(
                app.world().get::<WorkspacePanel>(root_b).map(|tag| tag.0),
                Some(b)
            );
        }
    }

    // 4. 内容系统只写聚焦工作区：A 写一行，B 看不到。
    #[test]
    fn writing_rows_only_touches_the_focused_workspace() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        assert_eq!(app.world().resource::<WorkspaceSet>().focused, Some(a));

        // 内容系统走全局代理（聚焦工作区的工作副本）。
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(11, "only in A")]);
        app.update();

        let rows_a = rows_of(&app, a);
        assert_eq!(rows_a.get(PanelId::Transcript).len(), 1);
        assert_eq!(plain(&rows_a.get(PanelId::Transcript)[0]), "only in A");
        assert!(
            rows_of(&app, b).get(PanelId::Transcript).is_empty(),
            "B 不能看到 A 的行"
        );
    }

    // 5. 切换聚焦：全局代理换成新工作区的状态；切回去 A 的行还在。
    #[test]
    fn switching_focus_swaps_the_global_proxy() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(1, "A")]);
        app.update();

        // 切到 B：全局代理是 B 的空行，写 B 的行。
        let previous = focus_workspace(app.world_mut(), b);
        assert_eq!(previous, Some(a));
        assert!(app
            .world()
            .resource::<PanelRows>()
            .get(PanelId::Transcript)
            .is_empty());
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(2, "B")]);
        app.update();

        assert_eq!(
            plain(&rows_of(&app, a).get(PanelId::Transcript)[0]),
            "A",
            "B 聚焦期间 A 的行原样保留"
        );
        assert_eq!(
            plain(&rows_of(&app, b).get(PanelId::Transcript)[0]),
            "B"
        );

        // 切回 A：全局代理恢复 A 的状态。
        focus_workspace(app.world_mut(), a);
        assert_eq!(
            app.world()
                .resource::<PanelRows>()
                .get(PanelId::Transcript)
                .len(),
            1
        );
        assert_eq!(
            plain(&app.world().resource::<PanelRows>().get(PanelId::Transcript)[0]),
            "A"
        );
    }

    // 5b. 直接改 `WorkspaceSet::focused`（不走 `focus_workspace`）也会在下一帧
    // 的 `Prepare` 装载新工作区。
    #[test]
    fn directly_setting_focused_is_picked_up_by_prepare() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(3, "A only")]);
        app.update();

        app.world_mut().resource_mut::<WorkspaceSet>().focused = Some(b);
        app.update();
        assert!(
            app.world()
                .resource::<PanelRows>()
                .get(PanelId::Transcript)
                .is_empty(),
            "切到 B 后全局代理是 B 的空行"
        );
        assert_eq!(
            plain(&rows_of(&app, a).get(PanelId::Transcript)[0]),
            "A only",
            "A 的行在组件里保留"
        );

        app.world_mut().resource_mut::<WorkspaceSet>().focused = Some(a);
        app.update();
        assert_eq!(
            plain(&app.world().resource::<PanelRows>().get(PanelId::Transcript)[0]),
            "A only",
            "直接改回 A，Prepare 把 A 的行装回全局"
        );
    }

    // 6. 行同步按工作区走：各自的根下只挂自己的行。
    #[test]
    fn each_workspace_syncs_its_own_row_entities() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(1, "alpha"), keyed(2, "beta")]);
        app.update();

        let root_a = root_of(&app, a, PanelId::Transcript);
        let root_b = root_of(&app, b, PanelId::Transcript);
        assert_eq!(row_entities(&app, root_a).len(), 2, "A 的两行");
        assert!(
            row_entities(&app, root_b).is_empty(),
            "B 的根下没有行实体"
        );

        // 直接改非聚焦工作区的组件（模拟以后第 3 批的多会话内容），同步也照常。
        let row = keyed(9, "only B");
        app.world_mut()
            .get_mut::<WorkspacePanels>(b)
            .unwrap()
            .rows
            .set(PanelId::Transcript, vec![row]);
        app.update();
        let root_b = root_of(&app, b, PanelId::Transcript);
        let entities_b = row_entities(&app, root_b);
        assert_eq!(entities_b.len(), 1);
        assert_eq!(
            app.world().get::<RowText>(entities_b[0]).map(|text| text.0.as_str()),
            Some("only B")
        );
        assert_eq!(row_entities(&app, root_a).len(), 2, "A 的行没有被 B 覆盖");
    }

    // 7. 每块面板的已生成行状态也按工作区分开。
    #[test]
    fn each_workspace_has_its_own_spawned_row_state() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(1, "one")]);
        app.update();

        let spawned_a = &app.world().get::<WorkspacePanels>(a).unwrap().spawned;
        let spawned_b = &app.world().get::<WorkspacePanels>(b).unwrap().spawned;
        assert_eq!(spawned_a.0.get(&PanelId::Transcript).unwrap().entities.len(), 1);
        assert!(
            spawned_b
                .0
                .get(&PanelId::Transcript)
                .is_none_or(|spawned| spawned.entities.is_empty()),
            "B 还没有生成过行"
        );
        // 全局 PanelSpawned 是聚焦工作区的代理。
        let global = app.world().resource::<PanelSpawned>();
        assert_eq!(
            global.0.get(&PanelId::Transcript).unwrap().keys,
            spawned_a.0[&PanelId::Transcript].keys
        );
    }

    // 8. 放置：两个工作区 pose 不同 → 同一个 PanelId 的锚点差 = pose 差。
    #[test]
    fn placement_offsets_a_panel_by_the_workspace_pose() {
        let app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let pose_a = app.world().get::<Workspace>(a).unwrap().pose;
        let pose_b = app.world().get::<Workspace>(b).unwrap().pose;
        for id in [PanelId::Transcript, PanelId::Status] {
            let anchor_a = app
                .world()
                .get::<feathers_in_3d::WorldPanel>(root_of(&app, a, id))
                .expect("A 的 WorldPanel")
                .anchor
                .translation;
            let anchor_b = app
                .world()
                .get::<feathers_in_3d::WorldPanel>(root_of(&app, b, id))
                .expect("B 的 WorldPanel")
                .anchor
                .translation;
            let delta = anchor_b - anchor_a;
            let want = pose_b.translation - pose_a.translation;
            assert!(
                (delta - want).length() < 1e-5,
                "{id:?}: 锚点差 {delta:?} != pose 差 {want:?}"
            );
        }
    }

    // 9. 放置遍历所有工作区：每块面板都从自己工作区的树/区域拿到尺寸。
    #[test]
    fn placement_uses_each_workspaces_own_tree_and_areas() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        // B 换成一棵两叶子的树：放置要读每个工作区**自己的**树。
        let tree_b = LayoutNode::split(
            Split::Vertical,
            NO_GAP,
            vec![
                Child::proportion(1.0, LayoutNode::leaf(PanelId::Transcript)),
                Child::proportion(1.0, LayoutNode::leaf(PanelId::Status)),
            ],
        );
        app.world_mut().get_mut::<Workspace>(b).unwrap().tree = tree_b;
        app.update();

        let size = app.world().resource::<FrameSize>();
        let frame = crate::frame::CellRect::new(0, 0, size.cols, size.rows);
        let frame_tile = crate::niri::TileRect {
            center: Vec2::ZERO,
            size: Vec2::new(f32::from(frame.w), f32::from(frame.h)) * CELL * UNITS_TO_WORLD,
        };

        for workspace in [a, b] {
            let tree = app.world().get::<Workspace>(workspace).unwrap().tree.clone();
            let panels = app.world().get::<WorkspacePanels>(workspace).unwrap();
            let rects = tree.rects(frame_tile);
            assert_eq!(
                panels.areas.len(),
                rects.len(),
                "工作区 {workspace:?} 的每个叶子都有区域"
            );
            for (id, tile) in rects {
                let root = panels.roots.entity(id).expect("叶子有根实体");
                let area = panels.area(root).expect("叶子有区域");
                assert!(
                    (area.size - tile.size / UNITS_TO_WORLD).length() < 1e-3,
                    "{id:?} 的尺寸要来自自己工作区的树"
                );
                // 3D 锚点来自自己工作区：B 的锚点平移了 pose。
                let anchor = app
                    .world()
                    .get::<feathers_in_3d::WorldPanel>(root)
                    .expect("WorldPanel")
                    .anchor
                    .translation;
                let pose = app.world().get::<Workspace>(workspace).unwrap().pose;
                assert!(
                    (anchor
                        - (pose.translation
                            + Vec3::new(
                                area.center.unwrap().x,
                                area.center.unwrap().y,
                                0.0,
                            ) * UNITS_TO_WORLD))
                        .length()
                        < 1e-4,
                    "{id:?} 的锚点 = pose + 帧内中心"
                );
            }
        }

        // 全局 PanelAreas 是聚焦工作区（A）的代理。
        let focused = app.world().resource::<WorkspaceSet>().focused.unwrap();
        let panels = app.world().get::<WorkspacePanels>(focused).unwrap();
        assert_eq!(app.world().resource::<PanelAreas>().0, panels.areas);
    }

    // 10. 每个工作区的叶子铺满自己的条带（叶子面积之和 = 条带面积）。
    #[test]
    fn each_workspace_tree_fills_its_own_strip() {
        let app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let size = app.world().resource::<FrameSize>();
        let frame_tile = crate::niri::TileRect {
            center: Vec2::ZERO,
            size: Vec2::new(f32::from(size.cols), f32::from(size.rows)) * CELL * UNITS_TO_WORLD,
        };
        let strip = frame_tile.size.x * frame_tile.size.y;
        for workspace in [a, b] {
            let tree = app.world().get::<Workspace>(workspace).unwrap().tree.clone();
            let rects = tree.rects(frame_tile);
            let leaves: f32 = rects
                .iter()
                .map(|(_, rect)| rect.size.x * rect.size.y)
                .sum();
            assert!(
                (leaves - strip).abs() < 1e-4,
                "工作区 {workspace:?} 的叶子面积 {leaves} != 条带面积 {strip}"
            );
            // 叶子区域都真的落在条带里。
            for (id, rect) in rects {
                let left = rect.center.x - rect.size.x * 0.5;
                let right = rect.center.x + rect.size.x * 0.5;
                let bottom = rect.center.y - rect.size.y * 0.5;
                let top = rect.center.y + rect.size.y * 0.5;
                assert!(
                    left >= -frame_tile.size.x * 0.5 - 1e-5
                        && right <= frame_tile.size.x * 0.5 + 1e-5
                );
                assert!(
                    bottom >= -frame_tile.size.y * 0.5 - 1e-5
                        && top <= frame_tile.size.y * 0.5 + 1e-5,
                    "{id:?}"
                );
                let root = app
                    .world()
                    .get::<WorkspacePanels>(workspace)
                    .unwrap()
                    .roots
                    .entity(id)
                    .expect("叶子有根实体");
                assert!(
                    app.world()
                        .get::<WorkspacePanels>(workspace)
                        .unwrap()
                        .area(root)
                        .is_some(),
                    "{id:?} 有区域"
                );
            }
        }
    }

    // 11. 聚焦工作区的树可以改（补全 tile / split），别的工作区树不动。
    #[test]
    fn the_focused_tree_can_change_without_touching_other_workspaces() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let tree_b_before = app.world().get::<Workspace>(b).unwrap().tree.clone();

        app.world_mut()
            .resource_mut::<FrameTree>()
            .0
            .split_leaf(PanelId::Transcript, Split::Horizontal, PanelId::Pane2);
        app.update();

        let tree_a = app.world().get::<Workspace>(a).unwrap().tree.clone();
        assert!(tree_a.leaves().contains(&PanelId::Pane2), "A 的树更新了");
        assert_eq!(
            app.world().get::<Workspace>(b).unwrap().tree,
            tree_b_before,
            "B 的树一个字节都不动"
        );

        // 切到 B：全局 FrameTree 装载 B 的树。
        focus_workspace(app.world_mut(), b);
        assert_eq!(app.world().resource::<FrameTree>().0, tree_b_before);
        assert!(!app
            .world()
            .resource::<FrameTree>()
            .0
            .leaves()
            .contains(&PanelId::Pane2));
    }

    // 14. 显式工作区 + FramePlugin 的默认屏幕：默认屏幕收编成第 0 个工作区，
    // 不能被当成孤儿，也不能让全局内容写进第一个显式工作区。
    #[test]
    fn the_default_screen_is_adopted_as_workspace_zero_alongside_explicit_ones() {
        if crate::niri::niri_layout() {
            return;
        }
        fn spawn_extra(mut commands: Commands, mut set: ResMut<WorkspaceSet>) {
            spawn_workspace(
                &mut commands,
                &mut set,
                1,
                Transform::from_xyz(0.0, -2.0, 0.0),
                1,
            );
        }

        let mut app = App::new();
        app.init_resource::<Selection>()
            .insert_resource(TranscriptFont(FontSource::default()))
            .add_plugins((FramePlugin, PanelsPlugin, WorkspacesPlugin));
        app.insert_resource(FrameSize { cols: 96, rows: 28 });
        app.add_systems(Startup, spawn_extra);
        app.update();

        let set = app.world().resource::<WorkspaceSet>();
        assert_eq!(set.items.len(), 2, "默认屏幕 + 一个显式工作区");
        let default = set.items[0];
        let extra = set.items[1];
        assert_eq!(
            set.focused,
            Some(default),
            "第 0 个会话（默认屏幕）是聚焦工作区"
        );
        assert_eq!(app.world().get::<Workspace>(default).unwrap().session, 0);
        assert_eq!(app.world().get::<Workspace>(default).unwrap().pose, Transform::IDENTITY);
        assert_eq!(app.world().get::<Workspace>(extra).unwrap().session, 1);
        assert_eq!(
            app.world().get::<Workspace>(default).unwrap().order,
            0
        );
        assert_eq!(
            app.world().get::<Workspace>(extra).unwrap().order,
            1
        );

        // 默认屏幕的面板归 default，显式工作区的面板仍归 extra。
        let default_transcript = root_of(&app, default, PanelId::Transcript);
        let extra_transcript = root_of(&app, extra, PanelId::Transcript);
        assert_ne!(default_transcript, extra_transcript);
        assert_eq!(
            app.world().get::<WorkspacePanel>(default_transcript).map(|tag| tag.0),
            Some(default)
        );
        assert_eq!(
            app.world().get::<WorkspacePanel>(extra_transcript).map(|tag| tag.0),
            Some(extra)
        );

        // 真会话的内容（全局代理）只进 default；extra 自己的行不受影响。
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(5, "session zero")]);
        app.update();
        assert_eq!(
            plain(&rows_of(&app, default).get(PanelId::Transcript)[0]),
            "session zero"
        );
        assert!(
            rows_of(&app, extra).get(PanelId::Transcript).is_empty(),
            "显式工作区不能收到默认会话的内容"
        );
        // 默认屏幕的帧在原点：锚点 = 帧内中心（pose 单位阵）。
        let anchor = app
            .world()
            .get::<feathers_in_3d::WorldPanel>(default_transcript)
            .unwrap()
            .anchor
            .translation;
        let want_x = -16.0 * CELL.x * UNITS_TO_WORLD;
        assert!((anchor.x - want_x).abs() < 1e-5, "anchor.x = {}", anchor.x);
        assert_eq!(anchor.y, 0.0);
    }

    // 12. 拆转写只改聚焦工作区：A 多出 Pane2，B 的树/根不变。
    #[test]
    fn splitting_the_transcript_only_changes_the_focused_workspace() {
        fn split_once(
            mut commands: Commands,
            mut tree: ResMut<FrameTree>,
            mut roots: ResMut<PanelRoots>,
            mut rows: ResMut<PanelRows>,
        ) -> bool {
            crate::frame::split_transcript(&mut commands, &mut tree, &mut roots, &mut rows)
        }

        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let tree_b_before = app.world().get::<Workspace>(b).unwrap().tree.clone();
        let roots_b_before = app.world().get::<WorkspacePanels>(b).unwrap().roots.clone();

        assert!(app.world_mut().run_system_once(split_once).unwrap());
        app.update();

        let panels_a = app.world().get::<WorkspacePanels>(a).unwrap();
        assert!(panels_a.roots.entity(PanelId::Pane2).is_some());
        let tree_a = app.world().get::<Workspace>(a).unwrap().tree.clone();
        assert!(tree_a.leaves().contains(&PanelId::Pane2));
        assert_eq!(app.world().get::<Workspace>(b).unwrap().tree, tree_b_before);
        assert_eq!(
            app.world().get::<WorkspacePanels>(b).unwrap().roots.0,
            roots_b_before.0
        );

        // 新拆出来的 Pane2 面板归当前聚焦工作区，并放到自己的位置。
        let pane2 = panels_a.roots.entity(PanelId::Pane2).unwrap();
        assert_eq!(
            app.world().get::<WorkspacePanel>(pane2).map(|tag| tag.0),
            Some(a)
        );
        assert!(panels_a.area(pane2).is_some());
    }

    // -----------------------------------------------------------------------
    // 第 3 批：世界层四个操作（§5 六条不变量）
    // -----------------------------------------------------------------------

    // 13. 自动整理：顺序 = order、竖直堆叠、间距一致、x/z = 0、朝向单位阵。
    #[test]
    fn arrange_stacks_workspaces_vertically_in_order() {
        fn scrambled(mut commands: Commands, mut set: ResMut<WorkspaceSet>) {
            spawn_workspace(
                &mut commands,
                &mut set,
                10,
                Transform::from_xyz(3.0, 4.0, 5.0),
                2,
            );
            spawn_workspace(
                &mut commands,
                &mut set,
                11,
                Transform::from_xyz(-1.0, -2.0, 0.5).with_rotation(Quat::from_rotation_y(0.3)),
                0,
            );
            spawn_workspace(
                &mut commands,
                &mut set,
                12,
                Transform::from_xyz(0.5, 0.5, 0.5),
                1,
            );
        }
        let mut app = workspace_app();
        app.add_systems(Startup, scrambled);
        app.update();

        let ordered = arrange_workspaces(app.world_mut());
        assert_eq!(ordered.len(), 3);
        let sessions: Vec<u64> = ordered
            .iter()
            .map(|entity| app.world().get::<Workspace>(*entity).unwrap().session)
            .collect();
        assert_eq!(sessions, vec![11, 12, 10], "整理后的顺序 = order");

        let frame = *app.world().resource::<FrameSize>();
        let step = workspace_step(frame);
        for (index, entity) in ordered.iter().enumerate() {
            let pose = pose_of(&app, *entity);
            assert_eq!(pose.translation.x, 0.0, "x 归 0");
            assert_eq!(pose.translation.z, 0.0, "z 归 0");
            let want_y = -(index as f32) * step;
            assert!(
                (pose.translation.y - want_y).abs() < 1e-6,
                "第 {index} 个 y = {}，期望 {want_y}",
                pose.translation.y
            );
            assert_eq!(pose.rotation, Quat::IDENTITY, "朝向统一为单位阵");
        }
        // 同一次整理里相邻工作区间距一致。
        for pair in ordered.windows(2) {
            let upper = pose_of(&app, pair[0]).translation.y;
            let lower = pose_of(&app, pair[1]).translation.y;
            assert!(
                (upper - lower - step).abs() < 1e-6,
                "相邻间距 {upper} - {lower} != {step}"
            );
        }
    }

    // 14. 整理只重置世界位姿，不动树的内部布局（区域/尺寸/树）。
    #[test]
    fn arrange_resets_positions_without_touching_the_inner_layout() {
        let mut app = app_with_two_workspaces();
        let (a, _) = workspaces(&app);
        let tree = app.world().get::<Workspace>(a).unwrap().tree.clone();
        let areas = app
            .world()
            .get::<WorkspacePanels>(a)
            .unwrap()
            .areas
            .clone();
        let sizes = panel_sizes(&app, a);

        arrange_workspaces(app.world_mut());
        assert_eq!(
            pose_of(&app, a).translation,
            Vec3::ZERO,
            "order 0 的工作区回到原点（手放的位置被覆盖）"
        );
        app.update();

        assert_eq!(app.world().get::<Workspace>(a).unwrap().tree, tree, "树不动");
        assert_eq!(
            app.world().get::<WorkspacePanels>(a).unwrap().areas,
            areas,
            "帧内区域不动"
        );
        assert_eq!(panel_sizes(&app, a), sizes, "面板尺寸不动");
    }

    // 15. 整理步长 = 帧高 × 行高 × 单位换算 + niri 默认 gap 16px。
    #[test]
    fn arrange_uses_the_frame_height_and_the_niri_gap() {
        let frame = FrameSize { cols: 96, rows: 28 };
        let strip = workspace_strip_height(frame);
        assert!(
            (strip - 28.0 * crate::transcript::ROW_HEIGHT * UNITS_TO_WORLD).abs() < 1e-6,
            "条带高度 = 帧高 × 行高 × 单位换算"
        );
        assert!(
            (WORKSPACE_GAP - 16.0 * UNITS_TO_WORLD).abs() < 1e-9,
            "间距 = niri 默认 gap 16px"
        );
        assert!((workspace_step(frame) - strip - WORKSPACE_GAP).abs() < 1e-6);
        assert!(
            workspace_step(FrameSize { cols: 96, rows: 40 }) > workspace_step(frame),
            "帧越高步长越大"
        );
    }

    // 16. 放置：只改目标工作区，别人像素级不动。
    #[test]
    fn place_workspace_moves_only_the_target() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let pose_a = pose_of(&app, a);
        let anchors_a = panel_anchors(&app, a);
        let pose_b =
            Transform::from_xyz(-2.0, 0.5, 0.25).with_rotation(Quat::from_rotation_y(0.4));

        assert!(place_workspace(app.world_mut(), b, pose_b));
        assert_eq!(pose_of(&app, b), pose_b);
        assert_eq!(pose_of(&app, a), pose_a, "A 的位姿不动");

        app.update();
        assert_eq!(panel_anchors(&app, a), anchors_a, "A 的面板锚点不动");
        assert!(
            !place_workspace(app.world_mut(), Entity::PLACEHOLDER, pose_b),
            "不存在的实体返回 false"
        );
    }

    // 17. 拖拽一步（世界位移）：只改被拖的，其余像素级不动。
    #[test]
    fn drag_workspace_moves_only_the_target() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let pose_a = pose_of(&app, a);
        let pose_b = pose_of(&app, b);
        let anchors_b = panel_anchors(&app, b);
        let sizes_b = panel_sizes(&app, b);

        let delta = Vec3::new(0.3, -0.2, 0.1);
        assert!(drag_workspace(app.world_mut(), a, delta));
        assert_eq!(pose_of(&app, a).translation, pose_a.translation + delta);
        assert_eq!(pose_of(&app, b), pose_b, "B 的位姿不动");

        app.update();
        assert_eq!(panel_anchors(&app, b), anchors_b, "B 的面板锚点像素级不动");
        assert_eq!(panel_sizes(&app, b), sizes_b, "B 的面板尺寸不动");
    }

    // 18. 新工作区出现不改变已有工作区的位姿/尺寸（不变量 1）。
    #[test]
    fn a_new_workspace_does_not_touch_the_existing_ones() {
        fn spawn_third(mut commands: Commands, mut set: ResMut<WorkspaceSet>) {
            spawn_workspace(
                &mut commands,
                &mut set,
                3,
                Transform::from_xyz(9.0, 9.0, 9.0),
                2,
            );
        }

        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let poses = (pose_of(&app, a), pose_of(&app, b));
        let areas = (
            app.world().get::<WorkspacePanels>(a).unwrap().areas.clone(),
            app.world().get::<WorkspacePanels>(b).unwrap().areas.clone(),
        );
        let roots = (
            app.world().get::<WorkspacePanels>(a).unwrap().roots.0.clone(),
            app.world().get::<WorkspacePanels>(b).unwrap().roots.0.clone(),
        );
        let anchors = (panel_anchors(&app, a), panel_anchors(&app, b));
        let sizes = (panel_sizes(&app, a), panel_sizes(&app, b));

        assert!(app.world_mut().run_system_once(spawn_third).is_ok());
        app.update();

        assert_eq!(app.world().resource::<WorkspaceSet>().items.len(), 3);
        assert_eq!(pose_of(&app, a), poses.0, "A 的位姿不变");
        assert_eq!(pose_of(&app, b), poses.1, "B 的位姿不变");
        assert_eq!(
            app.world().get::<WorkspacePanels>(a).unwrap().areas,
            areas.0,
            "A 的区域不变"
        );
        assert_eq!(
            app.world().get::<WorkspacePanels>(b).unwrap().areas,
            areas.1,
            "B 的区域不变"
        );
        assert_eq!(
            app.world().get::<WorkspacePanels>(a).unwrap().roots.0,
            roots.0
        );
        assert_eq!(
            app.world().get::<WorkspacePanels>(b).unwrap().roots.0,
            roots.1
        );
        assert_eq!(panel_anchors(&app, a), anchors.0);
        assert_eq!(panel_anchors(&app, b), anchors.1);
        assert_eq!(panel_sizes(&app, a), sizes.0);
        assert_eq!(panel_sizes(&app, b), sizes.1);
    }

    // 19. 聚焦/取消聚焦**什么都不改**：不隐藏工作区、不动位姿。
    //
    // niri 的工作区是竖直摆着的，靠视口移动去看，不靠隐藏；聚焦只决定"内容写哪个
    // 工作区"（见 `commit_focused_panels`）。所以可见性永远 `Inherited`。
    #[test]
    fn focus_changes_nothing_visible() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        for id in [
            PanelId::Transcript,
            PanelId::Input,
            PanelId::Status,
            PanelId::Complete,
        ] {
            assert_eq!(
                visibility_of(&app, a, id),
                Visibility::Inherited,
                "A 的 {id:?} 可见"
            );
            assert_eq!(
                visibility_of(&app, b, id),
                Visibility::Inherited,
                "B 的 {id:?} 也可见（不隐藏工作区）"
            );
        }

        let poses = (pose_of(&app, a), pose_of(&app, b));
        let anchors = (panel_anchors(&app, a), panel_anchors(&app, b));

        focus_workspace(app.world_mut(), b);
        app.update();
        assert_eq!(visibility_of(&app, a, PanelId::Transcript), Visibility::Inherited);
        assert_eq!(visibility_of(&app, b, PanelId::Transcript), Visibility::Inherited);
        assert_eq!((pose_of(&app, a), pose_of(&app, b)), poses, "位姿不变");
        assert_eq!(
            (panel_anchors(&app, a), panel_anchors(&app, b)),
            anchors,
            "锚点不变"
        );
    }

    // 20. 聚焦不动相机（用户要求"无特殊动作"）。
    #[test]
    fn focus_does_not_move_the_camera() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let camera = app
            .world_mut()
            .spawn((
                Camera::default(),
                GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 5.0)),
            ))
            .id();
        let before = *app
            .world()
            .get::<GlobalTransform>(camera)
            .expect("相机位姿");

        focus_workspace(app.world_mut(), b);
        app.update();
        assert_eq!(*app.world().get::<GlobalTransform>(camera).unwrap(), before);
        focus_workspace(app.world_mut(), a);
        app.update();
        assert_eq!(
            *app.world().get::<GlobalTransform>(camera).unwrap(),
            before,
            "聚焦/取消聚焦都不动相机"
        );
    }

    // 21. 屏幕位移 -> 世界位移：假相机的 view-projection 逆变换 + 过中心的平面。
    #[test]
    fn drag_world_delta_unprojects_through_the_workspace_plane() {
        let clip_from_view = bevy::math::proj::orthographic(-2.0, 2.0, -2.0, 2.0, 0.1, 100.0);
        let world_from_view = GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 5.0));
        let clip_from_world = clip_from_view * Mat4::from(world_from_view.affine().inverse());

        // 视线与过原点的平面（法向 Z）求交：ndc 差 (0.2, 0.2) -> 世界差 (0.4, 0.4)。
        let delta = drag_world_delta(
            clip_from_world,
            Vec2::new(0.2, 0.0),
            Vec2::new(0.4, 0.2),
            Vec3::ZERO,
            Vec3::Z,
        )
        .expect("视线与平面不平行");
        assert!(
            (delta - Vec3::new(0.4, 0.4, 0.0)).length() < 1e-5,
            "delta = {delta:?}"
        );

        // 平面过工作区中心（不是原点）时公式同样成立。
        let delta = drag_world_delta(
            clip_from_world,
            Vec2::new(-0.2, 0.0),
            Vec2::new(-0.1, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::Z,
        )
        .expect("视线与平面不平行");
        assert!((delta.x - 0.2).abs() < 1e-5, "delta.x = {}", delta.x);

        // 视线与平面平行：没有交点。
        assert!(
            drag_world_delta(clip_from_world, Vec2::ZERO, Vec2::ZERO, Vec3::ZERO, Vec3::X)
                .is_none()
        );
    }

    // 22. 拖拽系统：中键拖动只改被拖的工作区，位移 = 反算的世界位移。
    #[test]
    fn middle_drag_moves_only_the_dragged_workspace() {
        let mut fixture = drag_app();
        let (a, b) = workspaces(&fixture.app);
        assert_eq!(fixture.app.world().resource::<WorkspaceSet>().focused, Some(a));
        let pose_b = pose_of(&fixture.app, b);
        let anchors_a = panel_anchors(&fixture.app, a);
        let anchors_b = panel_anchors(&fixture.app, b);
        let sizes_b = panel_sizes(&fixture.app, b);

        fixture.set_cursor_ndc(Vec2::new(0.2, 0.0));
        fixture.press(MouseButton::Middle);
        fixture.update();
        assert_eq!(fixture.dragging(), Some(a), "中键按在 A 的条带里开始拖 A");

        fixture.set_cursor_ndc(Vec2::new(0.4, 0.2));
        fixture.update();
        let pose_a = pose_of(&fixture.app, a);
        let delta = pose_a.translation - Vec3::ZERO;
        assert!(
            (delta - Vec3::new(0.4, 0.4, 0.0)).length() < 1e-4,
            "pose 位移 = 反算的世界位移，实际 {delta:?}"
        );
        assert_eq!(pose_a.rotation, Quat::IDENTITY, "拖拽只改平移");

        assert_eq!(pose_of(&fixture.app, b), pose_b, "B 的位姿不动");
        assert_eq!(
            panel_anchors(&fixture.app, b),
            anchors_b,
            "B 的面板锚点像素级不动"
        );
        assert_eq!(panel_sizes(&fixture.app, b), sizes_b);
        for ((id, before), (after_id, after)) in
            anchors_a.iter().zip(panel_anchors(&fixture.app, a))
        {
            assert_eq!(*id, after_id);
            assert!(
                (after - *before - delta).length() < 1e-4,
                "A 的 {id:?} 锚点整体平移 delta"
            );
        }
    }

    // 23. 松开后位置保留，不会自动弹回（不变量 6 的后半）。
    #[test]
    fn releasing_a_drag_keeps_the_dropped_position() {
        let mut fixture = drag_app();
        let (a, _) = workspaces(&fixture.app);
        fixture.set_cursor_ndc(Vec2::new(0.2, 0.0));
        fixture.press(MouseButton::Middle);
        fixture.update();
        fixture.set_cursor_ndc(Vec2::new(0.4, 0.0));
        fixture.update();
        let dropped = pose_of(&fixture.app, a);
        assert!(dropped.translation.x > 0.3, "已经拖动了");

        fixture.release(MouseButton::Middle);
        fixture.update();
        assert_eq!(fixture.dragging(), None, "松开结束拖拽");
        assert_eq!(pose_of(&fixture.app, a), dropped, "位置留在松手处");

        fixture.update();
        fixture.update();
        assert_eq!(pose_of(&fixture.app, a), dropped, "不自动弹回");
    }

    // 24. Alt+左键也能拖；普通左键不拖工作区（留给选字）。
    #[test]
    fn alt_left_drags_while_plain_left_does_not() {
        let mut fixture = drag_app();
        let (a, _) = workspaces(&fixture.app);
        let pose = pose_of(&fixture.app, a);

        fixture.set_cursor_ndc(Vec2::new(0.0, 0.0));
        fixture.press(MouseButton::Left);
        fixture.update();
        assert_eq!(fixture.dragging(), None, "普通左键是选字，不是拖工作区");
        fixture.set_cursor_ndc(Vec2::new(0.2, 0.0));
        fixture.update();
        assert_eq!(pose_of(&fixture.app, a), pose, "普通左键不移动工作区");
        fixture.release(MouseButton::Left);
        fixture.update();

        fixture
            .app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::AltLeft);
        fixture.set_cursor_ndc(Vec2::new(0.0, 0.0));
        fixture.press(MouseButton::Left);
        fixture.update();
        assert_eq!(fixture.dragging(), Some(a), "Alt+左键开始拖工作区");
        fixture.set_cursor_ndc(Vec2::new(0.2, 0.0));
        fixture.update();
        assert!(
            pose_of(&fixture.app, a).translation.x > 0.3,
            "工作区跟着指针走"
        );
    }

    // 25. 隐藏的工作区不能被指针选中（拖不到）。
    #[test]
    fn a_hidden_workspace_cannot_be_dragged() {
        let mut fixture = drag_app();
        let (_, b) = workspaces(&fixture.app);
        let pose_b = pose_of(&fixture.app, b);
        // B 在 y = -1：屏幕 NDC y = -0.5 正对着它。
        fixture.set_cursor_ndc(Vec2::new(0.0, -0.5));
        fixture.press(MouseButton::Middle);
        fixture.update();
        assert_eq!(fixture.dragging(), None, "B 隐藏着，按不到");
        fixture.set_cursor_ndc(Vec2::new(0.2, -0.5));
        fixture.update();
        assert_eq!(pose_of(&fixture.app, b), pose_b, "隐藏工作区一个像素都不动");
    }

    // 27. 相机在空间里动，工作区位姿不变（世界坐标，不是相对相机；不变量 5）。
    #[test]
    fn moving_the_camera_does_not_move_the_workspaces() {
        let mut fixture = drag_app();
        let (a, b) = workspaces(&fixture.app);
        let poses = (pose_of(&fixture.app, a), pose_of(&fixture.app, b));
        let anchors = (panel_anchors(&fixture.app, a), panel_anchors(&fixture.app, b));

        *fixture
            .app
            .world_mut()
            .get_mut::<GlobalTransform>(fixture.camera)
            .expect("假相机")
            = GlobalTransform::from(Transform::from_xyz(2.0, 1.0, 4.0));
        fixture.update();

        assert_eq!(
            (pose_of(&fixture.app, a), pose_of(&fixture.app, b)),
            poses,
            "相机移动不改工作区 pose（绝对世界坐标）"
        );
        assert_eq!(
            (panel_anchors(&fixture.app, a), panel_anchors(&fixture.app, b)),
            anchors,
            "相机移动不改面板锚点"
        );
    }

    // 26. 整理会把拖过的位置重置回 order 对应的竖直序列。
    #[test]
    fn arrange_resets_a_dragged_workspace() {
        let mut fixture = drag_app();
        let (a, _) = workspaces(&fixture.app);
        fixture.set_cursor_ndc(Vec2::new(0.2, 0.0));
        fixture.press(MouseButton::Middle);
        fixture.update();
        fixture.set_cursor_ndc(Vec2::new(0.4, 0.2));
        fixture.update();
        assert_ne!(pose_of(&fixture.app, a).translation, Vec3::ZERO, "先拖开");

        // 松开再整理：整理是覆盖手放坐标的，不是"拖拽中"能一直压着的。
        fixture.release(MouseButton::Middle);
        fixture.update();
        arrange_workspaces(fixture.app.world_mut());
        fixture.update();
        let pose = pose_of(&fixture.app, a);
        assert_eq!(
            pose.translation,
            Vec3::ZERO,
            "A 是 order 0，整理后回到竖直序列的第 0 位"
        );
        assert_eq!(pose.rotation, Quat::IDENTITY);
    }
}
