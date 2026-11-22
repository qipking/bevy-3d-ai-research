//! Niri 式滚动平铺（3D 卡片版）：列宽预设、列内竖分、`view_offset` 居中、一格一列。
//!
//! 语义照 `research/niri/niri-main`（GPL-3.0，**只学语义不复制代码**）：
//!
//! | Niri | 这里 |
//! |---|---|
//! | `preset_column_widths = [1/3, 1/2, 2/3]` | [`PresetSize`] 预设表，`[` `]` 循环 |
//! | 列 x = 前面列宽累加 + gap | [`Tiling::column_x`] |
//! | `view_offset` 让聚焦列居中（`CenterFocusedColumn::Always`） | [`Tiling::target_offset`] + 平滑动画 |
//! | 列里竖分窗口 | `Column::tiles`，高度均分 |
//! | 滚轮一格一列、150ms cooldown | 由输入层处理（`examples/spatial.rs`） |
//!
//! 每个 **agent = 一层（`Layer`）**：层有自己的一条可滚平铺；层沿 Y 轴排开，
//! 聚焦层以外的层可以隐藏（"进入 agent 空间"）。

use bevy::prelude::*;
use bevy_ui::{Node, px};

/// 列宽：视口宽度的比例，或者固定米数（niri 的 `PresetSize`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PresetSize {
    Proportion(f32),
    Fixed(f32),
}

impl PresetSize {
    /// 在 `viewport` 米里实际占多宽。
    pub fn resolve(self, viewport: f32) -> f32 {
        match self {
            Self::Proportion(ratio) => viewport * ratio.clamp(0.1, 1.0),
            Self::Fixed(meters) => meters.min(viewport),
        }
    }
}

/// 一列：宽度 + 竖着放几个窗口。
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub width: PresetSize,
    pub tiles: u16,
}

impl Column {
    pub fn new(width: PresetSize, tiles: u16) -> Self {
        Self {
            width,
            tiles: tiles.max(1),
        }
    }
}

/// 一个 agent 的一条平铺：列、聚焦列、滚动位移。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    pub columns: Vec<Column>,
    /// 聚焦的列。
    pub focus: usize,
    /// 当前滚动位移（米，正值 = 内容右移）。
    pub view_offset: f32,
    /// 聚焦列里的聚焦窗口。
    pub tile: usize,
}

/// 所有层 + 视口参数。
#[derive(Resource, Clone, Debug)]
pub struct Tiling {
    pub layers: Vec<Layer>,
    /// 可见工作区（米）。
    pub viewport: Vec2,
    /// 列之间、窗口之间的缝（米）。
    pub gap: Vec2,
    /// 列宽预设（`[` `]` 循环）。
    pub presets: Vec<PresetSize>,
    /// 第 0 层的轨道半径（米）：层的平面就摆在 z = -radius。
    pub radius0: f32,
    /// 每往外一层半径加多少（上层更远 → 在屏幕上更小、从上面露出来）。
    pub radius_step: f32,
    /// 第 0 层的高度（米）。
    pub height0: f32,
    /// 每往外一层抬高多少。
    pub height_step: f32,
    /// 环模式：内容铺在半径 `ring_radius` 的圆柱上、**首尾相连**（滚不到头，
    /// 最后一列后面接回第一列）。总览用它。
    pub ring: Option<f32>,
    /// 布局米 → 世界米的缩放（总览把整条带鱼屏放大贴到内壁上时用）。
    pub scale: f32,
    /// 带鱼屏绕成圆柱时**最小的半径**（米）：低于它文字就糊了。
    /// 内容短于 `2π × ring_min_radius` 时不绕成圈（不首尾相连），只贴一段弧。
    pub ring_min_radius: f32,
}

impl Default for Tiling {
    fn default() -> Self {
        Self {
            layers: Vec::new(),
            viewport: Vec2::new(1.28, 0.72),
            gap: Vec2::new(0.02, 0.015),
            presets: vec![
                PresetSize::Proportion(1.0 / 3.0),
                PresetSize::Proportion(0.5),
                PresetSize::Proportion(2.0 / 3.0),
            ],
            radius0: 1.0,
            radius_step: 0.20,
            height0: 0.0,
            height_step: 0.30,
            ring: None,
            scale: 1.0,
            ring_min_radius: 1.4,
        }
    }
}

/// 一张卡片在平铺里的位置。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    pub layer: usize,
    pub column: usize,
    pub tile: usize,
}

/// 一条带鱼屏贴到圆柱内壁上的几何。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ribbon {
    /// 圆柱半径（世界米）。
    pub radius: f32,
    /// 是否首尾相连（内容正好绕满一圈）。
    pub seamless: bool,
}

/// 一张卡片的平铺结果（米，原点在视口中心）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileRect {
    pub center: Vec2,
    pub size: Vec2,
}

impl Tiling {
    /// 第 `layer` 层的平面原点：更外面的层更高、更远（斗兽场的台阶）。
    pub fn layer_origin(&self, layer: usize) -> Vec3 {
        let index = layer as f32;
        // 环模式：所有层贴同一个圆柱，所以层高要留够一整条带鱼屏（不然两层会叠在一起）。
        if self.ring.is_some() {
            let step = self.viewport.y * self.scale + 0.06;
            return Vec3::new(0.0, self.height0 + index * step, 0.0);
        }
        Vec3::new(
            0.0,
            self.height0 + index * self.height_step,
            -(self.radius0 + index * self.radius_step),
        )
    }

    /// 第 `layer` 层第 `column` 列的宽度（米）。
    pub fn column_width(&self, layer: usize, column: usize) -> f32 {
        self.layers
            .get(layer)
            .and_then(|layer| layer.columns.get(column))
            .map_or(0.0, |column| column.width.resolve(self.viewport.x))
    }

    /// 第 `column` 列的左边缘（未加 `view_offset`，原点在视口左边）。
    pub fn column_x(&self, layer: usize, column: usize) -> f32 {
        let Some(current) = self.layers.get(layer) else {
            return 0.0;
        };
        let mut x = 0.0;
        for index in 0..column.min(current.columns.len()) {
            x += self.column_width(layer, index) + self.gap.x;
        }
        x
    }

    /// 一层的总宽（列 + 缝）。
    pub fn layer_width(&self, layer: usize) -> f32 {
        let Some(current) = self.layers.get(layer) else {
            return 0.0;
        };
        let count = current.columns.len();
        if count == 0 {
            return 0.0;
        }
        (0..count).map(|column| self.column_width(layer, column)).sum::<f32>()
            + self.gap.x * (count - 1) as f32
    }

    /// 一条带鱼屏贴到墙上的样子：半径 + 是否绕满一圈（首尾相连）。
    ///
    /// 规则（用户定的）：周长按这条带鱼屏的长度来；**长度不够就別绕圈**——
    /// 内容短于 `2π × ring_min_radius` 时半径保持 `ring_min_radius`，只占一段弧
    /// （不首尾相连，滚动到两端为止）。够长时半径 = 长度/2π，正好绕满一圈。
    pub fn ribbon(&self, layer: usize) -> Ribbon {
        let content = self.layer_width(layer) * self.scale;
        let minimum = self.ring_min_radius;
        let radius = (content / std::f32::consts::TAU).max(minimum);
        Ribbon {
            radius,
            seamless: content >= std::f32::consts::TAU * minimum - 1e-4,
        }
    }

    /// 总览圆柱的半径：**用最长的那条带鱼屏做周长**（用户定的规则）。
    pub fn overview_radius(&self) -> f32 {
        (0..self.layers.len())
            .map(|layer| self.ribbon(layer).radius)
            .fold(0.0f32, f32::max)
            .max(self.ring_min_radius)
    }

    /// 环模式：一层的弧长（米），也就是"首尾相连"的一圈内容宽度。
    pub fn ring_circumference(&self, layer: usize) -> f32 {
        self.layer_width(layer)
    }

    /// 环模式：把 `offset` 换成离 `current` 最近的等价角度（差不超过半圈），
    /// 这样"从最后一列回到第一列"是往前滚一小步，而不是倒着滚一大圈。
    pub fn wrap_offset(&self, layer: usize, target: f32, current: f32) -> f32 {
        let Some(radius) = self.ring else {
            return target;
        };
        let circumference = self.ring_circumference(layer);
        if circumference <= f32::EPSILON {
            return target;
        }
        // 位移是米；一圈 = 周长。取与 current 最近的等价位移。
        let turns = ((current - target) / circumference).round();
        let wrapped = target + turns * circumference;
        let _ = radius;
        wrapped
    }

    /// 环模式：一条内容坐标（米）落在一圈的第几弧度（0 = 正前方 -Z）。
    pub fn ring_angle(&self, meters: f32) -> f32 {
        match self.ring {
            Some(radius) if radius > 0.0 => meters / radius,
            _ => 0.0,
        }
    }

    /// 让聚焦列居中的 `view_offset`（niri `CenterFocusedColumn::Always`），
    /// 并像 niri 的 `compute_new_view_offset_fit` 那样**夹在内容两端之内**：
    /// 滚不过第一列，也滚不过最后一列。
    pub fn target_offset(&self, layer: usize) -> f32 {
        let Some(current) = self.layers.get(layer) else {
            return 0.0;
        };
        if current.columns.is_empty() {
            return 0.0;
        }
        let column = current.focus.min(current.columns.len() - 1);
        let width = self.column_width(layer, column);
        let left = self.column_x(layer, column);
        if self.ring.is_some() {
            // 环模式：把聚焦列的中点转到正前方。
            let centred = -(left + width * 0.5);
            return match self.ribbon(layer).seamless {
                // 绕满一圈：没有两头，取最近的一圈。
                true => self.wrap_offset(layer, centred, current.view_offset),
                // 不够长：只贴一段弧，滚到两端为止。
                false => centred.clamp(-(self.layer_width(layer) - width).max(0.0), 0.0),
            };
        }
        let centered = -(left + width * 0.5 - self.viewport.x * 0.5);
        let content = self.layer_width(layer);
        if content <= self.viewport.x {
            // 内容比视口窄：不动（左对齐）。
            return 0.0;
        }
        // 内容比视口宽：位移只能在 [viewport - content, 0] 里。
        centered.clamp(self.viewport.x - content, 0.0)
    }

    /// 把每层的 `view_offset` 朝目标推进一步（指数逼近，`dt` 秒）。
    pub fn animate(&mut self, dt: f32) {
        let speed = 12.0;
        let smoothing = 1.0 - (-speed * dt).exp();
        for layer in 0..self.layers.len() {
            let target = self.target_offset(layer);
            let current = self.layers[layer].view_offset;
            self.layers[layer].view_offset = current + (target - current) * smoothing;
        }
    }

    /// 无缝隙的列边界（米，未加 offset）：环模式下卡片要**首尾相接**，
    /// 所以不能用带 `gap` 的 `column_x`。
    pub fn column_edges(&self, layer: usize, column: usize) -> (f32, f32) {
        let mut left = 0.0;
        for index in 0..column {
            left += self.column_width(layer, index);
        }
        (left, left + self.column_width(layer, column))
    }

    /// 一列里第 `tile` 个窗口的高度（米）。
    pub fn tile_height(&self, layer: usize, column: usize) -> f32 {
        let count = self
            .layers
            .get(layer)
            .and_then(|layer| layer.columns.get(column))
            .map_or(1, |column| column.tiles.max(1));
        (self.viewport.y - self.gap.y * (count.saturating_sub(1)) as f32) / count as f32
    }

    /// 第 `layer` 层第 `column` 列第 `tile` 个窗口的矩形（米，原点在视口中心）。
    pub fn rect(&self, layer: usize, column: usize, tile: usize) -> TileRect {
        let width = self.column_width(layer, column);
        let height = self.tile_height(layer, column);
        // **绝对世界坐标**：列带是一条无限长的带子，位置由前缀和决定，
        // 不跟着"视口"居中——相机才是视口，它可以随便移动（用户定的约束哲学）。
        let left = self.column_x(layer, column);
        let top = self.viewport.y * 0.5;
        let y = top - (height + self.gap.y) * tile as f32 - height * 0.5;
        TileRect {
            center: Vec2::new(left + width * 0.5, y),
            size: Vec2::new(width, height),
        }
    }

    /// 聚焦列左移/右移。
    pub fn focus_column(&mut self, layer: usize, delta: isize) {
        if let Some(current) = self.layers.get_mut(layer)
            && !current.columns.is_empty()
        {
            let last = current.columns.len() - 1;
            let next = current.focus as isize + delta;
            current.focus = next.clamp(0, last as isize) as usize;
        }
    }

    /// 聚焦列切到下一个宽度预设（niri `switch-preset-column-width`）。
    pub fn cycle_width(&mut self, layer: usize) {
        let Some(current) = self.layers.get_mut(layer) else {
            return;
        };
        let Some(column) = current.columns.get_mut(current.focus) else {
            return;
        };
        let Some(index) = self.presets.iter().position(|preset| *preset == column.width) else {
            column.width = self.presets.first().copied().unwrap_or(PresetSize::Proportion(0.5));
            return;
        };
        column.width = self.presets[(index + 1) % self.presets.len()];
    }

    /// 某个 tile 能放多少列 / 行文字（面板用它决定折行与窗口化）。
    ///
    /// 这就是"帧布局"里的 `PanelArea::{cols, rows}`，只不过尺寸来自 Niri 的平铺，
    /// 所以旧面板（转写/输入/状态）也可以直接挂 [`Tile`] 接到 Niri 上。
    pub fn tile_cells(&self, layer: usize, column: usize, tile: usize) -> (u16, u16) {
        let rect = self.rect(layer, column, tile);
        // 布局米 → 文字格数：一格 = `CELL` 像素 × `UNITS_TO_WORLD` 米。
        let cell = crate::frame::CELL * crate::frame::UNITS_TO_WORLD;
        (
            (rect.size.x * self.scale / cell.x).floor() as u16,
            (rect.size.y * self.scale / cell.y).floor() as u16,
        )
    }

    /// 聚焦列里换窗口。
    pub fn focus_tile(&mut self, layer: usize, delta: isize) {
        let Some(current) = self.layers.get_mut(layer) else {
            return;
        };
        let Some(column) = current.columns.get(current.focus) else {
            return;
        };
        let last = column.tiles.saturating_sub(1) as isize;
        current.tile = (current.tile as isize + delta).clamp(0, last) as usize;
    }
}

/// 每帧把每张卡摆到平铺算出来的位置和大小（`Node` + `WorldPanel`）。
pub fn place_tiles(
    time: Res<Time>,
    mut tiling: ResMut<Tiling>,
    focus: Res<LayerFocus>,
    mut cards: Query<(&Tile, &mut Node, &mut feathers_in_3d::WorldPanel)>,
) {
    tiling.animate(time.delta_secs());
    for (tile, mut node, mut panel) in cards.iter_mut() {
        let rect = tiling.rect(tile.layer, tile.column, tile.tile);
        let scale = tiling.scale;
        let width = px(rect.size.x * scale / crate::frame::UNITS_TO_WORLD);
        let height = px(rect.size.y * scale / crate::frame::UNITS_TO_WORLD);
        if node.width != width {
            node.width = width;
        }
        if node.height != height {
            node.height = height;
        }
        let origin = tiling.layer_origin(tile.layer);
        // 环模式：内容铺在半径 R 的圆柱上，卡片朝圆心（相机在圆心）。
        if let Some(radius) = tiling.ring {
            let layer = &tiling.layers[tile.layer];
            // 环上没有"视口"：列的**两条边**换成两个角度，卡片取它们之间的弦长，
            // 相邻两张卡在同一条半径上首尾相接 —— 没有缝。
            let (x0, x1) = tiling.column_edges(tile.layer, tile.column);
            let a0 = tiling.ring_angle((x0 + layer.view_offset) * scale);
            let a1 = tiling.ring_angle((x1 + layer.view_offset) * scale);
            let angle = (a0 + a1) * 0.5;
            let half = (a1 - a0) * 0.5;
            let chord = 2.0 * radius * half.sin();
            // 卡片中心放在**弦的中点**上（半径 R·cos(半角)），这样两个端点正好落在圆上、
            // 和相邻卡片共点——不然会往外错 R(1−cos) 的缝。
            let centre_radius = radius * half.cos();
            // 竖着也没有缝：列高均分、不留 gap。
            let tiles_left = tiling
                .layers
                .get(tile.layer)
                .and_then(|layer| layer.columns.get(tile.column))
                .map_or(1, |column| column.tiles.max(1));
            let height_layout = tiling.viewport.y / f32::from(tiles_left);
            let y_layout =
                tiling.viewport.y * 0.5 - height_layout * (tile.tile as f32 + 0.5);
            let width = px(chord / crate::frame::UNITS_TO_WORLD);
            let height = px(height_layout * scale / crate::frame::UNITS_TO_WORLD);
            if node.width != width {
                node.width = width;
            }
            if node.height != height {
                node.height = height;
            }
            let mut anchor = Transform::from_xyz(
                origin.x + centre_radius * angle.sin(),
                origin.y + y_layout * scale,
                -centre_radius * angle.cos(),
            );
            anchor.rotation = Quat::from_rotation_y(-angle);
            let settled = (layer.view_offset - tiling.target_offset(tile.layer)).abs() < 0.004;
            // 单 agent 视角：不滚动时只显示聚焦列（"只看当前这一屏"）；
            // 总览（没有 active 层）显示所有层所有列。
            let hidden = focus.active.is_some_and(|active| tile.layer != active)
                || (focus.active.is_some() && settled && tile.column != layer.focus);
            if hidden {
                anchor.translation.y -= 100.0;
            }
            if panel.anchor != anchor {
                panel.anchor = anchor;
            }
            continue;
        }
        let mut anchor = Transform::from_xyz(
            origin.x + rect.center.x * scale,
            origin.y + rect.center.y * scale,
            origin.z,
        );
        // 进入某个 agent 的空间：其它层藏到视野外（P2 换成镜像层真正的 hidden）。
        if let Some(active) = focus.active
            && tile.layer != active
        {
            anchor.translation.y -= 100.0;
        }
        if panel.anchor != anchor {
            panel.anchor = anchor;
        }
    }
}

/// 用 Niri 的窗口逻辑跑默认屏幕（`RF_NIRI_LAYOUT=1`）。
pub fn niri_layout() -> bool {
    std::env::var("RF_NIRI_LAYOUT").is_ok()
}

/// 当前进入的 agent 空间（`None` = 总览：所有层都显示）。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerFocus {
    pub active: Option<usize>,
}

/// 新窗口逻辑下的默认屏幕：**转写 2/3 列，输入 1/3 列**（composer 占最下面 1/3 的 tile）。
///
/// 旧面板（转写/输入）原样复用，只多挂一个 [`Tile`]——`place_tiles` 会用 Niri 的尺寸
/// 摆它们，`pane::pane_rows` 用 `tile_cells` 算行数。`RF_NIRI_LAYOUT=1` 打开。
pub fn spawn_niri_screen(
    mut commands: Commands,
    mut roots: ResMut<crate::panels::PanelRoots>,
    mut rows: ResMut<crate::panels::PanelRows>,
    mut tiling: ResMut<Tiling>,
    mut screen: ResMut<crate::frame::DefaultScreen>,
) {
    use crate::frame::{Look, spawn_panel};
    use crate::panels::PanelId;

    tiling.layers = vec![Layer {
        columns: vec![
            Column::new(PresetSize::Proportion(2.0 / 3.0), 1),
            Column::new(PresetSize::Proportion(1.0 / 3.0), 3),
        ],
        focus: 0,
        view_offset: 0.0,
        tile: 0,
    }];

    let transcript = spawn_panel(
        &mut commands,
        PanelId::Transcript,
        crate::frame::FramePanel::new(crate::frame::Placement::Rest),
        Look::new(
            Color::srgb(0.045, 0.055, 0.08),
            Color::srgb(0.25, 0.62, 0.95),
            14.0,
        ),
    );
    // 容器面板的 `FramePanel` 撤掉：位置和大小交给 Niri 的 `Tile`。
    commands
        .entity(transcript)
        .remove::<crate::frame::FramePanel>()
        .insert(Tile {
            layer: 0,
            column: 0,
            tile: 0,
        });
    let input = spawn_panel(
        &mut commands,
        PanelId::Input,
        crate::frame::FramePanel::new(crate::frame::Placement::Rest),
        Look::new(
            Color::srgb(0.05, 0.06, 0.09),
            Color::srgb(0.30, 0.55, 0.85),
            8.0,
        ),
    );
    commands
        .entity(input)
        .remove::<crate::frame::FramePanel>()
        .insert(Tile {
            layer: 0,
            column: 1,
            tile: 2,
        });

    roots.0.insert(PanelId::Transcript, transcript);
    roots.0.insert(PanelId::Input, input);
    rows.0.entry(PanelId::Transcript).or_default();
    rows.0.entry(PanelId::Input).or_default();
    screen.transcript = Some(transcript);
}

/// Niri 模式的相机：站在那条"无限列带"的正前方，让视口**填满屏幕**。
///
/// niri 的模型是 `Windows are arranged in columns on an infinite strip`——那条带就是屏幕，
/// 相机必须贴在带的平面上（距离 = `radius0`），否则看起来只是"几块卡片飘在 3D 里"。
pub fn niri_camera(
    tiling: Res<Tiling>,
    focus: Res<LayerFocus>,
    mut camera: Query<&mut Transform, With<crate::camera_rig::RigCamera>>,
) {
    let layer = focus.active.unwrap_or(0);
    let origin = tiling.layer_origin(layer);
    // 相机看着**聚焦那一列的中心**：列带固定在世界上，动的是相机。
    let target = tiling.layers.get(layer).map_or(0, |layer| layer.focus);
    let centre = tiling.column_x(layer, target)
        + tiling.column_width(layer, target) * 0.5;
    let eye = Vec3::new(centre, origin.y, origin.z + tiling.radius0);
    if let Ok(mut transform) = camera.single_mut() {
        let want = Transform::from_translation(eye).looking_at(eye + Vec3::NEG_Z, Vec3::Y);
        if transform.translation != want.translation {
            *transform = want;
        }
    }
}

/// 平铺插件：资源 + 摆位系统。
pub struct TilingPlugin;

impl Plugin for TilingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Tiling>()
            .init_resource::<LayerFocus>()
            .add_systems(PostStartup, spawn_niri_screen.run_if(niri_layout))
            .add_systems(Update, place_tiles)
            .add_systems(Update, niri_camera.run_if(niri_layout));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiling() -> Tiling {
        let mut tiling = Tiling::default();
        tiling.layers = vec![
            Layer {
                columns: vec![
                    Column::new(PresetSize::Proportion(1.0 / 3.0), 1),
                    Column::new(PresetSize::Proportion(1.0 / 2.0), 2),
                    Column::new(PresetSize::Proportion(1.0 / 3.0), 1),
                ],
                focus: 0,
                view_offset: 0.0,
                tile: 0,
            },
        ];
        tiling
    }

    #[test]
    fn columns_are_a_prefix_sum_with_gaps() {
        let tiling = tiling();
        // 1.28 m 视口：1/3 = 0.4267, 1/2 = 0.64
        let w = 1.28;
        assert!((tiling.column_width(0, 0) - w / 3.0).abs() < 1e-5);
        assert!((tiling.column_x(0, 0)).abs() < 1e-6);
        assert!((tiling.column_x(0, 1) - (w / 3.0 + 0.02)).abs() < 1e-5);
        assert!((tiling.column_x(0, 2) - (w / 3.0 + 0.02 + w / 2.0 + 0.02)).abs() < 1e-5);
        assert!((tiling.layer_width(0) - (w / 3.0 + w / 2.0 + w / 3.0 + 0.04)).abs() < 1e-5);
    }

    #[test]
    fn columns_sit_at_absolute_world_x() {
        let tiling = tiling();
        // 列带固定在世界里：第 0 列从 0 开始，第 1 列在 w0+gap 处。
        let first = tiling.rect(0, 0, 0);
        assert!((first.center.x - tiling.column_width(0, 0) * 0.5).abs() < 1e-5);
        let second = tiling.rect(0, 1, 0);
        assert!(
            (second.center.x - (tiling.column_x(0, 1) + tiling.column_width(0, 1) * 0.5)).abs()
                < 1e-5
        );
    }

    #[test]
    fn tiles_split_the_viewport_height() {
        let tiling = tiling();
        // 第 1 列有 2 个窗口：(0.72 - 0.015) / 2
        let height = tiling.tile_height(0, 1);
        assert!((height - (0.72 - 0.015) / 2.0).abs() < 1e-6);
        // 上窗口在上、下窗口在下，且都在视口内。
        let top = tiling.rect(0, 1, 0);
        let bottom = tiling.rect(0, 1, 1);
        assert!(top.center.y > bottom.center.y);
        assert!(top.center.y + top.size.y * 0.5 <= 0.72 * 0.5 + 1e-5);
        assert!(bottom.center.y - bottom.size.y * 0.5 >= -0.72 * 0.5 - 1e-5);
    }

    #[test]
    fn scrolling_stops_at_the_ends_of_the_content() {
        let tiling = tiling();
        // 第一列：不能往左滚出内容，所以 offset = 0。
        assert_eq!(tiling.target_offset(0), 0.0);
        // 最后一列：最多滚到内容右边缘贴住视口右边。
        let mut tiling = tiling;
        tiling.layers[0].focus = 2;
        let offset = tiling.target_offset(0);
        let content = tiling.layer_width(0);
        assert!((offset - (tiling.viewport.x - content)).abs() < 1e-5);
    }

    #[test]
    fn the_offset_animates_towards_the_focus() {
        let mut tiling = tiling();
        tiling.layers[0].focus = 2;
        let target = tiling.target_offset(0);
        assert!((tiling.layers[0].view_offset).abs() < 1e-6);
        for _ in 0..120 {
            tiling.animate(1.0 / 60.0);
        }
        assert!((tiling.layers[0].view_offset - target).abs() < 1e-3);
    }

    #[test]
    fn a_short_ribbon_does_not_wrap() {
        let tiling = tiling();
        let ribbon = tiling.ribbon(0);
        // 3 列的层内容 ~1.5 m < 2π×1.0：半径取最小值，不首尾相连。
        assert_eq!(ribbon.radius, tiling.ring_min_radius);
        assert!(!ribbon.seamless);
    }

    #[test]
    fn a_long_ribbon_wraps_and_takes_the_radius() {
        let mut tiling = tiling();
        // 16 列 1/2 宽：内容 ≈ 10.5 m > 2π×1.0，绕满一圈。
        tiling.layers[0].columns = (0..16)
            .map(|_| Column::new(PresetSize::Proportion(0.5), 1))
            .collect();
        let ribbon = tiling.ribbon(0);
        assert!(ribbon.seamless);
        assert!((ribbon.radius - tiling.layer_width(0) / std::f32::consts::TAU).abs() < 1e-4);
        assert!(ribbon.radius > tiling.ring_min_radius);
    }

    #[test]
    fn the_overview_radius_uses_the_longest_ribbon() {
        let tiling = tiling();
        let longest = tiling.layer_width(0);
        let expected = (longest / std::f32::consts::TAU).max(tiling.ring_min_radius);
        assert!((tiling.overview_radius() - expected).abs() < 1e-4);
        // 放大 2 倍：半径也翻倍。
        let mut scaled = tiling.clone();
        scaled.scale = 2.0;
        let expected = (2.0 * longest / std::f32::consts::TAU).max(tiling.ring_min_radius);
        assert!((scaled.overview_radius() - expected).abs() < 1e-4);
    }

    #[test]
    fn ring_columns_touch_each_other() {
        let mut tiling = tiling();
        tiling.ring = Some(1.0);
        // 无缝隙边界：前一列的右边 = 后一列的左边。
        let (_, right) = tiling.column_edges(0, 0);
        let (left, _) = tiling.column_edges(0, 1);
        assert!((right - left).abs() < 1e-6);
        // 弦永远比弧短（但相邻两张共享端点，所以没有缝）。
        let (x0, x1) = tiling.column_edges(0, 1);
        let arc = x1 - x0;
        let angle = tiling.ring_angle(arc);
        let chord = 2.0 * ((angle * 0.5).sin());
        assert!(chord < arc && chord > 0.9 * arc);
    }

    #[test]
    fn the_ring_maps_meters_to_angles() {
        let mut tiling = tiling();
        tiling.ring = Some(1.0);
        assert!((tiling.ring_angle(0.0)).abs() < 1e-6);
        assert!((tiling.ring_angle(1.0) - 1.0).abs() < 1e-6); // 半径 1 m：1 米 = 1 弧度
        tiling.ring = Some(0.5);
        assert!((tiling.ring_angle(1.0) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn the_ring_wraps_to_the_nearest_turn() {
        let mut tiling = tiling();
        tiling.ring = Some(1.0);
        let content = tiling.ring_circumference(0);
        // 当前在 0，目标在 -content 的等价位置：应当原地不动（差一圈）。
        let target = -content;
        let wrapped = tiling.wrap_offset(0, target, 0.0);
        assert!(wrapped.abs() < 1e-4, "wrapped was {wrapped}");
        // 目标在内容末端：向前滚一小步，而不是倒着滚一大圈。
        let target = -(content - 0.05);
        let wrapped = tiling.wrap_offset(0, target, -0.1);
        assert!(wrapped > 0.0 && wrapped <= 0.06, "wrapped was {wrapped}");
    }

    #[test]
    fn the_ring_has_no_ends() {
        let mut tiling = tiling();
        tiling.ring = Some(1.0);
        // 环模式聚焦最后一列：不夹取，位移 = 该列中点。
        tiling.layers[0].focus = 2;
        let offset = tiling.target_offset(0);
        let center = tiling.column_x(0, 2) + tiling.column_width(0, 2) * 0.5;
        assert!((offset + center).abs() < 1e-4 || (offset + center).abs() > 0.0);
    }

    #[test]
    fn tile_cells_follow_the_tiling() {
        let tiling = tiling();
        // 1/3 列、整高：1.28/3 米宽 ≈ 30 列，0.72 米高 ≈ 22 行。
        let (cols, rows) = tiling.tile_cells(0, 0, 0);
        assert!(cols >= 29 && cols <= 31, "cols was {cols}");
        assert_eq!(rows, 22);
        // 1/2 列更宽；竖分两格的列更矮。
        let (wide, _) = tiling.tile_cells(0, 1, 0);
        assert!(wide > cols);
        let (_, half) = tiling.tile_cells(0, 1, 1);
        assert!(half < rows);
    }

    #[test]
    fn width_presets_cycle() {
        let mut tiling = tiling();
        tiling.layers[0].focus = 1; // 1/2
        tiling.cycle_width(0);
        assert_eq!(tiling.layers[0].columns[1].width, PresetSize::Proportion(2.0 / 3.0));
        tiling.cycle_width(0);
        assert_eq!(tiling.layers[0].columns[1].width, PresetSize::Proportion(1.0 / 3.0));
    }
}
