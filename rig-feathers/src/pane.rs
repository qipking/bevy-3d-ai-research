//! 转写窗格：把整段对话切成"面板这一帧放得下的那一窗"，并管滚动。
//!
//! rig-tui 的 `TranscriptPane` 每帧按宽度排好行、按高度截出可见的一段
//! （`PaneState::visible`），滚到底时跟着新行走，滚上去时留一行给"下面还有"
//! 的标记。这里的对应物是一条流水线：
//!
//! ```text
//! batch::collect_rows ──> TranscriptRows（整段，未窗口化）
//!                              │
//! frame::PanelAreas ──────────>│ window_transcript（窗顶 = 滚动位置）
//!                              ▼
//!                         PanelRows[Transcript]（这一帧真正生成的行）
//! ```
//!
//! 面板大小由 [`crate::frame`] 决定，所以窗口化放在 `FeathersSystems::Fit`：
//! 面板拿到区域之后、行被生成之前。

use bevy::prelude::*;
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};

use crate::frame::{PanelAreas, PanelRoots};
use crate::panels::{KeyedRow, PanelId, PanelRows};
use crate::transcript::Row;
use crate::FeathersSystems;

/// 整段转写的行（未窗口化）。[`crate::batch`] 写它。
#[derive(Resource, Default)]
pub struct TranscriptRows(pub Vec<KeyedRow>);

impl TranscriptRows {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn get(&self, index: usize) -> Option<&KeyedRow> {
        self.0.get(index)
    }
}

/// 主转写窗格的滚动：窗顶是第几行，是否跟着末尾（rig-tui 的 `Scroll`）。
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneScroll {
    /// 窗顶那一行在整段里的下标。
    pub top: usize,
    /// 新行到来时跟着末尾走（滚到底时恢复）。
    pub follow: bool,
}

impl Default for PaneScroll {
    fn default() -> Self {
        Self {
            top: 0,
            follow: true,
        }
    }
}

impl PaneScroll {
    /// 一页滚多少行：窗高减一行，让上下两页有一行重叠（rig 的 `PaneState::page`）。
    pub fn page(&self, rows: usize) -> usize {
        rows.saturating_sub(1).max(1)
    }

    /// 往上滚（离开末尾）。
    pub fn up(&mut self, rows: usize) {
        self.follow = false;
        self.top = self.top.saturating_sub(rows);
    }

    /// 往下滚；到达末尾就重新跟随。
    pub fn down(&mut self, rows: usize, max_top: usize) {
        self.top = self.top.saturating_add(rows).min(max_top);
        if self.top >= max_top {
            self.follow = true;
        }
    }

    /// 直接跳到某一行（`None` = 回到末尾跟随）。
    pub fn to(&mut self, top: Option<usize>, max_top: usize) {
        match top {
            Some(top) => {
                self.top = top.min(max_top);
                self.follow = self.top >= max_top;
            }
            None => {
                self.top = max_top;
                self.follow = true;
            }
        }
    }
}

/// 转写窗格这一帧能放多少行。
///
/// 优先看 Niri 的 `Tile`（新窗口逻辑），没有再退回帧布局的 `PanelAreas`。
fn pane_rows(
    areas: &PanelAreas,
    roots: &PanelRoots,
    tiles: &Query<&crate::niri::Tile>,
    tiling: Option<&crate::niri::Tiling>,
) -> Option<usize> {
    let root = roots.entity(PanelId::Transcript)?;
    if let Ok(tile) = tiles.get(root)
        && let Some(tiling) = tiling
    {
        let (_, rows) = tiling.tile_cells(tile.layer, tile.column, tile.tile);
        return (rows > 0).then_some(usize::from(rows));
    }
    let area = areas.get(root)?;
    (!area.is_empty()).then_some(usize::from(area.rows))
}

/// 滚上去时占掉最后一行的标记（rig-tui `pane::draw_below` 的同一句话）。
pub const BELOW_MARKER: &str = " \u{2193} more below (PgDn) ";

fn below_marker() -> KeyedRow {
    KeyedRow {
        key: u64::MAX,
        hash: 1,
        row: Row::new(Line::from("")).with_right(Line::from(Span::styled(
            BELOW_MARKER,
            Style::default()
                .fg(RColor::LightCyan)
                .add_modifier(Modifier::BOLD),
        ))),
    }
}

/// 把整段的可见窗口写进转写面板（`FeathersSystems::Fit`）。
///
/// 滚上去、下面还有行时，最后一行留给 [`BELOW_MARKER`]（rig-tui 是把标记
/// 画在最后一行的右端，占掉的是同一行）。
pub fn window_transcript(
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
    full: Res<TranscriptRows>,
    mut scroll: ResMut<PaneScroll>,
    mut panels: ResMut<PanelRows>,
    tiles: Query<&crate::niri::Tile>,
    tiling: Option<Res<crate::niri::Tiling>>,
) {
    let Some(height) = pane_rows(&areas, &roots, &tiles, tiling.as_deref()) else {
        return;
    };
    let len = full.len();
    let max_top = len.saturating_sub(height);
    if scroll.follow {
        scroll.top = max_top;
    }
    let top = scroll.top.min(max_top);
    if top != scroll.top {
        scroll.top = top;
    }
    let below = !scroll.follow && top < max_top && height > 1;
    let rows_shown = match below {
        true => height - 1,
        false => height,
    };
    let end = (top + rows_shown).min(len);
    let mut rows = full.0[top..end].to_vec();
    if below {
        rows.push(below_marker());
    }
    panels.set(PanelId::Transcript, rows);
}

/// PageUp / PageDown 滚动转写（rig-tui 在面板键位域里绑定它们）。
pub fn pane_keys(
    keys: Res<ButtonInput<KeyCode>>,
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
    full: Res<TranscriptRows>,
    mut scroll: ResMut<PaneScroll>,
    tiles: Query<&crate::niri::Tile>,
    tiling: Option<Res<crate::niri::Tiling>>,
) {
    let height = pane_rows(&areas, &roots, &tiles, tiling.as_deref()).unwrap_or(0);
    if height == 0 {
        return;
    }
    let max_top = full.len().saturating_sub(height);
    let page = scroll.page(height);
    if keys.just_pressed(KeyCode::PageUp) {
        scroll.up(page);
    }
    if keys.just_pressed(KeyCode::PageDown) {
        scroll.down(page, max_top);
    }
}

pub struct PanePlugin;

impl Plugin for PanePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TranscriptRows>()
            .init_resource::<PaneScroll>()
            .add_systems(
                Update,
                pane_keys
                    .in_set(FeathersSystems::Prepare)
                    .run_if(crate::picker::no_picker),
            )
            .add_systems(
                Update,
                window_transcript.in_set(FeathersSystems::Fit),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_follows_the_tail_until_scrolled_up() {
        let mut scroll = PaneScroll::default();
        // 内容比窗口长：跟随末尾。
        scroll.to(None, 40);
        assert!(scroll.follow);
        assert_eq!(scroll.top, 40);
        // 往上翻一页：不再跟随。
        scroll.up(scroll.page(20));
        assert!(!scroll.follow);
        assert_eq!(scroll.top, 21);
        // 翻回末尾：重新跟随。
        scroll.down(1000, 40);
        assert!(scroll.follow);
        assert_eq!(scroll.top, 40);
    }

    #[test]
    fn scrolling_up_from_the_top_stays_at_zero() {
        let mut scroll = PaneScroll::default();
        scroll.up(10);
        assert_eq!(scroll.top, 0);
        assert!(!scroll.follow);
    }

    #[test]
    fn a_one_row_pane_pages_by_one() {
        let scroll = PaneScroll::default();
        assert_eq!(scroll.page(1), 1);
        assert_eq!(scroll.page(0), 1);
        assert_eq!(scroll.page(20), 19);
    }
}
