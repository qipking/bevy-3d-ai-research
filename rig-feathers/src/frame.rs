//! 帧布局：`plugins/rig-tui` 的 `panel.rs` 在 3D 里的对应物。
//!
//! rig-tui 把终端切成一块块面板：一个 `TuiPanel` 带一个 `Placement`，视图按
//! spawn 顺序让每块面板从"剩下的地方"里取一块（上/下/左/右、输入框下、浮层、
//! 剩余区、另一块面板的某一列），界面就这么拼出来。终端只有格子，所以整套
//! 布局是整数行列。
//!
//! 3D 里模型不变，只是格子换成像素：
//!
//! ```text
//! FrameSize(96×28 格) ──lay_out（纯函数，照抄 rig 的算法）──> CellRect
//!                                                             │
//!   CellRect × CELL(7.8×18) ──> Node 尺寸（bevy_ui 排版）      │
//!   CellRect ──────────────────> WorldPanel.anchor（3D 位置）  │
//!   CellRect − 面板装饰 ───────> 文字列数 / 行数（look 换行、pane 高度）
//! ```
//!
//! 与 rig-tui 的差别只有两处，都是 3D 需要的：
//!
//! * 面板有装饰（内边距 + 边框），所以文字区是面板区域减掉装饰；
//! * 面板分层（[`Placement::stacking`]），层同时决定 3D 深度和绘制顺序，
//!   浮层因此永远盖在底下的面板上（见 `feathers_in_3d::WorldLayer`）。

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_ui::{BorderRadius, Node, UiRect, px};
use feathers_in_3d::{MirrorRoot, WorldLayer, WorldPanel};

use crate::panels::{PanelId, PanelRows};

pub use crate::panels::PanelRoots;

/// 一个格子在 UI 像素里的尺寸：宽 = 13px 等宽字的前进量，高 = 一行文本。
pub const CELL: Vec2 = Vec2::new(7.8, crate::transcript::ROW_HEIGHT);

/// 面板默认装饰（内边距 + 边框）。
pub const PANEL_PADDING: f32 = 8.0;
pub const PANEL_BORDER: f32 = 2.0;

/// UI 像素 → 世界米的换算（与 rig-feathers 里各面板一致）。
pub const UNITS_TO_WORLD: f32 = 0.0018;

/// rig-tui `panel::MIN_TRANSCRIPT`：面板不能把转写挤到比这更小。
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

    /// 上面 `top` 行，下面剩下的（`Layout::vertical([Length, Min(0)])`）。
    fn split_top(&self, top: u16) -> (Self, Self) {
        let top = top.min(self.h);
        (
            Self::new(self.x, self.y, self.w, top),
            Self::new(self.x, self.y + top, self.w, self.h - top),
        )
    }

    /// 左边 `left` 列，右边剩下的（`Layout::horizontal([Length, Min(0)])`）。
    fn split_left(&self, left: u16) -> (Self, Self) {
        let left = left.min(self.w);
        (
            Self::new(self.x, self.y, left, self.h),
            Self::new(self.x + left, self.y, self.w - left, self.h),
        )
    }

    /// 第 `part` 列，共 `of` 等分列（`Layout::horizontal([Fill(1); of])`）。
    fn column(&self, part: u16, of: u16) -> Self {
        if of == 0 || part >= of || self.is_empty() {
            return Self::default();
        }
        let start = u32::from(self.w) * u32::from(part) / u32::from(of);
        let end = u32::from(self.w) * u32::from(part + 1) / u32::from(of);
        Self::new(
            self.x.saturating_add(start as u16),
            self.y,
            (end - start) as u16,
            self.h,
        )
    }

    /// 在 `self` 里居中放 `w × h`（`Rect::centered`）。
    fn centered(&self, w: u16, h: u16) -> Self {
        let w = w.min(self.w);
        let h = h.min(self.h);
        Self::new(
            self.x + (self.w - w) / 2,
            self.y + (self.h - h) / 2,
            w,
            h,
        )
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
// 尺寸约束与放置
// ---------------------------------------------------------------------------

/// rig-tui 用到的那部分 `ratatui::layout::Constraint`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extent {
    /// 固定几格（`Constraint::Length`）。
    Cells(u16),
    /// 占这条边的百分之几（`Constraint::Percentage`）。
    Percent(u16),
    /// 剩下的全部（`Constraint::Fill(1)` / `Constraint::Min(0)`）。
    Rest,
}

impl Extent {
    /// 在 `total` 格里的实际格数。
    pub fn resolve(self, total: u16) -> u16 {
        match self {
            Self::Cells(cells) => cells.min(total),
            Self::Percent(percent) => {
                (u32::from(total) * u32::from(percent.min(100)) / 100) as u16
            }
            Self::Rest => total,
        }
    }
}

/// 面板放在哪（与 rig-tui 的 `Placement` 一一对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// 剩余区的上边。
    Top(Extent),
    /// 剩余区的下边。
    Bottom(Extent),
    /// 剩余区的左边。
    Left(Extent),
    /// 剩余区的右边。
    Right(Extent),
    /// 输入框下面（整个屏幕的最下面，几块按顺序往下堆）。
    BelowInput(Extent),
    /// 盖在整屏中央的盒子。
    Over { width: Extent, height: Extent },
    /// 别人剩下的地方（转写）；多块平分。
    Rest,
    /// 另一块面板（或 picker）的第 `part` 列，共 `of` 列。
    Within { panel: Entity, part: u16, of: u16 },
}

impl Placement {
    /// 绘制层，对应 rig-tui `render.rs` 的 `stacking`：0 是屏幕本身，
    /// 1 是 `Over` 盒子，2 是普通 `Within`，3 是 picker 里的 `Within`。
    pub fn stacking(&self, picker: Option<Entity>) -> u16 {
        match self {
            Self::Over { .. } => 1,
            Self::Within { panel, .. } if Some(*panel) == picker => 3,
            Self::Within { .. } => 2,
            _ => 0,
        }
    }
}

/// 一块参与布局的面板。
#[derive(Clone, Copy, Debug)]
pub struct ToLayOut {
    pub entity: Entity,
    pub placement: Placement,
    /// 布局顺序，小的先占位（对应 rig-tui 的 spawn 顺序）。
    pub order: u32,
    /// 面板按内容收成几行（编辑器、提示条）；`None` 用 placement 要的尺寸。
    pub fits: Option<u16>,
    /// 面板装饰（文字区 = 区域减掉它）。
    pub inset: PanelInset,
}

/// 每帧算出来的面板区域。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PanelArea {
    /// 面板在帧里的格子区域。
    pub rect: CellRect,
    /// 面板的 UI 像素尺寸（格子 × [`CELL`]）。
    pub size: Vec2,
    /// 文字能放多少列 / 行。
    pub cols: u16,
    pub rows: u16,
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
        }
    }

    /// 没有地方（面板这一帧不出现）。
    pub fn is_empty(&self) -> bool {
        self.rect.is_empty() || self.cols == 0 || self.rows == 0
    }
}

/// 把面板铺到 `screen` 上，返回每块面板的区域。
///
/// 顺序照抄 rig-tui `panel::lay_out`：
/// 1. 输入框下面的先堆在最底下，给"按内容收行"的面板和最小转写留够；
/// 2. 上/下/左/右各取自己的一边；
/// 3. `Over` 盒子居中盖在整屏上；
/// 4. `Rest` 平分剩下的；
/// 5. `Within` 取宿主面板（或 `boxes` 里的盒子，比如 picker）的一列。
pub fn lay_out(
    panels: impl IntoIterator<Item = ToLayOut>,
    screen: CellRect,
    boxes: &[(Entity, CellRect)],
) -> HashMap<Entity, PanelArea> {
    let mut panels: Vec<ToLayOut> = panels.into_iter().collect();
    panels.sort_by_key(|panel| panel.order);

    let fitted: u16 = panels
        .iter()
        .filter(|panel| !matches!(panel.placement, Placement::BelowInput(_)))
        .filter_map(|panel| panel.fits)
        .sum();
    let asked: Vec<u16> = panels
        .iter()
        .filter_map(|panel| match panel.placement {
            Placement::BelowInput(size) => {
                Some(panel.fits.unwrap_or_else(|| rows_of(size, screen.h)))
            }
            _ => None,
        })
        .collect();
    let room = screen
        .h
        .saturating_sub(fitted)
        .saturating_sub(MIN_TRANSCRIPT.1);
    let below_rows = below_rows(asked, room);
    let total: u16 = below_rows.iter().sum();

    let (mut main, mut below) = screen.split_top(screen.h.saturating_sub(total));
    let mut below_rows = below_rows.into_iter();
    let mut rest = Vec::new();
    let mut within = Vec::new();
    let mut areas: Vec<(Entity, CellRect)> = boxes.to_vec();
    let mut out: HashMap<Entity, PanelArea> = HashMap::new();

    for panel in &panels {
        let area = match panel.placement {
            Placement::Rest => {
                rest.push((panel.entity, panel.inset));
                continue;
            }
            Placement::Within { panel: host, part, of } => {
                within.push((panel.entity, host, part, of, panel.inset));
                continue;
            }
            Placement::Over { width, height } => {
                screen.centered(width.resolve(screen.w), height.resolve(screen.h))
            }
            Placement::BelowInput(_) => {
                let rows = below_rows.next().unwrap_or(0);
                let (taken, left) = below.split_top(rows);
                below = left;
                if rows == 0 { CellRect::default() } else { taken }
            }
            Placement::Top(size) => {
                let rows = panel.fits.unwrap_or_else(|| size.resolve(main.h));
                let (taken, left) = main.split_top(rows);
                take(&mut main, taken, left)
            }
            Placement::Bottom(size) => {
                let rows = panel.fits.unwrap_or_else(|| size.resolve(main.h));
                let (left, taken) = main.split_top(main.h.saturating_sub(rows));
                take(&mut main, taken, left)
            }
            Placement::Left(size) => {
                let cols = size.resolve(main.w);
                let (taken, right) = main.split_left(cols);
                take(&mut main, taken, right)
            }
            Placement::Right(size) => {
                let cols = size.resolve(main.w);
                let (left, taken) = main.split_left(main.w.saturating_sub(cols));
                take(&mut main, taken, left)
            }
        };
        areas.push((panel.entity, area));
        out.insert(panel.entity, PanelArea::new(area, panel.inset));
    }
    for (entity, inset) in rest {
        areas.push((entity, main));
        out.insert(entity, PanelArea::new(main, inset));
    }
    for (entity, host, part, of, inset) in within {
        let host_area = areas
            .iter()
            .find(|(candidate, _)| *candidate == host)
            .map(|(_, area)| *area)
            .unwrap_or_default();
        let area = host_area.column(part, of);
        areas.push((entity, area));
        out.insert(entity, PanelArea::new(area, inset));
    }
    out
}

/// 一块面板能拿的尺寸：`fits` 优先，否则 placement 要的。
fn take(main: &mut CellRect, panel: CellRect, rest: CellRect) -> CellRect {
    if rest.w < MIN_TRANSCRIPT.0 || rest.h < MIN_TRANSCRIPT.1 {
        return CellRect::default();
    }
    *main = rest;
    panel
}

/// 一条边为 `size` 的面板要高多少行（`rows_of`）。
fn rows_of(size: Extent, height: u16) -> u16 {
    size.resolve(height)
}

/// 输入框下面每块面板分几行；放不下的整块跳过（`below_rows`）。
fn below_rows(asked: impl IntoIterator<Item = u16>, mut room: u16) -> Vec<u16> {
    asked
        .into_iter()
        .map(|rows| match room.checked_sub(rows) {
            Some(left) => {
                room = left;
                rows
            }
            None => 0,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 组件与系统
// ---------------------------------------------------------------------------

/// 一块参与帧布局的面板（`bevy_ui` 子树根 + 3D 面板）。
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct FramePanel {
    pub placement: Placement,
    /// 布局顺序，小的先占位（rig-tui 用 spawn 顺序）。
    pub order: u32,
    /// 面板装饰：内边距 + 边框。
    pub inset: PanelInset,
}

impl FramePanel {
    pub fn new(placement: Placement) -> Self {
        Self {
            placement,
            order: 0,
            inset: PanelInset::DEFAULT,
        }
    }

    pub fn order(mut self, order: u32) -> Self {
        self.order = order;
        self
    }

    pub fn inset(mut self, inset: PanelInset) -> Self {
        self.inset = inset;
        self
    }
}

/// 面板按内容收成几行（编辑器、提示条），留空则用 placement 的尺寸。
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FitRows(pub u16);

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

/// 算每块面板这一帧的格子区域。
pub fn lay_out_frame(
    mut areas: ResMut<PanelAreas>,
    size: Res<FrameSize>,
    panels: Query<(Entity, &FramePanel, Option<&FitRows>)>,
) {
    let to_lay_out = panels
        .iter()
        .map(|(entity, panel, fits)| ToLayOut {
            entity,
            placement: panel.placement,
            order: panel.order,
            fits: fits.map(|fits| fits.0),
            inset: panel.inset,
        });
    let next = lay_out(to_lay_out, size.screen(), &[]);
    if next != areas.0 {
        areas.0 = next;
    }
}

/// 把格子区域落到每块面板的 `Node` 尺寸与 `WorldPanel` 锚点上。
pub fn place_panels(
    mut commands: Commands,
    areas: Res<PanelAreas>,
    size: Res<FrameSize>,
    geometry: Res<FrameGeometry>,
    mut panels: Query<(
        Entity,
        &FramePanel,
        &mut Node,
        &mut WorldPanel,
        Option<&mut WorldLayer>,
    )>,
) {
    if areas.0.is_empty() {
        return;
    }
    let frame_px = Vec2::new(f32::from(size.cols), f32::from(size.rows)) * CELL;
    for (entity, panel, mut node, mut world, layer) in panels.iter_mut() {
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
        // 面板中心在帧里的像素坐标（原点在帧中心，UI 的 y 朝下，世界的 y 朝上）。
        let center_px = Vec2::new(
            (f32::from(area.rect.x) + f32::from(area.rect.w) * 0.5) * CELL.x - frame_px.x * 0.5,
            frame_px.y * 0.5 - (f32::from(area.rect.y) + f32::from(area.rect.h) * 0.5) * CELL.y,
        );
        let want_layer = panel.placement.stacking(None);
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

/// 装默认屏幕（rig-tui `screen.rs`）：输入在最下面、状态行在它上面、
/// 转写占剩下的。顺序就是 rig 的 spawn 顺序，`order` 小的先占位。
fn spawn_default_screen(
    mut commands: Commands,
    mut roots: ResMut<PanelRoots>,
    mut rows: ResMut<PanelRows>,
    mut screen: ResMut<DefaultScreen>,
) {
    // 新窗口逻辑（Niri）接管时，默认屏幕由 `niri::spawn_niri_screen` 生成。
    if std::env::var("RF_NIRI_LAYOUT").is_ok() {
        return;
    }
    // 输入：4 行 —— 提示行 + 键位提示行 + 上下留白（rig 是 3 行，我们没有行间留白）。
    let input = spawn_panel(
        &mut commands,
        PanelId::Input,
        FramePanel::new(Placement::Bottom(Extent::Cells(4))).order(0),
        Look::new(
            Color::srgb(0.05, 0.06, 0.09),
            Color::srgb(0.30, 0.55, 0.85),
            8.0,
        ),
    );
    // 状态行：细条，2 行里居中放 1 行字。
    let status = spawn_panel(
        &mut commands,
        PanelId::Status,
        FramePanel::new(Placement::Bottom(Extent::Cells(2)))
            .order(1)
            .inset(PanelInset::THIN),
        Look {
            bg: Color::srgb(0.035, 0.045, 0.065),
            border: Color::srgb(0.16, 0.22, 0.32),
            radius: 6.0,
            justify: JustifyContent::Center,
        },
    );
    // 转写：剩下的全部（rig 的 `TranscriptPane` 默认 `Placement::Rest`）。
    let transcript = spawn_panel(
        &mut commands,
        PanelId::Transcript,
        FramePanel::new(Placement::Rest).order(2),
        Look::new(
            Color::srgb(0.045, 0.055, 0.08),
            Color::srgb(0.25, 0.62, 0.95),
            14.0,
        ),
    );
    roots.0.insert(PanelId::Input, input);
    roots.0.insert(PanelId::Status, status);
    roots.0.insert(PanelId::Transcript, transcript);
    for id in [PanelId::Input, PanelId::Status, PanelId::Transcript] {
        rows.0.entry(id).or_default();
    }
    screen.transcript = Some(transcript);
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
            WorldLayer(0),
            MirrorRoot,
        ))
        .id()
}

/// 帧布局插件：默认屏幕 + 每帧布局。
pub struct FramePlugin;

impl Plugin for FramePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FrameSize::from_env())
            .init_resource::<PanelAreas>()
            .init_resource::<FrameGeometry>()
            .init_resource::<DefaultScreen>()
            .add_systems(Startup, spawn_default_screen)
            .add_systems(
                Update,
                (lay_out_frame, place_panels)
                    .chain()
                    .in_set(crate::FeathersSystems::Layout),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::entity::Entity;

    fn panel(order: u32, placement: Placement) -> ToLayOut {
        ToLayOut {
            entity: Entity::from_raw_u32(order).expect("entity"),
            placement,
            order,
            fits: None,
            inset: PanelInset::DEFAULT,
        }
    }

    fn areas_of(screen: (u16, u16), panels: Vec<ToLayOut>) -> HashMap<u32, CellRect> {
        let screen = CellRect::new(0, 0, screen.0, screen.1);
        lay_out(panels, screen, &[])
            .into_iter()
            .map(|(entity, area)| (entity.index_u32(), area.rect))
            .collect()
    }

    /// 默认屏幕：96×28 里，转写 0..24、状态行 24..26、输入 26..28。
    #[test]
    fn default_screen_stacks_bottom_up() {
        let areas = areas_of(
            (96, 28),
            vec![
                panel(0, Placement::Bottom(Extent::Cells(4))),
                panel(1, Placement::Bottom(Extent::Cells(2))),
                panel(2, Placement::Rest),
            ],
        );
        assert_eq!(areas[&0], CellRect::new(0, 24, 96, 4), "input at the bottom");
        assert_eq!(areas[&1], CellRect::new(0, 22, 96, 2), "status above the input");
        assert_eq!(areas[&2], CellRect::new(0, 0, 96, 22), "transcript takes the rest");
    }

    /// 面板不能把转写挤到小于 20×3：挤不下就这一帧没有地方。
    #[test]
    fn a_panel_that_squeezes_the_transcript_gets_nothing() {
        let areas = areas_of(
            (96, 10),
            vec![
                panel(0, Placement::Bottom(Extent::Cells(8))),
                panel(1, Placement::Rest),
            ],
        );
        assert_eq!(areas[&0], CellRect::default(), "8 rows leave 2 < MIN_TRANSCRIPT");
        assert_eq!(areas[&1], CellRect::new(0, 0, 96, 10));
    }

    /// 左右分栏：左 30 列，转写拿剩下的。
    #[test]
    fn side_panels_take_their_side() {
        let areas = areas_of(
            (96, 28),
            vec![
                panel(0, Placement::Left(Extent::Cells(30))),
                panel(1, Placement::Rest),
            ],
        );
        assert_eq!(areas[&0], CellRect::new(0, 0, 30, 28));
        assert_eq!(areas[&1], CellRect::new(30, 0, 66, 28));
    }

    /// 两个 `Rest` 面板平分剩下的地方。
    #[test]
    fn rest_panels_share() {
        let areas = areas_of(
            (96, 28),
            vec![panel(0, Placement::Rest), panel(1, Placement::Rest)],
        );
        assert_eq!(areas[&0], areas[&1]);
        assert_eq!(areas[&0], CellRect::new(0, 0, 96, 28));
    }

    /// `Over` 盒子盖在整屏中央，不占剩余区。
    #[test]
    fn over_is_centred_and_free() {
        let areas = areas_of(
            (96, 28),
            vec![
                panel(0, Placement::Over {
                    width: Extent::Percent(50),
                    height: Extent::Cells(10),
                }),
                panel(1, Placement::Rest),
            ],
        );
        assert_eq!(areas[&0], CellRect::new(24, 9, 48, 10));
        assert_eq!(areas[&1], CellRect::new(0, 0, 96, 28));
    }

    /// 输入框下面的行堆在最底下，转写在上面。
    #[test]
    fn below_input_rows_sit_at_the_bottom() {
        let areas = areas_of(
            (96, 28),
            vec![
                panel(0, Placement::BelowInput(Extent::Cells(2))),
                panel(1, Placement::Rest),
            ],
        );
        assert_eq!(areas[&0], CellRect::new(0, 26, 96, 2));
        assert_eq!(areas[&1], CellRect::new(0, 0, 96, 26));
    }

    /// `Within` 取宿主的一列。
    #[test]
    fn within_takes_a_column_of_its_host() {
        let host = Entity::from_raw_u32(7).expect("entity");
        let screen = CellRect::new(0, 0, 96, 28);
        let mut areas = lay_out(
            vec![
                panel(0, Placement::Rest),
                ToLayOut {
                    entity: Entity::from_raw_u32(1).expect("entity"),
                    placement: Placement::Within {
                        panel: host,
                        part: 1,
                        of: 2,
                    },
                    order: 1,
                    fits: None,
                    inset: PanelInset::DEFAULT,
                },
            ],
            screen,
            &[(host, CellRect::new(0, 0, 60, 20))],
        )
        .into_iter()
        .map(|(entity, area)| (entity.index_u32(), area.rect))
        .collect::<HashMap<_, _>>();
        assert_eq!(areas.remove(&1), Some(CellRect::new(30, 0, 30, 20)));
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

    /// 格子换算：一帧 96×28 的像素尺寸。
    #[test]
    fn frame_size_scales() {
        let size = FrameSize { cols: 96, rows: 28 };
        assert_eq!(size.screen(), CellRect::new(0, 0, 96, 28));
        assert_eq!(FrameSize::parse("120x40"), Some(FrameSize { cols: 120, rows: 40 }));
        assert_eq!(FrameSize::parse("nope"), None);
    }
}
