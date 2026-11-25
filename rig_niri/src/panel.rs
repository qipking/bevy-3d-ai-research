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
//! 切片 1 只实现“注册 + 列 + 根 + 行读回”；`StatusLine` 组件留给切片 2
//! （设计见 `docs/design/PANEL_CANVAS.md` §5）。

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_ui::JustifyContent;

use feathers_in_3d::WorldPanel;

use crate::frame::{CELL, FramePanel, FrameSize, FrameTree, Look, PanelArea, PanelInset};
use crate::layout::{self, Node as LayoutNode};
use crate::panels::{
    PanelId, PanelRoots, PanelRows, PanelSpawned, PluginPanel, RowLayout,
};
use crate::theme::{PANEL_BG, PANEL_BORDER as PANEL_BORDER_TOKEN};
use crate::transcript::ROW_HEIGHT;
use crate::workspaces::{Workspace, WorkspacePanels, WorkspaceScroll, WorkspaceSet};

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
}

impl PanelRegistry {
    /// 注册（或取回）名字为 `name` 的面板，返回句柄。
    ///
    /// 空名字会生成一个稳定于本次注册的 `plugin-panel-<n>`，不会 panic。
    pub fn register(&mut self, name: impl Into<String>) -> PluginPanel {
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
    /// 注册（或取回）名字为 `name` 的面板。
    fn register_panel(&mut self, name: impl Into<String>) -> PluginPanel;
}

impl PanelRegistryExt for App {
    fn register_panel(&mut self, name: impl Into<String>) -> PluginPanel {
        self.world_mut()
            .get_resource_or_init::<PanelRegistry>()
            .register(name)
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
    let fraction = if fraction.is_finite() && fraction > 0.0 {
        fraction.min(1.0)
    } else {
        layout::DEFAULT_COLUMN_FRACTION
    };
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
    //! 切片 1 的闭环：注册 → 放进条带 → 根 → 区域 → 行世界矩形。
    //! 不建窗口、不跑 GPU；3D“可见”用 `WorldPanel.anchor` + `clip` 的交集断言。

    use super::*;
    use crate::frame::{FrameGeometry, PanelAreas, UNITS_TO_WORLD};
    use crate::layout::Node as TestNode;
    use crate::panels::{KeyedRow, PanelsPlugin};
    use crate::transcript::{Row, TranscriptFont};
    use crate::workspaces::{WorkspaceSet, WorkspacesPlugin, spawn_workspace};
    use bevy::text::FontSource;
    use rig_tui::ratatui::text::Line;

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
            .add_plugins((PanelsPlugin, WorkspacesPlugin))
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
}
