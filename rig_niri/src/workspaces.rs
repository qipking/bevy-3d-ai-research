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

use bevy::input::ButtonState;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::MouseScrollUnit;
use bevy::picking::events::PointerScroll;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};

use crate::frame::{
    CELL, DefaultScreen, FramePanel, FrameSize, FrameTree, PanelArea, PanelAreas, UNITS_TO_WORLD,
};
use crate::layout;
use crate::niri::TileRect;
use crate::panels::{PanelId, PanelRoots, PanelRows, PanelSpawned};
use crate::pane::ActiveTranscript;
use crate::panel::ActivePanelColumn;
use crate::picker::PickerInput;
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

/// 一个工作区条带的**横向滚动量**（组件，挂在工作区实体上）。
///
/// 为什么不是 [`Workspace`] 的字段：`Workspace` 在 `picker.rs` 的测试里用结构体
/// 字面量直接构造（`Workspace { session, tree, pose, order }`），加字段会让那些
/// 收编路径编译不过；而 scroll 是"条带视口"的运行时状态，和 session/tree/pose
/// 不同生命周期，独立组件更合适（也是 `NIRI_SCROLL.md` §3.5 说的"每个工作区
/// 各自持有自己的 scroll"）。
///
/// 单位是 **UI 像素**（和 [`crate::layout::Size::Fixed`] 一致），`0` = 最左；
/// 正值把条带内容往左推。只有横向条带（含转写列的那条）会应用它，
/// Hints/Status 这类横跨整条、位于条带之外的 chrome 行不动。
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct WorkspaceScroll {
    /// 当前滚动量（UI 像素，`>= 0`）。
    pub scroll: f32,
}

impl WorkspaceScroll {
    /// 负值 / NaN 夹到 0；上界由 [`clamp_scroll`] 在内容宽度已知时再夹。
    pub fn new(scroll: f32) -> Self {
        Self {
            scroll: finite_px(scroll).max(0.0),
        }
    }
}

/// f32 的"有限值"清洗：NaN / ±∞ → 0。
fn finite_px(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// 滚动量的合法上界：`[0, max(0, total_width - viewport_width)]`。
///
/// 入参和返回都是 **UI 像素**。总宽比视口窄、或总宽是 NaN/负数时返回 `0.0`
/// （不滚动）；视口是 NaN/∞ 时也返回 `0.0`（∞ 视口等于"全都看得见"）。
/// L3 的动作做完一次滚动后用它夹住；L2 只导出，不做交互。
pub fn clamp_scroll(total_width: f32, viewport_width: f32) -> f32 {
    // 视口本身是 NaN/∞ 时没有可滚动的窗口：∞ 视口等于"全都看得见"。
    if !viewport_width.is_finite() {
        return 0.0;
    }
    let total = finite_px(total_width).max(0.0);
    let viewport = viewport_width.max(0.0);
    (total - viewport).max(0.0)
}

/// 把 `[x, x + width)` 滚进 `[scroll, scroll + viewport_width)`。
///
/// 语义照 niri：
/// * 列比视口宽 → **左贴边**（`scroll = x`，此时右边必然超出，只能牺牲右边）；
/// * 否则**左边优先**：列在视口左边就滚到 `x`；只在右边超出时才往右多滚
///   `x + width - viewport`；
/// * 结果不小于 0（不滚出内容最左端）；NaN/负的视口/宽度按 0 处理。
///
/// 上界不在这里夹——调用方拿到结果后再用 [`clamp_scroll`]（它需要总宽）。
/// 入参/返回都是 **UI 像素**。
pub fn scroll_to_visible(scroll: f32, x: f32, width: f32, viewport_width: f32) -> f32 {
    let viewport = finite_px(viewport_width).max(0.0);
    if viewport <= 0.0 {
        return 0.0;
    }
    let scroll = finite_px(scroll).max(0.0);
    let x = finite_px(x);
    let width = finite_px(width).max(0.0);
    // 比视口宽：左贴边（也只能这样）。
    if width >= viewport {
        return x.max(0.0);
    }
    // 左边优先。
    if x < scroll {
        return x.max(0.0);
    }
    // 右边夹紧：列的右边界超过视口右边界时把 scroll 往右补。
    let right = x + width;
    if right > scroll + viewport {
        return (right - viewport).max(0.0);
    }
    scroll
}

/// 条带里**一列**的位置（UI 像素，`x` 相对视口左边界）。
///
/// "列"= 包含聚焦面板的、条带最外层横向 split 的直接子节点：默认树里就是
/// 转写列（0.62 视口宽）或右列；picker 打开时是它追加的那一列（0.5 视口宽）。
/// 列里的竖直分裂（右列、picker 的列表/preview）整体算同一列。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnBounds {
    /// 列左边界相对视口左边界的位置（`>= 0`）。
    pub x: f32,
    /// 列宽。
    pub width: f32,
    /// 条带内容总宽：滚动的合法上界是 `content_width - viewport_width`。
    pub content_width: f32,
    /// 条带视口宽（默认树里 = 帧宽）。
    pub viewport_width: f32,
}

impl ColumnBounds {
    /// 把这一列完整滚进视口后的 `scroll`（已按 `[0, content - viewport]` 夹紧）。
    ///
    /// 列已经完整可见时返回传入的 `scroll`（niri 的 "focused window 不该自己
    /// 移动"）；列比视口宽时左贴边。
    pub fn scroll_to(self, scroll: f32) -> f32 {
        let target = scroll_to_visible(scroll, self.x, self.width, self.viewport_width);
        target
            .min(clamp_scroll(self.content_width, self.viewport_width))
            .max(0.0)
    }
}

/// 把 `[scroll, scroll + viewport)` 平移 `delta` 像素，并夹进 `[0, max]`。
///
/// 键位（一屏）和滚轮都走这一条：正 `delta` 往右（内容左移），负 `delta` 往左。
pub fn scroll_by(scroll: f32, delta: f32, total_width: f32, viewport_width: f32) -> f32 {
    (finite_px(scroll) + finite_px(delta))
        .min(clamp_scroll(total_width, viewport_width))
        .max(0.0)
}

/// 条带里包含 `panel` 的最外层横向 split（`niri` 的 strip）。
///
/// 与 [`layout::Node::strip_area`] 同一口径：从根往下第一次遇到"横向 split
/// 且子树里有 `panel`"就是它，更内层的横 split（拆窗后转写列里的）不覆盖。
fn strip_node<'a>(node: &'a layout::Node, panel: PanelId) -> Option<&'a layout::Node> {
    match node {
        layout::Node::Leaf { .. } => None,
        layout::Node::Container { node, .. } => strip_node(node, panel),
        layout::Node::Split {
            direction,
            children,
            ..
        } => {
            let holds = children
                .iter()
                .any(|child| child.node.leaves().contains(&panel));
            if !holds {
                return None;
            }
            match direction {
                layout::Split::Horizontal => Some(node),
                layout::Split::Vertical => children
                    .iter()
                    .find_map(|child| strip_node(&child.node, panel)),
            }
        }
    }
}

/// 条带内容总宽（UI 像素）：最外层横向条带里所有东西的最右边界 − 视口左边。
///
/// 默认树 = 1.0 个视口宽；picker 打开后 = 1.5 个视口宽（右边界向外伸出）。
/// 没有横向条带（树只是一列/一块面板）时等于视口宽——没有可横向滚的内容。
fn strip_content_width(tree: &layout::Node, panel: PanelId, area: TileRect) -> f32 {
    let Some(node) = strip_node(tree, panel) else {
        return area.size.x / layout::PX_TO_METERS;
    };
    let strip = tree.strip_area(area, panel).unwrap_or(area);
    let left = strip.center.x - strip.size.x * 0.5;
    let mut right = left;
    for (_, rect, _) in node.placements(strip) {
        right = right.max(rect.center.x + rect.size.x * 0.5);
    }
    ((right - left) / layout::PX_TO_METERS).max(0.0)
}

/// 面板 `panel` 所在**列**在条带视口里的位置（见 [`ColumnBounds`]）。
///
/// `viewport_width` 是帧宽（UI 像素）。`None` 表示面板不在树上（或视口为 0）、
/// 没有可滚的列；调用方据此跳过，不 panic。
pub fn column_bounds(
    tree: &layout::Node,
    panel: PanelId,
    viewport_width: f32,
) -> Option<ColumnBounds> {
    let viewport = finite_px(viewport_width).max(0.0);
    if viewport <= 0.0 {
        return None;
    }
    // 高度只用来看"这是不是横向条带"，取 1 米足够；真正的视口宽由入参决定。
    let area = TileRect {
        center: Vec2::ZERO,
        size: Vec2::new(viewport * layout::PX_TO_METERS, 1.0),
    };
    let leaves = tree.leaves();
    let Some(node) = strip_node(tree, panel) else {
        // 没有横向条带：整块区域就是视口，没有可滚的列。
        return leaves.contains(&panel).then_some(ColumnBounds {
            x: 0.0,
            width: viewport,
            content_width: viewport,
            viewport_width: viewport,
        });
    };
    let layout::Node::Split { children, .. } = node else {
        return None;
    };
    let child = children
        .iter()
        .find(|child| child.node.leaves().contains(&panel))?;
    let column_panels = child.node.leaves();
    let strip = tree.strip_area(area, panel).unwrap_or(area);
    let strip_left = strip.center.x - strip.size.x * 0.5;
    let strip_width = (strip.size.x / layout::PX_TO_METERS).max(0.0);
    if strip_width <= 0.0 {
        return None;
    }
    // 列的 x/宽 = 这一列里所有叶子（含容器内叶子）的并集；列内竖直分裂
    // 不改变 x 范围，所以并集就是列矩形（容器本身不含叶子，不用算）。
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    for (id, rect, _) in tree.placements(area) {
        if !column_panels.contains(&id) {
            continue;
        }
        min_x = min_x.min(rect.center.x - rect.size.x * 0.5);
        max_x = max_x.max(rect.center.x + rect.size.x * 0.5);
    }
    if !min_x.is_finite() || !max_x.is_finite() {
        return None;
    }
    Some(ColumnBounds {
        x: ((min_x - strip_left) / layout::PX_TO_METERS).max(0.0),
        width: ((max_x - min_x) / layout::PX_TO_METERS).max(0.0),
        content_width: strip_content_width(tree, panel, area),
        viewport_width: strip_width,
    })
}

/// 活动列是**哪块面板**所在的列：picker 打开时是列表那一列，否则是活动
/// 转写根（[`ActiveTranscript`]）所在的列；解析不出来时退回主转写。
pub fn active_panel(roots: &PanelRoots, active_root: Option<Entity>, picker_open: bool) -> PanelId {
    active_panel_with_plugin(roots, active_root, picker_open, None)
}

/// [`active_panel`] 的切片 2 版本：除了 picker / `ActiveTranscript`，还认一个
/// **插件列焦点**（[`ActivePanelColumn`]）。
///
/// 优先级：picker 打开时 picker 列表列（现有语义不变）→ 插件列（只在它的根
/// 还在当前工作区根表里时）→ `ActiveTranscript` 的转写窗格 → 主转写。
pub fn active_panel_with_plugin(
    roots: &PanelRoots,
    active_root: Option<Entity>,
    picker_open: bool,
    plugin: Option<PanelId>,
) -> PanelId {
    if picker_open {
        return PanelId::List;
    }
    if let Some(panel) = plugin.filter(|panel| roots.entity(*panel).is_some()) {
        return panel;
    }
    active_root
        .and_then(|root| {
            roots
                .0
                .iter()
                .find_map(|(id, candidate)| (*candidate == root).then_some(*id))
        })
        .unwrap_or(PanelId::Transcript)
}

/// 一次"把活动列滚进视口"的请求 + 上次活动列的快照。
///
/// 触发口径（`NIRI_SCROLL.md` L3）：
/// * picker 打开/关闭时由 [`crate::picker`] 调 [`WorkspaceScrollFocus::request`]；
/// * 活动列本身变化（`ActiveTranscript` 切换、picker 开关）由这里比对快照；
/// * 帧尺寸变化（窗口 resize）时重新计算并夹紧。
///
/// 用户用键位/滚轮主动滚开之后快照不变，所以**不会被拉回去**——niri 的
/// "focused window 不该自己移动"。
#[derive(Resource, Default, Debug)]
pub struct WorkspaceScrollFocus {
    /// 上一次算活动列时的快照。
    last: Option<ActiveColumnKey>,
    /// 上一次见到的帧尺寸（resize 检测）。
    last_frame: Option<(u16, u16)>,
    /// 待处理的请求（picker 打开/关闭时写）。
    pending: Vec<Entity>,
}

impl WorkspaceScrollFocus {
    /// 请求在下一次布局时把 `workspace` 的活动列滚进视口。
    pub fn request(&mut self, workspace: Entity) {
        if !self.pending.contains(&workspace) {
            self.pending.push(workspace);
        }
    }

    /// `workspace` 有没有还没处理的请求。
    pub fn is_pending(&self, workspace: Entity) -> bool {
        self.pending.contains(&workspace)
    }
}

/// 活动列变化的判定依据（值相等 = 不需要重算 scroll）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveColumnKey {
    workspace: Entity,
    panel: PanelId,
    cols: u16,
    rows: u16,
    picker_open: bool,
}

/// 把聚焦工作区的活动列滚进视口；resize 时夹紧所有工作区的 scroll。
///
/// 只写 [`WorkspaceScroll`]：树、列宽、列 x 一律不碰（niri 不变量 3）。
/// 没有工作区 / 没有 `WorkspaceScroll` 的旧路径直接跳过，不 panic。
fn scroll_active_column(
    set: Option<Res<WorkspaceSet>>,
    frame: Option<Res<FrameSize>>,
    active: Option<Res<ActiveTranscript>>,
    active_column: Option<Res<ActivePanelColumn>>,
    picker: Option<Res<PickerInput>>,
    mut focus: ResMut<WorkspaceScrollFocus>,
    mut workspaces: Query<(
        &Workspace,
        Option<&WorkspacePanels>,
        Option<&mut WorkspaceScroll>,
    )>,
) {
    let (Some(set), Some(frame)) = (set, frame) else {
        return;
    };
    let viewport = f32::from(frame.cols) * CELL.x;
    let picker_open = picker
        .as_deref()
        .is_some_and(|picker| picker.column.is_some());
    let active_root = active.as_deref().and_then(|active| active.0);
    let active_column = active_column.as_deref();
    let frame_key = (frame.cols, frame.rows);
    let frame_changed = focus.last_frame != Some(frame_key);
    focus.last_frame = Some(frame_key);

    let focused_key = set.focused.and_then(|entity| {
        let (_, panels, _) = workspaces.get(entity).ok()?;
        let roots = panels.map(|panels| &panels.roots);
        let plugin = active_column.and_then(|active| active.plugin_for(entity)).map(|panel| panel.id());
        let panel = roots
            .map(|roots| active_panel_with_plugin(roots, active_root, picker_open, plugin))
            .unwrap_or(PanelId::Transcript);
        Some(ActiveColumnKey {
            workspace: entity,
            panel,
            cols: frame.cols,
            rows: frame.rows,
            picker_open,
        })
    });
    let active_changed = focused_key != focus.last;
    focus.last = focused_key;
    let focused_panel = focused_key.map(|key| key.panel);
    let pending = std::mem::take(&mut focus.pending);

    for entity in &set.items {
        let Ok((workspace, _, scroll)) = workspaces.get_mut(*entity) else {
            continue;
        };
        let Some(mut scroll) = scroll else {
            continue;
        };
        let focused = set.focused == Some(*entity);
        let panel = match (focused, focused_panel) {
            (true, Some(panel)) => panel,
            _ => PanelId::Transcript,
        };
        let Some(bounds) = column_bounds(&workspace.tree, panel, viewport) else {
            continue;
        };
        let requested = pending.contains(entity);
        let mut next = scroll.scroll;
        if requested || (focused && active_changed) {
            // 只在必要时滚：列已完整可见时 `scroll_to` 原样返回。
            next = bounds.scroll_to(next);
        } else if frame_changed {
            next = next
                .min(clamp_scroll(bounds.content_width, bounds.viewport_width))
                .max(0.0);
        }
        if next != scroll.scroll {
            scroll.scroll = next;
        }
    }
}

/// 键位横向滚动：`Ctrl+Alt+H/L` 一屏，`Ctrl+Alt+Home/End` 到最左/最右。
///
/// 键位选择见 [`crate::keys::workspace_scroll_bindings`]：`Alt+←/→` 被内置的
/// WordLeft/WordRight 占用；`Ctrl+Alt+←/→` 又会被 picker 的方向键（不看修饰
/// 键）顺带展开/收起说明，所以用 niri 自己的 H/L 约定。键由这里读原始
/// `KeyboardInput`（picker 打开时 `editor::keys` 不跑，绑定表也不分发），
/// 只在改完 `WorkspaceScroll` 后夹进 `[0, max]`。
fn scroll_workspace_keys(
    mut input: MessageReader<KeyboardInput>,
    held: Option<Res<ButtonInput<KeyCode>>>,
    set: Option<Res<WorkspaceSet>>,
    frame: Option<Res<FrameSize>>,
    mut workspaces: Query<(&Workspace, &mut WorkspaceScroll)>,
) {
    let presses: Vec<KeyCode> = input
        .read()
        .filter(|event| event.state == ButtonState::Pressed)
        .map(|event| event.key_code)
        .collect();
    if presses.is_empty() {
        return;
    }
    let (Some(held), Some(set), Some(frame)) = (held, set, frame) else {
        return;
    };
    let modifiers = crate::keys::Modifiers::from_held(&held);
    let control_alt = modifiers.contains(crate::keys::Modifiers::CONTROL)
        && modifiers.contains(crate::keys::Modifiers::ALT);
    let extra = modifiers.intersects(crate::keys::Modifiers::SHIFT | crate::keys::Modifiers::SUPER);
    if !control_alt || extra {
        return;
    }
    let Some(focused) = set.focused else {
        return;
    };
    let Ok((workspace, mut scroll)) = workspaces.get_mut(focused) else {
        return;
    };
    let viewport = f32::from(frame.cols) * CELL.x;
    let Some(bounds) = column_bounds(&workspace.tree, PanelId::Transcript, viewport) else {
        return;
    };
    let total = bounds.content_width;
    let viewport = bounds.viewport_width;
    for code in presses {
        let next = match code {
            KeyCode::KeyH => scroll_by(scroll.scroll, -viewport, total, viewport),
            KeyCode::KeyL => scroll_by(scroll.scroll, viewport, total, viewport),
            KeyCode::Home => 0.0,
            KeyCode::End => clamp_scroll(total, viewport),
            _ => continue,
        };
        if next != scroll.scroll {
            scroll.scroll = next;
        }
    }
}

/// 滚轮横向滚动：`Shift+滚轮` 或横向滚轮轴（`PointerScroll.x`）滚当前工作区。
///
/// 普通滚轮（无 Shift、`y` 方向）不读，仍归转写行滚动（`pane::pane_wheel`）。
/// 一格 = [`WHEEL_CELLS_PER_LINE`] 个字宽；像素单位（触摸板）按像素算。
/// 每滚一次都夹进 `[0, content - viewport]`；只改 `WorkspaceScroll`。
fn scroll_workspace_wheel(
    mut scrolls: MessageReader<PointerScroll>,
    held: Option<Res<ButtonInput<KeyCode>>>,
    set: Option<Res<WorkspaceSet>>,
    frame: Option<Res<FrameSize>>,
    mut workspaces: Query<(&Workspace, &mut WorkspaceScroll)>,
) {
    let shift = held.as_deref().is_some_and(|keys| {
        keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight)
    });
    // 先读完消息：没有工作区时也不能把事件留在队列里。
    let events: Vec<(MouseScrollUnit, f32, f32)> = scrolls
        .read()
        .map(|event| (event.unit, event.x, event.y))
        .collect();
    let (Some(set), Some(frame)) = (set, frame) else {
        return;
    };
    let Some(focused) = set.focused else {
        return;
    };
    let Ok((workspace, mut scroll)) = workspaces.get_mut(focused) else {
        return;
    };
    let viewport = f32::from(frame.cols) * CELL.x;
    let Some(bounds) = column_bounds(&workspace.tree, PanelId::Transcript, viewport) else {
        return;
    };
    let total = bounds.content_width;
    let viewport = bounds.viewport_width;
    let mut current = scroll.scroll;
    for (unit, x, y) in events {
        // 正方向 = 内容往左（scroll 增大）。横向轴正 x 往右；Shift+滚轮往下
        // （y < 0）也往右，和列表滚动的手感一致。
        let raw = if x != 0.0 {
            x
        } else if shift && y != 0.0 {
            -y
        } else {
            continue;
        };
        let delta = match unit {
            MouseScrollUnit::Line => raw * WHEEL_CELLS_PER_LINE * CELL.x,
            MouseScrollUnit::Pixel => raw,
        };
        current = scroll_by(current, delta, total, viewport);
    }
    if current != scroll.scroll {
        scroll.scroll = current;
    }
}

/// 滚轮一格（一行）横向滚几个字宽。
const WHEEL_CELLS_PER_LINE: f32 = 8.0;

/// 列宽/移动列的动作（见 `column_workspace_keys`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColumnAction {
    /// 改活动列宽度：`true` = 往上取最近一档（变宽），`false` = 往下取（变窄）。
    Resize(bool),
    /// 与相邻列交换：`-1` 左、`+1` 右。
    Move(isize),
}

/// 列宽预设与移动列（L4，niri `switch-preset-column-width` /
/// `move-column-left/right`）。
///
/// 键位见 [`crate::keys::workspace_column_bindings`]：
/// * `Alt+Shift+H/L`：活动列按预设宽度 `1/3 → 1/2 → 2/3` 循环——L（往右）
///   往上取最近一档、到头回绕到 `1/3`；H（往左）反过来。默认转写列是
///   `0.62`，第一次按 L 就落到 `2/3`。
/// * `Ctrl+Alt+Shift+H/L`：活动列与左/右邻列**交换**（宽度跟着列走；到边界
///   不动、不 panic）。
///
/// 三条约束（`NIRI_SCROLL.md` L4）：
/// * 只改活动列自己的 `Size::Fraction`；移动时整列（尺寸 + 子树）一起搬，
///   其它列的尺寸约束一个字节不改；
/// * 动作当帧生效：先改 [`Workspace::tree`]，再同步聚焦工作区的全局
///   [`FrameTree`]（`lay_out_frame` 读它），同一帧的布局就按新列宽/新顺序摆；
/// * 改完按需把活动列完整滚进视口（只动 [`WorkspaceScroll`]）。
///
/// picker 打开时 `active_panel` 返回列表所在的列，所以 picker 列也能改宽/
/// 移动；补全 tile 在右列内部（竖直分裂），列的宽度/位置变了它也只是跟着
/// 右列走，尺寸不变。
fn column_workspace_keys(
    mut input: MessageReader<KeyboardInput>,
    held: Option<Res<ButtonInput<KeyCode>>>,
    set: Option<Res<WorkspaceSet>>,
    frame: Option<Res<FrameSize>>,
    active: Option<Res<ActiveTranscript>>,
    active_column: Option<Res<ActivePanelColumn>>,
    picker: Option<Res<PickerInput>>,
    mut frame_tree: Option<ResMut<FrameTree>>,
    mut workspaces: Query<(
        &mut Workspace,
        Option<&WorkspacePanels>,
        Option<&mut WorkspaceScroll>,
    )>,
) {
    // 先读完消息：没有工作区时也不能把事件留在队列里。
    let presses: Vec<KeyCode> = input
        .read()
        .filter(|event| event.state == ButtonState::Pressed)
        .map(|event| event.key_code)
        .collect();
    if presses.is_empty() {
        return;
    }
    let (Some(held), Some(set), Some(frame)) = (held, set, frame) else {
        return;
    };
    let Some(focused) = set.focused else {
        return;
    };
    let Ok((mut workspace, panels, mut scroll)) = workspaces.get_mut(focused) else {
        return;
    };
    let modifiers = crate::keys::Modifiers::from_held(&held);
    let control = modifiers.contains(crate::keys::Modifiers::CONTROL);
    let alt = modifiers.contains(crate::keys::Modifiers::ALT);
    let shift = modifiers.contains(crate::keys::Modifiers::SHIFT);
    // Super 一起按着不算（那是别的绑定）。
    if modifiers.contains(crate::keys::Modifiers::SUPER) {
        return;
    }
    let picker_open = picker
        .as_deref()
        .is_some_and(|picker| picker.column.is_some());
    let active_root = active.as_deref().and_then(|active| active.0);
    let plugin = active_column
        .as_deref()
        .and_then(|active| active.plugin_for(focused))
        .map(|panel| panel.id());
    let panel = panels
        .map(|panels| active_panel_with_plugin(&panels.roots, active_root, picker_open, plugin))
        .unwrap_or(PanelId::Transcript);
    let viewport = f32::from(frame.cols) * CELL.x;

    for code in presses {
        let action = match (control, alt, shift, code) {
            // 移动列：L3 的方向键（Ctrl+Alt）再加 Shift。
            (true, true, true, KeyCode::KeyH) => Some(ColumnAction::Move(-1)),
            (true, true, true, KeyCode::KeyL) => Some(ColumnAction::Move(1)),
            // 改列宽：Alt+Shift，不带 Ctrl（和滚动/移动都不重叠）。
            (false, true, true, KeyCode::KeyH) => Some(ColumnAction::Resize(false)),
            (false, true, true, KeyCode::KeyL) => Some(ColumnAction::Resize(true)),
            _ => continue,
        };
        let Some(action) = action else { continue };
        let changed = match action {
            ColumnAction::Resize(forward) => {
                let Some(current) = workspace.tree.column_fraction(panel) else {
                    continue;
                };
                let next = if forward {
                    layout::next_preset_fraction(current)
                } else {
                    layout::previous_preset_fraction(current)
                };
                workspace.tree.set_column_fraction(panel, next)
            }
            ColumnAction::Move(delta) => workspace.tree.move_column(panel, delta),
        };
        if !changed {
            continue;
        }
        // 聚焦工作区的布局读全局树：同一帧同步过去，动作才是"当帧生效"；
        // `commit_focused_panels` 在 Render 会把全局树写回组件（口径不变）。
        if let Some(tree) = frame_tree.as_deref_mut() {
            tree.0 = workspace.tree.clone();
        }
        // 改宽/换位会改变活动列的位置或宽度：按需把它完整滚进视口。
        if let Some(scroll) = scroll.as_deref_mut()
            && let Some(bounds) = column_bounds(&workspace.tree, panel, viewport)
        {
            let next = bounds.scroll_to(scroll.scroll);
            if next != scroll.scroll {
                scroll.scroll = next;
            }
        }
    }
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
            WorkspaceScroll::default(),
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
                WorkspaceScroll::default(),
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

/// 工作区插件：启动收编 + 聚焦装载 + 新面板打标签 + L3/L4 横向滚动与列操作。
///
/// L3 的三条系统（均只改 [`WorkspaceScroll`]）：
/// * `scroll_active_column`：活动列变化 / picker 开关请求 / 窗口 resize 时
///   把聚焦工作区的活动列滚进视口并夹紧；
/// * `scroll_workspace_keys`：`Ctrl+Alt+H/L` 一屏、`Ctrl+Alt+Home/End` 到两端；
/// * `scroll_workspace_wheel`：`Shift+滚轮` / 横向滚轮轴滚当前工作区。
///
/// L4 的一条系统（见 [`column_workspace_keys`]）：
/// * `column_workspace_keys`：`Alt+Shift+H/L` 改活动列宽（三档预设循环）、
///   `Ctrl+Alt+Shift+H/L` 把活动列与左右邻列交换；改完按需滚进视口。
///
/// 放置（`frame::lay_out_frame` / `frame::place_panels`）和行同步
/// （`panels::sync_panels`）在自己的插件里按 [`WorkspaceSet`] 分派，不在这里。
pub struct WorkspacesPlugin;

impl Plugin for WorkspacesPlugin {
    fn build(&self, app: &mut App) {
        // L3/L4 的全局绑定：只用来遮住 Input 作用域的同键回退 / 占住键
        //（键本身由 `scroll_workspace_keys` / `column_workspace_keys` 读，
        // picker 打开时也能用）。
        crate::keys::add_workspace_scroll_keys(app);
        crate::keys::add_workspace_column_keys(app);
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
        // 切片 2：插件列焦点（`ActiveTranscript` 之外的另一个焦点）。
        .init_resource::<ActivePanelColumn>()
        // 第 3 批：拖拽状态（中键 / Alt+左键，见 `drag_workspaces`）。
        .init_resource::<WorkspaceDrag>()
        // L3：活动列快照 + picker 打开/关闭的滚动请求。
        .init_resource::<WorkspaceScrollFocus>()
        // L3：键位/滚轮读同一批原始输入事件（headless 测试也能写）。
        .add_message::<KeyboardInput>()
        .add_message::<PointerScroll>()
        // niri 模式的屏幕在 `PostStartup` 生成：先让它写全局资源，再收编。
        .add_systems(
            PostStartup,
            bootstrap_workspaces.after(crate::niri::spawn_niri_screen),
        )
        .add_systems(
            Update,
            apply_focus.in_set(FeathersSystems::Prepare),
        )
        // 切片 2：把注册表里“自动装进所有工作区”的面板补齐到每个工作区
        // （已有工作区 + 之后 spawn 的工作区）。exclusive system：要 &mut World
        // 调 `panel::add_panel_column`；排在 `apply_focus` 之后，全局代理已就位。
        .add_systems(
            Update,
            crate::panel::apply_auto_panels
                .in_set(FeathersSystems::Prepare)
                .after(apply_focus),
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
        )
        // L3：活动列滚进视口 + 键位/滚轮横向滚动。都排在 `lay_out_frame`
        // 之前，同一帧就能看到新 scroll；键位/滚轮排在活动列之后，用户
        // 这一帧的操作压过"自动滚到活动列"。
        // L4：改列宽/移动列排在滚动之后，改完把活动列重新滚进视口。
        .add_systems(
            Update,
            (
                scroll_active_column,
                scroll_workspace_keys,
                scroll_workspace_wheel,
                column_workspace_keys,
            )
                .chain()
                .in_set(FeathersSystems::Layout)
                .before(crate::frame::lay_out_frame),
        );
    }
}

#[cfg(test)]
mod tests {
    //! 每个工作区一套面板状态的 headless 测试：不建窗口、不跑渲染。
    use super::*;
    use bevy::camera::NormalizedRenderTarget;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::input::mouse::MouseScrollUnit;
    use bevy::input::touch::TouchPhase;
    use bevy::picking::backend::HitData;
    use bevy::picking::events::{Pointer, PointerScroll};
    use bevy::picking::pointer::{Location, PointerId};
    use bevy::text::FontSource;
    use rig_harness::prelude::Picker;
    use rig_tui::ratatui::text::Line;

    use crate::frame::{FrameGeometry, FramePlugin, FrameSize, CELL, UNITS_TO_WORLD};
    use crate::layout::{Child, Node as LayoutNode, Split, NO_GAP};
    use crate::niri::TileRect;
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
            // L3：手动摆 picker 状态（不装 PickerPlugin 的测试也能覆盖滚动）。
            .init_resource::<PickerInput>()
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
        // 转写宽 0.62 × 96 = 59.52 列，中心 = 59.52/2 - 48 = -18.24 列。
        let want_x = -18.24 * CELL.x * UNITS_TO_WORLD;
        assert!(
            (anchor.x - want_x).abs() < 1e-5,
            "默认工作区的转写锚点 x = {}",
            anchor.x
        );
        // 转写只占条带上半部分（下面还有提示行/状态行两条横跨的 tile），
        // 所以它的锚点 y 是"上半部分的中心"，不再等于 0。
        let want_y = 1.5 * crate::transcript::ROW_HEIGHT * crate::layout::PX_TO_METERS;
        assert!((anchor.y - want_y).abs() < 1e-5, "转写锚点 y = {}", anchor.y);
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
            // 留白叶子只吃空间、没有面板实体，不参与"每片叶子都有区域"。
            let drawn = rects.iter().filter(|(id, _)| *id != PanelId::Spacer).count();
            assert_eq!(
                panels.areas.len(),
                drawn,
                "工作区 {workspace:?} 的每个叶子都有区域（留白除外）"
            );
            for (id, tile) in rects {
                if id == PanelId::Spacer {
                    continue;
                }
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
                if id == PanelId::Spacer {
                    continue;
                }
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
        // 转写宽 0.62 × 96 = 59.52 列，中心 = 59.52/2 - 48 = -18.24 列。
        let want_x = -18.24 * CELL.x * UNITS_TO_WORLD;
        assert!((anchor.x - want_x).abs() < 1e-5, "anchor.x = {}", anchor.x);
        // 转写只占条带上半部分（下面还有提示行/状态行两条横跨的 tile），
        // 所以它的锚点 y 是"上半部分的中心"，不再等于 0。
        let want_y = 1.5 * crate::transcript::ROW_HEIGHT * crate::layout::PX_TO_METERS;
        assert!((anchor.y - want_y).abs() < 1e-5, "转写锚点 y = {}", anchor.y);
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

    // ---- L2：每工作区一个横向 scroll + 条带视口裁剪 ----

    /// 工作区里每块面板这一帧的**精确**中心（UI 像素，相对帧中心）。
    fn area_centers(app: &App, workspace: Entity) -> HashMap<PanelId, Vec2> {
        let panels = app
            .world()
            .get::<WorkspacePanels>(workspace)
            .expect("WorkspacePanels");
        let mut out = HashMap::new();
        for id in [
            PanelId::Transcript,
            PanelId::Input,
            PanelId::Status,
            PanelId::Complete,
            PanelId::Hints,
            PanelId::Confirm,
        ] {
            let Some(root) = panels.roots.entity(id) else {
                continue;
            };
            let area = panels.area(root).unwrap_or_else(|| panic!("{id:?} 有区域"));
            out.insert(id, area.center.unwrap_or_else(|| panic!("{id:?} 有精确中心")));
        }
        out
    }

    /// 面板 `Node` 上的像素尺寸（`place_panels` 每帧写入）。
    fn panel_px(app: &App, root: Entity) -> Vec2 {
        let node = app.world().get::<Node>(root).expect("Node");
        let px = |value: bevy::ui::Val| match value {
            bevy::ui::Val::Px(value) => value,
            _ => 0.0,
        };
        Vec2::new(px(node.width), px(node.height))
    }

    /// 面板在世界里的轴对齐矩形（米）：锚点是中心，`Node` 像素 × units。
    fn panel_rect(app: &App, root: Entity) -> (feathers_in_3d::WorldPanel, bevy::math::Rect) {
        let world = *app
            .world()
            .get::<feathers_in_3d::WorldPanel>(root)
            .expect("WorldPanel");
        let size = panel_px(app, root) * world.units_to_world;
        (world, feathers_in_3d::panel_world_rect(&world.anchor, size))
    }

    // 28. `clamp_scroll` / `scroll_to_visible` 的边界：左、右、比视口宽、负值。
    #[test]
    fn scroll_helpers_clamp_to_the_valid_range() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        // clamp：总宽 <= 视口 → 0；否则 total - viewport。
        assert_eq!(clamp_scroll(0.0, 0.0), 0.0);
        assert_eq!(clamp_scroll(1200.0, 1200.0), 0.0);
        assert!(near(clamp_scroll(1200.0, 748.8), 451.2));
        assert_eq!(clamp_scroll(100.0, 748.8), 0.0, "内容比视口窄：不滚");
        assert_eq!(clamp_scroll(-5.0, 100.0), 0.0, "负总宽当 0");
        assert_eq!(clamp_scroll(f32::NAN, 100.0), 0.0, "NaN 当 0");
        assert_eq!(clamp_scroll(100.0, f32::INFINITY), 0.0, "∞ 视口 = 全可见");

        // 已经在视口里：一个像素都不动。
        assert_eq!(scroll_to_visible(40.0, 100.0, 200.0, 748.8), 40.0);
        // 列在视口左边：滚到列的左边（左边优先）。
        assert_eq!(scroll_to_visible(300.0, 120.0, 200.0, 748.8), 120.0);
        // 列在视口右边：滚到列右边界贴视口右边界。
        assert!(near(
            scroll_to_visible(0.0, 700.0, 200.0, 748.8),
            151.2
        ));
        // 列比视口宽：左贴边（右边界必然超出）。
        assert_eq!(scroll_to_visible(500.0, 300.0, 1200.0, 748.8), 300.0);
        // 负 scroll / 负 x 夹到 0；0 宽视口不滚。
        assert_eq!(scroll_to_visible(-50.0, 100.0, 200.0, 748.8), 0.0);
        assert_eq!(scroll_to_visible(0.0, -50.0, 200.0, 748.8), 0.0);
        assert_eq!(scroll_to_visible(50.0, 100.0, 200.0, 0.0), 0.0);
        // 夹过的结果再交给 clamp：不超过内容末端。
        let target = scroll_to_visible(0.0, 1000.0, 200.0, 748.8);
        assert!(near(clamp_scroll(1400.0, 748.8), 651.2));
        assert!(target <= clamp_scroll(1400.0, 748.8));
    }

    // 29. 给工作区设 scroll=200px：只滚转写列，右列（Input/Confirm/Complete）
    //     与 Hints/Status 是固定 chrome、一个像素都不动（`NIRI_SCROLL.md` §2.1）；
    //     面板尺寸不变；另一个工作区一个像素都不动。
    #[test]
    fn scrolling_a_workspace_moves_the_strip_but_not_the_chrome_rows() {
        const SCROLL: f32 = 200.0;
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let before_a = area_centers(&app, a);
        let before_b = area_centers(&app, b);
        let sizes_before = panel_sizes(&app, a);
        let anchor_before = app
            .world()
            .get::<feathers_in_3d::WorldPanel>(root_of(&app, a, PanelId::Transcript))
            .expect("WorldPanel")
            .anchor
            .translation;

        app.world_mut()
            .get_mut::<WorkspaceScroll>(a)
            .expect("工作区带 WorkspaceScroll")
            .scroll = SCROLL;
        app.update();

        let after_a = area_centers(&app, a);
        let transcript = PanelId::Transcript;
        assert!(
            (after_a[&transcript].x - (before_a[&transcript].x - SCROLL)).abs() < 1e-3,
            "{transcript:?} 在条带里，应该左移 {SCROLL}px：{} -> {}",
            before_a[&transcript].x,
            after_a[&transcript].x
        );
        assert_eq!(
            after_a[&transcript].y, before_a[&transcript].y,
            "{transcript:?} 不上下动"
        );
        // niri：条带里的**所有**列一起滚，右列的输入/确认/补全也是列。
        for id in [PanelId::Confirm, PanelId::Complete, PanelId::Input] {
            assert!(
                (after_a[&id].x - (before_a[&id].x - SCROLL)).abs() < 1e-3,
                "{id:?} 在条带里，应该跟着左移 {SCROLL}px：{} -> {}",
                before_a[&id].x,
                after_a[&id].x
            );
        }
        for id in [PanelId::Hints, PanelId::Status] {
            assert_eq!(
                after_a[&id], before_a[&id],
                "{id:?} 是横跨整条的 chrome，固定在视口里"
            );
        }
        // 3D 锚点同样左移 200px（世界米）：这是"转写列 rect 左移"的直接证据。
        let anchor_after = app
            .world()
            .get::<feathers_in_3d::WorldPanel>(root_of(&app, a, PanelId::Transcript))
            .expect("WorldPanel")
            .anchor
            .translation;
        assert!(
            (anchor_after.x - (anchor_before.x - SCROLL * UNITS_TO_WORLD)).abs() < 1e-5,
            "转写锚点 x：{} -> {}（期望左移 {SCROLL}px）",
            anchor_before.x,
            anchor_after.x
        );
        // 尺寸不因滚动而改（裁剪不是把面板改小）。
        assert_eq!(panel_sizes(&app, a), sizes_before);
        // 非聚焦工作区不受影响。
        assert_eq!(area_centers(&app, b), before_b);
    }

    // 30. scroll = 0（默认）：每个工作区面板都挂条带视口，但有效裁剪矩形 == 自身矩形。
    #[test]
    fn without_scroll_every_panel_clips_to_its_own_rect() {
        let app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        for workspace in [a, b] {
            let panels = app
                .world()
                .get::<WorkspacePanels>(workspace)
                .expect("WorkspacePanels");
            let mut seen = 0;
            for id in [
                PanelId::Transcript,
                PanelId::Input,
                PanelId::Status,
                PanelId::Complete,
                PanelId::Hints,
                PanelId::Confirm,
            ] {
                let Some(root) = panels.roots.entity(id) else {
                    continue;
                };
                let (world, rect) = panel_rect(&app, root);
                let clip = world.clip.unwrap_or_else(|| panic!("{id:?} 要有条带视口"));
                let effective = feathers_in_3d::effective_clip(rect, Some(clip));
                assert!(
                    (effective.min - rect.min).length() < 1e-4
                        && (effective.max - rect.max).length() < 1e-4,
                    "{id:?} 的裁剪矩形必须等于面板自身：{effective:?} vs {rect:?}"
                );
                seen += 1;
            }
            assert!(seen >= 6, "六块面板都要检查（实际 {seen}）");
        }
    }

    // 31. 面板整个滚出视口：仍然存在（不 despawn），有效裁剪矩形为空、不 panic。
    #[test]
    fn a_panel_fully_outside_the_viewport_stays_alive_with_an_empty_clip() {
        let mut app = app_with_two_workspaces();
        let (a, _) = workspaces(&app);
        app.world_mut()
            .get_mut::<WorkspaceScroll>(a)
            .expect("WorkspaceScroll")
            .scroll = 10_000.0;
        app.update();

        let root = root_of(&app, a, PanelId::Transcript);
        assert!(
            app.world().get_entity(root).is_ok(),
            "滚出视口不 despawn 面板"
        );
        let (world, rect) = panel_rect(&app, root);
        let clip = world.clip.expect("条带视口还在");
        let effective = feathers_in_3d::effective_clip(rect, Some(clip));
        assert!(
            effective.is_empty(),
            "完全在视口外：交集为空（{effective:?}）"
        );
        // 空交集也算得出材质 uniform（宽/高 0），没有 panic。
        assert!(effective.width() == 0.0 || effective.height() == 0.0);
    }

    // ---- L3：滚动接上交互（活动列必须可见 + 键位/滚轮 + resize 夹紧） ----

    /// 视口宽（UI 像素）：测试统一用 96×28 帧。
    fn viewport_px(app: &App) -> f32 {
        f32::from(app.world().resource::<FrameSize>().cols) * CELL.x
    }

    /// 帧矩形（米），给 `tree.rects` 断言列几何用。
    fn frame_tile(app: &App) -> TileRect {
        let frame = *app.world().resource::<FrameSize>();
        TileRect {
            center: Vec2::ZERO,
            size: Vec2::new(
                f32::from(frame.cols) * CELL.x,
                f32::from(frame.rows) * CELL.y,
            ) * UNITS_TO_WORLD,
        }
    }

    fn workspace_tree(app: &App, workspace: Entity) -> LayoutNode {
        app.world()
            .get::<Workspace>(workspace)
            .expect("Workspace")
            .tree
            .clone()
    }

    /// 换掉 `workspace` 的树，并同步聚焦工作区的全局 `FrameTree`
    ///（`lay_out_frame` 对聚焦工作区读全局树）。
    fn set_workspace_tree(app: &mut App, workspace: Entity, tree: LayoutNode) {
        if let Some(mut component) = app.world_mut().get_mut::<Workspace>(workspace) {
            component.tree = tree.clone();
        }
        if app.world().resource::<WorkspaceSet>().focused == Some(workspace) {
            app.world_mut().resource_mut::<FrameTree>().0 = tree;
        }
    }

    /// 打开路径真实追加的那一列：`Picker` 容器 + `List` 叶子。
    fn picker_column() -> LayoutNode {
        LayoutNode::container(PanelId::Picker, LayoutNode::leaf(PanelId::List))
    }

    fn append_picker_column(app: &mut App, workspace: Entity) -> LayoutNode {
        let mut tree = workspace_tree(app, workspace);
        assert!(tree.append_column_at(PanelId::Transcript, picker_column()));
        set_workspace_tree(app, workspace, tree.clone());
        tree
    }

    fn scroll_of(app: &App, workspace: Entity) -> f32 {
        app.world()
            .get::<WorkspaceScroll>(workspace)
            .expect("WorkspaceScroll")
            .scroll
    }

    /// 手动摆 picker 状态（不跑 `sync_picker`；树上那一列由调用方追加）。
    fn set_picker_open(app: &mut App, open: bool) {
        if !open {
            *app.world_mut().resource_mut::<PickerInput>() = PickerInput::default();
            return;
        }
        let column = app.world_mut().spawn_empty().id();
        let mut state = app.world_mut().resource_mut::<PickerInput>();
        state.picker = Some(column);
        state.column = Some(column);
    }

    fn mouse_pointer() -> Pointer {
        Pointer::new(
            PointerId::Mouse,
            Location {
                target: NormalizedRenderTarget::None {
                    width: 1280,
                    height: 800,
                },
                position: Vec2::ZERO,
            },
        )
    }

    /// 写一次 `PointerScroll`（`x` 横向轴，`y` 纵向轴）并跑一帧。
    fn wheel(app: &mut App, unit: MouseScrollUnit, x: f32, y: f32) {
        app.world_mut().write_message(PointerScroll {
            entity: Entity::PLACEHOLDER,
            pointer: mouse_pointer(),
            unit,
            x,
            y,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            phase: TouchPhase::Moved,
        });
        app.update();
    }

    /// 写一次键盘按下并跑一帧（修饰键状态由调用方提前按在 `ButtonInput` 里）。
    fn press_code(app: &mut App, code: KeyCode) {
        let logical = match code {
            KeyCode::KeyH => Key::Character("h".into()),
            KeyCode::KeyL => Key::Character("l".into()),
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            _ => Key::Character("x".into()),
        };
        app.world_mut().write_message(KeyboardInput {
            key_code: code,
            logical_key: logical,
            state: ButtonState::Pressed,
            repeat: false,
            text: None,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    fn set_key(app: &mut App, code: KeyCode, pressed: bool) {
        let mut held = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        if pressed {
            held.press(code);
        } else {
            held.release(code);
        }
    }

    // 32. 列几何：转写列在视口左端；picker 列从视口右边界开始、伸出视口；
    //     `scroll_to` 只在必要时动，列比视口宽时左贴边。
    #[test]
    fn column_bounds_finds_the_transcript_and_the_picker_column() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        let viewport = 96.0 * CELL.x;
        let mut tree = layout::default_tree();
        let transcript = column_bounds(&tree, PanelId::Transcript, viewport).expect("转写列");
        assert!(near(transcript.x, 0.0));
        assert!(near(
            transcript.width,
            viewport * layout::TRANSCRIPT_COLUMN_FRACTION
        ));
        assert!(near(transcript.content_width, viewport), "默认树正好一个视口宽");
        assert_eq!(transcript.scroll_to(0.0), 0.0, "已完整可见：不动");
        assert_eq!(transcript.scroll_to(120.0), 0.0, "列在视口左边：滚回 0");

        assert!(tree.append_column_at(PanelId::Transcript, picker_column()));
        let picker = column_bounds(&tree, PanelId::List, viewport).expect("picker 列");
        assert!(near(picker.x, viewport), "picker 列从视口右边界开始");
        assert!(near(
            picker.width,
            viewport * layout::DEFAULT_COLUMN_FRACTION
        ));
        assert!(near(picker.content_width, viewport * 1.5), "内容比视口宽半个屏");
        assert!(
            near(picker.scroll_to(0.0), viewport * 0.5),
            "滚到能完整看见这一列"
        );
        assert!(
            near(picker.scroll_to(viewport * 0.5), viewport * 0.5),
            "已完整可见：一个像素都不动"
        );

        // 列比视口宽：左贴边（x 是列自己左边界，不是 0）。
        let wide = LayoutNode::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::fraction(0.5, LayoutNode::leaf(PanelId::Spacer)),
                Child::fraction(1.5, LayoutNode::leaf(PanelId::Transcript)),
            ],
        );
        let bounds = column_bounds(&wide, PanelId::Transcript, viewport).expect("宽列");
        assert!(near(bounds.x, viewport * 0.5));
        assert!(near(bounds.width, viewport * 1.5));
        assert!(
            near(bounds.scroll_to(0.0), viewport * 0.5),
            "列比视口宽：左贴边"
        );
    }

    // 33. picker 打开（列伸出视口）→ scroll 把该列完整展示；关闭 → 滚回让
    //     转写可见。全程只改 `WorkspaceScroll`，树的列 x/宽度一个字节不变。
    #[test]
    fn opening_an_off_screen_picker_scrolls_it_in_and_closing_scrolls_back() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let viewport = viewport_px(&app);
        let tree_before = append_picker_column(&mut app, a);
        let rects_before = tree_before.rects(frame_tile(&app));
        assert_eq!(scroll_of(&app, a), 0.0);

        set_picker_open(&mut app, true);
        app.update();

        let scroll = scroll_of(&app, a);
        assert!(
            (scroll - viewport * layout::DEFAULT_COLUMN_FRACTION).abs() < 1e-3,
            "picker 列应完整可见：scroll = 半个视口，实际 {scroll}"
        );
        assert_eq!(workspace_tree(&app, a), tree_before, "滚动不改树");
        assert_eq!(
            workspace_tree(&app, a).rects(frame_tile(&app)),
            rects_before,
            "列 x/宽度一个字节不变（niri 不变量 3）"
        );
        assert_eq!(scroll_of(&app, b), 0.0, "别的工作区不动");

        set_picker_open(&mut app, false);
        app.update();
        assert_eq!(scroll_of(&app, a), 0.0, "关闭后滚回去让转写可见");
        assert_eq!(workspace_tree(&app, a), tree_before, "关闭也不改树");
    }

    // 34. 活动列已完整可见时，打开/关闭 picker 都不改变 scroll
    //     （niri：focused window 不该自己移动）。
    #[test]
    fn opening_and_closing_a_visible_picker_does_not_move_the_strip() {
        // 窄树：转写 0.3 视口 + picker 0.3 视口，两列都在视口里。
        let mut app = app_with_two_workspaces();
        let (a, _) = workspaces(&app);
        let tree = LayoutNode::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::fraction(0.3, LayoutNode::leaf(PanelId::Transcript)),
                Child::fraction(0.3, picker_column()),
            ],
        );
        set_workspace_tree(&mut app, a, tree);
        app.update();
        assert_eq!(scroll_of(&app, a), 0.0);

        set_picker_open(&mut app, true);
        app.update();
        assert_eq!(scroll_of(&app, a), 0.0, "picker 列已完整可见：不动");

        set_picker_open(&mut app, false);
        app.update();
        assert_eq!(scroll_of(&app, a), 0.0, "转写也完整可见：不动");
    }

    // 35. picker 打开前已经滚到能看见它：打开不再移动 scroll；关闭时转写
    //     不可见才滚回去（只在必要时）。
    #[test]
    fn a_picker_that_is_already_visible_is_not_moved_on_open() {
        let mut app = app_with_two_workspaces();
        let (a, _) = workspaces(&app);
        let viewport = viewport_px(&app);
        append_picker_column(&mut app, a);
        app.world_mut()
            .get_mut::<WorkspaceScroll>(a)
            .expect("WorkspaceScroll")
            .scroll = viewport * 0.5;
        app.update();
        assert!((scroll_of(&app, a) - viewport * 0.5).abs() < 1e-3);

        set_picker_open(&mut app, true);
        app.update();
        assert!(
            (scroll_of(&app, a) - viewport * 0.5).abs() < 1e-3,
            "picker 列已经完整可见，不该再动"
        );

        set_picker_open(&mut app, false);
        app.update();
        assert_eq!(scroll_of(&app, a), 0.0, "转写看不见了，才滚回去");
    }

    // 36. 键位：Ctrl+Alt+H/L 一屏、Ctrl+Alt+Home/End 到两端；夹在 [0, max]，
    //     列宽/列 x/面板尺寸都不变。
    #[test]
    fn keyboard_scrolling_stays_inside_the_strip_and_leaves_columns_alone() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let viewport = viewport_px(&app);
        let max = viewport * layout::DEFAULT_COLUMN_FRACTION;
        let tree_before = append_picker_column(&mut app, a);
        let rects_before = tree_before.rects(frame_tile(&app));
        let sizes_before = panel_sizes(&app, a);
        app.init_resource::<ButtonInput<KeyCode>>();
        set_key(&mut app, KeyCode::ControlLeft, true);
        set_key(&mut app, KeyCode::AltLeft, true);

        // 一次一屏，但右端只有半屏：夹到 max。
        press_code(&mut app, KeyCode::KeyL);
        assert!((scroll_of(&app, a) - max).abs() < 1e-3, "右端夹住");
        press_code(&mut app, KeyCode::Home);
        assert_eq!(scroll_of(&app, a), 0.0);
        press_code(&mut app, KeyCode::KeyH);
        assert_eq!(scroll_of(&app, a), 0.0, "左端夹住");
        press_code(&mut app, KeyCode::End);
        assert!((scroll_of(&app, a) - max).abs() < 1e-3);
        press_code(&mut app, KeyCode::KeyL);
        assert!((scroll_of(&app, a) - max).abs() < 1e-3, "到右端不越界");

        // 没有修饰键时不是滚动键。
        set_key(&mut app, KeyCode::AltLeft, false);
        set_key(&mut app, KeyCode::ControlLeft, false);
        press_code(&mut app, KeyCode::KeyH);
        assert!((scroll_of(&app, a) - max).abs() < 1e-3, "普通 H 不滚");

        assert_eq!(workspace_tree(&app, a), tree_before, "不改树");
        assert_eq!(
            workspace_tree(&app, a).rects(frame_tile(&app)),
            rects_before,
            "列 x/宽度不变"
        );
        assert_eq!(panel_sizes(&app, a), sizes_before, "面板尺寸不变");
        assert_eq!(scroll_of(&app, b), 0.0, "非聚焦工作区不动");
    }

    // 37. 滚轮：Shift+滚轮 / 横向轴滚当前工作区，普通滚轮不横向滚；夹紧。
    #[test]
    fn shift_wheel_and_horizontal_wheel_scroll_only_the_workspace() {
        let mut app = app_with_two_workspaces();
        let (a, _) = workspaces(&app);
        let viewport = viewport_px(&app);
        let max = viewport * layout::DEFAULT_COLUMN_FRACTION;
        append_picker_column(&mut app, a);
        app.init_resource::<ButtonInput<KeyCode>>();
        set_key(&mut app, KeyCode::ShiftLeft, true);

        wheel(&mut app, MouseScrollUnit::Line, 0.0, -3.0);
        let step = 3.0 * WHEEL_CELLS_PER_LINE * CELL.x;
        assert!(
            (scroll_of(&app, a) - step.min(max)).abs() < 1e-3,
            "Shift+滚轮下 = 往右一屏的 1/4 左右"
        );
        wheel(&mut app, MouseScrollUnit::Line, 0.0, -100.0);
        assert!((scroll_of(&app, a) - max).abs() < 1e-3, "夹到右端");
        wheel(&mut app, MouseScrollUnit::Line, 0.0, 100.0);
        assert_eq!(scroll_of(&app, a), 0.0, "夹到左端");

        // 普通滚轮（没有 Shift、没有横向轴）不横向滚。
        set_key(&mut app, KeyCode::ShiftLeft, false);
        wheel(&mut app, MouseScrollUnit::Line, 0.0, -3.0);
        assert_eq!(scroll_of(&app, a), 0.0);

        // 横向滚轮轴（没有 Shift 也算）。
        wheel(&mut app, MouseScrollUnit::Line, 2.0, 0.0);
        let horizontal = (2.0 * WHEEL_CELLS_PER_LINE * CELL.x).min(max);
        assert!((scroll_of(&app, a) - horizontal).abs() < 1e-3);
    }

    // 38. resize 后 scroll 被夹进新的 [0, max]，仍然完整展示活动列。
    #[test]
    fn resizing_the_window_clamps_the_strip_scroll() {
        let mut app = app_with_two_workspaces();
        let (a, _) = workspaces(&app);
        let viewport = viewport_px(&app);
        append_picker_column(&mut app, a);
        set_picker_open(&mut app, true);
        app.update();
        assert!(
            (scroll_of(&app, a) - viewport * layout::DEFAULT_COLUMN_FRACTION).abs() < 1e-3
        );

        *app.world_mut().resource_mut::<FrameSize>() = FrameSize { cols: 64, rows: 28 };
        app.update();

        let viewport = 64.0 * CELL.x;
        let max = clamp_scroll(viewport * 1.5, viewport);
        let scroll = scroll_of(&app, a);
        assert!(scroll >= 0.0 && scroll <= max + 1e-3, "夹在 [0, {max}]，实际 {scroll}");
        assert!((scroll - max).abs() < 1e-3, "活动列（picker）仍完整可见");
    }

    // 39. 没有工作区 / 没有 WorkspaceScroll 的旧路径不 panic。
    #[test]
    fn scrolling_without_workspaces_or_scroll_does_not_panic() {
        let mut app = App::new();
        app.init_resource::<WorkspaceSet>()
            .init_resource::<WorkspaceScrollFocus>()
            .insert_resource(FrameSize { cols: 96, rows: 28 })
            .add_systems(Update, scroll_active_column);
        app.update();

        let workspace = app
            .world_mut()
            .spawn(Workspace {
                session: 0,
                tree: layout::default_tree(),
                pose: Transform::IDENTITY,
                order: 0,
            })
            .id();
        {
            let mut set = app.world_mut().resource_mut::<WorkspaceSet>();
            set.items.push(workspace);
            set.focused = Some(workspace);
        }
        app.update();
        assert!(
            app.world().get::<WorkspaceScroll>(workspace).is_none(),
            "没有 WorkspaceScroll 也不 panic、不凭空加组件"
        );
    }

    // 40. 真 PickerPlugin：打开时 `sync_picker` 追加列并请求滚动；despawn 关闭
    //     后滚回让转写可见（不依赖测试里手摆的 PickerInput）。
    #[test]
    fn a_real_picker_open_and_close_scrolls_the_workspace() {
        let mut app = workspace_app();
        app.init_resource::<crate::theme::Theme>()
            .add_plugins(crate::picker::PickerPlugin);
        app.update(); // `PostStartup` 收编出默认工作区
        let focused = app
            .world()
            .resource::<WorkspaceSet>()
            .focused
            .expect("默认工作区");
        let viewport = viewport_px(&app);
        assert_eq!(scroll_of(&app, focused), 0.0);

        let picker = app
            .world_mut()
            .spawn(Picker::new("test").item("a", ""))
            .id();
        app.update();
        assert!(
            workspace_tree(&app, focused)
                .leaves()
                .contains(&PanelId::List),
            "picker 列加进树"
        );
        let scroll = scroll_of(&app, focused);
        assert!(
            (scroll - viewport * layout::DEFAULT_COLUMN_FRACTION).abs() < 1e-3,
            "打开后滚到 picker 列：{scroll}"
        );
        assert!(
            !app.world()
                .resource::<WorkspaceScrollFocus>()
                .is_pending(focused),
            "打开时的滚动请求已在布局里消费掉"
        );

        app.world_mut().entity_mut(picker).despawn();
        app.update();
        assert_eq!(scroll_of(&app, focused), 0.0, "关闭后滚回去看转写");
    }

    // 41. `ActiveTranscript` 切换：活动列从第二块转写切到伸出视口的那一列时
    //     滚过去；切回已可见的列时不动（只在必要时滚）。
    #[test]
    fn switching_the_active_transcript_column_scrolls_it_into_view() {
        let mut app = app_with_two_workspaces();
        let (a, _) = workspaces(&app);
        let viewport = viewport_px(&app);
        let tree = LayoutNode::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::fraction(0.5, LayoutNode::leaf(PanelId::Transcript)),
                Child::fraction(0.5, LayoutNode::leaf(PanelId::Spacer)),
                Child::fraction(0.5, LayoutNode::leaf(PanelId::Pane2)),
            ],
        );
        set_workspace_tree(&mut app, a, tree);
        let pane2 = app.world_mut().spawn_empty().id();
        app.world_mut()
            .get_mut::<WorkspacePanels>(a)
            .expect("WorkspacePanels")
            .roots
            .0
            .insert(PanelId::Pane2, pane2);
        app.insert_resource(ActiveTranscript(Some(pane2)));
        app.update();

        assert!(
            (scroll_of(&app, a) - viewport * 0.5).abs() < 1e-3,
            "Pane2 列在 x = 视口宽处，要滚到能完整看见它"
        );

        app.insert_resource(ActiveTranscript(None));
        app.update();
        assert_eq!(scroll_of(&app, a), 0.0, "切回主转写：它已可见，滚回 0");
    }

    // ---- L4：列宽预设 + 移动列 ----

    /// 条带在 **UI 像素**下的叶子矩形（x/宽直接是像素，便于和
    /// `fraction × viewport` 比较）。
    fn strip_rects_px(app: &App, workspace: Entity) -> Vec<(PanelId, TileRect)> {
        let viewport = viewport_px(app);
        let area = TileRect {
            center: Vec2::ZERO,
            size: Vec2::new(viewport, 1.0),
        };
        workspace_tree(app, workspace).rects(area)
    }

    fn strip_rect_px(app: &App, workspace: Entity, panel: PanelId) -> TileRect {
        strip_rects_px(app, workspace)
            .into_iter()
            .find(|(id, _)| *id == panel)
            .map(|(_, rect)| rect)
            .unwrap_or_else(|| panic!("{panel:?} 在条带上"))
    }

    fn tile_left(rect: &TileRect) -> f32 {
        rect.center.x - rect.size.x * 0.5
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// 活动列完整落在视口 `[0, viewport]` 里（L4 改宽/移动后必须仍可见）。
    fn assert_column_visible(app: &App, workspace: Entity, panel: PanelId) {
        let viewport = viewport_px(app);
        let bounds = column_bounds(&workspace_tree(app, workspace), panel, viewport)
            .unwrap_or_else(|| panic!("{panel:?} 要有列"));
        let scroll = scroll_of(app, workspace);
        let left = bounds.x - scroll;
        let right = left + bounds.width;
        assert!(
            left >= -1e-3 && right <= viewport + 1e-3,
            "{panel:?} 必须完整可见：显示区间 [{left}, {right}] 不在 [0, {viewport}] 里（scroll={scroll}）"
        );
    }

    fn hold_keys(app: &mut App, keys: &[KeyCode]) {
        app.init_resource::<ButtonInput<KeyCode>>();
        for key in keys {
            set_key(app, *key, true);
        }
    }

    // 42. L4 列宽：`Alt+Shift+L/H` 在三档预设宽度里循环。每次只有活动列
    //     （最右边的 picker 列）的宽度变，其它叶子的 (x, width) 逐字节不变；
    //     宽度 = fraction × 视口，活动列始终完整可见。
    #[test]
    fn keyboard_width_presets_cycle_on_the_active_column_only() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let viewport = viewport_px(&app);
        append_picker_column(&mut app, a);
        set_picker_open(&mut app, true);
        app.update();
        let tree_b = workspace_tree(&app, b);
        let baseline = strip_rects_px(&app, a);
        let picker_before = strip_rect_px(&app, a, PanelId::List);
        assert!(close(picker_before.size.x, viewport * 0.5), "picker 列初始半屏宽");
        assert_column_visible(&app, a, PanelId::List);

        hold_keys(&mut app, &[KeyCode::AltLeft, KeyCode::ShiftLeft]);
        // 0.5 往上取 2/3；到头回绕 1/3；再 1/2、2/3。
        for expected in [
            layout::PRESET_COLUMN_FRACTIONS[2],
            layout::PRESET_COLUMN_FRACTIONS[0],
            layout::PRESET_COLUMN_FRACTIONS[1],
            layout::PRESET_COLUMN_FRACTIONS[2],
        ] {
            press_code(&mut app, KeyCode::KeyL);
            let tree = workspace_tree(&app, a);
            assert_eq!(tree.column_fraction(PanelId::List), Some(expected));
            assert_eq!(
                app.world().resource::<FrameTree>().0,
                tree,
                "同一帧同步全局树：lay_out_frame 读它"
            );
            let after = strip_rects_px(&app, a);
            for (panel, old) in &baseline {
                if *panel == PanelId::List {
                    continue;
                }
                let fresh = after
                    .iter()
                    .find(|(id, _)| id == panel)
                    .map(|(_, rect)| *rect)
                    .unwrap_or_else(|| panic!("{panel:?} 还在条带上"));
                assert_eq!(fresh, *old, "{panel:?} 的 (x, width) 必须逐字节不变");
            }
            let picker = strip_rect_px(&app, a, PanelId::List);
            assert_eq!(picker.size.x, viewport * expected, "宽度 = fraction × 视口");
            assert_eq!(
                tile_left(&picker),
                tile_left(&picker_before),
                "最右列的左边不动，只有宽度变"
            );
            assert_column_visible(&app, a, PanelId::List);
        }

        // 反向：2/3 → 1/2。
        press_code(&mut app, KeyCode::KeyH);
        assert_eq!(
            workspace_tree(&app, a).column_fraction(PanelId::List),
            Some(layout::PRESET_COLUMN_FRACTIONS[1])
        );
        assert_column_visible(&app, a, PanelId::List);
        assert_eq!(workspace_tree(&app, b), tree_b, "非聚焦工作区一个字节不动");
    }

    // 43. L4 列宽当帧生效：改聚焦工作区的转写列后，同一帧 `lay_out_frame`
    //     写出的 Node 宽度就是 2/3 视口；右列宽度不变、只是整体左移。
    #[test]
    fn keyboard_width_keys_resize_the_transcript_column_in_the_same_frame() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let viewport = viewport_px(&app);
        let tree_b = workspace_tree(&app, b);
        let sizes_before = panel_sizes(&app, a);
        let width_of = |sizes: &[(PanelId, Vec2)], panel: PanelId| {
            sizes
                .iter()
                .find(|(id, _)| *id == panel)
                .map(|(_, size)| size.x)
                .unwrap_or_else(|| panic!("{panel:?} 有 Node 尺寸"))
        };
        let input_before = width_of(&sizes_before, PanelId::Input);

        hold_keys(&mut app, &[KeyCode::AltLeft, KeyCode::ShiftLeft]);
        press_code(&mut app, KeyCode::KeyL);

        let tree = workspace_tree(&app, a);
        assert_eq!(
            tree.column_fraction(PanelId::Transcript),
            Some(layout::PRESET_COLUMN_FRACTIONS[2])
        );
        assert_eq!(app.world().resource::<FrameTree>().0, tree);
        // 同一帧的放置：转写 Node 宽 = 2/3 视口，右列（输入）宽度不变。
        let sizes = panel_sizes(&app, a);
        assert!(
            (width_of(&sizes, PanelId::Transcript) - viewport * layout::PRESET_COLUMN_FRACTIONS[2]).abs()
                < 1e-3,
            "转写 Node 宽应当已是 2/3 视口"
        );
        assert!((width_of(&sizes, PanelId::Input) - input_before).abs() < 1e-3, "右列宽度不变");
        // 几何：右列紧贴转写右边、宽度不变（只平移）。
        let transcript = strip_rect_px(&app, a, PanelId::Transcript);
        let input = strip_rect_px(&app, a, PanelId::Input);
        assert!(close(tile_left(&input), tile_left(&transcript) + transcript.size.x));
        assert!(close(input.size.x, viewport * layout::RIGHT_COLUMN_FRACTION));
        assert_eq!(scroll_of(&app, a), 0.0, "转写列在视口左端，已经可见");
        assert_column_visible(&app, a, PanelId::Transcript);
        assert_eq!(workspace_tree(&app, b), tree_b);
    }

    // 44. L4 移动列：`Ctrl+Alt+Shift+L/H` 与邻列交换。两列的宽度跟着列走、
    //     x 重排；其它列不动；到左边界不越界、不 panic；来回移动逐字节复原。
    #[test]
    fn keyboard_move_swaps_the_active_column_with_its_neighbour() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let viewport = viewport_px(&app);
        let before_tree = workspace_tree(&app, a);
        let before = strip_rects_px(&app, a);
        let tree_b = workspace_tree(&app, b);
        // `strip_rects_px` 的坐标以帧中心为原点：条带最左边是 `-viewport/2`。
        let left_edge = -viewport * 0.5;

        hold_keys(
            &mut app,
            &[KeyCode::ControlLeft, KeyCode::AltLeft, KeyCode::ShiftLeft],
        );
        // 转写（0.62）与右列（0.38）交换：右列到最左，转写接在它后面。
        press_code(&mut app, KeyCode::KeyL);
        let tree = workspace_tree(&app, a);
        assert_eq!(
            tree.column_fraction(PanelId::Input),
            Some(layout::RIGHT_COLUMN_FRACTION),
            "右列的尺寸约束跟着列走"
        );
        assert_eq!(
            tree.column_fraction(PanelId::Transcript),
            Some(layout::TRANSCRIPT_COLUMN_FRACTION)
        );
        assert_eq!(app.world().resource::<FrameTree>().0, tree);
        let transcript = strip_rect_px(&app, a, PanelId::Transcript);
        let input = strip_rect_px(&app, a, PanelId::Input);
        assert!(close(input.size.x, viewport * layout::RIGHT_COLUMN_FRACTION), "右列宽度不变");
        assert!(close(transcript.size.x, viewport * layout::TRANSCRIPT_COLUMN_FRACTION), "转写宽度不变");
        assert!(close(tile_left(&input), left_edge), "右列换到最左");
        assert!(
            close(tile_left(&transcript), left_edge + input.size.x),
            "转写接在右列后面"
        );
        // 右列内部的补全/输入宽度不变，只随整列平移。
        let complete_before = before
            .iter()
            .find(|(id, _)| *id == PanelId::Complete)
            .map(|(_, rect)| *rect)
            .unwrap();
        assert_eq!(
            strip_rect_px(&app, a, PanelId::Complete).size.x,
            complete_before.size.x,
            "补全 tile 在右列内部，宽度不受移动影响"
        );
        // 同帧放置：Node 宽度已按新顺序写。
        let sizes = panel_sizes(&app, a);
        let width_of = |panel: PanelId| {
            sizes
                .iter()
                .find(|(id, _)| *id == panel)
                .map(|(_, size)| size.x)
                .unwrap()
        };
        assert!((width_of(PanelId::Input) - viewport * layout::RIGHT_COLUMN_FRACTION).abs() < 1e-3);
        assert!((width_of(PanelId::Transcript) - viewport * layout::TRANSCRIPT_COLUMN_FRACTION).abs() < 1e-3);
        assert_column_visible(&app, a, PanelId::Transcript);
        assert_eq!(scroll_of(&app, a), 0.0, "活动列换位后正好可见：不动 scroll");

        // 换回来：树逐字节复原；再往左就是边界，不动、不 panic。
        press_code(&mut app, KeyCode::KeyH);
        assert_eq!(workspace_tree(&app, a), before_tree, "来回移动后树逐字节复原");
        press_code(&mut app, KeyCode::KeyH);
        assert_eq!(workspace_tree(&app, a), before_tree, "最左列再往左：不动");
        assert_eq!(scroll_of(&app, a), 0.0);
        assert_eq!(workspace_tree(&app, b), tree_b);
    }

    // 45. L4 与 picker 共存：picker 列也在条带里，可以左移穿过右列/转写列，
    //     宽度不变；到最左再按不动；右移两次逐字节复原，始终完整可见。
    #[test]
    fn the_picker_column_can_be_moved_across_the_strip_and_stops_at_the_edge() {
        let mut app = app_with_two_workspaces();
        let (a, b) = workspaces(&app);
        let viewport = viewport_px(&app);
        append_picker_column(&mut app, a);
        set_picker_open(&mut app, true);
        app.update();
        let before_tree = workspace_tree(&app, a);
        let tree_b = workspace_tree(&app, b);
        let right_width = strip_rect_px(&app, a, PanelId::Input).size.x;
        let complete_width = strip_rect_px(&app, a, PanelId::Complete).size.x;
        let transcript_before = strip_rect_px(&app, a, PanelId::Transcript);
        assert!(close(
            strip_rect_px(&app, a, PanelId::List).size.x,
            viewport * layout::DEFAULT_COLUMN_FRACTION
        ));
        let left_edge = -viewport * 0.5;

        hold_keys(
            &mut app,
            &[KeyCode::ControlLeft, KeyCode::AltLeft, KeyCode::ShiftLeft],
        );
        // 左移一格：与 0.38 的右列交换。
        press_code(&mut app, KeyCode::KeyH);
        let picker = strip_rect_px(&app, a, PanelId::List);
        assert!(close(
            tile_left(&picker),
            left_edge + viewport * layout::TRANSCRIPT_COLUMN_FRACTION
        ));
        assert!(close(picker.size.x, viewport * layout::DEFAULT_COLUMN_FRACTION), "picker 宽度跟列走");
        assert!(close(strip_rect_px(&app, a, PanelId::Input).size.x, right_width));
        assert!(close(strip_rect_px(&app, a, PanelId::Complete).size.x, complete_width));
        assert_eq!(
            strip_rect_px(&app, a, PanelId::Transcript),
            transcript_before,
            "交换的只是 picker 与右列，转写列一个字节不动"
        );
        assert_column_visible(&app, a, PanelId::List);

        // 再左移一格：与转写交换，picker 到最左；再左就是边界。
        press_code(&mut app, KeyCode::KeyH);
        assert!(close(
            tile_left(&strip_rect_px(&app, a, PanelId::List)),
            left_edge
        ));
        assert_column_visible(&app, a, PanelId::List);
        let at_left = workspace_tree(&app, a);
        press_code(&mut app, KeyCode::KeyH);
        assert_eq!(workspace_tree(&app, a), at_left, "最左列再往左：不动、不 panic");

        // 右移两次回到原序：树逐字节复原。
        press_code(&mut app, KeyCode::KeyL);
        press_code(&mut app, KeyCode::KeyL);
        assert_eq!(workspace_tree(&app, a), before_tree, "移动到原序后树逐字节复原");
        assert_column_visible(&app, a, PanelId::List);
        // 最右列再往右：不动、不越界。
        press_code(&mut app, KeyCode::KeyL);
        assert_eq!(workspace_tree(&app, a), before_tree, "最右列再往右：不动");
        assert_eq!(workspace_tree(&app, b), tree_b);
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
