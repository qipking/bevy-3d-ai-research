//! 插件面板画布：注册一块自己的面板、放进工作区条带、拿根节点画 UI、读回行。
//!
//! rig-tui 里插件通过 `TuiPanel` + `PanelCanvas` 往自己的面板上画（终端的
//! `Buffer`）；我们的面板是 **bevy_ui 子树根**，3D 副本由 `feathers_in_3d`
//! 把整棵子树镜像出来。所以插件不用再学一套“画布 API”：
//!
//! ```text
//! PanelRegistry::register("git")            // 一次，拿句柄
//! add_panel_column(world, workspace, h, .3) // 放进条带，拿根实体
//! world.entity_mut(root).with_children(..)  // 自己 spawn Node/Text/ImageNode
//! drawn_row(world, workspace, h.id(), 2)    // 读回第 2 行的世界矩形
//! ```
//!
//! 这一层只做“注册 + 放进条带 + 读回”，**不**做画布门控/重绘请求：
//! `bevy_ui` 自己每帧重排，`feathers_in_3d` 每帧（`PostUpdate`）重新镜像，
//! 不需要上游 `RequestRedraw`/`RedrawArea` 那套终端优化。弹层、自由坐标、
//! 逐格重绘也都不在这里（见 `docs/design/PANEL_CANVAS.md` 的“不做”一节）。
//!
//! ## 与内置面板的关系
//!
//! 插件面板在树里就是一个普通叶子，`PanelId::Plugin(handle)` 是它的键；
//! `frame::lay_out_frame` / `place_panels` / `panels::sync_panels` 走的是和
//! 内置叶子完全相同的路径。因此内置 `PanelId` 的行为、几何、行同步一个字节
//! 都不变——只有在插件真的调用 [`add_panel_column`] 时，才多出一列。
//!
//! ## 行是哪来的
//!
//! 插件有两条画法，可以混用：
//!
//! 1. **自己 spawn 子树**（推荐）：拿根实体，`Node`/`Text`/`ImageNode` 随便画；
//!    3D 镜像不挑组件，子树里有什么就镜像什么。
//! 2. **写行数组**：把 [`KeyedRow`] 写进 [`PanelRows`]（聚焦工作区）或
//!    `WorkspacePanels::rows`（任意工作区），[`crate::panels::sync_panels`] 会
//!    和内置面板一样做增量 diff，并给每行挂 [`RowLayout`]——这样
//!    [`drawn_row`] 才读得到“第 n 行在哪”。
//!
//! 切片 1 实现“注册 + 列 + 根 + 行读回”；切片 2（2026-11-26）加了
//! [`PanelAuto::AllWorkspaces`] 自动工作区策略、[`remove_panel_everywhere`] /
//! [`unregister_panel`] 的整树移除、[`ActivePanelColumn`] 插件列焦点；
//! 任意面板底部的状态行在 [`crate::status::StatusLine`]（复用同一份来源/裁剪）。
//! 设计见 `docs/design/PANEL_CANVAS.md`。

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_ui::JustifyContent;

use feathers_in_3d::WorldPanel;

use crate::frame::{
    CELL, FramePanel, FrameSize, FrameTree, Look, PanelArea, PanelAreas, PanelInset,
};
use crate::layout::{self, Node as LayoutNode};
use crate::panels::{
    PanelId, PanelRoots, PanelRows, PanelSpawned, PluginPanel, RowLayout,
};
use crate::theme::{PANEL_BG, PANEL_BORDER as PANEL_BORDER_TOKEN};
use crate::transcript::ROW_HEIGHT;
use crate::workspaces::{
    Workspace, WorkspacePanels, WorkspaceScroll, WorkspaceScrollFocus, WorkspaceSet,
};

/// 注册一块插件面板时，它和“工作区”的关系（切片 2）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelAuto {
    /// 只在插件显式 [`add_panel_column`] 时放进某个工作区。
    #[default]
    Manual,
    /// 自动装进**所有**工作区；之后 `spawn_workspace` 创建的工作区也会带上
    /// （[`apply_auto_panels`] 每帧补齐）。
    AllWorkspaces,
}

/// 一块插件面板的注册规格：工作区策略 + 自动列的宽度。
///
/// [`PanelSpec::everywhere`] 是最常用的构造：`fraction × 视口宽`，所有工作区
/// （含以后新建的）各一列；已有列的宽度/位置不改（niri 第一原则）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelSpec {
    /// 自动列的宽度（视口比例）；非法值退回 [`layout::DEFAULT_COLUMN_FRACTION`]。
    pub fraction: f32,
    /// 放哪些工作区。
    pub auto: PanelAuto,
}

impl Default for PanelSpec {
    fn default() -> Self {
        Self {
            fraction: layout::DEFAULT_COLUMN_FRACTION,
            auto: PanelAuto::Manual,
        }
    }
}

impl PanelSpec {
    /// 自动装进所有工作区（含以后新建的），列宽 = `fraction × 视口宽`。
    pub fn everywhere(fraction: f32) -> Self {
        Self {
            fraction: sanitize_fraction(fraction),
            auto: PanelAuto::AllWorkspaces,
        }
    }

    /// 只手动放置（默认）。
    pub fn manual(fraction: f32) -> Self {
        Self {
            fraction: sanitize_fraction(fraction),
            auto: PanelAuto::Manual,
        }
    }
}

/// 列宽的比例清洗：`NaN`/`<= 0` 用默认值，超过 1 夹到 1。
fn sanitize_fraction(fraction: f32) -> f32 {
    if fraction.is_finite() && fraction > 0.0 {
        fraction.min(1.0)
    } else {
        layout::DEFAULT_COLUMN_FRACTION
    }
}

/// 插件面板的注册表：名字 → 句柄。
///
/// 由 [`crate::panels::PanelsPlugin`] 初始化，也可以在插件 `build` 里用
/// [`PanelRegistryExt::register_panel`] 直接拿（缺资源时自动建）。
///
/// 注册是**幂等**的：同名重复注册返回同一个句柄，不会多出一块面板；句柄在
/// 一个 App 的生命周期里唯一。注册本身不放置面板——放置是
/// [`add_panel_column`]，因为“放哪个工作区”只有插件知道。
#[derive(Resource, Default, Clone, Debug)]
pub struct PanelRegistry {
    next: u64,
    by_name: HashMap<String, PluginPanel>,
    names: HashMap<PluginPanel, String>,
    /// 每块面板的注册规格（自动工作区策略 + 列宽）。`register_panel` 默认
    /// [`PanelAuto::Manual`]；[`PanelRegistryExt::register_panel_with`] 可以带
    /// [`PanelAuto::AllWorkspaces`]。
    specs: HashMap<PluginPanel, PanelSpec>,
}

impl PanelRegistry {
    /// 注册（或取回）名字为 `name` 的面板，返回句柄；默认不自动放置。
    ///
    /// 空名字会生成一个稳定于本次注册的 `plugin-panel-<n>`，不会 panic。
    pub fn register(&mut self, name: impl Into<String>) -> PluginPanel {
        let panel = self.register_inner(name);
        // 已经带自动策略时，再次 `register` 不把它降回 Manual。
        self.specs.entry(panel).or_insert_with(PanelSpec::default);
        panel
    }

    /// 同 [`Self::register`]，但带上 [`PanelSpec`]（自动装进所有工作区等）。
    pub fn register_with(&mut self, name: impl Into<String>, spec: PanelSpec) -> PluginPanel {
        let panel = self.register_inner(name);
        self.specs.insert(panel, spec);
        panel
    }

    fn register_inner(&mut self, name: impl Into<String>) -> PluginPanel {
        let mut name = name.into();
        if name.is_empty() {
            name = format!("plugin-panel-{}", self.next);
        }
        if let Some(existing) = self.by_name.get(&name) {
            return *existing;
        }
        let panel = PluginPanel(self.next);
        self.next = self.next.wrapping_add(1);
        self.by_name.insert(name.clone(), panel);
        self.names.insert(panel, name);
        panel
    }

    /// 句柄对应的名字。
    pub fn name(&self, panel: PluginPanel) -> Option<&str> {
        self.names.get(&panel).map(String::as_str)
    }

    /// 这个句柄是不是本注册表发出去的。
    pub fn contains(&self, panel: PluginPanel) -> bool {
        self.names.contains_key(&panel)
    }

    /// 这块面板的注册规格。
    pub fn spec(&self, panel: PluginPanel) -> Option<PanelSpec> {
        self.specs.get(&panel).copied()
    }

    /// 改一块已注册面板的规格（比如运行时改成“装进所有工作区”）。
    pub fn set_spec(&mut self, panel: PluginPanel, spec: PanelSpec) -> bool {
        if !self.contains(panel) {
            return false;
        }
        self.specs.insert(panel, spec);
        true
    }

    /// 所有“自动装进所有工作区”的面板，按句柄排序（放置顺序确定）。
    pub fn auto_panels(&self) -> Vec<(PluginPanel, PanelSpec)> {
        let mut panels: Vec<(PluginPanel, PanelSpec)> = self
            .specs
            .iter()
            .filter(|(_, spec)| spec.auto == PanelAuto::AllWorkspaces)
            .map(|(panel, spec)| (*panel, *spec))
            .collect();
        panels.sort_by_key(|(panel, _)| panel.0);
        panels
    }

    /// 注销一块面板：名字、句柄、规格一起删掉。已经放进工作区的列由
    /// [`unregister_panel`] 摘除（本方法只改注册表）。
    pub fn unregister(&mut self, panel: PluginPanel) -> bool {
        let Some(name) = self.names.remove(&panel) else {
            return false;
        };
        self.by_name.remove(&name);
        self.specs.remove(&panel);
        true
    }

    /// 已注册的面板数量。
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// 是否一个都没注册。
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// 在 [`App`] 上注册插件面板的便捷入口（内部用 `get_resource_or_init`，
/// 所以插件顺序不影响：先注册、后装 `PanelsPlugin` 也不会把注册表盖掉）。
pub trait PanelRegistryExt {
    /// 注册（或取回）名字为 `name` 的面板；默认不自动放置。
    fn register_panel(&mut self, name: impl Into<String>) -> PluginPanel;

    /// 同 [`Self::register_panel`]，但带 [`PanelSpec`]（自动装进所有工作区等）。
    fn register_panel_with(&mut self, name: impl Into<String>, spec: PanelSpec) -> PluginPanel;
}

impl PanelRegistryExt for App {
    fn register_panel(&mut self, name: impl Into<String>) -> PluginPanel {
        self.world_mut()
            .get_resource_or_init::<PanelRegistry>()
            .register(name)
    }

    fn register_panel_with(&mut self, name: impl Into<String>, spec: PanelSpec) -> PluginPanel {
        self.world_mut()
            .get_resource_or_init::<PanelRegistry>()
            .register_with(name, spec)
    }
}

/// 把一块**已注册**的面板作为新列放进 `workspace` 的 niri 条带，返回根实体。
///
/// * 位置：追加到包含 [`PanelId::Transcript`] 的最外层横向条带末尾（和 picker
///   开列同一条路径），列宽是 `fraction × 视口宽`（[`crate::layout::Size::Fraction`]）。
///   **已有列的宽度和 x 一个字节都不改**（niri 第一原则）。
/// * 幂等：同一块面板已经在这棵树里时，直接返回已有的根实体，不再开列。
/// * 找不到工作区 / 没注册的句柄 / 树里没有转写条带：返回 `None`，只打一行
///   `warn!`，不 panic。
///
/// 新列通常落在视口右边（默认树的两列已经占满一屏），需要滚动才看得见；
/// [`reveal_panel_column`] 把这一列滚进视口。面板根已经挂好
/// [`crate::workspaces::WorkspacePanel`] 标签，下一帧 `Layout` 就会拿到区域。
///
/// 这是 `&mut World` API：在 exclusive system 里直接调用，或在普通系统里
/// `commands.queue(move |world: &mut World| { add_panel_column(world, ..); })`。
pub fn add_panel_column(
    world: &mut World,
    workspace: Entity,
    panel: PluginPanel,
    fraction: f32,
) -> Option<Entity> {
    let registered = world
        .get_resource::<PanelRegistry>()
        .is_some_and(|registry| registry.contains(panel));
    if !registered {
        warn!("panel: 拒绝放置未注册的插件面板 {panel:?}");
        return None;
    }
    if world.get::<Workspace>(workspace).is_none()
        || world.get::<WorkspacePanels>(workspace).is_none()
    {
        warn!("panel: 工作区 {workspace:?} 没有 Workspace/WorkspacePanels，拒绝放置 {panel:?}");
        return None;
    }

    let id = panel.id();
    let mut tree = world.get::<Workspace>(workspace)?.tree.clone();
    if tree.leaves().contains(&id) {
        return world.get::<WorkspacePanels>(workspace)?.roots.entity(id);
    }

    // 先在树的工作副本上加列：失败时不留下面板实体，也不改原树。
    if !tree.append_column_at(PanelId::Transcript, LayoutNode::leaf(id)) {
        warn!("panel: 工作区 {workspace:?} 的条带里没有转写列，放不下 {panel:?}");
        return None;
    }
    let fraction = sanitize_fraction(fraction);
    if !tree.set_column_fraction(id, fraction) {
        warn!("panel: 新列不是 Fraction 列，拒绝放置 {panel:?}");
        return None;
    }

    // 面板根走和内置面板同一个 `spawn_panel`：chrome / MirrorRoot / WorldPanel
    // / PanelId 组件一次装好。
    let root = {
        let mut commands = world.commands();
        crate::frame::spawn_panel(
            &mut commands,
            id,
            FramePanel::tree(),
            Look::panel(PANEL_BG, PANEL_BORDER_TOKEN),
        )
    };
    world.flush();
    world.entity_mut(root).insert(crate::workspaces::WorkspacePanel(workspace));

    if let Some(mut component) = world.get_mut::<Workspace>(workspace) {
        component.tree = tree.clone();
    }
    if let Some(mut panels) = world.get_mut::<WorkspacePanels>(workspace) {
        panels.roots.0.insert(id, root);
        panels.rows.0.entry(id).or_default();
        panels.spawned.0.entry(id).or_default();
    }

    // 聚焦工作区的全局代理是内容系统的写入口径：同一帧就要能看到新列，
    // 否则下一次 `commit_focused_panels` 会把组件写回成没有新列的旧全局副本。
    let focused = world
        .get_resource::<WorkspaceSet>()
        .is_some_and(|set| set.focused == Some(workspace));
    if focused {
        if let Some(mut resource) = world.get_resource_mut::<FrameTree>() {
            resource.0 = tree;
        }
        if let Some(mut roots) = world.get_resource_mut::<PanelRoots>() {
            roots.0.insert(id, root);
        }
        if let Some(mut rows) = world.get_resource_mut::<PanelRows>() {
            rows.0.entry(id).or_default();
        }
        if let Some(mut spawned) = world.get_resource_mut::<PanelSpawned>() {
            spawned.0.entry(id).or_default();
        }
    }
    Some(root)
}

// ---------------------------------------------------------------------------
// 切片 2：自动工作区策略 / 移除 / 活动列
// ---------------------------------------------------------------------------

/// 当前工作区列表（没有 `WorkspaceSet` 时为空）。
fn all_workspaces(world: &World) -> Vec<Entity> {
    world
        .get_resource::<WorkspaceSet>()
        .map(|set| set.items.clone())
        .unwrap_or_default()
}

/// 把一块**已注册**面板装进**所有**工作区（含以后新建的），返回这次新加的列数。
///
/// 等价于“运行时打开这个插件面板”：把注册规格改成
/// [`PanelAuto::AllWorkspaces`]（之后 [`apply_auto_panels`] 会继续补齐新工作区），
/// 再对每个还没有这列的工作区调用 [`add_panel_column`]。列宽 = `fraction × 视口`，
/// **已有列的宽度/位置一个字节不改**。
///
/// 未注册句柄返回 `0`（只 `warn!`）；重复调用是幂等的（已有列不重复加，
/// 返回 `0`）。
pub fn add_panel_everywhere(world: &mut World, panel: PluginPanel, fraction: f32) -> usize {
    let registered = world
        .get_resource::<PanelRegistry>()
        .is_some_and(|registry| registry.contains(panel));
    if !registered {
        warn!("panel: 拒绝自动放置未注册的插件面板 {panel:?}");
        return 0;
    }
    let fraction = sanitize_fraction(fraction);
    if let Some(mut registry) = world.get_resource_mut::<PanelRegistry>() {
        registry.set_spec(
            panel,
            PanelSpec {
                fraction,
                auto: PanelAuto::AllWorkspaces,
            },
        );
    }
    let mut added = 0;
    for workspace in all_workspaces(world) {
        let present = world
            .get::<Workspace>(workspace)
            .is_some_and(|workspace| workspace.tree.leaves().contains(&panel.id()));
        if !present && add_panel_column(world, workspace, panel, fraction).is_some() {
            added += 1;
        }
    }
    added
}

/// 把注册表里所有 [`PanelAuto::AllWorkspaces`] 的面板补齐到每个工作区。
///
/// 注册时（`register_panel_with`）通常还没有工作区，所以这里每帧检查一次：
/// * 已有 N 个工作区在注册后的第一帧全部带上该列；
/// * 之后 `spawn_workspace` / `spawn_workspace_with_tree` 新建的工作区下一帧也带上；
/// * 已有该列的工作区不重复加（[`add_panel_column`] 本身也幂等）。
///
/// 这是 exclusive system（要 `&mut World` 调 `add_panel_column`），挂在
/// [`crate::workspaces::WorkspacesPlugin`] 的 `Prepare`，排在 `apply_focus` 之后。
pub fn apply_auto_panels(world: &mut World) {
    let Some(registry) = world.get_resource::<PanelRegistry>() else {
        return;
    };
    let panels = registry.auto_panels();
    if panels.is_empty() {
        return;
    }
    for workspace in all_workspaces(world) {
        for (panel, spec) in &panels {
            let present = world
                .get::<Workspace>(workspace)
                .is_some_and(|workspace| workspace.tree.leaves().contains(&panel.id()));
            if !present {
                add_panel_column(world, workspace, *panel, spec.fraction);
            }
        }
    }
}

/// 从**一个**工作区里摘掉插件列（关闭/注销时用），返回是否真的摘了。
///
/// 只动包含这块面板的那一列（[`LayoutNode::remove_column`] 语义）：其它列的
/// 宽度、x 一个字节不改；面板块从 `WorkspacePanels` 的根/行/区域表里移除并
/// despawn；如果它是聚焦工作区的列，同一帧同步全局 `FrameTree` / `PanelRoots`
/// / `PanelRows` / `PanelSpawned`（否则下一次 `commit_focused_panels` 会把旧
/// 列写回去）。活动列如果正指着它，一并清空。
pub fn remove_panel_column(
    world: &mut World,
    workspace: Entity,
    panel: PluginPanel,
) -> bool {
    let id = panel.id();
    let tree = {
        let Some(mut component) = world.get_mut::<Workspace>(workspace) else {
            return false;
        };
        if !component.tree.leaves().contains(&id) {
            return false;
        }
        // 插件列是条带里的一个整列；`remove_column` 不动兄弟列的尺寸。
        if !component.tree.remove_column(id) {
            // 兜底：树不是条带（只有一块面板）时按叶子摘。
            if !component.tree.remove_leaf(id) {
                return false;
            }
        }
        component.tree.clone()
    };
    let root = world
        .get::<WorkspacePanels>(workspace)
        .and_then(|panels| panels.roots.entity(id));

    if let Some(mut panels) = world.get_mut::<WorkspacePanels>(workspace) {
        panels.roots.0.remove(&id);
        panels.rows.0.remove(&id);
        panels.spawned.forget(id);
        if let Some(root) = root {
            panels.areas.remove(&root);
        }
    }

    // 聚焦工作区的全局代理是布局/内容系统的读入口：同一帧就更新。
    let focused = world
        .get_resource::<WorkspaceSet>()
        .is_some_and(|set| set.focused == Some(workspace));
    if focused {
        if let Some(mut resource) = world.get_resource_mut::<FrameTree>() {
            resource.0 = tree;
        }
        let global_root = root.filter(|root| {
            world
                .get_resource::<PanelRoots>()
                .and_then(|roots| roots.entity(id))
                == Some(*root)
        });
        if global_root.is_some() {
            if let Some(mut roots) = world.get_resource_mut::<PanelRoots>() {
                roots.0.remove(&id);
            }
            if let Some(mut rows) = world.get_resource_mut::<PanelRows>() {
                rows.0.remove(&id);
            }
            if let Some(mut spawned) = world.get_resource_mut::<PanelSpawned>() {
                spawned.forget(id);
            }
        }
    }

    if let Some(mut active) = world.get_resource_mut::<ActivePanelColumn>()
        && active.workspace == Some(workspace)
        && active.panel == Some(panel)
    {
        *active = ActivePanelColumn::default();
    }

    if let Some(root) = root {
        world.entity_mut(root).despawn();
    }
    true
}

/// 把插件列从**所有**工作区里摘掉（关闭插件面板），返回摘掉的工作区数。
///
/// 同时把它的注册规格降回 [`PanelAuto::Manual`]，否则下一帧
/// [`apply_auto_panels`] 又会在“自动装进所有工作区”的策略下把它加回来。
/// 注册本身保留——之后还可以 [`add_panel_column`] / [`add_panel_everywhere`]
/// 重新打开；要连注册一起删用 [`unregister_panel`]。
pub fn remove_panel_everywhere(world: &mut World, panel: PluginPanel) -> usize {
    if let Some(mut registry) = world.get_resource_mut::<PanelRegistry>()
        && let Some(mut spec) = registry.spec(panel)
    {
        spec.auto = PanelAuto::Manual;
        registry.set_spec(panel, spec);
    }
    let workspaces = all_workspaces(world);
    if workspaces.is_empty() {
        return usize::from(remove_legacy_global(world, panel));
    }
    workspaces
        .into_iter()
        .filter(|workspace| remove_panel_column(world, *workspace, panel))
        .count()
}

/// 注销一块插件面板：从注册表删除，并从所有工作区摘掉它的列。
///
/// 返回它之前是否已注册（未注册返回 `false`，不动任何工作区）。
pub fn unregister_panel(world: &mut World, panel: PluginPanel) -> bool {
    let registered = world
        .get_resource_mut::<PanelRegistry>()
        .is_some_and(|mut registry| registry.unregister(panel));
    if !registered {
        return false;
    }
    remove_panel_everywhere(world, panel);
    if let Some(mut active) = world.get_resource_mut::<ActivePanelColumn>()
        && active.panel == Some(panel)
    {
        *active = ActivePanelColumn::default();
    }
    true
}

/// 旧单屏路径（没有 `WorkspaceSet`）的全局列移除。
fn remove_legacy_global(world: &mut World, panel: PluginPanel) -> bool {
    let id = panel.id();
    let Some(root) = world
        .get_resource::<PanelRoots>()
        .and_then(|roots| roots.entity(id))
    else {
        return false;
    };
    if let Some(mut tree) = world.get_resource_mut::<FrameTree>() {
        if !tree.0.remove_column(id) {
            tree.0.remove_leaf(id);
        }
    }
    if let Some(mut roots) = world.get_resource_mut::<PanelRoots>() {
        roots.0.remove(&id);
    }
    if let Some(mut rows) = world.get_resource_mut::<PanelRows>() {
        rows.0.remove(&id);
    }
    if let Some(mut spawned) = world.get_resource_mut::<PanelSpawned>() {
        spawned.forget(id);
    }
    if let Some(mut areas) = world.get_resource_mut::<PanelAreas>() {
        areas.0.remove(&root);
    }
    world.entity_mut(root).despawn();
    true
}

/// 当前作为“活动列”的插件面板（切片 2 的另一个焦点）。
///
/// `ActiveTranscript` 管的是转写窗格（`Transcript` / `Pane2`）；插件列不在这套
/// 窗格模型里，所以单独记一个焦点：
///
/// * [`crate::workspaces::scroll_active_column`] 在“活动列变化 / resize / picker
///   开关请求”时把它滚进视口；
/// * [`crate::workspaces::column_workspace_keys`] 的改宽 / 移动列作用于它；
/// * **picker 打开时 picker 列优先**（现有语义不变）；picker 关掉后如果焦点
///   还指着插件列，就继续作用于插件列；
/// * 只对 `workspace == 聚焦工作区` 生效；切到别的工作区时按那里的
///   `ActiveTranscript` / 转写列算。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActivePanelColumn {
    /// 焦点在哪个工作区。
    pub workspace: Option<Entity>,
    /// 焦点列里的插件面板。
    pub panel: Option<PluginPanel>,
}

impl ActivePanelColumn {
    /// `workspace` 的活动插件面板（不匹配 / 没焦点时 `None`）。
    pub fn plugin_for(&self, workspace: Entity) -> Option<PluginPanel> {
        (self.workspace == Some(workspace))
            .then_some(self.panel)
            .flatten()
    }

    /// `workspace` 的活动列是不是 `panel`。
    pub fn is(&self, workspace: Entity, panel: PluginPanel) -> bool {
        self.workspace == Some(workspace) && self.panel == Some(panel)
    }
}

/// 把插件列设为活动列，并把它滚进视口（只写 [`WorkspaceScroll`]）。
///
/// 返回 `false` 表示工作区不存在 / 这块面板不在它的树里，不 panic、不改焦点。
/// 之后 [`crate::workspaces::scroll_active_column`] 在活动列变化时按它对齐，
/// 改宽/移动列键也作用于它。要交回转写窗格用 [`clear_panel_column_focus`]。
pub fn focus_panel_column(world: &mut World, workspace: Entity, panel: PluginPanel) -> bool {
    let id = panel.id();
    let present = world
        .get::<Workspace>(workspace)
        .is_some_and(|workspace| workspace.tree.leaves().contains(&id));
    if !present {
        return false;
    }
    if world.get_resource::<ActivePanelColumn>().is_none() {
        world.insert_resource(ActivePanelColumn::default());
    }
    if let Some(mut active) = world.get_resource_mut::<ActivePanelColumn>() {
        active.workspace = Some(workspace);
        active.panel = Some(panel);
    }
    if let Some(mut focus) = world.get_resource_mut::<WorkspaceScrollFocus>() {
        focus.request(workspace);
    }
    reveal_panel_column(world, workspace, panel)
}

/// 清掉插件列焦点（回到 `ActiveTranscript` / 转写列），返回之前有没有焦点。
///
/// 会让聚焦工作区重新请求一次“活动列滚进视口”（picker 的同类请求也是这么做的）。
pub fn clear_panel_column_focus(world: &mut World) -> bool {
    let had_focus = world
        .get_resource::<ActivePanelColumn>()
        .is_some_and(|active| active.panel.is_some());
    if let Some(mut active) = world.get_resource_mut::<ActivePanelColumn>() {
        *active = ActivePanelColumn::default();
    }
    if had_focus
        && let Some(focused) = world
            .get_resource::<WorkspaceSet>()
            .and_then(|set| set.focused)
        && let Some(mut focus) = world.get_resource_mut::<WorkspaceScrollFocus>()
    {
        focus.request(focused);
    }
    had_focus
}

/// 把插件面板所在的那一列滚进工作区视口（只写 [`WorkspaceScroll`]）。
///
/// 返回 `true` 表示面板在树上、`WorkspaceScroll` 可写；找不到工作区/面板、
/// 没有 [`FrameSize`] 时返回 `false`，不 panic。它是“让新列可见”的一步，
/// **不**改列宽、列 x 或树。注意：`workspaces::scroll_active_column` 在
/// “活动列变化 / 窗口 resize”时会把活动转写列重新滚进视口；在启动第一帧
/// 之后调用本函数最稳（插件在 `Update` 里放面板时天然满足）。
pub fn reveal_panel_column(world: &mut World, workspace: Entity, panel: PluginPanel) -> bool {
    let id = panel.id();
    let Some(frame) = world.get_resource::<FrameSize>().copied() else {
        return false;
    };
    let Some(tree) = world.get::<Workspace>(workspace).map(|workspace| workspace.tree.clone())
    else {
        return false;
    };
    let viewport = f32::from(frame.cols) * CELL.x;
    let Some(bounds) = crate::workspaces::column_bounds(&tree, id, viewport) else {
        return false;
    };
    let Some(mut scroll) = world.get_mut::<WorkspaceScroll>(workspace) else {
        return false;
    };
    let next = bounds.scroll_to(scroll.scroll);
    if next != scroll.scroll {
        scroll.scroll = next;
    }
    true
}

/// 某块面板第 `index` 行这一帧在**世界里**的矩形。
///
/// `world_rect` 是轴对齐的世界矩形（米），和 `WorldPanel::clip` 同一口径；
/// 工作区通常只平移，所以它就是行自身的矩形；如果以后工作区带旋转，取的是
/// 四个角的世界 AABB（只多不少，不会把还看得见的部分裁掉）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawnRow {
    /// 行属于哪块面板。
    pub panel: PanelId,
    /// 面板里的可见行下标（从 0 数，和 [`RowLayout::index`] 一致）。
    pub index: usize,
    /// 行在在世界坐标里的矩形（米）。
    pub world_rect: Rect,
}

/// 读回“某工作区、某面板的第 `index` 行现在在世界哪里”。
///
/// 只用三样已有数据：面板这一帧的 [`PanelArea`]（树给的精确尺寸/中心）、
/// 面板根的 [`FramePanel::inset`] 与 [`Node::justify_content`]（行从内容区
/// 顶部/居中/底部排）、以及 `WorldPanel.anchor`（3D 锚点）。行实体本身来自
/// [`crate::panels::sync_panels`] 挂的 [`RowLayout`]，所以返回的
/// [`DrawnRow::index`] 和行同步用的是同一个下标。
///
/// 没这块面板、行还没同步出来、或面板这一帧没有区域时返回 `None`；不 panic。
pub fn drawn_row(
    world: &World,
    workspace: Entity,
    panel: PanelId,
    index: usize,
) -> Option<DrawnRow> {
    let panels = world.get::<WorkspacePanels>(workspace)?;
    let root = panels.roots.entity(panel)?;
    let area = panels.areas.get(&root).copied()?;
    let frame_panel = world.get::<FramePanel>(root)?;
    let world_panel = world.get::<WorldPanel>(root)?;
    let root_node = world.get::<Node>(root)?;
    let row_count = panels.rows.get(panel).len();
    let spawned = panels.spawned.0.get(&panel)?;
    let entity = *spawned.entities.get(index)?;
    let layout = world.get::<RowLayout>(entity)?;
    if layout.panel != panel || layout.index != index || layout.panel_entity != root {
        return None;
    }
    let world_rect = row_world_rect(
        area,
        frame_panel.inset,
        root_node.justify_content,
        row_count,
        index,
        world_panel,
    )?;
    Some(DrawnRow {
        panel,
        index,
        world_rect,
    })
}

/// 读回一块面板这一帧所有已同步的行（按 index 顺序）。
pub fn drawn_rows(world: &World, workspace: Entity, panel: PanelId) -> Vec<DrawnRow> {
    let count = world
        .get::<WorkspacePanels>(workspace)
        .and_then(|panels| panels.spawned.0.get(&panel))
        .map_or(0, |spawned| spawned.entities.len());
    (0..count)
        .filter_map(|index| drawn_row(world, workspace, panel, index))
        .collect()
}

/// 一行在面板内的几何：纯函数，读回和测试用同一份公式。
///
/// 行的 x 永远居中（`sync_panels` 生成的行是 `width: 100%`，填满内容区）；
/// y 由根节点的 `justify_content` 决定起始位置（转写/插件面板是 FlexStart，
/// 状态行是 Center），行高固定 [`ROW_HEIGHT`]。
fn row_world_rect(
    area: PanelArea,
    inset: PanelInset,
    justify: JustifyContent,
    row_count: usize,
    index: usize,
    world_panel: &WorldPanel,
) -> Option<Rect> {
    if row_count == 0 || index >= row_count {
        return None;
    }
    // `PanelInset::total()` 是上下（左右）各一份之和，所以内容区减一份、
    // 起始偏移取一半。
    let side = inset.total() * 0.5;
    let inner = (area.size - Vec2::splat(inset.total())).max(Vec2::ZERO);
    let rows = row_count as f32 * ROW_HEIGHT;
    let align = match justify {
        JustifyContent::Center => (inner.y - rows) * 0.5,
        JustifyContent::FlexEnd | JustifyContent::End => inner.y - rows,
        _ => 0.0,
    };
    // 行中心在面板局部坐标里（原点 = 面板中心；x 右、y 上，单位 UI 像素）。
    let top_from_panel_top = side + align + index as f32 * ROW_HEIGHT;
    let local_center = Vec2::new(
        0.0,
        area.size.y * 0.5 - (top_from_panel_top + ROW_HEIGHT * 0.5),
    );
    let half = Vec2::new(inner.x * 0.5, ROW_HEIGHT * 0.5);
    let units = world_panel.units_to_world;
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let local = Vec3::new(
            (local_center.x + sx * half.x) * units,
            (local_center.y + sy * half.y) * units,
            0.0,
        );
        let world = world_panel.anchor.transform_point(local);
        min = min.min(world.truncate());
        max = max.max(world.truncate());
    }
    Some(Rect::from_corners(min, max))
}

#[cfg(test)]
mod tests {
    //! 切片 1/2 的闭环：注册 → 放进条带 → 根 → 区域 → 行世界矩形；
    //! 切片 2 再加 `StatusLine`、自动工作区策略、移除与活动列。
    //! 不建窗口、不跑 GPU；3D“可见”用 `WorldPanel.anchor` + `clip` 的交集断言。

    use super::*;
    use crate::frame::{FrameGeometry, PanelAreas, UNITS_TO_WORLD};
    use crate::layout::Node as TestNode;
    use crate::panels::{KeyedRow, PanelsPlugin, RowLayout};
    use crate::status::{StatusLine, StatusLineRow, StatusPlugin};
    use crate::transcript::{ROW_HEIGHT, Row, TranscriptFont};
    use crate::workspaces::{
        WorkspacePanel, WorkspaceSet, WorkspacesPlugin, spawn_workspace,
    };
    use bevy::text::FontSource;
    use bevy_ui::px;
    use rig_harness::prelude::ShownAgent;
    use rig_tui::ratatui::text::Line;
    use rig_tui::{Side, StatusItem, StatusItems, Tone};

    fn keyed(key: u64, text: &str) -> KeyedRow {
        KeyedRow {
            key,
            hash: key,
            row: Row::new(Line::from(text.to_owned())),
        }
    }

    fn one_workspace(mut commands: Commands, mut set: ResMut<WorkspaceSet>) {
        spawn_workspace(&mut commands, &mut set, 1, Transform::IDENTITY, 0);
    }

    /// 和 `workspaces.rs` 的测试 App 同构：固定 96×28 帧，工作区由测试自己
    /// spawn，布局系统手动接进 `Layout` 阶段。
    fn panel_app() -> App {
        let mut app = App::new();
        app.init_resource::<FrameTree>()
            .init_resource::<PanelAreas>()
            .init_resource::<FrameGeometry>()
            .insert_resource(FrameSize { cols: 96, rows: 28 })
            .insert_resource(TranscriptFont(FontSource::default()))
            .add_plugins((PanelsPlugin, WorkspacesPlugin, StatusPlugin))
            .add_systems(
                Update,
                (crate::frame::lay_out_frame, crate::frame::place_panels)
                    .chain()
                    .in_set(crate::FeathersSystems::Layout),
            )
            .add_systems(Startup, one_workspace);
        app.update();
        let set = app.world().resource::<WorkspaceSet>();
        assert_eq!(set.items.len(), 1, "一个工作区");
        app
    }

    fn workspace(app: &App) -> Entity {
        app.world().resource::<WorkspaceSet>().items[0]
    }

    fn panels(app: &App, workspace: Entity) -> &WorkspacePanels {
        app.world()
            .get::<WorkspacePanels>(workspace)
            .expect("工作区要有 WorkspacePanels")
    }

    /// 在测试 App 里再 spawn 一个工作区（命令 flush 后返回实体）。
    fn spawn_extra_workspace(app: &mut App, session: u64, order: usize) -> Entity {
        let mut set = app
            .world_mut()
            .remove_resource::<WorkspaceSet>()
            .expect("WorkspaceSet");
        let workspace = {
            let mut commands = app.world_mut().commands();
            spawn_workspace(
                &mut commands,
                &mut set,
                session,
                Transform::IDENTITY,
                order,
            )
        };
        app.world_mut().insert_resource(set);
        app.world_mut().flush();
        workspace
    }

    /// 面板根下由 `StatusLine` 生成的底部行 + 它的纯文本。
    fn status_line_of(app: &App, panel: Entity) -> Option<(Entity, String)> {
        let children = app.world().get::<Children>(panel)?;
        for row in children.iter() {
            if app.world().get::<StatusLineRow>(row).is_none() {
                continue;
            }
            let text = app
                .world()
                .get::<Children>(row)
                .and_then(|children| children.iter().next())
                .and_then(|text| app.world().get::<Children>(text))
                .map(|spans| {
                    spans
                        .iter()
                        .filter_map(|span| app.world().get::<TextSpan>(span))
                        .map(|span| span.0.to_string())
                        .collect::<String>()
                })
                .unwrap_or_default();
            return Some((row, text));
        }
        None
    }

    fn agent_with(app: &mut App, items: &[StatusItem]) -> Entity {
        let mut status = StatusItems::default();
        for item in items {
            status.show(item.clone());
        }
        app.world_mut().spawn(status).id()
    }

    /// 这块面板这一帧在 3D 视口里有没有可见部分（和切片 1 的判据一致）。
    fn panel_visible(app: &App, workspace: Entity, root: Entity) -> bool {
        let Some(panel) = app.world().get::<WorldPanel>(root) else {
            return false;
        };
        let Some(area) = panels(app, workspace).area(root) else {
            return false;
        };
        !feathers_in_3d::effective_clip(
            feathers_in_3d::panel_world_rect(
                &panel.anchor,
                area.size * UNITS_TO_WORLD,
            ),
            panel.clip,
        )
        .is_empty()
    }

    // 1. 注册 → 放进条带 → 根/区域/镜像根都在，内置列宽不变。
    #[test]
    fn a_registered_panel_becomes_a_column_with_a_mirror_root() {
        let mut app = panel_app();
        let ws = workspace(&app);
        let before = panels(&app, ws).roots.clone();
        let transcript_width = app
            .world()
            .get::<Workspace>(ws)
            .unwrap()
            .tree
            .column_fraction(PanelId::Transcript);

        let handle = app.register_panel("git");
        let root = add_panel_column(app.world_mut(), ws, handle, 0.25).expect("放进条带");
        app.update();

        let id = handle.id();
        let tree = &app.world().get::<Workspace>(ws).unwrap().tree;
        assert!(tree.leaves().contains(&id), "树里有插件面板");
        assert_eq!(tree.column_fraction(id), Some(0.25), "列宽是 Fraction(0.25)");
        assert_eq!(
            tree.column_fraction(PanelId::Transcript),
            transcript_width,
            "开新列不改已有列宽"
        );

        let stored = panels(&app, ws);
        assert_eq!(stored.roots.entity(id), Some(root), "根表登记了插件面板");
        assert_eq!(
            app.world().resource::<PanelRoots>().entity(id),
            Some(root),
            "聚焦工作区的全局代理也登记了"
        );
        let area = stored.area(root).expect("这一帧有区域");
        assert!(area.size.x > 0.0 && area.size.y > 0.0, "面板区域非零");
        assert!(
            app.world().get::<feathers_in_3d::MirrorRoot>(root).is_some(),
            "面板根是 3D 镜像根"
        );
        assert!(
            app.world().get::<WorldPanel>(root).is_some(),
            "面板根有世界锚点"
        );
        // 内置面板的根一个不少、实体也不变（行为不变的硬条件）。
        for panel in [
            PanelId::Transcript,
            PanelId::Status,
            PanelId::Input,
            PanelId::Hints,
            PanelId::Confirm,
            PanelId::Complete,
            PanelId::Spacer,
        ] {
            assert_eq!(stored.roots.entity(panel), before.entity(panel), "{panel:?} 不变");
        }
        // 插件能拿到根并往里面 spawn UI（文字/图片都走同一棵子树）。
        let mut text = None;
        app.world_mut()
            .entity_mut(root)
            .with_children(|parent| {
                text = Some(
                    parent
                        .spawn((Text::new("git status"), Node::default()))
                        .id(),
                );
            });
        let text = text.expect("spawn 了一行文字");
        assert!(
            app.world()
                .get::<Children>(root)
                .is_some_and(|children| children.contains(&text)),
            "根节点下面有插件自己的 UI 子树"
        );
    }

    // 2. 行读回：第 n 行的世界矩形按 PanelArea + inset + RowLayout 算准。
    #[test]
    fn the_world_rect_of_a_drawn_row_is_readable() {
        let mut app = panel_app();
        let ws = workspace(&app);
        let handle = app.register_panel("rows");
        let root = add_panel_column(app.world_mut(), ws, handle, 0.25).expect("放进条带");
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(handle.id(), vec![keyed(1, "one"), keyed(2, "two")]);
        app.update();

        let area = panels(&app, ws).area(root).expect("有区域");
        let inset = app.world().get::<FramePanel>(root).unwrap().inset.total();
        let world_panel = *app.world().get::<WorldPanel>(root).unwrap();
        let id = handle.id();

        let first = drawn_row(app.world(), ws, id, 0).expect("第 0 行");
        let second = drawn_row(app.world(), ws, id, 1).expect("第 1 行");
        assert_eq!(first.panel, id);
        assert_eq!(second.index, 1);
        assert_eq!(drawn_rows(app.world(), ws, id), vec![first, second]);
        assert!(drawn_row(app.world(), ws, id, 2).is_none(), "越界返回 None");

        // 期望值：内容区从面板顶 + inset/2 开始，行高 18px，行宽填满内容区。
        let side = inset * 0.5;
        let inner_width = area.size.x - inset;
        let local_x = 0.0;
        let row_center_y = |index: usize| {
            area.size.y * 0.5
                - (side + index as f32 * ROW_HEIGHT + ROW_HEIGHT * 0.5)
        };
        let expected = |index: usize| {
            let center = world_panel.anchor.transform_point(Vec3::new(
                local_x * world_panel.units_to_world,
                row_center_y(index) * world_panel.units_to_world,
                0.0,
            ));
            Rect::from_center_size(
                center.truncate(),
                Vec2::new(inner_width, ROW_HEIGHT) * world_panel.units_to_world,
            )
        };
        for (drawn, index) in [(first, 0usize), (second, 1)] {
            let want = expected(index);
            assert!(
                (drawn.world_rect.center() - want.center()).length() < 1.0e-4,
                "第 {index} 行中心：{:?} != {:?}",
                drawn.world_rect.center(),
                want.center()
            );
            assert!(
                (drawn.world_rect.size() - want.size()).length() < 1.0e-4,
                "第 {index} 行尺寸：{:?} != {:?}",
                drawn.world_rect.size(),
                want.size()
            );
        }
        // 两行之间正好差一个行高（世界米）。
        assert!(
            (first.world_rect.center().y - second.world_rect.center().y
                - ROW_HEIGHT * UNITS_TO_WORLD)
                .abs()
                < 1.0e-5
        );
    }

    // 3. 新列默认在视口外（niri 追加语义）；reveal 之后 3D 裁剪交集非空。
    #[test]
    fn revealing_a_plugin_column_makes_it_visible() {
        let mut app = panel_app();
        let ws = workspace(&app);
        let handle = app.register_panel("gallery");
        let root = add_panel_column(app.world_mut(), ws, handle, 0.25).expect("放进条带");
        app.update();

        let area = panels(&app, ws).area(root).expect("有区域");
        let panel_rect = feathers_in_3d::panel_world_rect(
            &app.world().get::<WorldPanel>(root).unwrap().anchor,
            area.size * UNITS_TO_WORLD,
        );
        let before = feathers_in_3d::effective_clip(
            panel_rect,
            app.world().get::<WorldPanel>(root).unwrap().clip,
        );
        assert!(before.is_empty(), "默认追加在视口右边，先看不见");

        assert!(reveal_panel_column(app.world_mut(), ws, handle));
        app.update();
        let world = app.world();
        let panel = world.get::<WorldPanel>(root).unwrap();
        let area = panels(&app, ws).area(root).unwrap();
        let visible = feathers_in_3d::effective_clip(
            feathers_in_3d::panel_world_rect(&panel.anchor, area.size * UNITS_TO_WORLD),
            panel.clip,
        );
        assert!(!visible.is_empty(), "滚进来之后面板在视口里可见");
    }

    // 4. 重复注册 / 重复放置 / 找不到工作区 / 空面板：都不 panic。
    #[test]
    fn duplicate_and_missing_inputs_do_not_panic() {
        let mut app = panel_app();
        let ws = workspace(&app);

        let first = app.register_panel("dup");
        let again = app.register_panel("dup");
        assert_eq!(first, again, "同名重复注册返回同一句柄");
        assert_eq!(
            app.world().resource::<PanelRegistry>().len(),
            1,
            "注册表没多出一块"
        );
        app.register_panel(""); // 空名字也不 panic

        let root = add_panel_column(app.world_mut(), ws, first, 0.2).unwrap();
        let same = add_panel_column(app.world_mut(), ws, first, 0.4).unwrap();
        assert_eq!(root, same, "重复放置是幂等的");
        let leaves = app
            .world()
            .get::<Workspace>(ws)
            .unwrap()
            .tree
            .leaves()
            .iter()
            .filter(|panel| **panel == first.id())
            .count();
        assert_eq!(leaves, 1, "树里只有一块");

        assert!(
            add_panel_column(app.world_mut(), Entity::PLACEHOLDER, first, 0.2).is_none(),
            "找不到工作区返回 None"
        );
        assert!(
            !reveal_panel_column(app.world_mut(), Entity::PLACEHOLDER, first),
            "找不到工作区返回 false"
        );
        assert!(
            add_panel_column(app.world_mut(), ws, PluginPanel(9999), 0.2).is_none(),
            "未注册句柄返回 None"
        );

        // 空面板：没有行也能布局，读回返回空，不 panic。
        app.update();
        let empty = app.register_panel("empty");
        let empty_root = add_panel_column(app.world_mut(), ws, empty, 0.2).unwrap();
        app.update();
        let area = panels(&app, ws).area(empty_root).expect("空面板也有区域");
        assert!(area.size.x > 0.0);
        assert!(drawn_rows(app.world(), ws, empty.id()).is_empty());
    }

    // 5. 非聚焦工作区：列只进它自己的树/根表，不污染聚焦工作区。
    #[test]
    fn a_column_can_target_a_specific_workspace() {
        let mut app = panel_app();
        let focused = workspace(&app);
        let other = {
            // 先借出 `WorkspaceSet`，否则 `commands()` 与它同时借 `app`。
            let mut set = app
                .world_mut()
                .remove_resource::<WorkspaceSet>()
                .expect("WorkspaceSet");
            let other = {
                let mut commands = app.world_mut().commands();
                spawn_workspace(
                    &mut commands,
                    &mut set,
                    2,
                    Transform::from_xyz(0.0, -1.0, 0.0),
                    1,
                )
            };
            app.world_mut().insert_resource(set);
            app.world_mut().flush();
            other
        };
        app.update();
        let handle = app.register_panel("only-other");
        let root = add_panel_column(app.world_mut(), other, handle, 0.3).expect("放进非聚焦工作区");

        assert!(
            app.world()
                .get::<Workspace>(other)
                .unwrap()
                .tree
                .leaves()
                .contains(&handle.id()),
            "非聚焦工作区的树有这一列"
        );
        assert!(
            !app.world()
                .get::<Workspace>(focused)
                .unwrap()
                .tree
                .leaves()
                .contains(&handle.id()),
            "聚焦工作区不受影响"
        );
        assert_eq!(
            panels(&app, other).roots.entity(handle.id()),
            Some(root)
        );
        // 全局代理只代表聚焦工作区：插件列不应该出现在这里。
        assert_eq!(app.world().resource::<PanelRoots>().entity(handle.id()), None);
    }

    #[derive(Resource)]
    struct QueuedRoot(Option<Entity>);

    // 6. 从 `Commands::queue` 里调用（普通系统最常见的入口）：不 panic。
    #[test]
    fn adding_a_column_from_a_queued_command_works() {
        let mut app = panel_app();
        let ws = workspace(&app);
        let handle = app.register_panel("queued");
        app.world_mut().commands().queue(move |world: &mut World| {
            let root = add_panel_column(world, ws, handle, 0.2);
            world.insert_resource(QueuedRoot(root));
        });
        app.world_mut().flush();
        let root = app
            .world()
            .resource::<QueuedRoot>()
            .0
            .expect("queue 里也能放列");
        assert!(app.world().get::<crate::workspaces::WorkspacePanel>(root).is_some());
    }

    /// 编译期：`PluginPanel` 可以直接变成 `PanelId` / 用来建树叶子。
    #[test]
    fn a_handle_is_a_layout_key() {
        let handle = PluginPanel(7);
        let node = TestNode::leaf(handle.id());
        assert_eq!(node.leaves(), vec![PanelId::Plugin(handle)]);
    }
    // -----------------------------------------------------------------------
    // 切片 2：StatusLine
    // -----------------------------------------------------------------------

    // 7. 插件面板根挂 StatusLine：底部出现绝对定位状态行，内容来自 agent items
    //    / no agent 规则；不挂就不画。
    #[test]
    fn a_status_line_on_a_plugin_panel_is_drawn_at_the_bottom() {
        let mut app = panel_app();
        let ws = workspace(&app);
        let handle = app.register_panel("status-plugin");
        let root = add_panel_column(app.world_mut(), ws, handle, 0.3).expect("放进条带");
        app.update();
        assert!(
            status_line_of(&app, root).is_none(),
            "不挂 StatusLine 的面板不该多出底部行"
        );

        app.world_mut()
            .entity_mut(root)
            .insert(StatusLine::default());
        app.update();
        let (row, text) = status_line_of(&app, root).expect("底部状态行");
        assert!(text.contains("no agent"), "没有 ShownAgent 时画占位：{text:?}");
        let node = app.world().get::<Node>(row).expect("状态行是 UI 节点");
        assert_eq!(node.position_type, PositionType::Absolute);
        assert_eq!(node.bottom, px(0.0), "贴面板底");
        assert_eq!(node.height, px(ROW_HEIGHT), "一行高");
        assert!(
            app.world().get::<RowLayout>(row).is_none(),
            "状态行不进 PanelRows 的行 diff"
        );

        // 有 agent：画它的 items，并且不再出现 no agent 占位。
        let agent = agent_with(
            &mut app,
            &[StatusItem::new("model", Side::Left, 30, u8::MAX)
                .says("acme-model", Tone::Bold)],
        );
        app.insert_resource(ShownAgent(Some(agent)));
        app.update();
        let (_, text) = status_line_of(&app, root).expect("状态行还在");
        assert!(text.contains("acme-model"), "{text:?}");
        assert!(!text.contains("no agent"), "{text:?}");

        // `StatusLine.agent` 可以画指定的别的 agent。
        let other = agent_with(
            &mut app,
            &[StatusItem::new("model", Side::Left, 30, u8::MAX)
                .says("other-agent", Tone::Bold)],
        );
        app.world_mut()
            .entity_mut(root)
            .insert(StatusLine { agent: Some(other) });
        app.update();
        let (_, text) = status_line_of(&app, root).expect("状态行还在");
        assert!(text.contains("other-agent"), "{text:?}");
        assert!(!text.contains("acme-model"), "{text:?}");
    }

    // 8. 输入框上的 StatusLine 画在编辑器底行：绝对定位、贴底，不占额外行/不改区域。
    #[test]
    fn a_status_line_on_the_input_panel_stays_in_its_bottom_row() {
        let mut app = panel_app();
        let ws = workspace(&app);
        let input = panels(&app, ws).roots.entity(PanelId::Input).expect("输入面板");
        let before = panels(&app, ws).area(input).expect("输入面板区域");
        let before_rows = app.world().resource::<PanelRows>().get(PanelId::Input).len();

        app.world_mut()
            .entity_mut(input)
            .insert(StatusLine::default());
        app.update();

        let (row, text) = status_line_of(&app, input).expect("输入框底行状态");
        assert!(text.contains("no agent"), "{text:?}");
        let node = app.world().get::<Node>(row).expect("状态行节点");
        assert_eq!(node.position_type, PositionType::Absolute);
        assert_eq!(node.bottom, px(0.0));
        assert_eq!(node.height, px(ROW_HEIGHT));

        // 不额外占一行：区域、面板行数、输入框自己的行都没变（绝对定位不参与 flex）。
        let after = panels(&app, ws).area(input).expect("输入面板区域");
        assert_eq!(after.size, before.size, "输入面板尺寸不变");
        assert_eq!(after.rows, before.rows, "输入面板可用行数不变");
        assert_eq!(
            app.world().resource::<PanelRows>().get(PanelId::Input).len(),
            before_rows,
            "StatusLine 不往 PanelRows 里写行"
        );
    }

    // 9. 多个面板各挂各的；宽度裁剪来自各自的面板宽度；换 agent 互不影响。
    #[test]
    fn status_lines_are_per_panel_and_clip_to_the_panel_width() {
        let mut app = panel_app();
        let ws = workspace(&app);
        let narrow = app.register_panel("narrow");
        let wide = app.register_panel("wide");
        let narrow_root =
            add_panel_column(app.world_mut(), ws, narrow, 0.3).expect("窄列");
        let wide_root = add_panel_column(app.world_mut(), ws, wide, 0.8).expect("宽列");
        app.update();

        let agent = agent_with(
            &mut app,
            &[
                StatusItem::new("model", Side::Left, 30, u8::MAX)
                    .says("acme-model", Tone::Bold),
                StatusItem::new("goal", Side::Left, 60, 20)
                    .says("goal: ship the parser", Tone::Plain)
                    .shrinks_to(8),
            ],
        );
        app.insert_resource(ShownAgent(Some(agent)));
        app.world_mut()
            .entity_mut(narrow_root)
            .insert(StatusLine::default());
        app.world_mut()
            .entity_mut(wide_root)
            .insert(StatusLine::default());
        app.update();

        let (_, narrow_text) = status_line_of(&app, narrow_root).expect("窄面板状态行");
        let (_, wide_text) = status_line_of(&app, wide_root).expect("宽面板状态行");
        assert!(narrow_text.contains("acme-model"), "{narrow_text:?}");
        assert!(wide_text.contains("goal: ship the parser"), "{wide_text:?}");
        assert!(narrow_text.ends_with('…'), "窄面板按宽度裁剪：{narrow_text:?}");
        assert!(
            wide_text.len() > narrow_text.len(),
            "宽度不同、裁剪结果不同：{narrow_text:?} vs {wide_text:?}"
        );

        // 两个面板互不影响：宽面板换成别的 agent，窄面板保持原样。
        let other = agent_with(
            &mut app,
            &[StatusItem::new("model", Side::Left, 30, u8::MAX)
                .says("other-agent", Tone::Bold)],
        );
        app.world_mut()
            .entity_mut(wide_root)
            .insert(StatusLine { agent: Some(other) });
        app.update();
        let (_, narrow_after) = status_line_of(&app, narrow_root).expect("窄面板状态行");
        let (_, wide_after) = status_line_of(&app, wide_root).expect("宽面板状态行");
        assert_eq!(narrow_after, narrow_text, "另一个面板换 agent 不影响它");
        assert!(wide_after.contains("other-agent"), "{wide_after:?}");
        assert!(!wide_after.contains("acme-model"), "{wide_after:?}");

        // 摘掉一个 StatusLine：只有它的底部行消失。
        app.world_mut()
            .entity_mut(wide_root)
            .remove::<StatusLine>();
        app.update();
        assert!(status_line_of(&app, wide_root).is_none(), "摘掉组件后不画");
        assert!(status_line_of(&app, narrow_root).is_some(), "别的面板照画");
    }

    // -----------------------------------------------------------------------
    // 切片 2：自动工作区 / 移除 / 活动列
    // -----------------------------------------------------------------------

    // 10. 注册时带 PanelAuto::AllWorkspaces：已有工作区全有，新 spawn 的也有。
    #[test]
    fn an_auto_panel_joins_every_workspace_including_new_ones() {
        let mut app = panel_app();
        let focused = workspace(&app);
        let second = spawn_extra_workspace(&mut app, 1, 1);
        let third = spawn_extra_workspace(&mut app, 2, 2);
        app.update();

        let handle =
            app.register_panel_with("auto", PanelSpec::everywhere(0.27));
        app.update();

        for ws in [focused, second, third] {
            let tree = &app.world().get::<Workspace>(ws).expect("工作区").tree;
            assert!(tree.leaves().contains(&handle.id()), "{ws:?} 有自动列");
            assert_eq!(
                tree.column_fraction(handle.id()),
                Some(0.27),
                "自动列宽 = fraction × 视口"
            );
            assert_eq!(
                tree.column_fraction(PanelId::Transcript),
                Some(layout::TRANSCRIPT_COLUMN_FRACTION),
                "已有列宽不变"
            );
            let root = panels(&app, ws)
                .roots
                .entity(handle.id())
                .expect("每个工作区各有根");
            assert_eq!(
                app.world().get::<WorkspacePanel>(root).expect("工作区标签").0,
                ws
            );
        }

        // 注册之后新建的工作区，下一帧自动带上。
        let fourth = spawn_extra_workspace(&mut app, 3, 3);
        app.update();
        let tree = &app.world().get::<Workspace>(fourth).expect("新工作区").tree;
        assert!(tree.leaves().contains(&handle.id()), "新工作区也有自动列");
        assert!(panels(&app, fourth).roots.entity(handle.id()).is_some());

        assert_eq!(
            app.world().resource::<PanelRegistry>().spec(handle).map(|spec| spec.auto),
            Some(PanelAuto::AllWorkspaces)
        );
    }

    // 11. 移除自动列：所有工作区都摘掉，树逐字节复原（旧列宽不变）、根实体 despawn；
    //     自动策略同时停掉，不会下一帧又加回来。
    #[test]
    fn removing_an_auto_panel_restores_every_workspace_tree() {
        let mut app = panel_app();
        let focused = workspace(&app);
        let second = spawn_extra_workspace(&mut app, 1, 1);
        app.update();
        let before: Vec<crate::layout::Node> = [focused, second]
            .iter()
            .map(|ws| app.world().get::<Workspace>(*ws).expect("工作区").tree.clone())
            .collect();

        let handle =
            app.register_panel_with("auto-remove", PanelSpec::everywhere(0.21));
        app.update();
        let roots: Vec<Entity> = [focused, second]
            .iter()
            .map(|ws| {
                panels(&app, *ws)
                    .roots
                    .entity(handle.id())
                    .expect("自动列已放进工作区")
            })
            .collect();
        assert_eq!(roots.len(), 2);

        assert_eq!(remove_panel_everywhere(app.world_mut(), handle), 2);
        for (ws, tree_before) in [focused, second].iter().zip(&before) {
            let tree_after = &app.world().get::<Workspace>(*ws).expect("工作区").tree;
            assert_eq!(
                tree_after, tree_before,
                "加列→摘列之后树逐字节复原（兄弟列宽不变）"
            );
            assert!(!tree_after.leaves().contains(&handle.id()));
            assert!(
                panels(&app, *ws).roots.entity(handle.id()).is_none(),
                "根表里也摘掉了"
            );
        }
        for root in roots {
            assert!(app.world().get_entity(root).is_err(), "面板根已 despawn");
        }

        // 规格降回 Manual：再跑几帧不会自动加回来。
        app.update();
        for ws in [focused, second] {
            assert!(
                !app.world()
                    .get::<Workspace>(ws)
                    .expect("工作区")
                    .tree
                    .leaves()
                    .contains(&handle.id()),
                "自动策略已停"
            );
        }

        // 注销：注册表里也没有了。
        assert!(app.world().resource::<PanelRegistry>().contains(handle));
        assert!(unregister_panel(app.world_mut(), handle));
        assert!(!app.world().resource::<PanelRegistry>().contains(handle));
    }

    // 12. 插件列 reveal 和真实 picker 列共存：开 picker 不会挤掉插件列，
    //     关 picker 只摘它自己的列，插件列宽不变。
    #[test]
    fn a_plugin_column_can_be_revealed_alongside_an_open_picker_column() {
        let mut app = panel_app();
        app.init_resource::<crate::theme::Theme>()
            .add_plugins(crate::picker::PickerPlugin);
        app.update();
        let ws = workspace(&app);

        let handle = app.register_panel("coexist");
        let root =
            add_panel_column(app.world_mut(), ws, handle, 0.25).expect("插件列");
        app.update();

        let picker = app
            .world_mut()
            .spawn(rig_harness::prelude::Picker::new("test").item("a", ""))
            .id();
        app.update();
        let tree = &app.world().get::<Workspace>(ws).expect("工作区").tree;
        assert!(tree.leaves().contains(&handle.id()), "插件列还在");
        assert!(tree.leaves().contains(&PanelId::List), "picker 列打开了");
        let plugin_fraction = tree.column_fraction(handle.id());
        assert_eq!(plugin_fraction, Some(0.25));

        // 先滚回 0，插件列在视口外；reveal 之后滚进来且 picker 列不被摘掉。
        app.world_mut()
            .get_mut::<WorkspaceScroll>(ws)
            .expect("WorkspaceScroll")
            .scroll = 0.0;
        assert!(reveal_panel_column(app.world_mut(), ws, handle));
        app.update();
        assert!(panel_visible(&app, ws, root), "reveal 后插件列在视口里");
        assert!(
            app.world()
                .get::<Workspace>(ws)
                .expect("工作区")
                .tree
                .leaves()
                .contains(&PanelId::List),
            "picker 列和插件列同时存在"
        );

        // 关 picker：只摘 picker 列；插件列和它的列宽都不动。
        app.world_mut().entity_mut(picker).despawn();
        app.update();
        let tree = &app.world().get::<Workspace>(ws).expect("工作区").tree;
        assert!(!tree.leaves().contains(&PanelId::List), "picker 列关掉了");
        assert!(tree.leaves().contains(&handle.id()), "插件列没被顺手摘掉");
        assert_eq!(
            tree.column_fraction(handle.id()),
            plugin_fraction,
            "插件列宽不变"
        );
        assert_eq!(
            tree.column_fraction(PanelId::Transcript),
            Some(layout::TRANSCRIPT_COLUMN_FRACTION),
            "转写列宽也不变"
        );
        assert!(
            panels(&app, ws).roots.entity(handle.id()).is_some(),
            "插件面板根还活着"
        );
        // 关完 picker 还能再把插件列滚进来。
        assert!(reveal_panel_column(app.world_mut(), ws, handle));
        app.update();
        assert!(panel_visible(&app, ws, root));
    }

    // 13. 插件列可以成为活动列（ActiveTranscript 之外的另一个焦点）：
    //     `scroll_active_column` 按它滚进视口，`ActiveTranscript` 切换不抢走焦点。
    #[test]
    fn a_plugin_column_can_become_the_active_column() {
        let mut app = panel_app();
        let ws = workspace(&app);
        let handle = app.register_panel("focus");
        let root =
            add_panel_column(app.world_mut(), ws, handle, 0.25).expect("插件列");
        app.update();
        assert!(!panel_visible(&app, ws, root), "默认追加在视口右边，先看不见");

        assert!(focus_panel_column(app.world_mut(), ws, handle));
        app.update();
        assert_eq!(
            app.world().resource::<ActivePanelColumn>().plugin_for(ws),
            Some(handle)
        );
        assert!(panel_visible(&app, ws, root), "活动列自动滚进视口");

        // `ActiveTranscript` 的焦点不冲突：插件列焦点优先，仍然可见。
        app.world_mut()
            .insert_resource(crate::pane::ActiveTranscript(None));
        app.update();
        assert!(panel_visible(&app, ws, root));

        // 清掉插件列焦点：活动列回到转写，滚动也回 0。
        assert!(clear_panel_column_focus(app.world_mut()));
        app.update();
        assert_eq!(
            app.world().resource::<ActivePanelColumn>().plugin_for(ws),
            None
        );
        assert_eq!(
            app.world()
                .get::<WorkspaceScroll>(ws)
                .expect("WorkspaceScroll")
                .scroll,
            0.0
        );
    }

}
