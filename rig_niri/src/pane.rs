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
//! 滚动有三个入口，语义一致：
//!
//! - 键盘 PgUp/PgDn 与 Shift+↑/↓（[`pane_keys`]）：一次一页 / 一行；
//! - 键盘 Alt+↑/↓：选上一条 / 下一条目，并把条目滚进可见区域
//!   （上游 `PaneKeys` 的 `SelectUp`/`SelectDown`，对应 `ScrollTo` 的
//!   `ItemRow::Reveal`）；Esc 清掉条目选区，把键交回输入框；
//! - 滚轮（[`pane_wheel`]）：一次 [`WHEEL_ROWS`] 行（上游 `input.rs::moused`
//!   的 `pane.scroll.up(3)`），指针在别的面板上时不动转写。
//!
//! 转写的行是扁平的，但 key 的高 32 位是块序号（`transcript.rs` 的
//! `row_key`），所以一块的行在流里相邻；[`item_rows`] 按它分组，Alt+↑/↓ 选中的
//! 就是“一整块”。选中态存在 [`Selection`]（复用鼠标选区的高亮/复制），条目
//! 身份是 [`Selection::item`]；`sync_item_selection` 每帧把身份换算回当前
//! 可见行。
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
use crate::keys::KeyFocus;
use crate::panels::{KeyedRow, PanelId, PanelRows, RowLayout};
use crate::selection::{Point, Selection};
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

/// 转写里的一个**条目**：一整块（一条消息、一次调用、一条 notice……）在行流里
/// 占的行区间。
///
/// 上游的 `TranscriptItem` 是块的身份；这里没有消息 id，块序号就是身份。
/// `row_key` 是 `ordinal << 32 | index`，所以同一块的行在流里相邻，
/// `key >> 32` 就是块序号（见 [`crate::transcript`] 末尾）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemRows {
    /// 块序号：行 key 的高 32 位。
    pub ordinal: u32,
    /// 这一块的第一行在整段里的下标。
    pub start: usize,
    /// 最后一行的下一行（左闭右开）。
    pub end: usize,
}

impl ItemRows {
    /// 这一块的行区间（左闭右开）。
    pub fn rows(&self) -> std::ops::Range<usize> {
        self.start..self.end
    }
}

/// 把一段扁平的行流按 `key >> 32` 分组，得到每个条目占的行区间（上游
/// `view::ItemRow` 里“行 -> 条目”的换算）。
///
/// “下面还有”标记行的 key 是 `u64::MAX`、不属于任何块，跳过它。
/// 可见窗口可能从一块的中间开始（滚动后），这时第一组也从窗口的第 0 行算起；
/// 组是相对**传入切片**的下标。
pub fn item_rows(rows: &[KeyedRow]) -> Vec<ItemRows> {
    let mut items: Vec<ItemRows> = Vec::new();
    for (index, keyed) in rows.iter().enumerate() {
        if keyed.key == BELOW_MARKER_KEY {
            continue;
        }
        let ordinal = (keyed.key >> 32) as u32;
        match items.last_mut() {
            Some(last) if last.ordinal == ordinal => last.end = index + 1,
            _ => items.push(ItemRows {
                ordinal,
                start: index,
                end: index + 1,
            }),
        }
    }
    items
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

/// “下面还有”标记行的 key：`u64::MAX`，不属于任何块（[`item_rows`] 跳过它）。
const BELOW_MARKER_KEY: u64 = u64::MAX;

fn below_marker() -> KeyedRow {
    KeyedRow {
        key: BELOW_MARKER_KEY,
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

/// 窗口这一帧能放几行**正文**：滚上去时最后一行留给 [`BELOW_MARKER`]。
fn text_rows(follow: bool, top: usize, height: usize, max_top: usize) -> usize {
    if !follow && top < max_top && height > 1 {
        height - 1
    } else {
        height
    }
}

/// Alt+↑/↓：把选中移到上一条 / 下一条，并把它滚进可见区域。
///
/// 对应上游 `input::select` + `ScrollTo { row: ItemRow::Reveal }`：条目第一行
/// 不在窗口里时把它顶到窗顶；还在窗口里就不动滚动位置。上游在两端会取消
/// 选中（“下一条”越过最新一条选中空），这里按任务要求**夹住**在第一条/最后
/// 一条，不越界也不丢选中。还没选中时 ↓ 从第一条开始，↑ 从最新一条开始。
#[allow(clippy::too_many_arguments)]
fn select_pane_item(
    down: bool,
    full: &[KeyedRow],
    roots: &PanelRoots,
    scroll: &mut PaneScroll,
    selection: &mut Selection,
    focus: Option<&mut KeyFocus>,
    height: usize,
    max_top: usize,
) {
    let items = item_rows(full);
    if items.is_empty() {
        return;
    }
    let at = selection
        .item()
        .and_then(|item| items.iter().position(|candidate| candidate.ordinal == item));
    let index = match (at, down) {
        (Some(at), true) => (at + 1).min(items.len() - 1),
        (Some(at), false) => at.saturating_sub(1),
        (None, true) => 0,
        (None, false) => items.len() - 1,
    };
    let item = items[index];
    let top = scroll.top.min(max_top);
    let shown = text_rows(scroll.follow, top, height, max_top);
    if item.start < top || item.start >= top + shown {
        // `PaneScroll::to` 的 `follow = top >= max_top` 与上游 `Scroll::place`
        // 一致：条目顶到末尾时恢复跟随。
        scroll.to(Some(item.start), max_top);
    }
    // 选区落在**当前可见行**上（和鼠标拖选同一套下标）：整段里的行号减窗口
    // 顶。条目比窗口高时只亮露出来的部分；`to` 用下一行的行首（左闭右开）。
    let top = scroll.top.min(max_top);
    let shown = text_rows(scroll.follow, top, height, max_top);
    let from = item.start.saturating_sub(top);
    let to = item
        .end
        .min(top.saturating_add(shown))
        .saturating_sub(top);
    selection.select_item(PanelId::Transcript, from, to, item.ordinal);
    if let Some(focus) = focus
        && let Some(root) = roots.entity(PanelId::Transcript)
    {
        // 拿住键：这样 `editor.rs` 的 Esc 走的是“面板没绑 Esc -> 交还输入框”
        // 分支，而不是输入框作用域里的 Stop（停 agent）。
        focus.0 = Some(root);
    }
}

/// 转写区的键（PgUp/PgDn、Shift+↑/↓、Alt+↑/↓、Esc）。
///
/// 为什么直接读 [`ButtonInput<KeyCode>`] 而不是走 `keys.rs` 的绑定表：
/// 内置绑定只有输入框作用域（[`crate::keys::KeyScope::Input`]）里的
/// `ScrollUp`/`ScrollDown`/`SelectUp`/`SelectDown`，它们经 `editor.rs` 分派后
/// 在本 crate 里是空实现；`KeyScope::Transcript` 也还没有系统会切进去。这里和
/// 原来的 PgUp/PgDn 一样，按键在窗格这一侧处理，动作名与上游一致。
///
/// Esc 只处理“转写里选着条目”的情况；没有选中时什么也不做，留给
/// `editor.rs` 的 Esc（停 agent）。
#[allow(clippy::too_many_arguments)]
pub fn pane_keys(
    keys: Res<ButtonInput<KeyCode>>,
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
    full: Res<TranscriptRows>,
    mut scroll: ResMut<PaneScroll>,
    mut selection: Option<ResMut<Selection>>,
    mut focus: Option<ResMut<KeyFocus>>,
    tiles: Query<&crate::niri::Tile>,
    tiling: Option<Res<crate::niri::Tiling>>,
) {
    // Esc：清掉条目选区。**不在这里清 KeyFocus**：`editor::keys` 也读同一个
    // Esc，只有看到焦点还在转写根上，它才会走面板作用域的“Esc 交还输入框”
    // 分支；在 Prepare 里先清焦点会把它送回输入作用域，Esc 就被当成 Stop
    // （停 agent）。焦点由 `release_transcript_focus` 在窗口化之后交还。
    if keys.just_pressed(KeyCode::Escape)
        && let Some(selection) = selection.as_deref_mut()
        && selection.item().is_some()
    {
        selection.clear();
    }

    let height = pane_rows(&areas, &roots, &tiles, tiling.as_deref()).unwrap_or(0);
    if height == 0 {
        return;
    }
    let max_top = full.len().saturating_sub(height);
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let (up, down) = (
        keys.just_pressed(KeyCode::ArrowUp),
        keys.just_pressed(KeyCode::ArrowDown),
    );

    // Alt+↑/↓：选条目（上游 PaneKeys 的 SelectUp/SelectDown）。
    if alt && (up || down) {
        if let Some(selection) = selection.as_deref_mut() {
            select_pane_item(
                down,
                &full.0,
                &roots,
                &mut scroll,
                selection,
                focus.as_deref_mut(),
                height,
                max_top,
            );
        }
    // Shift+↑/↓：逐行滚（上游 ScrollUp/ScrollDown），一次一行、夹在
    // `[0, max_top]`；不抢 PgUp/PgDn 的一页。
    } else if shift {
        if up {
            scroll.up(1);
        }
        if down {
            scroll.down(1, max_top);
        }
    }

    // PgUp/PgDn 的原行为不变：一次一页。
    let page = scroll.page(height);
    if keys.just_pressed(KeyCode::PageUp) {
        scroll.up(page);
    }
    if keys.just_pressed(KeyCode::PageDown) {
        scroll.down(page, max_top);
    }
}

/// 让键盘选中的条目跟着窗口走：窗口化之后，把条目身份换算回当前可见行。
///
/// [`Selection`] 里的行下标是**可见窗口**的，滚动、折行、resize 后同一块会
/// 换位置；[`Selection::item`] 的块序号不变。所以每帧在 [`window_transcript`]
/// 之后重算一次高亮区间：
///
/// - 条目在窗口里：按它现在占的行重新落选区（鼠标拖选不受影响：`item` 为
///   `None` 时这里直接返回）；
/// - 条目滚出窗口：保留身份、收起高亮（滚回来再亮）；
/// - 块整个消失了（压缩/重排）：清掉选中。
fn sync_item_selection(
    full: Res<TranscriptRows>,
    panels: Res<PanelRows>,
    roots: Res<PanelRoots>,
    mut selection: Option<ResMut<Selection>>,
    mut focus: Option<ResMut<KeyFocus>>,
) {
    // 先只读：鼠标选区（`item == None`）每帧原样返回，不把 `Selection` 标记成
    // 改过，也不去扫整段行。
    let Some(item) = selection.as_deref().and_then(Selection::item) else {
        return;
    };
    if !item_rows(&full.0)
        .iter()
        .any(|candidate| candidate.ordinal == item)
    {
        // 整块没了：键盘选中一起清掉，别指着一个不存在的块。
        if let Some(selection) = selection.as_deref_mut() {
            selection.clear();
        }
        return;
    }
    let visible = item_rows(panels.get(PanelId::Transcript));
    let Some(shown) = visible
        .iter()
        .find(|candidate| candidate.ordinal == item)
        .copied()
    else {
        // 条目滚出窗口：保留身份、收起高亮。已经收起就不重复写。
        let suspended = selection.as_deref().is_some_and(|selection| !selection.is_active());
        if !suspended
            && let Some(selection) = selection.as_deref_mut()
        {
            selection.suspend_item();
        }
        return;
    };
    let needs_reselect = selection.as_deref().is_some_and(|selection| {
        selection.panel() != Some(PanelId::Transcript)
            || selection.anchor() != Point::new(shown.start, 0)
            || selection.focus() != Point::new(shown.end, 0)
    });
    if needs_reselect
        && let Some(selection) = selection.as_deref_mut()
    {
        selection.select_item(PanelId::Transcript, shown.start, shown.end, item);
    }
    if let Some(focus) = focus.as_deref_mut()
        && let Some(root) = roots.entity(PanelId::Transcript)
    {
        focus.0 = Some(root);
    }
}

/// 条目选中没了就把 [`KeyFocus`] 还给输入框（`None`）。
///
/// 放在 `Fit`、[`pane_keys`] 之后而不是按键处理里：见 [`pane_keys`] 的注释
/// —— Esc 的“交还输入框”必须让 `editor::keys` 先看到焦点还在转写根上。这里
/// 同时兜住鼠标点击、滚轮、resize 清掉选中之后把键还给输入框。
fn release_transcript_focus(
    roots: Option<Res<PanelRoots>>,
    selection: Option<Res<Selection>>,
    mut focus: Option<ResMut<KeyFocus>>,
) {
    let Some(root) = roots
        .as_deref()
        .and_then(|roots| roots.entity(PanelId::Transcript))
    else {
        return;
    };
    let Some(focus) = focus.as_deref_mut() else {
        return;
    };
    if focus.0 != Some(root) {
        return;
    }
    if selection.as_deref().and_then(Selection::item).is_none() {
        focus.0 = None;
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
                (
                    window_transcript,
                    sync_item_selection,
                    release_transcript_focus,
                )
                    .chain()
                    .in_set(FeathersSystems::Fit),
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

    // ---- 条目身份 / Alt+↑↓ 选条目 / Shift+↑↓ 行滚 / Esc ----

    /// 一个块的行：key 的高 32 位 = 块序号，低 32 位 = 块内行号。
    fn block_rows(ordinal: u32, lines: &[&str]) -> Vec<KeyedRow> {
        lines
            .iter()
            .enumerate()
            .map(|(index, text)| KeyedRow {
                key: (u64::from(ordinal) << 32) | index as u64,
                hash: u64::from(ordinal) * 100 + index as u64,
                row: Row::new(Line::from((*text).to_owned())),
            })
            .collect()
    }

    /// `count` 个只有一行的块。
    fn single_row_blocks(count: usize) -> Vec<KeyedRow> {
        (0..count)
            .map(|ordinal| KeyedRow {
                key: (ordinal as u64) << 32,
                hash: ordinal as u64 + 1,
                row: Row::new(Line::from(format!("row {ordinal}"))),
            })
            .collect()
    }

    struct KeysFixture {
        app: App,
        root: Entity,
    }

    fn keys_app_rows(rows: Vec<KeyedRow>, height: u16) -> KeysFixture {
        let mut app = App::new();
        app.init_resource::<TranscriptRows>()
            .init_resource::<PaneScroll>()
            .init_resource::<PanelRows>()
            .init_resource::<PanelAreas>()
            .init_resource::<PanelRoots>()
            .init_resource::<Selection>()
            .init_resource::<KeyFocus>()
            .init_resource::<ButtonInput<KeyCode>>();
        let root = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<PanelRoots>()
            .0
            .insert(PanelId::Transcript, root);
        app.world_mut()
            .resource_mut::<PanelAreas>()
            .0
            .insert(root, pane_area(height));
        app.world_mut().resource_mut::<TranscriptRows>().0 = rows;
        // 与 `PanePlugin` 的 Fit 顺序一致：先窗口化，再按可见行重算条目高亮，
        // 最后在选中没了时交还焦点。
        app.add_systems(
            Update,
            (
                pane_keys,
                window_transcript,
                sync_item_selection,
                release_transcript_focus,
            )
                .chain(),
        );
        // 先跑一帧把窗口/跟随位置摆好。
        app.update();
        KeysFixture { app, root }
    }

    fn keys_app(blocks: &[&[&str]], height: u16) -> KeysFixture {
        let rows = blocks
            .iter()
            .enumerate()
            .flat_map(|(ordinal, lines)| block_rows(ordinal as u32, lines))
            .collect();
        keys_app_rows(rows, height)
    }

    /// 按一次键：`modifiers` 按住，`key` 这一帧按下。
    fn tap(app: &mut App, modifiers: &[KeyCode], key: KeyCode) {
        {
            // 松开上一帧按下的键：`press` 只有从“没按”变“按下”才进
            // `just_pressed`。
            let mut held = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            held.reset_all();
            for modifier in modifiers {
                held.press(*modifier);
            }
            held.press(key);
        }
        app.update();
        // 真实 App 的 `InputPlugin` 每帧开头会清 `just_pressed`；测试 App 没有，
        // 手动清掉，免得下一次 `app.update()` 又把同一个按键当成“刚按下”。
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
    }

    fn alt_tap(app: &mut App, key: KeyCode) {
        tap(app, &[KeyCode::AltLeft], key);
    }

    fn shift_tap(app: &mut App, key: KeyCode) {
        tap(app, &[KeyCode::ShiftLeft], key);
    }

    fn visible_values(app: &App) -> Vec<Row> {
        app.world()
            .resource::<PanelRows>()
            .row_values(PanelId::Transcript)
    }

    fn selection(app: &App) -> &Selection {
        app.world().resource::<Selection>()
    }

    fn scroll(app: &App) -> PaneScroll {
        *app.world().resource::<PaneScroll>()
    }

    fn focus(app: &App) -> KeyFocus {
        *app.world().resource::<KeyFocus>()
    }

    #[test]
    fn item_rows_group_by_the_high_key_bits_and_skip_the_below_marker() {
        let mut rows = block_rows(0, &["a", "b"]);
        rows.extend(block_rows(1, &["c", "d", "e"]));
        let mut with_marker = rows.clone();
        with_marker.push(below_marker());
        assert_eq!(
            item_rows(&with_marker),
            vec![
                ItemRows {
                    ordinal: 0,
                    start: 0,
                    end: 2
                },
                ItemRows {
                    ordinal: 1,
                    start: 2,
                    end: 5
                },
            ],
            "块序号 = key >> 32；下面还有标记不是条目"
        );

        // 窗口从一块的中间开始：行号是相对传入切片的。
        let window = rows[1..4].to_vec();
        assert_eq!(
            item_rows(&window),
            vec![
                ItemRows {
                    ordinal: 0,
                    start: 0,
                    end: 1
                },
                ItemRows {
                    ordinal: 1,
                    start: 1,
                    end: 3
                },
            ]
        );
        assert_eq!(item_rows(&window)[1].rows(), 1..3);
    }

    #[test]
    fn alt_down_selects_items_in_order_and_alt_up_walks_back_without_overshooting() {
        let mut fixture = keys_app(&[&["one"][..], &["two"][..], &["three"][..]], 10);

        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(selection(&fixture.app).item(), Some(0));
        assert_eq!(selection(&fixture.app).anchor(), Point::new(0, 0));
        assert_eq!(selection(&fixture.app).focus(), Point::new(1, 0));
        assert_eq!(
            selection(&fixture.app)
                .selected_text(&visible_values(&fixture.app))
                .as_deref(),
            Some("one")
        );
        assert_eq!(
            focus(&fixture.app).0,
            Some(fixture.root),
            "选条目时把键拿在转写根上（Esc 才能交还输入框）"
        );

        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(selection(&fixture.app).item(), Some(1));
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(selection(&fixture.app).item(), Some(2));

        // 到底不越界、不丢选中（上游在两端会取消选中，任务要求夹住）。
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(selection(&fixture.app).item(), Some(2));

        alt_tap(&mut fixture.app, KeyCode::ArrowUp);
        assert_eq!(selection(&fixture.app).item(), Some(1));
        alt_tap(&mut fixture.app, KeyCode::ArrowUp);
        assert_eq!(selection(&fixture.app).item(), Some(0));
        alt_tap(&mut fixture.app, KeyCode::ArrowUp);
        assert_eq!(selection(&fixture.app).item(), Some(0), "到顶也不越界");
    }

    #[test]
    fn a_selected_item_puts_all_of_its_rows_in_the_selection_and_no_others() {
        let mut fixture = keys_app(
            &[
                &["user 1", "user 2"][..],
                &["answer a", "answer b", "answer c"][..],
                &["notice"][..],
            ],
            10,
        );
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);

        let rows = visible_values(&fixture.app);
        let selection = selection(&fixture.app);
        assert_eq!(selection.item(), Some(1));
        assert_eq!(selection.anchor(), Point::new(2, 0));
        assert_eq!(selection.focus(), Point::new(5, 0), "左闭右开，最后一行整行");
        assert_eq!(
            selection.selected_text(&rows).as_deref(),
            Some("answer a\nanswer b\nanswer c")
        );
        assert_eq!(
            selection.all_ranges(&rows),
            vec![(2, vec![(0, 8)]), (3, vec![(0, 8)]), (4, vec![(0, 8)])],
            "只有这一块的三行"
        );
        for other in [0, 1, 5] {
            assert_eq!(
                selection.ranges_in(other, &rows),
                Vec::<(usize, usize)>::new(),
                "别的块 / 窗口外不进选区"
            );
        }
    }

    #[test]
    fn selecting_an_item_scrolls_it_into_view() {
        // 20 个一行块、视口 5 行：初始跟随末尾 top = 15。
        let mut fixture = keys_app_rows(single_row_blocks(20), 5);
        assert_eq!(scroll(&fixture.app).top, 15);
        assert!(scroll(&fixture.app).follow);

        // Alt+↓ 从第一条开始：它在窗口上面，要滚到窗顶。
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(selection(&fixture.app).item(), Some(0));
        assert_eq!(scroll(&fixture.app).top, 0, "条目 0 被顶到窗顶");
        assert!(!scroll(&fixture.app).follow, "往回看 = 不再跟随末尾");
        assert_eq!(
            crate::selection::row_text(&visible_values(&fixture.app)[0]),
            "row 0"
        );

        // 一路 ↓ 到第 6 条：第 4 条不在窗口里时窗口才下移，之后条目 5 可见。
        for _ in 0..5 {
            alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        }
        assert_eq!(selection(&fixture.app).item(), Some(5));
        let top = scroll(&fixture.app).top;
        assert_eq!(top, 4);
        assert!(top <= 5 && 5 < top + 5, "选中的条目在可见窗口里");
    }

    #[test]
    fn alt_up_with_no_selection_starts_at_the_newest_item() {
        let mut fixture = keys_app_rows(single_row_blocks(20), 5);
        alt_tap(&mut fixture.app, KeyCode::ArrowUp);
        assert_eq!(selection(&fixture.app).item(), Some(19), "没选中时 ↑ = 最新一条");
        assert_eq!(scroll(&fixture.app).top, 15);
        assert!(selection(&fixture.app).has_selection());
    }

    #[test]
    fn escape_clears_the_item_selection_and_gives_the_keys_back_to_the_input() {
        let mut fixture = keys_app(&[&["one"][..], &["two"][..]], 10);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert!(selection(&fixture.app).has_selection());
        assert_eq!(focus(&fixture.app).0, Some(fixture.root));

        tap(&mut fixture.app, &[], KeyCode::Escape);
        assert_eq!(selection(&fixture.app).item(), None);
        assert!(!selection(&fixture.app).has_selection());
        assert_eq!(
            focus(&fixture.app).0,
            None,
            "Esc 把 KeyFocus 还给输入框（None）"
        );

        // 没有选中时 Esc 不动转写：停 agent 的语义在 editor.rs。
        tap(&mut fixture.app, &[], KeyCode::Escape);
        assert_eq!(focus(&fixture.app).0, None);
        assert_eq!(selection(&fixture.app).item(), None);
    }

    #[test]
    fn a_mouse_click_takes_the_keys_back_from_the_transcript() {
        let mut fixture = keys_app(&[&["hello world"][..]], 10);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(focus(&fixture.app).0, Some(fixture.root));

        // 鼠标点击走的是 selection.rs 的 press_kind：条目身份没了，焦点也要
        // 跟着还回输入框，否则点完之后打不了字。
        let rows = visible_values(&fixture.app);
        fixture
            .app
            .world_mut()
            .resource_mut::<Selection>()
            .press_kind(PanelId::Transcript, &rows, 0, 2, ClickKind::Single);
        fixture.app.update();
        assert_eq!(selection(&fixture.app).item(), None);
        assert_eq!(focus(&fixture.app).0, None);
    }

    #[test]
    fn a_vanished_block_clears_the_item_selection() {
        let mut fixture = keys_app(&[&["one"][..], &["two"][..]], 10);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(selection(&fixture.app).item(), Some(0));

        // 块 0 被压缩/重排掉：身份不存在了，选中和焦点都要收掉。
        fixture.app.world_mut().resource_mut::<TranscriptRows>().0 =
            block_rows(1, &["two"]);
        fixture.app.update();
        assert_eq!(selection(&fixture.app).item(), None);
        assert!(!selection(&fixture.app).has_selection());
        assert_eq!(focus(&fixture.app).0, None);
    }

    #[test]
    fn shift_arrows_scroll_one_row_and_clamp_to_the_viewport() {
        // 10 行、视口 3 行：max_top = 7。
        let mut fixture = keys_app_rows(single_row_blocks(10), 3);
        assert_eq!(scroll(&fixture.app).top, 7);
        assert!(scroll(&fixture.app).follow);

        shift_tap(&mut fixture.app, KeyCode::ArrowUp);
        assert_eq!(scroll(&fixture.app).top, 6, "一次一行，不是一页（3-1=2 行）");
        assert!(!scroll(&fixture.app).follow);
        shift_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(scroll(&fixture.app).top, 7);
        assert!(scroll(&fixture.app).follow, "滚回末尾恢复跟随");

        for _ in 0..20 {
            shift_tap(&mut fixture.app, KeyCode::ArrowUp);
        }
        assert_eq!(scroll(&fixture.app).top, 0, "顶部夹在 0");
        assert!(!scroll(&fixture.app).follow);

        for _ in 0..20 {
            shift_tap(&mut fixture.app, KeyCode::ArrowDown);
        }
        assert_eq!(scroll(&fixture.app).top, 7, "底部夹在 max_top");
        assert!(scroll(&fixture.app).follow);
    }

    #[test]
    fn page_up_and_page_down_still_move_a_page() {
        // 100 行、视口 10 行：一页 = 9 行，max_top = 90。
        let mut fixture = keys_app_rows(single_row_blocks(100), 10);
        assert_eq!(scroll(&fixture.app).top, 90);

        tap(&mut fixture.app, &[], KeyCode::PageUp);
        assert_eq!(scroll(&fixture.app).top, 81);
        assert!(!scroll(&fixture.app).follow);

        tap(&mut fixture.app, &[], KeyCode::PageDown);
        assert_eq!(scroll(&fixture.app).top, 90);
        assert!(scroll(&fixture.app).follow);

        for _ in 0..20 {
            tap(&mut fixture.app, &[], KeyCode::PageUp);
        }
        assert_eq!(scroll(&fixture.app).top, 0, "PgUp 到顶夹住");
    }

    #[test]
    fn a_keyboard_item_follows_the_rows_after_paging_and_scrolling_back() {
        let mut fixture = keys_app_rows(single_row_blocks(20), 5);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(selection(&fixture.app).item(), Some(0));
        assert_eq!(scroll(&fixture.app).top, 0);

        // PgDn 把条目 0 滚出窗口：保留身份、收起高亮，但不丢选中。
        tap(&mut fixture.app, &[], KeyCode::PageDown);
        assert_eq!(selection(&fixture.app).item(), Some(0));
        assert!(!selection(&fixture.app).has_selection(), "滚出窗口时不画高亮");
        assert_eq!(
            focus(&fixture.app).0,
            Some(fixture.root),
            "条目还选着，键还在转写上"
        );

        // PgUp 滚回来：同一个条目重新亮起来。
        tap(&mut fixture.app, &[], KeyCode::PageUp);
        assert_eq!(selection(&fixture.app).item(), Some(0));
        assert!(selection(&fixture.app).has_selection());
        assert_eq!(selection(&fixture.app).anchor(), Point::new(0, 0));
    }
}

