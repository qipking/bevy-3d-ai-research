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

/// 一行面板内容 + 稳定的 key/hash（增量 diff 靠它对齐）。
#[derive(Clone, Debug)]
pub struct KeyedRow {
    pub key: u64,
    pub hash: u64,
    pub row: Row,
}

/// 面板身份（决定行来源）。
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PanelId {
    Transcript,
    Status,
    Input,
    /// 浮层（picker / 补全）的容器：底色 + 边框，行在 [`PanelId::List`] 里。
    Overlay,
    /// 浮层里的列表列（有 preview 时只占容器的一半）。
    List,
    /// 浮层里的一列（picker 的 preview）。
    Preview,
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
#[derive(Resource, Default)]
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
#[derive(Resource, Default)]
pub struct PanelRoots(pub HashMap<PanelId, Entity>);

impl PanelRoots {
    pub fn entity(&self, id: PanelId) -> Option<Entity> {
        self.0.get(&id).copied()
    }
}

/// 一块面板已经生成的行（增量 diff 用）。
#[derive(Default)]
pub struct SpawnedRows {
    pub keys: Vec<u64>,
    pub hashes: Vec<u64>,
    pub entities: Vec<Entity>,
}

/// 每块面板各自的已生成行。
#[derive(Resource, Default)]
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

/// 每块面板做增量 diff：前缀原地改文字，只重建变化的后缀。
///
/// 选区高亮也在这里落：选中行会被 [`selected_row`] 套上 `REVERSED` 后再
/// 生成 span。`SpawnedRows::hashes` 里存的是 **上游 hash × 选区样式 hash**
/// 的合成值，所以拖动选区导致同一行的选中区间变化时，这一行也会重画；
/// 选区清掉后，合成值回到上游 hash，同样触发一次去高亮的重画。
#[allow(clippy::type_complexity)]
fn sync_panels(
    mut commands: Commands,
    rows: Res<PanelRows>,
    roots: Res<PanelRoots>,
    mut spawned: ResMut<PanelSpawned>,
    font: Res<crate::transcript::TranscriptFont>,
    theme: Option<Res<crate::theme::Theme>>,
    selection: Option<Res<Selection>>,
) {
    // 主题是可选资源：没有主题的测试 App 退化成默认主题，不让系统被跳过。
    let theme = theme.map(|t| *t.as_ref()).unwrap_or_default();
    for (id, panel_rows) in rows.0.iter() {
        let Some(root) = roots.entity(*id) else {
            continue;
        };
        let state = spawned.0.entry(*id).or_default();

        // 只有"选区就在这块面板里"时才需要行值/字节区间；否则 ranges 为空，
        // 下面的 row 原样生成，合成 hash 退回上游 hash。
        let selected = selection
            .as_deref()
            .filter(|selection| selection.panel() == Some(*id));
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
                fill_row_text_themed(&mut commands, entity, row, &font.0, &theme);
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
                    entities.push(spawn_row_themed(parent, row, &font.0, &theme));
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

pub struct PanelsPlugin;

impl Plugin for PanelsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PanelRows>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelSpawned>()
            .add_systems(
                Update,
                sync_panels.in_set(crate::FeathersSystems::Render),
            );
    }
}

#[cfg(test)]
mod tests {
    //! 行布局与选区高亮的接线测试：不建窗口，只跑 `sync_panels`。
    use super::*;
    use crate::selection::{ClickKind, Selection};
    use crate::transcript::{RowText, TranscriptFont};
    use bevy::text::FontSource;
    use rig_tui::ratatui::style::{Modifier, Style};
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

    /// 一行的 span 列表：`(文本, 实际画出来的颜色)`。
    fn spans(app: &App, row: Entity) -> Vec<(String, Color)> {
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
        let reversed = crate::transcript::span_style_color(
            &Style::default().add_modifier(Modifier::REVERSED),
            &theme,
        );
        let plain = crate::transcript::span_style_color(&Style::default(), &theme);
        assert_eq!(
            spans(&app, rows[0]),
            vec![
                ("hello ".to_owned(), plain),
                ("world".to_owned(), reversed),
            ]
        );

        // 清掉选区：同一行要重画回普通 span（合成 hash 变了）。
        app.world_mut()
            .resource_mut::<Selection>()
            .clear();
        app.update();
        assert_eq!(
            spans(&app, rows[0]),
            vec![("hello world".to_owned(), plain)],
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
        assert_eq!(spans(&app, rows[0]), vec![("hello".to_owned(), plain)]);
    }
}
