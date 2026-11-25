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
//! 转写的行是扁平的，但 key 的高 32 位是块序号（引擎的
//! `transcript::row_key` 是 `ordinal << 32 | index`，实时块的
//! `batch::live_key` 是同一约定：高 32 位 = 调用/块身份、低 32 位 = 块内行号），
//! 所以一块的行在流里相邻；[`item_rows`] 按它分组，Alt+↑/↓ 选中的
//! 就是“一整块”。选中态存在 [`Selection`]（复用鼠标选区的高亮/复制），条目
//! 身份是 [`Selection::item`]；`sync_item_selection` 每帧把身份换算回当前
//! 可见行。
//!
//! 选中条目会把 [`KeyFocus`] 拿到转写根上（Esc 走面板作用域的“交还输入框”，
//! 而不是输入框作用域的 Stop）。**交还只发生在显式动作之后**：Esc、鼠标点
//! 输入面板（`selection::pointer_selection`）、条目滚出窗口收起高亮
//! （[`release_transcript_focus`]）；[`sync_item_selection`] 是被动同步，
//! 不再每帧把焦点抢回转写根 —— 否则点完输入框下一帧就被抢回去，字符键全被
//! 面板作用域丢掉（用户可见：打字没反应、也没有提示）。
//!
//! 往上滚是用户主动往回看，取消 [`PaneScroll::follow`]；滚到末尾恢复跟随。
//! 窗口/面板尺寸变化不在本模块缓存：窗口化每帧从 [`PanelAreas`]（或 Niri 的
//! 分裂树）读当前行数，resize 后的下一帧就是新尺寸（清选区在
//! [`crate::selection::clear_selection_on_resize`]）。

use bevy::picking::Pickable;
use bevy::picking::events::{PointerDrag, PointerDragEnd, PointerDragStart, PointerScroll};
use bevy::prelude::*;
use bevy::ui::{BorderRadius, ComputedNode, Node, percent, px};
use bevy_feathers::theme::{ThemeBackgroundColor, UiTheme};
use bevy_feathers::tokens;
use rig_tui::TranscriptSource;
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};

use crate::frame::{PANEL_PADDING, PanelAreas, PanelRoots};
use crate::keys::KeyFocus;
use crate::panels::{KeyedRow, PanelId, PanelRows, RowLayout};
use crate::selection::{Point, Selection};
use crate::theme::{
    SCROLLBAR_MIN_THUMB, SCROLLBAR_RADIUS, SCROLLBAR_WIDTH, ui_theme_or_default,
};
use crate::transcript::{Below, BelowSeen, ROW_HEIGHT, Row};
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
/// 两种产行方用同一个约定（高 32 位 = 块身份、低 32 位 = 块内行号）：引擎是
/// `transcript::row_key` 的 `ordinal << 32 | index`，实时内容是
/// `batch::live_key` 的 `段标记 | part 哈希 << 32 | index`。所以一条消息 /
/// 一次调用 / 一段流式回复各自一个条目，Alt+↑/↓ 一下一条。
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

/// 一块转写窗格的滚动：窗顶是第几行，是否跟着末尾（rig-tui 的 `Scroll`）。
///
/// S1 之后它主要是 [`TranscriptPane::scroll`] 的字段类型（每块窗格各一份）。
/// 同时保留 `Resource` 只是为了不逼 `editor.rs` 改：发送消息时它仍然写
/// [`PaneScroll::follow`]（见 [`window_transcript`] 里的兼容入口）。**资源不是
/// 状态的第二份副本**：只在资源被改的那一帧把 `top`/`follow` 交给主转写，
/// 之后由窗格自己的字段负责，没有任何每帧反向同步。
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

// ---------------------------------------------------------------------------
// 滚动条（F4）：窗格右侧一条纯 `Node` 的轨道 + 滑块
// ---------------------------------------------------------------------------
//
// 位置/长度都按**像素**算，和 picker 的滚动条（`picker::sync_picker_scrollbar`）
// 用同一套几何常量：轨道高 = 可见行数 × [`ROW_HEIGHT`]，正好覆盖行栈；宽度 /
// 圆角 / 最短滑块取 [`crate::theme`] 的 `SCROLLBAR_*`（feathers listview 的
// 6px / 3px / 8px）。轨道绝对定位在面板**右侧 padding 里**（taffy 里
// `right: 0` 是 padding box 的右缘），所以**不占转写列宽**：正文的 `area.cols`
// 不算它，右对齐的 `↓ more below` / `↓ new output` 也不会被盖住。
//
// 状态来源只有一份：每块窗格自己的 [`TranscriptPane::scroll`]。窗口化在
// [`window_transcript`] 里已经把 `top`/`follow` 夹好，[`sync_transcript_scrollbars`]
// 排在它后面，只读不算；拖拽观察者直接改 `scroll`，下一帧窗口化再夹一次。

/// 滚动条这一帧的几何模型（纯数据，headless 测试直接断言）。
///
/// `len`/`offset` 是比例，`track_px`/`thumb_px` 是 UI 逻辑像素。内容放得下时
/// 滑块铺满轨道（`len == 1`、`offset == 0`），用轨道色画，看起来就是一条
/// 普通竖条；内容溢出时 `len = 可见/总数`（至少 [`SCROLLBAR_MIN_THUMB`]），
/// `offset = top / max_top`，`offset == 1` 时滑块贴底。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarModel {
    /// 整段行数。
    pub total: usize,
    /// 这一帧可见的行数（[`crate::frame::PanelArea::rows`] / Niri tile 的行数）。
    pub visible: usize,
    /// 实际窗顶：跟随末尾时就是 `max_top`。
    pub top: usize,
    /// `total - visible`（不小于 0）。
    pub max_top: usize,
    /// 滑块长度 / 轨道长度（0..=1）。
    pub len: f32,
    /// 滑块在可滑动行程上的位置（0 = 顶，1 = 底）。
    pub offset: f32,
    /// 内容比可见行多：滑块用强调色，否则和轨道同色。
    pub overflow: bool,
    /// 轨道长度（UI 逻辑像素）= 可见行数 × [`ROW_HEIGHT`]。
    pub track_px: f32,
    /// 滑块长度（UI 逻辑像素）。
    pub thumb_px: f32,
}

impl Default for ScrollbarModel {
    fn default() -> Self {
        Self {
            total: 0,
            visible: 0,
            top: 0,
            max_top: 0,
            len: 0.0,
            offset: 0.0,
            overflow: false,
            track_px: 0.0,
            thumb_px: 0.0,
        }
    }
}

impl ScrollbarModel {
    /// 从「总行数 / 可见行数 / 窗顶 / 是否跟随末尾」算这一帧的滑块。
    pub fn new(total: usize, visible: usize, top: usize, follow: bool) -> Self {
        let max_top = total.saturating_sub(visible);
        let top = if follow { max_top } else { top.min(max_top) };
        let track_px = visible as f32 * ROW_HEIGHT;
        let overflow = visible > 0 && total > visible;
        let thumb_px = if !overflow || track_px <= 0.0 {
            // 放得下：滑块铺满，不画“半个滑块”。
            track_px
        } else {
            (track_px * visible as f32 / total as f32)
                .max(SCROLLBAR_MIN_THUMB)
                .min(track_px)
        };
        let len = if track_px > 0.0 {
            thumb_px / track_px
        } else {
            0.0
        };
        let travel = (track_px - thumb_px).max(0.0);
        let offset = if max_top == 0 || travel <= 0.0 {
            0.0
        } else {
            (top as f32 / max_top as f32).clamp(0.0, 1.0)
        };
        Self {
            total,
            visible,
            top,
            max_top,
            len,
            offset,
            overflow,
            track_px,
            thumb_px,
        }
    }

    /// 滑块上边缘距轨道顶部的像素。
    pub fn thumb_top_px(&self) -> f32 {
        self.offset * (self.track_px - self.thumb_px).max(0.0)
    }

    /// 滑块下边缘是否贴住轨道底部（跟随时 / 滚到底）。
    pub fn is_at_bottom(&self) -> bool {
        (self.thumb_top_px() + self.thumb_px - self.track_px).abs() < 0.5
    }
}

/// 把滑块拖 `distance_y`（屏幕像素，向下为正）后应落的窗顶行。
///
/// 按滑块行程比例换算：拖过整段行程 = 从 `0` 滚到 `max_top`，结果夹在
/// `[0, max_top]`。`distance_y` 是相对按下点的总位移（`PointerDrag::distance`），
/// 所以起点用按下时的 `start_top`，不做增量累加。
pub fn dragged_top(model: &ScrollbarModel, start_top: usize, distance_y: f32) -> usize {
    if model.max_top == 0 {
        return 0;
    }
    let travel = model.track_px - model.thumb_px;
    if !(travel > 0.0) {
        return start_top.min(model.max_top);
    }
    let delta = distance_y / travel * model.max_top as f32;
    (start_top as f32 + delta)
        .round()
        .clamp(0.0, model.max_top as f32) as usize
}

/// 转写窗格右侧滚动条的**轨道**（纯 `Node`，挂在窗格根下）。
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct TranscriptScrollbar {
    /// 这块滚动条属于哪块窗格根。
    pub pane: Entity,
    /// 滑块实体。
    pub thumb: Entity,
    /// 上一帧算出的几何（拖拽观察者直接复用，不再重复量布局）。
    pub model: ScrollbarModel,
}

/// 滚动条滑块实体（`TranscriptScrollbar::thumb`）。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TranscriptScrollThumb {
    /// 这块滑块服务的窗格根。
    pub pane: Entity,
    /// 它所在的轨道。
    pub track: Entity,
}

/// 正在拖的滑块。
///
/// 用资源而不是组件：`PointerDragStart` / `PointerDrag` 由 bevy_picking 在
/// 同一帧里连续派发，观察者里写资源是**立即**生效的（写组件要等命令队列），
/// 第一段拖动就不会丢。
#[derive(Resource, Default, Debug)]
pub struct TranscriptScrollbarDrag {
    state: Option<ScrollDrag>,
}

#[derive(Clone, Copy, Debug)]
struct ScrollDrag {
    thumb: Entity,
    pane: Entity,
    start_top: usize,
    model: ScrollbarModel,
    /// 屏幕像素 / UI 逻辑像素（高分屏下 `PointerDrag::distance` 是物理像素）。
    scale: f32,
}

/// 给新出现的转写窗格（主转写 / `Pane2`）挂一条滚动条。
///
/// `spawn_panel`（`frame.rs`）只负责面板本身，这里用 `Add<TranscriptPane>`
/// 观察者补滚动条：两块转写走同一条路，`panels.rs` 的行渲染不用知道它。
fn spawn_transcript_scrollbar(
    added: On<Add<TranscriptPane>>,
    theme: Option<Res<UiTheme>>,
    mut commands: Commands,
) {
    let theme = ui_theme_or_default(theme.as_deref());
    let track_bg = theme.color(&tokens::SCROLLBAR_BG);
    let thumb_bg = theme.color(&tokens::SCROLLBAR_THUMB);
    let pane = added.entity;
    let track = commands
        .spawn((
            Name::new("transcript:scrollbar"),
            Node {
                position_type: PositionType::Absolute,
                // taffy 的绝对定位以 padding box 为基准：right 0 = 内边框
                // 右缘，6px 轨道落在右侧 8px padding 里，不压正文列宽。
                right: px(0.0),
                top: px(PANEL_PADDING),
                width: px(SCROLLBAR_WIDTH),
                // 高度/位置每帧由 `sync_transcript_scrollbars` 按行数写。
                height: px(0.0),
                border_radius: BorderRadius::all(px(SCROLLBAR_RADIUS)),
                ..default()
            },
            BackgroundColor(track_bg),
            ThemeBackgroundColor(tokens::SCROLLBAR_BG),
            Pickable::default(),
            ChildOf(pane),
        ))
        .id();
    let thumb = commands
        .spawn((
            Name::new("transcript:scrollbar-thumb"),
            Node {
                position_type: PositionType::Absolute,
                left: px(0.0),
                top: px(0.0),
                width: percent(100.0),
                height: px(0.0),
                border_radius: BorderRadius::all(px(SCROLLBAR_RADIUS)),
                ..default()
            },
            BackgroundColor(thumb_bg),
            ThemeBackgroundColor(tokens::SCROLLBAR_THUMB),
            Pickable::default(),
            ChildOf(track),
            TranscriptScrollThumb { pane, track },
        ))
        .id();
    commands.entity(track).insert(TranscriptScrollbar {
        pane,
        thumb,
        model: ScrollbarModel::default(),
    });
}

/// 每帧把每块窗格的轨道/滑块摆到「`scroll.top` / 总行数 / 可见行数」上。
///
/// 排在 [`window_transcript`] 之后（`Fit` 里 chain）：窗口化已经夹好
/// `scroll.top`，这里只读。`follow` 时 `ScrollbarModel::new` 自己把有效窗顶
/// 放在 `max_top`，滑块贴底——即使这一帧 `window_transcript` 还没跑。
#[allow(clippy::too_many_arguments)]
pub fn sync_transcript_scrollbars(
    areas: Res<PanelAreas>,
    full: Res<TranscriptRows>,
    theme: Option<Res<UiTheme>>,
    mut tracks: Query<(&mut TranscriptScrollbar, &mut Node, &mut BackgroundColor)>,
    mut thumbs: Query<
        (&mut Node, &mut BackgroundColor),
        (With<TranscriptScrollThumb>, Without<TranscriptScrollbar>),
    >,
    panes: Query<&TranscriptPane>,
    tiles: Query<&crate::niri::Tile>,
    tiling: Option<Res<crate::niri::Tiling>>,
) {
    let theme = ui_theme_or_default(theme.as_deref());
    let track_color = theme.color(&tokens::SCROLLBAR_BG);
    let thumb_color = theme.color(&tokens::SCROLLBAR_THUMB);
    for (mut bar, mut track_node, mut track_bg) in &mut tracks {
        let Ok(pane) = panes.get(bar.pane) else {
            continue;
        };
        let Some((_, visible)) = pane_size_of(&areas, bar.pane, &tiles, tiling.as_deref()) else {
            continue;
        };
        let model = ScrollbarModel::new(
            full.len(),
            usize::from(visible),
            pane.scroll.top,
            pane.scroll.follow,
        );
        if bar.model != model {
            bar.model = model;
        }
        let track_top = px(PANEL_PADDING);
        if track_node.top != track_top {
            track_node.top = track_top;
        }
        let track_height = px(model.track_px);
        if track_node.height != track_height {
            track_node.height = track_height;
        }
        if track_bg.0 != track_color {
            track_bg.0 = track_color;
        }
        let Ok((mut thumb_node, mut thumb_bg)) = thumbs.get_mut(bar.thumb) else {
            continue;
        };
        let thumb_top = px(model.thumb_top_px());
        if thumb_node.top != thumb_top {
            thumb_node.top = thumb_top;
        }
        let thumb_height = px(model.thumb_px);
        if thumb_node.height != thumb_height {
            thumb_node.height = thumb_height;
        }
        // 放得下时滑块铺满轨道、和轨道同色，看起来就是一条普通竖条。
        let want_thumb = if model.overflow {
            thumb_color
        } else {
            track_color
        };
        if thumb_bg.0 != want_thumb {
            thumb_bg.0 = want_thumb;
        }
    }
}

/// 按下滑块：记下起点窗顶与行程，后续 `PointerDrag` 按总位移换算。
fn begin_scrollbar_drag(
    drag: On<PointerDragStart>,
    thumbs: Query<&TranscriptScrollThumb>,
    bars: Query<&TranscriptScrollbar>,
    panes: Query<&TranscriptPane>,
    nodes: Query<&ComputedNode>,
    mut state: ResMut<TranscriptScrollbarDrag>,
) {
    let Ok(thumb) = thumbs.get(drag.entity) else {
        return;
    };
    let Ok(bar) = bars.get(thumb.track) else {
        return;
    };
    let start_top = panes
        .get(thumb.pane)
        .map(|pane| {
            if pane.scroll.follow {
                bar.model.max_top
            } else {
                pane.scroll.top.min(bar.model.max_top)
            }
        })
        .unwrap_or(bar.model.top);
    // 高分屏上 `PointerDrag::distance` 是屏幕物理像素，模型是 UI 逻辑像素；
    // 用轨道真实的 `ComputedNode` 高度校正（headless 测试没有布局时为 1）。
    let scale = nodes
        .get(thumb.track)
        .ok()
        .map(|node| node.size().y / bar.model.track_px)
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(1.0);
    state.state = Some(ScrollDrag {
        thumb: drag.entity,
        pane: thumb.pane,
        start_top,
        model: bar.model,
        scale,
    });
}

/// 拖滑块：按 [`dragged_top`] 算窗顶，滚到底恢复跟随（[`PaneScroll::to`]）。
fn drag_scrollbar(
    drag: On<PointerDrag>,
    state: Res<TranscriptScrollbarDrag>,
    mut panes: Query<&mut TranscriptPane>,
) {
    let Some(scroll) = state.state else {
        return;
    };
    if scroll.thumb != drag.entity {
        return;
    }
    let Ok(mut pane) = panes.get_mut(scroll.pane) else {
        return;
    };
    let top = dragged_top(
        &scroll.model,
        scroll.start_top,
        drag.distance.y / scroll.scale,
    );
    pane.scroll.to(Some(top), scroll.model.max_top);
}

/// 松开滑块：忘掉这次拖拽的起点。
fn end_scrollbar_drag(
    drag: On<PointerDragEnd>,
    mut state: ResMut<TranscriptScrollbarDrag>,
) {
    if state.state.is_some_and(|scroll| scroll.thumb == drag.entity) {
        state.state = None;
    }
}

/// 一块转写窗格（`PanelId::Transcript` / `PanelId::Pane2` 的根实体）的全部
/// 逐窗格状态。
///
/// 上游同名组件（rig-tui `pane.rs:57`）只有 `source`/`title`；我们的滚动与条目
/// 选中原本是全局 `PaneScroll` + `Selection`，S1 把每块窗格自己的那几份收到这里。
/// `title`/`selected` 在 S1 还没有读取方（只是先把归属搬对），行为与改动前一致。
#[derive(Component, Clone, Debug)]
pub struct TranscriptPane {
    /// 这块窗格画谁。S1 里两块都是 [`TranscriptSource::Shown`]（`Pane2` 仍由
    /// [`crate::panels`] 的镜像填行），S2 才按来源产行。
    pub source: TranscriptSource,
    /// 窗格第一行的标题。`None` = 不占行（S1 不读它，保证画面零变化）。
    pub title: Option<String>,
    /// 这块窗格自己的滚动（窗顶 + 是否跟随末尾）。
    pub scroll: PaneScroll,
    /// 这块窗格自己的条目选中（`Selection::item` 的块序号）。
    pub selected: Option<u32>,
    /// 这块窗格自己的“下面还有”状态：`Pane2` 与主窗格的滚动位置可以不同，
    /// 标记文案（more below / new output）也必须各记各的。
    pub below: BelowSeen,
}

impl Default for TranscriptPane {
    fn default() -> Self {
        Self {
            source: TranscriptSource::Shown,
            title: None,
            scroll: PaneScroll::default(),
            selected: None,
            below: BelowSeen::default(),
        }
    }
}

impl TranscriptPane {
    /// 一块画 `source`、没有标题行的窗格。
    pub fn new(source: TranscriptSource) -> Self {
        Self {
            source,
            ..Self::default()
        }
    }

    /// 同一块窗格，带上标题行（S3 才画）。
    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

/// 当前活动转写窗格：`pane_keys` / `pane_wheel` / 条目选择作用于它。
///
/// `None` 或指向已经不在当前工作副本里的实体时，退回
/// `PanelRoots[PanelId::Transcript]`（设计 §2.4 的优先级：最近一次拆窗/点击/
/// Alt+↑↓ → 否则主窗格）。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActiveTranscript(pub Option<Entity>);

impl ActiveTranscript {
    /// 在当前工作副本里解析出活动转写根。
    pub fn root(&self, roots: &PanelRoots) -> Option<Entity> {
        active_root(Some(self), roots)
    }
}

/// 活动窗格解析：优先 `ActiveTranscript`，失效时退回主转写。
fn active_root(active: Option<&ActiveTranscript>, roots: &PanelRoots) -> Option<Entity> {
    active
        .and_then(|active| active.0)
        .filter(|entity| roots.0.values().any(|root| root == entity))
        .or_else(|| roots.entity(PanelId::Transcript))
}

/// 一个根在 [`PanelRoots`] 里的 [`PanelId`]（活动窗格的键要写回它那块面板）。
fn panel_of(root: Entity, roots: &PanelRoots) -> Option<PanelId> {
    roots
        .0
        .iter()
        .find_map(|(id, candidate)| (*candidate == root).then_some(*id))
}

/// 某一块转写根这一帧能放多少列、多少行（`(cols, rows)`）。
///
/// 优先看 Niri 的 `Tile`（新窗口逻辑），没有再退回帧布局的 `PanelAreas`。
/// 列数只用于宽度裁剪"下面还有"标记（上游 `pane::draw_below` 在窗格比标记
/// 窄时整句不画）。S1 从 `pane_size` 泛化成按根查询：每块窗格用自己的区域。
fn pane_size_of(
    areas: &PanelAreas,
    root: Entity,
    tiles: &Query<&crate::niri::Tile>,
    tiling: Option<&crate::niri::Tiling>,
) -> Option<(usize, usize)> {
    if let Ok(tile) = tiles.get(root)
        && let Some(tiling) = tiling
    {
        let (cols, rows) = tiling.tile_cells(tile.layer, tile.column, tile.tile);
        return (rows > 0).then_some((usize::from(cols), usize::from(rows)));
    }
    let area = areas.get(root)?;
    (!area.is_empty()).then_some((usize::from(area.cols), usize::from(area.rows)))
}

/// 滚上去、下面还是旧行时占掉最后一行的标记（rig-tui `pane::draw_below` 的同一句话）。
///
/// 只留一份定义：转写引擎的 [`crate::transcript::MORE_BELOW`]（`Below::More` 用的也是它）。
/// 滚动之后新到的行用 [`crate::transcript::NEW_OUTPUT`]（`Below::New`），不再单独定义常量。
pub const BELOW_MARKER: &str = crate::transcript::MORE_BELOW;

/// “下面还有”标记行的 key：`u64::MAX`，不属于任何块（[`item_rows`] 跳过它）。
const BELOW_MARKER_KEY: u64 = u64::MAX;

/// 最后一行右端的标记行；`text` 是 [`Below::marker`] 给的那句话（窗格太窄时
/// 传空串：上游仍把最后一行留给标记，只是不画字）。
///
/// `hash` 用这一行自己的内容算（[`crate::transcript::row_hash`]）：标记从
/// `↓ more below` 变成 `↓ new output` 时，面板的增量 diff 才会重画这一行。
fn below_marker(text: &'static str) -> KeyedRow {
    let row = Row::new(Line::from("")).with_right(Line::from(Span::styled(
        text,
        Style::default()
            .fg(RColor::LightCyan)
            .add_modifier(Modifier::BOLD),
    )));
    let hash = crate::transcript::row_hash(&row);
    KeyedRow {
        key: BELOW_MARKER_KEY,
        hash,
        row,
    }
}

/// 把整段的可见窗口写进**每一块**转写窗格（`FeathersSystems::Fit`）。
///
/// 遍历**所有**带 [`TranscriptPane`] 的转写根（有工作区时只处理当前工作副本
/// 里的根），每块用自己的 [`pane_size_of`] 与 [`TranscriptPane::scroll`]
/// 窗口化，写自己的 `PanelRows[这块的 PanelId]`。S1 里 `Pane2` 随后仍会被
/// `panels` 的镜像覆盖（拆窗语义不变）；这里先各算各的，S2 删镜像时不用再改
/// 窗口化。
///
/// 滚上去、下面还有行时，最后一行留给 [`BELOW_MARKER`]（rig-tui 是把标记
/// 画在最后一行的右端，占掉的是同一行）。标记的文案由窗格自己的
/// [`BelowSeen`] 判定：滚上去那一刻已经存在的行是 `↓ more below`；之后整段
/// 又长了，才换成 `↓ new output`（上游 `pane::draw_below` 的 `Below::{More, New}`）。
///
/// 宽度裁剪和上游一致：窗格比标记窄时整句不画，但最后一行仍然留空（上游
/// `draw_below` 里 `area.width < width` 直接 return）。
// `legacy` 是 `editor.rs` 的兼容入口（见 [`PaneScroll`] 的文档）。
pub fn window_transcript(
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
    full: Res<TranscriptRows>,
    legacy: Option<Res<PaneScroll>>,
    set: Option<Res<crate::workspaces::WorkspaceSet>>,
    mut panes: Query<(Entity, Option<&PanelId>, &mut TranscriptPane)>,
    mut panels: ResMut<PanelRows>,
    tiles: Query<&crate::niri::Tile>,
    tiling: Option<Res<crate::niri::Tiling>>,
    mut first_frame: Local<bool>,
) {
    // 兼容入口：资源被改的那一帧把请求交给**主转写**（`editor.rs` 发送时设
    // `follow` 的语义一直是“主窗格跟末尾”；`Pane2` 在 S1 仍镜像主窗格）。
    // 这里不反向写回资源，所以资源永远只是“最近一次请求”，不是滚动状态的
    // 第二份副本。第一帧跳过：`PanePlugin` 刚 `init_resource` 的默认值不是
    // 请求，别拿它覆盖窗格自己带的初始位置。
    if !*first_frame {
        *first_frame = true;
    } else if let Some(legacy) = legacy.as_ref().filter(|legacy| legacy.is_changed())
        && let Some(root) = roots.entity(PanelId::Transcript)
        && let Ok((_, _, mut pane)) = panes.get_mut(root)
    {
        pane.scroll = **legacy;
    }

    // 先收集再写 `PanelRows`：查询借用窗格组件，面板资源是另一份容器的借用。
    // 遍历**所有**带 `TranscriptPane` 的根；`PanelId` 优先从当前工作副本的
    // `PanelRoots` 反查（旧的无窗口测试只登记了根），没有时用根上挂的
    // `PanelId`（`spawn_panel` 会挂）。
    //
    // 有工作区时只处理当前工作副本里的根：非聚焦工作区的行在它自己的
    // `WorkspacePanels` 里，不能拿聚焦工作区的 `TranscriptRows` 去写。
    let workspace_mode = set
        .as_deref()
        .is_some_and(|set| !set.items.is_empty());
    let mut updates: Vec<(PanelId, Vec<KeyedRow>)> = Vec::new();
    for (entity, component_id, mut pane) in panes.iter_mut() {
        let Some(id) = panel_of(entity, &roots).or(component_id.copied()) else {
            continue;
        };
        if !matches!(id, PanelId::Transcript | PanelId::Pane2) {
            continue;
        }
        if workspace_mode && roots.entity(id) != Some(entity) {
            continue;
        }
        let Some((width, height)) = pane_size_of(&areas, entity, &tiles, tiling.as_deref()) else {
            continue;
        };
        updates.push((id, window_pane(&full.0, &mut pane, width, height)));
    }
    for (id, rows) in updates {
        panels.set(id, rows);
    }
}

/// 一块窗格的窗口化（[`window_transcript`] 的主体；每块窗格各用自己的滚动）。
fn window_pane(
    full: &[KeyedRow],
    pane: &mut TranscriptPane,
    width: usize,
    height: usize,
) -> Vec<KeyedRow> {
    let len = full.len();
    let max_top = len.saturating_sub(height);
    if pane.scroll.follow {
        pane.scroll.top = max_top;
    }
    let top = pane.scroll.top.min(max_top);
    if top != pane.scroll.top {
        pane.scroll.top = top;
    }
    // 先让窗格自己的 `BelowSeen` 看到这一帧的 follow/total/top/height：跟随
    // 末尾就复位，刚离开末尾就记下"已经看到哪里"。
    let below = pane.below.of(pane.scroll.follow, len, top, height);
    // 只有一行的窗格把那一行留给正文（上游 `text_rows` 的同一个分支）。
    let below = if height > 1 { below } else { Below::Nothing };
    let marker = below.marker();
    let marker_fits = marker.is_some_and(|text| Span::from(text).width() <= width);
    let rows_shown = match below {
        Below::Nothing => height,
        Below::More | Below::New => height - 1,
    };
    let end = (top + rows_shown).min(len);
    let mut rows = full[top..end].to_vec();
    if below != Below::Nothing {
        rows.push(below_marker(if marker_fits {
            marker.unwrap_or_default()
        } else {
            ""
        }));
    }
    rows
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
    panel: PanelId,
    root: Entity,
    pane: &mut TranscriptPane,
    selection: &mut Selection,
    focus: Option<&mut KeyFocus>,
    height: usize,
    max_top: usize,
) {
    let items = item_rows(full);
    if items.is_empty() {
        return;
    }
    // 当前身份只认“选区就在这个窗格里”的那一份；切到另一块窗格后不沿用
    // 上一块的条目（每块窗格的 selected 各记各的）。
    let at = selection
        .item()
        .filter(|_| selection.panel() == Some(panel))
        .and_then(|item| items.iter().position(|candidate| candidate.ordinal == item));
    let index = match (at, down) {
        (Some(at), true) => (at + 1).min(items.len() - 1),
        (Some(at), false) => at.saturating_sub(1),
        (None, true) => 0,
        (None, false) => items.len() - 1,
    };
    let item = items[index];
    let scroll = &mut pane.scroll;
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
    pane.selected = Some(item.ordinal);
    selection.select_item(panel, from, to, item.ordinal);
    if let Some(focus) = focus {
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
    active: Option<Res<ActiveTranscript>>,
    mut panes: Query<&mut TranscriptPane>,
    mut selection: Option<ResMut<Selection>>,
    mut focus: Option<ResMut<KeyFocus>>,
    tiles: Query<&crate::niri::Tile>,
    tiling: Option<Res<crate::niri::Tiling>>,
) {
    // 目标 = 活动窗格（没有/失效时退回主转写）。滚动、条目选中都只改它那一份。
    let Some(root) = active_root(active.as_deref(), &roots) else {
        return;
    };
    let Some(panel) = panel_of(root, &roots) else {
        return;
    };
    if !matches!(panel, PanelId::Transcript | PanelId::Pane2) {
        return;
    }
    let Ok(mut pane) = panes.get_mut(root) else {
        return;
    };

    // Esc：清掉条目选区。**不在这里清 KeyFocus**：`editor::keys` 也读同一个
    // Esc，只有看到焦点还在转写根上，它才会走面板作用域的“Esc 交还输入框”
    // 分支；在 Prepare 里先清焦点会把它送回输入作用域，Esc 就被当成 Stop
    // （停 agent）。焦点由 `release_transcript_focus` 在窗口化之后交还。
    if keys.just_pressed(KeyCode::Escape) {
        pane.selected = None;
        if let Some(selection) = selection.as_deref_mut()
            && selection.item().is_some()
        {
            selection.clear();
        }
    }

    let Some((_, height)) = pane_size_of(&areas, root, &tiles, tiling.as_deref()) else {
        return;
    };
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
                panel,
                root,
                &mut pane,
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
            pane.scroll.up(1);
        }
        if down {
            pane.scroll.down(1, max_top);
        }
    }

    // PgUp/PgDn 的原行为不变：一次一页。
    let page = pane.scroll.page(height);
    if keys.just_pressed(KeyCode::PageUp) {
        pane.scroll.up(page);
    }
    if keys.just_pressed(KeyCode::PageDown) {
        pane.scroll.down(page, max_top);
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
///
/// **不碰 [`KeyFocus`]**：焦点只由用户的显式动作决定 —— Alt+↑/↓ 在
/// [`select_pane_item`] 里拿键、Esc / 点输入面板 / 条目收起高亮在
/// [`release_transcript_focus`] 里还键。以前这里每帧把焦点抢回转写根：点完
/// 输入框打不了字，而且"滚出去、没高亮"时也会被一直按住；键盘焦点不该由
/// “这一帧重新算了一遍高亮”这种被动同步来抢。
fn sync_item_selection(
    full: Res<TranscriptRows>,
    panels: Res<PanelRows>,
    mut selection: Option<ResMut<Selection>>,
) {
    // 先只读：鼠标选区（`item == None`）每帧原样返回，不把 `Selection` 标记成
    // 改过，也不去扫整段行。
    let Some(panel) = selection.as_deref().and_then(Selection::panel) else {
        return;
    };
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
    // 可见窗口用**选区所在那块面板**的行（Pane2 有自己的窗口，不能拿主转写
    // 的行下标去套）。
    let visible = item_rows(panels.get(panel));
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
        selection.panel() != Some(panel)
            || selection.anchor() != Point::new(shown.start, 0)
            || selection.focus() != Point::new(shown.end, 0)
    });
    if needs_reselect
        && let Some(selection) = selection.as_deref_mut()
    {
        selection.select_item(panel, shown.start, shown.end, item);
    }
}

/// 条目选中不再“有效”（清掉、或者滚出窗口被 [`Selection::suspend_item`] 收起
/// 高亮）就把 [`KeyFocus`] 还给输入框（`None`）。
///
/// 放在 `Fit`、[`pane_keys`] 之后而不是按键处理里：见 [`pane_keys`] 的注释
/// —— Esc 的“交还输入框”必须让 `editor::keys` 先看到焦点还在转写根上。这里
/// 同时兜住鼠标点击、滚轮、resize 清掉选中之后把键还给输入框。
///
/// “选着但没高亮”也算可归还：条目滚出可见区后身份还在（滚回来还能亮），
/// 但用户眼前没有任何选中标记，此时再按住键盘不给输入框就是纯粹的焦点陷阱
/// （用户可见：Alt+↑ 选了一条、滑走之后再点输入框也打不了字）。
fn release_transcript_focus(
    roots: Option<Res<PanelRoots>>,
    selection: Option<Res<Selection>>,
    mut focus: Option<ResMut<KeyFocus>>,
) {
    let Some(roots) = roots.as_deref() else {
        return;
    };
    let Some(focus) = focus.as_deref_mut() else {
        return;
    };
    // 焦点可能拿在主转写或拆出来的 Pane2 上（活动窗格是谁就谁）；别的要求
    // 的焦点（输入面板等）不归这里管。
    let Some(focused) = focus.0 else {
        return;
    };
    let is_transcript = roots.entity(PanelId::Transcript) == Some(focused)
        || roots.entity(PanelId::Pane2) == Some(focused);
    if !is_transcript {
        return;
    }
    // 只有“条目选着、而且高亮真的亮在屏幕上”才继续拿键；`item` 为 `None`
    // （鼠标选区/没有选中）或已 suspend（anchor == focus）都交还输入框。
    let keeping = selection
        .as_deref()
        .is_some_and(|selection| selection.item().is_some() && selection.has_selection());
    if !keeping {
        focus.0 = None;
    }
}

/// 滚轮落在哪块面板上：目标是行实体时看 [`RowLayout`]，是面板根时看 [`PanelRoots`]。
///
/// 行实体是拾取目标（`transcript::spawn_row_themed` 给行加了 `Pickable`），
/// 所以滚轮消息的 `entity` 正常就是某一行；面板根那条只是兜底（背景可拾取时）。
/// 滚动条的轨道/滑块也是 `Pickable`，落在它们上面时同样归它服务的那块窗格。
fn wheel_panel(
    entity: Entity,
    roots: &PanelRoots,
    layouts: &Query<&RowLayout>,
    bars: &Query<&TranscriptScrollbar>,
    thumbs: &Query<&TranscriptScrollThumb>,
) -> Option<PanelId> {
    if let Ok(layout) = layouts.get(entity) {
        return Some(layout.panel);
    }
    if let Ok(bar) = bars.get(entity) {
        return panel_of(bar.pane, roots);
    }
    if let Ok(thumb) = thumbs.get(entity) {
        return panel_of(thumb.pane, roots);
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
    bars: Query<&TranscriptScrollbar>,
    thumbs: Query<&TranscriptScrollThumb>,
    areas: Res<PanelAreas>,
    full: Res<TranscriptRows>,
    mut panes: Query<&mut TranscriptPane>,
    mut selection: Option<ResMut<Selection>>,
    tiles: Query<&crate::niri::Tile>,
    tiling: Option<Res<crate::niri::Tiling>>,
) {
    for event in scrolls.read() {
        // 横向滚轮（触摸板的左右滑）不是转写的滚动。
        let Some(panel) = wheel_panel(event.entity, &roots, &layouts, &bars, &thumbs) else {
            continue;
        };
        if event.y == 0.0 || !matches!(panel, PanelId::Transcript | PanelId::Pane2) {
            continue;
        }
        // 上游 `moused` 的顺序：先收起选区，再看有没有窗格可滚。
        if let Some(selection) = selection.as_deref_mut() {
            selection.clear();
        }
        // 滚的是**指针下面那块**窗格（`wheel_panel` 已把行实体/根换算成
        // PanelId），不是“当前活动窗格”：行数也按那块根自己的区域算。
        let Some(root) = roots.entity(panel) else {
            continue;
        };
        let Ok(mut pane) = panes.get_mut(root) else {
            continue;
        };
        let Some((_, height)) = pane_size_of(&areas, root, &tiles, tiling.as_deref()) else {
            continue;
        };
        let max_top = full.len().saturating_sub(height);
        if event.y > 0.0 {
            pane.scroll.wheel_up(max_top);
        } else {
            pane.scroll.wheel_down(max_top);
        }
    }
}

/// 给已登记但没经过 [`crate::frame::spawn_panel`] 的转写根补上默认
/// [`TranscriptPane`]。
///
/// 生产路径全部由 `spawn_panel` 挂组件；这条只兼容旧的无窗口测试/集成方：
/// 它们手工 spawn 根实体（没有 `PanelId` 以外的面板构造）再写 `PanelRoots`。
/// 补默认值与 `spawn_panel` 完全一致，所以不影响正常路径。
fn hydrate_transcript_panes(
    mut commands: Commands,
    roots: Option<Res<PanelRoots>>,
    panes: Query<(), Without<TranscriptPane>>,
) {
    let Some(roots) = roots else {
        return;
    };
    for id in [PanelId::Transcript, PanelId::Pane2] {
        let Some(root) = roots.entity(id) else {
            continue;
        };
        if panes.get(root).is_ok() {
            commands.entity(root).insert(TranscriptPane::default());
        }
    }
}

pub struct PanePlugin;

impl Plugin for PanePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TranscriptRows>()
            .init_resource::<ActiveTranscript>()
            // 只为 `editor.rs` 的兼容入口保留（见 `PaneScroll` 的文档）；没有
            // 任何系统把它当滚动状态读。
            .init_resource::<PaneScroll>()
            // 兼容旧的读取方；窗口化现在用每块窗格自己的 `TranscriptPane::below`。
            .init_resource::<BelowSeen>()
            // 拖拽滚动条的按下点/行程；观察者写、`PointerDrag` 读。
            .init_resource::<TranscriptScrollbarDrag>()
            // F4：转写窗格一出现就挂轨道 + 滑块（主转写与 Pane2 同一路径）。
            .add_observer(spawn_transcript_scrollbar)
            .add_observer(begin_scrollbar_drag)
            .add_observer(drag_scrollbar)
            .add_observer(end_scrollbar_drag)
            // 生产里 `PickingPlugin` 已经注册了这个消息；这里补上是为了让不建
            // 窗口的测试 App 也能写 `PointerScroll`（`add_message` 幂等）。
            .add_message::<PointerScroll>()
            .add_systems(Startup, hydrate_transcript_panes)
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
                    // 滑块几何读窗口化之后的 `scroll`；排在它后面同一帧刷新。
                    sync_transcript_scrollbars,
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
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::input::mouse::MouseScrollUnit;
    use bevy::input::touch::TouchPhase;
    use bevy::picking::backend::HitData;
    use bevy::picking::events::{
        Pointer, PointerDrag, PointerDragStart, PointerPress,
    };
    use bevy::picking::pointer::{Location, PointerButton, PointerId};
    use bevy::ui::ComputedNode;
    use feathers_in_3d::WorldPanel;

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

    /// 高 `rows` 行、宽 `cols` 列的转写面板区域。
    fn pane_area_cols(cols: u16, rows: u16) -> PanelArea {
        PanelArea {
            rect: CellRect::new(0, 0, cols, rows),
            size: Vec2::new(
                f32::from(cols) * crate::frame::CELL.x,
                f32::from(rows) * crate::frame::CELL.y,
            ),
            cols,
            rows,
            center: None,
        }
    }

    /// 宽 80 列、高 `rows` 行的转写面板区域。
    fn pane_area(rows: u16) -> PanelArea {
        pane_area_cols(80, rows)
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

    /// 往整段末尾追加 `count` 行（流式/新一轮输出到达）。
    fn append_rows(app: &mut App, count: usize) {
        let start = app.world().resource::<TranscriptRows>().len();
        let rows: Vec<KeyedRow> = (start..start + count)
            .map(|index| KeyedRow {
                key: index as u64,
                hash: index as u64 + 1,
                row: Row::new(Line::from(format!("row {index}"))),
            })
            .collect();
        app.world_mut()
            .resource_mut::<TranscriptRows>()
            .0
            .extend(rows);
    }

    /// 一块高 `height` 的转写面板 + 整段 `count` 行；换掉 `PanelAreas` 里的区域
    /// 就是一次 resize（[`window_transcript`] 每帧读当前区域，不缓存）。
    fn windowing_app(count: usize, height: u16, top: usize, follow: bool) -> App {
        let mut app = App::new();
        app.init_resource::<PanelRows>();
        let root = app
            .world_mut()
            .spawn((
                PanelId::Transcript,
                TranscriptPane {
                    scroll: view(top, follow),
                    ..default()
                },
            ))
            .id();
        app.insert_resource(PanelRoots(HashMap::from([(PanelId::Transcript, root)])));
        app.insert_resource(PanelAreas(HashMap::from([(root, pane_area(height))])));
        app.insert_resource(TranscriptRows(keyed_rows(count)));
        // 不插入 `PaneScroll` 资源：它是 `editor.rs` 的兼容入口，插入会被当成
        // “最近一次请求”而覆盖组件里的初始位置。
        app.add_systems(Update, window_transcript);
        app
    }

    /// 主转写根。
    fn transcript_root(app: &App) -> Entity {
        app.world()
            .resource::<PanelRoots>()
            .entity(PanelId::Transcript)
            .expect("主转写面板根")
    }

    /// 一块窗格的组件状态。
    fn pane_of(app: &App, root: Entity) -> &TranscriptPane {
        app.world()
            .get::<TranscriptPane>(root)
            .expect("转写根要挂 TranscriptPane")
    }

    /// 主转写窗格的“下面还有”状态。
    fn below(app: &App) -> BelowSeen {
        pane_of(app, transcript_root(app)).below
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
        assert_eq!(scroll(&app).top, 90, "top 被夹到新末尾");
    }

    #[test]
    fn windowing_recomputes_when_the_panel_row_count_changes() {
        // 面板先 5 行再长到 10 行：每帧按当前区域重算，不拿旧尺寸。
        let mut app = windowing_app(100, 5, 95, true);
        app.update();
        assert_eq!(visible_rows(&app).len(), 5);
        assert_eq!(
            scroll(&app).top,
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
        assert_eq!(scroll(&app).top, 90);
    }

    // ---- “下面还有”标记：more below / new output（上游 pane::draw_below） ----

    /// 跟随末尾时没有标记：最后一行就是真正的末行。
    #[test]
    fn following_the_tail_draws_no_below_marker() {
        let mut app = windowing_app(100, 10, 90, true);
        app.update();
        let rows = visible_rows(&app);
        assert_eq!(rows.len(), 10);
        assert_eq!(rows.last().map(String::as_str), Some("row 99"));
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("more below") || row.contains("new output")),
            "{rows:?}"
        );
    }

    /// 滚上去：先画 `↓ more below`；此时追加新行 → 变成 `↓ new output`。
    #[test]
    fn scrolling_up_shows_more_below_then_new_output_when_rows_arrive() {
        let mut app = windowing_app(100, 10, 80, false);
        app.update();
        let rows = visible_rows(&app);
        assert_eq!(rows.len(), 10, "9 行正文 + 1 行标记");
        assert_eq!(rows.first().map(String::as_str), Some("row 80"));
        assert_eq!(
            rows.last().map(String::as_str),
            Some(" ↓ more below (PgDn) "),
            "滚上去、下面还是旧行：more below"
        );
        assert_eq!(below(&app).seen(), 100);
        assert!(below(&app).scrolled());

        append_rows(&mut app, 5);
        app.update();
        let rows = visible_rows(&app);
        assert_eq!(rows.len(), 10);
        assert_eq!(
            rows.first().map(String::as_str),
            Some("row 80"),
            "新行到达不该移动用户正在看的位置"
        );
        assert_eq!(
            rows.last().map(String::as_str),
            Some(" ↓ new output (PgDn) "),
            "滚动之后才到的行：new output"
        );
        assert!(
            !rows.iter().any(|row| row.contains("more below")),
            "两种标记不能同时出现：{rows:?}"
        );
    }

    /// 滚回底部：标记消失，`BelowSeen` 复位（下一次上滚重新从 More 开始）。
    #[test]
    fn scrolling_back_to_the_bottom_clears_the_marker_and_resets_below_seen() {
        let mut app = windowing_app(100, 10, 80, false);
        app.update();
        append_rows(&mut app, 5);
        app.update();
        assert_eq!(
            visible_rows(&app).last().map(String::as_str),
            Some(" ↓ new output (PgDn) ")
        );

        // PgDn 到底 / `Scroll::follow`：窗顶回到 max_top，重新跟随末尾。
        let max_top = 105 - 10;
        let root = transcript_root(&app);
        app.world_mut()
            .get_mut::<TranscriptPane>(root)
            .expect("主转写")
            .scroll
            .to(None, max_top);
        app.update();

        let rows = visible_rows(&app);
        assert_eq!(rows.len(), 10);
        assert_eq!(rows.last().map(String::as_str), Some("row 104"));
        assert!(
            !rows.iter().any(|row| row.contains("PgDn")),
            "回到末尾就不再占一行给标记：{rows:?}"
        );
        let seen = below(&app);
        assert!(!seen.scrolled(), "跟随末尾后 BelowSeen 复位");
        assert_eq!(seen.seen(), 105, "记下的行数跟着整段更新");
    }

    /// 窗格比标记窄：整句不画（上游 `draw_below` 的宽度裁剪），最后一行留空。
    #[test]
    fn a_pane_narrower_than_the_below_marker_leaves_the_last_row_blank() {
        let mut app = windowing_app(100, 10, 80, false);
        let root = app
            .world()
            .resource::<PanelRoots>()
            .entity(PanelId::Transcript)
            .expect("转写面板根");
        app.world_mut()
            .resource_mut::<PanelAreas>()
            .0
            .insert(root, pane_area_cols(8, 10));
        app.update();

        let rows = visible_rows(&app);
        assert_eq!(rows.len(), 10, "标记行仍被保留，只是不画字");
        assert_eq!(rows.last().map(String::as_str), Some(""));
        assert!(
            !rows.iter().any(|row| row.contains("PgDn")),
            "窗格 8 列放不下整句：{rows:?}"
        );
        assert!(below(&app).scrolled());
    }

    // ---- 滚轮接线：不建窗口，直接写 PointerScroll 消息 ----

    struct WheelFixture {
        app: App,
        /// 指针下面那一行（`RowLayout` 指到面板根）。
        row: Entity,
        /// 主转写根（组件里存着这块窗格的滚动）。
        root: Entity,
    }

    /// 摆好主转写的初始滚动（测试里不经过 `window_transcript`）。
    fn set_view(fixture: &mut WheelFixture, scroll: PaneScroll) {
        fixture
            .app
            .world_mut()
            .get_mut::<TranscriptPane>(fixture.root)
            .expect("主转写")
            .scroll = scroll;
    }

    fn wheel_fixture(panel: PanelId, height: u16, count: usize) -> WheelFixture {
        let mut app = App::new();
        app.init_resource::<TranscriptRows>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelAreas>()
            .init_resource::<Selection>();
        app.add_message::<PointerScroll>()
            .add_systems(Update, pane_wheel.run_if(crate::picker::no_picker));
        let root = app
            .world_mut()
            .spawn((PanelId::Transcript, TranscriptPane::default()))
            .id();
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
        WheelFixture { app, row, root }
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
        set_view(&mut fixture, view(90, true));
        wheel(&mut fixture, 1.0);
        fixture.app.update();
        let scroll = scroll(&fixture.app);
        assert_eq!(scroll.top, 87);
        assert!(!scroll.follow);
    }

    #[test]
    fn a_wheel_down_over_the_transcript_follows_again_at_the_bottom() {
        let mut fixture = wheel_fixture(PanelId::Transcript, 10, 100);
        set_view(&mut fixture, view(88, false));
        wheel(&mut fixture, -1.0);
        fixture.app.update();
        let scroll = scroll(&fixture.app);
        assert_eq!(scroll.top, 90);
        assert!(scroll.follow);
    }

    #[test]
    fn a_wheel_outside_the_transcript_leaves_the_view_alone() {
        // 指针在状态行上：滚轮不归转写管（picker 的列表行同理）。
        let mut fixture = wheel_fixture(PanelId::Status, 10, 100);
        set_view(&mut fixture, view(40, false));
        wheel(&mut fixture, 1.0);
        fixture.app.update();
        let scroll = scroll(&fixture.app);
        assert_eq!(scroll.top, 40);
        assert!(!scroll.follow);
    }

    #[test]
    fn a_wheel_while_a_picker_is_open_leaves_the_view_alone() {
        let mut fixture = wheel_fixture(PanelId::Transcript, 10, 100);
        set_view(&mut fixture, view(90, true));
        fixture
            .app
            .world_mut()
            .spawn(rig_harness::prelude::Picker::default());
        wheel(&mut fixture, 1.0);
        fixture.app.update();
        let scroll = scroll(&fixture.app);
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
            .init_resource::<PanelRows>()
            .init_resource::<PanelAreas>()
            .init_resource::<PanelRoots>()
            .init_resource::<Selection>()
            .init_resource::<KeyFocus>()
            .init_resource::<ButtonInput<KeyCode>>();
        let root = app
            .world_mut()
            .spawn((PanelId::Transcript, TranscriptPane::default()))
            .id();
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
        pane_of(app, transcript_root(app)).scroll
    }

    fn focus(app: &App) -> KeyFocus {
        *app.world().resource::<KeyFocus>()
    }

    #[test]
    fn item_rows_group_by_the_high_key_bits_and_skip_the_below_marker() {
        let mut rows = block_rows(0, &["a", "b"]);
        rows.extend(block_rows(1, &["c", "d", "e"]));
        let mut with_marker = rows.clone();
        with_marker.push(below_marker(BELOW_MARKER));
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
            None,
            "没有高亮就没有键盘目标：键还给输入框，不留焦点陷阱"
        );

        // PgUp 滚回来：同一个条目重新亮起来；键仍在输入框（被动同步不抢焦点）。
        tap(&mut fixture.app, &[], KeyCode::PageUp);
        assert_eq!(selection(&fixture.app).item(), Some(0));
        assert!(selection(&fixture.app).has_selection());
        assert_eq!(selection(&fixture.app).anchor(), Point::new(0, 0));
        assert_eq!(focus(&fixture.app).0, None);
    }

    // ---- 焦点归还：条目收起高亮 / 焦点被别处拿走 / 点输入面板 ----

    #[test]
    fn an_item_that_scrolls_out_of_view_gives_the_keys_back() {
        let mut fixture = keys_app_rows(single_row_blocks(20), 5);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(selection(&fixture.app).item(), Some(0));
        assert_eq!(focus(&fixture.app).0, Some(fixture.root));

        // 条目滚出可见区：`suspend_item` 保留身份、收起高亮。此时转写上已经
        // 没有可操作的选中目标，键盘必须还给输入框（否则点输入框打字没反应）。
        tap(&mut fixture.app, &[], KeyCode::PageDown);
        assert_eq!(selection(&fixture.app).item(), Some(0), "身份保留，滚回来还能亮");
        assert!(!selection(&fixture.app).has_selection(), "没有高亮");
        assert_eq!(
            focus(&fixture.app).0,
            None,
            "`item().is_some()` 但已 suspend 也要归还焦点"
        );
    }

    #[test]
    fn a_focus_taken_elsewhere_is_not_grabbed_back_by_the_selection_sync() {
        let mut fixture = keys_app(&[&["one"][..], &["two"][..]], 10);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(selection(&fixture.app).item(), Some(0));
        assert_eq!(focus(&fixture.app).0, Some(fixture.root));

        // 别的系统把焦点拿走（点击输入面板、切面板……）。被动的高亮同步
        // 不该把它抢回转写根：以前这里每帧无条件 `focus.0 = Some(root)`。
        fixture.app.world_mut().resource_mut::<KeyFocus>().0 = None;
        fixture.app.update();
        assert_eq!(
            focus(&fixture.app).0,
            None,
            "同步条目高亮不是“用户回到转写”，不能抢键盘"
        );
        assert!(
            selection(&fixture.app).has_selection(),
            "条目仍然亮着（选区本身没被动过）"
        );
        assert_eq!(selection(&fixture.app).item(), Some(0));
    }

    struct ClickFixture {
        app: App,
        transcript_root: Entity,
        input_root: Entity,
        input_row: Entity,
        /// 输入面板根下面那块铺满输入框的拾取面（生产里点空白命中的就是它）。
        input_surface: Entity,
    }

    /// 生产结构的无窗口 App：`PanePlugin` + `SelectionPlugin` + `EditorPlugin`，
    /// 带转写/输入两块面板（输入面板根要给 3D 拾取换算用的 `WorldPanel` +
    /// `ComputedNode`）和一行输入行实体。
    fn focus_click_app(blocks: &[&[&str]], height: u16) -> ClickFixture {
        let mut app = App::new();
        // 生产里由 `FeathersViewPlugin::build` 排好的阶段顺序。
        app.configure_sets(
            Update,
            (
                FeathersSystems::Prepare,
                FeathersSystems::Layout,
                FeathersSystems::Draw,
                FeathersSystems::Fit,
                FeathersSystems::Render,
            )
                .chain(),
        );
        app.add_plugins((
            crate::pane::PanePlugin,
            crate::selection::SelectionPlugin,
            crate::editor::EditorPlugin,
        ))
        .init_resource::<crate::theme::Theme>()
        .init_resource::<ButtonInput<KeyCode>>()
        // `PanePlugin` 只建 `TranscriptRows`/`PaneScroll`；面板资源平时由
        // `PanelsPlugin` 建，这里不装整条面板同步，只补资源。
        .init_resource::<PanelRoots>()
        .init_resource::<PanelRows>()
        .init_resource::<PanelAreas>()
        .add_message::<KeyboardInput>();

        let transcript_root = app
            .world_mut()
            .spawn((PanelId::Transcript, TranscriptPane::default()))
            .id();
        let input_root = app
            .world_mut()
            .spawn((
                WorldPanel {
                    units_to_world: crate::frame::UNITS_TO_WORLD,
                    anchor: Transform::default(),
                    clip: None,
                },
                // 800px 宽、没有边框/内边距：行首在面板局部 x = -400。
                ComputedNode {
                    size: Vec2::new(800.0, 400.0),
                    ..default()
                },
            ))
            .id();
        {
            let mut roots = app.world_mut().resource_mut::<PanelRoots>();
            roots.0.insert(PanelId::Transcript, transcript_root);
            roots.0.insert(PanelId::Input, input_root);
        }
        {
            let mut areas = app.world_mut().resource_mut::<PanelAreas>();
            areas.0.insert(transcript_root, pane_area(height));
            areas.0.insert(
                input_root,
                PanelArea {
                    rect: CellRect::new(0, 0, 80, 3),
                    size: Vec2::new(80.0 * crate::frame::CELL.x, 3.0 * crate::frame::CELL.y),
                    cols: 80,
                    rows: 3,
                    center: None,
                },
            );
        }
        {
            let rows: Vec<KeyedRow> = blocks
                .iter()
                .enumerate()
                .flat_map(|(ordinal, lines)| block_rows(ordinal as u32, lines))
                .collect();
            app.world_mut().resource_mut::<TranscriptRows>().0 = rows;
        }
        // 输入面板的一行（`EditorPlugin` 的 `editor_rows` 每帧会重写成同一行数）。
        app.world_mut().resource_mut::<PanelRows>().set(
            PanelId::Input,
            vec![KeyedRow {
                key: 1,
                hash: 1,
                row: Row::new(Line::from("draft")),
            }],
        );
        let input_row = app
            .world_mut()
            .spawn(RowLayout {
                panel: PanelId::Input,
                panel_entity: input_root,
                index: 0,
            })
            .id();
        // 先跑一帧：窗口/跟随位置、输入行、拾取面都就位。
        app.update();
        let mut surfaces = app
            .world_mut()
            .query_filtered::<Entity, With<crate::selection::InputFocusSurface>>();
        let input_surface = surfaces
            .iter(app.world())
            .next()
            .expect("输入面板根下面要有拾取面");
        ClickFixture {
            app,
            transcript_root,
            input_root,
            input_row,
            input_surface,
        }
    }

    /// 往输入面板的 `target`（输入行或拾取面/面板根）写一条主键按下：命中点落在
    /// 第 0 格正中（和 `selection.rs` 的鼠标测试同一套换算）。
    fn press_input(fixture: &mut ClickFixture, target: Entity) {
        let node = fixture
            .app
            .world()
            .get::<ComputedNode>(fixture.input_root)
            .expect("输入面板的 ComputedNode");
        let x = crate::selection::content_left(node) + 0.5 * crate::frame::CELL.x;
        let local = Vec3::new(x * crate::frame::UNITS_TO_WORLD, 0.0, 0.0);
        let world = Transform::default().transform_point(local);
        fixture.app.world_mut().write_message(PointerPress {
            entity: target,
            pointer: mouse_pointer(),
            button: PointerButton::Primary,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, Some(world), Some(Vec3::Z)),
            count: 1,
        });
        fixture.app.update();
    }

    /// 敲一个字符（走真实的 `KeyboardInput` 消息和 `editor::keys`）。
    fn type_character(app: &mut App, character: char) {
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::KeyX, // 测试只关心文本，物理键随意
            logical_key: Key::Character(character.to_string().into()),
            state: ButtonState::Pressed,
            text: Some(character.to_string().into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    #[test]
    fn clicking_an_input_row_gives_the_keys_back_and_typing_reaches_the_editor() {
        let mut fixture = focus_click_app(&[&["one"][..], &["two"][..]], 10);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(selection(&fixture.app).item(), Some(0));
        assert_eq!(
            focus(&fixture.app).0,
            Some(fixture.transcript_root),
            "选条目时键在转写上"
        );

        // 用户点输入框：必须把键盘交回输入框，否则字符键被面板作用域丢掉。
        let input_row = fixture.input_row;
        press_input(&mut fixture, input_row);
        assert_eq!(
            focus(&fixture.app).0,
            None,
            "点输入行 = 键盘回输入框（以前这里一直卡在转写根）"
        );

        // 端到端：焦点回来之后敲的字符真的进 `Editor`。
        type_character(&mut fixture.app, 'x');
        assert_eq!(
            fixture.app.world().resource::<crate::editor::Editor>().text(),
            "x",
            "点完输入框打字要能落进输入框"
        );
    }

    #[test]
    fn clicking_the_input_panels_blank_area_gives_the_keys_back() {
        // 输入框的空白处不是行实体：生产里点它命中的是铺满输入框的
        // `InputFocusSurface`（没有它连 `PointerPress` 都不会产生）。
        let mut fixture = focus_click_app(&[&["one"][..], &["two"][..]], 10);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(focus(&fixture.app).0, Some(fixture.transcript_root));

        let surface = fixture.input_surface;
        press_input(&mut fixture, surface);
        assert_eq!(focus(&fixture.app).0, None);
        assert_eq!(
            selection(&fixture.app).item(),
            None,
            "点输入框空白 = 转写上的键盘选中作废（和点转写行一致）"
        );

        // 点回输入行也照样归还（拾取面/行两种命中都覆盖）。
        let mut fixture = focus_click_app(&[&["one"][..], &["two"][..]], 10);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        let row = fixture.input_row;
        press_input(&mut fixture, row);
        assert_eq!(focus(&fixture.app).0, None);
    }

    #[test]
    fn clicking_the_input_boxes_blank_area_keeps_its_own_text_selection() {
        let mut fixture = focus_click_app(&[&["one"][..], &["two"][..]], 10);
        // 输入面板里拖选一段文字（鼠标选区，`item` 为 None）。
        let rows = fixture
            .app
            .world()
            .resource::<PanelRows>()
            .row_values(PanelId::Input);
        assert!(!rows.is_empty(), "输入面板要有行");
        {
            let mut selection = fixture.app.world_mut().resource_mut::<Selection>();
            selection.press_kind(PanelId::Input, &rows, 0, 0, ClickKind::Single);
            selection.drag(PanelId::Input, &rows, 0, 3);
            assert!(selection.has_selection());
        }

        // 拾取面和输入行共面：空白处那一下不能把行上的拖选清掉。
        let surface = fixture.input_surface;
        press_input(&mut fixture, surface);
        let selection = selection(&fixture.app);
        assert_eq!(selection.panel(), Some(PanelId::Input));
        assert!(
            selection.has_selection(),
            "点输入框空白不该清掉输入框里的拖选"
        );
    }

    // ---- 实时块：一个调用 = 一个条目（`batch::live_key` 的高 32 位） ----

    /// 两个并行实时调用（不同 effect）经真实 `batch::live_additions` 产出的行。
    fn live_call_rows(width: u16) -> Vec<KeyedRow> {
        use rig_core::message::{CallId, ToolCall, ToolFunction, ToolName};
        use rig_ecs::effects::Effects;
        use rig_ecs::prelude::*;

        let mut world = World::new();
        world.insert_resource(Effects::default());
        let agent = world.spawn(AgentId("live".into())).id();
        for (id, name) in [("call_a", "search_products"), ("call_b", "create_order")] {
            let name = ToolName::new(name).expect("工具名非空");
            world.spawn((
                ToolRequest {
                    call: ToolCall::new(
                        CallId::from_wire(id),
                        ToolFunction::new(name, serde_json::json!({ "q": id })),
                    ),
                },
                ForAgent(agent),
            ));
        }
        world.flush();
        crate::batch::live_additions(&mut world, agent, width)
    }

    #[test]
    fn live_calls_become_one_item_each_and_alt_down_steps_between_them() {
        // 宽度故意很小：每张卡都会折成多行，正好检验“不是一行一个条目”。
        let rows = live_call_rows(8);
        assert!(rows.len() >= 4, "两张卡各折成几行：{}", rows.len());

        // 段标记：live 行的高 32 位落在 [0x4000_0000, 0x8000_0000)（bit62 = 1、
        // bit63 = 0），避开引擎按位置发的 0x0_ 段和 Ask 的 0x8000 段。
        for key in rows.iter().map(|keyed| keyed.key) {
            let high = key >> 32;
            assert!(
                (0x4000_0000..0x8000_0000).contains(&high),
                "live 行的段标记不对：{key:#x}"
            );
        }

        // key 的位分配：同一块高 32 位相同、低 32 位是块内行号且从 0 连续递增。
        let mut runs: Vec<(u64, Vec<u32>)> = Vec::new();
        for key in rows.iter().map(|keyed| keyed.key) {
            let block = key >> 32;
            let line = (key & 0xFFFF_FFFF) as u32;
            match runs.last_mut() {
                Some((last, lines)) if *last == block => {
                    assert_eq!(line, lines.len() as u32, "块内行号从 0 连续递增");
                    lines.push(line);
                }
                _ => runs.push((block, vec![line])),
            }
        }
        assert_eq!(runs.len(), 2, "两个并行调用 = 两个块：{runs:?}");
        assert_ne!(runs[0].0, runs[1].0, "不同调用的块身份必须不同");
        assert!(
            runs.iter().all(|(_, lines)| lines.len() >= 2),
            "每张卡都折成多行（否则测不出一行一条目的 bug）：{runs:?}"
        );
        assert!(
            runs.iter().all(|(_, lines)| lines.first() == Some(&0)),
            "行号是**调用内**行号，不是全局行号：{runs:?}"
        );

        let items = item_rows(&rows);
        assert_eq!(items.len(), 2, "一个调用 = 一个条目：{items:?}");
        assert_eq!(items[0].rows(), 0..runs[0].1.len());
        assert_eq!(
            items[1].rows(),
            runs[0].1.len()..runs[0].1.len() + runs[1].1.len()
        );
        assert_ne!(items[0].ordinal, items[1].ordinal);

        // Alt+↓ 两下分别落在两个调用上（一下一条，不逐行、也不跳过整片）。
        let height = 40;
        let mut fixture = keys_app_rows(rows, height);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(
            selection(&fixture.app).item(),
            Some(items[0].ordinal),
            "第一下选中第一个调用"
        );
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(
            selection(&fixture.app).item(),
            Some(items[1].ordinal),
            "第二下选第二个调用（不是同一块里的下一行）"
        );
    }

    // ---- S1：逐窗格状态 + 活动窗格（`Pane2` 仍镜像） ----

    use bevy::ecs::system::RunSystemOnce;
    use bevy::text::FontSource;

    use crate::panels::PanelsPlugin;
    use crate::transcript::TranscriptFont;

    const SPLIT_ROWS: usize = 100;
    const SPLIT_HEIGHT: u16 = 10;

    /// 一块转写面板 + 默认树里的主转写，然后拆一次窗（走真的 `split_transcript`）。
    fn spawn_split_panes(
        mut commands: Commands,
        mut tree: ResMut<crate::frame::FrameTree>,
        mut roots: ResMut<PanelRoots>,
        mut rows: ResMut<PanelRows>,
        mut areas: ResMut<PanelAreas>,
        mut full: ResMut<TranscriptRows>,
    ) {
        let transcript = crate::frame::spawn_panel(
            &mut commands,
            PanelId::Transcript,
            crate::frame::FramePanel::tree(),
            crate::frame::Look::new(Color::BLACK, Color::WHITE, 0.0),
        );
        roots.0.insert(PanelId::Transcript, transcript);
        areas.0.insert(transcript, pane_area(SPLIT_HEIGHT));
        assert!(crate::frame::split_transcript(
            &mut commands,
            &mut tree,
            &mut roots,
            &mut rows
        ));
        let pane2 = roots.entity(PanelId::Pane2).expect("拆窗要 spawn Pane2");
        areas.0.insert(pane2, pane_area(SPLIT_HEIGHT));
        full.0 = keyed_rows(SPLIT_ROWS);
    }

    struct SplitFixture {
        app: App,
        transcript: Entity,
        pane2: Entity,
    }

    /// 两块转写 + 真的 `window_transcript`（Fit）与 `mirror_transcript_rows`
    /// （Render）；不建窗口。
    fn split_app() -> SplitFixture {
        let mut app = App::new();
        // 生产里由 `FeathersViewPlugin::build` 排好的阶段顺序（键在 Prepare、
        // 窗口化在 Fit、镜像在 Render）。
        app.configure_sets(
            Update,
            (
                FeathersSystems::Prepare,
                FeathersSystems::Layout,
                FeathersSystems::Draw,
                FeathersSystems::Fit,
                FeathersSystems::Render,
            )
                .chain(),
        );
        app.add_plugins((PanePlugin, PanelsPlugin))
            .insert_resource(TranscriptFont(FontSource::default()))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Selection>()
            .init_resource::<crate::frame::FrameTree>()
            .init_resource::<crate::frame::PanelAreas>();
        app.add_systems(Startup, spawn_split_panes);
        app.update();
        let roots = app.world().resource::<PanelRoots>();
        let transcript = roots.entity(PanelId::Transcript).expect("主转写");
        let pane2 = roots.entity(PanelId::Pane2).expect("Pane2");
        SplitFixture {
            app,
            transcript,
            pane2,
        }
    }

    fn rows_of(app: &App, panel: PanelId) -> Vec<KeyedRow> {
        app.world().resource::<PanelRows>().get(panel).to_vec()
    }

    /// 行的身份 + 内容（逐行比较镜像语义）。
    fn signatures(rows: &[KeyedRow]) -> Vec<(u64, u64, String)> {
        rows.iter()
            .map(|keyed| {
                (
                    keyed.key,
                    keyed.hash,
                    crate::selection::row_text(&keyed.row),
                )
            })
            .collect()
    }

    fn set_pane_scroll(app: &mut App, root: Entity, scroll: PaneScroll) {
        app.world_mut()
            .get_mut::<TranscriptPane>(root)
            .expect("转写根")
            .scroll = scroll;
    }

    fn pane_scroll(app: &App, root: Entity) -> PaneScroll {
        app.world().get::<TranscriptPane>(root).expect("转写根").scroll
    }

    fn set_active(app: &mut App, root: Entity) {
        app.world_mut().resource_mut::<ActiveTranscript>().0 = Some(root);
    }

    fn pane_component_count(app: &mut App) -> usize {
        let mut query = app.world_mut().query::<&TranscriptPane>();
        query.iter(app.world()).count()
    }

    /// 遍历的是“所有带 `TranscriptPane` 的根”：根没登记进 `PanelRoots` 也能
    /// 凭根上的 `PanelId` 窗口化（旧的无窗口调用方就是这样手工 spawn 的）。
    #[test]
    fn a_component_root_is_windowed_without_a_roots_entry() {
        let mut app = App::new();
        app.init_resource::<PanelRows>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelAreas>();
        let root = app
            .world_mut()
            .spawn((PanelId::Transcript, TranscriptPane::default()))
            .id();
        app.world_mut()
            .resource_mut::<PanelAreas>()
            .0
            .insert(root, pane_area(5));
        app.insert_resource(TranscriptRows(keyed_rows(20)));
        app.add_systems(Update, window_transcript);
        app.update();

        let rows = app.world().resource::<PanelRows>().get(PanelId::Transcript);
        assert_eq!(rows.len(), 5, "按自己的区域窗口化");
        assert_eq!(rows[0].key, 15, "默认跟随末尾：20 - 5");
    }

    /// 拆窗后两列逐行相等：`Pane2` 仍由 `mirror_transcript_rows` 填。
    #[test]
    fn split_panes_show_the_same_rows_line_by_line() {
        let fixture = split_app();
        let main = rows_of(&fixture.app, PanelId::Transcript);
        let mirrored = rows_of(&fixture.app, PanelId::Pane2);
        assert_eq!(main.len(), SPLIT_HEIGHT as usize);
        assert_eq!(
            signatures(&main),
            signatures(&mirrored),
            "拆窗后两列逐行相等（key/hash/文本）"
        );
    }

    /// 主窗格 PgDn：主列变，`Pane2` 跟着变（镜像仍然生效）。
    #[test]
    fn paging_the_main_pane_moves_the_mirrored_pane_too() {
        let mut fixture = split_app();
        // 拆窗后活动窗格是 Pane2；这条测的是主窗格，先切回去。
        set_active(&mut fixture.app, fixture.transcript);

        let before_main = signatures(&rows_of(&fixture.app, PanelId::Transcript));
        let before_pane2 = signatures(&rows_of(&fixture.app, PanelId::Pane2));
        assert_eq!(before_main, before_pane2);

        // 跟随末尾时 PgDn 不动；先 PgUp 离开末尾，再 PgDn 回来，两次都应变化。
        tap(&mut fixture.app, &[], KeyCode::PageUp);
        let after_up_main = signatures(&rows_of(&fixture.app, PanelId::Transcript));
        let after_up_pane2 = signatures(&rows_of(&fixture.app, PanelId::Pane2));
        assert_ne!(before_main, after_up_main, "PgUp 后主窗格换了窗口");
        assert_eq!(
            after_up_main, after_up_pane2,
            "Pane2 镜像主窗格（同一份可见行）"
        );

        tap(&mut fixture.app, &[], KeyCode::PageDown);
        let after_down_main = signatures(&rows_of(&fixture.app, PanelId::Transcript));
        let after_down_pane2 = signatures(&rows_of(&fixture.app, PanelId::Pane2));
        assert_ne!(after_up_main, after_down_main, "PgDn 后主窗格又变了");
        assert_eq!(
            after_down_main, after_down_pane2,
            "主窗格 PgDn 后 Pane2 跟着变"
        );
    }

    /// `title = None` 时行数/行 key 与改动前相等：不插标题行。
    #[test]
    fn a_none_title_adds_no_row_and_keeps_the_same_keys() {
        let mut app = windowing_app(100, 10, 0, true);
        let root = transcript_root(&app);
        assert!(
            pane_of(&app, root).title.is_none(),
            "默认 TranscriptPane 没有标题"
        );
        app.update();
        let shown = rows_of(&app, PanelId::Transcript);
        let want: Vec<u64> = keyed_rows(100)[90..].iter().map(|keyed| keyed.key).collect();
        assert_eq!(
            shown.iter().map(|keyed| keyed.key).collect::<Vec<_>>(),
            want,
            "行 key 就是整段最后 10 行，没有被标题行挤掉/插入"
        );
        assert_eq!(shown.len(), 10);
        assert!(
            !shown
                .iter()
                .any(|keyed| crate::selection::row_text(&keyed.row).contains("PANEL_TITLE")),
            "title 字段还没人读"
        );
    }

    /// 连拆两次仍是两个窗格，`ActiveTranscript` 仍指向同一块 Pane2。
    #[test]
    fn splitting_twice_keeps_exactly_two_panes() {
        fn split_once(
            mut commands: Commands,
            mut tree: ResMut<crate::frame::FrameTree>,
            mut roots: ResMut<PanelRoots>,
            mut rows: ResMut<PanelRows>,
        ) -> bool {
            crate::frame::split_transcript(&mut commands, &mut tree, &mut roots, &mut rows)
        }

        let mut fixture = split_app();
        let pane2 = fixture.pane2;
        assert_eq!(pane_component_count(&mut fixture.app), 2);

        assert!(
            !fixture
                .app
                .world_mut()
                .run_system_once(split_once)
                .unwrap(),
            "第二次拆窗是空操作"
        );
        assert_eq!(pane_component_count(&mut fixture.app), 2, "仍然只有两块转写");
        assert_eq!(
            fixture.app.world().resource::<PanelRoots>().entity(PanelId::Pane2),
            Some(pane2),
            "Pane2 还是同一块"
        );
        assert_eq!(
            fixture.app.world().resource::<ActiveTranscript>(),
            &ActiveTranscript(Some(pane2)),
            "活动窗格仍指向 Pane2"
        );
    }

    /// 活动窗格切换后，Alt+↑↓ / PgDn 作用在新活动窗格上（两块互不影响）。
    #[test]
    fn keys_follow_the_active_pane_after_splitting() {
        let mut fixture = split_app();
        let (transcript, pane2) = (fixture.transcript, fixture.pane2);
        // 两块摆在不同位置，任何一块被动都会看得出来。
        set_pane_scroll(&mut fixture.app, transcript, view(30, false));
        set_pane_scroll(&mut fixture.app, pane2, view(50, false));
        // 拆窗后的活动窗格是 Pane2。
        assert_eq!(
            fixture.app.world().resource::<ActiveTranscript>().0,
            Some(pane2)
        );

        // Alt+↓ 在 Pane2：选第一条并把它顶到窗顶；主窗格不动。
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(pane_scroll(&fixture.app, pane2).top, 0);
        assert_eq!(
            fixture.app.world().get::<TranscriptPane>(pane2).unwrap().selected,
            Some(0)
        );
        assert_eq!(
            pane_scroll(&fixture.app, transcript).top,
            30,
            "主窗格不应被 Pane2 的键改动"
        );

        // PgDn 也作用在 Pane2。
        tap(&mut fixture.app, &[], KeyCode::PageDown);
        assert_eq!(pane_scroll(&fixture.app, pane2).top, 9, "一页 9 行");
        assert_eq!(pane_scroll(&fixture.app, transcript).top, 30);

        // 切到主窗格：同一组键改的是主窗格，Pane2 保持刚才的位置。
        set_active(&mut fixture.app, transcript);
        alt_tap(&mut fixture.app, KeyCode::ArrowDown);
        assert_eq!(pane_scroll(&fixture.app, transcript).top, 0);
        assert_eq!(
            fixture.app.world().get::<TranscriptPane>(transcript).unwrap().selected,
            Some(0)
        );
        tap(&mut fixture.app, &[], KeyCode::PageDown);
        assert_eq!(pane_scroll(&fixture.app, transcript).top, 9);
        assert_eq!(
            pane_scroll(&fixture.app, pane2).top,
            9,
            "Pane2 停在自己最后的位置"
        );
    }

    /// `spawn_panel` 给两块转写根挂默认组件；拆窗把活动窗格指到 `Pane2`。
    #[test]
    fn transcript_panes_start_with_shown_and_no_title() {
        let fixture = split_app();
        for root in [fixture.transcript, fixture.pane2] {
            let pane = pane_of(&fixture.app, root);
            assert!(
                matches!(pane.source, TranscriptSource::Shown),
                "S1 两块都还是 Shown（Pane2 靠镜像填行）"
            );
            assert!(pane.title.is_none(), "默认没有标题行");
            assert_eq!(pane.selected, None);
        }
    }

    /// `editor.rs` 的兼容入口：只改全局 `PaneScroll.follow`，主窗格仍然跟末尾。
    #[test]
    fn the_legacy_pane_scroll_request_still_follows_the_main_pane() {
        let mut fixture = split_app();
        let (transcript, pane2) = (fixture.transcript, fixture.pane2);
        set_pane_scroll(&mut fixture.app, transcript, view(20, false));
        // 拆窗后活动窗格是 Pane2；兼容入口仍按旧语义作用于主窗格。
        assert_eq!(
            fixture.app.world().resource::<ActiveTranscript>().0,
            Some(pane2)
        );
        fixture.app.world_mut().resource_mut::<PaneScroll>().follow = true;
        fixture.app.update();
        let scroll = pane_scroll(&fixture.app, transcript);
        assert_eq!(scroll.top, SPLIT_ROWS - SPLIT_HEIGHT as usize);
        assert!(scroll.follow, "发送后主窗格回到跟随末尾");
    }

    /// 滚轮按指针换算：在 `Pane2` 上滚滚的是 `Pane2`，主窗格不动。
    #[test]
    fn a_wheel_over_pane2_scrolls_pane2_only() {
        let mut fixture = split_app();
        let (transcript, pane2) = (fixture.transcript, fixture.pane2);
        set_pane_scroll(&mut fixture.app, transcript, view(20, false));
        set_pane_scroll(&mut fixture.app, pane2, view(50, false));
        let row = fixture
            .app
            .world_mut()
            .spawn(RowLayout {
                panel: PanelId::Pane2,
                panel_entity: pane2,
                index: 0,
            })
            .id();
        fixture.app.world_mut().write_message(PointerScroll {
            entity: row,
            pointer: mouse_pointer(),
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            phase: TouchPhase::Moved,
        });
        fixture.app.update();

        assert_eq!(
            pane_scroll(&fixture.app, pane2).top,
            47,
            "指针下的 Pane2 往上滚一格（3 行）"
        );
        assert_eq!(
            pane_scroll(&fixture.app, transcript).top,
            20,
            "主窗格不该跟着滚"
        );
    }

    // ---- F4：滚动条（纯 Node） ----

    /// 某个窗格根下挂的滚动条：轨道实体 + 轨道组件。
    fn scrollbar_of(app: &App, pane: Entity) -> (Entity, TranscriptScrollbar) {
        let track = app
            .world()
            .get::<Children>(pane)
            .expect("窗格根下要有子节点")
            .iter()
            .find(|child| app.world().get::<TranscriptScrollbar>(*child).is_some())
            .expect("每块转写窗格都要有滚动条");
        let bar = *app
            .world()
            .get::<TranscriptScrollbar>(track)
            .expect("轨道组件");
        (track, bar)
    }

    fn bar_model(app: &App, pane: Entity) -> ScrollbarModel {
        scrollbar_of(app, pane).1.model
    }

    fn px_value(value: Val) -> f32 {
        match value {
            Val::Px(value) => value,
            other => panic!("期望 Val::Px，得到 {other:?}"),
        }
    }

    /// 100 行、可见 20 行：滑块长度 ≈ 20%，位置随 `top` 变，滚到底贴底。
    #[test]
    fn a_scrollbar_thumb_shows_the_visible_fraction_position_and_bottom() {
        let mut fixture = split_app();
        for root in [fixture.transcript, fixture.pane2] {
            fixture
                .app
                .world_mut()
                .resource_mut::<PanelAreas>()
                .0
                .insert(root, pane_area(20));
        }
        set_pane_scroll(&mut fixture.app, fixture.transcript, view(0, false));
        set_pane_scroll(&mut fixture.app, fixture.pane2, view(10, false));
        fixture.app.update();

        let main = bar_model(&fixture.app, fixture.transcript);
        assert_eq!(main.total, SPLIT_ROWS, "整段 100 行");
        assert_eq!(main.visible, 20, "视口 20 行");
        assert_eq!(main.max_top, 80);
        assert!(
            (main.len - 0.2).abs() < 1e-4,
            "可见 20 / 总 100 → 滑块 20%，实际 {}",
            main.len
        );
        assert!((main.track_px - 20.0 * ROW_HEIGHT).abs() < 1e-4);
        assert_eq!(main.offset, 0.0, "窗顶 0：滑块在顶部");

        let (_, bar) = scrollbar_of(&fixture.app, fixture.transcript);
        let node = fixture
            .app
            .world()
            .get::<Node>(bar.thumb)
            .expect("滑块 Node");
        assert!((px_value(node.height) - main.thumb_px).abs() < 1e-4);
        assert_eq!(px_value(node.top), 0.0);

        // Pane2 有自己的一份 scroll：滑块位置跟着它，不跟主窗格。
        let pane2 = bar_model(&fixture.app, fixture.pane2);
        assert!(
            (pane2.offset - 10.0 / 80.0).abs() < 1e-4,
            "Pane2 top=10 / max_top=80 → 行程 12.5%"
        );
        let (_, bar2) = scrollbar_of(&fixture.app, fixture.pane2);
        let node2 = fixture
            .app
            .world()
            .get::<Node>(bar2.thumb)
            .expect("Pane2 滑块");
        assert!(
            (px_value(node2.top) - pane2.thumb_top_px()).abs() < 1e-4,
            "Pane2 的滑块位置独立"
        );

        // 主窗格滚到中间：滑块位置 = top / max_top。
        set_pane_scroll(&mut fixture.app, fixture.transcript, view(40, false));
        fixture.app.update();
        let mid = bar_model(&fixture.app, fixture.transcript);
        assert!((mid.offset - 0.5).abs() < 1e-4, "40 / 80 = 一半行程");
        let (_, bar) = scrollbar_of(&fixture.app, fixture.transcript);
        let node = fixture
            .app
            .world()
            .get::<Node>(bar.thumb)
            .expect("滑块 Node");
        assert!(
            (px_value(node.top) - mid.thumb_top_px()).abs() < 1e-4,
            "滑块 top 跟着 top 变"
        );
        assert!((px_value(node.height) - mid.thumb_px).abs() < 1e-4);

        // 跟随末尾：窗口化把 top 放到 80，滑块贴底。
        set_pane_scroll(&mut fixture.app, fixture.transcript, view(0, true));
        fixture.app.update();
        let tail = bar_model(&fixture.app, fixture.transcript);
        assert_eq!(tail.top, 80, "跟随时有效窗顶 = max_top");
        assert!(tail.is_at_bottom(), "滚到底/跟随时滑块贴底");
        let (_, bar) = scrollbar_of(&fixture.app, fixture.transcript);
        let node = fixture
            .app
            .world()
            .get::<Node>(bar.thumb)
            .expect("滑块 Node");
        let bottom = px_value(node.top) + px_value(node.height);
        assert!(
            (bottom - tail.track_px).abs() < 1e-4,
            "滑块下沿 {bottom} 要贴住轨道底 {}",
            tail.track_px
        );

        // 轨道不占列宽：宽 6px、绝对定位在右侧 padding 里。
        let (track, _bar) = scrollbar_of(&fixture.app, fixture.transcript);
        let track_node = fixture.app.world().get::<Node>(track).expect("轨道 Node");
        assert_eq!(track_node.width, px(crate::theme::SCROLLBAR_WIDTH));
        assert_eq!(track_node.right, px(0.0));
        assert_eq!(track_node.position_type, PositionType::Absolute);
        assert_eq!(
            track_node.top,
            px(PANEL_PADDING),
            "轨道从内容区顶部开始，不占面板列宽"
        );
    }

    /// 滚轮落在滑块上时也滚它服务的窗格（走现有的 `PointerScroll` 路径）。
    #[test]
    fn a_wheel_over_the_scrollbar_thumb_scrolls_its_pane() {
        let mut fixture = split_app();
        set_pane_scroll(&mut fixture.app, fixture.transcript, view(20, false));
        fixture.app.update();
        let (_, bar) = scrollbar_of(&fixture.app, fixture.transcript);
        fixture.app.world_mut().write_message(PointerScroll {
            entity: bar.thumb,
            pointer: mouse_pointer(),
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            phase: TouchPhase::Moved,
        });
        fixture.app.update();
        assert_eq!(
            pane_scroll(&fixture.app, fixture.transcript).top,
            17,
            "滑块上的滚轮一样滚它所服务的窗格（一格 3 行）"
        );
    }

    /// 拖滑块：`PointerDragStart` 记起点，`PointerDrag` 按总位移换算并夹在
    /// `[0, max_top]`；拖到底恢复跟随。
    #[test]
    fn dragging_the_scrollbar_thumb_moves_top_and_clamps() {
        let mut fixture = split_app();
        for root in [fixture.transcript, fixture.pane2] {
            fixture
                .app
                .world_mut()
                .resource_mut::<PanelAreas>()
                .0
                .insert(root, pane_area(20));
        }
        set_pane_scroll(&mut fixture.app, fixture.transcript, view(0, false));
        fixture.app.update();
        let (_, bar) = scrollbar_of(&fixture.app, fixture.transcript);
        let thumb = bar.thumb;
        assert_eq!(bar.model.max_top, 80);
        let travel = bar.model.track_px - bar.model.thumb_px;
        assert!(travel > 0.0);

        let start = |world: &mut World| {
            world.trigger(PointerDragStart {
                entity: thumb,
                pointer: mouse_pointer(),
                button: PointerButton::Primary,
                hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            });
        };
        let drag = |world: &mut World, dy: f32| {
            world.trigger(PointerDrag {
                entity: thumb,
                pointer: mouse_pointer(),
                button: PointerButton::Primary,
                distance: Vec2::new(0.0, dy),
                delta: Vec2::new(0.0, dy),
            });
        };

        // 拖过半个行程 → 窗顶走到一半。
        start(fixture.app.world_mut());
        drag(fixture.app.world_mut(), travel * 0.5);
        let half = pane_scroll(&fixture.app, fixture.transcript);
        assert_eq!(half.top, 40);
        assert!(!half.follow);

        // 再往下猛拖：夹在 max_top，并恢复跟随末尾。
        start(fixture.app.world_mut());
        drag(fixture.app.world_mut(), 10_000.0);
        let bottom = pane_scroll(&fixture.app, fixture.transcript);
        assert_eq!(bottom.top, 80, "不会超过 max_top");
        assert!(bottom.follow, "拖到底恢复跟随");

        // 往上猛拖：夹在 0。
        start(fixture.app.world_mut());
        drag(fixture.app.world_mut(), -10_000.0);
        let top = pane_scroll(&fixture.app, fixture.transcript);
        assert_eq!(top.top, 0, "不会小于 0");
        assert!(!top.follow);
    }

    /// 滚动条只是多画一条轨道，不改窗口化：`↓ more below` / 跟随末尾都照旧。
    #[test]
    fn the_scrollbar_does_not_change_the_below_marker() {
        let mut fixture = split_app();
        set_pane_scroll(&mut fixture.app, fixture.transcript, view(20, false));
        fixture.app.update();
        let (_track, bar) = scrollbar_of(&fixture.app, fixture.transcript);
        assert!(bar.model.overflow, "100 行 / 可见 10 行：有可滚行程");
        let rows = rows_of(&fixture.app, PanelId::Transcript);
        let last = crate::selection::row_text(&rows.last().expect("有行").row);
        assert!(
            last.contains("more below"),
            "滚上去最后一行仍是 more below：{last:?}"
        );

        // 跟随末尾：标记消失、滑块贴底。
        set_pane_scroll(&mut fixture.app, fixture.transcript, view(0, true));
        fixture.app.update();
        let rows = rows_of(&fixture.app, PanelId::Transcript);
        assert!(
            !rows.iter().any(|keyed| {
                let text = crate::selection::row_text(&keyed.row);
                text.contains("PgDn")
            }),
            "跟随时不画标记：{rows:?}"
        );
        assert!(
            bar_model(&fixture.app, fixture.transcript).is_at_bottom(),
            "跟随时滑块贴底"
        );
    }
}

