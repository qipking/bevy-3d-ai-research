//! 鼠标拖选：把面板里的一段文字变成可复制、可高亮的选区。
//!
//! 上游 rig-tui 0.47 的 `selection.rs` 只有 54 行：它直接按终端 buffer 的
//! 格子选，鼠标事件在 `input.rs::moused`，复制在 `render.rs` 的绘制回调里。
//! 这里没有 buffer，行是 [`Row`]（ratatui `Line`），面板是 [`PanelId`]，
//! 所以把语义抽成与渲染无关的数据模型：
//!
//! - 位置 = **行号 + grapheme 列**（不是字节）：与 `editor.rs` 的光标一致，
//!   CJK、组合字符、emoji ZWJ 序列都只算一列。
//! - 鼠标给的是**格子列**（CJK 占两格）；[`hit_test`] 把它换算成 grapheme
//!   列。选区边界永远落在 grapheme 边界上，不会切开一个字符。
//! - 复制：每行 `trim_end`，行间 `\n`，整体再 `trim_end`，与上游
//!   `Selection::mark` 的 `line.trim_end()` + `lines.join("\n").trim_end()`
//!   一致（行尾空白裁掉、末尾没有多余换行）。
//! - 高亮：调用方拿 [`Selection::ranges_in`] / [`Selection::all_ranges`] 的
//!   字符区间去给 span 套样式；上游用的是 `Modifier::REVERSED`
//!   （[`SELECTED_MODIFIER`]）。
//!
//! 鼠标语义：
//!
//! | 操作 | 接口 | 结果 |
//! |---|---|---|
//! | 按下左键 | [`Selection::press`] | 从这一格开始拖选 |
//! | 拖动 | [`Selection::drag`] | 扩展选区（反向也行） |
//! | 松开 | [`Selection::release`] | 结束拖选，返回要复制的文本 |
//! | 双击 | [`Selection::press_kind`]（或用 [`Selection::press`] 自动数） | 选中当前词 |
//! | 三击 | 同上 | 选中整行 |
//! | 点空白 | 行越界，或 [`Selection::clear`] | 清除选区 |
//! | 滚轮 | [`crate::pane::pane_wheel`] | 滚走的内容收起选区（滚动本身在 `pane.rs`） |
//! | 窗口/面板尺寸变化 | [`clear_selection_on_resize`] | 清除选区（上游 `Event::Resize`） |
//!
//! 上游 0.47 本身只有拖选，没有双击/三击；这里按任务要求补齐终端惯例。
//! 键盘 Shift+方向不在本模块范围内。

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use bevy::picking::events::{PointerMove, PointerPress, PointerRelease};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::ComputedNode;
use bevy::window::WindowResized;
use feathers_in_3d::WorldPanel;
use rig_tui::AppStatus;
use rig_tui::ratatui::style::{Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::clipboard::{self, Copied, MemoryClipboard};
use crate::panels::{PanelId, PanelRoots, PanelRows, RowLayout};
use crate::transcript::Row;

/// 上游给选中格子加的样式：反色。调用方把选中区间套上它即可。
pub const SELECTED_MODIFIER: Modifier = Modifier::REVERSED;

/// 双击/三击的判定窗口：两次按下间隔不超过这么久，且落在同一格附近。
///
/// 与常见终端的默认双击间隔一致（500ms）。
pub const MULTI_CLICK: Duration = Duration::from_millis(500);

/// 转写里的一个位置：行下标 + 行内第几个 grapheme（从 0 数）。
///
/// **不是字节**，也不是终端格子：`editor.rs` 的光标同样按 grapheme 走，
/// 这样 CJK（一个汉字一格字符、两格显示）和组合字符都不会被切开。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    /// 行下标（从 0 数）。
    pub row: usize,
    /// 行内第几个 grapheme（从 0 数）。
    pub col: usize,
}

impl Point {
    /// 第一行第一个字符。
    pub const ZERO: Self = Self { row: 0, col: 0 };

    /// 行 `row`、第 `col` 个 grapheme。
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// 一次点击被数成了几下。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickKind {
    /// 单选：开始（或收起）拖选。
    Single,
    /// 双击：选当前词。
    Double,
    /// 三击：选整行。
    Triple,
}

/// 上一次按下的记录，用来数连击。
#[derive(Clone, Copy, Debug)]
struct ClickRecord {
    panel: PanelId,
    at: Point,
    when: Instant,
    /// 这一串连击已经数到第几下（1..=3）。
    count: u8,
}

/// 当前的鼠标选区。
///
/// 一个资源就够：视图里同时只有一个拖选（上游 `TuiView::selection` 也是
/// 一个 `Option<Selection>`）。`anchor` 是按下处，`focus` 是鼠标当前/松开处；
/// 两者顺序相反就是反向拖选，取值时用 [`Selection::start`] /
/// [`Selection::end`] 归一化。
///
/// `copied` 记着松手复制了几行（上游同名），状态行据此显示 `Copied N lines`；
/// 下一次按下或 [`Selection::clear`] 时清掉。
#[derive(Resource, Clone, Debug)]
pub struct Selection {
    /// 选区所在面板；`None` 表示还没有按过。
    panel: Option<PanelId>,
    /// 按下处（拖选时不动）。
    anchor: Point,
    /// 鼠标当前处 / 松开处。
    focus: Point,
    /// 左键还按着：`drag` 才会扩展选区。
    dragging: bool,
    /// 上一次成功复制了几行；`None` 表示没有提示。
    copied: Option<usize>,
    /// 连击计数（双击/三击）。
    clicked: Option<ClickRecord>,
}

impl Default for Selection {
    fn default() -> Self {
        Self::new()
    }
}

impl Selection {
    /// 空选区。
    pub const fn new() -> Self {
        Self {
            panel: None,
            anchor: Point::ZERO,
            focus: Point::ZERO,
            dragging: false,
            copied: None,
            clicked: None,
        }
    }

    /// 选区所在面板。
    pub fn panel(&self) -> Option<PanelId> {
        self.panel
    }

    /// 按下处（反向拖选时比 [`Selection::focus`] 靠后）。
    pub fn anchor(&self) -> Point {
        self.anchor
    }

    /// 鼠标当前处 / 松开处。
    pub fn focus(&self) -> Point {
        self.focus
    }

    /// 归一化后的起点（含）。
    pub fn start(&self) -> Point {
        self.ordered().0
    }

    /// 归一化后的终点（不含）。
    pub fn end(&self) -> Point {
        self.ordered().1
    }

    /// 是否有一段非空选区（起止不同）。
    pub fn is_active(&self) -> bool {
        self.panel.is_some() && self.anchor != self.focus
    }

    /// [`Selection::is_active`] 的别名，语义更直白。
    pub fn has_selection(&self) -> bool {
        self.is_active()
    }

    /// 左键是否还按着（拖选进行中）。
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// 上一次复制了几行。
    pub fn copied(&self) -> Option<usize> {
        self.copied
    }

    /// 记下复制了几行，状态行据此显示 `Copied N lines`。
    pub fn mark_copied(&mut self, lines: usize) {
        self.copied = Some(lines);
    }

    /// 清除选区与连击状态（点空白、滚动、窗口 resize 时调）。
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// 左键按下：`row` 是行下标，`column` 是**格子列**（CJK 占两格）。
    ///
    /// 500ms 内落在同一格的连续按下自动数成双击/三击；行越界（点空白）
    /// 清掉选区并返回 `None`。返回这一次被当成几下，方便调用方播放反馈。
    pub fn press(
        &mut self,
        panel: PanelId,
        rows: &[Row],
        row: usize,
        column: usize,
        now: Instant,
    ) -> Option<ClickKind> {
        let Some(at) = hit_test(rows, row, column) else {
            self.clear();
            return None;
        };
        let kind = self.note_click(panel, at, now);
        self.apply(panel, rows, at, kind);
        Some(kind)
    }

    /// 按下左键，点击次数由调用方判定（比如窗口/系统自己的双击间隔）。
    ///
    /// 与 [`Selection::press`] 的区别只是不按时间数连击；测试和已经自己
    /// 数好连击的调用方用这个。
    pub fn press_kind(
        &mut self,
        panel: PanelId,
        rows: &[Row],
        row: usize,
        column: usize,
        kind: ClickKind,
    ) {
        self.clicked = None;
        let Some(at) = hit_test(rows, row, column) else {
            self.clear();
            return;
        };
        self.apply(panel, rows, at, kind);
    }

    /// 左键拖动到 `row`/`column`：扩展选区。
    ///
    /// 只有左键还按着、且还是同一个面板时才动；行越界就停在最后一个
    /// 有效位置（不跳到别的面板）。
    pub fn drag(&mut self, panel: PanelId, rows: &[Row], row: usize, column: usize) {
        if !self.dragging || self.panel != Some(panel) {
            return;
        }
        if let Some(at) = hit_test(rows, row, column) {
            self.focus = at;
        }
    }

    /// 左键松开：结束拖选，返回要复制的文本（没有选区/全是空白时 `None`）。
    ///
    /// 选区本身保留（松手后还高亮，直到下一次按下或 [`Selection::clear`]），
    /// 与上游一致；复制与 `Copied N lines` 提示由剪贴板模块负责。
    pub fn release(&mut self, rows: &[Row]) -> Option<String> {
        self.dragging = false;
        self.selected_text(rows)
    }

    /// 选中内容：跨行用 `\n` 连接；每行行尾空白裁掉，整体末尾不留换行。
    ///
    /// 与上游 `Selection::mark` 相同：`line.trim_end()` 后 `join("\n")`，
    /// 最后再 `trim_end()`。因此“选到下一行行首”不会带出一个多余的末尾
    /// 换行，中间的空行则原样保留。
    pub fn selected_text(&self, rows: &[Row]) -> Option<String> {
        let (start, end) = self.ordered();
        if start == end || rows.is_empty() || start.row >= rows.len() {
            return None;
        }
        // 行可能因为滚动/重建变短：越界的终点收到最后一行的行尾，不 panic。
        let last = end.row.min(rows.len() - 1);
        let mut lines = Vec::with_capacity(last - start.row + 1);
        for index in start.row..=last {
            let text = row_text(&rows[index]);
            let count = grapheme_count(&text);
            let from = if index == start.row {
                start.col.min(count)
            } else {
                0
            };
            let to = if index == end.row {
                end.col.min(count)
            } else {
                count
            };
            lines.push(char_slice(&text, from, to).trim_end().to_owned());
        }
        let text = lines.join("\n").trim_end().to_owned();
        (!text.is_empty()).then_some(text)
    }

    /// 第 `row` 行被选中的**字符（grapheme）区间**，左闭右开。
    ///
    /// 通常 0 或 1 段；返回 `Vec` 是为了和任务约定的形状一致，也方便将来
    /// 做断行高亮。调用方按它给 span 套 [`SELECTED_MODIFIER`]。
    pub fn ranges_in(&self, row: usize, rows: &[Row]) -> Vec<(usize, usize)> {
        let Some(row_ref) = rows.get(row) else {
            return Vec::new();
        };
        if !self.is_active() {
            return Vec::new();
        }
        let (start, end) = self.ordered();
        if row < start.row || row > end.row {
            return Vec::new();
        }
        let count = row_chars(row_ref);
        let from = if row == start.row {
            start.col.min(count)
        } else {
            0
        };
        let to = if row == end.row {
            end.col.min(count)
        } else {
            count
        };
        if from < to {
            vec![(from, to)]
        } else {
            Vec::new()
        }
    }

    /// [`Selection::ranges_in`] 的字节版本：区间是行纯文本（ [`row_text`]）
    /// 的字节偏移，可直接拿去切 span。
    pub fn byte_ranges_in(&self, row: usize, rows: &[Row]) -> Vec<(usize, usize)> {
        let Some(row_ref) = rows.get(row) else {
            return Vec::new();
        };
        let text = row_text(row_ref);
        self.ranges_in(row, rows)
            .into_iter()
            .map(|(from, to)| (byte_offset(&text, from), byte_offset(&text, to)))
            .collect()
    }

    /// 所有被选中的行：`(行下标, 字符区间)`。没有选中的行不会出现。
    ///
    /// 调用方遍历这个就能给每一行套样式，不用自己算跨行边界。
    pub fn all_ranges(&self, rows: &[Row]) -> Vec<(usize, Vec<(usize, usize)>)> {
        if !self.is_active() || rows.is_empty() {
            return Vec::new();
        }
        let (start, end) = self.ordered();
        if start.row >= rows.len() {
            return Vec::new();
        }
        let last = end.row.min(rows.len() - 1);
        (start.row..=last)
            .filter_map(|row| {
                let ranges = self.ranges_in(row, rows);
                (!ranges.is_empty()).then_some((row, ranges))
            })
            .collect()
    }

    /// 归一化的起止（反向拖选时交换）。
    fn ordered(&self) -> (Point, Point) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    /// 数连击：同一面板、同一格附近、间隔不超过 [`MULTI_CLICK`]。
    fn note_click(&mut self, panel: PanelId, at: Point, now: Instant) -> ClickKind {
        let count = match self.clicked {
            Some(previous)
                if previous.panel == panel
                    && previous.at.row == at.row
                    && previous.at.col.abs_diff(at.col) <= 1
                    && now.saturating_duration_since(previous.when) <= MULTI_CLICK =>
            {
                match previous.count {
                    1 => 2,
                    2 => 3,
                    // 第四下回到单选：终端惯例。
                    _ => 1,
                }
            }
            _ => 1,
        };
        self.clicked = Some(ClickRecord {
            panel,
            at,
            when: now,
            count,
        });
        match count {
            1 => ClickKind::Single,
            2 => ClickKind::Double,
            _ => ClickKind::Triple,
        }
    }

    /// 按点击种类落选区。
    fn apply(&mut self, panel: PanelId, rows: &[Row], at: Point, kind: ClickKind) {
        self.panel = Some(panel);
        self.copied = None;
        self.dragging = false;
        match kind {
            ClickKind::Single => {
                self.anchor = at;
                self.focus = at;
                self.dragging = true;
            }
            ClickKind::Double => {
                let (from, to) = word_range(rows, at.row, at.col).unwrap_or((at.col, at.col));
                self.anchor = Point::new(at.row, from);
                self.focus = Point::new(at.row, to);
            }
            ClickKind::Triple => {
                let to = line_range(rows, at.row).map_or(at.col, |(_, to)| to);
                self.anchor = Point::new(at.row, 0);
                self.focus = Point::new(at.row, to);
            }
        }
    }
}

/// 一行的纯文本：`line` 的 span 接上 `right` 的 span，与 `transcript.rs`
/// 里 `RowText` 拼法一致（不额外插空格）。
pub fn row_text(row: &Row) -> String {
    let mut text: String = row
        .line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    if let Some(right) = &row.right {
        text.extend(right.spans.iter().map(|span| span.content.as_ref()));
    }
    text
}

/// 一行的 grapheme 数（选区的列数）。
pub fn row_chars(row: &Row) -> usize {
    grapheme_count(&row_text(row))
}

/// 一行在终端格子上占的宽度：CJK/全角/emoji 按 2 格算。
///
/// 和上游按 `Cell::cell_width` 选格子一致；宽度为 0 的 grapheme（孤立的
/// 组合符号等）按 1 格算，保证总能点到。
pub fn row_width(row: &Row) -> usize {
    row_text(row)
        .graphemes(true)
        .map(|grapheme| grapheme.width().max(1))
        .sum()
}

/// 每一行的格子宽度（调用方按面板宽度算点击列时可以用）。
pub fn row_widths(rows: &[Row]) -> Vec<usize> {
    rows.iter().map(row_width).collect()
}

/// 把**格子列**换算成行内第几个 grapheme。
///
/// 点在一个宽字符覆盖的任意一格上，都算点中这个字符；越过行尾给
/// `grapheme_count`（行尾位置）。
pub fn cell_to_char(text: &str, column: usize) -> usize {
    let mut cells = 0;
    let mut count = 0;
    for grapheme in text.graphemes(true) {
        let width = grapheme.width().max(1);
        if column < cells + width {
            return count;
        }
        cells += width;
        count += 1;
    }
    count
}

/// 命中测试：`row` 行、`column` 格子列 -> 选区的字符位置。
///
/// 行越界（点空白/面板外）返回 `None`；列越界收到行尾。
pub fn hit_test(rows: &[Row], row: usize, column: usize) -> Option<Point> {
    let text = row_text(rows.get(row)?);
    Some(Point::new(row, cell_to_char(&text, column)))
}

/// 双击选词：`col` 所在的那一串“词字符”（字母、数字、`_`，CJK 也算）
/// 的起止 grapheme 下标（左闭右开）。
///
/// 点在词后面的行尾位置时选最后一个词；点标点/空白时选它所在的那一串
/// 同类字符。
pub fn word_range(rows: &[Row], row: usize, col: usize) -> Option<(usize, usize)> {
    let text = row_text(rows.get(row)?);
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    if graphemes.is_empty() {
        return Some((0, 0));
    }
    let at = col.min(graphemes.len() - 1);
    let word = is_word(graphemes[at]);
    let mut from = at;
    while from > 0 && is_word(graphemes[from - 1]) == word {
        from -= 1;
    }
    let mut to = at + 1;
    while to < graphemes.len() && is_word(graphemes[to]) == word {
        to += 1;
    }
    Some((from, to))
}

/// 三击选行：整行的 grapheme 区间 `(0, 行字符数)`；行越界给 `None`。
pub fn line_range(rows: &[Row], row: usize) -> Option<(usize, usize)> {
    Some((0, row_chars(rows.get(row)?)))
}

/// `grapheme` 是否属于一个词：首字符是字母、数字或 `_`。
///
/// 与 `editor.rs` 的 `word_grapheme` 同一套规则，所以双击选词和 Ctrl+W
/// 对“词”的理解一致；CJK 字符 `is_alphanumeric()`，整段连写算一个词。
fn is_word(grapheme: &str) -> bool {
    grapheme
        .chars()
        .next()
        .is_some_and(|first| first.is_alphanumeric() || first == '_')
}

/// 文本的 grapheme 数。
pub fn grapheme_count(text: &str) -> usize {
    text.graphemes(true).count()
}

/// 第 `grapheme` 个 grapheme 的字节偏移；越过末尾给 `text.len()`。
fn byte_offset(text: &str, grapheme: usize) -> usize {
    if grapheme == 0 {
        return 0;
    }
    text.grapheme_indices(true)
        .nth(grapheme)
        .map_or(text.len(), |(at, _)| at)
}

/// `text` 里第 `from..to` 个 grapheme 的切片。
fn char_slice(text: &str, from: usize, to: usize) -> &str {
    let start = byte_offset(text, from);
    let end = byte_offset(text, to).max(start);
    &text[start..end]
}

// ---------------------------------------------------------------------------
// 接线：3D 面板上的鼠标事件 -> Selection
// ---------------------------------------------------------------------------
//
// 数据流：
//
// ```text
// bevy_picking PointerPress/Move/Release（行实体是拾取目标）
//        │  HitData.position 是世界坐标命中点
//        ▼
// WorldPanel.anchor 逆变换 -> 面板局部像素（原点=面板中心，y 朝下）
//        │  RowLayout（面板根实体 + 行下标）
//        ▼
// 行内 x -> 终端格子列 -> Selection::press_kind / drag / release
//        │
//        └─ 松手：clipboard::copy_text + mark_copied + Copied 消息
// ```
//
// 为什么拖动用 `PointerMove` 而不是 `PointerDrag`：bevy 0.20 的
// `PointerDrag` 只带窗口坐标的位移，**不带 `HitData`**；拖动要的是"鼠标
// 现在落在哪一行、行内哪一格"，而按住时的 `PointerMove` 每帧都带命中点。
// 按下/松开仍然走 `PointerPress`/`PointerRelease`（它们带 `HitData`）。

/// 一次鼠标拾取换算出来的位置：哪块面板、第几行、行内第几**格**。
///
/// `column` 是终端格子列（CJK 占两格），直接交给 [`hit_test`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowHit {
    /// 行来自哪块面板。
    pub panel: PanelId,
    /// 面板里的行下标（[`RowLayout::index`]）。
    pub row: usize,
    /// 行内终端格子列（CJK 占两格）。
    pub column: usize,
}

/// 世界坐标命中点 -> 面板局部像素（原点在面板中心，y 朝下）。
///
/// 与 `feathers_in_3d::pick` 的换算一致：先用面板 `anchor` 的逆变换回到
/// 面板局部米，再除 `units_to_world` 变回 UI 物理像素。
pub fn panel_local_pixels(position: Vec3, panel: &WorldPanel) -> Vec2 {
    let local = panel.anchor.to_matrix().inverse().transform_point3(position);
    Vec2::new(
        local.x / panel.units_to_world,
        -local.y / panel.units_to_world,
    )
}

/// 面板根节点的正文左边缘（面板中心为原点，UI 物理像素）。
///
/// 行是面板的直接子节点、宽度 100% 拉伸，所以行的左边缘就是内容区左边：
/// 面板尺寸减掉边框与内边距（[`ComputedNode::content_box`]）。
pub fn content_left(node: &ComputedNode) -> f32 {
    node.content_box().min.x
}

/// 行内 x（面板物理像素）-> 终端格子列。
///
/// 一行按等宽字体排：一个终端格子是 [`crate::frame::CELL`].x 像素
/// （FiraMono 13px 的 advance = 0.6em = 7.8px）。CJK 占两格这层语义在
/// [`hit_test`]/[`cell_to_char`] 里，这里只给格子列；点在本行左边之外按
/// 第 0 格算。
pub fn cell_column(x: f32, text_left: f32) -> usize {
    let cells = (x - text_left) / crate::frame::CELL.x;
    cells.max(0.0) as usize
}

/// 把一次拾取换算成 [`RowHit`]；目标不是面板行（点空白/面板背景/窗口）
/// 或命中点没有世界坐标时给 `None`。
fn hit_row(
    entity: Entity,
    hit: &bevy::picking::backend::HitData,
    layouts: &Query<&RowLayout>,
    panels: &Query<&WorldPanel>,
    nodes: &Query<&ComputedNode>,
) -> Option<RowHit> {
    let layout = layouts.get(entity).ok()?;
    let position = hit.position?;
    let panel = panels.get(layout.panel_entity).ok()?;
    let node = nodes.get(layout.panel_entity).ok()?;
    let local = panel_local_pixels(position, panel);
    Some(RowHit {
        panel: layout.panel,
        row: layout.index,
        column: cell_column(local.x, content_left(node)),
    })
}

/// bevy 已经数好的连击次数 -> 选区语义。
///
/// 终端惯例：第 4 下回到单选（`Selection::press` 自己数连击时也是这个规则，
/// 见 `note_click`）。
fn click_kind(count: u8) -> ClickKind {
    match count {
        2 => ClickKind::Double,
        3 => ClickKind::Triple,
        _ => ClickKind::Single,
    }
}

/// 松手复制：优先内存替身（单测/显式后备），其次系统剪贴板，都没有就报错。
///
/// 不 panic：错误文本交给状态行（[`clipboard::show_copy_failed`]）。
fn copy_released(
    text: &str,
    system: Option<&mut bevy_clipboard::Clipboard>,
    memory: Option<&mut MemoryClipboard>,
) -> Result<usize, String> {
    match (memory, system) {
        (Some(memory), _) => clipboard::copy_text(text, memory),
        (None, Some(system)) => clipboard::copy_text(text, system),
        (None, None) => Err(clipboard::NO_CLIPBOARD.to_owned()),
    }
}

/// 上一次看到的面板尺寸：`(面板根实体, UI 物理像素尺寸)`。
///
/// 只用来发现 [`clear_selection_on_resize`] 要处理的"面板尺寸变了"；没有选区
/// 或面板查不到时是 `None`（不计 resize）。
#[derive(Resource, Default, Debug)]
pub struct PanelSizeSeen(Option<(Entity, Vec2)>);

/// 面板尺寸变化多少物理像素才算 resize：f32 布局的抖动不算。
const RESIZE_EPSILON: f32 = 0.5;

/// resize 时清选区：上游 `input.rs` 的 `Event::Resize(width, height)` 分支
/// （`view.selection = None`）。
///
/// 两种变化都算 resize：
///
/// - **窗口尺寸**：`WindowResized`（拖大拖小、换显示器）。上游收到终端 resize
///   就先清选区，不等布局算完；
/// - **面板尺寸**：选区所在面板的 [`ComputedNode`] 尺寸变了——帧布局/分裂树
///   重排、编辑器面板长高把转写挤矮，都可能是窗口没动而面板动了。
///
/// 这里只清选区，不动滚动：行的窗口化在 [`crate::pane::window_transcript`] 里
/// 每帧按当前区域重算（上游 resize 也只重排、清选区，不动滚动位置）。
///
/// 第一次看到一块面板的尺寸只记基准、不算 resize：否则选区刚落好就被自己清了。
/// 面板尺寸资源缺失（没有面板/没有 UI 布局的测试 App）时退化成只看窗口 resize。
pub fn clear_selection_on_resize(
    mut resized: MessageReader<WindowResized>,
    mut seen: ResMut<PanelSizeSeen>,
    roots: Option<Res<PanelRoots>>,
    nodes: Query<&ComputedNode>,
    mut selection: ResMut<Selection>,
) {
    // 一帧里可能来好几条：读干净，只看有没有。
    let window_resized = resized.read().count() > 0;

    let panel_size = selection
        .panel()
        .and_then(|id| roots.as_deref().and_then(|roots| roots.entity(id)))
        .and_then(|entity| {
            let node = nodes.get(entity).ok()?;
            Some((entity, node.size))
        });

    if window_resized {
        if selection.panel().is_some() {
            selection.clear();
        }
        // 布局这一帧还没按新窗口算完：下一帧重新记基准。
        *seen = PanelSizeSeen(None);
        return;
    }

    let Some((entity, size)) = panel_size else {
        *seen = PanelSizeSeen(None);
        return;
    };

    match seen.0 {
        Some((seen_entity, seen_size)) if seen_entity == entity => {
            if seen_size.abs_diff_eq(size, RESIZE_EPSILON) {
                return;
            }
            // 同一块面板尺寸变了：行会按新尺寸重新窗口化，旧的选区下标不再
            // 对应同一段内容（上游 resize 也清选区）。
            selection.clear();
            *seen = PanelSizeSeen(Some((entity, size)));
        }
        // 第一次看到这块面板（或选区换到了别的面板）：只记基准，按 resize 处理
        // 会把刚拖好的选区立刻清掉。
        _ => {
            *seen = PanelSizeSeen(Some((entity, size)));
        }
    }
}

/// 鼠标 -> 选区：按下、拖动、松开；松手时把选中的文字写进剪贴板。
///
/// 系统参数全都可以缺省（`Option<Res<...>>`），所以没有窗口/剪贴板/状态行
/// 的测试 App 也能跑：点空白清空、有内存剪贴板就复制、没有就写错误状态。
#[allow(clippy::too_many_arguments)]
pub fn pointer_selection(
    mut presses: MessageReader<PointerPress>,
    mut moves: MessageReader<PointerMove>,
    mut releases: MessageReader<PointerRelease>,
    layouts: Query<&RowLayout>,
    panels: Query<&WorldPanel>,
    nodes: Query<&ComputedNode>,
    panel_rows: Res<PanelRows>,
    mut selection: ResMut<Selection>,
    mut system_clipboard: Option<ResMut<bevy_clipboard::Clipboard>>,
    mut memory_clipboard: Option<ResMut<MemoryClipboard>>,
    mut copied: MessageWriter<Copied>,
    mut status: Option<ResMut<AppStatus>>,
) {
    // 按下：从这一格起拖；连击次数由 bevy_picking 数好（同一行、500ms 内）。
    for press in presses.read() {
        if press.button != PointerButton::Primary {
            continue;
        }
        // 新的一次按下：清掉上一次的复制失败提示（成功提示由 apply 清）。
        if let Some(status) = status.as_deref_mut() {
            clipboard::clear_copy_failed(status);
        }
        let Some(hit) = hit_row(press.entity, &press.hit, &layouts, &panels, &nodes) else {
            // 点在没有行的空白上（面板背景、窗口、别的控件）：收起选区。
            selection.clear();
            continue;
        };
        let rows = panel_rows.row_values(hit.panel);
        selection.press_kind(hit.panel, &rows, hit.row, hit.column, click_kind(press.count));
    }

    // 拖动：左键还按着时，逐帧的 PointerMove 把 focus 挪到指针下面那一格。
    // 行/面板越界（拖出面板）时没有 RowHit，focus 停在最后一个有效位置。
    for moved in moves.read() {
        if !selection.is_dragging() {
            continue;
        }
        let Some(hit) = hit_row(moved.entity, &moved.hit, &layouts, &panels, &nodes) else {
            continue;
        };
        let rows = panel_rows.row_values(hit.panel);
        selection.drag(hit.panel, &rows, hit.row, hit.column);
    }

    // 松开：结束拖选并复制。双击/三击在按下时就已经落好选区（dragging
    // 为 false），这里同样要复制，所以不能只看 is_dragging。
    for release in releases.read() {
        if release.button != PointerButton::Primary {
            continue;
        }
        // 松开处还落在同一面板的某一行时，先把 focus 收过去：拖得快时最后
        // 一帧的 PointerMove 可能没来。
        if let Some(hit) = hit_row(release.entity, &release.hit, &layouts, &panels, &nodes)
            && selection.is_dragging()
            && selection.panel() == Some(hit.panel)
        {
            let rows = panel_rows.row_values(hit.panel);
            selection.drag(hit.panel, &rows, hit.row, hit.column);
        }
        let Some(panel) = selection.panel() else {
            continue;
        };
        let rows = panel_rows.row_values(panel);
        let Some(text) = selection.release(&rows) else {
            continue;
        };
        match copy_released(
            &text,
            system_clipboard.as_deref_mut(),
            memory_clipboard.as_deref_mut(),
        ) {
            Ok(lines) => {
                selection.mark_copied(lines);
                copied.write(Copied { lines });
                if let Some(status) = status.as_deref_mut() {
                    clipboard::clear_copy_failed(status);
                }
            }
            Err(why) => {
                // 复制失败不动选区、不显示 Copied：只把原因写进状态行。
                if let Some(status) = status.as_deref_mut() {
                    clipboard::show_copy_failed(status, &why);
                }
            }
        }
    }
}

/// 把 `ranges`（[`row_text`] 的字节区间，按起点升序且不重叠）里的字符套上
/// [`SELECTED_MODIFIER`]，其余字符/style 原样保留；区间会切开 span。
///
/// 行是值，所以高亮不改原行：返回一份带 `REVERSED` 的拷贝，由
/// [`crate::panels::sync_panels`] 交给 `spawn_row*` 画出来。
pub fn selected_row(row: &Row, ranges: &[(usize, usize)]) -> Row {
    if ranges.is_empty() {
        return row.clone();
    }
    let mut styled = row.clone();
    let mut offset = 0;
    styled.line = selected_line(&row.line, ranges, &mut offset);
    styled.right = row
        .right
        .as_ref()
        .map(|line| selected_line(line, ranges, &mut offset));
    styled
}

/// 一行里按字节区间套反色的 span（`offset` 是这一行在整行纯文本里的起点）。
fn selected_line(line: &Line<'static>, ranges: &[(usize, usize)], offset: &mut usize) -> Line<'static> {
    let mut spans = Vec::new();
    for span in &line.spans {
        let text = span.content.as_ref();
        let start = *offset;
        let end = start + text.len();
        *offset = end;

        let mut cursor = 0;
        for &(from, to) in ranges {
            // 区间是整行的字节偏移，剪到这个 span 里。
            let from = from.max(start);
            let to = to.min(end);
            if from >= to {
                continue;
            }
            let from = from - start;
            let to = to - start;
            if cursor < from {
                spans.push(Span::styled(text[cursor..from].to_owned(), span.style));
            }
            spans.push(Span::styled(
                text[from..to].to_owned(),
                selected_style(span.style),
            ));
            cursor = to;
        }
        if cursor < text.len() {
            spans.push(Span::styled(text[cursor..].to_owned(), span.style));
        }
    }
    Line {
        spans,
        style: line.style,
        alignment: line.alignment,
    }
}

/// 只加上 `REVERSED`，其他样式（前景、底色、粗体……）保持不变。
fn selected_style(mut style: Style) -> Style {
    style.add_modifier |= SELECTED_MODIFIER;
    style
}

/// 把上游行 hash 和选区字节区间合成一个重画用的 hash。
///
/// 上游 hash 不含选区，拖动时同一行内容不变但选中区间变了；`sync_panels`
/// 用这个合成值判断要不要重画。没有选中区间时原样返回上游 hash。
pub fn paint_hash(hash: u64, ranges: &[(usize, usize)]) -> u64 {
    if ranges.is_empty() {
        return hash;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for &(from, to) in ranges {
        from.hash(&mut hasher);
        to.hash(&mut hasher);
    }
    hash ^ hasher.finish().rotate_left(1)
}

/// 选区接线的插件：注册 [`Selection`]、指针消息、[`pointer_selection`] 和
/// resize 清选区的 [`clear_selection_on_resize`]。
///
/// 它需要 [`crate::panels::PanelsPlugin`]（`PanelRows`/行实体上的 [`RowLayout`]）
/// 和 `feathers_in_3d` 的 3D 拾取；没有这些资源时系统会被跳过，不会 panic。
/// `PanelRoots` 缺失时 resize 只看窗口消息（面板尺寸那一半退化成 no-op）。
/// 系统放进 [`crate::FeathersSystems::Prepare`]：先清 resize、再处理鼠标，
/// 同一帧的 `Render` 就能画出新的高亮。
///
/// 剪贴板：本插件**不**注册 `bevy_clipboard::ClipboardPlugin`（避免和
/// 应用层重复添加插件）。真机要复制到系统剪贴板，应用还需要加
/// `bevy_clipboard::ClipboardPlugin`；没有它时松手会往 [`AppStatus`] 写
/// [`clipboard::NO_CLIPBOARD`]，不会 panic。单测/无系统剪贴板的后备是
/// 插入 [`MemoryClipboard`]。
pub struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Selection>()
            .init_resource::<PanelSizeSeen>()
            .add_message::<Copied>()
            // 生产里 `WindowPlugin` 已经注册了它；不建窗口的测试 App / 只加
            // 本插件的 App 由这里补上（`add_message` 幂等）。
            .add_message::<WindowResized>()
            .add_message::<PointerPress>()
            .add_message::<PointerMove>()
            .add_message::<PointerRelease>()
            // resize 先清（清的是上一帧的旧选区），同一帧的按下/拖动再落新选区。
            .add_systems(
                Update,
                (clear_selection_on_resize, pointer_selection)
                    .chain()
                    .in_set(crate::FeathersSystems::Prepare),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_tui::ratatui::text::Line;

    fn row(text: &str) -> Row {
        Row::new(Line::from(text.to_owned()))
    }

    fn transcript(lines: &[&str]) -> Vec<Row> {
        lines.iter().map(|line| row(line)).collect()
    }

    fn select(
        rows: &[Row],
        from: (usize, usize),
        to: (usize, usize),
        panel: PanelId,
    ) -> Selection {
        let mut selection = Selection::new();
        selection.press_kind(panel, rows, from.0, from.1, ClickKind::Single);
        selection.drag(panel, rows, to.0, to.1);
        selection
    }

    // ---- 单行 / 跨行 / 反向 ----

    #[test]
    fn single_line_selection_extracts_the_dragged_range() {
        let rows = transcript(&["hello world"]);
        let mut selection = select(
            &rows,
            (0, 1),
            (0, 5),
            PanelId::Transcript,
        );
        assert!(selection.is_active());
        assert_eq!(selection.start(), Point::new(0, 1));
        assert_eq!(selection.end(), Point::new(0, 5));
        assert_eq!(selection.release(&rows).as_deref(), Some("ello"));
        assert!(!selection.is_dragging(), "松开后不再扩展");
        assert!(selection.is_active(), "松手后选区还留着（上游行为）");
    }

    #[test]
    fn cross_line_selection_joins_with_newlines() {
        let rows = transcript(&["hello", "world"]);
        let mut selection = select(
            &rows,
            (0, 2),
            (1, 2),
            PanelId::Transcript,
        );
        assert_eq!(selection.selected_text(&rows).as_deref(), Some("llo\nwo"));
        assert_eq!(selection.release(&rows).as_deref(), Some("llo\nwo"));
    }

    #[test]
    fn reverse_drag_normalizes_the_range_but_keeps_the_anchor() {
        let rows = transcript(&["hello", "world"]);
        let selection = select(
            &rows,
            (1, 3),
            (0, 1),
            PanelId::Transcript,
        );
        assert_eq!(selection.anchor(), Point::new(1, 3), "anchor 是按下处");
        assert_eq!(selection.focus(), Point::new(0, 1), "focus 是鼠标处");
        assert_eq!(selection.start(), Point::new(0, 1));
        assert_eq!(selection.end(), Point::new(1, 3));
        assert_eq!(selection.selected_text(&rows).as_deref(), Some("ello\nwor"));
    }

    #[test]
    fn dragging_in_another_panel_is_ignored() {
        let rows = transcript(&["hello"]);
        let mut selection = select(
            &rows,
            (0, 0),
            (0, 2),
            PanelId::Transcript,
        );
        selection.drag(PanelId::Status, &rows, 0, 5);
        assert_eq!(selection.focus(), Point::new(0, 2));
        assert_eq!(selection.selected_text(&rows).as_deref(), Some("he"));
    }

    // ---- 双击 / 三击 / 连击 ----

    #[test]
    fn double_click_selects_the_english_word_under_the_pointer() {
        let rows = transcript(&["hello, world"]);
        let mut selection = Selection::new();
        selection.press_kind(
            PanelId::Transcript,
            &rows,
            0,
            8,
            ClickKind::Double,
        );
        assert_eq!(selection.start(), Point::new(0, 7));
        assert_eq!(selection.end(), Point::new(0, 12));
        assert_eq!(selection.release(&rows).as_deref(), Some("world"));

        let mut selection = Selection::new();
        selection.press_kind(
            PanelId::Transcript,
            &rows,
            0,
            1,
            ClickKind::Double,
        );
        assert_eq!(selection.selected_text(&rows).as_deref(), Some("hello"));
    }

    #[test]
    fn double_click_selects_a_cjk_run() {
        let rows = transcript(&["你好，世界"]);
        let mut selection = Selection::new();
        // 点“好”（第 2、3 格）。
        selection.press_kind(
            PanelId::Transcript,
            &rows,
            0,
            2,
            ClickKind::Double,
        );
        assert_eq!(selection.start(), Point::new(0, 0));
        assert_eq!(selection.end(), Point::new(0, 2));
        assert_eq!(selection.selected_text(&rows).as_deref(), Some("你好"));
    }

    #[test]
    fn double_click_past_the_end_selects_the_last_word() {
        let rows = transcript(&["hello world"]);
        let mut selection = Selection::new();
        selection.press_kind(
            PanelId::Transcript,
            &rows,
            0,
            999,
            ClickKind::Double,
        );
        assert_eq!(selection.selected_text(&rows).as_deref(), Some("world"));
    }

    #[test]
    fn triple_click_selects_the_whole_line() {
        let rows = transcript(&["  hi  ", "next"]);
        let mut selection = Selection::new();
        selection.press_kind(
            PanelId::Transcript,
            &rows,
            0,
            3,
            ClickKind::Triple,
        );
        assert_eq!(selection.start(), Point::new(0, 0));
        assert_eq!(selection.end(), Point::new(0, 6));
        // 行尾空白裁掉，行首空白保留。
        assert_eq!(selection.release(&rows).as_deref(), Some("  hi"));
    }

    #[test]
    fn consecutive_presses_count_as_double_and_triple_click() {
        let rows = transcript(&["hello world"]);
        let start = Instant::now();
        let mut selection = Selection::new();
        assert_eq!(
            selection.press(PanelId::Transcript, &rows, 0, 1, start),
            Some(ClickKind::Single)
        );
        assert_eq!(
            selection.press(
                PanelId::Transcript,
                &rows,
                0,
                1,
                start + Duration::from_millis(120)
            ),
            Some(ClickKind::Double)
        );
        assert_eq!(
            selection.selected_text(&rows).as_deref(),
            Some("hello")
        );
        assert_eq!(
            selection.press(
                PanelId::Transcript,
                &rows,
                0,
                1,
                start + Duration::from_millis(240)
            ),
            Some(ClickKind::Triple)
        );
        assert_eq!(
            selection.selected_text(&rows).as_deref(),
            Some("hello world")
        );
        // 第四下回到单选。
        assert_eq!(
            selection.press(
                PanelId::Transcript,
                &rows,
                0,
                1,
                start + Duration::from_millis(360)
            ),
            Some(ClickKind::Single)
        );
        assert!(!selection.is_active(), "单选把选区收成一点");
    }

    #[test]
    fn a_slow_second_click_is_a_single_click() {
        let rows = transcript(&["hello world"]);
        let start = Instant::now();
        let mut selection = Selection::new();
        assert_eq!(
            selection.press(PanelId::Transcript, &rows, 0, 1, start),
            Some(ClickKind::Single)
        );
        assert_eq!(
            selection.press(
                PanelId::Transcript,
                &rows,
                0,
                1,
                start + MULTI_CLICK + Duration::from_millis(1)
            ),
            Some(ClickKind::Single)
        );
        assert!(!selection.is_active());
    }

    // ---- 行尾空白 / 空行 / 越界 ----

    #[test]
    fn trailing_blanks_are_trimmed_from_every_line() {
        let rows = transcript(&["hello   ", "world  "]);
        let selection = select(
            &rows,
            (0, 0),
            (1, 7),
            PanelId::Transcript,
        );
        assert_eq!(
            selection.selected_text(&rows).as_deref(),
            Some("hello\nworld")
        );
    }

    #[test]
    fn empty_lines_are_kept_between_text() {
        let rows = transcript(&["a", "", "b"]);
        let selection = select(
            &rows,
            (0, 0),
            (2, 1),
            PanelId::Transcript,
        );
        assert_eq!(selection.selected_text(&rows).as_deref(), Some("a\n\nb"));
    }

    #[test]
    fn a_selection_ending_at_a_line_start_has_no_trailing_newline() {
        let rows = transcript(&["abc", "def"]);
        let selection = select(
            &rows,
            (0, 1),
            (1, 0),
            PanelId::Transcript,
        );
        assert_eq!(selection.selected_text(&rows).as_deref(), Some("bc"));
    }

    #[test]
    fn clicking_outside_the_rows_clears_the_selection() {
        let rows = transcript(&["hello"]);
        let mut selection = select(
            &rows,
            (0, 0),
            (0, 5),
            PanelId::Transcript,
        );
        assert!(selection.is_active());
        let now = Instant::now();
        assert_eq!(
            selection.press(PanelId::Transcript, &rows, 99, 0, now),
            None,
            "行越界算点空白"
        );
        assert!(!selection.is_active());
        assert_eq!(selection.selected_text(&rows), None);
        assert_eq!(selection.release(&rows), None);
    }

    #[test]
    fn columns_past_the_line_end_clamp_to_the_end() {
        let rows = transcript(&["hello"]);
        assert_eq!(hit_test(&rows, 0, 999), Some(Point::new(0, 5)));
        assert_eq!(hit_test(&rows, 3, 0), None);
        let mut selection = Selection::new();
        selection.press_kind(
            PanelId::Transcript,
            &rows,
            0,
            999,
            ClickKind::Single,
        );
        assert!(!selection.is_active(), "点在行尾外收成行尾一点");
    }

    #[test]
    fn an_empty_row_is_hit_testable() {
        let rows = transcript(&["", "x"]);
        assert_eq!(hit_test(&rows, 0, 0), Some(Point::new(0, 0)));
        assert_eq!(row_chars(&rows[0]), 0);
        assert_eq!(row_width(&rows[0]), 0);
        assert_eq!(row_widths(&rows), vec![0, 1]);
    }

    // ---- CJK / grapheme ----

    #[test]
    fn cjk_columns_cover_two_cells() {
        let rows = transcript(&["你好世界"]);
        assert_eq!(row_width(&rows[0]), 8);
        assert_eq!(hit_test(&rows, 0, 0), Some(Point::new(0, 0)));
        assert_eq!(hit_test(&rows, 0, 1), Some(Point::new(0, 0)));
        assert_eq!(hit_test(&rows, 0, 2), Some(Point::new(0, 1)));
        assert_eq!(hit_test(&rows, 0, 3), Some(Point::new(0, 1)));
        assert_eq!(hit_test(&rows, 0, 8), Some(Point::new(0, 4)));
    }

    #[test]
    fn combining_marks_stay_one_character() {
        // e + U+0301 是一个 grapheme（一个格），x 是第二个。
        let rows = transcript(&["e\u{301}x"]);
        assert_eq!(row_width(&rows[0]), 2);
        assert_eq!(hit_test(&rows, 0, 0), Some(Point::new(0, 0)));
        assert_eq!(hit_test(&rows, 0, 1), Some(Point::new(0, 1)));
        let selection = select(
            &rows,
            (0, 0),
            (0, 2),
            PanelId::Transcript,
        );
        assert_eq!(
            selection.selected_text(&rows).as_deref(),
            Some("e\u{301}x")
        );
    }

    #[test]
    fn emoji_zwj_sequences_are_one_character() {
        let family = "👨\u{200d}👩\u{200d}👧";
        let rows = transcript(&[family, "ok"]);
        assert_eq!(row_chars(&rows[0]), 1, "一家人是一个 grapheme");
        assert_eq!(
            hit_test(&rows, 0, 0),
            Some(Point::new(0, 0)),
            "宽 emoji 的第一格"
        );
        assert_eq!(hit_test(&rows, 0, 1), Some(Point::new(0, 0)));
        let selection = select(
            &rows,
            (0, 0),
            (1, 2),
            PanelId::Transcript,
        );
        let expected = format!("{family}\nok");
        assert_eq!(
            selection.selected_text(&rows).as_deref(),
            Some(expected.as_str())
        );
    }

    // ---- 区间 / 字节偏移 ----

    #[test]
    fn ranges_split_a_selection_by_row() {
        let rows = transcript(&["hello", "world"]);
        let selection = select(
            &rows,
            (0, 2),
            (1, 2),
            PanelId::Transcript,
        );
        assert_eq!(
            selection.all_ranges(&rows),
            vec![(0, vec![(2, 5)]), (1, vec![(0, 2)])]
        );
        assert_eq!(selection.ranges_in(0, &rows), vec![(2, 5)]);
        assert_eq!(selection.ranges_in(1, &rows), vec![(0, 2)]);
        assert_eq!(selection.ranges_in(9, &rows), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn ranges_are_empty_without_a_selection() {
        let rows = transcript(&["hello"]);
        let mut selection = Selection::new();
        assert_eq!(selection.all_ranges(&rows), Vec::new());
        selection.press_kind(
            PanelId::Transcript,
            &rows,
            0,
            2,
            ClickKind::Single,
        );
        assert!(!selection.is_active());
        assert_eq!(selection.ranges_in(0, &rows), Vec::new());
    }

    #[test]
    fn byte_ranges_land_on_grapheme_boundaries() {
        let rows = transcript(&["你好世界"]);
        // 鼠标给的是格子列：第 2 格是“好”的第一格，第 6 格是“界”的第一格
        // （也就是选到“世”后面的边界）。
        let selection = select(
            &rows,
            (0, 2),
            (0, 6),
            PanelId::Transcript,
        );
        assert_eq!(selection.ranges_in(0, &rows), vec![(1, 3)]);
        assert_eq!(selection.byte_ranges_in(0, &rows), vec![(3, 9)]);
        assert_eq!(selection.selected_text(&rows).as_deref(), Some("好世"));
    }

    #[test]
    fn all_ranges_skip_empty_rows_at_the_edges() {
        let rows = transcript(&["", "ab", ""]);
        let selection = select(
            &rows,
            (0, 0),
            (2, 0),
            PanelId::Transcript,
        );
        // 第 0 行起止都空；第 2 行只选到行首，也为空。
        assert_eq!(selection.all_ranges(&rows), vec![(1, vec![(0, 2)])]);
    }

    #[test]
    fn a_shrunk_transcript_clamps_the_end_instead_of_panicking() {
        let rows = transcript(&["ab", "cd", "ef"]);
        let selection = select(
            &rows,
            (0, 0),
            (2, 2),
            PanelId::Transcript,
        );
        assert_eq!(selection.selected_text(&rows).as_deref(), Some("ab\ncd\nef"));
        // 窗口化之后可见行变少：终点行越界，收到最后一行的行尾。
        let fewer = transcript(&["ab", "cd"]);
        assert_eq!(selection.selected_text(&fewer).as_deref(), Some("ab\ncd"));
        assert_eq!(selection.ranges_in(1, &fewer), vec![(0, 2)]);
        assert_eq!(selection.ranges_in(9, &fewer), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn copied_lines_are_remembered_until_the_next_press() {
        let rows = transcript(&["a", "b"]);
        let mut selection = select(
            &rows,
            (0, 0),
            (1, 1),
            PanelId::Transcript,
        );
        selection.mark_copied(2);
        assert_eq!(selection.copied(), Some(2));
        selection.press_kind(
            PanelId::Transcript,
            &rows,
            0,
            0,
            ClickKind::Single,
        );
        assert_eq!(selection.copied(), None, "新的一次按下清掉提示");
        selection.mark_copied(1);
        selection.clear();
        assert_eq!(selection.copied(), None);
    }
}

#[cfg(test)]
mod wiring_tests {
    //! 接线测试：不建窗口、不碰系统剪贴板，直接往 App 里写 bevy 的指针消息。
    //!
    //! 面板用固定的 800×400 物理像素、没有内边距；行的位置由
    //! [`RowLayout`] 给出。命中点按真实 3D 面板的换算反推成世界坐标，所以
    //! 走的是和 `feathers_in_3d::pick` 同一套数学。

    use super::*;
    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::HitData;
    use bevy::picking::events::Pointer;
    use bevy::picking::pointer::{Location, PointerId};
    use rig_tui::ratatui::text::Line;

    use crate::panels::KeyedRow;

    fn text_row(text: &str) -> Row {
        Row::new(Line::from(text.to_owned()))
    }

    struct Fixture {
        app: App,
        panel: Entity,
        rows: Vec<Entity>,
        panel_data: WorldPanel,
    }

    fn fixture(lines: &[&str]) -> Fixture {
        let mut app = App::new();
        app.add_plugins(SelectionPlugin)
            .init_resource::<PanelRows>()
            .init_resource::<PanelRoots>()
            .init_resource::<MemoryClipboard>();
        let panel = app
            .world_mut()
            .spawn((
                WorldPanel {
                    units_to_world: crate::frame::UNITS_TO_WORLD,
                    anchor: Transform::default(),
                },
                // 800px 宽、没有边框/内边距：一行从 x = -400 开始。
                ComputedNode {
                    size: Vec2::new(800.0, 400.0),
                    ..default()
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<PanelRoots>()
            .0
            .insert(PanelId::Transcript, panel);
        let values: Vec<Row> = lines.iter().map(|line| text_row(line)).collect();
        {
            let mut panel_rows = app.world_mut().resource_mut::<PanelRows>();
            panel_rows.set(
                PanelId::Transcript,
                values
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, row)| KeyedRow {
                        key: index as u64,
                        hash: index as u64 + 1,
                        row,
                    })
                    .collect(),
            );
        }
        let rows = (0..values.len())
            .map(|index| {
                app.world_mut()
                    .spawn(RowLayout {
                        panel: PanelId::Transcript,
                        panel_entity: panel,
                        index,
                    })
                    .id()
            })
            .collect();
        let panel_data = *app.world().get::<WorldPanel>(panel).expect("面板");
        Fixture {
            app,
            panel,
            rows,
            panel_data,
        }
    }

    fn pointer() -> Pointer {
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

    /// 让命中点落在面板局部像素 `x`（行内第 `column` 格的正中）。
    fn hit_at(fixture: &Fixture, column: usize) -> HitData {
        let node = fixture
            .app
            .world()
            .get::<ComputedNode>(fixture.panel)
            .expect("面板的 ComputedNode");
        let x = content_left(node) + (column as f32 + 0.5) * crate::frame::CELL.x;
        let local = Vec3::new(x * fixture.panel_data.units_to_world, 0.0, 0.0);
        let world = fixture.panel_data.anchor.transform_point(local);
        HitData::new(Entity::PLACEHOLDER, 0.0, Some(world), Some(Vec3::Z))
    }

    fn press(fixture: &mut Fixture, row: usize, column: usize, count: u8) {
        let hit = hit_at(fixture, column);
        fixture.app.world_mut().write_message(PointerPress {
            entity: fixture.rows[row],
            pointer: pointer(),
            button: PointerButton::Primary,
            hit,
            count,
        });
    }

    fn drag(fixture: &mut Fixture, row: usize, column: usize) {
        let hit = hit_at(fixture, column);
        fixture.app.world_mut().write_message(PointerMove {
            entity: fixture.rows[row],
            pointer: pointer(),
            hit,
            delta: Vec2::ZERO,
        });
    }

    fn release(fixture: &mut Fixture, row: usize, column: usize) {
        let hit = hit_at(fixture, column);
        fixture.app.world_mut().write_message(PointerRelease {
            entity: fixture.rows[row],
            pointer: pointer(),
            button: PointerButton::Primary,
            hit,
        });
    }

    fn press_blank(fixture: &mut Fixture) {
        let blank = fixture.app.world_mut().spawn_empty().id();
        fixture.app.world_mut().write_message(PointerPress {
            entity: blank,
            pointer: pointer(),
            button: PointerButton::Primary,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            count: 1,
        });
    }

    fn selection(fixture: &Fixture) -> &Selection {
        fixture.app.world().resource::<Selection>()
    }

    fn copied(fixture: &Fixture) -> Vec<Copied> {
        fixture
            .app
            .world()
            .resource::<Messages<Copied>>()
            .iter_current_update_messages()
            .copied()
            .collect()
    }

    fn printed(fixture: &Fixture) -> Option<String> {
        fixture
            .app
            .world()
            .resource::<MemoryClipboard>()
            .written
            .last()
            .cloned()
    }

    // ---- 选中区间 -> 高亮 span ----

    #[test]
    fn selected_row_reverses_exactly_the_selected_bytes() {
        use rig_tui::ratatui::style::Color as RColor;
        use rig_tui::ratatui::text::Span;

        let row = Row::new(Line::from(vec![
            Span::raw("hello "),
            Span::styled("world", Style::default().fg(RColor::Green)),
        ]));
        let styled = selected_row(&row, &[(6, 11)]);
        let spans: Vec<(&str, bool)> = styled
            .line
            .spans
            .iter()
            .map(|span| {
                (
                    span.content.as_ref(),
                    span.style.add_modifier.contains(SELECTED_MODIFIER),
                )
            })
            .collect();
        assert_eq!(spans, vec![("hello ", false), ("world", true)]);
        // 原行不动：高亮是画出来的拷贝。
        assert!(!row.line.spans[1]
            .style
            .add_modifier
            .contains(SELECTED_MODIFIER));
    }

    #[test]
    fn selected_row_splits_a_span_at_the_selection_edges() {
        let row = Row::new(Line::from("你好世界")).with_right(Line::from("ok"));
        // “好世”的字节区间（3..9），横向切开了两个 span 之外的整行。
        let styled = selected_row(&row, &[(3, 9)]);
        let spans: Vec<(&str, bool)> = styled
            .line
            .spans
            .iter()
            .map(|span| {
                (
                    span.content.as_ref(),
                    span.style.add_modifier.contains(SELECTED_MODIFIER),
                )
            })
            .collect();
        assert_eq!(
            spans,
            vec![("你", false), ("好世", true), ("界", false)]
        );
        // 右对齐段在整行纯文本里有自己的字节偏移，不在选区里就保持原样。
        let right = styled.right.as_ref().expect("right");
        assert_eq!(right.spans.len(), 1);
        assert_eq!(right.spans[0].content.as_ref(), "ok");
        assert!(!right.spans[0]
            .style
            .add_modifier
            .contains(SELECTED_MODIFIER));
    }

    #[test]
    fn selection_ranges_keep_the_right_segment_in_place() {
        let row = Row::new(Line::from("left")).with_right(Line::from("right"));
        // 整行纯文本是 "leftright"；只选右段的 "ig"（偏移 5..7）。
        let styled = selected_row(&row, &[(5, 7)]);
        assert_eq!(styled.line.spans.len(), 1);
        assert_eq!(styled.line.spans[0].content.as_ref(), "left");
        let right = styled.right.as_ref().expect("right");
        let parts: Vec<(&str, bool)> = right
            .spans
            .iter()
            .map(|span| {
                (
                    span.content.as_ref(),
                    span.style.add_modifier.contains(SELECTED_MODIFIER),
                )
            })
            .collect();
        assert_eq!(parts, vec![("r", false), ("ig", true), ("ht", false)]);
    }

    #[test]
    fn paint_hash_changes_with_the_selected_range() {
        let upstream = 0xDEAD_BEEF_u64;
        assert_eq!(paint_hash(upstream, &[]), upstream, "没有选区不改变 hash");
        let first = paint_hash(upstream, &[(0, 1)]);
        let second = paint_hash(upstream, &[(0, 2)]);
        assert_ne!(first, upstream);
        assert_ne!(first, second, "同一行选中区间变长要触发重画");
        assert_eq!(paint_hash(upstream, &[(0, 1)]), first, "同样选区哈希稳定");
    }

    // ---- 3D 坐标 -> 行/格子 ----

    #[test]
    fn panel_local_pixels_round_trips_the_world_hit_point() {
        let panel = WorldPanel {
            units_to_world: crate::frame::UNITS_TO_WORLD,
            anchor: Transform::from_xyz(1.5, -2.0, 3.0),
        };
        let pixels = Vec2::new(-400.0, 12.0);
        let world = panel.anchor.transform_point(Vec3::new(
            pixels.x * panel.units_to_world,
            -pixels.y * panel.units_to_world,
            0.0,
        ));
        let back = panel_local_pixels(world, &panel);
        assert!((back.x - pixels.x).abs() < 1e-3);
        assert!((back.y - pixels.y).abs() < 1e-3);
    }

    #[test]
    fn cell_columns_count_fixed_width_cells_and_clamp_at_the_left_edge() {
        use crate::frame::CELL;
        let left = content_left(&ComputedNode {
            size: Vec2::new(800.0, 400.0),
            ..default()
        });
        assert_eq!(left, -400.0);
        assert_eq!(cell_column(left - 100.0, left), 0, "行左边之外算第 0 格");
        assert_eq!(cell_column(left, left), 0);
        assert_eq!(cell_column(left + CELL.x * 0.99, left), 0);
        // 边界上有 f32 舍入噪声，点比边界多 0.01px 才算下一格。
        assert_eq!(cell_column(left + CELL.x + 0.01, left), 1);
        assert_eq!(cell_column(left + CELL.x * 7.5, left), 7);
    }

    #[test]
    fn content_left_skips_the_panel_padding_and_border() {
        let node = ComputedNode {
            size: Vec2::new(800.0, 400.0),
            border: bevy::sprite::BorderRect::all(2.0),
            padding: bevy::sprite::BorderRect::all(8.0),
            ..default()
        };
        assert_eq!(content_left(&node), -400.0 + 2.0 + 8.0);
    }

    // ---- 按下 / 拖动 / 松开 ----

    #[test]
    fn pressing_a_row_starts_a_drag_at_the_clicked_cell() {
        let mut fixture = fixture(&["hello world"]);
        press(&mut fixture, 0, 1, 1);
        fixture.app.update();
        let selection = selection(&fixture);
        assert!(selection.is_dragging());
        assert_eq!(selection.panel(), Some(PanelId::Transcript));
        assert_eq!(selection.anchor(), Point::new(0, 1));
        assert_eq!(selection.focus(), Point::new(0, 1));
        assert!(!selection.is_active(), "只按下还没拖，选区是一个点");
    }

    #[test]
    fn dragging_across_two_rows_selects_both_lines_and_releasing_copies_them() {
        let mut fixture = fixture(&["hello", "world"]);
        press(&mut fixture, 0, 1, 1);
        fixture.app.update();
        drag(&mut fixture, 1, 2);
        fixture.app.update();
        assert_eq!(selection(&fixture).start(), Point::new(0, 1));
        assert_eq!(selection(&fixture).focus(), Point::new(1, 2));
        assert_eq!(
            selection(&fixture)
                .selected_text(&fixture.app.world().resource::<PanelRows>().row_values(
                    PanelId::Transcript
                ))
                .as_deref(),
            Some("ello\nwo")
        );

        release(&mut fixture, 1, 2);
        fixture.app.update();
        assert_eq!(printed(&fixture).as_deref(), Some("ello\nwo"));
        assert_eq!(copied(&fixture), vec![Copied { lines: 2 }]);
        assert_eq!(selection(&fixture).copied(), Some(2));
        assert!(
            selection(&fixture).is_active(),
            "松手后选区保留（上游行为），状态行显示 Copied"
        );
    }

    #[test]
    fn a_double_click_selects_the_word_and_releasing_copies_it() {
        let mut fixture = fixture(&["hello, world"]);
        press(&mut fixture, 0, 8, 2);
        fixture.app.update();
        let rows = fixture
            .app
            .world()
            .resource::<PanelRows>()
            .row_values(PanelId::Transcript);
        assert_eq!(
            selection(&fixture).selected_text(&rows).as_deref(),
            Some("world")
        );

        release(&mut fixture, 0, 8);
        fixture.app.update();
        assert_eq!(printed(&fixture).as_deref(), Some("world"));
        assert_eq!(copied(&fixture), vec![Copied { lines: 1 }]);
    }

    #[test]
    fn a_triple_click_selects_the_whole_line_and_copies_it_trimmed() {
        let mut fixture = fixture(&["  hi  ", "next"]);
        press(&mut fixture, 0, 3, 3);
        fixture.app.update();
        release(&mut fixture, 0, 3);
        fixture.app.update();
        assert_eq!(printed(&fixture).as_deref(), Some("  hi"));
        assert_eq!(copied(&fixture), vec![Copied { lines: 1 }]);
    }

    #[test]
    fn clicking_blank_space_clears_the_selection() {
        let mut fixture = fixture(&["hello"]);
        press(&mut fixture, 0, 0, 1);
        fixture.app.update();
        drag(&mut fixture, 0, 5);
        fixture.app.update();
        assert!(selection(&fixture).has_selection());

        press_blank(&mut fixture);
        fixture.app.update();
        assert_eq!(selection(&fixture).panel(), None);
        assert!(!selection(&fixture).has_selection());
        assert!(!selection(&fixture).is_dragging());
    }

    #[test]
    fn releasing_on_another_row_without_a_move_still_extends_the_selection() {
        let mut fixture = fixture(&["hello", "world"]);
        press(&mut fixture, 0, 0, 1);
        fixture.app.update();
        // 拖得很快、最后一帧的 PointerMove 没来：松开时的命中点也要收进 focus。
        release(&mut fixture, 1, 3);
        fixture.app.update();
        assert_eq!(printed(&fixture).as_deref(), Some("hello\nwor"));
        assert_eq!(copied(&fixture), vec![Copied { lines: 2 }]);
    }

    #[test]
    fn a_missing_clipboard_is_reported_in_the_status_line_not_panicked() {
        let mut fixture = fixture(&["hello"]);
        fixture.app.init_resource::<AppStatus>();
        fixture
            .app
            .world_mut()
            .remove_resource::<MemoryClipboard>();
        press(&mut fixture, 0, 0, 1);
        fixture.app.update();
        drag(&mut fixture, 0, 5);
        fixture.app.update();
        release(&mut fixture, 0, 5);
        fixture.app.update();

        let status = fixture.app.world().resource::<AppStatus>();
        let item = status
            .0
            .iter()
            .find(|item| item.name == clipboard::COPY_FAILED.name)
            .expect("复制失败写进状态行");
        assert_eq!(item.text, clipboard::NO_CLIPBOARD);
        assert!(copied(&fixture).is_empty(), "失败不产生 Copied");
        assert_eq!(selection(&fixture).copied(), None);
    }

    #[test]
    fn non_primary_buttons_do_not_move_the_selection() {
        let mut fixture = fixture(&["hello"]);
        let hit = hit_at(&fixture, 2);
        fixture.app.world_mut().write_message(PointerPress {
            entity: fixture.rows[0],
            pointer: pointer(),
            button: PointerButton::Secondary,
            hit,
            count: 1,
        });
        fixture.app.update();
        assert_eq!(selection(&fixture).panel(), None, "右键不碰选区");
    }

    // ---- resize 清选区（上游 input.rs 的 Event::Resize） ----

    /// 拖一段选区出来，返回还活着的 fixture。
    fn fixture_with_a_selection() -> Fixture {
        let mut fixture = fixture(&["hello world"]);
        press(&mut fixture, 0, 0, 1);
        fixture.app.update();
        drag(&mut fixture, 0, 5);
        fixture.app.update();
        assert!(selection(&fixture).has_selection());
        fixture
    }

    #[test]
    fn resizing_the_window_clears_the_selection() {
        let mut fixture = fixture_with_a_selection();
        fixture.app.world_mut().write_message(WindowResized {
            window: Entity::PLACEHOLDER,
            width: 1024.0,
            height: 640.0,
        });
        fixture.app.update();
        assert_eq!(
            selection(&fixture).panel(),
            None,
            "窗口 resize 清选区（上游 Event::Resize）"
        );
        assert!(!selection(&fixture).has_selection());
        assert!(!selection(&fixture).is_dragging());
    }

    #[test]
    fn a_panel_size_change_clears_the_selection() {
        let mut fixture = fixture_with_a_selection();
        // 先跑一帧让 resize 系统记下基准尺寸：第一次只是记，不清。
        fixture.app.update();
        assert!(
            selection(&fixture).has_selection(),
            "尺寸没变（或第一次看到）不清选区"
        );

        fixture
            .app
            .world_mut()
            .get_mut::<ComputedNode>(fixture.panel)
            .expect("面板的 ComputedNode")
            .size = Vec2::new(640.0, 320.0);
        fixture.app.update();
        assert_eq!(
            selection(&fixture).panel(),
            None,
            "面板尺寸变化也清选区"
        );
        assert!(!selection(&fixture).has_selection());
    }

    #[test]
    fn a_reflow_that_keeps_the_panel_size_keeps_the_selection() {
        let mut fixture = fixture_with_a_selection();
        // 尺寸没变的帧（浮层开/关这类重排只要不动这块面板）不该误清选区。
        fixture.app.update();
        fixture.app.update();
        assert!(selection(&fixture).has_selection());
        assert_eq!(selection(&fixture).end(), Point::new(0, 5));
    }
}
