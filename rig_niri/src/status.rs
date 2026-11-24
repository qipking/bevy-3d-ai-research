//! 状态行：画 rig 自己的 `StatusItems` / `AppStatus`（与 rig-tui 同一份数据）。
//!
//! rig-tui 的 `status.rs` 把条目裁成两行：左边一串、右边一串，行太窄时先丢
//! `keep` 最低的，能缩的条目缩到 `min_width` 再丢（`status::line`，公开函数）。
//! 这里**直接调用它**，不重写裁剪规则——状态行的行为因此和终端一模一样；
//! 我们只负责把这两行摆成一行内的两端（`Row::with_right`，bevy_ui 的
//! `SpaceBetween` 负责右对齐，不靠算空格）。

use bevy::prelude::*;
use rig_ecs::prelude::Agent;
use rig_tui::ratatui::style::Style;
use rig_tui::ratatui::text::Line;
use rig_tui::{AppStatus, StatusItems};

use crate::frame::{PanelAreas, PanelRoots};
use crate::panels::{KeyedRow, PanelId, PanelRows};
use crate::transcript::Row;

/// 把 `AppStatus` 与当前 agent 的 `StatusItems` 合起来，按面板宽度裁好，
/// 写进状态面板（左边一条、右边一条）。
pub fn status_rows(
    mut panels: ResMut<PanelRows>,
    app_status: Option<Res<AppStatus>>,
    agents: Query<&StatusItems, With<Agent>>,
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
) {
    let mut items: Vec<_> = Vec::new();
    if let Some(app) = &app_status {
        items.extend(app.0.iter().cloned());
    }
    if let Some(agent_items) = agents.iter().next() {
        items.extend(agent_items.iter().cloned());
    }
    if items.is_empty() {
        panels.set(PanelId::Status, Vec::new());
        return;
    }
    let width = roots
        .entity(PanelId::Status)
        .and_then(|root| areas.get(root))
        .map_or(96, |area| area.cols);
    // rig-tui 的裁剪：左边一条、右边一条，丢/缩按 keep 与 min_width。
    let (left, right) = rig_tui::status::line(items.iter(), width);
    let mut row = Row::new(left);
    if !right.width().eq(&0) {
        row = row.with_right(right);
    }
    let line = row.line.clone();
    let right = row.right.clone();
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        format!("{line:?}").hash(&mut hasher);
        if let Some(right) = &right {
            format!("{right:?}").hash(&mut hasher);
        }
        hasher.finish()
    };
    panels.set(
        PanelId::Status,
        vec![KeyedRow { key: 1, hash, row }],
    );
}

/// 注册状态行：`AppStatus` 资源 + 每帧把条目画成一行。
///
/// `Agent` 上的 `StatusItems` 由 [`crate::FeathersViewPlugin`] 在 `finish` 里注册
/// （和 rig-tui 的 `TuiViewPlugin` 一样，插件添加顺序因此不影响结果）。
pub struct StatusPlugin;

impl Plugin for StatusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppStatus>()
            .add_systems(
                Update,
                status_rows.in_set(crate::FeathersSystems::Draw),
            );
    }
}

/// 状态行的一行样式（给业务画自己的状态条目时用）。
pub fn plain(text: impl Into<String>) -> Line<'static> {
    Line::from(rig_tui::ratatui::text::Span::styled(
        text.into(),
        Style::default(),
    ))
}
