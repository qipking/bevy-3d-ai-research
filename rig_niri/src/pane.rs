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
//!
//! 滚动有两个入口，语义一致：
//!
//! - 键盘 PgUp/PgDn（[`pane_keys`]）：一次一页；
//! - 滚轮（[`pane_wheel`]）：一次 [`WHEEL_ROWS`] 行（上游 `input.rs::moused`
//!   的 `pane.scroll.up(3)`），指针在别的面板上时不动转写。
//!
//! 往上滚是用户主动往回看，取消 [`PaneScroll::follow`]；滚到末尾恢复跟随。
//! 窗口/面板尺寸变化不在本模块缓存：窗口化每帧从 [`PanelAreas`]（或 Niri 的
//! 分裂树）读当前行数，resize 后的下一帧就是新尺寸（清选区在
//! [`crate::selection::clear_selection_on_resize`]）。

use bevy::picking::events::PointerScroll;
use bevy::prelude::*;
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};

use crate::frame::{PanelAreas, PanelRoots};
use crate::panels::{KeyedRow, PanelId, PanelRows, RowLayout};
use crate::selection::Selection;
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

/// 滚轮一格滚几行：上游 `input.rs::moused` 里 `pane.scroll.up(3)` 的 3。
pub const WHEEL_ROWS: usize = 3;

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

    /// 滚轮往上滚一格（[`WHEEL_ROWS`] 行）。
    ///
    /// 往上滚是用户主动往回看：取消跟随末尾。跟随中时"当前位置"就是末尾
    /// （[`window_transcript`] 每帧把跟随的窗顶放在 `max_top`），所以从末尾
    /// 起算，而不是从可能还停在旧位置的 `top` 起算。
    ///
    /// 内容不够一屏（`max_top == 0`）时没有可回看的：仍然跟随末尾——上游
    /// `Scroll::place` 里 `top >= bottom` 就是这个分支（不会停在半空）。
    pub fn wheel_up(&mut self, max_top: usize) {
        if max_top == 0 {
            self.top = 0;
            self.follow = true;
            return;
        }
        let top = if self.follow {
            max_top
        } else {
            self.top.min(max_top)
        };
        self.top = top.saturating_sub(WHEEL_ROWS);
        self.follow = false;
    }

    /// 滚轮往下滚一格；滚到末尾就恢复跟随（与 [`PaneScroll::down`] 同一条规则）。
    pub fn wheel_down(&mut self, max_top: usize) {
        self.down(WHEEL_ROWS, max_top);
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
/// 滚上去、下面还有行时的标记。
///
/// 只留一份定义：转写引擎的 [`crate::transcript::MORE_BELOW`]（`Below::More` 用的也是它）。
/// 以前这里另写了一份同样的字符串，两处改一处就会漂。
pub const BELOW_MARKER: &str = crate::transcript::MORE_BELOW;

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

/// 滚轮落在哪块面板上：目标是行实体时看 [`RowLayout`]，是面板根时看 [`PanelRoots`]。
///
/// 行实体是拾取目标（`transcript::spawn_row_themed` 给行加了 `Pickable`），
/// 所以滚轮消息的 `entity` 正常就是某一行；面板根那条只是兜底（背景可拾取时）。
fn wheel_panel(entity: Entity, roots: &PanelRoots, layouts: &Query<&RowLayout>) -> Option<PanelId> {
    if let Ok(layout) = layouts.get(entity) {
        return Some(layout.panel);
    }
    roots
        .0
        .iter()
        .find_map(|(id, root)| (*root == entity).then_some(*id))
}

/// 滚轮滚转写窗格：一格 [`WHEEL_ROWS`] 行（上游 `input.rs::moused`）。
///
/// 为什么用 `PointerScroll`（`bevy::picking::events`）而不是 `MouseWheel`：
/// 滚轮只该滚指针**下面**那块面板（上游按鼠标所在的窗格滚），`PointerScroll`
/// 带命中实体，落点已经由拾取算好；`MouseWheel` 只有窗口和像素增量，还要
/// 自己拿相机重做一遍拾取才知道指针在哪块面板上。一个 `PointerScroll` 事件
/// 算一格——终端里 crossterm 一次 `ScrollUp`/`ScrollDown` 就是一格；触控板
/// 按像素给的增量不做跨帧累计（上游没有像素滚轮这回事），宁可滚快一点。
///
/// 与 [`pane_keys`] 一样用 [`crate::picker::no_picker`] 让开：picker 的滚轮
/// 归 `picker.rs`，转写不抢。
///
/// 上游 `moused` 在滚之前先 `view.selection = None`：滚走的内容不该留着高亮，
/// 这里同样收起选区（选区资源是可选参数，没有 `SelectionPlugin` 的 App 也能跑）。
#[allow(clippy::too_many_arguments)]
pub fn pane_wheel(
    mut scrolls: MessageReader<PointerScroll>,
    roots: Res<PanelRoots>,
    layouts: Query<&RowLayout>,
    areas: Res<PanelAreas>,
    full: Res<TranscriptRows>,
    mut scroll: ResMut<PaneScroll>,
    mut selection: Option<ResMut<Selection>>,
    tiles: Query<&crate::niri::Tile>,
    tiling: Option<Res<crate::niri::Tiling>>,
) {
    // 行数每帧从当前区域算（不缓存）：resize 之后这一帧读到的就是新行数。
    let height = pane_rows(&areas, &roots, &tiles, tiling.as_deref()).unwrap_or(0);
    for event in scrolls.read() {
        // 横向滚轮（触摸板的左右滑）不是转写的滚动。
        let over_transcript =
            wheel_panel(event.entity, &roots, &layouts) == Some(PanelId::Transcript);
        if event.y == 0.0 || !over_transcript {
            continue;
        }
        // 上游 `moused` 的顺序：先收起选区，再看有没有窗格可滚。
        if let Some(selection) = selection.as_deref_mut() {
            selection.clear();
        }
        if height == 0 {
            continue;
        }
        let max_top = full.len().saturating_sub(height);
        if event.y > 0.0 {
            scroll.wheel_up(max_top);
        } else {
            scroll.wheel_down(max_top);
        }
    }
}

pub struct PanePlugin;

impl Plugin for PanePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TranscriptRows>()
            .init_resource::<PaneScroll>()
            // 生产里 `PickingPlugin` 已经注册了这个消息；这里补上是为了让不建
            // 窗口的测试 App 也能写 `PointerScroll`（`add_message` 幂等）。
            .add_message::<PointerScroll>()
            .add_systems(
                Update,
                pane_keys
                    .in_set(FeathersSystems::Prepare)
                    .run_if(crate::picker::no_picker),
            )
            .add_systems(
                Update,
                pane_wheel
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
    use std::collections::HashMap;

    use bevy::camera::NormalizedRenderTarget;
    use bevy::input::mouse::MouseScrollUnit;
    use bevy::input::touch::TouchPhase;
    use bevy::picking::backend::HitData;
    use bevy::picking::events::Pointer;
    use bevy::picking::pointer::{Location, PointerId};

    use super::*;
    use crate::frame::{CellRect, PanelArea};
    use crate::selection::ClickKind;

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

    // ---- 滚轮：一格 3 行（上游 input.rs::moused 的 pane.scroll.up(3)） ----

    /// 窗顶 `top`、是否跟随。
    fn view(top: usize, follow: bool) -> PaneScroll {
        PaneScroll { top, follow }
    }

    #[test]
    fn a_wheel_notch_moves_three_rows_up_and_stops_following() {
        let mut scroll = view(90, true);
        scroll.wheel_up(90);
        assert_eq!(scroll.top, 87, "一格 3 行");
        assert!(!scroll.follow, "往上滚 = 主动往回看，不再跟随末尾");
        scroll.wheel_up(90);
        assert_eq!(scroll.top, 84);
    }

    #[test]
    fn a_wheel_notch_moves_three_rows_down_and_follows_at_the_bottom() {
        let mut scroll = view(84, false);
        scroll.wheel_down(90);
        assert_eq!(scroll.top, 87);
        assert!(!scroll.follow, "还没到底，继续停在用户滚到的位置");
        scroll.wheel_down(90);
        assert_eq!(scroll.top, 90);
        assert!(scroll.follow, "滚到末尾恢复跟随");
    }

    #[test]
    fn wheel_up_clamps_at_the_top() {
        let mut scroll = view(2, false);
        scroll.wheel_up(90);
        assert_eq!(scroll.top, 0, "到顶就停在 0");
        assert!(!scroll.follow);
        scroll.wheel_up(90);
        assert_eq!(scroll.top, 0, "再往上还是 0");
    }

    #[test]
    fn wheel_down_clamps_at_the_bottom() {
        let mut scroll = view(89, false);
        scroll.wheel_down(90);
        assert_eq!(scroll.top, 90, "夹在末尾，不会超过 max_top");
        assert!(scroll.follow);
    }

    #[test]
    fn wheel_up_from_the_tail_counts_from_the_bottom() {
        // 跟随中窗顶还停在旧位置时，上滚要从末尾算，否则会一下跳到很上面。
        let mut scroll = view(12, true);
        scroll.wheel_up(90);
        assert_eq!(scroll.top, 87);
        assert!(!scroll.follow);
    }

    #[test]
    fn wheel_up_keeps_following_when_everything_fits() {
        // 内容不够一屏（max_top = 0）时没有可回看的：保持跟随末尾
        //（上游 Scroll::place 的 `top >= bottom` 分支）。
        let mut scroll = view(0, false);
        scroll.wheel_up(0);
        assert_eq!(scroll.top, 0);
        assert!(scroll.follow);
    }

    // ---- 尺寸变化后的窗口化 ----

    /// 高 `rows` 行的转写面板区域（列数随便给够）。
    fn pane_area(rows: u16) -> PanelArea {
        PanelArea {
            rect: CellRect::new(0, 0, 80, rows),
            size: Vec2::new(
                80.0 * crate::frame::CELL.x,
                f32::from(rows) * crate::frame::CELL.y,
            ),
            cols: 80,
            rows,
            center: None,
        }
    }

    fn keyed_rows(count: usize) -> Vec<KeyedRow> {
        (0..count)
            .map(|index| KeyedRow {
                key: index as u64,
                hash: index as u64 + 1,
                row: Row::new(Line::from(format!("row {index}"))),
            })
            .collect()
    }

    /// 一块高 `height` 的转写面板 + 整段 `count` 行；换掉 `PanelAreas` 里的区域
    /// 就是一次 resize（[`window_transcript`] 每帧读当前区域，不缓存）。
    fn windowing_app(count: usize, height: u16, top: usize, follow: bool) -> App {
        let mut app = App::new();
        app.init_resource::<PaneScroll>()
            .init_resource::<PanelRows>();
        let root = app.world_mut().spawn_empty().id();
        app.insert_resource(PanelRoots(HashMap::from([(PanelId::Transcript, root)])));
        app.insert_resource(PanelAreas(HashMap::from([(root, pane_area(height))])));
        app.insert_resource(TranscriptRows(keyed_rows(count)));
        app.insert_resource(view(top, follow));
        app.add_systems(Update, window_transcript);
        app
    }

    /// 转写面板这一帧显示的行文本。
    fn visible_rows(app: &App) -> Vec<String> {
        app.world()
            .resource::<PanelRows>()
            .get(PanelId::Transcript)
            .iter()
            .map(|keyed| crate::selection::row_text(&keyed.row))
            .collect()
    }

    #[test]
    fn a_ten_row_viewport_shows_the_last_ten_rows_when_top_is_past_the_bottom() {
        // 100 行、视口 10 行、top=95：末尾是 90，窗口化夹到 90，显示最后 10 行。
        let mut app = windowing_app(100, 10, 95, false);
        app.update();
        let rows = visible_rows(&app);
        assert_eq!(rows.len(), 10, "视口 10 行就显示 10 行");
        assert_eq!(rows.first().map(String::as_str), Some("row 90"));
        assert_eq!(rows.last().map(String::as_str), Some("row 99"));
        assert_eq!(app.world().resource::<PaneScroll>().top, 90, "top 被夹到新末尾");
    }

    #[test]
    fn windowing_recomputes_when_the_panel_row_count_changes() {
        // 面板先 5 行再长到 10 行：每帧按当前区域重算，不拿旧尺寸。
        let mut app = windowing_app(100, 5, 95, true);
        app.update();
        assert_eq!(visible_rows(&app).len(), 5);
        assert_eq!(
            app.world().resource::<PaneScroll>().top,
            95,
            "跟随末尾 = 100 - 5"
        );

        let root = app
            .world()
            .resource::<PanelRoots>()
            .entity(PanelId::Transcript)
            .expect("转写面板根");
        app.world_mut()
            .resource_mut::<PanelAreas>()
            .0
            .insert(root, pane_area(10));
        app.update();

        let rows = visible_rows(&app);
        assert_eq!(rows.len(), 10, "面板变高后这一帧就多显示 5 行");
        assert_eq!(rows.first().map(String::as_str), Some("row 90"));
        assert_eq!(rows.last().map(String::as_str), Some("row 99"));
        assert_eq!(app.world().resource::<PaneScroll>().top, 90);
    }

    // ---- 滚轮接线：不建窗口，直接写 PointerScroll 消息 ----

    struct WheelFixture {
        app: App,
        /// 指针下面那一行（`RowLayout` 指到面板根）。
        row: Entity,
    }

    fn wheel_fixture(panel: PanelId, height: u16, count: usize) -> WheelFixture {
        let mut app = App::new();
        app.init_resource::<TranscriptRows>()
            .init_resource::<PaneScroll>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelAreas>()
            .init_resource::<Selection>();
        app.add_message::<PointerScroll>()
            .add_systems(Update, pane_wheel.run_if(crate::picker::no_picker));
        let root = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<PanelRoots>()
            .0
            .insert(PanelId::Transcript, root);
        app.world_mut()
            .resource_mut::<PanelAreas>()
            .0
            .insert(root, pane_area(height));
        app.world_mut()
            .resource_mut::<TranscriptRows>()
            .0
            .extend(keyed_rows(count));
        let row = app
            .world_mut()
            .spawn(RowLayout {
                panel,
                panel_entity: root,
                index: 0,
            })
            .id();
        WheelFixture { app, row }
    }

    fn mouse_pointer() -> Pointer {
        Pointer::new(
            PointerId::Mouse,
            Location {
                target: NormalizedRenderTarget::None {
                    width: 1280,
                    height: 800,
                },
                position: Vec2::ZERO,
            },
        )
    }

    /// 往指针下面那一行写一条滚轮消息：`y > 0` 上滚，`y < 0` 下滚。
    fn wheel(fixture: &mut WheelFixture, y: f32) {
        fixture.app.world_mut().write_message(PointerScroll {
            entity: fixture.row,
            pointer: mouse_pointer(),
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            phase: TouchPhase::Moved,
        });
    }

    #[test]
    fn a_wheel_over_the_transcript_moves_the_view_three_rows_up() {
        let mut fixture = wheel_fixture(PanelId::Transcript, 10, 100);
        fixture.app.insert_resource(view(90, true));
        wheel(&mut fixture, 1.0);
        fixture.app.update();
        let scroll = *fixture.app.world().resource::<PaneScroll>();
        assert_eq!(scroll.top, 87);
        assert!(!scroll.follow);
    }

    #[test]
    fn a_wheel_down_over_the_transcript_follows_again_at_the_bottom() {
        let mut fixture = wheel_fixture(PanelId::Transcript, 10, 100);
        fixture.app.insert_resource(view(88, false));
        wheel(&mut fixture, -1.0);
        fixture.app.update();
        let scroll = *fixture.app.world().resource::<PaneScroll>();
        assert_eq!(scroll.top, 90);
        assert!(scroll.follow);
    }

    #[test]
    fn a_wheel_outside_the_transcript_leaves_the_view_alone() {
        // 指针在状态行上：滚轮不归转写管（picker 的列表行同理）。
        let mut fixture = wheel_fixture(PanelId::Status, 10, 100);
        fixture.app.insert_resource(view(40, false));
        wheel(&mut fixture, 1.0);
        fixture.app.update();
        let scroll = *fixture.app.world().resource::<PaneScroll>();
        assert_eq!(scroll.top, 40);
        assert!(!scroll.follow);
    }

    #[test]
    fn a_wheel_while_a_picker_is_open_leaves_the_view_alone() {
        let mut fixture = wheel_fixture(PanelId::Transcript, 10, 100);
        fixture.app.insert_resource(view(90, true));
        fixture
            .app
            .world_mut()
            .spawn(rig_harness::prelude::Picker::default());
        wheel(&mut fixture, 1.0);
        fixture.app.update();
        let scroll = *fixture.app.world().resource::<PaneScroll>();
        assert_eq!(scroll.top, 90, "picker 打开时滚轮归 picker.rs");
        assert!(scroll.follow);
    }

    #[test]
    fn a_wheel_over_the_transcript_clears_the_selection() {
        let mut fixture = wheel_fixture(PanelId::Transcript, 10, 100);
        let rows: Vec<Row> = keyed_rows(1).into_iter().map(|keyed| keyed.row).collect();
        {
            let mut selection = fixture.app.world_mut().resource_mut::<Selection>();
            selection.press_kind(PanelId::Transcript, &rows, 0, 0, ClickKind::Single);
            selection.drag(PanelId::Transcript, &rows, 0, 3);
            assert!(selection.has_selection());
        }
        wheel(&mut fixture, 1.0);
        fixture.app.update();
        // 上游 moused：滚轮先 `view.selection = None`。
        assert_eq!(fixture.app.world().resource::<Selection>().panel(), None);
        assert!(!fixture.app.world().resource::<Selection>().has_selection());
    }
}

