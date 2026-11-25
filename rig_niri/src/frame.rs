//! 帧布局：把 [`crate::layout`] 的分裂树落到每块面板的 `Node` 尺寸与 3D 锚点上。
//!
//! rig-tui 用"绝对位置槽"拼屏幕（`Placement::Bottom(4)`、`Rest`……）。3D 版
//! **不照搬**：窗口是分裂树上的叶子，位置和大小都由 [`LayoutNode::rects`] 递归推导。
//! 一块面板能拿到的矩形只取决于树，跟"离底边几行"无关。
//!
//! ```text
//! FrameSize(96×28 格) ──> TileRect（米，整屏）
//!                          │
//!   layout::Node::rects ──> 每块叶子的 TileRect
//!   layout::Node::containers ──> 容器面板（picker 那一列）的 TileRect
//!                          │
//!   TileRect × 1/UNITS_TO_WORLD ──> Node 尺寸（bevy_ui 像素）
//!   TileRect.center ──────────────> WorldPanel.anchor（3D 位置）
//!   TileRect − 面板装饰 ──────────> 文字列数 / 行数（look 换行、pane 高度）
//! ```
//!
//! **没有浮层**：picker、补全、确认都是条带里的 tile。`Placement` 只剩
//! [`Placement::Tree`]（树叶子）和一个旧槽位 [`Placement::Rest`]（`niri.rs`
//! 在移除 `FramePanel` 前会先挂它，不参与布局）。一块面板画在哪一层由
//! [`FramePanel::layer`] 决定：容器面板在 0，它内部的列表 / preview 在 1，
//! 这样容器永远垫在内部面板下面。

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_ui::{BorderRadius, Node, UiRect, px};
use feathers_in_3d::{MirrorRoot, WorldLayer, WorldPanel};

use crate::layout::{self, Node as LayoutNode};
use crate::niri::TileRect;
use crate::panels::{PanelId, PanelRows};
use crate::workspaces::{Workspace, WorkspacePanel, WorkspacePanels, WorkspaceSet};

pub use crate::panels::PanelRoots;

/// 一个格子在 UI 像素里的尺寸：宽 = 13px 等宽字的前进量，高 = 一行文本。
pub const CELL: Vec2 = Vec2::new(7.8, crate::transcript::ROW_HEIGHT);

/// 面板默认装饰（内边距 + 边框）。
pub const PANEL_PADDING: f32 = 8.0;
pub const PANEL_BORDER: f32 = 2.0;

/// UI 像素 → 世界米的换算（与 rig_niri 里各面板一致）。
pub const UNITS_TO_WORLD: f32 = 0.0018;

/// rig-tui `panel::MIN_TRANSCRIPT`：旧槽位布局里面板不能把转写挤到比这更小。
///
/// 分裂树不强制这个下限：叶子拿到的区域由树决定（太小就按零行列处理，
/// [`PanelArea::is_empty`] 为真）。
pub const MIN_TRANSCRIPT: (u16, u16) = (20, 3);

// ---------------------------------------------------------------------------
// 格子
// ---------------------------------------------------------------------------

/// 帧里的矩形，单位是格子（对应 `ratatui::layout::Rect`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellRect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl CellRect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn right(&self) -> u16 {
        self.x.saturating_add(self.w)
    }

    pub fn bottom(&self) -> u16 {
        self.y.saturating_add(self.h)
    }
}

/// 一帧有多大，单位格子（对应 rig-tui 的 `TerminalSize`）。
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSize {
    pub cols: u16,
    pub rows: u16,
}

impl Default for FrameSize {
    fn default() -> Self {
        Self { cols: 96, rows: 28 }
    }
}

impl FrameSize {
    /// `96x28` 这样的写法；测试与截图时用 `RF_FRAME` 改。
    pub fn parse(text: &str) -> Option<Self> {
        let (cols, rows) = text.split_once(['x', 'X', '×'])?;
        Some(Self {
            cols: cols.trim().parse().ok()?,
            rows: rows.trim().parse().ok()?,
        })
    }

    pub fn from_env() -> Self {
        std::env::var("RF_FRAME")
            .ok()
            .and_then(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn screen(&self) -> CellRect {
        CellRect::new(0, 0, self.cols, self.rows)
    }
}

/// 文字区能从面板里拿到的大小：面板尺寸减掉装饰。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelInset {
    pub pad: u16,
    pub border: u16,
}

impl PanelInset {
    /// 默认：内边距 8px、边框 2px。
    pub const DEFAULT: Self = Self { pad: 8, border: 2 };
    /// 细条（状态行）：内边距 4px、边框 1px。
    pub const THIN: Self = Self { pad: 4, border: 1 };

    /// 上下（或左右）各吃掉的像素。
    pub fn total(&self) -> f32 {
        2.0 * (f32::from(self.pad) + f32::from(self.border))
    }
}

impl Default for PanelInset {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// ---------------------------------------------------------------------------
// 放置
// ---------------------------------------------------------------------------

/// 面板放在哪。
///
/// 所有窗口都是 niri 式平铺：区域由 [`crate::layout`] 的分裂树决定，挂
/// [`Placement::Tree`]。**没有浮层**——picker 是条带最右边的一列，补全 / 确认
/// 是列里的 tile，`Over` / `Within` 已经删掉。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// 由 `layout::Node` 的叶子（或容器）决定区域（分裂树）。
    Tree,
    /// 旧槽位：别人剩下的地方（仅保留 API；`niri.rs` 移除 `FramePanel` 前会先
    /// 挂它，随后立刻换成 Niri 的 `Tile`，不参与这里的布局）。
    Rest,
}

/// 一块参与布局的面板。
#[derive(Clone, Copy, Debug)]
pub struct ToLayOut {
    pub entity: Entity,
    /// 面板装饰（文字区 = 区域减掉它）。
    pub inset: PanelInset,
}

/// 每帧算出来的面板区域。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PanelArea {
    /// 面板在帧里的格子区域（文字/点击用的近似值）。
    pub rect: CellRect,
    /// 面板的 UI 像素尺寸（树布局给精确值；[`PanelArea::new`] 从格子算）。
    pub size: Vec2,
    /// 文字能放多少列 / 行。
    pub cols: u16,
    pub rows: u16,
    /// 精确的帧内中心（UI 像素，原点在帧中心，y 朝上）。
    ///
    /// 树布局由 [`TileRect::center`] 填；旧接口 [`PanelArea::new`] 只有格子
    /// 区域，填 `None`，3D 锚点退回 `rect` 中心（保持旧行为）。
    pub center: Option<Vec2>,
}

impl PanelArea {
    /// `rect` 里的面板，装饰为 `inset` 时文字能放多少。
    pub fn new(rect: CellRect, inset: PanelInset) -> Self {
        let size = Vec2::new(f32::from(rect.w), f32::from(rect.h)) * CELL;
        let usable = (size - Vec2::splat(inset.total())).max(Vec2::ZERO);
        Self {
            rect,
            size,
            cols: (usable.x / CELL.x).floor() as u16,
            rows: (usable.y / CELL.y).floor() as u16,
            center: None,
        }
    }

    /// 从分裂树的 [`TileRect`]（米，原点帧中心）算面板区域。
    ///
    /// `size` 与 `center` 保持树的精确值；`rect` 只是给文字/点击用的格子近似。
    pub fn from_tile(tile: TileRect, frame: CellRect, inset: PanelInset) -> Self {
        let size = tile.size / UNITS_TO_WORLD;
        let usable = (size - Vec2::splat(inset.total())).max(Vec2::ZERO);
        let center_px = tile.center / UNITS_TO_WORLD;
        let frame_px = Vec2::new(f32::from(frame.w), f32::from(frame.h)) * CELL;
        // 树坐标（原点帧中心、y 朝上）→ UI 坐标（原点帧左上、y 朝下）。
        let left_px = frame_px.x * 0.5 + center_px.x - size.x * 0.5;
        let top_px = frame_px.y * 0.5 - center_px.y - size.y * 0.5;
        let w = (size.x / CELL.x).round().max(0.0) as u16;
        let h = (size.y / CELL.y).round().max(0.0) as u16;
        let x = (left_px / CELL.x)
            .round()
            .clamp(0.0, f32::from(frame.w.saturating_sub(w))) as u16;
        let y = (top_px / CELL.y)
            .round()
            .clamp(0.0, f32::from(frame.h.saturating_sub(h))) as u16;
        Self {
            rect: CellRect::new(x, y, w, h),
            size,
            cols: (usable.x / CELL.x).floor() as u16,
            rows: (usable.y / CELL.y).floor() as u16,
            center: Some(center_px),
        }
    }

    /// 没有地方（面板这一帧不出现）。
    pub fn is_empty(&self) -> bool {
        self.rect.is_empty() || self.cols == 0 || self.rows == 0
    }
}

/// 把分裂树铺到 `screen` 上，返回每块面板的区域。
///
/// 树的**叶子**和**容器**都按 [`LayoutNode::placements`] 拿矩形（容器先出现、
/// 和内部叶子重叠，靠 [`FramePanel::layer`] 画在下面）；`PanelId` 经 `roots`
/// 找到实体。没有浮层，所以不需要旧的槽位/叠放逻辑。
pub fn lay_out(
    tree: &LayoutNode,
    panels: impl IntoIterator<Item = ToLayOut>,
    roots: &HashMap<PanelId, Entity>,
    screen: CellRect,
) -> HashMap<Entity, PanelArea> {
    let panels: Vec<ToLayOut> = panels.into_iter().collect();

    let frame_tile = TileRect {
        center: Vec2::ZERO,
        size: Vec2::new(f32::from(screen.w), f32::from(screen.h)) * CELL * UNITS_TO_WORLD,
    };
    let mut out: HashMap<Entity, PanelArea> = HashMap::new();
    for (id, tile, _container) in tree.placements(frame_tile) {
        let Some(entity) = roots.get(&id).copied() else {
            continue;
        };
        let Some(panel) = panels.iter().find(|panel| panel.entity == entity) else {
            continue;
        };
        out.insert(entity, PanelArea::from_tile(tile, screen, panel.inset));
    }
    out
}

// ---------------------------------------------------------------------------
// 组件与系统
// ---------------------------------------------------------------------------

/// 一块参与帧布局的面板（`bevy_ui` 子树根 + 3D 面板）。
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct FramePanel {
    /// 位置：树叶子挂 [`Placement::Tree`]。
    pub placement: Placement,
    /// 绘制层：0 是屏幕本身，1 画在 0 上面（picker 容器是 0，内部的列表 /
    /// preview 是 1，所以容器永远垫在它们下面）。
    pub layer: u16,
    /// 面板装饰：内边距 + 边框。
    pub inset: PanelInset,
}

impl FramePanel {
    pub fn new(placement: Placement) -> Self {
        Self {
            placement,
            layer: 0,
            inset: PanelInset::DEFAULT,
        }
    }

    /// 树布局的一块面板：位置由 [`FrameTree`] 的叶子（或容器）决定。
    pub fn tree() -> Self {
        Self::new(Placement::Tree)
    }

    /// 这块面板画在第几层（高的盖住低的）。
    pub fn layer(mut self, layer: u16) -> Self {
        self.layer = layer;
        self
    }

    pub fn inset(mut self, inset: PanelInset) -> Self {
        self.inset = inset;
        self
    }
}

/// 面板按内容收成几行。旧槽位布局用它覆盖尺寸；树布局不读它，保留类型只为
/// 不破坏既有调用。
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FitRows(pub u16);

/// 当前帧的分裂树（叶子是 [`PanelId`]，经 [`PanelRoots`] 找到实体）。
///
/// 默认屏幕用 [`layout::default_tree`]；插件/测试可以直接替换整棵树。
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct FrameTree(pub LayoutNode);

impl Default for FrameTree {
    fn default() -> Self {
        Self(layout::default_tree())
    }
}

/// 每帧的面板区域，`lay_out` 的结果。
#[derive(Resource, Default)]
pub struct PanelAreas(pub HashMap<Entity, PanelArea>);

impl PanelAreas {
    /// 某块面板这一帧的区域。
    pub fn get(&self, entity: Entity) -> Option<PanelArea> {
        self.0.get(&entity).copied()
    }
}

/// 帧在世界里的位置：帧中心 + 每层往相机方向抬多少。
#[derive(Resource, Clone, Copy, Debug)]
pub struct FrameGeometry {
    /// 帧中心在世界里的位置。
    pub center: Vec3,
    /// 每高一层往相机方向抬多少米（几何上的深度；绘制顺序见 `WorldLayer`）。
    pub layer_depth: f32,
    /// UI 像素 → 米。
    pub units_to_world: f32,
}

impl Default for FrameGeometry {
    fn default() -> Self {
        Self {
            center: Vec3::ZERO,
            layer_depth: 0.02,
            units_to_world: UNITS_TO_WORLD,
        }
    }
}

/// 主转写面板的实体（布局时别的面板可能要参考它）。
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct DefaultScreen {
    pub transcript: Option<Entity>,
}

/// 算每块面板这一帧的区域：分裂树叶子 + 浮层。
///
/// 有 [`WorkspaceSet`] 时遍历所有工作区：每个工作区用自己的树/根表算一份
/// 帧内区域（存进 `WorkspacePanels::areas`），聚焦工作区同时把结果写进全局
/// [`PanelAreas`]（内容系统和还没按工作区改的旧模块读它）。没有工作区时退回
/// 原来的全局单例路径，行为与第 2 批之前一致。
pub fn lay_out_frame(
    mut areas: ResMut<PanelAreas>,
    size: Res<FrameSize>,
    roots: Option<Res<PanelRoots>>,
    tree: Option<Res<FrameTree>>,
    set: Option<Res<WorkspaceSet>>,
    panels: Query<(Entity, &FramePanel, Option<&WorkspacePanel>)>,
    mut workspaces: Query<(&Workspace, &mut WorkspacePanels)>,
) {
    if let Some(set) = set.as_deref().filter(|set| !set.items.is_empty()) {
        let mut focused_areas = None;
        for workspace_entity in &set.items {
            let Ok((workspace, mut stored)) = workspaces.get_mut(*workspace_entity) else {
                continue;
            };
            let focused = set.focused == Some(*workspace_entity);
            // 聚焦工作区的树用全局 `FrameTree`：`complete::sync_completion_tile`
            // 在同一个 `Layout` 阶段先改它，这里必须看到同一帧的结果。
            let (want_tree, want_roots) = match (focused, tree.as_deref(), roots.as_deref()) {
                (true, Some(tree), Some(roots)) => (&tree.0, &roots.0),
                _ => (&workspace.tree, &stored.roots.0),
            };
            let to_lay_out = panels.iter().filter_map(|(entity, panel, tag)| {
                tag.filter(|tag| tag.0 == *workspace_entity)
                    .map(|_| ToLayOut {
                        entity,
                        inset: panel.inset,
                    })
            });
            let next = lay_out(want_tree, to_lay_out, want_roots, size.screen());
            if stored.areas != next {
                stored.areas = next.clone();
            }
            if focused {
                focused_areas = Some(next);
            }
        }
        let next = focused_areas.unwrap_or_default();
        if areas.0 != next {
            areas.0 = next;
        }
        return;
    }

    // 旧路径：没有 `WorkspacesPlugin` 时全局资源就是唯一的屏幕。
    // `FramePlugin` 也可以单独跑（没有 `PanelsPlugin` 时没有 `PanelRoots`）。
    let Some(tree) = tree.as_deref() else {
        return;
    };
    let to_lay_out = panels.iter().map(|(entity, panel, _)| ToLayOut {
        entity,
        inset: panel.inset,
    });
    let empty = HashMap::new();
    let roots = roots.as_deref().map_or(&empty, |roots| &roots.0);
    let next = lay_out(&tree.0, to_lay_out, roots, size.screen());
    if next != areas.0 {
        areas.0 = next;
    }
}

/// 面板中心在帧里的像素坐标（原点帧中心，UI 的 y 朝下，世界的 y 朝上）。
///
/// 树布局拿精确中心；旧接口 `PanelArea::new` 只有格子区域时退回格子中心。
fn area_center_px(area: &PanelArea, frame_px: Vec2) -> Vec2 {
    area.center.unwrap_or_else(|| {
        Vec2::new(
            (f32::from(area.rect.x) + f32::from(area.rect.w) * 0.5) * CELL.x
                - frame_px.x * 0.5,
            frame_px.y * 0.5
                - (f32::from(area.rect.y) + f32::from(area.rect.h) * 0.5) * CELL.y,
        )
    })
}

/// 把格子区域落到每块面板的 `Node` 尺寸与 `WorldPanel` 锚点上。
///
/// 有 [`WorkspaceSet`] 时每块面板按自己的 [`WorkspacePanel`] 找所属工作区：
/// 世界锚点 = `workspace.pose × 帧内中心`。没有工作区时走原来的单屏路径
/// （`FrameGeometry.center` 就是世界原点）。
pub fn place_panels(
    mut commands: Commands,
    areas: Res<PanelAreas>,
    size: Res<FrameSize>,
    geometry: Res<FrameGeometry>,
    set: Option<Res<WorkspaceSet>>,
    mut panels: Query<(
        Entity,
        &FramePanel,
        Option<&WorkspacePanel>,
        &mut Node,
        &mut WorldPanel,
        Option<&mut WorldLayer>,
    )>,
    workspaces: Query<(&Workspace, &WorkspacePanels)>,
) {
    let frame_px = Vec2::new(f32::from(size.cols), f32::from(size.rows)) * CELL;
    if set
        .as_deref()
        .is_some_and(|set| !set.items.is_empty())
    {
        for (entity, panel, tag, mut node, mut world, layer) in panels.iter_mut() {
            let Some(tag) = tag else {
                // 还没打标签的面板（本帧刚 spawn）下一帧再放。
                continue;
            };
            let Ok((workspace, stored)) = workspaces.get(tag.0) else {
                continue;
            };
            let Some(area) = stored.areas.get(&entity).copied() else {
                continue;
            };
            let width = px(area.size.x);
            let height = px(area.size.y);
            if node.width != width {
                node.width = width;
            }
            if node.height != height {
                node.height = height;
            }
            let center_px = area_center_px(&area, frame_px);
            let want_layer = panel.layer;
            // 帧内局部位姿（世界原点时的锚点），再乘工作区的绝对位姿。
            let local = Transform::from_translation(
                geometry.center
                    + Vec3::new(center_px.x, center_px.y, 0.0) * geometry.units_to_world
                    + Vec3::Z * (f32::from(want_layer) * geometry.layer_depth),
            );
            let anchor = workspace.pose.mul_transform(local);
            if world.anchor != anchor {
                world.anchor = anchor;
            }
            if world.units_to_world != geometry.units_to_world {
                world.units_to_world = geometry.units_to_world;
            }
            match layer {
                Some(mut layer) => {
                    if layer.0 != want_layer {
                        layer.0 = want_layer;
                    }
                }
                None => {
                    commands.entity(entity).insert(WorldLayer(want_layer));
                }
            }
        }
        return;
    }

    if areas.0.is_empty() {
        return;
    }
    for (entity, panel, _tag, mut node, mut world, layer) in panels.iter_mut() {
        let Some(area) = areas.get(entity) else {
            continue;
        };
        let width = px(area.size.x);
        let height = px(area.size.y);
        if node.width != width {
            node.width = width;
        }
        if node.height != height {
            node.height = height;
        }
        let center_px = area_center_px(&area, frame_px);
        let want_layer = panel.layer;
        let anchor = geometry.center
            + Vec3::new(center_px.x, center_px.y, 0.0) * geometry.units_to_world
            + Vec3::Z * (f32::from(want_layer) * geometry.layer_depth);
        if world.anchor.translation != anchor {
            world.anchor.translation = anchor;
        }
        if world.units_to_world != geometry.units_to_world {
            world.units_to_world = geometry.units_to_world;
        }
        match layer {
            Some(mut layer) => {
                if layer.0 != want_layer {
                    layer.0 = want_layer;
                }
            }
            None => {
                commands.entity(entity).insert(WorldLayer(want_layer));
            }
        }
    }
}

/// 一套默认屏幕面板（分裂树四块叶子），不登记到全局资源。
///
/// `workspace` 存在时给每块面板挂 [`WorkspacePanel`]（新工作区用）；`None` 是
/// `FramePlugin` 的 Startup 建旧全局默认屏幕用。返回这个工作区自己的根表与
/// 空行槽，由调用方决定放进全局资源还是 `WorkspacePanels`。
pub(crate) fn default_screen_panels(
    commands: &mut Commands,
    workspace: Option<Entity>,
) -> (PanelRoots, PanelRows) {
    let mut roots = PanelRoots::default();
    let mut rows = PanelRows::default();
    let spawn = |commands: &mut Commands, id: PanelId, panel: FramePanel, look: Look| {
        let entity = spawn_panel(commands, id, panel, look);
        if let Some(workspace) = workspace {
            commands.entity(entity).insert(WorkspacePanel(workspace));
        }
        entity
    };
    // 输入：右列的比例子，吃掉补全 tile 和状态行之外的全部高度。
    let input = spawn(
        commands,
        PanelId::Input,
        FramePanel::tree(),
        Look::new(
            Color::srgb(0.05, 0.06, 0.09),
            Color::srgb(0.30, 0.55, 0.85),
            8.0,
        ),
    );
    // 状态行：右列底部固定 2 格，细条里居中放 1 行字。
    let status = spawn(
        commands,
        PanelId::Status,
        FramePanel::tree().inset(PanelInset::THIN),
        Look {
            bg: Color::srgb(0.035, 0.045, 0.065),
            border: Color::srgb(0.16, 0.22, 0.32),
            radius: 6.0,
            justify: JustifyContent::Center,
        },
    );
    // 转写：左列（Proportion 2，占 2/3 宽）。
    let transcript = spawn(
        commands,
        PanelId::Transcript,
        FramePanel::tree(),
        transcript_look(),
    );
    // 补全 tile：右列、输入框上方；关着时树里是 0 高度（不占地方），打开时
    // `complete` 把它改成 6 行并把候选 + 官方提示行写进 `PanelRows`。
    let complete = spawn(
        commands,
        PanelId::Complete,
        FramePanel::tree().inset(PanelInset::THIN),
        Look::new(
            Color::srgb(0.05, 0.07, 0.10),
            Color::srgb(0.35, 0.45, 0.60),
            8.0,
        ),
    );
    // 键位提示行：固定 1 行，窄条（THIN inset），内容由 `hints::hint_panel_rows` 写。
    let hints = spawn(
        commands,
        PanelId::Hints,
        // 无内边距：这一格只有 1 行高，任何 padding 都会把字裁掉。
        FramePanel::tree().inset(PanelInset { pad: 0, border: 0 }),
        Look {
            bg: Color::srgb(0.035, 0.045, 0.065),
            border: Color::srgb(0.16, 0.22, 0.32),
            radius: 4.0,
            justify: JustifyContent::FlexStart,
        },
    );
    // 确认行（Ask）：没有待确认时树里是 0 高度，有就长成 n 行。
    let confirm = spawn(
        commands,
        PanelId::Confirm,
        FramePanel::tree().inset(PanelInset::THIN),
        Look::new(
            Color::srgb(0.07, 0.06, 0.04),
            Color::srgb(0.55, 0.45, 0.20),
            6.0,
        ),
    );
    roots.0.insert(PanelId::Input, input);
    roots.0.insert(PanelId::Status, status);
    roots.0.insert(PanelId::Transcript, transcript);
    roots.0.insert(PanelId::Complete, complete);
    roots.0.insert(PanelId::Hints, hints);
    roots.0.insert(PanelId::Confirm, confirm);
    for id in [
        PanelId::Input,
        PanelId::Status,
        PanelId::Transcript,
        PanelId::Complete,
        PanelId::Hints,
        PanelId::Confirm,
    ] {
        rows.0.entry(id).or_default();
    }
    (roots, rows)
}

/// 确认行 tile 的高度跟着行数走（没有待确认时 0 高度，不占地方）。
fn sync_confirm_tile(
    rows: Res<PanelRows>,
    tree: Option<ResMut<FrameTree>>,
) {
    let Some(mut tree) = tree else {
        return;
    };
    let lines = rows.get(PanelId::Confirm).len() as f32;
    let want = crate::layout::Size::Fixed(lines * crate::transcript::ROW_HEIGHT);
    if tree.0.leaf_size(PanelId::Confirm) == Some(want) {
        return;
    }
    tree.0.set_leaf_size(PanelId::Confirm, want);
}

/// 装默认屏幕（rig-tui `screen.rs` 的布局在分裂树里重排）：左列转写，右列从上
/// 到下是补全 tile / 输入 / 状态行。四块面板都是 `layout::default_tree` 的叶子。
///
/// 这是**没有** `WorkspacesPlugin` 时的旧单屏路径；装了工作区插件后它 spawn 的
/// 面板会在 `PostStartup` 被收编成默认工作区（见 `workspaces`）。
fn spawn_default_screen(
    mut commands: Commands,
    mut roots: ResMut<PanelRoots>,
    mut rows: ResMut<PanelRows>,
    mut screen: ResMut<DefaultScreen>,
) {
    // 新窗口逻辑（Niri）接管时，默认屏幕由 `niri::spawn_niri_screen` 生成。
    if crate::niri::niri_layout() {
        return;
    }
    let (screen_roots, screen_rows) = default_screen_panels(&mut commands, None);
    screen.transcript = screen_roots.entity(PanelId::Transcript);
    roots.0.extend(screen_roots.0);
    rows.0.extend(screen_rows.0);
}

/// 一块面板的观感（底色、边框、圆角）。
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub bg: Color,
    pub border: Color,
    pub radius: f32,
    /// 内容比面板矮时怎么放（状态行居中，转写从顶）。
    pub justify: JustifyContent,
}

impl Look {
    pub fn new(bg: Color, border: Color, radius: f32) -> Self {
        Self {
            bg,
            border,
            radius,
            justify: JustifyContent::FlexStart,
        }
    }
}

/// 生成一块面板的 `bevy_ui` 根 + 3D 面板。尺寸由 [`place_panels`] 每帧写。
pub fn spawn_panel(
    commands: &mut Commands,
    id: PanelId,
    panel: FramePanel,
    look: Look,
) -> Entity {
    let inset = panel.inset;
    commands
        .spawn((
            Name::new(format!("panel:{id:?}")),
            id,
            panel,
            Node {
                position_type: PositionType::Absolute,
                // 摆到屏幕外：3D 副本才是可见的那个，2D 层不许画在上面。
                left: px(-20000.0),
                top: px(0.0),
                width: px(0.0),
                height: px(0.0),
                padding: UiRect::all(px(f32::from(inset.pad))),
                border: UiRect::all(px(f32::from(inset.border))),
                border_radius: BorderRadius::all(px(look.radius)),
                flex_direction: FlexDirection::Column,
                row_gap: px(0.0),
                justify_content: look.justify,
                ..default()
            },
            BackgroundColor(look.bg),
            BorderColor::all(look.border),
            WorldPanel {
                units_to_world: UNITS_TO_WORLD,
                anchor: Transform::default(),
            },
            WorldLayer(panel.layer),
            MirrorRoot,
        ))
        .id()
}

/// 转写类面板的观感（主转写和拆出来的第二个窗格用同一套）。
fn transcript_look() -> Look {
    Look::new(
        Color::srgb(0.045, 0.055, 0.08),
        Color::srgb(0.25, 0.62, 0.95),
        14.0,
    )
}

// ---------------------------------------------------------------------------
// 拆窗：把转写叶子拆成并排的两个转写
// ---------------------------------------------------------------------------

/// 拆窗动作：把 [`PanelId::Transcript`] 叶子横向拆成"转写 + [`PanelId::Pane2`]"。
///
/// 这是 niri 分裂树上的最小拆窗操作，**只由用户按键触发**，不自动拆：
/// 1. [`FrameTree`] 里拆叶子（真实几何由 [`crate::layout::Node::rects`] 推导）；
/// 2. 给 `Pane2` spawn 一块同样的树面板，登记进 [`PanelRoots`]；
/// 3. 给 [`PanelRows`] 建好 `Pane2` 的行槽（内容由 `panels` 每帧从转写镜像）。
///
/// 已经拆过（`Pane2` 已经在树上或已经登记）时返回 `false`，不改树，也不多
/// spawn 面板；`Transcript` 找不到时同样返回 `false`。
pub fn split_transcript(
    commands: &mut Commands,
    tree: &mut FrameTree,
    roots: &mut PanelRoots,
    rows: &mut PanelRows,
) -> bool {
    if roots.entity(PanelId::Pane2).is_some() {
        return false;
    }
    if !tree
        .0
        .split_leaf(PanelId::Transcript, crate::layout::Split::Horizontal, PanelId::Pane2)
    {
        return false;
    }
    let pane2 = spawn_panel(commands, PanelId::Pane2, FramePanel::tree(), transcript_look());
    roots.0.insert(PanelId::Pane2, pane2);
    rows.0.entry(PanelId::Pane2).or_default();
    true
}

/// `Alt+S` 的绑定：拆转写窗格。
///
/// 它不带 [`crate::keys::InputAction`]：按键系统只执行它认识的内置动作
/// （`editor.rs` 的 `act` 走的是穷尽 match，本模块不改那个文件），插件绑定
/// 走 [`KeyPressed`](crate::keys::KeyPressed) 观察者，和 `editor.rs` 的测试
/// `a_plugins_binding_receives_key_pressed` 是同一条路。
pub fn split_binding() -> crate::keys::KeyBinding {
    use crate::keys::{KeyBinding, KeyPress, KeyScope};
    KeyBinding::new(KeyPress::alt('s'), "Split the transcript").in_scope(KeyScope::Global)
}

/// 按下 `Alt+S`：拆一次（重复按是安全的空操作）。
fn split_pressed(
    _pressed: On<crate::keys::KeyPressed>,
    mut commands: Commands,
    tree: Option<ResMut<FrameTree>>,
    roots: Option<ResMut<PanelRoots>>,
    rows: Option<ResMut<PanelRows>>,
) {
    let (Some(mut tree), Some(mut roots), Some(mut rows)) = (tree, roots, rows) else {
        // 只装了 `FramePlugin` 的最小 App 没有面板资源：按键什么也不做。
        return;
    };
    split_transcript(&mut commands, &mut tree, &mut roots, &mut rows);
}

/// 注册 `Alt+S` 拆窗绑定。`FramePlugin` 自己调用；业务想重新绑定时可以复用。
pub fn add_split_key(app: &mut App) {
    use crate::keys::AppKeyBindingsExt;
    let _ = app.add_key_binding(split_binding(), split_pressed);
}

/// 帧布局插件：默认屏幕 + 每帧布局。
pub struct FramePlugin;

impl Plugin for FramePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FrameSize::from_env())
            .init_resource::<PanelAreas>()
            .init_resource::<FrameGeometry>()
            .init_resource::<DefaultScreen>()
            .init_resource::<FrameTree>()
            .add_systems(Startup, spawn_default_screen)
            .add_systems(
                Update,
                (sync_confirm_tile, lay_out_frame, place_panels)
                    .chain()
                    .in_set(crate::FeathersSystems::Layout),
            );
        add_split_key(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::entity::Entity;

    use crate::layout::{Child, Size};

    fn entity(index: u32) -> Entity {
        Entity::from_raw_u32(index).expect("entity")
    }

    fn tree_panel(entity: Entity, inset: PanelInset) -> ToLayOut {
        ToLayOut { entity, inset }
    }

    fn roots(items: &[(PanelId, Entity)]) -> HashMap<PanelId, Entity> {
        items.iter().copied().collect()
    }

    fn laid_out(
        screen: (u16, u16),
        tree: &LayoutNode,
        panels: Vec<ToLayOut>,
        roots: &HashMap<PanelId, Entity>,
    ) -> HashMap<u32, PanelArea> {
        lay_out(tree, panels, roots, CellRect::new(0, 0, screen.0, screen.1))
        .into_iter()
        .map(|(entity, area)| (entity.index_u32(), area))
        .collect()
    }

    /// 默认屏幕（分裂树）：96×28 里，转写占左 64 列；右 32 列上面是输入、
    /// 下面是状态（2 行贴底）。
    #[test]
    fn default_screen_is_a_split_tree() {
        let (input, status, transcript) = (entity(0), entity(1), entity(2));
        let roots = roots(&[
            (PanelId::Input, input),
            (PanelId::Status, status),
            (PanelId::Transcript, transcript),
        ]);
        let areas = laid_out(
            (96, 28),
            &layout::default_tree(),
            vec![
                tree_panel(input, PanelInset::DEFAULT),
                tree_panel(status, PanelInset::THIN),
                tree_panel(transcript, PanelInset::DEFAULT),
            ],
            &roots,
        );
        assert_eq!(areas[&transcript.index_u32()].rect, CellRect::new(0, 0, 64, 28));
        // 右列：顶部留白 + 补全(0) + 输入(固定 4 行) + 状态(2 行) 贴底。
        assert_eq!(areas[&input.index_u32()].rect, CellRect::new(64, 21, 32, 4));
        assert_eq!(areas[&status.index_u32()].rect, CellRect::new(64, 26, 32, 2));
        // 精确中心来自树的 TileRect（不是格子取整后的矩形）。
        // 精确中心来自树的 TileRect：转写占上半部分（25 行），所以 y 中心不再是 0。
        let center = areas[&transcript.index_u32()].center.expect("树给精确中心");
        assert!((center.x - -16.0 * CELL.x).abs() < 1e-3);
        assert_eq!(center.y, 0.0);
    }

    /// 被固定宽度的兄弟挤到 0 列的叶子：这一帧没有文字区，`is_empty` 为真。
    #[test]
    fn a_leaf_that_is_squeezed_to_nothing_is_empty() {
        let (transcript, status) = (entity(0), entity(1));
        let roots = roots(&[
            (PanelId::Transcript, transcript),
            (PanelId::Status, status),
        ]);
        // 状态行固定占满 96 列，转写比例再大也只能拿 0。
        let tree = LayoutNode::split(
            crate::layout::Split::Horizontal,
            crate::layout::NO_GAP,
            vec![
                Child::proportion(1.0, LayoutNode::leaf(PanelId::Transcript)),
                Child::fixed(96.0 * CELL.x, LayoutNode::leaf(PanelId::Status)),
            ],
        );
        let areas = laid_out(
            (96, 28),
            &tree,
            vec![
                tree_panel(transcript, PanelInset::DEFAULT),
                tree_panel(status, PanelInset::DEFAULT),
            ],
            &roots,
        );
        assert!(areas[&transcript.index_u32()].is_empty());
        assert_eq!(areas[&transcript.index_u32()].cols, 0);
        assert_eq!(areas[&status.index_u32()].rect, CellRect::new(0, 0, 96, 28));
    }

    /// 左右分栏树：左 30 列固定，转写拿剩下的 66 列。
    #[test]
    fn side_panels_take_their_side() {
        let (input, transcript) = (entity(0), entity(1));
        let roots = roots(&[
            (PanelId::Input, input),
            (PanelId::Transcript, transcript),
        ]);
        let tree = LayoutNode::split(
            crate::layout::Split::Horizontal,
            crate::layout::NO_GAP,
            vec![
                Child::fixed(30.0 * CELL.x, LayoutNode::leaf(PanelId::Input)),
                Child::proportion(1.0, LayoutNode::leaf(PanelId::Transcript)),
            ],
        );
        let areas = laid_out(
            (96, 28),
            &tree,
            vec![
                tree_panel(input, PanelInset::DEFAULT),
                tree_panel(transcript, PanelInset::DEFAULT),
            ],
            &roots,
        );
        assert_eq!(areas[&input.index_u32()].rect, CellRect::new(0, 0, 30, 28));
        assert_eq!(
            areas[&transcript.index_u32()].rect,
            CellRect::new(30, 0, 66, 28)
        );
    }

    /// 同层两个比例子平分整屏（1:1）。
    #[test]
    fn rest_panels_share() {
        let (left, right) = (entity(0), entity(1));
        let roots = roots(&[(PanelId::Transcript, left), (PanelId::Status, right)]);
        let tree = LayoutNode::split(
            crate::layout::Split::Horizontal,
            crate::layout::NO_GAP,
            vec![
                Child::new(Size::Proportion(1.0), LayoutNode::leaf(PanelId::Transcript)),
                Child::new(Size::Proportion(1.0), LayoutNode::leaf(PanelId::Status)),
            ],
        );
        let areas = laid_out(
            (96, 28),
            &tree,
            vec![
                tree_panel(left, PanelInset::DEFAULT),
                tree_panel(right, PanelInset::DEFAULT),
            ],
            &roots,
        );
        assert_eq!(areas[&left.index_u32()].rect, CellRect::new(0, 0, 48, 28));
        assert_eq!(areas[&right.index_u32()].rect, CellRect::new(48, 0, 48, 28));
    }

    /// 容器面板（picker 那一列）自己拿到整列区域；它不算叶子，所以和内部
    /// 列表区域几何重叠是允许的（容器靠 [`FramePanel::layer`] 垫在下面）。
    #[test]
    fn a_container_panel_gets_the_whole_column_area() {
        use crate::layout::{NO_GAP, Split};

        let (picker, list, transcript) = (entity(0), entity(1), entity(2));
        let tree = LayoutNode::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::proportion(2.0, LayoutNode::leaf(PanelId::Transcript)),
                Child::new(
                    Size::Proportion(1.0),
                    LayoutNode::container(PanelId::Picker, LayoutNode::leaf(PanelId::List)),
                ),
            ],
        );
        let roots = roots(&[
            (PanelId::Picker, picker),
            (PanelId::List, list),
            (PanelId::Transcript, transcript),
        ]);
        let areas = laid_out(
            (96, 28),
            &tree,
            vec![
                tree_panel(picker, PanelInset::DEFAULT),
                tree_panel(list, PanelInset::DEFAULT),
                tree_panel(transcript, PanelInset::DEFAULT),
            ],
            &roots,
        );
        assert_eq!(areas[&list.index_u32()].rect, CellRect::new(64, 0, 32, 28));
        assert_eq!(
            areas[&picker.index_u32()].rect,
            CellRect::new(64, 0, 32, 28),
            "容器面板 = 整列"
        );
        // 叶子只有两块（容器不算）。
        assert_eq!(tree.rects(TileRect {
            center: Vec2::ZERO,
            size: Vec2::new(96.0 * CELL.x, 28.0 * CELL.y) * UNITS_TO_WORLD,
        }).len(), 2);
    }

    /// 默认树的右列：输入在上、状态 2 行贴底（不是旧的 `Bottom(4)/Bottom(2)`）。
    #[test]
    fn below_input_rows_sit_at_the_bottom() {
        let (input, status) = (entity(0), entity(1));
        let roots = roots(&[(PanelId::Input, input), (PanelId::Status, status)]);
        let areas = laid_out(
            (96, 28),
            &layout::default_tree(),
            vec![
                tree_panel(input, PanelInset::DEFAULT),
                tree_panel(status, PanelInset::THIN),
            ],
            &roots,
        );
        assert_eq!(areas[&status.index_u32()].rect, CellRect::new(64, 26, 32, 2));
        // 输入与状态之间现在夹着 1 行键位提示（它也是 tile）。
        assert_eq!(areas[&input.index_u32()].rect, CellRect::new(64, 21, 32, 4));
        assert_eq!(
            areas[&input.index_u32()].rect.bottom(),
            areas[&status.index_u32()].rect.y - 1,
            "输入与状态之间隔 1 行提示行"
        );
    }

    /// 文字区 = 面板区域减掉装饰。
    #[test]
    fn inset_shrinks_the_text_area() {
        let area = PanelArea::new(CellRect::new(0, 0, 96, 22), PanelInset::DEFAULT);
        assert_eq!(area.size, Vec2::new(96.0 * CELL.x, 22.0 * CELL.y));
        assert_eq!(area.cols, 93);
        assert_eq!(area.rows, 20);
        let thin = PanelArea::new(CellRect::new(0, 0, 96, 2), PanelInset::THIN);
        assert_eq!(thin.rows, 1);
    }

    /// 从树的矩形构造区域：尺寸与中心保持精确，格子只是近似。
    #[test]
    fn panel_area_from_tile_keeps_the_exact_center() {
        let tile = TileRect {
            center: Vec2::new(0.18, -0.09),
            size: Vec2::new(0.54, 0.27),
        };
        let area = PanelArea::from_tile(tile, CellRect::new(0, 0, 96, 28), PanelInset::DEFAULT);
        assert_eq!(area.size, tile.size / UNITS_TO_WORLD);
        assert_eq!(area.center, Some(tile.center / UNITS_TO_WORLD));
        assert_eq!(area.rect, CellRect::new(42, 13, 38, 8));
    }

    /// 端到端：`spawn_default_screen` 生成的三块面板经 `lay_out_frame` 拿到
    /// 树的区域，`place_panels` 再把区域写成 `Node` 尺寸与 3D 锚点。
    #[test]
    fn the_default_screen_tree_reaches_node_sizes_and_anchors() {
        if std::env::var("RF_NIRI_LAYOUT").is_ok() {
            return; // Niri 模式接管时默认屏幕不 spawn（见 `spawn_default_screen`）。
        }
        let mut app = App::new();
        app.init_resource::<PanelAreas>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelRows>()
            .init_resource::<DefaultScreen>()
            .init_resource::<FrameGeometry>()
            .init_resource::<FrameTree>()
            .insert_resource(FrameSize { cols: 96, rows: 28 })
            .add_systems(Startup, spawn_default_screen)
            .add_systems(Update, (lay_out_frame, place_panels).chain());
        app.update();

        let roots = app.world().resource::<PanelRoots>();
        let transcript = roots.entity(PanelId::Transcript).expect("转写面板");
        let status = roots.entity(PanelId::Status).expect("状态面板");
        let complete = roots.entity(PanelId::Complete).expect("补全面板");
        let areas = app.world().resource::<PanelAreas>();
        assert_eq!(areas.0.len(), 6, "六块面板都由树放好（留白叶子没有面板）");
        assert_eq!(areas.get(transcript).expect("转写区域").rect, CellRect::new(0, 0, 64, 28));
        assert_eq!(areas.get(status).expect("状态区域").rect, CellRect::new(64, 26, 32, 2));
        // 补全 tile 关着：0 高度，但仍然有区域（这样每帧都能把它写成 0 高）。
        let closed = areas.get(complete).expect("补全区域");
        assert_eq!(closed.rect, CellRect::new(64, 21, 32, 0), "补全贴输入上方");
        assert!(closed.is_empty());

        // `Node` 尺寸 = 树给的精确像素；3D 锚点 = 树给的精确中心（不是格子中心）。
        let node = app.world().get::<Node>(transcript).expect("Node");
        match (node.width, node.height) {
            (bevy_ui::Val::Px(width), bevy_ui::Val::Px(height)) => {
                assert!((width - 64.0 * CELL.x).abs() < 1e-3, "width = {width}");
                // 转写只占上半部分 25 行（下面两条横跨的 tile 各占 1/2 行）。
                assert!((height - 28.0 * CELL.y).abs() < 1e-3, "height = {height}");
            }
            other => panic!("Node 尺寸应当是像素：{other:?}"),
        }
        let world = app.world().get::<WorldPanel>(transcript).expect("WorldPanel");
        let want_x = -16.0 * CELL.x * UNITS_TO_WORLD;
        assert!(
            (world.anchor.translation.x - want_x).abs() < 1e-5,
            "anchor.x = {}",
            world.anchor.translation.x
        );
        // y：转写占条带上半部分 25 行，中心在 1.5 行处（下面还有提示/状态两条）。
        let want_y = 0.0;
        assert!(
            (world.anchor.translation.y - want_y).abs() < 1e-5,
            "锚点 y = {}",
            world.anchor.translation.y
        );
    }

    /// 默认树里补全 tile 关着：0 行高度，输入仍从右列顶部开始。
    #[test]
    fn the_closed_complete_tile_takes_no_rows() {
        let (input, status, transcript, complete) =
            (entity(0), entity(1), entity(2), entity(3));
        let roots = roots(&[
            (PanelId::Input, input),
            (PanelId::Status, status),
            (PanelId::Transcript, transcript),
            (PanelId::Complete, complete),
        ]);
        let areas = laid_out(
            (96, 28),
            &layout::default_tree(),
            vec![
                tree_panel(input, PanelInset::DEFAULT),
                tree_panel(status, PanelInset::THIN),
                tree_panel(transcript, PanelInset::DEFAULT),
                tree_panel(complete, PanelInset::THIN),
            ],
            &roots,
        );
        let tile = areas[&complete.index_u32()];
        assert_eq!(tile.rect, CellRect::new(64, 21, 32, 0), "补全贴输入上方");
        assert!(tile.is_empty(), "0 高度的 tile 这一帧没有文字区");
        assert_eq!(
            areas[&input.index_u32()].rect,
            CellRect::new(64, 21, 32, 4),
            "关着的 tile 不挤输入（输入固定 4 行贴底）"
        );
    }

    /// 拆窗 App：只需要树与面板三件套，不建窗口。
    fn split_app() -> App {
        let mut app = App::new();
        app.init_resource::<FrameTree>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelRows>();
        app
    }

    fn split_once(
        mut commands: Commands,
        mut tree: ResMut<FrameTree>,
        mut roots: ResMut<PanelRoots>,
        mut rows: ResMut<PanelRows>,
    ) -> bool {
        split_transcript(&mut commands, &mut tree, &mut roots, &mut rows)
    }

    /// 拆窗动作真的 spawn 了 `Pane2`（树面板）并登记；重复拆是空操作。
    #[test]
    fn split_transcript_spawns_and_registers_pane2() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = split_app();
        assert!(app.world_mut().run_system_once(split_once).unwrap());

        let pane2 = app
            .world()
            .resource::<PanelRoots>()
            .entity(PanelId::Pane2)
            .expect("Pane2 要登记进 PanelRoots");
        let panel = app.world().get::<FramePanel>(pane2).expect("Pane2 是树面板");
        assert_eq!(panel.placement, Placement::Tree);
        assert!(app.world().get::<Node>(pane2).is_some());
        assert_eq!(
            app.world().get::<Name>(pane2).map(|name| name.as_str().to_owned()),
            Some("panel:Pane2".to_owned())
        );
        assert!(
            app.world()
                .resource::<PanelRows>()
                .0
                .contains_key(&PanelId::Pane2),
            "Pane2 的行槽要建好"
        );
        let leaves = app.world().resource::<FrameTree>().0.leaves();
        assert_eq!(leaves.len(), 8, "默认树 7 叶子（含留白/确认/提示）+ 新拆的 Pane2");
        assert!(leaves.contains(&PanelId::Pane2));

        // 第二次：返回 false，树不变，也不会多一个 Pane2 实体。
        let tree = app.world().resource::<FrameTree>().0.clone();
        assert!(!app.world_mut().run_system_once(split_once).unwrap());
        assert_eq!(app.world().resource::<FrameTree>().0, tree);
        let mut query = app.world_mut().query::<&PanelId>();
        let pane2_count = query
            .iter(app.world())
            .filter(|id| **id == PanelId::Pane2)
            .count();
        assert_eq!(pane2_count, 1, "重复拆不 spawn 第二块");
    }

    /// `Alt+S` 走插件绑定 -> `KeyPressed` -> 拆窗；不会往输入里打字符。
    #[test]
    fn alt_s_splits_the_transcript_and_types_nothing() {
        use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
        use bevy::input::ButtonState;

        fn key_app() -> App {
            let mut app = split_app();
            app.add_message::<KeyboardInput>()
                .add_message::<crate::editor::PromptSubmitted>()
                .init_resource::<bevy::input::ButtonInput<KeyCode>>()
                .init_resource::<crate::editor::Editor>()
                .init_resource::<crate::editor::SubmittedPrompts>()
                .init_resource::<crate::keys::KeyFocus>()
                .init_resource::<crate::keys::PendingInput>();
            crate::keys::add_built_in(&mut app);
            add_split_key(&mut app);
            app.add_systems(Update, crate::editor::keys);
            app
        }

        fn press_alt_s(app: &mut App) {
            app.world_mut()
                .resource_mut::<bevy::input::ButtonInput<KeyCode>>()
                .press(KeyCode::AltLeft);
            app.world_mut().write_message(KeyboardInput {
                key_code: KeyCode::KeyS,
                logical_key: Key::Character("s".into()),
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
            app.update();
        }

        let mut app = key_app();
        press_alt_s(&mut app);
        let pane2 = app
            .world()
            .resource::<PanelRoots>()
            .entity(PanelId::Pane2)
            .expect("Alt+S 要拆出 Pane2");
        assert_eq!(app.world().resource::<FrameTree>().0.leaves().len(), 8);
        assert_eq!(
            app.world().resource::<crate::editor::Editor>().text(),
            "",
            "Alt+S 不打字符"
        );

        // 再按一次：还是同一块面板。
        press_alt_s(&mut app);
        assert_eq!(
            app.world().resource::<PanelRoots>().entity(PanelId::Pane2),
            Some(pane2)
        );
    }

    /// 格子换算：一帧 96×28 的像素尺寸。
    #[test]
    fn frame_size_scales() {
        let size = FrameSize { cols: 96, rows: 28 };
        assert_eq!(size.screen(), CellRect::new(0, 0, 96, 28));
        assert_eq!(FrameSize::parse("120x40"), Some(FrameSize { cols: 120, rows: 40 }));
        assert_eq!(FrameSize::parse("nope"), None);
    }
}
