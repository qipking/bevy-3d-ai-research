//! 输入编辑器：多行文本框 + 光标 + 历史，键位走 [`crate::keys`] 的动作表。
//!
//! 游标语义对齐 rig-tui 0.47（`editor.rs:64`、`:133`）：`cursor` 是 `text` 的
//! **字节偏移**，永远落在 grapheme cluster 边界上；左右移动、退格、删除、
//! 插入、Ctrl+W/Alt+D 都按 grapheme 走，中文、emoji（含 ZWJ 序列）与组合字符
//! 都当一个字符。
//!
//! 历史模型也对齐上游：Up 先在第一行内上移，到第一行行首后再进历史；
//! Down 往回走，翻过最新一条时把浏览前写的草稿带回来。`Ctrl+P`/`Ctrl+N`
//! 分别对应上游的 `line_up`/`line_down`（不从补全走）。被清掉、被取走的
//! 未发送文本也进 `history`（上游 `keep` 的行为）；Enter 发送的（以及 Tab 排队
//! 的）进 `history` 与 [`SubmittedPrompts`]。
//!
//! 发送、排队、停止与退出的通道（与上游 rig-tui `input.rs` 对齐）：
//!
//! - 非空输入 Enter -> [`rig_ecs::inbox::Deliver`]（[`DeliveryMode::Steer`]）给
//!   [`ShownAgent`] 指的那个 agent，也就是当前聚焦工作区的 agent；同时本地回显、
//!   记历史、转写窗格跟着末尾（[`PaneScroll::follow`]）；
//! - turn 跑着时 Tab -> `Deliver`（[`DeliveryMode::Queue`]）；补全列表开着时
//!   Tab 仍然是接受候选；
//! - turn 跑着时 Esc -> [`rig_ecs::agent::StopTurn`]；补全列表开着时 Esc 仍然是
//!   关列表；
//! - 空输入 Ctrl+D -> [`bevy::app::AppExit`]（`built_in` 里 `ctrl('d')` 本来就是
//!   [`InputAction::Exit`]，不用加绑定）。
//!
//! `/` 开头的行仍然由应用层的 `run_typed_prompts` 交给 `RunCommand`，这里**不再**
//! 额外送一条普通消息（上游靠内核的 `Incoming::command` 分流；本 crate 没有
//! refused 概念，所以也不做 `Deliver::literal` 重发）。
//!
//! 显示：`editor_rows` 把文本按输入面板列宽折行（和转写同一个
//! `rig_tui::wrap::wrap`），每行一个 [`KeyedRow`]；光标画在真实行列上
//! （反色 + 下划线的 `▏`），超过 [`MOST_LINES`] 行时按光标滚动。
//! [`sync_editor_tile`] 让输入 tile 的高度跟着行数走（最少 [`LEAST_ROWS`]
//! 行、最多 [`MOST_LINES`] 行 + 面板上下 chrome），右列的总高仍由 Spacer
//! 这个比例子吸收，精确不变。
//!
//! 未接的行为：粘贴图片（Ctrl+V 现在只粘文本；图片见 [`crate::clipboard`]
//! 模块文档）、转写区滚动/选择（由 `pane.rs` 现有系统负责）。

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::{ButtonInput, ButtonState};
use bevy::prelude::*;
use rig_ecs::prelude::{ActiveTurn, Deliver, DeliveryMode, StopTurn};
use rig_harness::main_agent::ShownAgent;
use rig_tui::AppStatus;
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::clipboard::{self, MemoryClipboard};
use crate::frame::{FrameTree, PanelAreas, PanelRoots};
use crate::keys::{
    self, BindingRef, Found, InputAction, KeyBinding, KeyFocus, KeyPressed, KeyScope, Modifiers,
    PendingInput,
};
use crate::pane::PaneScroll;
use crate::panels::{KeyedRow, PanelId, PanelRows};
use crate::theme::Theme;
use crate::transcript::{ROW_HEIGHT, Row, row_hash};

/// 编辑器状态。
///
/// 四个公开字段都还在；`cursor` 的语义从“字符下标”改成“字节偏移
/// （grapheme 边界）”，`history` 从“只装已发送”变成上游那样的整体历史
/// （已发送 + 被清掉的草稿）。`draft` 是浏览历史前的草稿，不公开——本
/// crate 内没有外部代码用结构体字面量构造 `Editor`，但外部 crate 若这样做
/// 会少一个字段，见移植回报。
#[derive(Resource, Default)]
pub struct Editor {
    /// 已经打出来的文本。
    pub text: String,
    /// 光标：`text` 的字节偏移，落在 grapheme 边界上。
    pub cursor: usize,
    /// 历史：Enter 发送的，加上被清掉/取走的未发送文本（上游 `keep`）。
    pub history: Vec<String>,
    /// 正在浏览历史的第几条（`None` 表示在写新内容）。
    pub history_pos: Option<usize>,
    /// 开始浏览历史前正在写的草稿；Down 翻过最新一条时带回来。
    draft: String,
}

/// 输入框最多显示几行；再多就滚动（上游 `rig-tui editor.rs:28` 的
/// `MOST_LINES`）。
pub const MOST_LINES: usize = 10;

/// 输入框最低的**视觉**高度：3 行。
///
/// 空输入按公式只有 `1 行 + chrome` = 38px，比现在固定的 72px 矮近一半：
/// 点击目标太小，而且输入框下面紧贴着 hints / 状态行，输入框一缩这些 tile
/// 会跟着往下跳。3 行（54px）留出 1.9 行左右的文字可见区，点击目标接近
/// 原来 4 行的观感，又不会比现在更占高度。
pub const LEAST_ROWS: f32 = 3.0;

/// 输入面板上下装饰（pad + border）占的像素。
///
/// 从 [`crate::frame::PanelInset::DEFAULT`] 算出来，**不要**写死 20：
/// 输入面板由 `frame::default_screen_panels` 以 `FramePanel::tree()` spawn，
/// 用的就是 `PanelInset::DEFAULT`；改 inset 时这里跟着变。
pub const INPUT_CHROME: f32 = 2.0
    * (crate::frame::PanelInset::DEFAULT.pad as f32
        + crate::frame::PanelInset::DEFAULT.border as f32);

/// 输入 tile 的高度（UI 像素）。
///
/// `max(LEAST_ROWS * ROW_HEIGHT, clamp(行数, 1, MOST_LINES) * ROW_HEIGHT + INPUT_CHROME)`。
/// 超过 [`MOST_LINES`] 后 tile 不再长高，
/// 多出来的行由 [`EditorLayout`] 滚动露出（上游 `fit_rows` 的
/// `clamp(1, MOST_LINES) + 2`，我们的 `+2` 是面板 chrome 而不是两行边框）。
pub fn input_tile_height(lines: usize) -> f32 {
    let wanted = lines.clamp(1, MOST_LINES) as f32 * ROW_HEIGHT + INPUT_CHROME;
    wanted.max(LEAST_ROWS * ROW_HEIGHT)
}

/// 输入框这一帧的折行结果 + 光标/滚动位置（上游 `editor::Layout`）。
///
/// `rows` 是**不带**行首提示符的正文行；第一行前面的 `› ` 和续行的两个
/// 空格由 [`Self::visible_rows`] 加，所以光标列也不含提示符（调用方加 2）。
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct EditorLayout {
    /// 按面板列宽折好的行（空输入也有一行空行）。
    pub rows: Vec<Line<'static>>,
    /// 光标在第几行（0 起，坐标相对 `rows`）。
    pub cursor_row: usize,
    /// 光标在行内第几列（unicode 显示宽，0 起，不含提示符）。
    pub cursor_column: usize,
    /// 可见段从 `rows` 的第几行开始（滚动）。
    pub top: usize,
    /// 可见几行（`clamp(rows.len(), 1, MOST_LINES)`）。
    pub visible: usize,
}

impl EditorLayout {
    /// 光标在可见段里的下标；光标不在可见段时 `None`。
    pub fn cursor_in_view(&self) -> Option<usize> {
        (self.cursor_row >= self.top && self.cursor_row < self.top.saturating_add(self.visible))
            .then(|| self.cursor_row - self.top)
    }

    /// 这一帧要显示的行：`top..top+visible`，第一行带 `› `、续行带两个空格，
    /// 光标所在行在真实列上插入 [`CURSOR`]。
    pub fn visible_rows(&self, theme: &Theme) -> Vec<Line<'static>> {
        if self.rows.is_empty() {
            // 布局还没跑过（例如系统被跳过）时也至少画出提示符和光标。
            return vec![Line::from(vec![prompt_span(true, theme), cursor_span()])];
        }
        let start = self.top.min(self.rows.len().saturating_sub(1));
        let end = start
            .saturating_add(self.visible.max(1))
            .min(self.rows.len());
        (start..end)
            .map(|index| {
                let mut row = self.rows[index].clone();
                if index == self.cursor_row {
                    row = with_cursor(row, self.cursor_column);
                }
                let mut spans = vec![prompt_span(index == 0, theme)];
                spans.extend(row.spans);
                Line {
                    spans,
                    style: row.style,
                    alignment: row.alignment,
                }
            })
            .collect()
    }
}

/// 光标画成的字符（沿用旧实现的 `▏`，但位置是真的）。
pub const CURSOR: &str = "▏";

/// 光标的样式：反色 + 下划线，前景仍是旧的浅青色。
pub fn cursor_style() -> Style {
    Style::default()
        .fg(RColor::LightCyan)
        .add_modifier(Modifier::REVERSED | Modifier::UNDERLINED)
}

fn cursor_span() -> Span<'static> {
    Span::styled(CURSOR, cursor_style())
}

/// 行首提示符：第一行 `› `（绿色粗体），续行两个空格（dim，和上游一致）。
fn prompt_span(first: bool, theme: &Theme) -> Span<'static> {
    match first {
        true => ratatui_span("› ", theme.prompt),
        false => ratatui_span("  ", theme.dim),
    }
}

/// 在行内第 `column` 个显示列处插入光标。
///
/// 光标是额外的一列而不是盖在字上：中文字/emoji 的宽度由
/// [`UnicodeWidthStr`] 算，和折行用的是同一套列宽。行比 `column` 短时先补
/// 空格——光标可能停在折行时被丢掉的空白里（上游终端光标也停在那里）。
fn with_cursor(line: Line<'static>, column: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut seen = 0usize;
    let mut placed = false;
    for span in line.spans {
        let mut text = String::new();
        for grapheme in span.content.graphemes(true) {
            let width = UnicodeWidthStr::width(grapheme);
            if !placed && seen.saturating_add(width) > column {
                if !text.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut text), span.style));
                }
                spans.push(cursor_span());
                placed = true;
            }
            text.push_str(grapheme);
            seen = seen.saturating_add(width);
        }
        if !text.is_empty() {
            spans.push(Span::styled(text, span.style));
        }
    }
    if !placed {
        if seen < column {
            spans.push(Span::styled(" ".repeat(column - seen), Style::default()));
        }
        spans.push(cursor_span());
    }
    Line {
        spans,
        style: line.style,
        alignment: line.alignment,
    }
}

impl Editor {
    /// 文本。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 光标：`text` 的字节偏移，落在 grapheme 边界上。
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// 是否什么都没打。
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// 替换文本，光标放到末尾，`\r\n`/`\r` 当换行。被替换的文本不保留
    /// （插件编辑文本时用；要丢弃用户文本请先调 [`Self::clear`]）。换一个
    /// 文本会停止浏览历史，浏览前的草稿进历史。
    pub fn set(&mut self, text: impl Into<String>) {
        let text = newlines(text.into());
        if text != self.text {
            self.stop_browsing();
        }
        self.show(text);
    }

    /// 把光标放到字节 `offset`，或它前面最近的 grapheme 边界。
    pub fn set_cursor(&mut self, offset: usize) {
        let starts = self.text.grapheme_indices(true).map(|(at, _)| at);
        self.cursor = starts.take_while(|at| *at <= offset).last().unwrap_or(0);
        if offset >= self.text.len() {
            self.cursor = self.text.len();
        }
    }

    /// 光标停在某个 grapheme 中间时（比如在组合字符前插入），挪到它末尾，
    /// 与上游 `settle` 一致。
    fn settle(&mut self) {
        let ends = self
            .text
            .grapheme_indices(true)
            .map(|(at, grapheme)| at + grapheme.len());
        let mut ends = std::iter::once(0).chain(ends);
        self.cursor = ends
            .find(|end| *end >= self.cursor)
            .unwrap_or(self.text.len());
    }

    /// 显示 `text`，光标放到末尾。
    fn show(&mut self, text: String) {
        self.text = text;
        self.cursor = self.text.len();
    }

    /// 清空编辑器并停止浏览历史。用户自己的文本（以及正在浏览的历史
    /// 提示词后面藏着的草稿）进历史，Up 能找回来。
    pub fn clear(&mut self) {
        let recalled = self.recalled();
        self.stop_browsing();
        let text = std::mem::take(&mut self.text);
        if !recalled {
            self.keep(text);
        }
        self.cursor = 0;
    }

    /// 把文本取走，留下空编辑器。浏览历史时，后面的草稿进历史。
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        self.stop_browsing();
        std::mem::take(&mut self.text)
    }

    /// 是否正原样显示一条历史提示词。
    pub fn recalled(&self) -> bool {
        self.history_pos
            .and_then(|at| self.history.get(at))
            .is_some_and(|entry| *entry == self.text)
    }

    /// 记录一段调用方已发送的文本，排在此前丢弃的文本之后。
    pub fn sent(&mut self, text: String) {
        self.stop_browsing();
        self.text.clear();
        self.cursor = 0;
        self.draft.clear();
        self.keep(text);
    }

    /// 停止浏览历史：正在显示的历史提示词从此算用户自己的文本，浏览前的
    /// 草稿进历史（上游 `stop_browsing`）。
    pub fn stop_browsing(&mut self) {
        if self.history_pos.take().is_some() {
            let draft = std::mem::take(&mut self.draft);
            self.keep(draft);
        }
    }

    /// 把 `text` 收进历史，除非它是空白。
    fn keep(&mut self, text: String) {
        if !text.trim().is_empty() {
            self.history.push(text);
        }
    }

    /// 在光标处打 `text`，光标落在它后面；`\r\n`/`\r` 归一成 `\n`。
    pub fn insert(&mut self, text: &str) {
        let text = newlines(text.to_owned());
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.settle();
    }

    /// 在光标处打一个字符。
    pub fn insert_char(&mut self, character: char) {
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
        self.settle();
    }

    /// 光标前一个 grapheme 的起点。
    fn previous_boundary(&self, from: usize) -> usize {
        self.text
            .get(..from)
            .and_then(|before| before.grapheme_indices(true).next_back())
            .map_or(0, |(index, _)| index)
    }

    /// 光标处 grapheme 的终点。
    fn next_boundary(&self, from: usize) -> usize {
        self.text
            .get(from..)
            .and_then(|after| after.graphemes(true).next())
            .map_or(self.text.len(), |grapheme| from + grapheme.len())
    }

    /// 后退一个 grapheme。
    pub fn left(&mut self) {
        self.cursor = self.previous_boundary(self.cursor);
    }

    /// 前进一个 grapheme。
    pub fn right(&mut self) {
        self.cursor = self.next_boundary(self.cursor);
    }

    /// 光标前一个词的开头。
    fn word_start(&self) -> usize {
        let before = self.text.get(..self.cursor).unwrap_or_default();
        let mut start = self.cursor;
        let mut in_word = false;
        for (at, grapheme) in before.grapheme_indices(true).rev() {
            match (word_grapheme(grapheme), in_word) {
                (true, _) => in_word = true,
                (false, true) => break,
                (false, false) => {}
            }
            start = at;
        }
        start
    }

    /// 光标后一个词的结尾。
    fn word_end(&self) -> usize {
        let after = self.text.get(self.cursor..).unwrap_or_default();
        let mut end = self.cursor;
        let mut in_word = false;
        for (at, grapheme) in after.grapheme_indices(true) {
            match (word_grapheme(grapheme), in_word) {
                (true, _) => in_word = true,
                (false, true) => break,
                (false, false) => {}
            }
            end = self.cursor + at + grapheme.len();
        }
        end
    }

    /// 光标前一个词的开头。
    pub fn word_left(&mut self) {
        self.cursor = self.word_start();
    }

    /// 光标后一个词的结尾。
    pub fn word_right(&mut self) {
        self.cursor = self.word_end();
    }

    /// 光标所在行的行首。
    fn line_start(&self) -> usize {
        self.text
            .get(..self.cursor)
            .and_then(|before| before.rfind('\n'))
            .map_or(0, |index| index + 1)
    }

    /// 光标所在行的行尾（不含换行）。
    fn line_end(&self) -> usize {
        self.text
            .get(self.cursor..)
            .and_then(|after| after.find('\n'))
            .map_or(self.text.len(), |index| self.cursor + index)
    }

    /// 行首。
    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    /// 行尾。
    pub fn end(&mut self) {
        self.cursor = self.line_end();
    }

    /// 删光标前一个 grapheme。
    pub fn backspace(&mut self) {
        self.remove(self.previous_boundary(self.cursor)..self.cursor);
    }

    /// 删光标后一个 grapheme。
    pub fn delete(&mut self) {
        self.remove(self.cursor..self.next_boundary(self.cursor));
    }

    /// 删光标前一个词。
    pub fn delete_word_before(&mut self) {
        self.remove(self.word_start()..self.cursor);
    }

    /// 删光标后一个词。
    pub fn delete_word_after(&mut self) {
        self.remove(self.cursor..self.word_end());
    }

    /// 删到行首；已经在行首时删掉前面的换行。
    pub fn delete_to_line_start(&mut self) {
        let start = match self.line_start() {
            start if start == self.cursor => self.previous_boundary(start),
            start => start,
        };
        self.remove(start..self.cursor);
    }

    /// 删到行尾；已经在行尾时删掉后面的换行。
    pub fn delete_to_line_end(&mut self) {
        let end = match self.line_end() {
            end if end == self.cursor => self.next_boundary(end),
            end => end,
        };
        self.remove(self.cursor..end);
    }

    /// 删掉 `range` 这些字节，光标放在删处；两侧会拼成一个 grapheme 时，
    /// 光标落在拼出来的 grapheme 后面。
    fn remove(&mut self, range: std::ops::Range<usize>) {
        self.cursor = range.start;
        self.text.replace_range(range, "");
        self.settle();
    }

    /// 光标在它那一行的第几个 grapheme。
    fn column(&self) -> usize {
        self.text
            .get(self.line_start()..self.cursor)
            .map_or(0, |line| line.graphemes(true).count())
    }

    /// 移到以 `start` 开头那一行的第 `column` 个 grapheme，没有就移到行尾。
    fn move_to_column(&mut self, start: usize, column: usize) {
        let line = self.text.get(start..).unwrap_or_default();
        let line = line.split('\n').next().unwrap_or_default();
        self.cursor = start
            + line
                .grapheme_indices(true)
                .nth(column)
                .map_or(line.len(), |(index, _)| index);
    }

    /// 按输入面板的 `width` 个文字列折行，并算出光标落在第几行第几列。
    ///
    /// 折行用 [`rig_tui::wrap::wrap`]——和转写 `transcript::wrap_rows` 是同一个
    /// 折行器（按 unicode 列宽、词边界、列表续行缩进）。第一行前面的 `› `
    /// 也占两列，所以正文宽度是 `width - 2`，与上游 `layout(width)` 一致。
    /// 控制字符按上游换成同样字节数的空格（tab 因此显示成一个空格）；换行
    /// 按 `\n` 拆成逻辑行。光标是 grapheme 边界上的字节偏移，折行时被丢掉
    /// 的换行处空格（[`rig_tui::wrap::RowWrap::gap`]）也按上游的规则映射。
    ///
    /// 返回的 [`EditorLayout`] 不含提示符：`cursor_column` 是行内显示列，
    /// 提示符两列由 [`EditorLayout::visible_rows`] 加。
    pub fn layout(&self, width: usize) -> EditorLayout {
        // `width` 是面板文字列数，去掉提示符的两列；至少留 1 列。
        let width = width.max(3) - 2;
        let mut rows: Vec<Line<'static>> = Vec::new();
        let mut cursor = (0usize, 0usize);
        let mut start = 0;
        for line in self.text.split('\n') {
            let shown: String = line
                .chars()
                .map(|character| match character.is_control() {
                    // 保持字节数不变，光标偏移仍然能对回原文。
                    true => " ".repeat(character.len_utf8()),
                    false => character.to_string(),
                })
                .collect();
            let at = self
                .cursor
                .checked_sub(start)
                .filter(|at| *at <= line.len());
            let mut end = 0;
            for (row, joint) in rig_tui::wrap::wrap(&Line::from(shown.as_str()), width) {
                let (gap, indent) = joint
                    .as_ref()
                    .map_or(("", 0), |joint| (joint.gap.as_str(), joint.indent));
                let from = end + gap.len();
                // 光标落在折行丢掉的那段空格里：列不够就放到上一行末尾，
                // 否则从下一行的缩进处开始（上游同一条规则）。
                if let Some(at) = at
                    && (end..from).contains(&at)
                {
                    let dropped = gap.get(..at - end).map_or(0, UnicodeWidthStr::width);
                    let column = rows.last().map_or(0, Line::width) + dropped;
                    cursor = match column < width {
                        true => (rows.len().saturating_sub(1), column),
                        false => (rows.len(), indent),
                    };
                }
                let bytes: usize = row.spans.iter().map(|span| span.content.len()).sum();
                end = from + bytes.saturating_sub(indent);
                if let Some(at) = at
                    && (from..=end).contains(&at)
                {
                    let before = shown.get(from..at).map_or(0, UnicodeWidthStr::width);
                    cursor = (rows.len(), indent + before);
                }
                rows.push(row);
            }
            // 光标正好在一行写满之后：上游开一个空行放光标，避免它落在
            // 下一行开头却被画到上一行末尾。
            if at == Some(line.len()) && cursor.1 >= width {
                cursor = (rows.len(), 0);
                rows.push(Line::default());
            }
            start += line.len() + 1;
        }
        let visible = rows.len().clamp(1, MOST_LINES);
        let top = (cursor.0 + 1).saturating_sub(visible);
        EditorLayout {
            rows,
            cursor_row: cursor.0,
            cursor_column: cursor.1,
            top,
            visible,
        }
    }

    /// Up/Down 是否正在历史里走：原样显示一条历史提示词，光标在它的
    /// 开头或末尾。
    fn walking(&self) -> bool {
        self.recalled() && (self.cursor == 0 || self.cursor == self.text.len())
    }

    /// Up：在历史里走时取更早的一条，否则上一行，否则第一行行首，否则
    /// 历史最新一条（把当前文本留作草稿）。
    pub fn up(&mut self) {
        let history = self.history.clone();
        self.up_from(&history, true);
    }

    /// Ctrl+P：与 [`Self::up`] 一样，但从第一行的任何位置直接进历史。
    pub fn line_up(&mut self) {
        let history = self.history.clone();
        self.up_from(&history, false);
    }

    fn up_from(&mut self, history: &[String], to_start: bool) {
        let start = self.line_start();
        if self.walking() || (start == 0 && (self.cursor == 0 || !to_start)) {
            self.history_back(history);
            return;
        }
        if start == 0 {
            self.cursor = 0;
            return;
        }
        let column = self.column();
        let above = self
            .text
            .get(..start - 1)
            .and_then(|before| before.rfind('\n'))
            .map_or(0, |index| index + 1);
        self.move_to_column(above, column);
    }

    /// Down：在历史里走时取更晚的一条（翻过最新一条回到草稿），否则下一
    /// 行，否则最后一行行尾。
    pub fn down(&mut self) {
        let history = self.history.clone();
        self.down_with(&history);
    }

    /// Ctrl+N：与 [`Self::down`] 一样（本 crate 没有补全，没有别的分支）。
    pub fn line_down(&mut self) {
        self.down();
    }

    fn down_with(&mut self, history: &[String]) {
        if self.walking() {
            self.history_forward(history);
            return;
        }
        let end = self.line_end();
        if end == self.text.len() {
            self.cursor = end;
            return;
        }
        let column = self.column();
        self.move_to_column(end + 1, column);
    }

    fn history_back(&mut self, history: &[String]) {
        if !self.recalled() {
            self.stop_browsing();
        }
        let index = match self.history_pos {
            Some(shown) => match place(shown, &self.text, history) {
                0 => return,
                index => index - 1,
            },
            None if history.is_empty() => return,
            None => {
                self.draft = self.text.clone();
                history.len() - 1
            }
        };
        self.browse(history, index);
    }

    fn history_forward(&mut self, history: &[String]) {
        let Some(shown) = self.history_pos else {
            return;
        };
        let index = place(shown, &self.text, history) + 1;
        if index < history.len() {
            self.browse(history, index);
        } else {
            self.history_pos = None;
            let draft = std::mem::take(&mut self.draft);
            self.show(draft);
        }
    }

    /// 显示历史第 `index` 条。
    fn browse(&mut self, history: &[String], index: usize) {
        let text = history.get(index).cloned().unwrap_or_default();
        self.history_pos = Some(index);
        self.show(text);
    }
}

/// 已提交的提示词：输入框的本地回显（面板行之外的一份记录，测试/插件可读）。
///
/// 真正送达 agent 走 [`Deliver`]；这份回显没有别的用途，也不再是"假装发送"。
#[derive(Resource, Default)]
pub struct SubmittedPrompts(pub Vec<String>);

/// 提交事件（其他插件可以监听）：Enter 与 Tab 排队的文本都从这里过一遍。
///
/// 斜杠命令由应用层从这条消息接到 `RunCommand`；普通消息另外触发 [`Deliver`]。
#[derive(Message, Debug, Clone)]
pub struct PromptSubmitted(pub String);

/// 一次发送：Trim 过的文本 + 送达模式。
///
/// 模式与上游一致：Enter 是 [`DeliveryMode::Steer`]（空闲时开一轮、忙时插进
/// 这一轮的下一趟模型调用），Tab 是 [`DeliveryMode::Queue`]（等这一轮结束再作为
/// 下一步一起送）。
#[derive(Debug)]
struct Sending {
    text: String,
    mode: DeliveryMode,
}

/// [`act`] 做完编辑器该做的事之后，调用方还要执行的副作用。
///
/// 动作本身（清空、移动、删除……）在 `act` 里就地做完；发送/停止/退出/粘贴
/// 需要 agent、`Commands` 或剪贴板，由 [`keys`] 统一接线。
#[derive(Debug)]
enum Act {
    /// 把文本送给 agent。
    Send(Sending),
    /// 停掉 agent 正在跑的这一轮。
    Stop,
    /// 输入为空时按了 Ctrl+D：退出程序。
    Exit,
    /// Ctrl+V：读剪贴板文本，插到光标处（图片分支见 [`act`] 的注释）。
    Paste,
}

/// 执行一个内置动作；需要 agent 或 `Commands` 的留给调用方。
///
/// `busy` 是 [`ShownAgent`] 上有没有 [`ActiveTurn`]（和状态行的 `turn` 条目
/// 同一个事实来源）：它只影响 Tab（排队还是停止浏览历史）与 Esc（停 agent
/// 还是什么也不做）。
fn act(action: InputAction, editor: &mut Editor, busy: bool) -> Option<Act> {
    use InputAction as Do;
    match action {
        Do::Clear => editor.clear(),
        // 输入为空时 Ctrl+D 退出（上游 `Do::Exit if input.is_empty()`）；有字时
        // 不退也不清空，和上游一样当作按错。
        Do::Exit if editor.is_empty() => return Some(Act::Exit),
        Do::Exit => {}
        Do::NewLine => editor.insert_char('\n'),
        Do::LineStart => editor.home(),
        Do::LineEnd => editor.end(),
        Do::Left => editor.left(),
        Do::Right => editor.right(),
        Do::WordLeft => editor.word_left(),
        Do::WordRight => editor.word_right(),
        Do::Backspace => editor.backspace(),
        Do::Delete => editor.delete(),
        Do::DeleteWordBefore => editor.delete_word_before(),
        Do::DeleteWordAfter => editor.delete_word_after(),
        Do::DeleteToLineStart => editor.delete_to_line_start(),
        Do::DeleteToLineEnd => editor.delete_to_line_end(),
        Do::LineUp => editor.line_up(),
        Do::LineDown => editor.line_down(),
        Do::Up => editor.up(),
        Do::Down => editor.down(),
        // 滚动/选择/内联回答由别的模块负责；这里只有动作表。
        Do::ScrollUp
        | Do::ScrollDown
        | Do::PageUp
        | Do::PageDown
        | Do::SelectUp
        | Do::SelectDown
        | Do::Deselect
        | Do::AnswerInline(_)
        | Do::DismissInline => {}
        // Ctrl+V 的绑定是上游同名动作 `PasteImage`：上游同时读文本和图片，
        // 本 crate **只接文本**。图片分支没做的原因不是漏了，而是
        // `bevy_clipboard` 没有开 `image` feature、也没有"剪贴板图片落盘到
        // 会话目录"的约定（见 `clipboard.rs` 模块文档）；剪贴板里只有图片
        // 时读文本会失败，由 [`Act::Paste`] 把可显示的错误交给状态行。
        Do::PasteImage => return Some(Act::Paste),
        // 一轮正在跑：Tab 是排队（上游 `send(.., queue = true)`）。补全列表开着
        // 时走不到这里——[`completion_act`] 先把它吃掉了。空输入不排队，也不
        // 停止浏览历史（上游 `send` 先返回，`stop_browsing` 不会被执行）。
        Do::Tab if busy => return send(editor, DeliveryMode::Queue),
        // 没在跑：Tab 把正在浏览的历史提示词变成用户自己的文本。
        Do::Tab => editor.stop_browsing(),
        Do::Send => return send(editor, DeliveryMode::Steer),
        // 补全列表开着时 Esc 先在 [`completion_act`] 里关列表；到这里就是停
        // agent。没有正在跑的 turn 时什么也不做（上游也会触发 `StopTurn` 去停
        // 插件的工作，本 crate 没有这类工作，就不空触发）。
        Do::Stop if busy => return Some(Act::Stop),
        Do::Stop => {}
    }
    None
}

/// 取走输入，trim 后记进编辑器历史（[`Editor::sent`]），返回要送出的文本。
///
/// 空输入（只有空白也算）不发送，也不清空——与上游 `send` 开头的
/// `input.text().trim().is_empty()` 早退一致。
fn send(editor: &mut Editor, mode: DeliveryMode) -> Option<Act> {
    let text = editor.text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    editor.sent(text.clone());
    Some(Act::Send(Sending { text, mode }))
}

/// 这一行是不是应用层要按 `/命令` 跑的（与 `run_typed_prompts` 同一条入口
/// 规则：trim 过、`/` 开头）。命令走 `RunCommand`，视图不再额外送普通消息。
///
/// 这里只判断"是不是命令形状"，命令名在不在由应用层的命令表决定：本 crate
/// 没有上游的 `refused` 概念，所以没有 `Deliver::new(..).literal()` 的重发。
fn is_command(text: &str) -> bool {
    text.trim().starts_with('/')
}

/// 补全列表开着时先吃掉这个动作；返回 `true` 表示已处理。
///
/// 对应上游 `input.rs::accept_completion` 与列表导航：↑↓ 改选中项、Tab/Enter 接受、
/// Esc 关掉（并记住这个 token，直到它变了才允许重开）。
///
/// 列表关着时这里一律返回 `false`，把动作交回 [`act`]：那是上游
/// `view.completion.is_none()` 的分支——↑↓ 走编辑器历史/行移动，Tab 排队或
/// 停止浏览历史，Enter 发送，Esc 停 agent。以前的实现在列表关着时也把
/// ↑↓/Tab/Esc 吃掉，发送与停止因此永远到不了。
fn completion_act(
    action: InputAction,
    state: &mut crate::complete::CompletionState,
    editor: &mut Editor,
) -> bool {
    use InputAction as Do;
    if !state.is_open() {
        return false;
    }
    match action {
        Do::Up => {
            if let Some(completion) = state.completion_mut() {
                completion.select(-1);
            }
            true
        }
        Do::Down => {
            if let Some(completion) = state.completion_mut() {
                completion.select(1);
            }
            true
        }
        Do::Tab => {
            if let Some(completion) = state.take() {
                completion.accept(editor);
            }
            true
        }
        // 精确命中时 Enter 直接发送、列表关掉；否则接受候选（上游同义）。
        Do::Send => {
            let exact = state.completion().is_some_and(|c| c.is_exact());
            if exact {
                state.close();
                return false;
            }
            if let Some(completion) = state.take() {
                completion.accept(editor);
            }
            true
        }
        Do::Stop => {
            state.dismiss();
            true
        }
        _ => false,
    }
}

/// 一次按键里要用的剪贴板/状态资源，三个都可能是 `None`。
///
/// 和 `selection::pointer_selection` 同一套优先级：有内存替身（单测/没有
/// 系统剪贴板的后备）就用它，否则用 `bevy_clipboard::Clipboard`；都没有时
/// 粘贴报 [`clipboard::NO_CLIPBOARD`]。状态行可缺省，缺省时错误只返回给
/// 调用方（无窗口测试 App），不会 panic。
#[derive(SystemParam)]
pub struct Clipboards<'w> {
    system: Option<ResMut<'w, bevy_clipboard::Clipboard>>,
    memory: Option<ResMut<'w, MemoryClipboard>>,
    status: Option<ResMut<'w, AppStatus>>,
}

/// 输入系统的键处理：走 [`crate::keys`] 的绑定表；编辑动作在这里执行，
/// 插件绑定（没有 [`InputAction`] 的）触发 [`KeyPressed`]。
///
/// 发送/排队/停止/退出/粘贴在这里落到内核与 bevy 上：
/// * [`Deliver`] 给 [`ShownAgent`]（应用层在切工作区时更新它，见
///   `rig_harness_in_3d::workspaces_demo`），斜杠命令让给应用层的
///   `RunCommand` 桥，不重复送；
/// * [`StopTurn`] 给同一个 agent；
/// * [`AppExit`] 是空输入 Ctrl+D；
/// * Ctrl+V 读剪贴板文本，插到光标处；空剪贴板/读失败写状态行的
///   [`clipboard::PASTE_FAILED`]（和 `Copied` 同一套状态反馈）。
#[allow(clippy::too_many_arguments)]
pub fn keys(
    mut input: MessageReader<KeyboardInput>,
    mut editor: ResMut<Editor>,
    mut prompts: ResMut<SubmittedPrompts>,
    mut submitted: MessageWriter<PromptSubmitted>,
    held: Res<ButtonInput<KeyCode>>,
    bindings: Query<(Entity, &KeyBinding, Option<&InputAction>, Has<Disabled>), Allow<Disabled>>,
    mut focus: ResMut<KeyFocus>,
    mut pending: ResMut<PendingInput>,
    mut commands: Commands,
    mut completion: Option<ResMut<crate::complete::CompletionState>>,
    shown: Option<Res<ShownAgent>>,
    entering: Query<(), With<ActiveTurn>>,
    mut scroll: Option<ResMut<PaneScroll>>,
    mut clipboards: Clipboards,
) {
    let rows: Vec<BindingRef> = bindings
        .iter()
        .filter(|(.., disabled)| !*disabled)
        .map(|(entity, binding, action, _)| BindingRef {
            entity,
            binding,
            action: action.copied(),
        })
        .collect();
    let mut modifiers = Modifiers::from_held(&held);
    for event in input.read() {
        // 修饰键自己更新状态；同一帧里先按修饰键的事件也因此正确。
        if Modifiers::is_modifier(event.key_code) {
            match event.state {
                ButtonState::Pressed => {
                    modifiers.insert(modifier_of(event.key_code));
                }
                ButtonState::Released => {
                    modifiers.remove(modifier_of(event.key_code));
                }
            }
            continue;
        }
        let scope = match focus.0 {
            Some(entity) => KeyScope::Panel(entity),
            None => KeyScope::Input,
        };
        let press = keys::press_with(&event, modifiers);
        if event.state != ButtonState::Pressed {
            if let Some(entity) = keys::released(&rows, &press, scope) {
                commands.trigger(KeyPressed {
                    entity,
                    agent: None,
                    released: true,
                });
            }
            continue;
        }
        pending.0.push(press.clone());
        match keys::find(&rows, &pending.0, scope) {
            Found::Partial => continue,
            Found::Binding(entity) => {
                pending.0.clear();
                let Some(row) = rows.iter().find(|row| row.entity == entity) else {
                    continue;
                };
                // 请求松开的绑定按住不放时只算一次（终端会上报 repeat）。
                if row.binding.releases && event.repeat {
                    continue;
                }
                match row.action {
                    Some(action) => {
                        // 补全列表开着时，↑↓/Tab/Enter/Esc 先归列表（上游 accept_completion）。
                        if let Some(state) = completion.as_mut() {
                            if completion_act(action, state, &mut editor) {
                                continue;
                            }
                        }
                        // 要送的 agent 是 `ShownAgent`：应用层在切聚焦工作区时更新它。
                        let agent = shown.as_deref().and_then(|shown| shown.0);
                        let busy = agent.is_some_and(|agent| entering.contains(agent));
                        match act(action, &mut editor, busy) {
                            Some(Act::Send(Sending { text, mode })) => {
                                // 本地回显与历史照旧；历史由 `history` 模块从
                                // `Editor.history` + `PromptSubmitted` 收一次。
                                prompts.0.push(text.clone());
                                submitted.write(PromptSubmitted(text.clone()));
                                if let Some(scroll) = scroll.as_mut() {
                                    // 新行到来时跟着末尾（上游 `pane.scroll.follow()`）。
                                    scroll.follow = true;
                                }
                                // 斜杠命令行由应用层从 `PromptSubmitted` 接到
                                // `RunCommand`，这里不再多送一条普通消息。上游靠内核的
                                // `Incoming::command` 分流；本 crate 没有 `refused` 概念，
                                // 所以也不做 `Deliver::new(..).literal()` 的重发。
                                if !is_command(&text)
                                    && let Some(agent) = agent
                                {
                                    commands.trigger(Deliver::new(agent, text, mode));
                                }
                            }
                            Some(Act::Stop) => {
                                if let Some(agent) = agent {
                                    commands.trigger(StopTurn::new(agent));
                                }
                            }
                            Some(Act::Exit) => {
                                commands.write_message(AppExit::Success);
                            }
                            Some(Act::Paste) => {
                                // 文本粘贴：有就插到光标处（多行文本由
                                // `Editor::insert` 归一成 `\n` 后原样插入），
                                // 没有就写一条和 `Copied` 同款的红色状态。
                                // 图片分支不做，原因见 [`Act::Paste`] 和
                                // `clipboard.rs` 模块文档。
                                match clipboard::paste_from(
                                    &mut editor,
                                    clipboards.system.as_deref_mut(),
                                    clipboards.memory.as_deref_mut(),
                                ) {
                                    Ok(()) => {
                                        if let Some(status) = clipboards.status.as_deref_mut() {
                                            clipboard::clear_paste_failed(status);
                                        }
                                    }
                                    Err(why) => {
                                        if let Some(status) = clipboards.status.as_deref_mut() {
                                            clipboard::show_paste_failed(status, &why);
                                        }
                                    }
                                }
                            }
                            None => {}
                        }
                    }
                    None => commands.trigger(KeyPressed {
                        entity,
                        agent: None,
                        released: false,
                    }),
                }
            }
            Found::Nothing => {
                let sequence = pending.0.len() > 1;
                pending.0.clear();
                // 多键前缀走空：整串吃掉，不把字符打进输入。
                if sequence {
                    continue;
                }
                // 面板作用域下 Esc 没被绑：把键交回输入框。
                if matches!(scope, KeyScope::Panel(_)) && press.key == Key::Escape {
                    focus.0 = None;
                    continue;
                }
                // 本 crate 只有主编辑器；面板拿键时按键不该落进主输入框
                // （上游会敲进拿到焦点的那个面板的编辑器）。
                if matches!(scope, KeyScope::Panel(_)) {
                    continue;
                }
                if press
                    .modifiers
                    .intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER)
                {
                    continue;
                }
                // 用事件带的文本（死键组合后的完整文本），不要控制字符。
                let text: String = event
                    .text
                    .as_deref()
                    .unwrap_or_default()
                    .chars()
                    .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
                    .collect();
                if !text.is_empty() {
                    editor.insert(&text);
                }
            }
        }
    }
}

/// `key_code` 对应的修饰键。
fn modifier_of(code: KeyCode) -> Modifiers {
    match code {
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Modifiers::SHIFT,
        KeyCode::ControlLeft | KeyCode::ControlRight => Modifiers::CONTROL,
        KeyCode::AltLeft | KeyCode::AltRight => Modifiers::ALT,
        KeyCode::SuperLeft | KeyCode::SuperRight => Modifiers::SUPER,
        _ => Modifiers::NONE,
    }
}

/// 输入框这一帧能折成多少列：树给 [`PanelId::Input`] 叶子的文字列数。
///
/// 根/区域还没建好（只装 `EditorPlugin` 的最小 App、第一帧）时用 97 兜底，
/// 和 `hints::hint_panel_rows` / `batch::confirm_panel_rows` 一致；行照写，
/// 只是没有根可画。
pub fn input_width(roots: Option<&PanelRoots>, areas: Option<&PanelAreas>) -> usize {
    roots
        .and_then(|roots| roots.entity(PanelId::Input))
        .and_then(|root| areas.and_then(|areas| areas.get(root)))
        .map_or(97, |area| usize::from(area.cols.max(1)))
}

/// 按输入面板的列宽算这一帧的 [`EditorLayout`]（折行、光标行列、滚动 top）。
///
/// 排在 [`keys`] 之后：同一帧敲进去的字、Shift+Enter 的新行立刻参与布局。
/// 宽度取**上一帧** `lay_out_frame` 写下的 `PanelAreas`：右列的宽度不随
/// 输入框高度变（高度只由 Spacer 吸收），所以这个宽度和这一帧布局后一致，
/// 且 `sync_editor_tile` 能在 `lay_out_frame` 之前用同一份行数改树。
pub fn layout_editor(
    editor: Res<Editor>,
    roots: Option<Res<PanelRoots>>,
    areas: Option<Res<PanelAreas>>,
    mut layout: ResMut<EditorLayout>,
) {
    let next = editor.layout(input_width(roots.as_deref(), areas.as_deref()));
    if *layout != next {
        *layout = next;
    }
}

/// 输入 tile 的高度跟着 [`EditorLayout`] 的行数走（[`input_tile_height`]）。
///
/// 和 `complete::sync_completion_tile` 一样只改 `FrameTree` 里 Input 叶子的
/// **固定高度**；右列顶部 Spacer 是比例子，多/少出来的高度全由它吸收，
/// 所以右列总高仍然精确等于区域高度。Input 保持固定子（不要改成比例子），
/// 否则 Spacer 不再吸收差额，整屏高度会失衡。
pub fn sync_editor_tile(layout: Res<EditorLayout>, tree: Option<ResMut<FrameTree>>) {
    let Some(mut tree) = tree else {
        // 只装了 `EditorPlugin` 的测试 App 没有帧树：没有 tile 可改。
        return;
    };
    let want = crate::layout::Size::Fixed(input_tile_height(layout.rows.len()));
    if tree.0.leaf_size(PanelId::Input) == Some(want) {
        return;
    }
    tree.0.set_leaf_size(PanelId::Input, want);
}

/// 输入面板这一帧的行：折好的可见行，每行一个 [`KeyedRow`]。
///
/// 行来自 [`EditorLayout`]（宽度/折行/滚动已在 [`layout_editor`] 里算好）；
/// 光标画在真实行列上，是行内的一个反色 + 下划线 span。超过
/// [`MOST_LINES`] 行时只写 `top..top+visible`，光标行一定在可见段里。
pub fn editor_rows(mut panels: ResMut<PanelRows>, layout: Res<EditorLayout>, theme: Res<Theme>) {
    let rows: Vec<KeyedRow> = layout
        .visible_rows(&theme)
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let row = Row::new(line);
            KeyedRow {
                key: (layout.top + index + 1) as u64,
                hash: row_hash(&row),
                row,
            }
        })
        .collect();
    panels.set(PanelId::Input, rows);
}

pub fn ratatui_span(text: impl Into<String>, style: Style) -> rig_tui::ratatui::text::Span<'static> {
    rig_tui::ratatui::text::Span::styled(text.into(), style)
}

pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Editor>()
            .init_resource::<EditorLayout>()
            .init_resource::<SubmittedPrompts>()
            .init_resource::<KeyFocus>()
            .init_resource::<PendingInput>()
            .add_message::<PromptSubmitted>();
        // 43 条输入键 + 转写区 Esc + 先禁用的 Alt+1..9/Alt+0。
        crate::keys::add_built_in(app);
        app.add_systems(
            Update,
            (
                keys.run_if(crate::picker::no_picker),
                layout_editor,
                sync_editor_tile,
            )
                .chain()
                .in_set(crate::FeathersSystems::Prepare),
        )
        .add_systems(
            Update,
            editor_rows.in_set(crate::FeathersSystems::Draw),
        );
    }
}

/// `text` 里的 `\r\n` 与单独的 `\r` 都归一成 `\n`。
fn newlines(text: String) -> String {
    match text.contains('\r') {
        true => text.replace("\r\n", "\n").replace('\r', "\n"),
        false => text,
    }
}

/// `grapheme` 是否属于一个词：首字符是字母、数字或 `_`。
fn word_grapheme(grapheme: &str) -> bool {
    grapheme
        .chars()
        .next()
        .is_some_and(|first| first.is_alphanumeric() || first == '_')
}

/// 历史条目 `shown` 现在在哪个位置：还在原下标就是它，否则按文本找，找不到
/// 就当它被挤到最新一条之后。
fn place(index: usize, shown: &str, history: &[String]) -> usize {
    if history.get(index).is_some_and(|entry| entry == shown) {
        return index;
    }
    history
        .iter()
        .rposition(|entry| entry == shown)
        .unwrap_or(history.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::ClipboardText;
    use crate::complete::{Candidate, Completion, CompletionKind, CompletionState};
    use crate::keys::{AppKeyBindingsExt, KeyPress};
    use bevy::ecs::event::EntityEvent;
    use rig_ecs::prelude::TurnOf;

    fn ed(text: &str) -> Editor {
        let mut editor = Editor::default();
        editor.set(text);
        editor
    }

    fn history_editor(draft: &str, history: &[&str]) -> Editor {
        let mut editor = ed(draft);
        editor.history = history.iter().map(|entry| (*entry).to_owned()).collect();
        editor
    }

    /// 一行里所有 span 的纯文本（光标是其中一个 span，测试直接找它）。
    fn plain(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    /// 只装布局/产行系统的最小 App：`width` 是输入面板的文字列数。
    fn layout_app(width: u16) -> App {
        let mut app = App::new();
        app.init_resource::<Editor>()
            .init_resource::<EditorLayout>()
            .init_resource::<PanelRows>()
            .init_resource::<Theme>();
        let root = app.world_mut().spawn_empty().id();
        let mut roots = PanelRoots::default();
        roots.0.insert(PanelId::Input, root);
        let mut areas = PanelAreas::default();
        areas.0.insert(
            root,
            crate::frame::PanelArea {
                cols: width,
                rows: 3,
                ..Default::default()
            },
        );
        app.insert_resource(roots)
            .insert_resource(areas)
            .add_systems(Update, (layout_editor, editor_rows).chain());
        app
    }

    /// 只装布局 + tile 同步的最小 App（带一棵默认帧树）。
    fn tile_app() -> App {
        let mut app = App::new();
        app.init_resource::<Editor>()
            .init_resource::<EditorLayout>()
            .insert_resource(FrameTree(crate::layout::default_tree()))
            .add_systems(Update, (layout_editor, sync_editor_tile).chain());
        app
    }

    /// 帧树里 Input 叶子的固定高度（UI 像素）。
    fn input_tile_in(app: &App) -> f32 {
        match app
            .world()
            .resource::<FrameTree>()
            .0
            .leaf_size(PanelId::Input)
        {
            Some(crate::layout::Size::Fixed(px)) => px,
            other => panic!("Input 应是 Fixed 尺寸：{other:?}"),
        }
    }

    // ---- grapheme：中文 / emoji / 组合字符 ----

    #[test]
    fn chinese_moves_and_deletes_by_grapheme() {
        let mut editor = ed("你好世界");
        assert_eq!(editor.cursor(), 12, "4 个汉字 = 12 字节");
        editor.left();
        assert_eq!(editor.cursor(), 9);
        editor.left();
        assert_eq!(editor.cursor(), 6);
        editor.right();
        assert_eq!(editor.cursor(), 9);
        editor.right();
        assert_eq!(editor.cursor(), 12);
        editor.backspace();
        assert_eq!(editor.text(), "你好世");
        assert_eq!(editor.cursor(), 9);
        editor.backspace();
        assert_eq!(editor.text(), "你好");
        assert_eq!(editor.cursor(), 6);
        editor.set_cursor(3);
        editor.insert_char('x');
        assert_eq!(editor.text(), "你x好");
        assert_eq!(editor.cursor(), 4);
        editor.set_cursor(1);
        assert_eq!(editor.cursor(), 0, "光标吸附到前一个 grapheme 边界");
    }

    #[test]
    fn emoji_zwj_sequences_are_one_grapheme() {
        // "a" + 👩‍💻（👩 4 字节 + ZWJ 3 字节 + 💻 4 字节）+ "b"
        let text = "a👩\u{200d}💻b";
        assert_eq!(text.len(), 13);
        let mut editor = ed(text);
        assert_eq!(editor.cursor(), 13);
        editor.left();
        assert_eq!(editor.cursor(), 12);
        editor.left();
        assert_eq!(editor.cursor(), 1);
        editor.left();
        assert_eq!(editor.cursor(), 0);
        editor.right();
        assert_eq!(editor.cursor(), 1);
        assert_eq!(editor.text().get(1..12), Some("👩\u{200d}💻"));
        editor.right();
        assert_eq!(editor.cursor(), 12);
        editor.backspace();
        assert_eq!(editor.text(), "ab");
        assert_eq!(editor.cursor(), 1);
        editor.delete();
        assert_eq!(editor.text(), "a");
        editor.set_cursor(2);
        assert_eq!(editor.cursor(), 1, "👩‍💻 中间吸附到它前面");
    }

    #[test]
    fn combining_marks_are_one_grapheme() {
        // e + U+0301（组合尖音）：一个 grapheme，3 字节
        let mut editor = ed("e\u{301}x");
        assert_eq!(editor.cursor(), 4);
        editor.left();
        assert_eq!(editor.cursor(), 3);
        editor.left();
        assert_eq!(editor.cursor(), 0);
        editor.set_cursor(3);
        editor.backspace();
        assert_eq!(editor.text(), "x");
        assert_eq!(editor.cursor(), 0);
        let mut editor = ed("e\u{301}");
        editor.home();
        editor.insert("a");
        assert_eq!(editor.text(), "ae\u{301}");
        assert_eq!(editor.cursor(), 1);
    }

    #[test]
    fn words_move_over_chinese_and_delete_by_grapheme() {
        let mut editor = ed("hello 世界");
        editor.word_left();
        assert_eq!(editor.cursor(), 6);
        editor.word_left();
        assert_eq!(editor.cursor(), 0);
        editor.word_right();
        assert_eq!(editor.cursor(), 5);
        editor.word_right();
        assert_eq!(editor.cursor(), 12);
        editor.delete_word_before();
        assert_eq!(editor.text(), "hello ");
        assert_eq!(editor.cursor(), 6);
        editor.delete_word_before();
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor(), 0);
        let mut editor = ed("hello 世界");
        editor.set_cursor(0);
        editor.delete_word_after();
        assert_eq!(editor.text(), " 世界");
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn line_deletes_use_grapheme_boundaries() {
        let mut editor = ed("abc\ndef");
        editor.set_cursor(5);
        editor.delete_to_line_start();
        assert_eq!(editor.text(), "abc\nef");
        assert_eq!(editor.cursor(), 4);
        editor.set_cursor(4);
        editor.delete_to_line_start();
        assert_eq!(editor.text(), "abcef");
        assert_eq!(editor.cursor(), 3);

        let mut editor = ed("abc\ndef");
        editor.set_cursor(1);
        editor.delete_to_line_end();
        assert_eq!(editor.text(), "a\ndef");
        assert_eq!(editor.cursor(), 1);
        editor.set_cursor(1);
        editor.delete_to_line_end();
        assert_eq!(editor.text(), "adef");
        assert_eq!(editor.cursor(), 1);
    }

    // ---- 历史：空 / 单条 / 多条 / 跨行 ----

    #[test]
    fn up_and_down_on_an_empty_history_only_move_the_cursor_to_the_line_start() {
        let mut editor = ed("draft");
        editor.up();
        assert_eq!(editor.text(), "draft");
        assert_eq!(editor.cursor(), 0, "Up 先到第一行行首");
        assert_eq!(editor.history_pos, None);
        editor.up();
        assert_eq!(editor.text(), "draft");
        assert_eq!(editor.cursor(), 0, "空历史时停在行首");
        assert_eq!(editor.history_pos, None);
        editor.down();
        assert_eq!(editor.text(), "draft");
        assert_eq!(editor.cursor(), 5, "Down 到最后一行行尾");
        assert_eq!(editor.history_pos, None);
    }

    #[test]
    fn one_history_entry_round_trips_to_the_draft() {
        let mut editor = history_editor("draft", &["one"]);
        editor.up();
        assert_eq!((editor.text(), editor.cursor()), ("draft", 0));
        editor.up();
        assert_eq!(editor.text(), "one");
        assert_eq!(editor.cursor(), 3);
        assert_eq!(editor.history_pos, Some(0));
        assert_eq!(editor.recalled(), true);
        editor.up();
        assert_eq!(editor.text(), "one", "最新到最早一条后不再动");
        assert_eq!(editor.history_pos, Some(0));
        editor.down();
        assert_eq!(editor.text(), "draft", "翻过最新一条回到草稿");
        assert_eq!(editor.cursor(), 5);
        assert_eq!(editor.history_pos, None);
        editor.down();
        assert_eq!(editor.text(), "draft", "草稿后面没有了");
        assert_eq!(editor.cursor(), 5);
    }

    #[test]
    fn many_history_entries_walk_both_ways() {
        let mut editor = history_editor("draft", &["one", "two", "three"]);
        editor.up();
        editor.up();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("three".to_owned(), Some(2)));
        editor.up();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("two".to_owned(), Some(1)));
        editor.up();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("one".to_owned(), Some(0)));
        editor.up();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("one".to_owned(), Some(0)));
        editor.down();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("two".to_owned(), Some(1)));
        editor.down();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("three".to_owned(), Some(2)));
        editor.down();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("draft".to_owned(), None));
        assert_eq!(editor.cursor(), 5);
    }

    #[test]
    fn ctrl_p_enters_the_history_from_the_middle_of_the_first_line() {
        let mut editor = history_editor("draft", &["one", "two"]);
        editor.set_cursor(2);
        editor.line_up();
        assert_eq!(editor.text(), "two", "Ctrl+P 不走行内上移");
        assert_eq!(editor.history_pos, Some(1));
        editor.line_down();
        assert_eq!(editor.text(), "draft");
        assert_eq!(editor.history_pos, None);
    }

    #[test]
    fn up_and_down_move_between_lines_before_the_history() {
        let mut editor = ed("ab\ncd");
        assert_eq!(editor.cursor(), 5);
        editor.up();
        assert_eq!(editor.cursor(), 2, "上一行同列");
        editor.up();
        assert_eq!(editor.cursor(), 0, "第一行行首");
        editor.down();
        assert_eq!(editor.cursor(), 3, "下一行同列");
        editor.down();
        assert_eq!(editor.cursor(), 5, "最后一行行尾");
        let mut editor = history_editor("ab\ncd", &["old"]);
        editor.up();
        editor.up();
        editor.up();
        assert_eq!(editor.text(), "old");
        assert_eq!(editor.history_pos, Some(0));
    }

    #[test]
    fn editing_a_recalled_prompt_keeps_it_out_of_the_walking_path() {
        let mut editor = history_editor("draft", &["one"]);
        editor.up();
        editor.up();
        assert_eq!(editor.text(), "one");
        editor.insert("!");
        assert_eq!(editor.text(), "one!");
        assert_eq!(editor.recalled(), false);
        editor.down();
        assert_eq!(editor.text(), "one!");
        assert_eq!(editor.cursor(), 4, "改过之后 Down 只走行尾");
    }

    #[test]
    fn clear_keeps_the_unsent_text_for_up() {
        let mut editor = ed("keep me");
        editor.clear();
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor(), 0);
        assert_eq!(editor.history, vec!["keep me".to_owned()]);
        editor.up();
        assert_eq!(editor.text(), "keep me");
        assert_eq!(editor.history_pos, Some(0));
    }

    #[test]
    fn enter_sends_the_trimmed_text_and_clears_the_editor() {
        let mut editor = ed("  hello  ");
        assert!(matches!(
            act(InputAction::Send, &mut editor, false),
            Some(Act::Send(Sending { text, mode }))
                if text == "hello" && mode == DeliveryMode::Steer
        ));
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor(), 0);
        assert_eq!(editor.history, vec!["hello".to_owned()]);
        let mut empty = ed("   ");
        assert!(
            matches!(act(InputAction::Send, &mut empty, false), None),
            "空白不发送，也不清空"
        );
        assert_eq!(empty.text(), "   ");
        assert!(
            matches!(act(InputAction::Send, &mut empty, true), None),
            "turn 跑着时空输入也不排队"
        );
    }

    #[test]
    fn tab_queues_only_while_a_turn_runs() {
        let mut editor = ed("later");
        assert!(matches!(
            act(InputAction::Tab, &mut editor, true),
            Some(Act::Send(Sending { text, mode }))
                if text == "later" && mode == DeliveryMode::Queue
        ));
        assert!(editor.is_empty(), "排队也把输入取走");
        assert_eq!(editor.history, vec!["later".to_owned()]);

        // 没在跑：Tab 还是停止浏览历史，不发送。
        let mut idle = ed("draft");
        assert!(matches!(act(InputAction::Tab, &mut idle, false), None));
        assert_eq!(idle.text(), "draft", "不忙时 Tab 不动输入");
    }

    #[test]
    fn escape_stops_only_while_a_turn_runs() {
        let mut editor = ed("keep");
        assert!(matches!(
            act(InputAction::Stop, &mut editor, true),
            Some(Act::Stop)
        ));
        assert_eq!(editor.text(), "keep", "停 agent 不动输入");
        assert!(matches!(act(InputAction::Stop, &mut editor, false), None));
    }

    #[test]
    fn ctrl_d_quits_only_on_an_empty_input() {
        let mut empty = Editor::default();
        assert!(matches!(
            act(InputAction::Exit, &mut empty, false),
            Some(Act::Exit)
        ));
        let mut typed = ed("draft");
        assert!(matches!(act(InputAction::Exit, &mut typed, false), None));
        assert_eq!(typed.text(), "draft", "非空输入时 Ctrl+D 不退、也不清空");
    }

    // ---- keys 系统：动作表真的接上了 ----

    fn test_app() -> App {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .add_message::<PromptSubmitted>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Editor>()
            .init_resource::<SubmittedPrompts>()
            .init_resource::<KeyFocus>()
            .init_resource::<PendingInput>();
        crate::keys::add_built_in(&mut app);
        app.add_systems(Update, keys);
        app
    }

    fn send(app: &mut App, key: Key, code: KeyCode, text: Option<&str>) {
        app.world_mut().write_message(KeyboardInput {
            key_code: code,
            logical_key: key,
            state: ButtonState::Pressed,
            text: text.map(|text| text.into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    fn character(app: &mut App, character: char) {
        let code = KeyCode::KeyA; // 测试里只关心字符与文本，不关心物理键
        send(
            app,
            Key::Character(character.to_string().into()),
            code,
            Some(&character.to_string()),
        );
    }

    fn with_modifier(app: &mut App, modifier: KeyCode, key: Key, code: KeyCode) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(modifier);
        send(app, key, code, None);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(modifier);
    }

    fn text(app: &App) -> String {
        app.world().resource::<Editor>().text.clone()
    }

    #[test]
    fn the_built_in_table_drives_typing_and_editing() {
        let mut app = test_app();
        for character in ['a', 'b', 'c'] {
            self::character(&mut app, character);
        }
        assert_eq!(text(&app), "abc");
        assert_eq!(app.world().resource::<Editor>().cursor(), 3);

        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("a".into()), KeyCode::KeyA);
        assert_eq!(app.world().resource::<Editor>().cursor(), 0, "Ctrl+A 行首");
        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("e".into()), KeyCode::KeyE);
        assert_eq!(app.world().resource::<Editor>().cursor(), 3, "Ctrl+E 行尾");

        app.world_mut().resource_mut::<Editor>().set("hello 世界");
        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("w".into()), KeyCode::KeyW);
        assert_eq!(text(&app), "hello ", "Ctrl+W 按 grapheme 删掉「世界」");

        // Ctrl+U / Ctrl+K 的真实删除结果。
        app.world_mut().resource_mut::<Editor>().set("abc def");
        app.world_mut().resource_mut::<Editor>().set_cursor(3);
        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("k".into()), KeyCode::KeyK);
        assert_eq!(text(&app), "abc", "Ctrl+K 删到行尾");
        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("u".into()), KeyCode::KeyU);
        assert_eq!(text(&app), "", "Ctrl+U 删到行首");

        app.world_mut().resource_mut::<Editor>().set("one two");
        with_modifier(&mut app, KeyCode::AltLeft, Key::Character("b".into()), KeyCode::KeyB);
        assert_eq!(app.world().resource::<Editor>().cursor(), 4, "Alt+B 词首");
        with_modifier(&mut app, KeyCode::AltLeft, Key::Character("f".into()), KeyCode::KeyF);
        assert_eq!(app.world().resource::<Editor>().cursor(), 7, "Alt+F 词尾");
        app.world_mut().resource_mut::<Editor>().set("one two");
        app.world_mut().resource_mut::<Editor>().set_cursor(0);
        with_modifier(&mut app, KeyCode::AltLeft, Key::Character("d".into()), KeyCode::KeyD);
        assert_eq!(text(&app), " two", "Alt+D 删后一个词");
    }

    #[test]
    fn shift_enter_inserts_a_newline_and_enter_sends() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ShiftLeft);
        send(&mut app, Key::Enter, KeyCode::Enter, None);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ShiftLeft);
        assert_eq!(text(&app), "\n");
        self::character(&mut app, 'x');
        assert_eq!(text(&app), "\nx");
        send(&mut app, Key::Enter, KeyCode::Enter, None);
        assert_eq!(text(&app), "");
        let submitted = app.world().resource::<SubmittedPrompts>();
        assert_eq!(submitted.0, vec!["x".to_owned()], "发送时去掉首尾空白（沿用旧行为）");
        let editor = app.world().resource::<Editor>();
        assert_eq!(editor.history, vec!["x".to_owned()]);
    }

    #[test]
    fn esc_and_up_down_are_wired_through_the_table() {
        let mut app = test_app();
        self::character(&mut app, 'a');
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Escape,
            logical_key: Key::Escape,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert_eq!(text(&app), "a", "Esc 是 Stop，不是字符");

        app.world_mut().resource_mut::<Editor>().history =
            vec!["one".to_owned(), "two".to_owned()];
        send(&mut app, Key::ArrowUp, KeyCode::ArrowUp, None);
        assert_eq!(text(&app), "a", "先到第一行行首");
        send(&mut app, Key::ArrowUp, KeyCode::ArrowUp, None);
        assert_eq!(text(&app), "two");
        send(&mut app, Key::ArrowDown, KeyCode::ArrowDown, None);
        assert_eq!(text(&app), "a", "Down 回到草稿");
    }

    #[test]
    fn a_plugins_binding_receives_key_pressed() {
        let mut app = test_app();
        app.add_key_binding(
            KeyBinding::new(KeyPress::alt('t'), "Template").in_scope(KeyScope::Global),
            |pressed: On<KeyPressed>, mut editor: ResMut<Editor>| {
                if !pressed.released {
                    editor.insert("T");
                }
            },
        );
        let event = KeyboardInput {
            key_code: KeyCode::KeyT,
            logical_key: Key::Character("t".into()),
            state: ButtonState::Pressed,
            text: Some("t".into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        };
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::AltLeft);
        app.world_mut().write_message(event);
        app.update();
        assert_eq!(text(&app), "T", "全局插件绑定遮住打字");
        assert!(app.world().resource::<SubmittedPrompts>().0.is_empty());
    }

    // ---- 发送 / 排队 / 停止 / 退出：键 -> 内核事件的整条链路 ----

    /// 观察到的事件（无窗口 app 里由观察者收）。
    #[derive(Resource, Default)]
    struct Observed {
        delivered: Vec<(Entity, String, DeliveryMode)>,
        stopped: Vec<Entity>,
    }

    fn record_deliver(deliver: On<Deliver>, mut observed: ResMut<Observed>) {
        observed.delivered.push((
            deliver.entity,
            deliver.message.text().to_owned(),
            deliver.message.mode,
        ));
    }

    fn record_stop(stop: On<StopTurn>, mut observed: ResMut<Observed>) {
        observed.stopped.push(stop.event_target());
    }

    /// `test_app` + 一个真 agent + `ShownAgent` + 转写窗格 + `Deliver`/`StopTurn`
    /// 观察者；不建窗口、不装渲染。
    fn sending_app() -> (App, Entity) {
        let mut app = test_app();
        let agent = app.world_mut().spawn(Name::new("agent")).id();
        app.insert_resource(ShownAgent(Some(agent)))
            .insert_resource(PaneScroll {
                top: 3,
                follow: false,
            })
            .init_resource::<Observed>()
            .add_observer(record_deliver)
            .add_observer(record_stop);
        (app, agent)
    }

    /// 让 `agent` 看起来有一轮正在跑：`TurnOf` 的关系目标就是 `ActiveTurn`。
    fn run_a_turn(app: &mut App, agent: Entity) {
        app.world_mut().spawn(TurnOf(agent));
        app.update();
        assert!(
            app.world().entity(agent).contains::<ActiveTurn>(),
            "TurnOf 应给 agent 挂上 ActiveTurn"
        );
    }

    fn observed(app: &App) -> &Observed {
        app.world().resource::<Observed>()
    }

    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            self::character(app, character);
        }
    }

    /// 这一帧的 `AppExit`。
    fn exits(app: &App) -> Vec<AppExit> {
        app.world()
            .resource::<Messages<AppExit>>()
            .iter_current_update_messages()
            .cloned()
            .collect()
    }

    #[test]
    fn enter_delivers_the_trimmed_text_to_the_shown_agent_as_steer() {
        let (mut app, agent) = sending_app();
        assert_eq!(app.world().resource::<ShownAgent>().0, Some(agent));
        type_text(&mut app, "  hi  ");
        send(&mut app, Key::Enter, KeyCode::Enter, None);

        assert_eq!(
            observed(&app).delivered,
            [(agent, "hi".to_owned(), DeliveryMode::Steer)],
            "非空 Enter 应把 trim 过的文本以 Steer 送给 ShownAgent"
        );
        assert_eq!(
            app.world().resource::<SubmittedPrompts>().0,
            ["hi"],
            "本地回显照旧"
        );
        let editor = app.world().resource::<Editor>();
        assert_eq!(editor.text(), "", "发送后输入清空");
        assert_eq!(editor.history, ["hi"], "历史照旧记一条");
        assert!(
            app.world().resource::<PaneScroll>().follow,
            "发送后转写窗格跟着末尾"
        );

        // 空输入不触发。
        send(&mut app, Key::Enter, KeyCode::Enter, None);
        assert_eq!(observed(&app).delivered.len(), 1, "空输入不再送");
        assert_eq!(app.world().resource::<SubmittedPrompts>().0, ["hi"]);
    }

    #[test]
    fn enter_while_a_turn_runs_still_steers() {
        let (mut app, agent) = sending_app();
        run_a_turn(&mut app, agent);
        type_text(&mut app, "now");
        send(&mut app, Key::Enter, KeyCode::Enter, None);
        assert_eq!(
            observed(&app).delivered,
            [(agent, "now".to_owned(), DeliveryMode::Steer)],
            "Enter 永远是 Steer（忙时插进这一轮）"
        );
    }

    #[test]
    fn tab_queues_while_a_turn_runs() {
        let (mut app, agent) = sending_app();
        run_a_turn(&mut app, agent);
        type_text(&mut app, "later");
        send(&mut app, Key::Tab, KeyCode::Tab, None);

        assert_eq!(
            observed(&app).delivered,
            [(agent, "later".to_owned(), DeliveryMode::Queue)],
            "turn 跑着时 Tab 是排队"
        );
        let editor = app.world().resource::<Editor>();
        assert_eq!(editor.text(), "", "排队也把输入取走");
        assert_eq!(editor.history, ["later"]);
        assert!(app.world().resource::<PaneScroll>().follow);
    }

    #[test]
    fn tab_accepts_the_completion_instead_of_queueing_when_the_list_is_open() {
        let (mut app, agent) = sending_app();
        run_a_turn(&mut app, agent);
        app.init_resource::<CompletionState>();
        app.world_mut().resource_mut::<Editor>().set("he");
        app.world_mut()
            .resource_mut::<CompletionState>()
            .show(Completion::from_items(
                CompletionKind::Command,
                0,
                "he",
                vec![Candidate {
                    text: "help".into(),
                    detail: "Show help".into(),
                }],
            ));

        send(&mut app, Key::Tab, KeyCode::Tab, None);

        assert!(
            !app.world().resource::<CompletionState>().is_open(),
            "补全开着时 Tab 仍然接受候选"
        );
        assert!(
            observed(&app).delivered.is_empty(),
            "接受补全不能顺手发一条消息"
        );
        assert!(text(&app).contains("help"), "候选落进输入：{:?}", text(&app));
    }

    #[test]
    fn escape_stops_the_running_turn() {
        let (mut app, agent) = sending_app();
        run_a_turn(&mut app, agent);
        type_text(&mut app, "keep");
        send(&mut app, Key::Escape, KeyCode::Escape, None);

        assert_eq!(observed(&app).stopped, [agent], "Esc 停的是 ShownAgent");
        assert!(observed(&app).delivered.is_empty(), "Esc 不发送");
        assert_eq!(text(&app), "keep", "停 agent 不动输入");
    }

    #[test]
    fn escape_closes_the_completion_without_stopping_the_turn() {
        let (mut app, agent) = sending_app();
        run_a_turn(&mut app, agent);
        app.init_resource::<CompletionState>();
        app.world_mut().resource_mut::<Editor>().set("he");
        app.world_mut()
            .resource_mut::<CompletionState>()
            .show(Completion::from_items(
                CompletionKind::Command,
                0,
                "he",
                vec![Candidate {
                    text: "help".into(),
                    detail: "Show help".into(),
                }],
            ));

        send(&mut app, Key::Escape, KeyCode::Escape, None);

        assert!(
            !app.world().resource::<CompletionState>().is_open(),
            "补全开着时 Esc 只关列表"
        );
        assert!(
            observed(&app).stopped.is_empty(),
            "关补全不能顺手停掉正在跑的 turn"
        );
        assert_eq!(text(&app), "he", "Esc 不动输入");
    }

    #[test]
    fn ctrl_d_exits_only_when_the_input_is_empty() {
        let mut app = test_app();
        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("d".into()), KeyCode::KeyD);
        assert_eq!(exits(&app), [AppExit::Success], "空输入 Ctrl+D 退出");

        app.world_mut().resource_mut::<Editor>().set("draft");
        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("d".into()), KeyCode::KeyD);
        assert!(exits(&app).is_empty(), "有字时 Ctrl+D 不退");
        assert_eq!(text(&app), "draft", "有字时 Ctrl+D 也不清空");
    }

    #[test]
    fn a_slash_command_is_submitted_but_not_delivered() {
        let (mut app, _agent) = sending_app();
        type_text(&mut app, "/hello");
        send(&mut app, Key::Enter, KeyCode::Enter, None);

        assert!(
            observed(&app).delivered.is_empty(),
            "斜杠命令交给应用层的 RunCommand，视图不再额外送普通消息"
        );
        assert_eq!(
            app.world().resource::<SubmittedPrompts>().0,
            ["/hello"],
            "本地回显照旧"
        );
        let messages: Vec<&PromptSubmitted> = app
            .world()
            .resource::<Messages<PromptSubmitted>>()
            .iter_current_update_messages()
            .collect();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, "/hello");
    }

    #[test]
    fn arrows_reach_history_when_the_completion_list_is_closed() {
        let mut app = test_app();
        app.init_resource::<CompletionState>();
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.set("draft");
        editor.history = vec!["one".to_owned()];
        drop(editor);

        // 补全资源在、但列表关着：↑ 应该先到行首、再进历史（以前被补全吃掉）。
        send(&mut app, Key::ArrowUp, KeyCode::ArrowUp, None);
        assert_eq!(text(&app), "draft");
        send(&mut app, Key::ArrowUp, KeyCode::ArrowUp, None);
        assert_eq!(text(&app), "one", "补全关着时 ↑ 进历史");
        send(&mut app, Key::ArrowDown, KeyCode::ArrowDown, None);
        assert_eq!(text(&app), "draft", "Down 回到草稿");
    }

    // ---- 多行显示：折行 / 光标行列 / 滚动 ----

    #[test]
    fn long_text_wraps_to_the_input_width() {
        let mut editor = ed("one two three four five");
        // width=12：提示符占 2 列，正文 10 列。
        let layout = editor.layout(12);
        assert_eq!(layout.rows.len(), 3, "10 列宽折成 3 行");
        assert_eq!(layout.visible, 3);
        assert_eq!(layout.top, 0, "行数没超上限，不滚动");
        let raw: Vec<String> = layout.rows.iter().map(plain).collect();
        assert_eq!(raw, ["one two", "three", "four five"]);
        editor.set_cursor(0);
        let layout = editor.layout(12);
        let shown: Vec<String> = layout
            .visible_rows(&Theme::default())
            .iter()
            .map(plain)
            .collect();
        assert_eq!(shown, ["› ▏one two", "  three", "  four five"]);

        // 更窄：同样的文本要折更多行，每行正文不超过正文宽度。
        let narrow = editor.layout(8);
        assert!(
            narrow.rows.len() > layout.rows.len(),
            "8 列宽应折出更多行：{} → {}",
            layout.rows.len(),
            narrow.rows.len()
        );
        assert!(
            narrow.rows.iter().all(|row| row.width() <= 6),
            "每行正文宽度不超过 6 列"
        );
    }

    #[test]
    fn cursor_is_drawn_at_its_real_row_and_column() {
        let mut editor = ed("one two three four five");
        // "one two " 之后是 "three" 的 8..13；11 在 "thr|ee" 中间。
        editor.set_cursor(11);
        let layout = editor.layout(12);
        assert_eq!((layout.cursor_row, layout.cursor_column), (1, 3));
        assert_eq!(layout.cursor_in_view(), Some(1));
        let rows: Vec<String> = layout
            .visible_rows(&Theme::default())
            .iter()
            .map(plain)
            .collect();
        assert_eq!(rows[0], "› one two");
        assert_eq!(rows[1], "  thr▏ee", "光标画在第二行第 3 个显示列");
        assert_eq!(rows[2], "  four five");
        assert!(!rows[0].contains(CURSOR), "光标不能留在第一行");
        let line = &layout.visible_rows(&Theme::default())[1];
        let marker = line
            .spans
            .iter()
            .find(|span| span.content == CURSOR)
            .expect("光标是行里的一个 span");
        assert!(
            marker.style.add_modifier.contains(Modifier::REVERSED),
            "光标要反色"
        );
        assert!(
            marker.style.add_modifier.contains(Modifier::UNDERLINED),
            "光标要下划线"
        );

        // 行首：光标在最前面。
        editor.set_cursor(8);
        let layout = editor.layout(12);
        assert_eq!((layout.cursor_row, layout.cursor_column), (1, 0));
        let rows: Vec<String> = layout
            .visible_rows(&Theme::default())
            .iter()
            .map(plain)
            .collect();
        assert_eq!(rows[1], "  ▏three");
    }

    #[test]
    fn cursor_columns_count_double_width_characters() {
        let mut editor = ed("你好世界");
        editor.set_cursor(6); // 世界 前
        let layout = editor.layout(6); // 正文 4 列：你好 / 世界
        assert_eq!(layout.rows.len(), 2);
        assert_eq!((layout.cursor_row, layout.cursor_column), (1, 0));
        let rows: Vec<String> = layout
            .visible_rows(&Theme::default())
            .iter()
            .map(plain)
            .collect();
        assert_eq!(rows[1], "  ▏世界");

        editor.set_cursor(9); // 世 与 界 之间
        let layout = editor.layout(6);
        assert_eq!((layout.cursor_row, layout.cursor_column), (1, 2));
        let rows: Vec<String> = layout
            .visible_rows(&Theme::default())
            .iter()
            .map(plain)
            .collect();
        assert_eq!(rows[1], "  世▏界", "汉字算 2 个显示列");
    }

    #[test]
    fn a_cursor_in_whitespace_dropped_by_wrapping_stays_at_its_column() {
        let mut editor = ed("ab   cd");
        editor.set_cursor(3); // 三个空格里的第一个：折行时可能被丢掉
        let layout = editor.layout(6); // 正文 4 列
        let in_view = layout.cursor_in_view().expect("光标行要在可见段里");
        let row = &layout.visible_rows(&Theme::default())[in_view];
        let text = plain(row);
        let at = text.find(CURSOR).expect("光标标记");
        assert_eq!(
            UnicodeWidthStr::width(&text[..at]),
            2 + layout.cursor_column,
            "光标前的显示宽 = 提示符 2 列 + 算出的列：{text:?}"
        );
    }

    #[test]
    fn a_cursor_after_a_full_row_starts_a_new_row() {
        let editor = ed("0123456789"); // 正好 10 列 = 正文宽度
        let layout = editor.layout(12);
        assert_eq!(layout.rows.len(), 2, "写满一行后光标另起一行");
        assert_eq!((layout.cursor_row, layout.cursor_column), (1, 0));
        let rows: Vec<String> = layout
            .visible_rows(&Theme::default())
            .iter()
            .map(plain)
            .collect();
        assert_eq!(rows[1], "  ▏", "新行只有光标");
    }

    #[test]
    fn scrolling_keeps_the_cursor_row_visible() {
        let text = (0..15)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = ed(&text);
        let layout = editor.layout(97);
        assert_eq!(layout.rows.len(), 15, "15 行文本");
        assert_eq!(layout.visible, MOST_LINES, "最多显示 10 行");
        assert_eq!(layout.top, 5, "光标在最后一行：可见段是 5..15");
        assert_eq!(layout.cursor_in_view(), Some(MOST_LINES - 1));
        let rows: Vec<String> = layout
            .visible_rows(&Theme::default())
            .iter()
            .map(plain)
            .collect();
        assert_eq!(rows.len(), MOST_LINES);
        assert_eq!(rows[0], "  line 5", "滚到光标所在的那一段");
        assert!(rows[9].contains("line 14"));
        assert!(rows[9].contains(CURSOR), "光标行在可见段里");

        // 光标回到第一行：滚回顶部。
        editor.set_cursor(0);
        let layout = editor.layout(97);
        assert_eq!(layout.top, 0);
        assert_eq!(layout.cursor_in_view(), Some(0));
        let rows: Vec<String> = layout
            .visible_rows(&Theme::default())
            .iter()
            .map(plain)
            .collect();
        assert_eq!(rows[0], "› ▏line 0");
    }

    #[test]
    fn editor_rows_writes_every_visible_line_with_the_cursor_in_place() {
        let mut app = layout_app(12);
        app.world_mut()
            .resource_mut::<Editor>()
            .set("one two three four five");
        app.update();
        assert_eq!(
            app.world()
                .resource::<PanelRows>()
                .get(PanelId::Input)
                .len(),
            3,
            "3 个可见行 = 3 个 KeyedRow"
        );

        app.world_mut().resource_mut::<Editor>().set_cursor(11);
        app.update();
        let rows = app.world().resource::<PanelRows>().get(PanelId::Input);
        let shown: Vec<String> = rows.iter().map(|keyed| plain(&keyed.row.line)).collect();
        assert_eq!(shown[0], "› one two");
        assert_eq!(shown[1], "  thr▏ee", "系统产出的行里光标在真实列上");
        assert_eq!(shown[2], "  four five");
    }

    #[test]
    fn editor_rows_scrolls_to_keep_the_cursor_line_visible() {
        let mut app = layout_app(97);
        let text = (0..20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.world_mut().resource_mut::<Editor>().set(text);
        app.update();

        let rows = app.world().resource::<PanelRows>().get(PanelId::Input);
        assert_eq!(rows.len(), MOST_LINES, "只写可见的 10 行");
        let shown: Vec<String> = rows.iter().map(|keyed| plain(&keyed.row.line)).collect();
        assert_eq!(shown[0], "  line 10", "滚到光标所在段的第一行");
        assert!(shown[9].contains("line 19"), "最后一行是光标行");
        assert!(shown[9].contains(CURSOR), "光标标记在最后一行");
    }

    // ---- 输入 tile 高度 ----

    #[test]
    fn input_tile_height_grows_with_lines_and_has_a_floor() {
        assert_eq!(INPUT_CHROME, 20.0, "pad 8 + border 2 的上下共 20px");
        assert_eq!(input_tile_height(0), 3.0 * ROW_HEIGHT, "空输入也保底 3 行");
        assert_eq!(input_tile_height(1), 3.0 * ROW_HEIGHT, "1 行 + chrome 低于下限");
        assert_eq!(input_tile_height(1), 54.0);
        assert_eq!(input_tile_height(2), 2.0 * ROW_HEIGHT + INPUT_CHROME);
        assert_eq!(input_tile_height(2), 56.0);
        assert_eq!(input_tile_height(4), 4.0 * ROW_HEIGHT + INPUT_CHROME);
        assert_eq!(input_tile_height(4), 92.0);
        assert_eq!(input_tile_height(10), 10.0 * ROW_HEIGHT + INPUT_CHROME);
        assert_eq!(input_tile_height(10), 200.0);
        assert_eq!(
            input_tile_height(99),
            input_tile_height(10),
            "超过 10 行封顶，由滚动露出"
        );
    }

    #[test]
    fn the_input_tile_grows_and_shrinks_with_the_text() {
        let mut app = tile_app();
        // 初始树保留旧的 4 行，布局系统跑过之后才按内容改。
        assert_eq!(input_tile_in(&app), 4.0 * ROW_HEIGHT);
        app.update();
        assert_eq!(input_tile_in(&app), input_tile_height(1));

        app.world_mut()
            .resource_mut::<Editor>()
            .set("one\ntwo\nthree");
        app.update();
        assert_eq!(input_tile_in(&app), input_tile_height(3));

        app.world_mut()
            .resource_mut::<Editor>()
            .set("one\ntwo\nthree\nfour\nfive");
        app.update();
        assert_eq!(input_tile_in(&app), input_tile_height(5));
        assert!(
            input_tile_in(&app) > input_tile_height(3),
            "换行后 tile 变高"
        );

        app.world_mut().resource_mut::<Editor>().set("one");
        app.update();
        assert_eq!(input_tile_in(&app), input_tile_height(1));
        assert!(
            input_tile_in(&app) < input_tile_height(5),
            "删回去 tile 变矮"
        );

        let many = (0..20).map(|line| line.to_string()).collect::<Vec<_>>().join("\n");
        app.world_mut().resource_mut::<Editor>().set(many);
        app.update();
        assert_eq!(input_tile_in(&app), input_tile_height(10), "20 行封顶 10 行");
    }

    #[test]
    fn growing_the_input_tile_keeps_the_right_column_exact() {
        use crate::layout::{PX_TO_METERS, Size, default_tree};
        use crate::niri::TileRect;

        let area = TileRect {
            center: Vec2::ZERO,
            size: Vec2::new(1.2, 0.8),
        };
        let bottom = |rect: &TileRect| rect.center.y - rect.size.y * 0.5;
        let top = |rect: &TileRect| rect.center.y + rect.size.y * 0.5;
        let mut tree = default_tree();
        let mut spacer_heights = Vec::new();
        for lines in [0usize, 1, 2, 4, 10, 99] {
            // 1 行和 0 行、10 行和 99 行的目标高度相同，`set_leaf_size`
            // 在尺寸没变时返回 false（和补全 tile 一样），这里不关心返回值。
            tree.set_leaf_size(PanelId::Input, Size::Fixed(input_tile_height(lines)));
            let rects = tree.rects(area);
            let input = rects
                .iter()
                .find(|(panel, _)| *panel == PanelId::Input)
                .expect("输入叶子")
                .1;
            let spacer = rects
                .iter()
                .find(|(panel, _)| *panel == PanelId::Spacer)
                .expect("顶部留白")
                .1;
            let hints = rects
                .iter()
                .find(|(panel, _)| *panel == PanelId::Hints)
                .expect("提示行")
                .1;
            let status = rects
                .iter()
                .find(|(panel, _)| *panel == PanelId::Status)
                .expect("状态行")
                .1;
            assert!(
                (input.size.y - input_tile_height(lines) * PX_TO_METERS).abs() < 1e-5,
                "{lines} 行时输入 tile 高度精确"
            );
            assert!((bottom(&input) - top(&hints)).abs() < 1e-5, "输入贴着提示行");
            assert!((bottom(&hints) - top(&status)).abs() < 1e-5, "提示行贴着状态行");
            assert!((bottom(&status) - bottom(&area)).abs() < 1e-5, "状态行贴底");
            let leaves: f32 = rects
                .iter()
                .map(|(_, rect)| rect.size.x * rect.size.y)
                .sum();
            assert!(
                (leaves - area.size.x * area.size.y).abs() < 1e-4,
                "叶子面积仍然精确铺满：{leaves}"
            );
            spacer_heights.push(spacer.size.y);
        }
        assert!(
            spacer_heights[3] < spacer_heights[0],
            "输入变高时由 Spacer 吸收差额：{:?}",
            spacer_heights
        );
        assert_eq!(
            spacer_heights[4], spacer_heights[5],
            "超过 10 行封顶后 Spacer 不再变"
        );

        // 补全 tile 打开（6 行）且输入 10 行：两个固定子一起长高，仍然铺满。
        tree.set_leaf_size(PanelId::Complete, Size::Fixed(6.0 * ROW_HEIGHT));
        tree.set_leaf_size(PanelId::Input, Size::Fixed(input_tile_height(10)));
        let leaves: f32 = tree
            .rects(area)
            .iter()
            .map(|(_, rect)| rect.size.x * rect.size.y)
            .sum();
        assert!(
            (leaves - area.size.x * area.size.y).abs() < 1e-4,
            "补全打开 + 10 行输入时叶子面积仍然铺满：{leaves}"
        );
    }

    // ---- Ctrl+V 粘贴 ----

    #[test]
    fn ctrl_v_pastes_text_at_the_cursor() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<Editor>()
            .set("say: ");
        let mut memory = MemoryClipboard::default();
        memory.write_text("hello\nworld").expect("memory");
        app.insert_resource(memory);

        with_modifier(
            &mut app,
            KeyCode::ControlLeft,
            Key::Character("v".into()),
            KeyCode::KeyV,
        );

        let editor = app.world().resource::<Editor>();
        assert_eq!(editor.text(), "say: hello\nworld", "多行文本原样插在光标处");
        assert_eq!(editor.cursor(), editor.text().len());
    }

    #[test]
    fn ctrl_v_with_an_empty_clipboard_shows_a_visible_error() {
        let mut app = test_app();
        app.init_resource::<AppStatus>();
        app.insert_resource(MemoryClipboard::default());
        app.world_mut().resource_mut::<Editor>().set("keep");

        with_modifier(
            &mut app,
            KeyCode::ControlLeft,
            Key::Character("v".into()),
            KeyCode::KeyV,
        );

        assert_eq!(text(&app), "keep", "空剪贴板不动输入");
        let status = app.world().resource::<AppStatus>();
        let item = status
            .0
            .iter()
            .find(|item| item.name == clipboard::PASTE_FAILED.name)
            .expect("空剪贴板要在状态行留一条错误");
        assert_eq!(item.text, clipboard::NO_CLIPBOARD_TEXT);
    }

    #[test]
    fn ctrl_v_without_any_clipboard_resource_still_reports() {
        let mut app = test_app();
        app.init_resource::<AppStatus>();
        app.world_mut().resource_mut::<Editor>().set("keep");

        with_modifier(
            &mut app,
            KeyCode::ControlLeft,
            Key::Character("v".into()),
            KeyCode::KeyV,
        );

        assert_eq!(text(&app), "keep");
        let status = app.world().resource::<AppStatus>();
        let item = status
            .0
            .iter()
            .find(|item| item.name == clipboard::PASTE_FAILED.name)
            .expect("没有剪贴板后端也要有可见错误");
        assert_eq!(item.text, clipboard::NO_CLIPBOARD);
    }

    #[test]
    fn a_successful_paste_clears_the_previous_error() {
        let mut app = test_app();
        app.init_resource::<AppStatus>();
        app.insert_resource(MemoryClipboard::default());
        let item = clipboard::PASTE_FAILED.name;
        with_modifier(
            &mut app,
            KeyCode::ControlLeft,
            Key::Character("v".into()),
            KeyCode::KeyV,
        );
        assert!(app.world().resource::<AppStatus>().0.iter().any(|found| found.name == item));

        app.world_mut()
            .resource_mut::<MemoryClipboard>()
            .write_text("now")
            .expect("memory");
        with_modifier(
            &mut app,
            KeyCode::ControlLeft,
            Key::Character("v".into()),
            KeyCode::KeyV,
        );
        assert_eq!(text(&app), "now");
        assert!(
            !app.world().resource::<AppStatus>().0.iter().any(|found| found.name == item),
            "成功粘贴清掉上一次的红色错误"
        );
    }

    #[test]
    fn enter_sends_a_multiline_prompt_with_its_newlines() {
        let (mut app, agent) = sending_app();
        app.world_mut()
            .resource_mut::<Editor>()
            .set("first\nsecond");
        send(&mut app, Key::Enter, KeyCode::Enter, None);
        assert_eq!(
            observed(&app).delivered,
            [(agent, "first\nsecond".to_owned(), DeliveryMode::Steer)],
            "换行只是文本的一部分，Enter 照样整段发送"
        );
        assert_eq!(app.world().resource::<Editor>().text(), "");
    }
}

#[cfg(test)]
mod completion_key_tests {
    use super::*;
    use crate::complete::{Candidate, Completion, CompletionKind, CompletionState};

    fn open_list(state: &mut CompletionState) {
        state.show(Completion::from_items(
            CompletionKind::Marker('#'),
            0,
            "",
            vec![
                Candidate {
                    text: "142".into(),
                    detail: "Crash on empty input".into(),
                },
                Candidate {
                    text: "143".into(),
                    detail: "Slow start".into(),
                },
            ],
        ));
    }

    #[test]
    fn down_moves_the_selection_and_does_not_touch_the_editor() {
        let mut state = CompletionState::default();
        open_list(&mut state);
        let mut editor = Editor::default();
        editor.insert("hello");
        assert!(completion_act(InputAction::Down, &mut state, &mut editor));
        assert_eq!(state.completion().unwrap().selected, 1);
        assert_eq!(editor.text, "hello", "↑↓ 只改选中项，不动输入");
    }

    #[test]
    fn tab_accepts_the_candidate() {
        let mut state = CompletionState::default();
        open_list(&mut state);
        let mut editor = Editor::default();
        assert!(completion_act(InputAction::Tab, &mut state, &mut editor));
        assert!(!state.is_open(), "接受后列表关掉");
        assert!(editor.text.contains("142"), "候选落进输入：{}", editor.text);
    }

    #[test]
    fn escape_dismisses_and_keeps_the_token_from_reopening() {
        let mut state = CompletionState::default();
        open_list(&mut state);
        let mut editor = Editor::default();
        assert!(completion_act(InputAction::Stop, &mut state, &mut editor));
        assert!(!state.is_open());
        assert_eq!(editor.text, "", "Esc 不改输入");
    }

    #[test]
    fn enter_on_an_inexact_list_accepts_instead_of_sending() {
        let mut state = CompletionState::default();
        open_list(&mut state);
        let mut editor = Editor::default();
        // 返回 true = 已吃掉这个动作，调用方不会再去 `act` 里发送。
        assert!(completion_act(InputAction::Send, &mut state, &mut editor));
        assert!(!state.is_open());
    }

    #[test]
    fn keys_outside_the_list_fall_through() {
        let mut state = CompletionState::default();
        let mut editor = Editor::default();
        assert!(!completion_act(InputAction::Left, &mut state, &mut editor));
    }

    #[test]
    fn a_closed_list_does_not_eat_tab_enter_escape_or_arrows() {
        let mut state = CompletionState::default();
        let mut editor = Editor::default();
        for action in [
            InputAction::Tab,
            InputAction::Send,
            InputAction::Stop,
            InputAction::Up,
            InputAction::Down,
        ] {
            assert!(
                !completion_act(action, &mut state, &mut editor),
                "列表关着时 {action:?} 应交回 act（发送/排队/停止/历史）"
            );
        }
    }

    #[test]
    fn enter_on_an_exact_list_closes_it_and_lets_the_send_through() {
        let mut state = CompletionState::default();
        state.show(Completion::from_items(
            CompletionKind::Command,
            0,
            "help",
            vec![Candidate {
                text: "help".into(),
                detail: "Show help".into(),
            }],
        ));
        let mut editor = Editor::default();
        editor.set("/help");
        // 返回 false = 调用方继续 `act(Send)`，精确命中时 Enter 就是发送。
        assert!(!completion_act(InputAction::Send, &mut state, &mut editor));
        assert!(!state.is_open(), "发送前列表关掉");
    }
}
