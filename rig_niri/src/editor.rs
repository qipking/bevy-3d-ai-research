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
//! - 有未回答的 free-text `Ask`（`free_text && !multiple`）时，Enter **不发送**
//!   消息，而是用输入框里的文本回答最老的那个（`answer_with_text`，上游
//!   rig-tui 的 `in_input`/`draft` 语义）；问题第一次成为最老的可作答问题时，
//!   它的 `initial` 会填进空输入框（只填一次，`seed_initial_answer`）。选项型
//!   问题（`free_text == false`）不抢 Enter，仍是普通发送；
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
//! 显示：`editor_rows` 把文本按输入面板列宽折行（用 `rig_tui::wrap::wrap_typed`，
//! 人打的文本不因首词后的空格悬挂缩进），每行一个 [`KeyedRow`]。
//!
//! 光标**不是额外字符**：旧实现往行里插一个 `▏`，渲染行宽会比折行宽多一列，
//! 写满一行的中间改字时行尾被裁；现在只把光标所在列的那个 grapheme 反色 +
//! 下划线，行尾（或折行丢掉的空白里）补一个反色空格，放不下就不补，所以任何
//! 情况下渲染行宽都不超过面板列数。
//!
//! 可见行数取 `min(折行行数, 输入面板实际能放的行数, [`MOST_LINES`])`：面板
//! 能放几行从 [`PanelAreas`] 的 [`PanelId::Input`] 区域的**像素高度**和
//! [`INPUT_ROW_HEIGHT`] 算，每个 Update 重算，窗口矮到放不下 [`MOST_LINES`]
//! 时滚动仍把光标行留在可见段里。[`sync_editor_tile`] 让输入 tile 的高度跟着
//! 行数走（最少 [`LEAST_ROWS`] 行、最多 [`MOST_LINES`] 行 + 面板上下 chrome），
//! 右列的总高仍由 Spacer 这个比例子吸收，精确不变。
//!
//! 外壳（F5）：输入面板根就是 feathers 输入框那一块——底色 [`crate::theme::INPUT_BG`]
//! （`textinput.bg`）、圆角 4px、左右内边距 3px；焦点在输入框时边框用
//! [`FOCUS_BORDER`]（`focus.ring`），焦点在别的面板时用 [`PANEL_BORDER`]
//! （普通边框 token）。行高 24px、字号 13px 与 feathers text input 一致；多行
//! 只是同一块输入框里的多行，行本身不带边框（见 [`sync_input_field`] /
//! [`sync_input_row_height`]）。
//!
//! 未接的行为：粘贴图片（Ctrl+V 现在只粘文本；图片见 [`crate::clipboard`]
//! 模块文档）、转写区滚动/选择（由 `pane.rs` 现有系统负责）。

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::{ButtonInput, ButtonState};
use bevy::prelude::*;
use bevy_feathers::constants::size as feathers_size;
use bevy_feathers::theme::{ThemeBorderColor, UiTheme};
use bevy_ui::{px, BorderRadius};
use rig_core::effect::EffectId;
use rig_ecs::prelude::{
    ActiveTurn, Decision, Decider, Deliver, DeliveryMode, Effect, ForAgent, Outcome, StopTurn,
};
use rig_harness::main_agent::ShownAgent;
use rig_harness::prelude::{Answer, Ask};
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
use crate::panels::{KeyedRow, PanelId, PanelRows, RowLayout};
use crate::theme::{FOCUS_BORDER, PANEL_BORDER, Theme};
use crate::transcript::{Row, row_hash};

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
/// 用的就是 `PanelInset::DEFAULT`；改 inset 时这里跟着变。改的只是输入框的
/// **水平**内边距（[`INPUT_PADDING_X`]），上下仍然是这里的 pad。
pub const INPUT_CHROME: f32 = 2.0
    * (crate::frame::PanelInset::DEFAULT.pad as f32
        + crate::frame::PanelInset::DEFAULT.border as f32);

/// 把 feathers 的像素尺寸常量（`Val::Px`）取成 `f32`。
///
/// 输入框的行高/字号直接取 feathers 的常量，不在本文件重写数值：feathers 改
/// 尺寸时这里跟着变。
const fn pixels(value: bevy_ui::Val) -> f32 {
    match value {
        bevy_ui::Val::Px(px) => px,
        _ => panic!("feathers 的尺寸常量应当是像素"),
    }
}

/// 把 feathers 的字号常量（`FontSize::Px`）取成 `f32`。
const fn font_pixels(size: bevy_text::FontSize) -> f32 {
    match size {
        bevy_text::FontSize::Px(px) => px,
        _ => panic!("feathers 的字号常量应当是像素"),
    }
}

/// 输入框水平内边距（左/右）。
///
/// 来自 `bevy_feathers::controls::text_input` 的 `FeathersTextInputContainer`：
/// `padding.left/right = 3px`。feathers 的间距还没有 design token，所以这里
/// 镜像那一个值；测试钉住它和上游一致。
pub const INPUT_PADDING_X: f32 = 3.0;

/// 输入框的行高：feathers `constants::size::ROW_HEIGHT`（24px）。
///
/// 这也是 `FeathersTextInputContainer` 的高度；多行输入时每一行都按它画，
/// 整块仍然是一个输入框（行本身没有边框）。
pub const INPUT_ROW_HEIGHT: f32 = pixels(feathers_size::ROW_HEIGHT);

/// 输入框的字号：feathers `constants::size::COMPACT_FONT`（13px，text input 用）。
pub const INPUT_FONT_SIZE: f32 = font_pixels(feathers_size::COMPACT_FONT);

/// 输入框的圆角：feathers text input scene 的 `BorderRadius::all(px(4.0))`。
///
/// 和面板一样先集中到 [`crate::theme::PANEL_RADIUS`]（feathers 还没有圆角 token）。
pub const INPUT_RADIUS: f32 = crate::theme::PANEL_RADIUS;

/// 输入 tile 的高度（UI 像素）。
///
/// `max(LEAST_ROWS * INPUT_ROW_HEIGHT, clamp(行数, 1, MOST_LINES) * INPUT_ROW_HEIGHT + INPUT_CHROME)`。
/// 超过 [`MOST_LINES`] 后 tile 不再长高，
/// 多出来的行由 [`EditorLayout`] 滚动露出（上游 `fit_rows` 的
/// `clamp(1, MOST_LINES) + 2`，我们的 `+2` 是面板 chrome 而不是两行边框）。
pub fn input_tile_height(lines: usize) -> f32 {
    let wanted = lines.clamp(1, MOST_LINES) as f32 * INPUT_ROW_HEIGHT + INPUT_CHROME;
    wanted.max(LEAST_ROWS * INPUT_ROW_HEIGHT)
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
    /// 可见几行：`min(rows.len(), 面板能放的行数, MOST_LINES)`。
    pub visible: usize,
    /// 折行用的面板列数（含第一行两列提示符）。
    ///
    /// 光标在行尾补空格、面板窄到连提示符都放不下时裁行，都靠它保证
    /// **渲染行宽 ≤ 面板列数**。布局还没跑过（`Default`）时是 0。
    width: usize,
}

impl EditorLayout {
    /// 光标在可见段里的下标；光标不在可见段时 `None`。
    pub fn cursor_in_view(&self) -> Option<usize> {
        (self.cursor_row >= self.top && self.cursor_row < self.top.saturating_add(self.visible))
            .then(|| self.cursor_row - self.top)
    }

    /// 这一帧要显示的行：`top..top+visible`，第一行带 `› `、续行带两个空格，
    /// 光标所在行把光标列上的 grapheme 反色（不插入字符，见 [`with_cursor`]）。
    ///
    /// 面板窄到连提示符（两列）都放不下时，行会被裁到面板列数以内——正常
    /// 宽度下折行与光标样式已经保证不越界，这里只是最后一道保险。
    pub fn visible_rows(&self, theme: &Theme) -> Vec<Line<'static>> {
        if self.rows.is_empty() {
            // 布局还没跑过（例如系统被跳过）时也至少画出提示符和光标。
            let line = Line::from(vec![prompt_span(true, theme), end_cursor_span()]);
            return vec![match self.width {
                0 => line,
                width => fit_to_width(line, width),
            }];
        }
        if self.visible == 0 {
            return Vec::new();
        }
        let start = self.top.min(self.rows.len().saturating_sub(1));
        let end = start.saturating_add(self.visible).min(self.rows.len());
        (start..end)
            .map(|index| {
                let mut row = self.rows[index].clone();
                if index == self.cursor_row {
                    row = with_cursor(row, self.cursor_column, self.body_width());
                }
                let mut spans = vec![prompt_span(index == 0, theme)];
                spans.extend(row.spans);
                fit_to_width(
                    Line {
                        spans,
                        style: row.style,
                        alignment: row.alignment,
                    },
                    self.width,
                )
            })
            .collect()
    }

    /// 正文（不含提示符两列）的显示列数；与 [`Editor::layout`] 的折行宽度一致。
    fn body_width(&self) -> usize {
        self.width.max(3) - 2
    }
}

/// 旧实现插在行里的光标字符 `▏`。
///
/// 现在光标**不再**多占一列：它把光标处的 grapheme 反色（行尾补一个反色
/// 空格），所以渲染行里不会出现这个字符。保留常量是为了兼容旧引用，测试
/// 也用它断言"没有额外插入字符"。
pub const CURSOR: &str = "▏";

/// 光标的样式：反色 + 下划线，前景仍是旧的浅青色。
pub fn cursor_style() -> Style {
    Style::default()
        .fg(RColor::LightCyan)
        .add_modifier(Modifier::REVERSED | Modifier::UNDERLINED)
}

/// 行尾（或折行丢掉的空白里）补出来的光标格：一个反色的空格。
fn end_cursor_span() -> Span<'static> {
    Span::styled(" ", cursor_style())
}

/// 行首提示符：第一行 `› `（绿色粗体），续行两个空格（dim，和上游一致）。
fn prompt_span(first: bool, theme: &Theme) -> Span<'static> {
    match first {
        true => ratatui_span("› ", theme.prompt),
        false => ratatui_span("  ", theme.dim),
    }
}

/// 把光标画在行内第 `column` 个显示列上：反色该列上的 grapheme，**不插入
/// 任何额外字符**。
///
/// 旧实现插入一个 `▏`（宽 1 列），渲染行宽 = 折行宽 + 1，写满一行时行尾
/// 会被面板裁掉。现在反色覆盖这一列的 grapheme，宽字/emoji 也是原地反色，
/// 行宽不变。
///
/// 光标可能停在行尾或折行时被丢掉的空白里：先补空格到 `column`，再反色
/// 一个空格。补这一格的前提是正文仍在 `body_width` 列以内；放不下就只停在
/// 行尾位置，绝不撑宽（正常情况下 `layout` 会把写满一行的行尾光标挪到下一
/// 行，所以只有空白/防御路径会走到这里）。
fn with_cursor(line: Line<'static>, column: usize, body_width: usize) -> Line<'static> {
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
                // 一个 grapheme（含 ZWJ emoji / 组合字符）整体反色，宽度不变。
                spans.push(Span::styled(
                    grapheme.to_owned(),
                    span.style.patch(cursor_style()),
                ));
                placed = true;
            } else {
                text.push_str(grapheme);
            }
            seen = seen.saturating_add(width);
        }
        if !text.is_empty() {
            spans.push(Span::styled(text, span.style));
        }
    }
    if !placed {
        let padding = column.saturating_sub(seen);
        if seen.saturating_add(padding).saturating_add(1) <= body_width {
            if padding > 0 {
                spans.push(Span::styled(" ".repeat(padding), Style::default()));
            }
            spans.push(end_cursor_span());
        }
    }
    Line {
        spans,
        style: line.style,
        alignment: line.alignment,
    }
}

/// 把一行裁到 `width` 列以内：只保留完整 grapheme，宽字/emoji 不会被切成
/// 半个。
///
/// 正常宽度下折行（正文 ≤ 面板列数 - 2）和光标（原地反色，最多补一个空格）
/// 已经保证 `2 + 正文 <= 面板列数`；只有面板窄到连提示符都放不下时
/// （[`Editor::layout`] 把正文宽度兜底成 1 列）才会真的裁。这是"渲染行宽
/// 永远不超过面板列数"的最后一道保险。
fn fit_to_width(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let mut spans = Vec::new();
    let mut used = 0usize;
    'spans: for span in line.spans {
        let mut text = String::new();
        for grapheme in span.content.graphemes(true) {
            let cells = UnicodeWidthStr::width(grapheme);
            if used.saturating_add(cells) > width {
                if !text.is_empty() {
                    spans.push(Span::styled(text, span.style));
                }
                break 'spans;
            }
            used = used.saturating_add(cells);
            text.push_str(grapheme);
        }
        if !text.is_empty() {
            spans.push(Span::styled(text, span.style));
        }
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

    /// 按输入面板的 `width` 个文字列折行，并算出光标落在第几行第几列；
    /// 可见行数最多 [`MOST_LINES`] 行。
    ///
    /// 折行用 [`rig_tui::wrap::wrap_typed`]——和转写 `transcript::wrap_rows`
    /// 的 [`rig_tui::wrap::wrap`] 是同一个折行器，但输入框里是人打的文本：
    /// 首词后面两个以上空格更可能是手滑，不该像代码/列表那样让续行悬挂
    /// 缩进（上游 `rig-tui editor.rs` 也分这两个入口）。第一行前面的 `› `
    /// 也占两列，所以正文宽度是 `width - 2`，与上游 `layout(width)` 一致。
    /// 控制字符按上游换成同样字节数的空格（tab 因此显示成一个空格）；换行
    /// 按 `\n` 拆成逻辑行。光标是 grapheme 边界上的字节偏移，折行时被丢掉
    /// 的换行处空格（[`rig_tui::wrap::RowWrap::gap`]）也按上游的规则映射。
    ///
    /// 返回的 [`EditorLayout`] 不含提示符：`cursor_column` 是行内显示列，
    /// 提示符两列由 [`EditorLayout::visible_rows`] 加。
    pub fn layout(&self, width: usize) -> EditorLayout {
        self.layout_within(width, MOST_LINES)
    }

    /// [`Self::layout`]，可见行数还受输入面板这一帧实际能放的行数
    /// （[`PanelAreas`] 的 `area.rows`）限制：`min(折行行数, max_rows,
    /// MOST_LINES)`。
    ///
    /// 面板变矮时 `top` 仍按光标滚动，所以光标行一定在 `top..top+visible`
    /// 里；`max_rows == 0` 时 `visible == 0`，一行都不写。
    fn layout_within(&self, width: usize, max_rows: usize) -> EditorLayout {
        let panel_width = width;
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
            for (row, joint) in rig_tui::wrap::wrap_typed(&Line::from(shown.as_str()), width) {
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
        let visible = rows.len().min(max_rows).min(MOST_LINES);
        let top = match visible {
            0 => 0,
            visible => (cursor.0 + 1).saturating_sub(visible),
        };
        EditorLayout {
            rows,
            cursor_row: cursor.0,
            cursor_column: cursor.1,
            top,
            visible,
            width: panel_width,
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
    /// 用确认面板里第一个问题的第 `number` 个选项回答它（Alt+1..9，
    /// 从 1 数）；越界什么也不做。
    AnswerInline(usize),
    /// 拒绝确认面板里第一个问题（Alt+0；Esc 也走同一条拒绝路径）。
    DismissInline,
}

// ---------------------------------------------------------------------------
// 回答确认面板里的 `Ask`：点击与键盘共用的唯一落点
// ---------------------------------------------------------------------------

/// 回答一个待确认的 `Ask`，用它的选项 `choice`（[`rig_harness::ask::Choice::id`]）。
///
/// **这是点击与键盘共用的唯一作答落点。** 确认行的点击处理器
/// （`rig_harness_in_3d::kernel::answer_ask` 观察者，读
/// [`crate::transcript::AskChoice`]）和键盘的 Alt+1..9（[`Act::AnswerInline`]）
/// 都调用它，所以两条路产生完全相同的 rig-harness 效果：在 Ask 的
/// [`Decision`] 上以 [`Decider::Person`] 记下 `Answer::choice(id).outcome()`
/// ——和点击路径原来的构造逐字相同，没有另造一套。
///
/// 返回这次决定是否生效：Ask 没有 `Decision`、已经被回答过（第一次决定赢）
/// 或实体没了都是 `false`。
pub fn answer_ask(ask: Entity, choice: &str, decisions: &mut Query<&mut Decision>) -> bool {
    let Ok(mut decision) = decisions.get_mut(ask) else {
        return false;
    };
    decision.decide(Decider::Person, Answer::choice(choice).outcome())
}

/// 拒绝一个待确认的 `Ask`（Alt+0 与 Esc）。
///
/// 用的是 rig-harness 的 `Pick::person(ask, None)` 分支同一个结论
/// （`rig-harness/src/pick.rs`：`Outcome::refused("The user closed the
/// question.")`），决策者同样是 [`Decider::Person`]，只是不经过 `Pick`
/// 事件：本 crate 的点击路径本来就直接写 `Decision`，拒绝走同一条落点才
/// 不会有第二套语义（也不依赖 `PickPlugin` 是否装在本 app 里）。
/// 返回这次决定是否生效。
pub fn refuse_ask(ask: Entity, decisions: &mut Query<&mut Decision>) -> bool {
    let Ok(mut decision) = decisions.get_mut(ask) else {
        return false;
    };
    decision.decide(
        Decider::Person,
        Outcome::refused("The user closed the question."),
    )
}

/// 确认面板里最老的那个未回答 `Ask`：键盘作答的对象。
///
/// 顺序和 [`crate::batch::confirm_rows`] 一致——只算当前工作区可见的
/// （[`crate::keys::ask_is_visible`]），按内核效应 id 从老到新，还没登记
/// 效应 id 的排最后。确认面板每个 `Ask` 一块，Alt+n 答的是第一块的
/// 第 n 个选项；点击则可以点任何一块的任何一行。
fn first_pending_ask(
    asks: &Query<
        (Entity, &Ask, Option<&Effect>, Option<&ForAgent>, Has<AnswerSeeded>),
        Without<Outcome>,
    >,
    shown: Option<Entity>,
) -> Option<(Entity, Ask)> {
    asks.iter()
        .filter(|(_, _, _, owner, _)| {
            crate::keys::ask_is_visible(shown, owner.map(|ForAgent(owner)| *owner))
        })
        .min_by_key(|(_, _, effect, _, _)| {
            effect
                .map(Effect::id)
                .unwrap_or(EffectId::from_raw(u64::MAX))
        })
        .map(|(entity, ask, ..)| (entity, ask.clone()))
}

/// 输入框里的 Enter 该回答的那个 `Ask`：**最老的、能靠打字作答的问题**。
///
/// 范围比 [`first_pending_ask`] 窄：只算 `free_text`、不是多选（`!multiple`）
/// 且不是内联（`!inline`）的问题——多选的 free-text 是"选项 + 备注"，本前端
/// 还没做（见 `docs/status/REMAINING.md` §5 的"还没做"）；`Ask::inline`
/// 明确"用户下一行是消息、不是它的答案"（上游 `ask.rs:78-82`），所以不能抢
/// Enter。可见性、FIFO（内核效应 id 从老到新，没登记 id 的排最后）与确认
/// tile / Alt+1..9 完全同一口径，所以自由文本问题也按先来先答。
///
/// 上游 rig-tui 用 `PickerInput.draft`/`in_input()` 表示"这个问题拿输入框
/// 作答"；本前端没有 picker 队列，改成"当前最老的 free-text 问题"直接拿
/// Enter。
fn first_free_text_ask(
    asks: &Query<
        (Entity, &Ask, Option<&Effect>, Option<&ForAgent>, Has<AnswerSeeded>),
        Without<Outcome>,
    >,
    shown: Option<Entity>,
) -> Option<(Entity, Ask, bool)> {
    asks.iter()
        .filter(|(_, ask, _, owner, _)| {
            ask.free_text
                && !ask.multiple
                && !ask.inline
                && crate::keys::ask_is_visible(shown, owner.map(|ForAgent(owner)| *owner))
        })
        .min_by_key(|(_, _, effect, _, _)| {
            effect
                .map(Effect::id)
                .unwrap_or(EffectId::from_raw(u64::MAX))
        })
        .map(|(entity, ask, _, _, seeded)| (entity, ask.clone(), seeded))
}

/// 给"已经轮到过输入框"的 free-text 问题打的标记：`initial` 只填一次。
///
/// 挂在 `Ask` 实体上（而不是记在资源里）：问题被回答、撤掉或 despawn 时
/// 标记跟着走，不会留下指向死实体的表。第一次成为最老的可作答问题时由
/// `seed_initial_answer` 插入。
///
/// 公开只是因为 [`keys`] 的 [`Ask`] 查询元组里有它（Bevy 的
/// `SystemParam` 存在 `pub fn` 的签名里）；本 crate 之外没有别的用途。
#[derive(Component)]
pub struct AnswerSeeded;

/// 把 free-text 问题的 `initial` 填进空输入框；每个问题只填一次。
///
/// 触发点是它**第一次**成为最老的、能靠打字作答的问题（和 Enter 回答用
/// 同一个 [`first_free_text_ask`]）。挂 [`AnswerSeeded`] 的时机是"第一次
/// 轮到它"，不管这一帧输入框是不是空的：用户自己写着的文本不动，之后
/// 用户清空输入也不会被 `initial` 再回填（"用户改过就不要再覆盖"）。上游
/// `pick::show_first` 也是问题一出现就把 `initial` 交给输入框，只填一次。
fn seed_initial_answer(
    asks: &Query<
        (Entity, &Ask, Option<&Effect>, Option<&ForAgent>, Has<AnswerSeeded>),
        Without<Outcome>,
    >,
    shown: Option<Entity>,
    editor: &mut Editor,
    commands: &mut Commands,
) {
    let Some((ask, question, seeded)) = first_free_text_ask(asks, shown) else {
        return;
    };
    if seeded {
        return;
    }
    // 先记"已经轮到过它"：即使输入框当时非空，以后也不再回填。
    commands.entity(ask).try_insert(AnswerSeeded);
    let Some(initial) = question.initial else {
        return;
    };
    if editor.is_empty() {
        editor.set(initial);
    }
}

/// 用输入框里打好的文本回答 `ask`：上游 `Chosen::text(input.take_answer())`
/// 在本前端的落点，以 [`Decider::Person`] 记下 `Answer::text(text)`。
///
/// `take` 取走文本、不进历史（和上游 `take_answer` 一样：答案不是要发送的
/// 消息）。先看 [`Decision::is_decided`] 再取文本：问题已经被点击 / Alt+n /
/// Esc 答掉时，不能把用户刚打的字吞掉。返回这次决定是否生效。
fn answer_with_text(
    ask: Entity,
    editor: &mut Editor,
    decisions: &mut Query<&mut Decision>,
) -> bool {
    let Ok(mut decision) = decisions.get_mut(ask) else {
        return false;
    };
    if decision.is_decided() {
        return false;
    }
    let text = editor.take();
    decision.decide(Decider::Person, Answer::text(text).outcome())
}

/// Alt+n：用当前确认问题的第 `number` 个选项回答；越界什么也不做。
fn answer_shown_ask(
    number: usize,
    asks: &Query<
        (Entity, &Ask, Option<&Effect>, Option<&ForAgent>, Has<AnswerSeeded>),
        Without<Outcome>,
    >,
    shown: Option<Entity>,
    decisions: &mut Query<&mut Decision>,
) -> bool {
    let Some((ask, question)) = first_pending_ask(asks, shown) else {
        return false;
    };
    let Some(choice) = number
        .checked_sub(1)
        .and_then(|index| question.choices.get(index))
    else {
        return false;
    };
    answer_ask(ask, &choice.id, decisions)
}

/// Alt+0 / Esc：拒绝当前确认问题（第一个未回答的 `Ask`）。
fn refuse_shown_ask(
    asks: &Query<
        (Entity, &Ask, Option<&Effect>, Option<&ForAgent>, Has<AnswerSeeded>),
        Without<Outcome>,
    >,
    shown: Option<Entity>,
    decisions: &mut Query<&mut Decision>,
) -> bool {
    first_pending_ask(asks, shown)
        .is_some_and(|(ask, _)| refuse_ask(ask, decisions))
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
        // 滚动/选择由别的模块负责；内联回答走 Act：Alt+n 选第 n 个选项，
        // Alt+0 拒绝。真正的作答落点是 [`answer_ask`] / [`refuse_ask`]，
        // 和点击确认行走同一条路。
        Do::ScrollUp
        | Do::ScrollDown
        | Do::PageUp
        | Do::PageDown
        | Do::SelectUp
        | Do::SelectDown
        | Do::Deselect => {}
        // F10：Ctrl+Shift+C 的复制由 `clipboard::copy_selection_keys` 读按键
        // 完成（绑定是 Global，焦点在转写根上时也走这里）。输入系统只把动作
        // 当成已消费，**不能**落到 Clear。
        Do::CopySelection => {}
        Do::AnswerInline(number) => return Some(Act::AnswerInline(usize::from(number))),
        Do::DismissInline => return Some(Act::DismissInline),
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
///   [`clipboard::PASTE_FAILED`]（和 `Copied` 同一套状态反馈）；
/// * 确认面板里有未回答的 `Ask` 时，Esc 先拒绝它、Alt+1..9 用第一个问题的
///   第 n 个选项回答、Alt+0 拒绝——都走 [`answer_ask`] / [`refuse_ask`]，
///   和点击确认行同一条落点；没有问题时的 Esc 行为不变；
/// * 有能靠打字作答的 free-text 问题（`free_text && !multiple && !inline`）
///   时，Enter 改成回答最老的那个（`first_free_text_ask` →
///   `answer_with_text`），不再发送消息；`Ask::inline` 的下一行仍是消息；
///   输入为空时 Enter 什么也不做。它第一次成为最老的
///   可作答问题时，`seed_initial_answer` 把 `initial` 填进空输入框
///   （只填一次）。
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
    asks: Query<
        (Entity, &Ask, Option<&Effect>, Option<&ForAgent>, Has<AnswerSeeded>),
        Without<Outcome>,
    >,
    mut decisions: Query<&mut Decision>,
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
    // `KeyPressed.agent` 和动作分支里的 `agent` 是同一事实来源：显示中的
    // `ShownAgent`（应用层切工作区时更新），没有就是 `None`。
    let shown_agent = shown.as_deref().and_then(|shown| shown.0);
    // free-text 问题第一次排到"最老的可作答问题"时，把它的 `initial`
    // 填进空输入框（每个问题只填一次，见 [`seed_initial_answer`]）。放在
    // 读按键之前，所以同一帧按下的 Enter 看到的就是填好的文本。
    seed_initial_answer(&asks, shown_agent, &mut editor, &mut commands);
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
                    agent: shown_agent,
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
                        // 要送的 agent 是 `ShownAgent`：应用层在切聚焦工作区时更新它。
                        let agent = shown.as_deref().and_then(|shown| shown.0);
                        // 确认面板里有未回答的问题时，Esc 先拒绝它（上游
                        // `input.rs::act` 的 `Do::Stop, Some(ask)` 分支）：
                        // 不关补全、不停 turn；没有问题才走下面的原有行为。
                        if matches!(action, InputAction::Stop)
                            && let Some((ask, _)) = first_pending_ask(&asks, agent)
                        {
                            refuse_ask(ask, &mut decisions);
                            continue;
                        }
                        // 补全列表开着时，↑↓/Tab/Enter/Esc 先归列表（上游 accept_completion）。
                        if let Some(state) = completion.as_mut() {
                            if completion_act(action, state, &mut editor) {
                                continue;
                            }
                        }
                        // 输入框作答（上游 `pick.rs` 的 `in_input`/`draft`）：
                        // 有 free-text 问题、不是多选、也不是 inline 时，Enter
                        // 归它，不再发送消息；有多个就答最老的（和确认 tile /
                        // Alt+1..9 同一 FIFO）。输入为空时 Enter 不作答、也不
                        // 发送——什么也不做。选项型（`free_text == false`）和
                        // `Ask::inline` 不进这个分支，Enter 照旧发送（上游：
                        // "the user's next line is a message, not its answer"）。
                        if matches!(action, InputAction::Send)
                            && let Some((ask, _, _)) = first_free_text_ask(&asks, agent)
                        {
                            if !editor.text.trim().is_empty() {
                                answer_with_text(ask, &mut editor, &mut decisions);
                            }
                            continue;
                        }
                        let busy = agent.is_some_and(|agent| entering.contains(agent));
                        match act(action, &mut editor, busy) {
                            // 内联作答（Alt+1..9）与拒绝（Alt+0）走和点击确认行
                            // 完全相同的 [`answer_ask`] / [`refuse_ask`]。
                            Some(Act::AnswerInline(number)) => {
                                answer_shown_ask(number, &asks, agent, &mut decisions);
                            }
                            Some(Act::DismissInline) => {
                                refuse_shown_ask(&asks, agent, &mut decisions);
                            }
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
                        agent: shown_agent,
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

/// 输入框这一帧**能放几行文字**：按输入面板的实际像素高度和
/// [`INPUT_ROW_HEIGHT`] 算，窗口矮到放不下时滚动仍把光标行留在可见段里。
///
/// 有精确 [`crate::frame::PanelArea::size`] 时用 `(size.y - INPUT_CHROME) / INPUT_ROW_HEIGHT`：
/// 输入行是 24px（feathers text input 的行高），不能再拿面板区域按 18px 行算出的
/// `rows`。只装 `EditorPlugin` 的最小 App / 测试里 `size` 还是 0 时退回
/// `area.rows`；根/区域还没建好（第一帧）时用 [`MOST_LINES`] 兜底，和
/// [`input_width`] 的 97 兜底同一套规则。
pub fn input_rows(roots: Option<&PanelRoots>, areas: Option<&PanelAreas>) -> usize {
    roots
        .and_then(|roots| roots.entity(PanelId::Input))
        .and_then(|root| areas.and_then(|areas| areas.get(root)))
        .map_or(MOST_LINES, |area| match area.size.y > 0.0 {
            true => {
                let usable = (area.size.y - INPUT_CHROME).max(0.0);
                (usable / INPUT_ROW_HEIGHT).floor().max(1.0) as usize
            }
            false => usize::from(area.rows),
        })
}

/// 按输入面板的列宽和高度算这一帧的 [`EditorLayout`]（折行、光标行列、
/// 滚动 top、可见行数）。
///
/// 排在 [`keys`] 之后：同一帧敲进去的字、Shift+Enter 的新行立刻参与布局。
/// 宽度和能放的行数取**上一帧** `lay_out_frame` 写下的 `PanelAreas`：右列
/// 宽度不随输入框高度变，高度也只会在窗口压扁 Fixed 高度时变，所以用上一帧
/// 的值不会抖；`sync_editor_tile` 能在 `lay_out_frame` 之前用同一份行数改树。
/// 每个 Update 都重算，面板尺寸变了不需要额外事件。
pub fn layout_editor(
    editor: Res<Editor>,
    roots: Option<Res<PanelRoots>>,
    areas: Option<Res<PanelAreas>>,
    mut layout: ResMut<EditorLayout>,
) {
    let width = input_width(roots.as_deref(), areas.as_deref());
    let max_rows = input_rows(roots.as_deref(), areas.as_deref());
    let next = editor.layout_within(width, max_rows);
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
/// 行来自 [`EditorLayout`]（宽度/折行/滚动/可见行数已在 [`layout_editor`]
/// 里算好）；光标是光标行里被反色的 grapheme，不再插入额外字符。超过
/// [`MOST_LINES`] 或面板能放的行数时只写 `top..top+visible`，光标行一定在
/// 可见段里。
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

/// 输入框外壳的标记：整块输入只有一个（面板根），多行时行不各自成框。
///
/// 测试用它数"边框节点"：不管输入里有几行，带这个标记的实体只有一个。
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputField;

/// 输入框是否拿到键盘焦点：`KeyFocus(None)` 就是输入框（见 [`KeyFocus`] 的约定）。
///
/// 没有 [`KeyFocus`] 资源的最小 App 按"常驻焦点在输入框"处理，和
/// `frame::default_screen_panels` 把输入框当 composer 的假设一致。
fn input_is_focused(focus: Option<&KeyFocus>) -> bool {
    focus.map_or(true, |focus| focus.0.is_none())
}

/// 输入框外壳：借 feathers `text_input` 的外观（底色/圆角/水平内边距/焦点环）。
///
/// * 底色/圆角在生成面板时就来自 feathers token（[`crate::theme::INPUT_BG`]、
///   [`INPUT_RADIUS`]），这里只补水平内边距和边框的焦点态；
/// * 焦点在输入框（[`KeyFocus`] 为 `None`）时边框用 [`FOCUS_BORDER`]
///   （feathers `focus.ring`），焦点在别的面板时用 [`PANEL_BORDER`]
///   （feathers `border.default`）。feathers 的 `FocusWithinIndicator` 是加
///   `Outline`；3D 镜像只画 `BorderColor`，所以这里换边框色而不是加 Outline。
///
/// 面板根的 [`ThemeBorderColor`] 会被摘掉：`frame::sync_panel_chrome` 每帧把
/// 带这个组件的边框刷回生成时的 token（输入框生成时是 focus ring），不摘的话
/// 焦点态永远换不掉。底色仍是 token（`ThemeBackgroundColor` 不动）。
///
/// 每个工作区都有自己的输入框：只有**聚焦工作区**的那个（全局
/// [`PanelRoots`] 里的 [`PanelId::Input`]）能是焦点态，其它工作区的输入框一律
/// 普通边框——切工作区后不会留下一圈旧 focus ring。
pub fn sync_input_field(
    mut commands: Commands,
    roots: Option<Res<PanelRoots>>,
    focus: Option<Res<KeyFocus>>,
    theme: Option<Res<UiTheme>>,
    mut panels: Query<(
        Entity,
        Option<&PanelId>,
        &mut Node,
        &mut BorderColor,
        Option<&ThemeBorderColor>,
    )>,
) {
    // 聚焦工作区的输入框；没有根表的最小 App 按"只有这一个"处理。
    let focused_input = roots
        .as_deref()
        .and_then(|roots| roots.entity(PanelId::Input));
    let keyboard_focused = input_is_focused(focus.as_deref());
    let theme = crate::theme::ui_theme_or_default(theme.as_deref());
    let padding = px(INPUT_PADDING_X);
    let radius = BorderRadius::all(px(INPUT_RADIUS));
    for (entity, id, mut node, mut border, themed_border) in &mut panels {
        // 真实面板根带 `PanelId::Input`；最小测试 App 的根可能没打标签，
        // 这时只认全局根表里的那个实体。
        let is_input =
            id.is_some_and(|id| *id == PanelId::Input) || focused_input == Some(entity);
        if !is_input {
            continue;
        }
        if themed_border.is_some() {
            commands.entity(entity).remove::<ThemeBorderColor>();
        }
        commands.entity(entity).insert_if_new(InputField);
        if node.padding.left != padding {
            node.padding.left = padding;
        }
        if node.padding.right != padding {
            node.padding.right = padding;
        }
        if node.border_radius != radius {
            node.border_radius = radius;
        }
        let focused = keyboard_focused && focused_input.map_or(true, |root| root == entity);
        let token = match focused {
            true => FOCUS_BORDER,
            false => PANEL_BORDER,
        };
        let color = theme.color(&token);
        if border.top != color {
            border.set_all(color);
        }
    }
}

/// 输入行的高度换成 feathers text input 的行高（[`INPUT_ROW_HEIGHT`]）。
///
/// 行是 `panels::sync_panels` 生成的（行模型与其它面板共用，行高写死
/// `transcript::ROW_HEIGHT`），编辑器不改行模型，只把这批输入行的 `Node`
/// 高度调成输入框的 24px；文字在行内垂直居中（行节点本来就 `align_items:
/// Center`）。别的面板的行不受影响。
///
/// 排在 [`crate::FeathersSystems::Render`] 之后：`sync_panels` 这一帧刚 spawn
/// 的行也能在同一帧拿到正确高度（延迟命令由调度器在这里之前 apply）。
pub fn sync_input_row_height(mut rows: Query<(&RowLayout, &mut Node)>) {
    let height = px(INPUT_ROW_HEIGHT);
    for (layout, mut node) in &mut rows {
        if layout.panel == PanelId::Input && node.height != height {
            node.height = height;
        }
    }
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
                // 确认面板里有未回答的 Ask 时才启用 Alt+1..9/Alt+0；排在
                // `keys` 前面，所以按键到来时开关已经生效（问题由内核在
                // AgentPass 里 spawn 时可能晚一帧，和上游按帧更新一致）。
                crate::keys::arm_inline_keys,
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
        )
        // F5：输入框外壳。排在整个 `Render`（含 `frame::sync_panel_chrome` 和
        // `panels::sync_panels`）之后，所以同一帧里既能盖掉面板 chrome 的边框
        // token，也能给刚 spawn 的输入行改高度。
        .add_systems(
            Update,
            (sync_input_field, sync_input_row_height)
                .chain()
                .after(crate::FeathersSystems::Render),
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
    use crate::panels::PanelsPlugin;
    use crate::transcript::ROW_HEIGHT;
    use bevy::ecs::event::EntityEvent;
    use bevy::text::FontSource;
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

    /// 一行里所有 span 的纯文本（光标只改样式，所以这也是渲染出来的文本）。
    fn plain(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    /// 光标标记：第一个反色 span 的起始显示列（含提示符）+ 文本。
    ///
    /// 光标不再插入 `▏`，而是把光标列的 grapheme 反色；行尾/空白处是反色的
    /// 一个空格。返回 `None` 表示这一行没有光标。
    fn cursor_marker(line: &Line<'_>) -> Option<(usize, String)> {
        let mut column = 0usize;
        for span in &line.spans {
            if span.style.add_modifier.contains(Modifier::REVERSED) {
                return Some((column, span.content.to_string()));
            }
            column += span.width();
        }
        None
    }

    /// 只装布局/产行系统的最小 App：`width` 是输入面板的文字列数，
    /// `rows` 是面板这一帧能放的行数（和 `PanelArea.rows` 一样已扣 chrome）。
    fn layout_app_with(width: u16, rows: u16) -> App {
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
                rows,
                ..Default::default()
            },
        );
        app.insert_resource(roots)
            .insert_resource(areas)
            .add_systems(Update, (layout_editor, editor_rows).chain());
        app
    }

    /// 只装布局/产行系统的最小 App；面板能放 [`MOST_LINES`] 行。
    fn layout_app(width: u16) -> App {
        layout_app_with(width, MOST_LINES as u16)
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
        app.add_systems(
            Update,
            (crate::keys::arm_inline_keys, keys).chain(),
        );
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
    fn ctrl_c_still_clears_but_ctrl_shift_c_does_not() {
        let mut app = test_app();
        app.world_mut().resource_mut::<Editor>().set("hello");
        with_modifier(
            &mut app,
            KeyCode::ControlLeft,
            Key::Character("c".into()),
            KeyCode::KeyC,
        );
        assert_eq!(text(&app), "", "Ctrl+C 仍是 Clear");

        // Ctrl+Shift+C：字符键的 Shift 由大写的 C 携带，命中的是
        // `CopySelection`（动作由独立系统执行），不是 Clear。
        app.world_mut().resource_mut::<Editor>().set("hello");
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ShiftLeft);
        with_modifier(
            &mut app,
            KeyCode::ControlLeft,
            Key::Character("C".into()),
            KeyCode::KeyC,
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ShiftLeft);
        assert_eq!(text(&app), "hello", "Ctrl+Shift+C 不能清空输入");
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

    // ---- 确认面板的键盘作答 / 拒绝：和点击共用一条落点 ----

    /// 带真内核（Ask 的 Effect/Decision/Outcome 循环）+ 编辑器按键 + 确认面板
    /// 资源的最小 App；不建窗口、不渲染。
    fn ask_app() -> App {
        use bevy::app::TaskPoolPlugin;

        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.add_plugins(rig_ecs::AgentPlugin);
        app.add_message::<KeyboardInput>()
            .add_message::<PromptSubmitted>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Editor>()
            .init_resource::<SubmittedPrompts>()
            .init_resource::<KeyFocus>()
            .init_resource::<PendingInput>()
            .init_resource::<PanelRows>();
        crate::keys::add_built_in(&mut app);
        app.add_systems(Update, (crate::keys::arm_inline_keys, keys).chain());
        app
    }

    /// 在世界里 spawn 一个确认问题（选项 `(id, label)`），等内核给它挂上
    /// `Effect`/`Decision`。
    fn spawn_ask(app: &mut App, choices: &[(&str, &str)]) -> Entity {
        let mut ask = Ask::new("确认下单并支付？").body("商品：Nike Air Zoom Pegasus 40\n合计：469");
        for (id, label) in choices {
            ask = ask.choice(*id, *label);
        }
        let entity = app.world_mut().spawn(ask).id();
        app.update();
        assert!(
            app.world().get::<Decision>(entity).is_some(),
            "Ask 的 Effect 要带上内核的 Decision"
        );
        entity
    }

    /// 在世界里 spawn 一个 free-text 问题（`initial` 是可编辑的起始文本，
    /// `multiline` 允许换行），等内核挂上 `Effect`/`Decision`。
    ///
    /// `app.update()` 也会跑一遍 [`seed_initial_answer`]：有 `initial` 时，
    /// 返回时输入框里已经填好它。
    fn spawn_free_text(
        app: &mut App,
        title: &str,
        initial: Option<&str>,
        multiline: bool,
    ) -> Entity {
        let mut ask = Ask::new(title);
        match initial {
            Some(initial) => ask = ask.initial(initial),
            None => ask = ask.free_text(),
        }
        if multiline {
            ask = ask.multiline();
        }
        let entity = app.world_mut().spawn(ask).id();
        app.update();
        assert!(
            app.world().get::<Decision>(entity).is_some(),
            "Ask 的 Effect 要带上内核的 Decision"
        );
        entity
    }

    /// `ask_app` + 一个真 agent + `ShownAgent` + `Deliver` 观察者：free-text
    /// 作答要同时断言"Ask 的 Decision"与"没有 Deliver"。
    fn answering_app() -> (App, Entity) {
        let mut app = ask_app();
        let agent = app.world_mut().spawn(Name::new("agent")).id();
        app.insert_resource(ShownAgent(Some(agent)))
            .init_resource::<Observed>()
            .add_observer(record_deliver)
            .add_observer(record_stop);
        (app, agent)
    }

    /// Alt+数字：按住 Alt 发一个数字键，再松开。
    fn alt_digit(app: &mut App, digit: char) {
        let code = match digit {
            '0' => KeyCode::Digit0,
            '1' => KeyCode::Digit1,
            '2' => KeyCode::Digit2,
            '3' => KeyCode::Digit3,
            other => panic!("测试只用 Alt+0..3，实际 {other}"),
        };
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::AltLeft);
        send(
            app,
            Key::Character(digit.to_string().into()),
            code,
            Some(&digit.to_string()),
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::AltLeft);
    }

    /// 内联回答键（Alt+1..9、Alt+0）各自的 `Disabled` 状态。
    fn inline_disabled(app: &mut App) -> Vec<bool> {
        let mut query = app.world_mut().query_filtered::<
            Has<Disabled>,
            (With<crate::keys::InlineKey>, Allow<Disabled>),
        >();
        query.iter(app.world()).collect()
    }

    /// 这一帧确认面板的行（手动跑一次产行系统，不依赖帧布局）。
    fn confirm_rows_now(app: &mut App) -> Vec<KeyedRow> {
        use bevy::ecs::system::RunSystemOnce;

        app.world_mut()
            .run_system_once(crate::batch::confirm_panel_rows)
            .expect("确认面板系统要能跑");
        app.world()
            .resource::<PanelRows>()
            .get(PanelId::Confirm)
            .to_vec()
    }

    /// 这个 Ask 已经收到的选项答案（没有决定/拒绝/文本答案时 `None`）。
    fn decision_answer(app: &App, ask: Entity) -> Option<Answer> {
        app.world()
            .get::<Decision>(ask)
            .and_then(Decision::get)
            .and_then(Answer::of)
    }

    fn decision_by_person(app: &App, ask: Entity) -> bool {
        app.world()
            .get::<Decision>(ask)
            .is_some_and(|decision| matches!(decision.by(), Some(Decider::Person)))
    }

    #[test]
    fn alt_one_answers_the_first_choice_of_the_confirm_ask() {
        let mut app = ask_app();
        let ask = spawn_ask(&mut app, &[("confirm", "确认支付"), ("cancel", "取消")]);
        assert_eq!(inline_disabled(&mut app).len(), 10, "Alt+0..9 共十条");
        assert!(
            inline_disabled(&mut app).iter().all(|disabled| !*disabled),
            "有未回答的 Ask 时内联回答键要启用"
        );

        alt_digit(&mut app, '1');

        assert_eq!(
            decision_answer(&app, ask),
            Some(Answer::choice("confirm")),
            "Alt+1 应答第一个选项"
        );
        assert!(decision_by_person(&app, ask), "作答者应是 Decider::Person");
    }

    #[test]
    fn alt_two_answers_the_second_choice_of_the_confirm_ask() {
        let mut app = ask_app();
        let ask = spawn_ask(&mut app, &[("confirm", "确认支付"), ("cancel", "取消")]);
        alt_digit(&mut app, '2');
        assert_eq!(
            decision_answer(&app, ask),
            Some(Answer::choice("cancel")),
            "Alt+2 应答第二个选项"
        );
    }

    #[test]
    fn alt_past_the_last_choice_answers_nothing() {
        let mut app = ask_app();
        let ask = spawn_ask(&mut app, &[("confirm", "确认支付"), ("cancel", "取消")]);
        alt_digit(&mut app, '3');
        assert!(
            !app.world()
                .get::<Decision>(ask)
                .is_some_and(Decision::is_decided),
            "Alt+3 越界：什么也不答"
        );
        assert!(
            !confirm_rows_now(&mut app).is_empty(),
            "越界不改确认面板，问题还在"
        );
    }

    #[test]
    fn alt_digits_answer_the_first_ask_in_the_confirm_tile() {
        let mut app = ask_app();
        let first = spawn_ask(&mut app, &[("one", "一")]);
        let second = spawn_ask(&mut app, &[("two", "二")]);

        alt_digit(&mut app, '1');

        assert_eq!(
            decision_answer(&app, first),
            Some(Answer::choice("one")),
            "Alt+n 答的是确认 tile 里最老的那块（FIFO，和 confirm_rows 一致）"
        );
        assert!(
            !app.world()
                .get::<Decision>(second)
                .expect("第二个 Ask 也要有 Decision")
                .is_decided(),
            "后面的问题不受影响"
        );
    }

    #[test]
    fn alt_zero_refuses_the_ask_the_way_the_harness_does() {
        let mut app = ask_app();
        let ask = spawn_ask(&mut app, &[("confirm", "确认支付"), ("cancel", "取消")]);

        alt_digit(&mut app, '0');

        let decision = app.world().get::<Decision>(ask).expect("Ask 要有 Decision");
        assert!(decision.is_decided(), "Alt+0 应拒绝问题");
        assert!(matches!(decision.by(), Some(Decider::Person)));
        assert!(
            Answer::of(decision.get().expect("已决定")).is_none(),
            "拒绝不是选项答案"
        );
        assert!(
            decision.get().expect("已决定").0.is_err(),
            "拒绝是 Outcome::refused（和 rig-harness 的 Pick::person(ask, None) 一致）"
        );
    }

    #[test]
    fn inline_keys_arm_only_while_an_ask_is_open() {
        let mut app = ask_app();
        app.update();
        assert_eq!(inline_disabled(&mut app).len(), 10);
        assert!(
            inline_disabled(&mut app).iter().all(|disabled| *disabled),
            "没有 Ask 时 Alt+0..9 全挂 Disabled"
        );

        let ask = spawn_ask(&mut app, &[("confirm", "确认支付")]);
        assert!(
            inline_disabled(&mut app).iter().all(|disabled| !*disabled),
            "有未回答的 Ask 时摘掉 Disabled"
        );

        alt_digit(&mut app, '1');
        for _ in 0..4 {
            app.update();
        }
        if let Some(decision) = app.world().get::<Decision>(ask) {
            assert!(decision.is_decided(), "问题应已被回答");
        }
        assert!(
            inline_disabled(&mut app).iter().all(|disabled| *disabled),
            "没有未回答的 Ask 后 Alt+0..9 挂回 Disabled"
        );
    }

    #[test]
    fn alt_digit_without_an_ask_does_not_swallow_a_plugin_binding() {
        #[derive(Resource, Default)]
        struct Pressed(bool);

        let mut app = test_app();
        app.init_resource::<Pressed>();
        app.add_key_binding(
            KeyBinding::new(KeyPress::alt('1'), "Plugin").in_scope(KeyScope::Global),
            |_pressed: On<KeyPressed>, mut seen: ResMut<Pressed>| seen.0 = true,
        );
        app.update();
        assert!(
            inline_disabled(&mut app).iter().all(|disabled| *disabled),
            "没有 Ask：内联键不能启用"
        );

        alt_digit(&mut app, '1');

        assert!(
            app.world().resource::<Pressed>().0,
            "没有 Ask 时 Alt+1 不能让内联绑定吞掉，插件绑定要收到 KeyPressed"
        );
        assert!(text(&app).is_empty(), "Alt+1 也不能当字符打进输入框");
    }

    #[test]
    fn answering_the_ask_clears_the_confirm_panel_next_frame() {
        let mut app = ask_app();
        let ask = spawn_ask(&mut app, &[("confirm", "确认支付"), ("cancel", "取消")]);
        let texts: Vec<String> = confirm_rows_now(&mut app)
            .iter()
            .map(|keyed| plain(&keyed.row.line))
            .collect();
        assert!(
            texts.iter().any(|text| text.contains("确认支付")),
            "确认面板要列出选项，实际 {texts:?}"
        );

        alt_digit(&mut app, '1');
        assert_eq!(decision_answer(&app, ask), Some(Answer::choice("confirm")));

        // 内核把 Decision 落成 Outcome 后，下一帧确认面板不再产出这些行。
        for _ in 0..4 {
            app.update();
        }
        assert!(
            confirm_rows_now(&mut app).is_empty(),
            "回答后确认行应消失（confirm_panel_rows 下一帧不再产出）"
        );
    }

    #[test]
    fn escape_refuses_the_open_ask_before_stopping_the_turn() {
        let (mut app, agent) = sending_app();
        run_a_turn(&mut app, agent);
        type_text(&mut app, "keep");
        let ask = app
            .world_mut()
            .spawn(
                Ask::new("确认下单并支付？")
                    .choice("confirm", "确认支付")
                    .choice("cancel", "取消"),
            )
            .id();
        app.update();

        send(&mut app, Key::Escape, KeyCode::Escape, None);

        let decision = app.world().get::<Decision>(ask).expect("Ask 要有 Decision");
        assert!(decision.is_decided(), "有 Ask 时 Esc 先拒绝问题");
        assert!(matches!(decision.by(), Some(Decider::Person)));
        assert!(
            Answer::of(decision.get().expect("已决定")).is_none(),
            "拒绝不是选项答案"
        );
        assert!(
            observed(&app).stopped.is_empty(),
            "拒绝问题不能顺手停掉正在跑的 turn"
        );
        assert_eq!(text(&app), "keep", "Esc 不动输入");

        // 问题没了以后 Esc 回到原来的行为：停 ShownAgent 的 turn。
        app.world_mut().entity_mut(ask).despawn();
        send(&mut app, Key::Escape, KeyCode::Escape, None);
        assert_eq!(observed(&app).stopped, [agent], "没有问题时 Esc 还是停 turn");
    }

    // ---- 输入框作答（free-text Ask）：Enter 归问题，不再发送 ----

    #[test]
    fn enter_answers_a_free_text_ask_instead_of_delivering() {
        let (mut app, _agent) = answering_app();
        let ask = spawn_free_text(&mut app, "说说你的想法", None, false);
        type_text(&mut app, "yes please");
        send(&mut app, Key::Enter, KeyCode::Enter, None);

        assert_eq!(
            decision_answer(&app, ask),
            Some(Answer::text("yes please")),
            "Enter 应把输入框里的文本作为 free-text 答案"
        );
        assert!(decision_by_person(&app, ask), "作答者是 Decider::Person");
        assert!(
            observed(&app).delivered.is_empty(),
            "用输入框作答时不能同时把文本发出去"
        );
        assert_eq!(text(&app), "", "作答后输入框清空");
        assert!(
            app.world().resource::<SubmittedPrompts>().0.is_empty(),
            "答案不是发送的消息，不回显"
        );
    }

    #[test]
    fn enter_with_an_empty_input_answers_nothing_and_sends_nothing() {
        let (mut app, _agent) = answering_app();
        let ask = spawn_free_text(&mut app, "怎么做？", None, false);
        send(&mut app, Key::Enter, KeyCode::Enter, None);

        assert!(
            !app.world()
                .get::<Decision>(ask)
                .expect("Ask 要有 Decision")
                .is_decided(),
            "空输入时 Enter 不作答"
        );
        assert!(
            observed(&app).delivered.is_empty(),
            "空输入时 Enter 也不发送"
        );
        assert_eq!(text(&app), "", "空输入保持为空");
    }

    #[test]
    fn enter_still_delivers_when_the_ask_only_has_choices() {
        let (mut app, agent) = answering_app();
        let ask = spawn_ask(&mut app, &[("confirm", "确认支付"), ("cancel", "取消")]);
        type_text(&mut app, "把这句话发出去");
        send(&mut app, Key::Enter, KeyCode::Enter, None);

        assert_eq!(
            observed(&app).delivered,
            [(agent, "把这句话发出去".to_owned(), DeliveryMode::Steer)],
            "选项型问题（free_text=false）不抢 Enter，仍按普通消息发送"
        );
        assert!(
            !app.world()
                .get::<Decision>(ask)
                .expect("Ask 要有 Decision")
                .is_decided(),
            "选项型问题不能被输入框里的文字答掉"
        );
    }

    #[test]
    fn an_inline_free_text_ask_leaves_enter_as_a_message() {
        let (mut app, agent) = answering_app();
        // `Ask::inline`：问题摆在旁边，用户下一行是给 agent 的消息而不是答案
        // （上游 `ask.rs:78-82`）。
        let ask = app
            .world_mut()
            .spawn(Ask::new("旁边的问题").free_text().inline())
            .id();
        app.update();
        assert!(
            app.world().get::<Decision>(ask).is_some(),
            "Ask 的 Effect 要带上内核的 Decision"
        );

        type_text(&mut app, "这是给 agent 的消息");
        send(&mut app, Key::Enter, KeyCode::Enter, None);

        assert_eq!(
            observed(&app).delivered,
            [(agent, "这是给 agent 的消息".to_owned(), DeliveryMode::Steer)],
            "inline 的 free-text 问题不能抢 Enter，输入仍然是消息"
        );
        assert!(
            !app.world()
                .get::<Decision>(ask)
                .expect("Ask 要有 Decision")
                .is_decided(),
            "inline 问题不能被这一行误答"
        );
        assert_eq!(text(&app), "", "消息发送后输入框照常清空");
    }

    #[test]
    fn enter_answers_the_oldest_free_text_ask_first() {
        let (mut app, _agent) = answering_app();
        let first = spawn_free_text(&mut app, "第一个问题", None, false);
        let second = spawn_free_text(&mut app, "第二个问题", None, false);
        type_text(&mut app, "先答最老的");
        send(&mut app, Key::Enter, KeyCode::Enter, None);

        assert_eq!(
            decision_answer(&app, first),
            Some(Answer::text("先答最老的")),
            "多个 free-text 问题时答最老的（FIFO，和确认 tile 同一顺序）"
        );
        assert!(
            decision_answer(&app, second).is_none(),
            "后面的问题不受影响"
        );
    }

    #[test]
    fn a_multiline_free_text_ask_takes_every_line_typed_before_enter() {
        let (mut app, _agent) = answering_app();
        let ask = spawn_free_text(&mut app, "分几行说", None, true);
        type_text(&mut app, "first");
        with_modifier(&mut app, KeyCode::ShiftLeft, Key::Enter, KeyCode::Enter);
        type_text(&mut app, "second");
        assert_eq!(text(&app), "first\nsecond", "Shift+Enter 仍换行");
        send(&mut app, Key::Enter, KeyCode::Enter, None);

        assert_eq!(
            decision_answer(&app, ask),
            Some(Answer::text("first\nsecond")),
            "multiline 问题把整段（含换行）作为答案"
        );
        assert!(observed(&app).delivered.is_empty(), "多行答案也不发送");
    }

    #[test]
    fn the_initial_text_is_filled_once_and_never_again() {
        let (mut app, _agent) = answering_app();
        let ask = spawn_free_text(&mut app, "从这句开始改", Some("改这句话"), false);
        assert_eq!(
            text(&app),
            "改这句话",
            "第一次成为最老的可作答问题时把 initial 填进空输入框"
        );

        // 用户清空输入（模拟改过/删光），再等一帧：不能回填。
        app.world_mut().resource_mut::<Editor>().take();
        app.update();
        assert_eq!(text(&app), "", "用户清空后 initial 不再回填");

        // 空输入按 Enter：既不作答，也不发送。
        send(&mut app, Key::Enter, KeyCode::Enter, None);
        assert_eq!(text(&app), "", "空 Enter 后依然不回填");
        assert!(
            !app.world()
                .get::<Decision>(ask)
                .expect("Ask 要有 Decision")
                .is_decided(),
            "空输入不能拿 initial 之外的空串作答"
        );
        assert!(observed(&app).delivered.is_empty());
    }

    #[test]
    fn the_confirm_tile_points_a_free_text_ask_at_the_input() {
        let mut app = ask_app();
        let _ask = spawn_free_text(&mut app, "说说你的想法", None, false);
        let rows = confirm_rows_now(&mut app);
        let hint = rows
            .iter()
            .find(|keyed| plain(&keyed.row.line).contains("类型作答"))
            .expect("free-text 问题的确认块要有一行指路提示");
        let hint_text = plain(&hint.row.line);
        assert!(
            hint_text.contains("在输入框") && hint_text.contains("Enter"),
            "提示要写明去输入框里按 Enter：{hint_text:?}"
        );
        assert!(
            rows.last()
                .is_some_and(|last| plain(&last.row.line) == hint_text),
            "提示是这块的最后一行：{:?}",
            rows.iter().map(|keyed| plain(&keyed.row.line)).collect::<Vec<_>>()
        );
        assert!(
            hint.row.line
                .spans
                .iter()
                .all(|span| span.style.add_modifier.contains(Modifier::DIM)),
            "提示行要用 dim 样式：{:?}",
            hint.row.line.spans
        );
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
        let shown = layout.visible_rows(&Theme::default());
        let text: Vec<String> = shown.iter().map(plain).collect();
        assert_eq!(text, ["› one two", "  three", "  four five"]);
        assert_eq!(
            cursor_marker(&shown[0]),
            Some((2, "o".to_owned())),
            "光标反色行首的 o，不多占列"
        );
        assert!(
            text.iter().all(|row| !row.contains(CURSOR)),
            "不再把 ▏ 插进行里"
        );

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
    fn typed_text_continuation_rows_are_not_hanging_indented() {
        // 首词后两个空格：普通 `wrap` 会把它当列（悬挂缩进），输入框用
        // `wrap_typed`，续行从行首开始（上游 `rig-tui editor.rs` 的选择）。
        let editor = ed("run  one two three");
        let layout = editor.layout(10); // 正文 8 列
        let raw: Vec<String> = layout.rows.iter().map(plain).collect();
        assert_eq!(raw, ["run", "one two", "three"]);
        assert!(
            raw[1..].iter().all(|row| !row.starts_with(' ')),
            "续行不能悬挂缩进：{raw:?}"
        );

        // 同一文本交给普通 `wrap` 会缩进续行——证明换的是对的入口。
        let upstream: Vec<String> = rig_tui::wrap::wrap(&Line::from("run  one two three"), 8)
            .into_iter()
            .map(|(row, _)| plain(&row))
            .collect();
        assert!(
            upstream[1..].iter().any(|row| row.starts_with(' ')),
            "普通折行器会悬挂缩进：{upstream:?}"
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
        let shown = layout.visible_rows(&Theme::default());
        let text: Vec<String> = shown.iter().map(plain).collect();
        assert_eq!(text, ["› one two", "  three", "  four five"]);
        assert_eq!(
            cursor_marker(&shown[1]),
            Some((5, "e".to_owned())),
            "光标反色第二行第 3 个显示列的 e"
        );
        assert!(cursor_marker(&shown[0]).is_none(), "光标不能留在第一行");
        let marker = shown[1]
            .spans
            .iter()
            .find(|span| span.style.add_modifier.contains(Modifier::REVERSED))
            .expect("光标是行内被反色的 span，不是额外字符");
        assert!(
            marker.style.add_modifier.contains(Modifier::UNDERLINED),
            "光标要下划线"
        );

        // 行首：光标在行内第一个字上。
        editor.set_cursor(8);
        let layout = editor.layout(12);
        assert_eq!((layout.cursor_row, layout.cursor_column), (1, 0));
        let shown = layout.visible_rows(&Theme::default());
        assert_eq!(plain(&shown[1]), "  three");
        assert_eq!(cursor_marker(&shown[1]), Some((2, "t".to_owned())));
    }

    #[test]
    fn cursor_columns_count_double_width_characters() {
        let mut editor = ed("你好世界");
        editor.set_cursor(6); // 世界 前
        let layout = editor.layout(6); // 正文 4 列：你好 / 世界
        assert_eq!(layout.rows.len(), 2);
        assert_eq!((layout.cursor_row, layout.cursor_column), (1, 0));
        let shown = layout.visible_rows(&Theme::default());
        assert_eq!(plain(&shown[1]), "  世界");
        assert_eq!(cursor_marker(&shown[1]), Some((2, "世".to_owned())));
        assert_eq!(shown[1].width(), 6, "宽字反色不多占列");

        editor.set_cursor(9); // 世 与 界 之间
        let layout = editor.layout(6);
        assert_eq!((layout.cursor_row, layout.cursor_column), (1, 2));
        let shown = layout.visible_rows(&Theme::default());
        assert_eq!(plain(&shown[1]), "  世界");
        assert_eq!(
            cursor_marker(&shown[1]),
            Some((4, "界".to_owned())),
            "汉字算 2 个显示列"
        );
        assert_eq!(shown[1].width(), 6);
    }

    #[test]
    fn a_cursor_in_whitespace_dropped_by_wrapping_stays_at_its_column() {
        let mut editor = ed("one two three");
        editor.set_cursor(3); // "one" 后面被折行丢掉的空格
        let layout = editor.layout(7); // 正文 5 列
        assert_eq!((layout.cursor_row, layout.cursor_column), (0, 3));
        assert_eq!(plain(&layout.rows[0]), "one", "空格被折行丢掉");
        let in_view = layout.cursor_in_view().expect("光标行要在可见段里");
        let shown = layout.visible_rows(&Theme::default());
        assert_eq!(
            cursor_marker(&shown[in_view]),
            Some((2 + layout.cursor_column, " ".to_owned())),
            "光标落在被丢掉空白的列上，补的是反色空格"
        );
        assert_eq!(
            shown[in_view].width(),
            2 + layout.cursor_column + 1,
            "提示符 2 列 + 空格补到光标列 + 反色格"
        );
        assert!(shown[in_view].width() <= 7);
    }

    #[test]
    fn a_cursor_after_a_full_row_starts_a_new_row() {
        let editor = ed("0123456789"); // 正好 10 列 = 正文宽度
        let layout = editor.layout(12);
        assert_eq!(layout.rows.len(), 2, "写满一行后光标另起一行");
        assert_eq!((layout.cursor_row, layout.cursor_column), (1, 0));
        let shown = layout.visible_rows(&Theme::default());
        assert_eq!(plain(&shown[0]), "› 0123456789");
        assert_eq!(shown[0].width(), 12, "写满的行正好 12 列，不超");
        assert_eq!(plain(&shown[1]), "   ", "新行只有反色的一个空格");
        assert_eq!(cursor_marker(&shown[1]), Some((2, " ".to_owned())));
        assert_eq!(shown[1].width(), 3);
    }

    #[test]
    fn a_full_row_does_not_widen_with_the_cursor() {
        // 审查算例：面板 12 列、正文正好 10 列、光标在第 5 列。旧实现插入
        // 额外一列 `▏`，渲染成 `› 01234▏56789` = 13 列 > 12，行尾被裁。
        let mut editor = ed("0123456789");
        editor.set_cursor(5);
        let layout = editor.layout(12);
        assert_eq!((layout.cursor_row, layout.cursor_column), (0, 5));
        let shown = layout.visible_rows(&Theme::default());
        assert_eq!(shown.len(), 1);
        assert_eq!(plain(&shown[0]), "› 0123456789", "没有多出来的字符");
        assert_eq!(shown[0].width(), 12, "渲染行宽 = 面板列数，不超");
        assert_eq!(
            cursor_marker(&shown[0]),
            Some((7, "5".to_owned())),
            "反色光标列上的 5"
        );
        assert!(!plain(&shown[0]).contains(CURSOR));
    }

    #[test]
    fn rendered_rows_never_exceed_the_panel_width() {
        let samples = [
            ("0123456789", 12usize),
            ("one two three four five", 12),
            ("你好世界", 6),
            ("abcdefghij", 3),
            ("👨\u{200d}👩\u{200d}👧\u{200d}👦 family", 8),
            ("one two three", 7),
            ("", 3),
        ];
        for (text, panel) in samples {
            // 逐字节放光标（会吸附到 grapheme 边界），所有行都不能超面板列数。
            for cursor in 0..=text.len() {
                let mut editor = ed(text);
                editor.set_cursor(cursor);
                let layout = editor.layout(panel);
                for (index, row) in layout
                    .visible_rows(&Theme::default())
                    .iter()
                    .enumerate()
                {
                    assert!(
                        row.width() <= panel,
                        "text={text:?} panel={panel} cursor={cursor} row={index}: \
                         渲染宽 {} > {panel}",
                        row.width()
                    );
                }
            }
        }
    }

    #[test]
    fn a_panel_narrower_than_the_prompt_still_never_overflows() {
        for panel in 1..=2 {
            let editor = ed("abc");
            let layout = editor.layout(panel);
            for row in layout.visible_rows(&Theme::default()) {
                assert!(row.width() <= panel, "panel={panel} row={row:?}");
            }
        }
    }

    #[test]
    fn the_cursor_is_always_painted_when_its_row_is_visible() {
        // 反色式光标不能因为"行尾放不下"或滚动窗口而消失：布局把写满一行的
        // 行尾光标挪到下一行，`top` 又保证光标行在可见段里。
        for panel in 3..=20 {
            for text in [
                "",
                "a",
                "0123456789",
                "one two three four five",
                "你好世界",
                "run  one two three",
                "ab   cd",
                "e\u{301}x",
            ] {
                for cursor in 0..=text.len() {
                    // 面板正文比文本里最宽的一个 grapheme 还窄时，那个字本身
                    // 都放不下（折行也切不开一个 grapheme），裁行会把光标格
                    // 一起裁掉——这是退化几何，不属于这里要保的可见性。
                    let widest = text
                        .graphemes(true)
                        .map(UnicodeWidthStr::width)
                        .max()
                        .unwrap_or(0);
                    if panel < 2 + widest {
                        continue;
                    }
                    let mut editor = ed(text);
                    editor.set_cursor(cursor);
                    let layout = editor.layout(panel);
                    assert!(
                        layout.cursor_column < layout.body_width(),
                        "panel={panel} text={text:?} cursor={cursor}: \
                         光标列 {} 不满足 < 正文宽 {}（上游同一条不变量）",
                        layout.cursor_column,
                        layout.body_width()
                    );
                    let Some(in_view) = layout.cursor_in_view() else {
                        continue;
                    };
                    let shown = layout.visible_rows(&Theme::default());
                    assert!(
                        cursor_marker(&shown[in_view]).is_some(),
                        "panel={panel} text={text:?} cursor={cursor}: \
                         光标行 {in_view} 没有被反色：{shown:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn zwj_family_emoji_keeps_the_cursor_column_in_step_with_wrapping() {
        // 已验证口径一致：rig-markdown 0.47 的折行按 **grapheme** 切 cell，
        // unicode-width 0.2.3 对完整 ZWJ 序列算 2 列；cursor_column 用
        // "grapheme 边界上的前缀 width"，三者是同一套列宽（没有偏一列）。
        let family = "👨\u{200d}👩\u{200d}👧\u{200d}👦";
        assert_eq!(family.graphemes(true).count(), 1, "ZWJ 家族是一个 grapheme");
        assert_eq!(
            UnicodeWidthStr::width(family),
            2,
            "整个 ZWJ 序列占 2 列，不是里面每个 emoji 各 2 列"
        );

        let mut editor = ed(&format!("ab{family}cd"));
        // 正文 6 列：ab + 家族 emoji(2) + cd 正好一行。
        editor.set_cursor(2);
        let layout = editor.layout(8);
        assert_eq!(layout.rows.len(), 1);
        assert_eq!(layout.rows[0].width(), 6);
        assert_eq!((layout.cursor_row, layout.cursor_column), (0, 2));
        let shown = layout.visible_rows(&Theme::default());
        assert_eq!(
            cursor_marker(&shown[0]),
            Some((4, family.to_owned())),
            "反色整个家族，占 2 列"
        );
        assert_eq!(shown[0].width(), 8, "提示符 2 + 正文 6，正好填满");

        // 光标在家族 emoji 之后：列数是 2 + 2 = 4，不是 2 + 8。
        editor.set_cursor(2 + family.len());
        let layout = editor.layout(8);
        assert_eq!((layout.cursor_row, layout.cursor_column), (0, 4));
        let shown = layout.visible_rows(&Theme::default());
        assert_eq!(
            cursor_marker(&shown[0]),
            Some((6, "c".to_owned())),
            "光标没有偏一列"
        );

        // 窄到正文 4 列：家族 emoji 跨行（ab + emoji 一行，cd 下一行），
        // 光标在它之后落到下一行行首。
        let layout = editor.layout(6);
        assert_eq!((layout.cursor_row, layout.cursor_column), (1, 0));
        let shown = layout.visible_rows(&Theme::default());
        assert_eq!(plain(&shown[0]), format!("› ab{family}"));
        assert_eq!(cursor_marker(&shown[1]), Some((2, "c".to_owned())));
        assert!(shown.iter().all(|row| row.width() <= 6));
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
        let shown = layout.visible_rows(&Theme::default());
        let rows: Vec<String> = shown.iter().map(plain).collect();
        assert_eq!(rows.len(), MOST_LINES);
        assert_eq!(rows[0], "  line 5", "滚到光标所在的那一段");
        assert_eq!(rows[9], "  line 14 ", "行尾反色空格也是光标格");
        assert!(cursor_marker(&shown[9]).is_some(), "光标行在可见段里");

        // 光标回到第一行：滚回顶部。
        editor.set_cursor(0);
        let layout = editor.layout(97);
        assert_eq!(layout.top, 0);
        assert_eq!(layout.cursor_in_view(), Some(0));
        let shown = layout.visible_rows(&Theme::default());
        assert_eq!(plain(&shown[0]), "› line 0");
        assert_eq!(cursor_marker(&shown[0]), Some((2, "l".to_owned())));
    }

    #[test]
    fn a_short_input_panel_shows_seven_rows_and_keeps_the_cursor_visible() {
        // 审查场景：窗口矮到主区 < 200px 时 Fixed(200) 被压成 ~162px，
        // 输入面板只放得下 7 行；文本有 10 行，不能照写 10 行把光标行裁掉。
        let mut app = layout_app_with(97, 7);
        let text = (0..10)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.world_mut().resource_mut::<Editor>().set(text);
        app.update();

        let layout = app.world().resource::<EditorLayout>();
        assert_eq!(layout.visible, 7, "可见行数被面板高度卡住");
        assert_eq!(layout.top, 3, "滚动到光标所在段");
        assert_eq!(layout.cursor_in_view(), Some(6), "光标行一定在可见段里");
        let rows = app.world().resource::<PanelRows>().get(PanelId::Input);
        assert_eq!(rows.len(), 7, "只写面板放得下的 7 行");
        let shown: Vec<String> = rows.iter().map(|keyed| plain(&keyed.row.line)).collect();
        assert_eq!(shown[0], "  line 3");
        assert_eq!(shown[6], "  line 9 ");
        assert!(
            cursor_marker(&rows[6].row.line).is_some(),
            "光标行在最后一行"
        );

        // 面板再变矮：每个 Update 重算，不需要专门的事件。
        let root = app
            .world()
            .resource::<PanelRoots>()
            .entity(PanelId::Input)
            .expect("输入根实体");
        app.world_mut()
            .resource_mut::<PanelAreas>()
            .0
            .get_mut(&root)
            .expect("输入区域")
            .rows = 4;
        app.update();
        let rows = app.world().resource::<PanelRows>().get(PanelId::Input);
        assert_eq!(rows.len(), 4, "面板变矮后立刻少写行");
        assert!(
            cursor_marker(&rows[3].row.line).is_some(),
            "光标仍然可见"
        );
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
        assert_eq!(shown[1], "  three", "光标只改样式，不改文本");
        assert_eq!(shown[2], "  four five");
        assert_eq!(
            cursor_marker(&rows[1].row.line),
            Some((5, "e".to_owned())),
            "系统产出的行里光标在真实列上"
        );
    }

    #[test]
    fn editor_rows_keeps_a_full_row_within_the_panel() {
        // 端到端：面板 12 列、正文正好 10 列、光标在第 5 列，写出来的行
        // （含提示符和光标样式）仍然是 12 列，不会因光标多一列。
        let mut app = layout_app(12);
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.set("0123456789");
        editor.set_cursor(5);
        drop(editor);
        app.update();

        let rows = app.world().resource::<PanelRows>().get(PanelId::Input);
        assert_eq!(rows.len(), 1);
        assert_eq!(plain(&rows[0].row.line), "› 0123456789");
        assert_eq!(rows[0].row.line.width(), 12, "渲染行宽不超面板列数");
        assert_eq!(
            cursor_marker(&rows[0].row.line),
            Some((7, "5".to_owned())),
            "光标是反色的 5，不是额外字符"
        );
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
        assert_eq!(shown[9], "  line 19 ", "最后一行是光标行（含行尾光标格）");
        assert!(
            cursor_marker(&rows[9].row.line).is_some(),
            "光标标记在最后一行"
        );
    }

    // ---- 输入 tile 高度 ----

    #[test]
    fn input_tile_height_grows_with_lines_and_has_a_floor() {
        assert_eq!(INPUT_CHROME, 20.0, "pad 8 + border 2 的上下共 20px");
        assert_eq!(
            input_tile_height(0),
            LEAST_ROWS * INPUT_ROW_HEIGHT,
            "空输入也保底 3 行"
        );
        assert_eq!(
            input_tile_height(1),
            LEAST_ROWS * INPUT_ROW_HEIGHT,
            "1 行 + chrome 低于下限"
        );
        assert_eq!(
            input_tile_height(2),
            LEAST_ROWS * INPUT_ROW_HEIGHT,
            "2 行 + chrome 仍低于下限"
        );
        assert_eq!(
            input_tile_height(3),
            3.0 * INPUT_ROW_HEIGHT + INPUT_CHROME
        );
        assert_eq!(
            input_tile_height(4),
            4.0 * INPUT_ROW_HEIGHT + INPUT_CHROME
        );
        assert_eq!(
            input_tile_height(10),
            10.0 * INPUT_ROW_HEIGHT + INPUT_CHROME
        );
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

    // -----------------------------------------------------------------------
    // F5：输入框外壳借 feathers text_input 的外观（多行编辑内核不动）
    // -----------------------------------------------------------------------

    /// 和 `frame::spawn_panel` 一样的输入面板根节点：8px pad + 2px 边框。
    fn field_root_node() -> Node {
        Node {
            padding: bevy_ui::UiRect::all(px(crate::frame::PANEL_PADDING)),
            border: bevy_ui::UiRect::all(px(crate::frame::PANEL_BORDER)),
            border_radius: BorderRadius::all(px(crate::theme::PANEL_RADIUS)),
            ..default()
        }
    }

    /// 只装输入框外壳系统的 App：一个输入面板根 + 一份 feathers 暗色主题。
    fn field_app() -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<PanelRoots>()
            .init_resource::<KeyFocus>()
            .insert_resource(crate::theme::dark_ui_theme())
            .add_systems(Update, (sync_input_field, sync_input_row_height).chain());
        let root = app
            .world_mut()
            .spawn((
                field_root_node(),
                BackgroundColor(Color::BLACK),
                BorderColor::all(Color::BLACK),
                ThemeBorderColor(FOCUS_BORDER.clone()),
            ))
            .id();
        app.world_mut()
            .resource_mut::<PanelRoots>()
            .0
            .insert(PanelId::Input, root);
        (app, root)
    }

    fn count_with<C: Component>(app: &mut App) -> usize {
        let mut query = app.world_mut().query_filtered::<Entity, With<C>>();
        query.iter(app.world()).count()
    }

    fn node_height(app: &App, entity: Entity) -> bevy_ui::Val {
        app.world().get::<Node>(entity).expect("Node").height
    }

    /// 这个节点是不是真的画边框（`Node.border` 有一边不是 0）。
    ///
    /// 不能数 `BorderColor`：bevy 给每个 `Node` 都 required 了一个默认
    /// `BorderColor`（透明），行/文字节点也有。
    fn draws_border(app: &App, entity: Entity) -> bool {
        app.world().get::<Node>(entity).is_some_and(|node| {
            [
                node.border.left,
                node.border.right,
                node.border.top,
                node.border.bottom,
            ]
            .into_iter()
            .any(|side| side != px(0.0))
        })
    }

    /// 从 `root` 开始的整棵子树（含 root），先序展开。
    fn subtree(app: &App, root: Entity) -> Vec<Entity> {
        let mut entities = vec![root];
        let mut index = 0;
        while index < entities.len() {
            if let Some(children) = app.world().get::<Children>(entities[index]) {
                entities.extend(children.iter());
            }
            index += 1;
        }
        entities
    }

    /// 焦点在输入框时边框 = `FOCUS_BORDER`，焦点在别的面板时 = `PANEL_BORDER`；
    /// 水平内边距/圆角是 feathers text_input 的值。
    #[test]
    fn the_input_field_border_uses_the_focus_ring_only_while_focused() {
        let (mut app, root) = field_app();
        let (ring, normal) = {
            let theme = app.world().resource::<UiTheme>();
            (theme.color(&FOCUS_BORDER), theme.color(&PANEL_BORDER))
        };
        assert_ne!(ring, normal, "focus ring 和普通边框必须是两个 token");

        // `KeyFocus(None)` = 焦点在输入框。
        app.update();
        let border = app.world().get::<BorderColor>(root).expect("边框");
        assert_eq!(border.top, ring, "焦点在输入框：边框 = focus ring token");
        assert!(
            app.world().get::<ThemeBorderColor>(root).is_none(),
            "输入框必须摘掉 ThemeBorderColor，否则 frame 每帧把边框刷回 focus ring"
        );
        assert_eq!(count_with::<InputField>(&mut app), 1, "整块输入只有一个框");
        let node = app.world().get::<Node>(root).expect("Node");
        assert_eq!(node.padding.left, px(INPUT_PADDING_X), "行左侧内边距");
        assert_eq!(node.padding.right, px(INPUT_PADDING_X), "右侧对称");
        assert_eq!(node.border_radius, BorderRadius::all(px(INPUT_RADIUS)));

        // 焦点移到别的面板：同一帧换回普通边框 token。
        let elsewhere = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<KeyFocus>().0 = Some(elsewhere);
        app.update();
        let border = app.world().get::<BorderColor>(root).expect("边框");
        assert_eq!(border.top, normal, "焦点不在输入框：边框 = 普通边框 token");

        // 每个工作区都有自己的输入框；不在全局根表里的那个不是聚焦工作区，
        // 即使键盘焦点在（全局）输入框，它也只画普通边框。
        app.world_mut().resource_mut::<KeyFocus>().0 = None;
        let other_input = app
            .world_mut()
            .spawn((
                field_root_node(),
                BackgroundColor(Color::BLACK),
                BorderColor::all(ring),
                ThemeBorderColor(FOCUS_BORDER.clone()),
                PanelId::Input,
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<BorderColor>(root).expect("边框").top,
            ring,
            "聚焦工作区的输入框：焦点环"
        );
        assert_eq!(
            app.world()
                .get::<BorderColor>(other_input)
                .expect("边框")
                .top,
            normal,
            "非聚焦工作区的输入框：普通边框"
        );
        assert!(
            app.world().get::<ThemeBorderColor>(other_input).is_none(),
            "别的输入框也要摘掉 ThemeBorderColor"
        );
        assert_eq!(count_with::<InputField>(&mut app), 2, "两个工作区各一个输入框");
    }

    /// 真实接线：`FramePlugin` 每帧都会按生成时的 token 刷面板边框；输入框
    /// 摘掉 `ThemeBorderColor` 之后，它不能再把边框刷回 focus ring。
    #[test]
    fn the_frame_chrome_does_not_take_the_input_field_border_back() {
        if std::env::var("RF_NIRI_LAYOUT").is_ok() {
            return; // Niri 模式接管时默认屏幕不 spawn（与 frame 的测试同一条）。
        }
        let mut app = App::new();
        app.init_resource::<PanelRoots>()
            .init_resource::<PanelRows>()
            .add_plugins(crate::frame::FramePlugin)
            .init_resource::<KeyFocus>()
            .add_systems(
                Update,
                (sync_input_field, sync_input_row_height)
                    .chain()
                    .after(crate::FeathersSystems::Render),
            );
        // 焦点不在输入框：期望普通边框，而不是面板生成时的 focus ring。
        let elsewhere = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<KeyFocus>().0 = Some(elsewhere);
        app.update();

        let theme = crate::theme::dark_ui_theme();
        let normal = theme.color(&PANEL_BORDER);
        let roots = app.world().resource::<PanelRoots>();
        let input = roots.entity(PanelId::Input).expect("输入面板");
        assert_eq!(
            app.world().get::<BorderColor>(input).expect("边框").top,
            normal,
            "frame 刷过之后，输入框外壳仍然用普通边框 token"
        );
        assert!(
            app.world().get::<ThemeBorderColor>(input).is_none(),
            "frame 的 chrome 同步不该再管输入框边框"
        );

        // 再跑一帧：`sync_panel_chrome` 仍然不能把它刷回 focus ring。
        app.update();
        assert_eq!(
            app.world().get::<BorderColor>(input).expect("边框").top,
            normal,
            "第二帧保持普通边框"
        );
    }

    /// 行高/字号直接来自 feathers 的尺寸常量，左右内边距是 text input scene 的 3px。
    #[test]
    fn input_rows_take_the_feathers_text_input_metrics() {
        let bevy_ui::Val::Px(row_height) = feathers_size::ROW_HEIGHT else {
            panic!("feathers size::ROW_HEIGHT 应当是像素");
        };
        assert_eq!(INPUT_ROW_HEIGHT, row_height);
        let bevy_text::FontSize::Px(font_size) = feathers_size::COMPACT_FONT else {
            panic!("feathers size::COMPACT_FONT 应当是像素");
        };
        assert_eq!(INPUT_FONT_SIZE, font_size);
        assert_eq!(
            crate::transcript::FONT_SIZE,
            INPUT_FONT_SIZE,
            "行内字号就是 feathers text input 的字号"
        );
        // `FeathersTextInputContainer` 的 scene 里写死 padding left/right 3px、
        // 圆角 4px；feathers 没有间距/圆角 token，只能钉住这两个上游值。
        assert_eq!(INPUT_PADDING_X, 3.0);
        assert_eq!(INPUT_RADIUS, 4.0);

        let (mut app, root) = field_app();
        let input_row = app
            .world_mut()
            .spawn((
                Node {
                    height: px(ROW_HEIGHT),
                    ..default()
                },
                RowLayout {
                    panel: PanelId::Input,
                    panel_entity: root,
                    index: 0,
                },
            ))
            .id();
        let transcript_row = app
            .world_mut()
            .spawn((
                Node {
                    height: px(ROW_HEIGHT),
                    ..default()
                },
                RowLayout {
                    panel: PanelId::Transcript,
                    panel_entity: root,
                    index: 0,
                },
            ))
            .id();
        app.update();
        assert_eq!(
            node_height(&app, input_row),
            px(INPUT_ROW_HEIGHT),
            "输入行高 = feathers ROW_HEIGHT"
        );
        assert_eq!(
            node_height(&app, transcript_row),
            px(ROW_HEIGHT),
            "别的面板的行高不变"
        );
    }

    /// 多行输入时整块仍然只有一个输入框：三行正文 = 三个行实体，但带边框的
    /// 实体只有面板根一个，且行高统一是 feathers 的 24px。
    #[test]
    fn a_multiline_input_is_still_one_field_box() {
        let mut app = App::new();
        app.init_resource::<Editor>()
            .init_resource::<EditorLayout>()
            .init_resource::<PanelRows>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelAreas>()
            .init_resource::<KeyFocus>()
            .insert_resource(Theme::default())
            .insert_resource(crate::theme::dark_ui_theme())
            .insert_resource(crate::transcript::TranscriptFont(FontSource::default()))
            .configure_sets(
                Update,
                (
                    crate::FeathersSystems::Prepare,
                    crate::FeathersSystems::Draw,
                    crate::FeathersSystems::Render,
                )
                    .chain(),
            )
            .add_plugins(PanelsPlugin);
        app.add_systems(
            Update,
            (layout_editor, sync_editor_tile)
                .chain()
                .in_set(crate::FeathersSystems::Prepare),
        );
        app.add_systems(Update, editor_rows.in_set(crate::FeathersSystems::Draw));
        app.add_systems(
            Update,
            (sync_input_field, sync_input_row_height)
                .chain()
                .after(crate::FeathersSystems::Render),
        );
        let root = app
            .world_mut()
            .spawn((
                field_root_node(),
                BackgroundColor(Color::BLACK),
                BorderColor::all(Color::BLACK),
                ThemeBorderColor(FOCUS_BORDER.clone()),
            ))
            .id();
        app.world_mut()
            .resource_mut::<PanelRoots>()
            .0
            .insert(PanelId::Input, root);
        app.world_mut().resource_mut::<PanelAreas>().0.insert(
            root,
            crate::frame::PanelArea {
                cols: 97,
                rows: MOST_LINES as u16,
                ..Default::default()
            },
        );
        app.world_mut()
            .resource_mut::<Editor>()
            .set("one\ntwo\nthree");
        app.update();

        assert_eq!(
            app.world().resource::<PanelRows>().get(PanelId::Input).len(),
            3,
            "三行正文"
        );
        assert_eq!(count_with::<InputField>(&mut app), 1, "整块输入只有一个框");
        let bordered: Vec<Entity> = subtree(&app, root)
            .into_iter()
            .filter(|entity| draws_border(&app, *entity))
            .collect();
        assert_eq!(bordered, vec![root], "输入行自己不带边框");
        let children: Vec<Entity> = app
            .world()
            .get::<Children>(root)
            .expect("输入面板的行节点")
            .iter()
            .collect();
        assert_eq!(children.len(), 3, "多行 = 多个行实体");
        for row in children {
            assert_eq!(
                app.world().get::<RowLayout>(row).expect("RowLayout").panel,
                PanelId::Input
            );
            assert_eq!(
                node_height(&app, row),
                px(INPUT_ROW_HEIGHT),
                "输入行高 = feathers ROW_HEIGHT"
            );
        }
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
