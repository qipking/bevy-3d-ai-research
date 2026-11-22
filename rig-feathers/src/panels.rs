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

use crate::transcript::{Row, RowText, fill_row_text, spawn_row};

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
#[allow(clippy::type_complexity)]
fn sync_panels(
    mut commands: Commands,
    rows: Res<PanelRows>,
    roots: Res<PanelRoots>,
    mut spawned: ResMut<PanelSpawned>,
    font: Res<crate::transcript::TranscriptFont>,
) {
    for (id, panel_rows) in rows.0.iter() {
        let Some(root) = roots.entity(*id) else {
            continue;
        };
        let state = spawned.0.entry(*id).or_default();

        let common = state
            .keys
            .iter()
            .zip(panel_rows.iter())
            .take_while(|(key, item)| **key == item.key)
            .count();
        for index in 0..common {
            if state.hashes[index] != panel_rows[index].hash {
                let entity = state.entities[index];
                fill_row_text(&mut commands, entity, &panel_rows[index].row, &font.0);
                commands
                    .entity(entity)
                    .insert(RowText(row_plain(&panel_rows[index].row)));
                state.hashes[index] = panel_rows[index].hash;
            }
        }
        for entity in state.entities.drain(common..) {
            commands.entity(entity).despawn();
        }
        state.keys.truncate(common);
        state.hashes.truncate(common);

        if common < panel_rows.len() {
            let new_rows: Vec<(u64, u64, Row)> = panel_rows[common..]
                .iter()
                .map(|item| (item.key, item.hash, item.row.clone()))
                .collect();
            let mut entities = Vec::new();
            commands.entity(root).with_children(|parent| {
                for (_, _, row) in &new_rows {
                    entities.push(spawn_row(parent, row, &font.0));
                }
            });
            for ((key, hash, _), entity) in new_rows.into_iter().zip(entities) {
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
