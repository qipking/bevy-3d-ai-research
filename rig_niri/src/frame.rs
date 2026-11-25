//! 帧布局：把 [`crate::layout`] 的分裂树落到每块面板的 `Node` 尺寸与 3D 锚点上。
//!
//! rig-tui 用"绝对位置槽"拼屏幕（`Placement::Bottom(4)`、`Rest`……）。3D 版
//! **不照搬**：窗口是分裂树上的叶子，位置和大小都由 [`LayoutNode::rects`] 递归推导。
//! 一块面板能拿到的矩形只取决于树，跟"离底边几行"无关。
//!
//! [`FrameSize`] 默认 96×28，但在装了 [`FramePlugin`] 的 App 里由主窗口的
//! **逻辑**像素尺寸驱动：[`sync_frame_size_to_window`] 每帧只读窗口、换算成
//! 格子数（[`FrameSize::from_window_size`]），值变了才写资源——窗口拖大/拖小
//! 就多/少几行几列，[`lay_out_frame`]、行窗口化、工作区条带间距都跟着走。
//! 截图与测试要确定性时用 `RF_FRAME=96x28`：只要它能解析，尺寸在启动时就被
//! [`FrameSizePinned`] 钉住，窗口再变也不覆盖。
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
use bevy::window::{PrimaryWindow, Window};
use bevy_feathers::theme::{ThemeBackgroundColor, ThemeBorderColor, ThemeToken, UiTheme};
use bevy_ui::{BorderRadius, Node, UiRect, px};
use feathers_in_3d::{MirrorRoot, WorldLayer, WorldPanel};

use crate::layout::{self, Node as LayoutNode};
use crate::niri::TileRect;
use crate::pane::{ActiveTranscript, TranscriptPane};
use crate::panels::{PanelId, PanelRows};
use crate::theme::{
    BAR_BG, CONFIRM_BG, FOCUS_BORDER, INPUT_BG, PANEL_BG, PANEL_BORDER as PANEL_BORDER_TOKEN,
    PANEL_RADIUS,
};
use crate::workspaces::{Workspace, WorkspacePanel, WorkspacePanels, WorkspaceSet};

pub use crate::panels::PanelRoots;

/// 一个格子在 UI 像素里的尺寸：宽 = 13px 等宽字的前进量，高 = 一行文本。
pub const CELL: Vec2 = Vec2::new(7.8, crate::transcript::ROW_HEIGHT);

/// 面板默认装饰（内边距 + 边框）。
///
/// feathers 目前只把**颜色**做成 design token（`bevy_feathers::tokens`），
/// 间距/边框宽度还没有 token：`containers::pane` 的 body padding 是 6px、
/// `group` 的边框是 1px。这里先集中成两个常量（不要散落到 `PanelInset`
/// 的字面量里），等 feathers 出 spacing/border token 后由 token 覆盖。
pub const PANEL_PADDING: f32 = 8.0;
pub const PANEL_BORDER: f32 = 2.0;

/// 细条（状态行 / 键位提示）的内边距与边框：同样等 feathers token。
pub const THIN_PADDING: f32 = 4.0;
pub const THIN_BORDER: f32 = 1.0;

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
///
/// 默认 96×28；装了 [`FramePlugin`] 后由主窗口的逻辑像素尺寸驱动，窗口
/// resize / 换显示器 / DPI 缩放都会让格子数跟着变（见
/// [`sync_frame_size_to_window`]）。
///
/// **确定性覆盖**：`RF_FRAME=96x28` 能解析时，[`FramePlugin`] 在启动时就把
/// 尺寸钉住（[`FrameSizePinned`]），窗口怎么拖都不再覆盖它——截图与测试依赖
/// 这一条。
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

    /// 启动时的确定性尺寸：`RF_FRAME` 可解析就用它，否则 [`Default`]。
    ///
    /// 优先级（[`FramePlugin`]）：
    /// 1. `RF_FRAME` 可解析 → 固定这个值，**窗口尺寸不覆盖**；
    /// 2. 否则 → 由主窗口逻辑像素驱动（[`FrameSize::from_window_size`]）。
    pub fn from_env() -> Self {
        Self::env_override().unwrap_or_default()
    }

    /// `RF_FRAME` 存在且可解析时的覆盖值；没设 / 坏值返回 `None`。
    ///
    /// `None` 才允许窗口接管。命令行的截图/测试脚本靠这里保确定性。
    /// 坏值的警告由 [`FramePlugin`] 通过 [`FrameEnv::from_env`] 打，见
    /// [`FrameEnv::warning`]。
    pub fn env_override() -> Option<Self> {
        FrameEnv::from_env().size()
    }

    /// 解析一个可选的 `RF_FRAME` 文本值（读环境变量与解析分开，方便测试）。
    ///
    /// `None` 或 `"nope"` 这类坏值都返回 `None`，与"没设环境变量"同义。
    pub fn from_env_text(value: Option<&str>) -> Option<Self> {
        FrameEnv::parse(value).size()
    }

    /// 主窗口的**逻辑**像素尺寸 → 格子数。
    ///
    /// `cols = floor(width / CELL.x)`、`rows = floor(height / CELL.y)`，各至少 1。
    /// 用 floor 而不是 round：一列格子必须**完整**放得下，帧不会因为半格像素
    /// 反过来超出窗口。窗口比一个格子还小（含 0 / 负数 / 非有限值）时钳到
    /// 1×1——布局对 0 列/行本来就有 [`PanelArea::is_empty`] 兜底，所以极小窗口
    /// 不会 panic。
    ///
    /// 为什么用 **logical**：布局写进 `Node` 的 `Val::Px`、[`CELL`]、字号都是
    /// 逻辑像素，`Window::size()` 也是逻辑像素（`physical_size() /
    /// scale_factor()`）；格子数 = 逻辑窗口 / 单元格逻辑尺寸，2× DPI 下才会
    /// 保持"字号与列数的比例"不变。若改用 physical，格子数会按缩放因子翻倍，
    /// 而 `Node` 的 `Val::Px` 仍是逻辑单位，面板会撑出窗口。
    ///
    /// 注意 `feathers_in_3d` 镜像读的是 `ComputedNode.size`（bevy 报的是**物理**
    /// 像素，1× 缩放下与逻辑值相同）。这里只定布局格子数；镜像在非 1× DPI 下
    /// 的物理/逻辑换算与相机取景是另一件事，别把它混进格子换算里。
    pub fn from_window_size(size: Vec2) -> Self {
        fn cells(pixels: f32, cell: f32) -> u16 {
            if !pixels.is_finite() || pixels <= 0.0 {
                return 1;
            }
            // `as u16` 对超大的 float 是饱和转换（到 u16::MAX），不会回绕。
            ((pixels / cell).floor() as u16).max(1)
        }
        Self {
            cols: cells(size.x, CELL.x),
            rows: cells(size.y, CELL.y),
        }
    }

    pub fn screen(&self) -> CellRect {
        CellRect::new(0, 0, self.cols, self.rows)
    }
}

/// `RF_FRAME` 的解析结果。
///
/// 把"没设 / 好值 / 坏值"做成可断言的结构：坏值保留原始文本，[`FrameEnv::warning`]
/// 给出要打的警告。行为与以前一致——只有 [`FrameEnv::Pinned`] 会钉住尺寸；坏值
/// 仍按"没设"处理（窗口尺寸接管），但不再静默，[`FramePlugin`] 会 `warn!` 一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameEnv {
    /// 环境变量没设：窗口尺寸接管。
    Unset,
    /// 环境变量可解析：启动时钉住这个尺寸。
    Pinned(FrameSize),
    /// 环境变量存在但解析失败：保留原始值，报告后按"没设"处理。
    Invalid(String),
}

impl FrameEnv {
    /// 读进程环境里的 `RF_FRAME`。
    pub fn from_env() -> Self {
        Self::parse(std::env::var("RF_FRAME").ok().as_deref())
    }

    /// 解析一个可选的 `RF_FRAME` 文本值（不含环境变量读取，方便测试）。
    ///
    /// 空串同样是坏值：环境变量"存在"却解析不出 `COLSxROWS`。
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            None => Self::Unset,
            Some(raw) => match FrameSize::parse(raw) {
                Some(size) => Self::Pinned(size),
                None => Self::Invalid(raw.to_string()),
            },
        }
    }

    /// 可以钉住的尺寸；[`FrameEnv::Unset`] / [`FrameEnv::Invalid`] 都是 `None`。
    pub fn size(&self) -> Option<FrameSize> {
        match self {
            Self::Pinned(size) => Some(*size),
            Self::Unset | Self::Invalid(_) => None,
        }
    }

    /// 坏值要打的警告（带原始值和期望格式）；好值/没设返回 `None`。
    pub fn warning(&self) -> Option<String> {
        match self {
            Self::Invalid(raw) => Some(format!(
                "[frame] RF_FRAME={raw:?} 无法解析；期望格式 COLSxROWS（例如 96x28），\
                 本次按未设置处理，尺寸改由窗口决定"
            )),
            Self::Unset | Self::Pinned(_) => None,
        }
    }
}

/// `FrameSize` 被钉住：窗口尺寸不再覆盖它（`RF_FRAME` 生效时由 [`FramePlugin`]
/// 插入，见 [`FrameSize::from_env`]）。
///
/// 只影响 [`sync_frame_size_to_window`]；布局系统照常读 [`FrameSize`]。
/// 截图/测试要的是"窗口怎么拖都是同一套格子"，这个资源就是那条确定性路径。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameSizePinned(pub bool);

/// 文字区能从面板里拿到的大小：面板尺寸减掉装饰。
///
/// `pad`/`border` 的单位是 UI 像素，数值来自 [`PANEL_PADDING`]/[`PANEL_BORDER`]
/// （细条用 [`THIN_PADDING`]/[`THIN_BORDER`]）——这几个常量是唯一的来源，
/// 不要在别处再写字面量。feathers 的间距 token 落地后，只改这几个常量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelInset {
    pub pad: u16,
    pub border: u16,
}

impl PanelInset {
    /// 默认：内边距 8px、边框 2px（[`PANEL_PADDING`] / [`PANEL_BORDER`]）。
    pub const DEFAULT: Self = Self {
        pad: PANEL_PADDING as u16,
        border: PANEL_BORDER as u16,
    };
    /// 细条（状态行 / 键位提示）：内边距 4px、边框 1px
    /// （[`THIN_PADDING`] / [`THIN_BORDER`]）。
    pub const THIN: Self = Self {
        pad: THIN_PADDING as u16,
        border: THIN_BORDER as u16,
    };

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

/// 把主窗口的逻辑像素尺寸换算成 [`FrameSize`]；值没变就**不写资源**。
///
/// 每帧只做一次只读查询再比较：winit 在发 `WindowResized` 之前已经更新了
/// `Window`，所以窗口拖动、DPI/缩放变化、启动时的初始尺寸都走同一条路径，
/// 不需要另外订阅 resize 消息（`selection.rs` 仍按消息清选区）。
///
/// 三种情况跳过：
/// - [`FrameSizePinned`] 为真（`RF_FRAME` 生效）：截图/测试的确定性优先，
///   窗口怎么拖都不改格子数；
/// - 没有 `PrimaryWindow`（headless 测试、只装布局的 App）；
/// - 窗口尺寸 ≤ 0（最小化）：保留上一次的格子数，恢复时再算，避免布局在
///   1×1 和正常尺寸之间来回重建。
///
/// 只有换算结果真的变了才写 [`FrameSize`]（比较后再写），并打一行 `info!`；
/// 没变化时连资源变更标记都不碰，下游不会无谓重建。
pub fn sync_frame_size_to_window(
    windows: Query<&Window, With<PrimaryWindow>>,
    pinned: Option<Res<FrameSizePinned>>,
    mut size: ResMut<FrameSize>,
) {
    if pinned.is_some_and(|pinned| pinned.0) {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let logical = window.size();
    if logical.x <= 0.0 || logical.y <= 0.0 {
        return;
    }
    let next = FrameSize::from_window_size(logical);
    if *size == next {
        return;
    }
    let previous = *size;
    *size = next;
    info!(
        "[frame] window {:.0}x{:.0} logical px -> {}x{} cells (was {}x{})",
        logical.x, logical.y, next.cols, next.rows, previous.cols, previous.rows
    );
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
    // 表面用 feathers 的 field 底，边框用 focus ring（composer 是常驻焦点）。
    let input = spawn(
        commands,
        PanelId::Input,
        FramePanel::tree(),
        Look::panel(INPUT_BG, FOCUS_BORDER),
    );
    // 状态行：右列底部固定 2 格，细条里居中放 1 行字。
    let status = spawn(
        commands,
        PanelId::Status,
        FramePanel::tree().inset(PanelInset::THIN),
        Look::panel(BAR_BG, PANEL_BORDER_TOKEN).justify(JustifyContent::Center),
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
        Look::panel(BAR_BG, PANEL_BORDER_TOKEN),
    );
    // 键位提示行：固定 1 行，窄条（THIN inset），内容由 `hints::hint_panel_rows` 写。
    let hints = spawn(
        commands,
        PanelId::Hints,
        // 无内边距：这一格只有 1 行高，任何 padding 都会把字裁掉。
        FramePanel::tree().inset(PanelInset { pad: 0, border: 0 }),
        Look::panel(BAR_BG, PANEL_BORDER_TOKEN).justify(JustifyContent::FlexStart),
    );
    // 确认行（Ask）：没有待确认时树里是 0 高度，有就长成 n 行。
    let confirm = spawn(
        commands,
        PanelId::Confirm,
        FramePanel::tree().inset(PanelInset::THIN),
        Look::panel(CONFIRM_BG, PANEL_BORDER_TOKEN),
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

/// 面板底色/边框的来源。
///
/// F1 之后默认屏幕走 [`Paint::Token`]（feathers 的 design token）；另一个变体
/// 留给还没迁移的调用点（picker 列、niri 默认屏幕、测试里的透明叠加面板）：
/// 它们传的是 `Color`，行为与换皮前完全一样。
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    /// 直接颜色：不挂 theme 组件，`UiTheme` 换了它不跟着变。
    Color(Color),
    /// feathers 设计 token：颜色由 `UiTheme` 解析（没有资源时用 feathers 暗色
    /// 主题兜底），并挂 [`ThemeBackgroundColor`]/[`ThemeBorderColor`] 组件。
    Token(ThemeToken),
}

impl From<Color> for Paint {
    fn from(color: Color) -> Self {
        Self::Color(color)
    }
}

impl From<ThemeToken> for Paint {
    fn from(token: ThemeToken) -> Self {
        Self::Token(token)
    }
}

/// 一块面板的观感（底色、边框、圆角）。
///
/// `bg`/`border` 可以是 feathers 的 [`ThemeToken`]（[`Look::panel`]）或直接
/// `Color`（[`Look::new`]，旧调用点）。圆角没有 feathers token，用
/// [`crate::theme::PANEL_RADIUS`]。
#[derive(Clone, Debug)]
pub struct Look {
    pub bg: Paint,
    pub border: Paint,
    pub radius: f32,
    /// 内容比面板矮时怎么放（状态行居中，转写从顶）。
    pub justify: JustifyContent,
}

impl Look {
    /// 直接给颜色：不挂 theme 组件，`UiTheme` 变了不跟着变。
    ///
    /// 参数是 `impl Into<Paint>`，所以老的 `Look::new(Color, Color, radius)`
    /// 调用点（picker / niri / 测试）不用改。
    pub fn new(bg: impl Into<Paint>, border: impl Into<Paint>, radius: f32) -> Self {
        Self {
            bg: bg.into(),
            border: border.into(),
            radius,
            justify: JustifyContent::FlexStart,
        }
    }

    /// 面板 chrome 全部走 feathers token：底色/边框挂 theme 组件，圆角用
    /// [`crate::theme::PANEL_RADIUS`]（feathers `containers::pane`/`group` 的 4px）。
    pub fn panel(bg: ThemeToken, border: ThemeToken) -> Self {
        Self::new(bg, border, PANEL_RADIUS)
    }

    /// 内容比面板矮时怎么放（状态行居中）。
    pub fn justify(mut self, justify: JustifyContent) -> Self {
        self.justify = justify;
        self
    }
}

/// 面板 chrome 由 feathers token 驱动（挂了 `ThemeBackgroundColor`/`ThemeBorderColor`）。
///
/// [`sync_panel_chrome`] 只刷这些实体：直接给颜色的旧面板不归它管。
#[derive(Component, Clone, Copy, Debug)]
pub struct PanelChrome;

/// 把一个 [`Paint`] 解析成当前颜色：有 tokens 走主题，没主题走 feathers 暗色兜底。
fn resolve_paint(paint: &Paint) -> Color {
    match paint {
        Paint::Color(color) => *color,
        Paint::Token(token) => crate::theme::ui_theme_or_default(None).color(token),
    }
}

/// 每帧把面板根上的 token 解析成真实颜色。
///
/// * 有 `UiTheme` 资源：用资源里的颜色（测试可断言 `== UiTheme.color(token)`）；
/// * 没有资源（headless / 最小 App）：退到 [`crate::theme::default_ui_theme`]，
///   不 panic；
/// * 只在颜色真的变了才写 `BackgroundColor`/`BorderColor`，不每帧触发
///   change detection。
///
/// 装了 `bevy_feathers::FeathersPlugins` 时它的 `theme::update_theme` 也会刷
/// 同样的组件（PostUpdate，带 `ThemeContext`）；两边解析的是同一个 token，
/// 结果一致，先后无所谓。
fn sync_panel_chrome(
    theme: Option<Res<UiTheme>>,
    mut backgrounds: Query<(&ThemeBackgroundColor, &mut BackgroundColor), With<PanelChrome>>,
    mut borders: Query<(&ThemeBorderColor, &mut BorderColor), With<PanelChrome>>,
) {
    let theme = crate::theme::ui_theme_or_default(theme.as_deref());
    for (ThemeBackgroundColor(token), mut background) in &mut backgrounds {
        let color = theme.color(token);
        if background.0 != color {
            background.0 = color;
        }
    }
    for (ThemeBorderColor(token), mut border) in &mut borders {
        let color = theme.color(token);
        if border.top != color {
            border.set_all(color);
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
    // 先按"没有主题资源"的 feathers 暗色主题给一个初值；有 `UiTheme` 时
    // `sync_panel_chrome` 在同一帧的 Update 里会刷成资源里的颜色。
    let background = resolve_paint(&look.bg);
    let border = resolve_paint(&look.border);
    let mut entity = commands.spawn((
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
        BackgroundColor(background),
        BorderColor::all(border),
        WorldPanel {
            units_to_world: UNITS_TO_WORLD,
            anchor: Transform::default(),
        },
        WorldLayer(panel.layer),
        MirrorRoot,
    ));
    // token 版 chrome：挂 feathers 的 theme 组件，并标记给 `sync_panel_chrome`。
    let mut themed = false;
    if let Paint::Token(token) = &look.bg {
        entity.insert(ThemeBackgroundColor(token.clone()));
        themed = true;
    }
    if let Paint::Token(token) = &look.border {
        entity.insert(ThemeBorderColor(token.clone()));
        themed = true;
    }
    if themed {
        entity.insert(PanelChrome);
    }
    // 转写类窗格带逐窗格状态（来源/标题/滚动/选中）；其余面板没有。
    if matches!(id, PanelId::Transcript | PanelId::Pane2) {
        entity.insert(TranscriptPane::default());
    }
    entity.id()
}

/// 转写类面板的观感（主转写和拆出来的第二个窗格用同一套）。
///
/// 表面/边框走 feathers pane token（比状态行亮一档的 pane body）；圆角 4px。
fn transcript_look() -> Look {
    Look::panel(PANEL_BG, PANEL_BORDER_TOKEN)
}

// ---------------------------------------------------------------------------
// 拆窗：把转写叶子拆成并排的两个转写
// ---------------------------------------------------------------------------

/// 拆窗动作：把 [`PanelId::Transcript`] 叶子横向拆成"转写 + [`PanelId::Pane2`]"。
///
/// 这是 niri 分裂树上的最小拆窗操作，**只由用户按键触发**，不自动拆：
/// 1. [`FrameTree`] 里拆叶子（真实几何由 [`crate::layout::Node::rects`] 推导）；
/// 2. 给 `Pane2` spawn 一块同样的树面板（[`spawn_panel`] 会给它挂默认
///    [`TranscriptPane`]），登记进 [`PanelRoots`]；
/// 3. 给 [`PanelRows`] 建好 `Pane2` 的行槽（S1 内容仍由 `panels` 每帧从转写镜像）；
/// 4. 把 [`ActiveTranscript`] 指到这块新窗格（键/滚轮从下一帧起作用于它）。
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
    // 拆出来的窗格成为活动窗格；`spawn_panel` 已挂好默认 `TranscriptPane`
    //（来源 `Shown`、无标题、自己的滚动从末尾跟随开始）。
    commands.insert_resource(ActiveTranscript(Some(pane2)));
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
        // `RF_FRAME` 生效 = 确定性覆盖：启动时钉住，窗口 resize 不再改它。
        // 坏值不能再静默忽略（截图脚本的 `96X28` 会悄悄跟窗口）：先 warn，
        // 再按"没设"处理——行为除日志外与以前一致。
        let env = FrameEnv::from_env();
        if let Some(warning) = env.warning() {
            warn!("{warning}");
        }
        let env_size = env.size();
        app.insert_resource(env_size.unwrap_or_default())
            .insert_resource(FrameSizePinned(env_size.is_some()))
            .init_resource::<PanelAreas>()
            .init_resource::<FrameGeometry>()
            .init_resource::<DefaultScreen>()
            .init_resource::<FrameTree>()
            .add_systems(Startup, spawn_default_screen)
            .add_systems(
                Update,
                (
                    sync_frame_size_to_window,
                    sync_confirm_tile,
                    lay_out_frame,
                    place_panels,
                )
                    .chain()
                    .in_set(crate::FeathersSystems::Layout),
            )
            // chrome 的 token → 颜色解析：布局之后、渲染之前（没有 `UiTheme`
            // 资源时用 feathers 暗色主题兜底，headless 测试也跑这条路）。
            .add_systems(
                Update,
                sync_panel_chrome.in_set(crate::FeathersSystems::Render),
            );
        add_split_key(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::entity::Entity;
    use bevy::window::WindowResolution;

    use crate::layout::{Child, Size};

    fn entity(index: u32) -> Entity {
        Entity::from_raw_u32(index).expect("entity")
    }

    /// 主窗口实体（bevy_ui 与镜像都用逻辑像素；这里是 1× 缩放）。
    fn primary_window(app: &mut App, width: u32, height: u32) -> Entity {
        app.world_mut()
            .spawn((
                Window {
                    resolution: WindowResolution::new(width, height),
                    ..default()
                },
                PrimaryWindow,
            ))
            .id()
    }

    /// 改窗口尺寸并像 winit 一样补一条 `WindowResized`（逻辑像素）。
    fn resize_window(app: &mut App, window: Entity, width: u32, height: u32) {
        app.world_mut()
            .get_mut::<Window>(window)
            .expect("主窗口")
            .resolution = WindowResolution::new(width, height);
        app.world_mut().write_message(bevy::window::WindowResized {
            window,
            width: width as f32,
            height: height as f32,
        });
    }

    /// 只装帧布局四件套 + 主窗口的最小 App（不建 UI 插件、不渲染）。
    fn windowed_frame_app(width: u32, height: u32) -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<PanelAreas>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelRows>()
            .init_resource::<DefaultScreen>()
            .init_resource::<FrameGeometry>()
            .init_resource::<FrameTree>()
            .insert_resource(FrameSize::default())
            .insert_resource(FrameSizePinned(false))
            .add_message::<bevy::window::WindowResized>()
            .add_systems(Startup, spawn_default_screen)
            .add_systems(
                Update,
                (sync_frame_size_to_window, lay_out_frame, place_panels).chain(),
            );
        let window = primary_window(&mut app, width, height);
        (app, window)
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
        assert_eq!(areas[&transcript.index_u32()].rect, CellRect::new(0, 0, 64, 25));
        // 右列：顶部留白 + 补全(0) + 输入(固定 4 行) + 状态(2 行) 贴底。
        assert_eq!(areas[&input.index_u32()].rect, CellRect::new(64, 21, 32, 4));
        assert_eq!(areas[&status.index_u32()].rect, CellRect::new(0, 26, 96, 2));
        // 精确中心来自树的 TileRect（不是格子取整后的矩形）。
        // 精确中心来自树的 TileRect：转写占上半部分（25 行），所以 y 中心不再是 0。
        let center = areas[&transcript.index_u32()].center.expect("树给精确中心");
        assert!((center.x - -16.0 * CELL.x).abs() < 1e-3);
        assert!((center.y - 1.5 * CELL.y).abs() < 1e-3, "y = {}", center.y);
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
        assert_eq!(areas[&status.index_u32()].rect, CellRect::new(0, 26, 96, 2));
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
        assert_eq!(areas.get(transcript).expect("转写区域").rect, CellRect::new(0, 0, 64, 25));
        assert_eq!(areas.get(status).expect("状态区域").rect, CellRect::new(0, 26, 96, 2));
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
                assert!((height - 25.0 * CELL.y).abs() < 1e-3, "height = {height}");
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
        let want_y = 1.5 * CELL.y * UNITS_TO_WORLD;
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

    // ---- 窗口尺寸 -> 格子数（REMAINING #3） ----

    /// 窗口逻辑像素 / `CELL`，只算完整格子；不足一格钳到 1。
    #[test]
    fn window_pixels_map_to_cells_by_cell_size() {
        assert_eq!(CELL, Vec2::new(7.8, 18.0));
        // 1280 / 7.8 = 164.1…，800 / 18 = 44.4…
        assert_eq!(
            FrameSize::from_window_size(Vec2::new(1280.0, 800.0)),
            FrameSize { cols: 164, rows: 44 }
        );
        // 800 / 7.8 = 102.5…，600 / 18 = 33.3…
        assert_eq!(
            FrameSize::from_window_size(Vec2::new(800.0, 600.0)),
            FrameSize { cols: 102, rows: 33 }
        );
        assert_eq!(
            FrameSize::from_window_size(Vec2::new(7.7, 17.9)),
            FrameSize { cols: 1, rows: 1 },
            "不足一格也算 1，不然布局拿到 0 列/行"
        );
        assert_eq!(
            FrameSize::from_window_size(Vec2::ZERO),
            FrameSize { cols: 1, rows: 1 }
        );
        assert_eq!(
            FrameSize::from_window_size(Vec2::new(-10.0, f32::NAN)),
            FrameSize { cols: 1, rows: 1 }
        );
    }

    /// 坏 `RF_FRAME` 不再静默：解析结果保留原始值并给出带期望格式的警告，
    /// 但尺寸仍不钉住（窗口接管）——除日志外与旧行为一致。
    #[test]
    fn a_bad_rf_frame_value_is_reported_instead_of_silently_following_the_window() {
        let bad = FrameEnv::parse(Some("96X28x"));
        assert_eq!(
            bad,
            FrameEnv::Invalid("96X28x".to_string()),
            "坏值必须保留原始文本"
        );
        assert_eq!(bad.size(), None, "坏值不钉住尺寸");
        let warning = bad.warning().expect("坏值必须产生警告");
        assert!(warning.contains("96X28x"), "警告要带原始值：{warning}");
        assert!(warning.contains("COLSxROWS"), "警告要写期望格式：{warning}");

        // 环境变量存在但是空串：同样算坏值，同样要警告。
        assert_eq!(FrameEnv::parse(Some("")), FrameEnv::Invalid(String::new()));
        assert!(FrameEnv::parse(Some("")).warning().is_some());

        // 好值 / 没设不警告。
        assert_eq!(
            FrameEnv::parse(Some("96x28")),
            FrameEnv::Pinned(FrameSize { cols: 96, rows: 28 })
        );
        assert_eq!(FrameEnv::parse(Some("96x28")).warning(), None);
        assert_eq!(FrameEnv::parse(None), FrameEnv::Unset);
        assert_eq!(FrameEnv::parse(None).warning(), None);
    }

    /// `RF_FRAME` 生效时窗口尺寸不覆盖 `FrameSize`（截图/测试确定性）。
    #[test]
    fn an_rf_frame_override_is_not_overwritten_by_the_window() {
        // `FramePlugin` 用 `env_override().is_some()` 决定 `FrameSizePinned`；
        // 这里不改进程环境变量（并行测试共享环境），直接走同一条分支。
        let pinned_size = FrameSize::from_env_text(Some("96x28")).expect("RF_FRAME 可解析");
        assert_eq!(pinned_size, FrameSize { cols: 96, rows: 28 });
        assert_eq!(FrameSize::from_env_text(Some("nope")), None, "坏值 = 不覆盖");
        assert_eq!(FrameSize::from_env_text(None), None, "没设 = 不覆盖");

        let mut app = App::new();
        app.insert_resource(pinned_size)
            .insert_resource(FrameSizePinned(true))
            .add_systems(Update, sync_frame_size_to_window);
        let window = primary_window(&mut app, 1280, 800);
        app.update();
        assert_eq!(
            *app.world().resource::<FrameSize>(),
            pinned_size,
            "窗口初值不能覆盖 RF_FRAME"
        );
        app.world_mut()
            .get_mut::<Window>(window)
            .expect("主窗口")
            .resolution = WindowResolution::new(800, 600);
        app.update();
        assert_eq!(
            *app.world().resource::<FrameSize>(),
            pinned_size,
            "拖窗口也不能覆盖 RF_FRAME"
        );
    }

    /// 只装同步系统的 App：启动时按窗口算，resize 后跟着变小。
    #[test]
    fn the_frame_size_follows_the_primary_window() {
        let mut app = App::new();
        app.insert_resource(FrameSize::default())
            .insert_resource(FrameSizePinned(false))
            .add_systems(Update, sync_frame_size_to_window);
        let window = primary_window(&mut app, 1280, 800);
        app.update();
        assert_eq!(
            *app.world().resource::<FrameSize>(),
            FrameSize { cols: 164, rows: 44 }
        );

        app.world_mut()
            .get_mut::<Window>(window)
            .expect("主窗口")
            .resolution = WindowResolution::new(800, 600);
        app.update();
        assert_eq!(
            *app.world().resource::<FrameSize>(),
            FrameSize { cols: 102, rows: 33 }
        );
    }

    /// 格子数来自**逻辑**尺寸：2× DPI 的 2560×1600 物理窗口还是 164×44。
    #[test]
    fn cells_come_from_the_logical_size_not_the_physical_size() {
        let mut app = App::new();
        app.insert_resource(FrameSize::default())
            .insert_resource(FrameSizePinned(false))
            .add_systems(Update, sync_frame_size_to_window);
        let mut resolution = WindowResolution::new(2560, 1600);
        resolution.set_scale_factor(2.0);
        app.world_mut().spawn((
            Window {
                resolution,
                ..default()
            },
            PrimaryWindow,
        ));
        app.update();
        assert_eq!(
            *app.world().resource::<FrameSize>(),
            FrameSize { cols: 164, rows: 44 },
            "逻辑尺寸 1280×800；不能用 physical 把格子数翻倍"
        );
    }

    /// 插件接线：`FramePlugin` 自己就把窗口同步系统挂在布局链最前面。
    #[test]
    fn frame_plugin_follows_the_primary_window() {
        if crate::niri::niri_layout() {
            return;
        }
        if FrameSize::env_override().is_some() {
            return; // RF_FRAME 生效时窗口不接管（上面有专门的覆盖测试）。
        }
        let mut app = App::new();
        // `FramePlugin` 只负责布局；面板的行槽/根表平时由 `PanelsPlugin` 建。
        app.init_resource::<PanelRoots>()
            .init_resource::<PanelRows>()
            .add_plugins(FramePlugin);
        primary_window(&mut app, 1280, 800);
        app.update();
        assert_eq!(
            *app.world().resource::<FrameSize>(),
            FrameSize { cols: 164, rows: 44 },
            "FramePlugin 的 Update 链要先同步窗口尺寸，再铺布局"
        );
    }

    /// 窗口拖小：`FrameSize`、`lay_out_frame` 的面板 rect、`place_panels` 的
    /// `Node` 尺寸都跟着变（不是只把整块 3D 画面缩放）。
    #[test]
    fn resizing_the_window_relayouts_the_panels() {
        if crate::niri::niri_layout() {
            return; // Niri 模式下默认屏幕不 spawn（见 `spawn_default_screen`）。
        }
        let (mut app, window) = windowed_frame_app(1280, 800);
        app.update();

        assert_eq!(
            *app.world().resource::<FrameSize>(),
            FrameSize { cols: 164, rows: 44 },
            "启动时 1280×800 / CELL"
        );
        let transcript = app
            .world()
            .resource::<PanelRoots>()
            .entity(PanelId::Transcript)
            .expect("转写面板");
        let before = app
            .world()
            .resource::<PanelAreas>()
            .get(transcript)
            .expect("转写区域");
        assert_eq!(before.rect, CellRect::new(0, 0, 109, 41));
        assert!(
            (node_width(&app, transcript) - 2.0 / 3.0 * 164.0 * CELL.x).abs() < 1e-3,
            "Node 宽度 = 树的精确像素"
        );

        resize_window(&mut app, window, 800, 600);
        app.update();

        assert_eq!(
            *app.world().resource::<FrameSize>(),
            FrameSize { cols: 102, rows: 33 },
            "窗口变小后格子数跟着变小"
        );
        let after = app
            .world()
            .resource::<PanelAreas>()
            .get(transcript)
            .expect("转写区域");
        assert_eq!(after.rect, CellRect::new(0, 0, 68, 30));
        assert!(
            after.rect.w < before.rect.w && after.rect.h < before.rect.h,
            "面板 rect 必须跟着变小：{before:?} -> {after:?}"
        );
        assert!(
            (node_width(&app, transcript) - 68.0 * CELL.x).abs() < 1e-3,
            "Node 宽度跟着树重算"
        );
    }

    /// 尺寸没变时同步系统不写资源（`Changed<FrameSize>` 不触发）；变了只写一次。
    #[test]
    fn an_unchanged_window_does_not_rewrite_the_frame_size() {
        #[derive(Resource, Default)]
        struct FrameSizeWrites(u32);

        fn count_writes(size: Res<FrameSize>, mut writes: ResMut<FrameSizeWrites>) {
            if size.is_changed() {
                writes.0 += 1;
            }
        }

        let mut app = App::new();
        app.insert_resource(FrameSize { cols: 164, rows: 44 })
            .insert_resource(FrameSizePinned(false))
            .init_resource::<FrameSizeWrites>()
            .add_systems(Update, (sync_frame_size_to_window, count_writes).chain());
        let window = primary_window(&mut app, 1280, 800);
        app.update(); // 基线：第一次看到资源不算"重复写"
        app.world_mut().resource_mut::<FrameSizeWrites>().0 = 0;

        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<FrameSizeWrites>().0,
            0,
            "尺寸没变时不能写资源（否则每帧触发重排）"
        );

        app.world_mut()
            .get_mut::<Window>(window)
            .expect("主窗口")
            .resolution = WindowResolution::new(800, 600);
        app.update();
        assert_eq!(
            app.world().resource::<FrameSizeWrites>().0,
            1,
            "尺寸变了才写一次"
        );
        app.update();
        assert_eq!(
            app.world().resource::<FrameSizeWrites>().0,
            1,
            "之后尺寸没再变，不重复写"
        );
    }

    /// 极小窗口：不 panic，cols/rows ≥ 1，面板 rect 不超过帧。
    #[test]
    fn a_tiny_window_lays_out_without_panicking() {
        if crate::niri::niri_layout() {
            return;
        }
        let (mut app, window) = windowed_frame_app(100, 80);
        app.update();

        let size = *app.world().resource::<FrameSize>();
        assert_eq!(size, FrameSize { cols: 12, rows: 4 });
        assert!(size.cols >= 1 && size.rows >= 1);
        let areas = app.world().resource::<PanelAreas>();
        assert!(!areas.0.is_empty(), "极小窗口也要有面板区域");
        for area in areas.0.values() {
            assert!(area.size.x.is_finite() && area.size.y.is_finite());
            assert!(
                area.rect.right() <= size.cols && area.rect.bottom() <= size.rows,
                "rect 出界：{area:?}（帧 {size:?}）"
            );
        }

        // 1×1 是最坏情况：固定行（提示/状态/输入）要整体缩水，不能 panic。
        resize_window(&mut app, window, 1, 1);
        app.update();
        let size = *app.world().resource::<FrameSize>();
        assert_eq!(size, FrameSize { cols: 1, rows: 1 });
        for area in app.world().resource::<PanelAreas>().0.values() {
            assert!(
                area.rect.right() <= 1 && area.rect.bottom() <= 1,
                "1×1 帧里 rect 出界：{area:?}"
            );
        }
    }

    /// 工作区条带间距读的是同一个 `FrameSize::rows`，改尺寸后比例一致
    /// （只读核对，不改 workspaces.rs）。
    #[test]
    fn workspace_strip_spacing_uses_the_same_frame_size() {
        use crate::workspaces::{WORKSPACE_GAP, workspace_step, workspace_strip_height};

        for size in [
            FrameSize::default(),
            FrameSize { cols: 164, rows: 44 },
            FrameSize { cols: 12, rows: 4 },
        ] {
            let strip = f32::from(size.rows) * CELL.y * UNITS_TO_WORLD;
            assert!(
                (workspace_strip_height(size) - strip).abs() < 1e-6,
                "条带高度 = rows × CELL.y × UNITS_TO_WORLD"
            );
            assert!(
                (workspace_step(size) - strip - WORKSPACE_GAP).abs() < 1e-6,
                "步长 = 条带高度 + 固定间距"
            );
        }
        assert!(
            workspace_step(FrameSize { cols: 164, rows: 44 })
                > workspace_step(FrameSize::default()),
            "窗口变高后工作区间距跟着变大（同一个 FrameSize）"
        );
    }

    fn node_width(app: &App, entity: Entity) -> f32 {
        match app.world().get::<Node>(entity).expect("面板 Node").width {
            bevy_ui::Val::Px(width) => width,
            other => panic!("Node 宽度应当是像素：{other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // F1：面板 chrome 走 feathers token（底/边框/圆角）
    // -----------------------------------------------------------------------

    /// 一份把 feathers 暗色主题里两个 surface 换成测试色的 `UiTheme`。
    fn theme_with_surfaces(bg: Color, border: Color) -> UiTheme {
        let mut theme = crate::theme::dark_ui_theme();
        theme
            .0
            .semantic_base
            .insert(bevy_feathers::tokens::semantic::SURFACE_PANE_BODY, bg);
        theme
            .0
            .semantic_base
            .insert(bevy_feathers::tokens::semantic::SURFACE_SUBPANE_HEADER, border);
        theme
    }

    /// 只装 `FramePlugin`（默认屏幕 + chrome 解析），不建窗口/UI 插件。
    fn chrome_app(theme: Option<UiTheme>) -> App {
        let mut app = App::new();
        app.init_resource::<PanelRoots>()
            .init_resource::<PanelRows>()
            .add_plugins(FramePlugin);
        if let Some(theme) = theme {
            app.insert_resource(theme);
        }
        app
    }

    /// 面板根的底色/边框 = `UiTheme.color(token)`，不是硬编码值；
    /// 根上挂着 feathers 的 `ThemeBackgroundColor`/`ThemeBorderColor`。
    #[test]
    fn panel_chrome_comes_from_the_ui_theme_tokens() {
        if std::env::var("RF_NIRI_LAYOUT").is_ok() {
            return; // Niri 模式接管时默认屏幕不 spawn。
        }
        let bg = Color::srgb(0.11, 0.22, 0.33);
        let border = Color::srgb(0.44, 0.55, 0.66);
        let mut app = chrome_app(Some(theme_with_surfaces(bg, border)));
        app.update();

        let theme = app.world().resource::<UiTheme>();
        // 测试色确实落到了这两个 token 上（防止断言的是同一份错误值）。
        assert_eq!(theme.color(&crate::theme::PANEL_BG), bg);
        assert_eq!(theme.color(&crate::theme::PANEL_BORDER), border);

        let roots = app.world().resource::<PanelRoots>();
        let transcript = roots.entity(PanelId::Transcript).expect("转写面板");
        let input = roots.entity(PanelId::Input).expect("输入面板");
        let status = roots.entity(PanelId::Status).expect("状态面板");
        for entity in [transcript, input, status] {
            let background = app
                .world()
                .get::<BackgroundColor>(entity)
                .expect("面板根有 BackgroundColor");
            let token = app
                .world()
                .get::<ThemeBackgroundColor>(entity)
                .expect("token 版面板要挂 ThemeBackgroundColor");
            assert_eq!(
                background.0,
                theme.color(&token.0),
                "底色必须等于 UiTheme.color({})",
                token.0
            );
            let border_color = app
                .world()
                .get::<BorderColor>(entity)
                .expect("面板根有 BorderColor");
            let border_token = app
                .world()
                .get::<ThemeBorderColor>(entity)
                .expect("token 版面板要挂 ThemeBorderColor");
            assert_eq!(
                border_color.top,
                theme.color(&border_token.0),
                "边框必须等于 UiTheme.color({})",
                border_token.0
            );
            // 转写/输入/状态各用自己那档 token，不是同一个写死的颜色。
            assert_eq!(
                token.0,
                match entity {
                    e if e == transcript => crate::theme::PANEL_BG,
                    e if e == input => crate::theme::INPUT_BG,
                    _ => crate::theme::BAR_BG,
                }
            );
        }
        // 圆角来自 feathers 的 pane/group 常量（集中成 theme::PANEL_RADIUS）。
        let node = app.world().get::<Node>(transcript).expect("Node");
        assert_eq!(
            node.border_radius,
            BorderRadius::all(px(crate::theme::PANEL_RADIUS))
        );
    }

    /// 没有 `UiTheme` 资源（headless / 最小 App）：退到 feathers 暗色主题，
    /// 不 panic，解析出来的就是 `dark_theme` 里的颜色。
    #[test]
    fn panel_chrome_falls_back_to_the_dark_theme_without_a_ui_theme() {
        if std::env::var("RF_NIRI_LAYOUT").is_ok() {
            return;
        }
        let mut app = chrome_app(None);
        app.update();
        assert!(
            app.world().get_resource::<UiTheme>().is_none(),
            "这条路径故意不插 UiTheme"
        );

        let fallback = crate::theme::default_ui_theme();
        let roots = app.world().resource::<PanelRoots>();
        let mut seen = 0;
        for entity in roots.0.values() {
            let Some(ThemeBackgroundColor(token)) = app.world().get::<ThemeBackgroundColor>(*entity)
            else {
                continue;
            };
            let background = app
                .world()
                .get::<BackgroundColor>(*entity)
                .expect("面板根有 BackgroundColor");
            assert_eq!(background.0, fallback.color(token));
            seen += 1;
        }
        assert!(seen >= 4, "默认屏幕的面板应当都走 token（实际 {seen} 块）");
    }

    /// 换主题后同一帧内重刷：`UiTheme` 是 chrome 的唯一颜色来源。
    #[test]
    fn changing_the_ui_theme_repaints_the_panel_chrome() {
        if std::env::var("RF_NIRI_LAYOUT").is_ok() {
            return;
        }
        let first = Color::srgb(0.10, 0.20, 0.30);
        let second = Color::srgb(0.60, 0.50, 0.40);
        let mut app = chrome_app(Some(theme_with_surfaces(first, first)));
        app.update();

        let roots = app.world().resource::<PanelRoots>();
        let transcript = roots.entity(PanelId::Transcript).expect("转写面板");
        let background = |app: &App| {
            app.world()
                .get::<BackgroundColor>(transcript)
                .expect("BackgroundColor")
                .0
        };
        assert_eq!(background(&app), first);

        app.world_mut()
            .resource_mut::<UiTheme>()
            .0
            .semantic_base
            .insert(bevy_feathers::tokens::semantic::SURFACE_PANE_BODY, second);
        app.update();
        assert_eq!(background(&app), second, "换主题要重新解析 token");
    }

    /// 换皮不动几何：两种主题下 `PanelAreas` 的 rect/尺寸、`Node` 的
    /// 宽高/padding/border 完全一致（裁剪、行点击、滚动都靠这些）。
    #[test]
    fn feathers_chrome_does_not_change_panel_geometry() {
        if std::env::var("RF_NIRI_LAYOUT").is_ok() {
            return;
        }
        let mut plain = chrome_app(None);
        plain.update();
        let mut themed = chrome_app(Some(theme_with_surfaces(
            Color::srgb(0.11, 0.22, 0.33),
            Color::srgb(0.44, 0.55, 0.66),
        )));
        themed.update();

        let ids = [
            PanelId::Transcript,
            PanelId::Input,
            PanelId::Status,
            PanelId::Complete,
            PanelId::Hints,
            PanelId::Confirm,
        ];
        for id in ids {
            let a = plain
                .world()
                .resource::<PanelRoots>()
                .entity(id)
                .unwrap_or_else(|| panic!("{id:?} 面板"));
            let b = themed
                .world()
                .resource::<PanelRoots>()
                .entity(id)
                .unwrap_or_else(|| panic!("{id:?} 面板"));
            let area_a = plain.world().resource::<PanelAreas>().get(a).expect("区域");
            let area_b = themed.world().resource::<PanelAreas>().get(b).expect("区域");
            assert_eq!(area_a, area_b, "{id:?} 的面板区域不能因换肤而变");

            let node_a = plain.world().get::<Node>(a).expect("Node");
            let node_b = themed.world().get::<Node>(b).expect("Node");
            assert_eq!(node_a.width, node_b.width, "{id:?} 宽度");
            assert_eq!(node_a.height, node_b.height, "{id:?} 高度");
            assert_eq!(node_a.padding, node_b.padding, "{id:?} padding");
            assert_eq!(node_a.border, node_b.border, "{id:?} border");
            // inset 仍然来自该面板自己的 `PanelInset`（文字区/裁剪的输入）。
            let inset = themed
                .world()
                .get::<FramePanel>(b)
                .expect("FramePanel")
                .inset;
            assert_eq!(node_a.padding, UiRect::all(px(f32::from(inset.pad))));
        }
    }
}
