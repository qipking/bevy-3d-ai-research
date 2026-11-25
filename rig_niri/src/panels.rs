//! 面板的行：一块面板显示哪些行、怎么增量更新。
//!
//! rig-tui 里面板是 `TuiPanel` + `PanelCanvas`，插件在 `TuiSystems::Draw` 里往
//! 画布上画；这里对应的是 [`PanelRows`] —— 谁产出行谁写进去（转写是
//! [`crate::pane`] 窗口化后的可见行，状态行是 `status`，输入行是 `editor`），
//! 再由 [`sync_panels`] 把行同步成 `bevy_ui` 子节点（增量：前缀原地改字，
//! 只重建变了的那段）。
//!
//! 面板**放在哪**不归这里管：那是 [`crate::frame`] 的 `Placement`。

use std::collections::HashMap;

use bevy::prelude::*;

use crate::selection::{Selection, paint_hash, selected_row};
use crate::transcript::{
    Row, RowText, fill_row_text_themed, spawn_row_themed,
};
use crate::workspaces::{
    WorkspacePanels, WorkspaceSet, commit_focused_panels, publish_focused_spawned,
};

/// 一行面板内容 + 稳定的 key/hash（增量 diff 靠它对齐）。
#[derive(Clone, Debug)]
pub struct KeyedRow {
    pub key: u64,
    pub hash: u64,
    pub row: Row,
}

/// 插件自定义面板的句柄（由 [`crate::panel::PanelRegistry`] 分配）。
///
/// 句柄很小、`Copy`，可以直接存进插件自己的资源/组件里；它只是身份，
/// 不持有实体，也不决定面板放在哪。换工作区/重建工作区时同一个句柄仍然
/// 指“同一个插件面板”，根实体由 [`crate::workspaces::WorkspacePanels::roots`]
/// 按 `PanelId::Plugin(handle)` 查。
///
/// 字段故意不公开：句柄只能从注册表拿，避免插件自己造一个没人登记过的 id。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginPanel(pub(crate) u64);

impl PluginPanel {
    /// 注册表分配的原始 id（日志/测试用；不要拿它当稳定格式）。
    pub fn raw(self) -> u64 {
        self.0
    }

    /// 它对应的 [`PanelId`]：树叶子、`PanelRows` / `PanelRoots` 的键都用这个。
    pub fn id(self) -> PanelId {
        PanelId::Plugin(self)
    }
}

impl std::fmt::Debug for PluginPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PluginPanel#{}", self.0)
    }
}

/// 面板身份（决定行来源）。
///
/// 内置面板是封闭的枚举**变体**（行为不变）；插件面板统一走
/// [`PanelId::Plugin`]，由 [`crate::panel::PanelRegistry`] 分配句柄。所以
/// “加一块面板”不再需要改这个枚举，也不会让内置面板的分支多一步判断。
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PanelId {
    Transcript,
    Status,
    Input,
    /// picker 那一列的容器面板：只给这一列画底色 + 边框，本身不产行。
    ///
    /// picker 不再是浮层：它是条带最右边的一列（`FrameTree` 根横向 split 的
    /// 最后一个子），容器里再竖着分成 [`PanelId::List`] / [`PanelId::Preview`]。
    Picker,
    /// picker 列里的列表面板：标题、说明、过滤行、列表、键位行都写在这里。
    List,
    /// picker 列里的 preview 面板（插件挂 `PickPreview::Text` 时才有）。
    Preview,
    /// 第二个转写窗格：拆窗（`Alt+S`，见 [`crate::frame::split_transcript`]）后
    /// 出现。它是一块**独立视口**：每帧由 [`crate::pane::window_transcript`] 按
    /// 根上 `TranscriptPane::source` 产行（默认 `Shown`，所以拆窗后与主窗格内容
    /// 一致；改成 `Agent`/`Value` 就各画各的，不再镜像）。
    Pane2,
    /// 占位：分裂树里只吃空间、不画任何东西（右列顶部留白，让输入框贴底）。
    Spacer,
    /// 键位提示行：从输入面板里拆出来，单独一个 1 行高的 tile（niri 里就是一块窗口）。
    Hints,
    /// 确认行（Ask / 上一条待确认的操作）：单独一个 tile，不再挤进转写。
    Confirm,
    /// 输入框上方的补全 tile：关着时在分裂树里是 0 高度，打开时占 6 行。
    ///
    /// 行由 [`crate::complete`] 产（候选 + 官方提示行），这里只当普通面板走
    /// [`PanelRows`] / [`PanelRoots`]，不写特殊情况。
    Complete,
    /// 插件注册的自定义面板（见 [`crate::panel`]）。
    ///
    /// 它在树里就是一个普通叶子：`layout` / `frame` / `PanelsPlugin` 都按
    /// “叶子 + 根表 + 行表”处理，不需要给插件加特例。内置面板的代码路径
    /// 完全不变。
    Plugin(PluginPanel),
}

/// 行实体在它所属面板里的位置：行是从 0 数的**可见行下标**（滚动后按窗口顶算）。
///
/// [`sync_panels`] 在生成行实体时插入它，鼠标拾取据此把「行实体」换算成
/// 「哪块面板的第几行」，再把命中点的 x 交给 `selection::hit_test`。
/// 这是只读查询：别的地方**不要**改它，行下标由本模块维护。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowLayout {
    /// 行来自哪块面板（[`PanelId`]，方便直接查 [`PanelRows`]）。
    pub panel: PanelId,
    /// 面板根实体：把命中点从世界坐标换算成这块面板的局部像素要用它。
    pub panel_entity: Entity,
    /// 行在面板里的下标（从 0 数）。
    pub index: usize,
}

/// 每块面板当前要显示的行。
///
/// 第 2 批之后：聚焦工作区的全局工作副本；每个工作区自己的权威行在
/// [`WorkspacePanels::rows`](crate::workspaces::WorkspacePanels) 里。
#[derive(Resource, Default, Clone)]
pub struct PanelRows(pub HashMap<PanelId, Vec<KeyedRow>>);

impl PanelRows {
    pub fn set(&mut self, id: PanelId, rows: Vec<KeyedRow>) {
        self.0.insert(id, rows);
    }
    pub fn get(&self, id: PanelId) -> &[KeyedRow] {
        self.0.get(&id).map(|rows| rows.as_slice()).unwrap_or(&[])
    }
    pub fn clear_all(&mut self) {
        for rows in self.0.values_mut() {
            rows.clear();
        }
    }

    /// 某块面板这一帧的行值（拷贝）。
    ///
    /// `selection` 的接口吃 `&[Row]`（它不改行），而 [`PanelRows`] 存的是带
    /// key/hash 的 [`KeyedRow`]；鼠标按下/拖动/松开时取一份当前可见行即可，
    /// 这些事件每帧至多几个，拷贝量是窗口大小。
    pub fn row_values(&self, id: PanelId) -> Vec<Row> {
        self.get(id).iter().map(|item| item.row.clone()).collect()
    }
}

/// 面板根实体，按 [`PanelId`] 索引（frame 生成时填）。
///
/// 第 2 批之后：聚焦工作区的全局工作副本；每个工作区自己的根表在
/// [`WorkspacePanels::roots`](crate::workspaces::WorkspacePanels) 里。
///
/// 面板根的**表面/边框/圆角**不归本模块：`frame::spawn_panel` 生成根时按
/// `Look` 挂 [`ThemeBackgroundColor`](bevy_feathers::theme::ThemeBackgroundColor) /
/// [`ThemeBorderColor`](bevy_feathers::theme::ThemeBorderColor)（feathers token），
/// 再由 `frame::sync_panel_chrome` 按当前的 `UiTheme` 解析成颜色。本模块只维护
/// 根表，不碰 chrome。
#[derive(Resource, Default, Clone)]
pub struct PanelRoots(pub HashMap<PanelId, Entity>);

impl PanelRoots {
    pub fn entity(&self, id: PanelId) -> Option<Entity> {
        self.0.get(&id).copied()
    }
}

/// 一块面板已经生成的行（增量 diff 用）。
#[derive(Default, Clone, PartialEq)]
pub struct SpawnedRows {
    pub keys: Vec<u64>,
    pub hashes: Vec<u64>,
    pub entities: Vec<Entity>,
}

/// 每块面板各自的已生成行。
#[derive(Resource, Default, Clone, PartialEq)]
pub struct PanelSpawned(pub HashMap<PanelId, SpawnedRows>);

impl PanelSpawned {
    /// 忘掉一块面板已生成的行。面板实体被 despawn（比如浮层关掉）时必须调，
    /// 否则下次同名面板出现时，行同步会去动已经死掉的行实体。
    pub fn forget(&mut self, id: PanelId) {
        self.0.remove(&id);
    }
}

fn row_plain(row: &Row) -> String {
    let mut plain: String = row.line.spans.iter().map(|span| span.content.as_ref()).collect();
    if let Some(right) = &row.right {
        plain.extend(right.spans.iter().map(|span| span.content.as_ref()));
    }
    plain
}

/// 一个工作区（或旧的全局单例）的增量 diff：前缀原地改字，只重建变化的后缀。
///
/// 选区高亮也在这里落：选中行会被 [`selected_row`] 套上 `REVERSED` 后再
/// 生成 span。`SpawnedRows::hashes` 里存的是 **上游 hash × 选区样式 hash**
/// 的合成值，所以拖动选区导致同一行的选中区间变化时，这一行也会重画；
/// 选区清掉后，合成值回到上游 hash，同样触发一次去高亮的重画。
#[allow(clippy::too_many_arguments)]
fn sync_panel_rows(
    mut commands: &mut Commands,
    rows: &PanelRows,
    roots: &PanelRoots,
    spawned: &mut PanelSpawned,
    font: &crate::transcript::TranscriptFont,
    theme: &crate::theme::Theme,
    selection: Option<&Selection>,
) {
    for (id, panel_rows) in rows.0.iter() {
        let Some(root) = roots.entity(*id) else {
            continue;
        };
        let state = spawned.0.entry(*id).or_default();

        // 只有"选区就在这块面板里"时才需要行值/字节区间；否则 ranges 为空，
        // 下面的 row 原样生成，合成 hash 退回上游 hash。
        let selected = selection.filter(|selection| selection.panel() == Some(*id));
        let row_values = selected.map(|_| panel_rows.iter().map(|item| item.row.clone()).collect::<Vec<_>>());
        let ranges: Vec<Vec<(usize, usize)>> = match (&selected, &row_values) {
            (Some(selection), Some(values)) => (0..panel_rows.len())
                .map(|index| selection.byte_ranges_in(index, values))
                .collect(),
            _ => Vec::new(),
        };
        // 第 index 行要高亮的行（None = 原样）。
        let painted = |index: usize| -> Option<Row> {
            let ranges = ranges.get(index)?;
            (!ranges.is_empty()).then(|| selected_row(&panel_rows[index].row, ranges))
        };
        let hash_of = |index: usize| -> u64 {
            paint_hash(
                panel_rows[index].hash,
                ranges.get(index).map_or(&[][..], |ranges| ranges.as_slice()),
            )
        };

        let common = state
            .keys
            .iter()
            .zip(panel_rows.iter())
            .take_while(|(key, item)| **key == item.key)
            .count();
        for index in 0..common {
            let hash = hash_of(index);
            if state.hashes[index] != hash {
                let entity = state.entities[index];
                let painted = painted(index);
                let row = painted.as_ref().unwrap_or(&panel_rows[index].row);
                fill_row_text_themed(&mut commands, entity, row, &font.0, theme);
                commands.entity(entity).insert(RowText(row_plain(row)));
                state.hashes[index] = hash;
            }
        }
        for entity in state.entities.drain(common..) {
            commands.entity(entity).despawn();
        }
        state.keys.truncate(common);
        state.hashes.truncate(common);

        if common < panel_rows.len() {
            let new_rows: Vec<(u64, u64, Row)> = (common..panel_rows.len())
                .map(|index| {
                    let row = painted(index).unwrap_or_else(|| panel_rows[index].row.clone());
                    (panel_rows[index].key, hash_of(index), row)
                })
                .collect();
            let mut entities = Vec::new();
            commands.entity(root).with_children(|parent| {
                for (_, _, row) in &new_rows {
                    entities.push(spawn_row_themed(parent, row, &font.0, theme));
                }
            });
            for (offset, ((key, hash, _), entity)) in
                new_rows.into_iter().zip(entities).enumerate()
            {
                commands.entity(entity).insert(RowLayout {
                    panel: *id,
                    panel_entity: root,
                    index: common + offset,
                });
                state.keys.push(key);
                state.hashes.push(hash);
                state.entities.push(entity);
            }
        }
    }
}

/// 按工作区分派行同步。
///
/// * 有 [`WorkspaceSet`]（插件模式）：每个工作区用自己的行/根/已生成行做 diff，
///   所以两条条带不会互相覆盖；聚焦工作区的状态是 `Render` 里刚提交的工作副本。
/// * 没有 `WorkspaceSet`（只装 `PanelsPlugin` 的旧 App）：退回原来的全局单例，
///   行为与第 2 批之前完全一致。
#[allow(clippy::type_complexity)]
fn sync_panels(
    mut commands: Commands,
    rows: Res<PanelRows>,
    roots: Res<PanelRoots>,
    mut spawned: ResMut<PanelSpawned>,
    font: Res<crate::transcript::TranscriptFont>,
    theme: Option<Res<crate::theme::Theme>>,
    selection: Option<Res<Selection>>,
    set: Option<Res<WorkspaceSet>>,
    mut workspaces: Query<&mut WorkspacePanels>,
) {
    // 主题是可选资源：没有主题的测试 App 退化成默认主题，不让系统被跳过。
    let theme = theme.map(|t| *t.as_ref()).unwrap_or_default();
    if let Some(set) = set.as_deref().filter(|set| !set.items.is_empty()) {
        for workspace in &set.items {
            let Ok(mut panels) = workspaces.get_mut(*workspace) else {
                continue;
            };
            let WorkspacePanels {
                rows: panel_rows,
                roots: panel_roots,
                spawned: panel_spawned,
                ..
            } = &mut *panels;
            // 聚焦工作区的窗格内容由 `pane::window_transcript` 按各自的
            // `TranscriptPane::source` 填，非聚焦工作区由应用层按自己的 agent
            // 同步；这里只做行 -> 节点的增量同步。

            // 选区现在只属于聚焦工作区（`Selection` 还没有工作区字段）：
            // 非聚焦的行按原样画，别把聚焦工作区的选中区间套过去。
            let workspace_selection = (set.focused == Some(*workspace))
                .then_some(selection.as_deref())
                .flatten();
            sync_panel_rows(
                &mut commands,
                panel_rows,
                panel_roots,
                panel_spawned,
                &font,
                &theme,
                workspace_selection,
            );
        }
        return;
    }
    sync_panel_rows(
        &mut commands,
        &rows,
        &roots,
        &mut spawned,
        &font,
        &theme,
        selection.as_deref(),
    );
}

pub struct PanelsPlugin;

impl Plugin for PanelsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PanelRows>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelSpawned>()
            // 插件面板的句柄表（`crate::panel`）。单独 init 而不是另装一个
            // 插件：`PanelRows`/`PanelRoots` 已经是这里初始化的。
            .init_resource::<crate::panel::PanelRegistry>()
            .add_systems(
                Update,
                (
                    commit_focused_panels,
                    sync_panels,
                    // F2：可点击行（Ask 选项 / picker 条目）的按钮态与 focus ring，
                    // 排在行同步之后（`chain` 会插入 deferred，能看到这一帧 spawn 的行）。
                    crate::transcript::update_row_button_styles,
                    crate::transcript::update_row_focus_rings,
                    publish_focused_spawned,
                )
                    .chain()
                    .in_set(crate::FeathersSystems::Render),
            );
    }
}

#[cfg(test)]
mod tests {
    //! 行布局与选区高亮的接线测试：不建窗口，只跑 `sync_panels`。
    use super::*;
    use crate::selection::{ClickKind, Selection};
    use crate::transcript::{RowText, TranscriptFont};
    use bevy::text::{FontSource, TextBackgroundColor};
    use rig_tui::ratatui::style::Style;
    use rig_tui::ratatui::text::Line;

    fn keyed(key: u64, text: &str) -> KeyedRow {
        KeyedRow {
            key,
            hash: key,
            row: Row::new(Line::from(text.to_owned())),
        }
    }

    fn app_with_root() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(PanelsPlugin)
            .init_resource::<Selection>()
            .insert_resource(TranscriptFont(FontSource::default()));
        let root = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<PanelRoots>()
            .0
            .insert(PanelId::Transcript, root);
        (app, root)
    }

    fn row_entities(app: &App, root: Entity) -> Vec<Entity> {
        app.world()
            .get::<Children>(root)
            .map(|children| children.iter().collect())
            .unwrap_or_default()
    }

    /// 新加的 `Pane2` / `Complete` 和普通面板走同一条 diff 路，不需要特例。
    #[test]
    fn the_new_panel_ids_sync_like_any_other_panel() {
        let mut app = App::new();
        app.add_plugins(PanelsPlugin)
            .insert_resource(TranscriptFont(FontSource::default()));
        let complete = app.world_mut().spawn_empty().id();
        let pane2 = app.world_mut().spawn_empty().id();
        {
            let roots = &mut app.world_mut().resource_mut::<PanelRoots>().0;
            roots.insert(PanelId::Complete, complete);
            roots.insert(PanelId::Pane2, pane2);
        }
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Complete, vec![keyed(1, "Tab or Enter completes")]);
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Pane2, vec![keyed(1, "transcript line")]);
        app.update();

        for (root, panel) in [(complete, PanelId::Complete), (pane2, PanelId::Pane2)] {
            let rows = row_entities(&app, root);
            assert_eq!(rows.len(), 1, "{panel:?} 的行要生成");
            let layout = app
                .world()
                .get::<RowLayout>(rows[0])
                .expect("行要有 RowLayout");
            assert_eq!(layout.panel, panel);
            assert_eq!(layout.panel_entity, root);
            assert_eq!(layout.index, 0);
        }
    }

    #[test]
    fn every_spawned_row_gets_its_panel_and_index() {
        let (mut app, root) = app_with_root();
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(10, "one"), keyed(20, "two")]);
        app.update();

        let rows = row_entities(&app, root);
        let layouts: Vec<RowLayout> = rows
            .iter()
            .map(|entity| *app.world().get::<RowLayout>(*entity).expect("行要有 RowLayout"))
            .collect();
        assert_eq!(
            layouts,
            vec![
                RowLayout {
                    panel: PanelId::Transcript,
                    panel_entity: root,
                    index: 0,
                },
                RowLayout {
                    panel: PanelId::Transcript,
                    panel_entity: root,
                    index: 1,
                },
            ]
        );
    }

    #[test]
    fn adding_rows_keeps_the_prefix_and_extends_the_indices() {
        let (mut app, root) = app_with_root();
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(10, "one")]);
        app.update();
        let first = row_entities(&app, root);

        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(10, "one!"), keyed(20, "two")]);
        app.update();
        let rows = row_entities(&app, root);
        assert_eq!(rows[0], first[0], "前缀行原地复用");
        let layouts: Vec<usize> = rows
            .iter()
            .map(|entity| app.world().get::<RowLayout>(*entity).expect("layout").index)
            .collect();
        assert_eq!(layouts, vec![0, 1]);
    }

    /// 一行的 span 列表：`(文本, 实际画出来的前景, 实际画出来的底色)`。
    ///
    /// 反色现在是真反色：前景换成底色、底色换成前景，两样都要断言。
    fn spans(app: &App, row: Entity) -> Vec<(String, Color, Option<Color>)> {
        let text_node = app
            .world()
            .get::<Children>(row)
            .and_then(|children| children.iter().next())
            .expect("行下面有 Text 节点");
        app.world()
            .get::<Children>(text_node)
            .expect("Text 下面有 span")
            .iter()
            .map(|span| {
                (
                    app.world()
                        .get::<TextSpan>(span)
                        .expect("TextSpan")
                        .0
                        .to_string(),
                    app.world().get::<TextColor>(span).expect("TextColor").0,
                    app.world()
                        .get::<TextBackgroundColor>(span)
                        .map(|background| background.0),
                )
            })
            .collect()
    }

    #[test]
    fn a_selected_row_is_painted_with_reversed_spans_and_repainted_when_cleared() {
        let (mut app, root) = app_with_root();
        let row = Row::new(Line::from("hello world"));
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(10, "hello world")]);

        // 选中 "world"（格子列 6..11；ASCII 一格一个 grapheme）。
        let mut selection = Selection::new();
        selection.press_kind(PanelId::Transcript, &[row.clone()], 0, 6, ClickKind::Single);
        selection.drag(PanelId::Transcript, &[row], 0, 11);
        app.world_mut().insert_resource(selection);
        app.update();

        let rows = row_entities(&app, root);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            app.world().get::<RowText>(rows[0]).expect("RowText").0,
            "hello world",
            "RowText 始终是纯文本"
        );

        let theme = crate::theme::Theme::default();
        let plain = crate::transcript::span_style_color(&Style::default(), &theme);
        // 选中 = 真反色：前景换成主题底色（`theme.band`），底色换成原来的前景。
        let reversed_fg = crate::style::color(Some(theme.band));
        assert_eq!(
            spans(&app, rows[0]),
            vec![
                ("hello ".to_owned(), plain, None),
                ("world".to_owned(), reversed_fg, Some(plain)),
            ],
            "选中区间：前景 = 底色、底色 = 前景，且底色是 span 组件真画"
        );

        // 清掉选区：同一行要重画回普通 span（合成 hash 变了）。
        app.world_mut()
            .resource_mut::<Selection>()
            .clear();
        app.update();
        assert_eq!(
            spans(&app, rows[0]),
            vec![("hello world".to_owned(), plain, None)],
            "去高亮"
        );
    }

    #[test]
    fn a_selection_in_another_panel_does_not_paint_this_one() {
        let (mut app, root) = app_with_root();
        app.world_mut()
            .resource_mut::<PanelRows>()
            .set(PanelId::Transcript, vec![keyed(10, "hello")]);
        let rows = [Row::new(Line::from("hello"))];
        let mut selection = Selection::new();
        selection.press_kind(PanelId::Status, &rows, 0, 0, ClickKind::Single);
        selection.drag(PanelId::Status, &rows, 0, 5);
        app.world_mut().insert_resource(selection);
        app.update();

        let theme = crate::theme::Theme::default();
        let plain = crate::transcript::span_style_color(&Style::default(), &theme);
        let rows = row_entities(&app, root);
        assert_eq!(
            spans(&app, rows[0]),
            vec![("hello".to_owned(), plain, None)],
            "别的面板的选区不该给这一行上底色"
        );
    }
}
