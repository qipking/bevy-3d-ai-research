//! 键位提示：照抄 rig-tui `hints.rs` 的两组键位与优先级。
//!
//! rig-tui 的 `keys()`/`fit_keys()` 是 `pub(crate)`，行为是公开的：
//! 条目用 ` · ` 连接；行太窄时先丢 `keep` 最低的，末尾加 `…`。
//!
//! 提示行**不再拼进输入面板**：它是一块独立的 tile（[`PanelId::Hints`]），
//! 行由 [`hint_rows`] 产。裁剪逻辑没有改，所以文案和原来输入框下面那一行
//! 逐字相同（只是挪了个位置）。
//!
//! 树里的建议（树不归本模块改）：右列输入框正下方、状态行上方，固定 1 行
//! （[`hint_tile_height`]）。原来它就在输入面板的最下面一行，拆出来后放同一
//! 位置，视觉顺序不变。

use bevy::prelude::*;
use rig_tui::ratatui::text::{Line, Span};

use crate::frame::{PanelAreas, PanelRoots};
use crate::panels::{KeyedRow, PanelId, PanelRows};
use crate::theme::Theme;
use crate::transcript::{Row, ROW_HEIGHT, row_hash};

/// 一条键位提示：文字 + 重要性（越高越不该丢）。
pub type Key = (&'static str, u8);

/// rig-tui 的两组键位提示（`hints::keys`）。
pub fn keys(turn_running: bool) -> &'static [Key] {
    match turn_running {
        true => &[
            ("Enter steers", 9),
            ("Tab sends after the turn", 7),
            ("Esc stops", 10),
            ("Shift+Enter or Ctrl+J new line", 5),
            ("Ctrl+V paste", 1),
            ("/ commands", 4),
            ("@ files", 2),
            ("Ctrl+D quits", 3),
        ],
        false => &[
            ("Enter sends", 9),
            ("Shift+Enter or Ctrl+J new line", 7),
            ("Ctrl+V paste", 4),
            ("/ commands", 8),
            ("@ files", 5),
            ("Ctrl+D quits", 6),
        ],
    }
}

/// 在 `width` 列里放下尽可能多的提示，丢掉的用 `…` 表示（rig-tui `fit_keys`）。
pub fn fit_keys(keys: &[Key], width: usize) -> String {
    let mut kept = keys.to_vec();
    loop {
        let mut items: Vec<&str> = kept.iter().map(|(item, _)| *item).collect();
        if kept.len() < keys.len() {
            items.push("…");
        }
        if items.is_empty() {
            return String::new();
        }
        let joined = items.join(" · ");
        if joined.chars().count() <= width {
            return joined;
        }
        // 丢掉重要性最低的一个
        let Some((index, _)) = kept
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, weight))| *weight)
        else {
            return joined;
        };
        kept.remove(index);
    }
}

/// 注册键位提示：资源 + 每帧把提示行写进 [`PanelId::Hints`]。
pub struct HintsPlugin;

impl Plugin for HintsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KeyHints>().add_systems(
            Update,
            hint_panel_rows.in_set(crate::FeathersSystems::Draw),
        );
    }

    fn finish(&self, _app: &mut App) {}
}

/// 键位提示是否启用（rig-tui 用 `KeyHints` 资源表示，这里同名）。
#[derive(Resource, Default)]
pub struct KeyHints;

/// 当前该显示的键位提示（`turn_running` 由业务传入；模板里恒为 false）。
pub fn current_hints(width_cells: usize, turn_running: bool) -> String {
    fit_keys(keys(turn_running), width_cells)
}

/// 提示 tile 固定占几行。
pub const HINT_ROWS: f32 = 1.0;

/// 提示 tile 的高度（UI 像素）：固定 1 行。
///
/// 和补全 tile 的 [`crate::complete::completion_tile_height`] 同一个算法
/// （行数 × [`ROW_HEIGHT`]）；树里把这块叶子设成这个高度即可。
pub fn hint_tile_height() -> f32 {
    HINT_ROWS * ROW_HEIGHT
}

/// 提示行本身：**恰好 1 行**，按 `width` 列裁剪（`fit_keys` 的规则）。
///
/// 和补全提示（`complete::completion_hint_rows` 会折行）不同，键位提示是
/// 一条状态行：太窄时丢低优先级的条目、末尾加 `…`，从不折成多行 —— 这是
/// rig-tui 原来的行为，拆成 tile 后保持不变。
///
/// 返回 `Vec<Row>` 是为了和面板的行模型一致；`width == 0` 时也返回一行
/// （内容退化成 `…`，不 panic）。
pub fn hint_rows(width: usize, turn_running: bool, theme: &Theme) -> Vec<Row> {
    vec![Row::new(Line::from(Span::styled(
        current_hints(width, turn_running),
        theme.dim,
    )))]
}

/// 提示面板这一帧的行：写进 [`PanelRows`] 的 [`PanelId::Hints`]。
///
/// 宽度取树给 Hints 叶子的文字列数；树里还没有这块面板（根不存在）时用
/// 97 兜底（和 `editor::editor_rows` 的兜底一致），行照写，只是没有根可画。
/// 提示文案和输入面板里原来的那一行完全相同，`turn_running` 在模板里是
/// `false`（业务要显示运行中的按键组时直接调 [`hint_rows`]）。
pub fn hint_panel_rows(
    mut panels: Option<ResMut<PanelRows>>,
    roots: Option<Res<PanelRoots>>,
    areas: Option<Res<PanelAreas>>,
    theme: Option<Res<Theme>>,
) {
    let Some(panels) = panels.as_deref_mut() else {
        // 只装 `HintsPlugin` 的测试 App 没有面板资源：没有东西可写。
        return;
    };
    let width = roots
        .as_deref()
        .and_then(|roots| roots.entity(PanelId::Hints))
        .and_then(|root| areas.as_deref().and_then(|areas| areas.get(root)))
        .map_or(97, |area| usize::from(area.cols.max(1)));
    let theme = theme.map(|theme| *theme).unwrap_or_default();
    let rows = hint_rows(width, false, &theme);
    panels.set(
        PanelId::Hints,
        rows.into_iter()
            .enumerate()
            .map(|(index, row)| KeyedRow {
                key: index as u64,
                hash: row_hash(&row),
                row,
            })
            .collect(),
    );
}

#[cfg(test)]
mod tests {
    //! 提示行的行为测试：纯值，不需要 App/窗口。

    use super::*;

    fn row_text(row: &Row) -> String {
        row.line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn the_hint_row_is_the_whole_official_line_at_120_columns() {
        let theme = Theme::default();
        let rows = hint_rows(120, false, &theme);
        assert_eq!(rows.len(), 1, "提示行永远是一行，不折行");
        assert_eq!(
            row_text(&rows[0]),
            "Enter sends · Shift+Enter or Ctrl+J new line · Ctrl+V paste · \
             / commands · @ files · Ctrl+D quits"
        );
        // 和输入面板原来用的函数同一个结果（拆行不改文案）。
        assert_eq!(
            row_text(&rows[0]),
            current_hints(120, false),
            "hint_rows 必须和 current_hints 逐字一致"
        );
    }

    #[test]
    fn a_narrow_hint_row_drops_low_priority_keys_and_ends_with_the_ellipsis() {
        let theme = Theme::default();
        let rows = hint_rows(40, false, &theme);
        assert_eq!(rows.len(), 1, "窄面板也只丢条目，不折行");
        assert_eq!(
            row_text(&rows[0]),
            "Enter sends · / commands · …",
            "40 列里先丢 Ctrl+V / @ files / Ctrl+D / Shift+Enter，保留最重要的两条"
        );
        // 被挤掉的低优先级键不再出现（不是折到下一行）。
        let text = row_text(&rows[0]);
        assert!(!text.contains("Ctrl+V"));
        assert!(!text.contains("@ files"));
    }

    #[test]
    fn a_zero_or_tiny_width_hint_row_does_not_panic() {
        let theme = Theme::default();
        for width in [0, 1, 2, 3] {
            let rows = hint_rows(width, false, &theme);
            assert_eq!(rows.len(), 1, "宽度 {width} 时也只产一行");
            let text = row_text(&rows[0]);
            assert!(!text.is_empty(), "宽度 {width} 时至少留一个占位符");
            assert_eq!(text, current_hints(width, false), "宽度 {width}");
        }
        // 上游 `fit_keys` 的退路：连 `…` 都放不下时也只返回 `…`。
        assert_eq!(row_text(&hint_rows(0, false, &theme)[0]), "…");
    }

    #[test]
    fn a_running_turn_uses_the_other_key_set() {
        let theme = Theme::default();
        let idle = row_text(&hint_rows(200, false, &theme)[0]);
        let running = row_text(&hint_rows(200, true, &theme)[0]);
        assert!(idle.contains("Enter sends"));
        assert!(!idle.contains("Enter steers"));
        assert!(running.contains("Enter steers"), "运行中：{running}");
        assert!(running.contains("Tab sends after the turn"), "运行中：{running}");
        assert!(running.contains("Esc stops"), "运行中：{running}");
        assert!(!running.contains("Enter sends"));
    }

    #[test]
    fn the_hint_tile_is_exactly_one_row_tall() {
        assert_eq!(hint_tile_height(), ROW_HEIGHT);
        assert_eq!(hint_tile_height(), 18.0);
    }
}
