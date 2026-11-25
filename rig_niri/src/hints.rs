//! 键位提示：照抄 rig-tui `hints.rs` 的两组键位、优先级与 `fit_keys`。
//!
//! 与上游一致的部分：
//! - 两张键位表与优先级（上游 `hints.rs:46`–`:67`），只有 `Ctrl+V` 一条按
//!   本 crate 的实际行为写 `paste`（上游是 `image`，图片粘贴还没接）；
//! - `fit_keys`（上游 `:72`–`:99`）：整行是 `format!(" {} ", …)`，**带首尾
//!   空格**；宽度按 `unicode_width` 的显示列算（中文/emoji 占 2 列）；同权重
//!   时丢**最后**一个（`min_by_key` 里的 `Reverse(at)`），丢过条目后行尾加 `…`；
//! - 提示行只在**输入为空**时显示（上游 `editor.rs:731`–`:733` 的
//!   `else if !editor.is_empty()`）；输入非空时内容是一行空行。
//!
//! 提示行**不再拼进输入面板**：它是一块独立的 tile（[`PanelId::Hints`]），
//! 行由 [`hint_rows`] 产。文案和原来输入框下面那一行逐字相同（只是挪了个位置）。
//!
//! 位置（树不归本模块改）：输入框正下方、状态行上方，横跨整条，固定 1 行
//! （[`hint_tile_height`]）。输入非空时只把内容清成空行、**不收起 tile**：
//! 它是根竖 split 里的固定子，收起会让主体行（以及右列的 Spacer/Input）
//! 整体长高、输入时整屏跳一下；固定高度空着画与 niri 的"窗口不因内容显隐"
//! 一致，也对应上游"输入区始终保留键位行，空时是空行"。
//!
//! `turn_running` 取自 `ShownAgent` 上有没有 [`ActiveTurn`]（和状态行/输入
//! 系统用的 `busy` 是同一个事实来源）。只有输入为空时才显示运行中那一组
//! 键位——"输入为空"和"turn 在跑"两个条件取交集，和上游一致。

use bevy::prelude::*;
use rig_ecs::prelude::ActiveTurn;
use rig_harness::main_agent::ShownAgent;
use rig_tui::ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::editor::Editor;
use crate::frame::{PanelAreas, PanelRoots};
use crate::panels::{KeyedRow, PanelId, PanelRows};
use crate::theme::Theme;
use crate::transcript::{ROW_HEIGHT, Row, row_hash};

/// 一条键位提示：文字 + 重要性（越高越不该丢）。
pub type Key = (&'static str, u8);

/// rig-tui 的两组键位提示（`hints::keys`，上游 `hints.rs:46`–`:67`）。
///
/// 与上游逐字相同，只有 `Ctrl+V` 一条按本 crate 的实际行为写 `paste`（上游
/// 是 `image`；图片粘贴还没接，见 `editor.rs` 的 `Do::PasteImage` 注释）。
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
///
/// 与上游逐字一致（上游 `hints.rs:72`–`:99`）：整行带首尾空格
/// （`format!(" {} ", …)`）、宽度按 `unicode_width` 的显示列算、同权重时丢
/// **最后**一个（`min_by_key` 的 `Reverse(at)`）。连 ` … ` 都放不下时返回
/// 空串——和上游一样，不是只留省略号。
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
        let row = format!(" {} ", items.join(" · "));
        if row.width() <= width {
            return row;
        }
        // The least important; of equals, the last.
        let least = kept
            .iter()
            .enumerate()
            .min_by_key(|(at, (_, keep))| (*keep, std::cmp::Reverse(*at)))
            .map(|(at, _)| at);
        match least {
            Some(at) => {
                kept.remove(at);
            }
            None => return String::new(),
        }
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

/// 当前该显示的键位提示（`turn_running` 由业务传入；模板里按实际 busy 算）。
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
/// rig-tui 原来的行为，拆成 tile 后保持不变。宽度小到连 ` … ` 都放不下时
/// 内容为空串（上游 `fit_keys` 的退路），行本身仍然存在。
///
/// 返回 `Vec<Row>` 是为了和面板的行模型一致；`width == 0` 时也返回一行
/// （内容为空串，不 panic）。
pub fn hint_rows(width: usize, turn_running: bool, theme: &Theme) -> Vec<Row> {
    vec![Row::new(Line::from(Span::styled(
        current_hints(width, turn_running),
        theme.dim,
    )))]
}

/// 提示 tile 这一帧的内容：输入为空时是提示行，输入非空时是**一行空行**。
///
/// 为什么是空行而不是把 tile 收成 0 高度：Hints 是根竖 split 里的固定子
/// （`layout::default_tree`），收起它会改变主体行高度、右列跟着变高，输入时
/// 整屏跳一下；固定 1 行空着画，tile 的几何完全不变，也和上游"输入区保留
/// 键位行、非空时是空行"一致。右列（Spacer / Confirm / Complete / Input）
/// 因此不受任何影响，总高始终精确等于它的区域高度。
pub fn hint_tile_rows(
    width: usize,
    turn_running: bool,
    input_empty: bool,
    theme: &Theme,
) -> Vec<Row> {
    match input_empty {
        true => hint_rows(width, turn_running, theme),
        // 一行空串（不是空 `Line`）：和 picker 的 gap 行同一种写法，渲染侧
        // 有一个空 TextSpan 可填，避免零 span 的 Text 节点。
        false => vec![Row::new(Line::from(""))],
    }
}

/// 提示面板这一帧的行：写进 [`PanelRows`] 的 [`PanelId::Hints`]。
///
/// 宽度取树给 Hints 叶子的文字列数；树里还没有这块面板（根不存在）时用
/// 97 兜底（和 `editor::editor_rows` 的兜底一致），行照写，只是没有根可画。
///
/// 显示条件与上游一致：**输入为空**才显示键位（上游 `draw_editors` 在
/// `!editor.is_empty()` 时把 keys 清空）。输入非空时写一行空行，tile 的固定
/// 高度不变。`turn_running` 由 `ShownAgent` + `ActiveTurn` 推出来：运行中
/// 且输入为空时显示 `Enter steers` / `Tab sends after the turn` / `Esc stops`
/// 那一组（"输入为空"与"turn 在跑"取交集）。
pub fn hint_panel_rows(
    mut panels: Option<ResMut<PanelRows>>,
    roots: Option<Res<PanelRoots>>,
    areas: Option<Res<PanelAreas>>,
    theme: Option<Res<Theme>>,
    editor: Option<Res<Editor>>,
    shown: Option<Res<ShownAgent>>,
    entering: Query<(), With<ActiveTurn>>,
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
    // 没有 Editor 资源（只装 `HintsPlugin` 的最小 App）时按空输入处理，
    // 保持本模块原来的输出。
    let input_empty = editor.is_none_or(|editor| editor.is_empty());
    // 和 `editor::keys` / 状态行同一个 busy：ShownAgent 指的那个 agent 身上
    // 有没有 `ActiveTurn`。
    let turn_running = shown
        .as_deref()
        .and_then(|shown| shown.0)
        .is_some_and(|agent| entering.contains(agent));
    let rows = hint_tile_rows(width, turn_running, input_empty, &theme);
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
    //! 提示行的行为测试：纯值 + 不建窗口的最小 App，不需要 GUI。

    use super::*;
    use crate::editor::EditorLayout;
    use crate::frame::FrameTree;
    use crate::layout::{PX_TO_METERS, default_tree};
    use crate::niri::TileRect;
    use rig_ecs::prelude::TurnOf;
    use unicode_width::UnicodeWidthStr;

    fn row_text(row: &Row) -> String {
        row.line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    /// 只装提示/输入布局 + 默认帧树的最小 App：Hints 根 120 列宽。
    fn hint_app() -> App {
        let mut app = App::new();
        app.init_resource::<Editor>()
            .init_resource::<EditorLayout>()
            .init_resource::<PanelRows>()
            .init_resource::<Theme>()
            .insert_resource(FrameTree(default_tree()));
        let root = app.world_mut().spawn_empty().id();
        let mut roots = PanelRoots::default();
        roots.0.insert(PanelId::Hints, root);
        let mut areas = PanelAreas::default();
        areas.0.insert(
            root,
            crate::frame::PanelArea {
                cols: 120,
                rows: 1,
                ..Default::default()
            },
        );
        app.insert_resource(roots)
            .insert_resource(areas)
            .add_systems(
                Update,
                (
                    crate::editor::layout_editor,
                    crate::editor::sync_editor_tile,
                    hint_panel_rows,
                )
                    .chain(),
            );
        app
    }

    fn hint_text(app: &App) -> String {
        let rows = app.world().resource::<PanelRows>().get(PanelId::Hints);
        assert_eq!(rows.len(), 1, "Hints tile 固定写一行（输入非空时是空行）");
        row_text(&rows[0].row)
    }

    // ---- fit_keys：与上游逐字一致 ----

    #[test]
    fn fit_keys_joins_the_whole_row_with_a_space_on_each_side() {
        let all = fit_keys(keys(false), 500);
        let names: Vec<&str> = keys(false).iter().map(|(name, _)| *name).collect();
        assert_eq!(
            all,
            format!(" {} ", names.join(" · ")),
            "上游整行是 format!(\" {{}} \", …)，带首尾空格"
        );
        assert!(all.starts_with(' ') && all.ends_with(' '), "{all:?}");
        // 空表：上游在拼行之前就返回空串（没有首尾空格）。
        assert_eq!(fit_keys(&[], 10), "");
    }

    #[test]
    fn fit_keys_measures_unicode_display_width_not_chars() {
        // "键" 是 1 个 char、2 个显示列：3 列放不下 " 键 "（4 列），只放得下
        // " … "（3 列）。按 chars().count() 会错误地留下 " 键 "。
        assert_eq!(fit_keys(&[("键", 1)], 3), " … ");
        assert_eq!(fit_keys(&[("键", 1)], 4), " 键 ");
        // 两个同权重的宽字：8 列放不下 " 键 · 盘 "（9 列），只留第一个。
        assert_eq!(fit_keys(&[("键", 1), ("盘", 1)], 8), " 键 · … ");
    }

    #[test]
    fn fit_keys_drops_the_last_of_equals() {
        // 两条同权重：宽度只放得下一条时，上游丢**最后**一个（Reverse(at)）。
        let keys = [("aaaa", 1), ("bbbb", 1)];
        assert_eq!(fit_keys(&keys, 12), " aaaa · … ");
        // 13 列正好放得下两条完整条目。
        assert_eq!(fit_keys(&keys, 13), " aaaa · bbbb ");
    }

    #[test]
    fn fit_keys_keeps_everything_that_fits_and_never_exceeds_the_width() {
        // 上游自带的宽度扫掠测试（`hints/tests.rs` 的
        // `a_narrow_key_row_keeps_what_matters_most_and_says_it_dropped_some`）。
        for keys in [keys(true), keys(false)] {
            let all = fit_keys(keys, 500);
            let names: Vec<&str> = keys.iter().map(|(name, _)| *name).collect();
            assert_eq!(all, format!(" {} ", names.join(" · ")));
            let most = keys.iter().map(|(_, keep)| *keep).max().unwrap_or_default();
            let top = keys.iter().find(|(_, keep)| *keep == most);
            let top = top.map_or("", |(name, _)| *name);
            for width in format!(" {top} · … ").width()..all.width() {
                let row = fit_keys(keys, width);
                assert!(row.width() <= width, "{row}");
                assert!(row.trim_end().ends_with('…'), "{row}");
                // 丢掉的东西都不能比留下的更重要；留下的保持原顺序。
                let kept: Vec<&str> = row
                    .trim()
                    .split(" · ")
                    .filter(|item| *item != "…")
                    .collect();
                let rank = |item: &&str| {
                    keys.iter()
                        .find(|(name, _)| name == item)
                        .map(|(_, keep)| *keep)
                };
                let lowest = kept.iter().filter_map(rank).min().unwrap_or_default();
                for (name, keep) in keys {
                    assert!(
                        *keep < lowest || kept.contains(name),
                        "{name} at {width}: {row}"
                    );
                }
                let order: Vec<&str> = names
                    .iter()
                    .copied()
                    .filter(|name| kept.contains(name))
                    .collect();
                assert_eq!(kept, order, "{width}");
            }
        }
        assert!(fit_keys(keys(true), 15).contains("Esc stops"));
    }

    // ---- 提示行的内容 ----

    #[test]
    fn the_hint_row_is_the_whole_official_line_at_120_columns() {
        let theme = Theme::default();
        let rows = hint_rows(120, false, &theme);
        assert_eq!(rows.len(), 1, "提示行永远是一行，不折行");
        assert_eq!(
            row_text(&rows[0]),
            " Enter sends · Shift+Enter or Ctrl+J new line · Ctrl+V paste · \
             / commands · @ files · Ctrl+D quits "
        );
        // 和 current_hints 同一个结果（拆行不改文案）。
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
            " Enter sends · / commands · … ",
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
            assert_eq!(
                row_text(&rows[0]),
                current_hints(width, false),
                "宽度 {width}"
            );
        }
        // 上游 `fit_keys` 的退路：连 " … " 都放不下时是空串（不是 "…"）。
        assert_eq!(row_text(&hint_rows(0, false, &theme)[0]), "");
        assert_eq!(row_text(&hint_rows(2, false, &theme)[0]), "");
        assert_eq!(row_text(&hint_rows(3, false, &theme)[0]), " … ");
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
    fn the_hint_tile_is_blank_while_the_input_has_text() {
        let theme = Theme::default();
        let empty = hint_tile_rows(120, false, true, &theme);
        assert_eq!(empty.len(), 1);
        assert!(!row_text(&empty[0]).is_empty(), "输入为空时有提示");

        let typed = hint_tile_rows(120, false, false, &theme);
        assert_eq!(typed.len(), 1, "非空时保留一行空行，tile 不收起");
        assert_eq!(row_text(&typed[0]), "");
        assert_eq!(typed[0].line.width(), 0);

        // 运行中且输入为空：换成运行中的表；输入非空仍然空行。
        let running = hint_tile_rows(200, true, true, &theme);
        assert!(row_text(&running[0]).contains("Enter steers"));
        let running_typed = hint_tile_rows(200, true, false, &theme);
        assert_eq!(row_text(&running_typed[0]), "");
    }

    #[test]
    fn the_hint_tile_is_exactly_one_row_tall() {
        assert_eq!(hint_tile_height(), ROW_HEIGHT);
        assert_eq!(hint_tile_height(), 18.0);
    }

    // ---- 接线：输入空/非空、turn running、右列总高 ----

    #[test]
    fn the_hints_row_follows_the_input_and_the_running_turn() {
        let mut app = hint_app();

        // 空输入：整行提示（带首尾空格）。
        app.update();
        assert_eq!(hint_text(&app), current_hints(120, false));
        assert!(hint_text(&app).starts_with(' '));

        // 输入非空：tile 内容清成空行。
        app.world_mut().resource_mut::<Editor>().set("draft");
        app.update();
        assert_eq!(hint_text(&app), "", "输入非空时提示行不显示");

        // 再清空：提示行回来。
        app.world_mut().resource_mut::<Editor>().clear();
        app.update();
        assert_eq!(hint_text(&app), current_hints(120, false));

        // turn 运行中：输入为空才显示运行中的键位（与上游取交集）。
        let agent = app.world_mut().spawn(Name::new("agent")).id();
        app.insert_resource(ShownAgent(Some(agent)));
        app.world_mut().spawn(TurnOf(agent));
        app.update();
        let running = hint_text(&app);
        assert!(running.contains("Enter steers"), "{running}");
        assert!(running.contains("Tab sends after the turn"), "{running}");
        assert!(running.contains("Esc stops"), "{running}");

        app.world_mut().resource_mut::<Editor>().set("draft");
        app.update();
        assert_eq!(hint_text(&app), "", "turn 在跑但输入非空：仍然不显示");
    }

    #[test]
    fn typing_and_clearing_keeps_the_right_column_exact() {
        let area = TileRect {
            center: Vec2::ZERO,
            size: Vec2::new(1.2, 0.8),
        };
        let top = |rect: &TileRect| rect.center.y + rect.size.y * 0.5;
        let bottom = |rect: &TileRect| rect.center.y - rect.size.y * 0.5;
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        // 根竖 split：主体（比例 1）+ Hints 固定 1 行 + Status 固定 2 行。
        let expected_right =
            area.size.y - (HINT_ROWS + 2.0) * ROW_HEIGHT * PX_TO_METERS;

        let mut app = hint_app();
        let mut right_heights = Vec::new();
        let mut input_heights = Vec::new();
        for text in ["", "one", "one\ntwo\nthree", ""] {
            app.world_mut().resource_mut::<Editor>().set(text);
            app.update();

            // 提示行：只有空输入有内容，tile 始终一行。
            assert_eq!(
                hint_text(&app).is_empty(),
                !text.is_empty(),
                "只有空输入才显示提示：{text:?}"
            );

            let lines = app.world().resource::<EditorLayout>().rows.len();
            let tree = &app.world().resource::<FrameTree>().0;
            let rects = tree.rects(area);
            let find = |panel: PanelId| {
                rects
                    .iter()
                    .find(|(id, _)| *id == panel)
                    .expect("默认树里的叶子")
                    .1
            };
            let input = find(PanelId::Input);
            let spacer = find(PanelId::Spacer);
            let hints = find(PanelId::Hints);
            let status = find(PanelId::Status);

            assert!(
                close(
                    input.size.y,
                    crate::editor::input_tile_height(lines) * PX_TO_METERS
                ),
                "{text:?}: 输入 tile 高度跟着内容走"
            );
            assert!(close(bottom(&input), top(&hints)), "输入贴着提示行");
            assert!(close(bottom(&hints), top(&status)), "提示行贴着状态行");
            assert!(close(bottom(&status), bottom(&area)), "状态行贴底");
            assert!(
                close(hints.size.y, HINT_ROWS * ROW_HEIGHT * PX_TO_METERS),
                "{text:?}: 提示行始终固定 1 行"
            );
            // 右列 = 顶部的 Spacer 到输入框底边：比例子吸收输入高度的差额，
            // 总高只由"区域高度 - 提示行 - 状态行"决定，与提示行内容无关。
            let right = top(&spacer) - bottom(&input);
            assert!(
                close(right, expected_right),
                "{text:?}: 右列总高不等于区域高度减去固定行（{right} vs {expected_right}）"
            );
            assert!(
                close(spacer.size.y + input.size.y, right),
                "{text:?}: Spacer 吸收输入高度的差额"
            );
            let leaves: f32 = rects
                .iter()
                .map(|(_, rect)| rect.size.x * rect.size.y)
                .sum();
            assert!(
                close(leaves, area.size.x * area.size.y),
                "{text:?}: 叶子面积仍然精确铺满"
            );
            right_heights.push(right);
            input_heights.push(input.size.y);
        }

        // 输入从空→非空（1 行）→3 行→空，输入高度确实变过……
        assert_ne!(input_heights[1], input_heights[2], "3 行输入让 tile 变高");
        // ……但右列总高一个像素都不变。
        for right in &right_heights {
            assert!(
                close(*right, expected_right),
                "输入空/非空切换时右列总高必须不变：{right_heights:?}"
            );
        }
    }
}
