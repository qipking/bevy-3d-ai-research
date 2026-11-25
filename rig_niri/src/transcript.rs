//! 转写层：`LookLine` -> bevy 节点，以及把一批 [`RenderBatch`] 组织成有序行的**对话引擎**。
//!
//! 上游 rig-tui 的 `transcript.rs`（1783 行）做两件事：把对话（message / call / notice /
//! aside / 流式回复）排成**行块**，以及把行画到终端格子上。这里照同样的分工：
//!
//! ```text
//! batch.rs（建批）──> RenderBatch ─┐
//!                                  ├─> transcript::Transcript（本文件）──> Row ──> bevy 节点
//! batch.rs（文字块）──> TextId ─────┘        │
//!                                            └─ 折行、排序、流式增补、finalize
//! ```
//!
//! 和上游的对应关系：
//!
//! | rig-tui | 这里 |
//! |---|---|
//! | `transcript::Part`（消息 / 本帧行） | [`BlockId`]（`RenderKey` 请求 / 没有 key 的文字块） |
//! | `transcript::Transcript`（缓存 + 宽度失效） | [`Transcript`]（按块缓存折行结果） |
//! | `transcript::{message_lines, content_lines, notice_lines}` | `batch::conversation_frame`（建块在 batch 侧） |
//! | `transcript::wrap_marked` | [`Transcript::rebuild`] 里的 `rig_tui::wrap::wrap` |
//! | `transcript::{Below, Scroll}` | [`Below`] / [`BelowSeen`]（滚动状态仍在 `pane.rs`） |
//!
//! `Row` 的公开结构保持不变（别的模块直接构造它），引擎只**产出** `Row`。

use std::collections::HashMap;

use bevy::picking::Pickable;
use bevy::prelude::*;
use bevy::text::{
    FontSource, LineBreak, Strikethrough, StrikethroughColor, TextBackgroundColor, TextLayout,
    Underline, UnderlineColor,
};
use bevy_ui::{BorderRadius, Node, percent, px};
use rig_tui::LookLine;
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::Line;
use rig_tui::wrap::wrap;
use rig_tui::{RenderBatch, RenderKey, RenderRequest};

use crate::panels::KeyedRow;
use crate::style;
use crate::theme::Theme;

/// 一行的纯文本（选择/复制用）。
#[derive(Component, Clone, Debug)]
pub struct RowText(pub String);

pub const ROW_HEIGHT: f32 = 18.0;
pub const FONT_SIZE: f32 = 13.0;

/// 转写里的一行：样式行 + 可选色带。
///
/// `right` 是右对齐的第二段（状态行右侧的条目、窗格滚上去时的"下面还有"标记，
/// 上游也画在行的右端）。
///
/// **公开结构不要改**：`panels`/`pane`/业务 look 都在直接构造它。
#[derive(Clone, Debug)]
pub struct Row {
    pub line: Line<'static>,
    /// 行右端的另一段（同一行）。
    pub right: Option<Line<'static>>,
    pub band: Option<RColor>,
    /// 若这一行是闸门问题的一个选项：问题的 Ask 实体 + 选项 id
    pub choice: Option<(Entity, String)>,
    /// 若这一行是 Picker 的选项：picker 实体 + 序号
    pub picker: Option<(Entity, usize)>,
}

/// 点击一行就能回答一个 [`Ask`](rig_harness::prelude::Ask)。
#[derive(Component, Clone)]
pub struct AskChoice {
    pub ask: Entity,
    pub choice: String,
}

impl Row {
    pub fn new(line: Line<'static>) -> Self {
        Self {
            line,
            right: None,
            band: None,
            choice: None,
            picker: None,
        }
    }

    /// 同一行，`right` 对齐到右端。
    pub fn with_right(mut self, right: Line<'static>) -> Self {
        self.right = Some(right);
        self
    }

    pub fn ask_choice(mut self, ask: Entity, choice: String) -> Self {
        self.choice = Some((ask, choice));
        self
    }

    /// 这一行是某个 `Picker` 的选项。
    pub fn picker_choice(mut self, picker: Entity, index: usize) -> Self {
        self.picker = Some((picker, index));
        self
    }
    pub fn band(mut self, color: RColor) -> Self {
        self.band = Some(color);
        self
    }
    /// 从 rig-tui 的 look 输出建行 —— 这是适配层。
    pub fn from_look(line: &LookLine) -> Self {
        Self::new(line.colored())
    }
}

/// 在一行节点里（重新）生成文字节点与 span —— 流式更新时只换这一行的内容。
pub fn fill_row_text(commands: &mut Commands, row_entity: Entity, row: &Row, font: &FontSource) {
    fill_row_text_themed(commands, row_entity, row, font, &Theme::default());
}

/// [`fill_row_text`] 的带主题版本：span 的 Modifier/底色由 `theme` 折算成颜色。
pub fn fill_row_text_themed(
    commands: &mut Commands,
    row_entity: Entity,
    row: &Row,
    font: &FontSource,
    theme: &Theme,
) {
    commands.entity(row_entity).despawn_children();
    let justify = match row.right.is_some() {
        true => JustifyContent::SpaceBetween,
        false => JustifyContent::FlexStart,
    };
    commands
        .entity(row_entity)
        .entry::<Node>()
        .and_modify(move |mut node| node.justify_content = justify);
    let font = font.clone();
    commands.entity(row_entity).with_children(|parent| {
        spawn_text_line(parent, &row.line, &font, theme);
        if let Some(right) = &row.right {
            spawn_text_line(parent, right, &font, theme);
        }
    });
}

/// 转写行用的字体（FiraMono + Noto CJK 回退链）。
#[derive(Resource, Clone)]
pub struct TranscriptFont(pub FontSource);

/// 生成一行 UI 节点，返回该行实体（增量更新需要它）。
pub fn spawn_row(parent: &mut ChildSpawnerCommands, row: &Row, font: &FontSource) -> Entity {
    spawn_row_themed(parent, row, font, &Theme::default())
}

/// [`spawn_row`] 的带主题版本。
pub fn spawn_row_themed(
    parent: &mut ChildSpawnerCommands,
    row: &Row,
    font: &FontSource,
    theme: &Theme,
) -> Entity {
    let font = font.clone();
    let mut entity = parent.spawn(Node {
        width: percent(100.0),
        height: px(ROW_HEIGHT),
        // 行高固定：不让 flex 压缩（否则面板变矮时行会被挤到一起而不是溢出）
        flex_shrink: 0.0,
        align_items: AlignItems::Center,
        // 有右对齐的行时两段分到两端（状态行的右侧条目、滚动的"下面还有"标记）。
        justify_content: match row.right.is_some() {
            true => JustifyContent::SpaceBetween,
            false => JustifyContent::FlexStart,
        },
        border_radius: BorderRadius::all(px(3.0)),
        ..default()
    });
    if let Some(band) = row.band {
        entity.insert(BackgroundColor(color_with_alpha(band, 0.55)));
    }
    let mut plain: String = row.line.spans.iter().map(|s| s.content.as_ref()).collect();
    if let Some(right) = &row.right {
        plain.push_str(
            &right
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
        );
    }
    entity.insert((RowText(plain), Pickable::default()));
    if let Some((ask, choice)) = &row.choice {
        entity.insert(AskChoice {
            ask: *ask,
            choice: choice.clone(),
        });
    }
    if let Some((picker, index)) = row.picker {
        entity.insert(crate::picker::PickerChoice { picker, index });
    }
    entity.with_children(|parent| {
        spawn_text_line(parent, &row.line, &font, theme);
        if let Some(right) = &row.right {
            spawn_text_line(parent, right, &font, theme);
        }
    });
    entity.id()
}

/// Spawns one styled line's text node (an empty `Text` with one `TextSpan` per run).
fn spawn_text_line(
    parent: &mut ChildSpawnerCommands,
    line: &Line<'static>,
    font: &FontSource,
    theme: &Theme,
) {
    parent
        .spawn((
            Text::new(String::new()),
            TextFont {
                font: font.clone(),
                font_size: FontSize::Px(FONT_SIZE),
                ..default()
            },
            TextColor(Color::WHITE),
            TextLayout {
                linebreak: LineBreak::NoWrap,
                ..default()
            },
        ))
        .with_children(|text| {
            for span in &line.spans {
                // 行自己的样式压在 span 上（ratatui 画行时也是这么合的）：
                // `Line::styled` 只设行样式，不并进来颜色就丢了。
                let style = line.style.patch(span.style);
                let paint = span_paint(&style, theme);
                // 字体必须显式给：TextSpan 隐式要求的 TextFont 是 20px 默认字体，
                // 不显式给会换字体、换图集（见 feathers_in_3d/README.md）。
                let mut entity = text.spawn((
                    TextSpan::new(span.content.clone()),
                    TextFont {
                        font: font.clone(),
                        font_size: FontSize::Px(FONT_SIZE),
                        ..default()
                    },
                    TextColor(paint.fg),
                ));
                // 底色/下划线/删除线是 bevy_ui_render 按 span 消费的普通组件
                // （`extract_text_decorations` 对每个 span 查它们），直接挂上。
                // 没有底色时**不要**插 `TextBackgroundColor`：它的默认值是黑，
                // 会给每一块普通文字刷一层黑底。
                if let Some(bg) = paint.bg {
                    entity.insert(TextBackgroundColor(bg));
                }
                if paint.underline {
                    entity.insert((Underline, UnderlineColor(paint.fg)));
                }
                if paint.strikethrough {
                    entity.insert((Strikethrough, StrikethroughColor(paint.fg)));
                }
            }
        });
}

/// Spawns `rows` as a column of UI nodes under `parent`.
pub fn spawn_rows(
    parent: &mut ChildSpawnerCommands,
    rows: &[Row],
    font: &FontSource,
) -> Vec<Entity> {
    let theme = Theme::default();
    rows.iter()
        .map(|row| spawn_row_themed(parent, row, font, &theme))
        .collect()
}

/// 一个 span 实际要画的东西：前景色 + 可选行内底色 + 装饰线。
///
/// 这是 [`spawn_text_line`] 给 span 实体挂组件的依据；[`span_style_color`]
/// 只算其中的前景色。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanPaint {
    /// 字形颜色（`DIM` / `BOLD` / 反色的前景已折进来）。
    pub fg: Color,
    /// 行内底色。`None` 表示**不画底**（别给 span 插 `TextBackgroundColor`）。
    pub bg: Option<Color>,
    /// 上游 `Modifier::UNDERLINED`：给 span 挂 [`Underline`]。
    pub underline: bool,
    /// 上游 `Modifier::CROSSED_OUT`：给 span 挂 [`Strikethrough`]。
    pub strikethrough: bool,
}

/// 把 span 的 `ratatui::Style` 折成渲染层能画的组件值。
///
/// bevy 0.20 的 `bevy_ui_render` 已经按 span 消费 `TextBackgroundColor` /
/// `Underline` / `Strikethrough`（`extract_text_decorations` 对每个 span 查
/// 这三个组件），它们就是普通 `Component`，直接挂在 `TextSpan` 实体上即可，
/// 不需要把 span 改成 `Node`。所以：
///
/// - 行内底色（`style.bg`）→ [`TextBackgroundColor`]，原样透传；
/// - 下划线 / 删除线 → [`Underline`] / [`Strikethrough`]，线色取前景色；
/// - `Modifier::REVERSED` → **真反色**：前景换成原本的底色（`style.bg`，没设
///   就用 `theme.band`），底色换成原本的前景。
///
/// 没有字重/斜体字形（`BOLD` / `DIM` / `ITALIC`）的部分仍由
/// [`span_style_color`] 折算成前景色，见那里的表。
pub fn span_paint(style: &Style, theme: &Theme) -> SpanPaint {
    let reversed = style.add_modifier.contains(Modifier::REVERSED);
    SpanPaint {
        fg: span_style_color(style, theme),
        bg: if reversed {
            // 终端反色把前景/底色对调：底色要画的是"原本的前景色"。
            Some(base_fg_color(style, theme))
        } else {
            style.bg.map(|bg| style::color(Some(bg)))
        },
        underline: style.add_modifier.contains(Modifier::UNDERLINED),
        strikethrough: style.add_modifier.contains(Modifier::CROSSED_OUT),
    }
}

/// 把 span 的样式折成**字形颜色**。
///
/// 底色、下划线、删除线由 span 实体上的组件真画（[`TextBackgroundColor`] /
/// [`Underline`] / [`Strikethrough`]，`bevy_ui_render` 按 span 消费，见
/// [`span_paint`]），函数只负责前景色：
///
/// | 上游样式 | 这里怎么办 |
/// |---|---|
/// | `Modifier::BOLD` | 往白提亮 25%（没有字重字形可用） |
/// | `Modifier::DIM` | 往面板底色混 45%（picker 的 `.patch_style(dim)` 靠这条才有用） |
/// | `Modifier::REVERSED` | 前景换成底色（`style.bg`，没设就用 `theme.band`）：和 [`span_paint`] 换出的底色合起来就是反色 |
/// | `Modifier::UNDERLINED` / `CROSSED_OUT` | 不在这里处理：由 [`span_paint`] 给出组件，`bevy_ui_render` 画线 |
/// | `Modifier::ITALIC` | 无字形变体，忽略 |
/// | `style.bg` | 不在这里处理：由 [`span_paint`] 交给 `TextBackgroundColor` |
///
/// 画不出来的只剩字重/斜体这类**没有字形变体**的样式（见
/// `docs/status/PARITY.md` §5），以及终端里才有的字符格对齐。
pub fn span_style_color(style: &Style, theme: &Theme) -> Color {
    // 真反色：前景 = "本来会画的底色"（底色组件由 [`span_paint`] 另给）。
    if style.add_modifier.contains(Modifier::REVERSED) {
        let bg = style.bg.unwrap_or(theme.band);
        return style::color(Some(bg));
    }

    let mut srgb = base_fg_color(style, theme).to_srgba();
    let band = style::color(Some(theme.band)).to_srgba();

    if style.add_modifier.contains(Modifier::DIM) {
        let k = 0.45;
        srgb.red = srgb.red * (1.0 - k) + band.red * k;
        srgb.green = srgb.green * (1.0 - k) + band.green * k;
        srgb.blue = srgb.blue * (1.0 - k) + band.blue * k;
    }
    if style.add_modifier.contains(Modifier::BOLD) {
        let k = 0.25;
        srgb.red += (1.0 - srgb.red) * k;
        srgb.green += (1.0 - srgb.green) * k;
        srgb.blue += (1.0 - srgb.blue) * k;
    }
    Color::srgb(srgb.red, srgb.green, srgb.blue)
}

/// span 的基础前景：span 自己的 `fg`，否则主题主文字，最后兜底浅灰
/// （不处理任何 `Modifier`）。
fn base_fg_color(style: &Style, theme: &Theme) -> Color {
    let fg = style
        .fg
        .or(theme.fg.fg)
        .unwrap_or(RColor::Rgb(210, 216, 228));
    style::color(Some(fg))
}

fn color_with_alpha(c: RColor, a: f32) -> Color {
    let base = style::color(Some(c));
    base.with_alpha(a)
}

// ---------------------------------------------------------------------------
// 对话引擎：一批 `RenderBatch` -> 有序、折好行的行块
// ---------------------------------------------------------------------------

/// 没有 `RenderKey` 的文字在对话里的位置。
///
/// 上游 `transcript::message_lines` 直接画这些（用户请求、assistant 正文、推理、
/// notice、流式回复、已接受未发送的输入）；它们不像 tool call / note 那样有
/// `RenderKey`，所以由建批的人（`batch.rs`）以 [`TextId`] 交出。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TextId {
    /// 第 `at` 条消息的第 `block` 个内容块（用户请求、assistant 正文或推理）。
    Message { at: usize, block: usize },
    /// 第 `index` 条 notice（按会话里出现的顺序；层级颜色由建批的人给）。
    Notice(usize),
    /// 正在流式输出的推理。
    StreamingReasoning,
    /// 正在流式输出的正文。
    StreamingReply,
    /// 本进程提交、但对话里还没有的提示词（没有可读对话时的兜底）。
    Prompt(usize),
    /// agent 已接受、尚未发送的输入（上游的 inbox）。
    Inbox(usize),
}

/// 一块行的稳定身份：决定它在行流里的位置，也决定增量更新时能不能原地替换。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BlockId {
    /// `RenderBatch` 里的一个请求，内容是 look 的产出（没有 look 时是默认行）。
    Request(RenderKey),
    /// 对话里没有 `RenderKey` 的一段文字（上游 `Part::Rows`）。
    Text(TextId),
}

/// aside / notice 落在对话的哪里（上游 `render::TranscriptSlot`）。
///
/// `RenderKey` 里没有这个信息（`RenderKey::Aside(AsideId)` 只带 id），
/// 所以由建批的人从 `Aside.after` / notice 的位置读出来，再 [`Transcript::place`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// 所有消息之前。
    Top,
    /// 第 `m` 条消息之后。
    After(usize),
    /// 第 `at` 条消息的第 `item` 个内容块之前（notice 写在消息里）。
    Inside { at: usize, item: usize },
}

/// 排序键：`(组, 组内位置, 块, 次序, 稳定序号)`。
///
/// 组：0 = 所有消息之前；`m + 1` = 第 m 条消息；`AFTER_MESSAGES` = 所有消息之后
/// （没给落点的 aside/notice、本进程提示词）；`STREAMING` = 流式回复与流式调用；
/// `INBOX` = 已接受未发送。组内位置：0 = 消息内块之前（inside），1 = 内容块，
/// 2 = 消息之后，3 = 所有消息之后。次序在同一 (组, 位置, 块) 里再分：
/// inside(0) < 内容(1)，以及同一行上的 tool(0) < content(1) < entry(2) < text(3)；
/// aside(0) < notice(1) < prompt(2)。
type Rank = (usize, u8, usize, u8, u64);

const AFTER_MESSAGES: usize = usize::MAX - 2;
const STREAMING: usize = usize::MAX - 1;
const INBOX: usize = usize::MAX;

/// 块 `id` 的排序键；`slot` 是 aside/notice 的落点。
fn rank(id: &BlockId, slot: Option<Slot>) -> Rank {
    match id {
        BlockId::Request(RenderKey::Tool(at, block)) => (at + 1, 1, *block, 1, 0),
        BlockId::Request(RenderKey::Content(at, block)) => (at + 1, 1, *block, 1, 1),
        BlockId::Request(RenderKey::Entry(at, block)) => (at + 1, 1, *block, 1, 2),
        // 流式进来的调用：上游把它画在回复正文之后、收件箱之前。
        BlockId::Request(RenderKey::Streaming(n)) => (STREAMING, 2, 0, 0, *n as u64),
        BlockId::Request(RenderKey::Aside(id)) => match slot {
            Some(Slot::Top) => (0, 2, 0, 0, id.get()),
            Some(Slot::After(m)) => (m + 1, 2, 0, 0, id.get()),
            Some(Slot::Inside { at, item }) => (at + 1, 1, item, 0, id.get()),
            None => (AFTER_MESSAGES, 3, 0, 0, id.get()),
        },
        BlockId::Text(TextId::Message { at, block }) => (at + 1, 1, *block, 1, 3),
        BlockId::Text(TextId::Notice(n)) => match slot {
            Some(Slot::Top) => (0, 2, 0, 1, *n as u64),
            Some(Slot::After(m)) => (m + 1, 2, 0, 1, *n as u64),
            Some(Slot::Inside { at, item }) => (at + 1, 1, item, 0, *n as u64),
            None => (AFTER_MESSAGES, 3, 0, 1, *n as u64),
        },
        BlockId::Text(TextId::StreamingReasoning) => (STREAMING, 0, 0, 0, 0),
        BlockId::Text(TextId::StreamingReply) => (STREAMING, 1, 0, 0, 0),
        BlockId::Text(TextId::Prompt(n)) => (AFTER_MESSAGES, 3, 0, 2, *n as u64),
        BlockId::Text(TextId::Inbox(n)) => (INBOX, 0, 0, 0, *n as u64),
    }
}

/// 一块行的缓存：未折的行 + 折好的行 + 折行时的宽度。
struct Block {
    /// 还没折的行（宽度变了只需要重折，不用重建）。
    lines: Vec<Line<'static>>,
    /// 折好的行，`None` 表示这一宽度下还没折过。
    rows: Option<Vec<Row>>,
    /// 折这些行用的宽度。
    width: usize,
    /// aside / notice 的落点。
    slot: Option<Slot>,
}

impl Block {
    fn new(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            rows: None,
            width: 0,
            slot: None,
        }
    }
}

/// 对话引擎：把每帧的一批 [`RenderBatch`]（look 的产出）和一批没有 `RenderKey`
/// 的文字块，组织成有序的 [`Row`] 行流。
///
/// - **排序**：按 [`BlockId`] 的身份排（消息序号 / 块序号 / aside 落点 /
///   流式位置），和上游 `transcript::Part` 的顺序一致。
/// - **折行**：宽度取自 `RenderBatch::width`（`batch.rs` 从上一帧 Layout 传进来），
///   用上游的 `rig_tui::wrap::wrap`，宽度变了只重折不重建。
/// - **流式增补**：同一 `RenderKey` 的行被整块替换，不会追加成两遍；
///   文字块（[`Transcript::set_text`]）同理 —— 给最后到达的全文即可。
/// - **finalize**：这一帧 batch 里没有的 `RenderKey` 连同它的块一起删掉
///   （流式调用落进对话时 `Streaming(n)` 消失、`Tool(m, b)` 出现，不会重复）。
#[derive(Resource, Default)]
pub struct Transcript {
    /// 上一次 Layout 给的文字列数；0 表示还不知道（这一帧不出行）。
    width: usize,
    blocks: HashMap<BlockId, Block>,
    /// 排好序、折好行的行流（给窗格与增量 diff）。
    rows: Vec<KeyedRow>,
    /// 需要重排／重折。
    dirty: bool,
}

impl Transcript {
    /// 设置折行宽度（来自 `RenderBatch::width`，它来自上一帧 Layout）。
    /// 返回是否真的变了。
    pub fn set_width(&mut self, width: u16) -> bool {
        let width = usize::from(width);
        if self.width == width {
            return false;
        }
        self.width = width;
        self.dirty = true;
        true
    }

    /// 折行宽度（0 = 还没从 Layout 拿到）。
    pub fn width(&self) -> usize {
        self.width
    }

    /// 用这一帧的 batch **覆盖**所有请求块：新增的插入、变了的替换、
    /// 不在 batch 里的（finalize 后消失的流式调用等）删掉。
    /// 返回是否变了。
    pub fn apply(&mut self, batch: &RenderBatch) -> bool {
        let mut changed = false;
        for (key, request) in &batch.requests {
            let lines = request_lines(request);
            let id = BlockId::Request(key.clone());
            match self.blocks.get_mut(&id) {
                Some(block) if block.lines == lines => {}
                Some(block) => {
                    block.lines = lines;
                    block.rows = None;
                    changed = true;
                }
                None => {
                    self.blocks.insert(id, Block::new(lines));
                    changed = true;
                }
            }
        }
        let before = self.blocks.len();
        self.blocks.retain(|id, _| match id {
            BlockId::Request(key) => batch.requests.contains_key(key),
            BlockId::Text(_) => true,
        });
        if self.blocks.len() != before {
            changed = true;
        }
        self.dirty |= changed;
        changed
    }

    /// 设置一段没有 `RenderKey` 的文字（用户请求、assistant 正文、推理、
    /// notice、流式回复……）。给的是**这一帧的全文**，整块替换；
    /// 空行集等于没有这块（上游"没有行的 item 不画"）。
    pub fn set_text(&mut self, id: TextId, lines: Vec<Line<'static>>) -> bool {
        if lines.is_empty() {
            return self.remove(&BlockId::Text(id));
        }
        let full = BlockId::Text(id);
        match self.blocks.get_mut(&full) {
            Some(block) if block.lines == lines => false,
            Some(block) => {
                block.lines = lines;
                block.rows = None;
                self.dirty = true;
                true
            }
            None => {
                self.blocks.insert(full, Block::new(lines));
                self.dirty = true;
                true
            }
        }
    }

    /// 删掉一块（没有行可画时）。
    pub fn remove(&mut self, id: &BlockId) -> bool {
        let removed = self.blocks.remove(id).is_some();
        self.dirty |= removed;
        removed
    }

    /// 告诉引擎这块落在哪（aside / notice 的位置）。块还不存在时先记下，
    /// 等它出现时生效。
    pub fn place(&mut self, id: BlockId, slot: Slot) -> bool {
        let block = self.blocks.entry(id).or_insert_with(|| Block::new(Vec::new()));
        if block.slot == Some(slot) {
            return false;
        }
        block.slot = Some(slot);
        self.dirty = true;
        true
    }

    /// 把 `keep` 说不要的文字块清掉（流式结束、消息被编辑掉时用）。
    /// 请求块不归它管（`apply` 每帧覆盖）。
    pub fn retain_text(&mut self, keep: impl Fn(&TextId) -> bool) -> bool {
        let before = self.blocks.len();
        self.blocks.retain(|id, _| match id {
            BlockId::Text(text) => keep(text),
            BlockId::Request(_) => true,
        });
        let changed = self.blocks.len() != before;
        self.dirty |= changed;
        changed
    }

    /// 排好序、折好行的行流；有变化时在这里重排。
    pub fn keyed_rows(&mut self) -> &[KeyedRow] {
        if self.dirty {
            self.rebuild();
        }
        &self.rows
    }

    /// 每一行的纯文本（测试与调试用；窗格用 [`Self::keyed_rows`]）。
    pub fn texts(&mut self) -> Vec<String> {
        let rows = self.keyed_rows();
        rows.iter()
            .map(|keyed| {
                keyed
                    .row
                    .line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    /// 有几块（测试用）。
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// 重排 + 重折，拼成行流。
    fn rebuild(&mut self) {
        self.dirty = false;
        // 宽度还没从 Layout 来：不出行，也别拿 1 列去折一段假的。
        if self.width == 0 {
            self.rows.clear();
            return;
        }
        let mut ids: Vec<BlockId> = self.blocks.keys().cloned().collect();
        ids.sort_by_key(|id| rank(id, self.blocks.get(id).and_then(|block| block.slot)));
        let width = self.width;
        let mut rows = Vec::new();
        for (ordinal, id) in ids.iter().enumerate() {
            let Some(block) = self.blocks.get_mut(id) else {
                continue;
            };
            if block.rows.is_none() || block.width != width {
                block.rows = Some(wrap_rows(&block.lines, width));
                block.width = width;
            }
            let Some(wrapped) = &block.rows else {
                continue;
            };
            for (index, row) in wrapped.iter().enumerate() {
                rows.push(KeyedRow {
                    key: row_key(ordinal, index),
                    hash: row_hash(row),
                    row: row.clone(),
                });
            }
        }
        self.rows = rows;
    }
}

/// 一个请求的可见行：look 的行 + look 加的具名贡献（上游 `RenderOutput::drawn`）。
fn request_lines(request: &RenderRequest) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = request
        .output
        .lines
        .iter()
        .map(LookLine::colored)
        .collect();
    for (_, more) in &request.output.decorations {
        lines.extend(more.iter().cloned());
    }
    lines
}

/// 把行级底色从 `Line` 里摘出来，返回它该落成的 [`Row::band`]。
///
/// `rig_markdown::wrap::wrap` 会把 `Line::style` 并进每个 span（见它的
/// `cells`），所以折过行的 span 会带着一份"行级底色"。那份底色已经由
/// [`Row::band`] 画了，span 上再挂 `TextBackgroundColor` 会重复画一遍，
/// 因此和行底色相同的 span 底色要清掉；不同色的才是真正的 span 级高亮。
///
/// `line.style.bg` 也一并清空，否则 [`fill_row_text_themed`] 的
/// `line.style.patch(span.style)` 又会把它并回 span。
pub(crate) fn take_line_band(line: &mut Line<'static>) -> Option<RColor> {
    let band = line.style.bg.take();
    for span in &mut line.spans {
        if span.style.bg == band {
            span.style.bg = None;
        }
    }
    band
}

/// 把未折的行按 `width` 折成 [`Row`]：用上游的折行（按 unicode 宽度、词边界、
/// 列表续行缩进），行自己的底色（`Line::style.bg`）变成 `Row::band`。
fn wrap_rows(lines: &[Line<'static>], width: usize) -> Vec<Row> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in lines {
        for (mut wrapped, _) in wrap(line, width) {
            let band = take_line_band(&mut wrapped);
            let mut row = Row::new(wrapped);
            row.band = band;
            rows.push(row);
        }
    }
    rows
}

/// 把一段行按 `width` 折成 [`Row`]（引擎之外的地方，比如 `Ask` 的行，也用它，
/// 保证和转写用同一套折行）。
pub fn wrap_lines(lines: &[Line<'static>], width: u16) -> Vec<Row> {
    wrap_rows(lines, usize::from(width.max(1)))
}

/// 行 key：块序号在高 32 位、块内第几行在低 32 位。位置稳定，`panels` 的前缀
/// diff 才能只重建变了的那段（`sync_panels` 用 key 对齐）。
fn row_key(ordinal: usize, index: usize) -> u64 {
    ((ordinal as u64) << 32) | (index as u64 & 0xFFFF_FFFF)
}

/// 行内容的 hash：文本 + 样式 + 色带 + 右段 + 可点身份。和 `batch` 里判定
/// "这一行变没变"用的是同一套。
pub fn row_hash(row: &Row) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for span in &row.line.spans {
        span.content.hash(&mut hasher);
        format!("{:?}", span.style).hash(&mut hasher);
    }
    for span in row.right.iter().flat_map(|right| right.spans.iter()) {
        span.content.hash(&mut hasher);
        format!("{:?}", span.style).hash(&mut hasher);
    }
    format!("{:?}", row.band).hash(&mut hasher);
    format!("{:?}", row.choice).hash(&mut hasher);
    format!("{:?}", row.picker).hash(&mut hasher);
    hasher.finish()
}

/// 视口下面还有什么（上游 `transcript::Below`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Below {
    /// 跟着末尾，下面没有未显示的行。
    #[default]
    Nothing,
    /// 滚上去时下面还有的行。
    More,
    /// 滚上去之后新到的行（上游画 `↓ new output`）。
    New,
}

/// "下面还有"标记的两句话（和上游 `pane::draw_below` 逐字一致）。
pub const MORE_BELOW: &str = " \u{2193} more below (PgDn) ";
/// 滚上去之后新到行的标记。
pub const NEW_OUTPUT: &str = " \u{2193} new output (PgDn) ";

impl Below {
    /// 下面还有什么：跟着末尾 → [`Below::Nothing`]；滚上去且下面还有行 →
    /// [`Below::More`]；滚动之后整段又长了（`total > seen`）→ [`Below::New`]。
    ///
    /// `seen` 是"离开末尾那一刻整段的行数"，由 [`BelowSeen`] 记。
    pub fn of(follow: bool, total: usize, top: usize, height: usize, seen: usize) -> Self {
        if follow || total <= top.saturating_add(height) {
            return Self::Nothing;
        }
        if total > seen {
            Self::New
        } else {
            Self::More
        }
    }

    /// 标记的文本；[`Below::Nothing`] 不画。
    pub fn marker(self) -> Option<&'static str> {
        match self {
            Self::Nothing => None,
            Self::More => Some(MORE_BELOW),
            Self::New => Some(NEW_OUTPUT),
        }
    }
}

/// 记"离开末尾那一刻整段的行数"，用来分辨 [`Below::More`] 和 [`Below::New`]。
///
/// 上游把这件事记在 `Scroll::held.grew` 里；我们的滚动状态在 `pane.rs`
/// （`PaneScroll`），所以本文件只提供判断与标记文本。
///
/// TODO(pane): `pane::window_transcript` 现在只会画 `↓ more below`（常量
/// `BELOW_MARKER`）。要画出 `↓ new output`，窗格得持有 [`BelowSeen`]、每帧
/// 用 `BelowSeen::of(follow, rows, top, height)` 取 [`Below`]，再拿
/// [`Below::marker`] 当右端标记 —— 改的是 `pane.rs`（不在本文件的改动范围内）。
#[derive(Clone, Copy, Debug, Default)]
pub struct BelowSeen {
    /// 离开末尾时整段的行数。
    seen: usize,
    /// 这一帧是否在滚动状态（上一帧在末尾）。
    scrolled: bool,
}

impl BelowSeen {
    /// 这一帧下面是什么。`follow` 是窗格是否跟着末尾。
    pub fn of(&mut self, follow: bool, total: usize, top: usize, height: usize) -> Below {
        if follow {
            self.scrolled = false;
            self.seen = total;
            return Below::Nothing;
        }
        if !self.scrolled {
            // 刚离开末尾：记下"已经看到哪里"，之后新到的行才算 new output。
            self.scrolled = true;
            self.seen = total;
        }
        let below = Below::of(false, total, top, height, self.seen);
        if below == Below::Nothing {
            self.scrolled = false;
        }
        below
    }
}

#[cfg(test)]
mod tests {
    //! 对话引擎的测试：全部用**构造的 [`RenderBatch`]**，不跑模型、不连网。
    //!
    //! 断言落在行文本与顺序上（行是值，这是比终端 buffer 更好断言的地方）。

    use super::*;
    use rig_core::message::{
        CallId, Text, ToolCall, ToolFunction, ToolName, ToolResult, ToolResultContent,
    };
    use rig_ecs::prelude::AsideId;
    use rig_tui::ratatui::text::Span;
    use rig_tui::{CallState, RenderInput, RenderOutput, ToolRender};

    /// 一个搜工具的调用。
    fn tool_call(name: &str, argument: &str) -> ToolCall {
        let name = ToolName::new(name).expect("工具名非空");
        ToolCall::new(
            CallId::from_wire("call_test"),
            ToolFunction::new(name, serde_json::json!({ "q": argument })),
        )
    }

    fn text_lines(lines: &[&str]) -> Vec<Line<'static>> {
        lines
            .iter()
            .map(|line| Line::from(line.to_string()))
            .collect()
    }

    /// 一个工具调用的请求：`lines` 是它的 look 输出。
    fn tool_request(name: &str, lines: &[&str]) -> RenderRequest {
        let call = ToolRender::new(tool_call(name, name), None, CallState::Running(None));
        RenderRequest {
            input: RenderInput::Tool(call),
            output: RenderOutput {
                lines: lines
                    .iter()
                    .map(|line| Line::from(line.to_string()).into())
                    .collect(),
                decorations: Vec::new(),
            },
        }
    }

    /// 一个 note 的请求（`RenderKey::Aside` 里装的是 `EntryRender`，上游也是这样）。
    fn note_request(title: &str, body: &[&str]) -> RenderRequest {
        let entry = rig_tui::EntryRender {
            id: None,
            plugin: "btw".into(),
            title: title.into(),
            text: body.join("\n"),
        };
        RenderRequest {
            input: RenderInput::Entry(entry),
            output: RenderOutput {
                lines: text_lines(&[title])
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                decorations: Vec::new(),
            },
        }
    }

    /// 一个内容块的请求：默认行是上游的 `[image]`（建批的人填默认行，look 再覆盖）。
    fn content_request() -> RenderRequest {
        let content = rig_tui::ContentRender {
            message: None,
            index: 2,
            content: rig_tui::RenderContent::Image(Default::default()),
            assistant: false,
        };
        RenderRequest {
            output: RenderOutput {
                lines: content.default_lines().into_iter().map(Into::into).collect(),
                decorations: Vec::new(),
            },
            input: RenderInput::Content(content),
        }
    }

    /// 一批请求 + 宽度。
    fn batch(width: u16, requests: Vec<(RenderKey, RenderRequest)>) -> RenderBatch {
        RenderBatch {
            pane: None,
            agent: None,
            width,
            requests: requests.into_iter().collect(),
        }
    }

    /// 一个 aside 的 id（`AsideId` 只给了 `FromStr` / `Default`，测试用 `a3` 这种写法）。
    fn aside_id(n: u64) -> AsideId {
        format!("a{n}").parse().expect("aside id")
    }

    #[test]
    fn rows_follow_the_conversation_order() {
        // 第 0 条消息：用户文字 + 一条送达的 entry + 一个内容块；
        // 第 1 条消息：assistant 正文 + 一个工具调用；最后是流式调用。
        let mut transcript = Transcript::default();
        transcript.set_width(60);
        transcript.apply(&batch(
            60,
            vec![
                (
                    RenderKey::Entry(0, 1),
                    note_request("compacted", &["summary"]),
                ),
                (
                    RenderKey::Content(0, 2),
                    content_request(),
                ),
                (RenderKey::Tool(1, 1), tool_request("search", &["search"])),
                (RenderKey::Streaming(0), tool_request("later", &["later"])),
            ],
        ));
        transcript.set_text(TextId::Message { at: 0, block: 0 }, text_lines(&["user"]));
        transcript.set_text(TextId::Message { at: 1, block: 0 }, text_lines(&["reply"]));
        transcript.set_text(TextId::Prompt(0), text_lines(&["typed"]));

        assert_eq!(
            transcript.texts(),
            vec![
                "user".to_owned(),
                "compacted".to_owned(),
                "  [image]".to_owned(),
                "reply".to_owned(),
                "search".to_owned(),
                "typed".to_owned(),
                "later".to_owned(),
            ],
            "顺序：消息 0、消息 1、本进程提示词、流式调用"
        );
    }

    #[test]
    fn a_streaming_call_finalizes_without_duplicating() {
        let mut transcript = Transcript::default();
        transcript.set_width(40);
        // 还在流：对话之外，用 `Streaming(0)`。
        transcript.apply(&batch(
            40,
            vec![(RenderKey::Streaming(0), tool_request("search", &["search"]))],
        ));
        assert_eq!(transcript.texts(), vec!["search".to_owned()]);

        // 落进对话：同一段文本换成 `Tool(3, 0)`，旧块必须消失而不是追加。
        transcript.apply(&batch(
            40,
            vec![(RenderKey::Tool(3, 0), tool_request("search", &["search"]))],
        ));
        assert_eq!(
            transcript.texts(),
            vec!["search".to_owned()],
            "finalize 之后只应有一份"
        );
        assert_eq!(transcript.block_count(), 1);
    }

    #[test]
    fn a_streaming_reply_grows_without_duplicating_or_dropping() {
        let mut transcript = Transcript::default();
        transcript.set_width(40);
        for text in ["你", "你好", "你好，世界"] {
            transcript.set_text(TextId::StreamingReply, text_lines(&[text]));
            assert_eq!(
                transcript.texts(),
                vec![text.to_owned()],
                "每次只画最后到达的全文"
            );
        }
        // 增补不换块身份：同一条流的第一行 key 不变，`panels` 的前缀 diff 还能原地改字。
        let first = transcript.keyed_rows()[0].key;
        transcript.set_text(TextId::StreamingReply, text_lines(&["你好，世界！"]));
        assert_eq!(transcript.keyed_rows()[0].key, first);
    }

    #[test]
    fn wrapping_uses_the_width_from_the_last_layout() {
        let mut transcript = Transcript::default();
        transcript.set_width(10);
        transcript.set_text(
            TextId::Message { at: 0, block: 0 },
            text_lines(&["hello world foo bar"]),
        );
        assert_eq!(
            transcript.texts(),
            vec!["hello", "world foo", "bar"],
            "10 列下按词边界折成三行"
        );
        // 宽度变了只重折，不重建：同一块的第一行 key 不变。
        let first = transcript.keyed_rows()[0].key;
        transcript.set_width(40);
        assert_eq!(transcript.texts(), vec!["hello world foo bar"]);
        assert_eq!(transcript.keyed_rows()[0].key, first);
    }

    #[test]
    fn an_aside_sits_after_the_message_it_follows() {
        let mut transcript = Transcript::default();
        transcript.set_width(40);
        let id = aside_id(1);
        transcript.apply(&batch(
            40,
            vec![
                (RenderKey::Tool(0, 0), tool_request("first", &["first"])),
                (RenderKey::Tool(1, 0), tool_request("second", &["second"])),
                (
                    RenderKey::Aside(id),
                    note_request("Why tabs?", &["The style says so."]),
                ),
            ],
        ));
        transcript.place(BlockId::Request(RenderKey::Aside(id)), Slot::After(0));
        assert_eq!(
            transcript.texts(),
            vec!["first".to_owned(), "Why tabs?".to_owned(), "second".to_owned()],
            "aside 落在那条消息的所有块之后、下一条消息之前"
        );

        // 换到所有消息之前。
        transcript.place(BlockId::Request(RenderKey::Aside(id)), Slot::Top);
        assert_eq!(
            transcript.texts(),
            vec!["Why tabs?".to_owned(), "first".to_owned(), "second".to_owned()]
        );

        // 空行集的 aside 不画（上游"没有行也没有装饰的 item 隐藏"）。
        transcript.remove(&BlockId::Request(RenderKey::Aside(id)));
        assert_eq!(transcript.block_count(), 2);
    }

    #[test]
    fn notices_are_drawn_where_they_are_placed() {
        let mut transcript = Transcript::default();
        transcript.set_width(40);
        transcript.apply(&batch(
            40,
            vec![
                (RenderKey::Tool(0, 1), tool_request("first", &["first"])),
                (RenderKey::Tool(1, 1), tool_request("second", &["second"])),
            ],
        ));
        transcript.set_text(TextId::Message { at: 0, block: 0 }, text_lines(&["user"]));
        transcript.set_text(TextId::Message { at: 1, block: 0 }, text_lines(&["reply"]));
        transcript.set_text(TextId::Notice(0), text_lines(&["top notice"]));
        transcript.set_text(TextId::Notice(1), text_lines(&["after notice"]));
        transcript.set_text(TextId::Notice(2), text_lines(&["inside notice"]));
        transcript.place(BlockId::Text(TextId::Notice(0)), Slot::Top);
        transcript.place(BlockId::Text(TextId::Notice(1)), Slot::After(0));
        transcript.place(
            BlockId::Text(TextId::Notice(2)),
            Slot::Inside { at: 0, item: 0 },
        );
        assert_eq!(
            transcript.texts(),
            vec![
                "top notice".to_owned(),
                "inside notice".to_owned(),
                "user".to_owned(),
                "first".to_owned(),
                "after notice".to_owned(),
                "reply".to_owned(),
                "second".to_owned(),
            ],
            "Top 在所有消息前，Inside 在消息内那块之前，After 在该消息所有块之后"
        );
    }

    #[test]
    fn decorations_follow_their_item() {
        let mut transcript = Transcript::default();
        transcript.set_width(40);
        let mut request = tool_request("spawn", &["spawn"]);
        request.output.decorate("children", text_lines(&["  ↳ helper"]));
        transcript.apply(&batch(40, vec![(RenderKey::Tool(0, 0), request)]));
        assert_eq!(transcript.texts(), vec!["spawn", "  ↳ helper"]);
    }

    #[test]
    fn no_width_draws_nothing_until_the_layout_comes() {
        let mut transcript = Transcript::default();
        transcript.apply(&batch(
            0,
            vec![(RenderKey::Tool(0, 0), tool_request("first", &["first"]))],
        ));
        assert!(transcript.texts().is_empty(), "宽度还没来时不出行");
        transcript.set_width(20);
        assert_eq!(transcript.texts(), vec!["first".to_owned()]);
    }

    #[test]
    fn bands_come_from_the_line_background() {
        let mut transcript = Transcript::default();
        transcript.set_width(40);
        let mut request = tool_request("first", &["first"]);
        request.output.lines = vec![Line::from("first")
            .style(rig_tui::ratatui::style::Style::new().bg(RColor::Rgb(1, 2, 3)))
            .into()];
        transcript.apply(&batch(40, vec![(RenderKey::Tool(0, 0), request)]));
        let rows = transcript.keyed_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].row.band, Some(RColor::Rgb(1, 2, 3)));
    }

    #[test]
    fn more_below_and_new_output_are_told_apart() {
        // 跟着末尾：没有标记。
        assert_eq!(Below::of(true, 100, 90, 10, 100), Below::Nothing);
        // 滚上去、下面还有上次就有的行：more below。
        assert_eq!(Below::of(false, 100, 20, 10, 100), Below::More);
        // 滚上去之后又长了：new output。
        assert_eq!(Below::of(false, 120, 20, 10, 100), Below::New);
        assert_eq!(Below::More.marker(), Some(MORE_BELOW));
        assert_eq!(Below::New.marker(), Some(NEW_OUTPUT));

        let mut seen = BelowSeen::default();
        assert_eq!(seen.of(true, 100, 90, 10), Below::Nothing);
        assert_eq!(seen.of(false, 100, 90, 10), Below::Nothing, "还没滚上去");
        assert_eq!(seen.of(false, 100, 20, 10), Below::More);
        assert_eq!(seen.of(false, 140, 20, 10), Below::New, "滚动之后新到的行");
    }

    #[test]
    fn a_tool_look_output_and_its_call_state_are_the_real_ones() {
        // 用真实 `ToolRender::default_lines()`（没有 look 注册时的上游默认 look）：
        // 标题、参数、结果的文本都来自 `ToolCall` / `ToolResult`，不是写死的。
        let call = tool_call("search", "跑鞋");
        let result = ToolResult {
            call: call.id.clone(),
            name: call.function.name.clone(),
            content: vec![ToolResultContent::Text(Text::new("3 products"))],
            is_error: false,
        };
        let render = ToolRender::new(call, Some(result), CallState::Done);
        let mut output = RenderOutput::default();
        output.lines = render.default_lines();
        let mut transcript = Transcript::default();
        transcript.set_width(80);
        transcript.apply(&batch(
            80,
            vec![(
                RenderKey::Tool(0, 0),
                RenderRequest {
                    input: RenderInput::Tool(render),
                    output,
                },
            )],
        ));
        let joined = transcript.texts().join("\n");
        assert!(joined.contains("search"), "标题是工具名：{joined}");
        assert!(joined.contains("跑鞋"), "参数来自 call：{joined}");
        assert!(joined.contains("3 products"), "结果来自 result：{joined}");
    }

    #[test]
    fn the_right_segment_and_choice_identity_survive_into_the_rows() {
        let mut transcript = Transcript::default();
        transcript.set_width(40);
        let mut request = tool_request("first", &["first"]);
        request.output.lines = vec![Line::from("first").into()];
        transcript.apply(&batch(40, vec![(RenderKey::Tool(0, 0), request)]));
        let row = Row::new(Line::from(vec![
            Span::styled("pick ", rig_tui::ratatui::style::Style::new()),
            Span::raw("me"),
        ]))
        .with_right(Line::from("PgDn"));
        assert_ne!(row_hash(&row), 0);
        assert!(row.right.is_some());
    }
}

#[cfg(test)]
mod span_style_tests {
    use super::*;
    use rig_tui::ratatui::style::{Modifier, Style};
    use rig_tui::ratatui::text::Span;

    fn luma(c: Color) -> f32 {
        let c = c.to_srgba();
        0.2126 * c.red + 0.7152 * c.green + 0.0722 * c.blue
    }

    #[test]
    fn bold_is_brighter_than_plain() {
        let theme = Theme::default();
        let plain = span_style_color(&Style::default().fg(RColor::Gray), &theme);
        let bold = span_style_color(
            &Style::default()
                .fg(RColor::Gray)
                .add_modifier(Modifier::BOLD),
            &theme,
        );
        assert!(luma(bold) > luma(plain), "BOLD 应该提亮");
    }

    #[test]
    fn dim_is_darker_than_plain() {
        let theme = Theme::default();
        let plain = span_style_color(&Style::default().fg(RColor::Gray), &theme);
        let dim = span_style_color(
            &Style::default()
                .fg(RColor::Gray)
                .add_modifier(Modifier::DIM),
            &theme,
        );
        assert!(luma(dim) < luma(plain), "DIM 应该压暗（picker 的 patch_style(dim) 靠这条）");
    }

    #[test]
    fn underlined_keeps_the_foreground_and_asks_for_the_component() {
        let theme = Theme::default();
        let plain = Style::default().fg(RColor::Gray);
        let under = plain.add_modifier(Modifier::UNDERLINED);
        // 下划线不再折成颜色：前景和普通文字一样，线由 `Underline` 组件真画。
        assert_eq!(
            span_style_color(&under, &theme),
            span_style_color(&plain, &theme)
        );
        let paint = span_paint(&under, &theme);
        assert!(paint.underline, "UNDERLINED 要挂 Underline 组件");
        assert!(!paint.strikethrough);
        assert_eq!(paint.bg, None, "下划线不是底色");
    }

    #[test]
    fn span_background_keeps_the_foreground_and_is_painted_as_a_component() {
        let theme = Theme::default();
        let plain = Style::default().fg(RColor::Gray);
        let tinted = plain.bg(RColor::Blue);
        // 底色自成一个组件，不再把前景提亮/压暗。
        assert_eq!(
            span_style_color(&tinted, &theme),
            span_style_color(&plain, &theme)
        );
        let paint = span_paint(&tinted, &theme);
        assert_eq!(paint.bg, Some(style::color(Some(RColor::Blue))));
        assert!(!paint.underline);
        assert!(!paint.strikethrough);
    }

    /// 生成一个只装这一行的世界（不建窗口、不跑渲染），返回行实体。
    fn spawn_row_world(row: &Row) -> (World, Entity) {
        let font = FontSource::default();
        let theme = Theme::default();
        let mut world = World::new();
        let mut row_entity = Entity::PLACEHOLDER;
        {
            let mut commands = world.commands();
            commands.spawn_empty().with_children(|parent| {
                row_entity = spawn_row_themed(parent, row, &font, &theme);
            });
        }
        world.flush();
        (world, row_entity)
    }

    /// 一个 span 实体上真正挂着的组件（值，不是引用）。
    #[derive(Debug, PartialEq)]
    struct PaintedSpan {
        fg: Color,
        bg: Option<Color>,
        underline: Option<Color>,
        strikethrough: Option<Color>,
    }

    /// 世界里所有 span 的 `(文本, 组件)`。
    fn painted_spans(world: &mut World) -> Vec<(String, PaintedSpan)> {
        let mut query = world.query::<(
            &TextSpan,
            &TextColor,
            Option<&TextBackgroundColor>,
            Option<&Underline>,
            Option<&UnderlineColor>,
            Option<&Strikethrough>,
            Option<&StrikethroughColor>,
        )>();
        query
            .iter(world)
            .map(
                |(span, color, bg, underline, underline_color, strike, strike_color)| {
                    (
                        span.0.clone(),
                        PaintedSpan {
                            fg: color.0,
                            bg: bg.map(|bg| bg.0),
                            underline: underline.map(|_| {
                                underline_color.expect("Underline 必须带线色").0
                            }),
                            strikethrough: strike.map(|_| {
                                strike_color.expect("Strikethrough 必须带线色").0
                            }),
                        },
                    )
                },
            )
            .collect()
    }

    /// 文本为 `text` 的那个 span 的组件。
    fn painted(world: &mut World, text: &str) -> PaintedSpan {
        let spans = painted_spans(world);
        spans
            .iter()
            .find(|(span, _)| span == text)
            .map(|(_, paint)| PaintedSpan {
                fg: paint.fg,
                bg: paint.bg,
                underline: paint.underline,
                strikethrough: paint.strikethrough,
            })
            .unwrap_or_else(|| {
                let all: Vec<&str> = spans.iter().map(|(span, _)| span.as_str()).collect();
                panic!("没有 span {text:?}：{all:?}")
            })
    }

    #[test]
    fn a_span_background_is_a_component_on_the_span() {
        let want = RColor::Rgb(1, 2, 3);
        let row = Row::new(Line::from(Span::styled(
            "tinted",
            Style::default().fg(RColor::Green).bg(want),
        )));
        let (mut world, _) = spawn_row_world(&row);
        let span = painted(&mut world, "tinted");
        assert_eq!(span.bg, Some(style::color(Some(want))));
        // 前景仍然是原来的前景色，没有被底色提亮/压暗。
        assert_eq!(span.fg, style::color(Some(RColor::Green)));
    }

    #[test]
    fn an_underlined_span_gets_the_underline_component() {
        let row = Row::new(Line::from(Span::styled(
            "under",
            Style::default()
                .fg(RColor::Green)
                .add_modifier(Modifier::UNDERLINED),
        )));
        let (mut world, _) = spawn_row_world(&row);
        let span = painted(&mut world, "under");
        assert_eq!(span.underline, Some(span.fg), "下划线颜色取前景色");
        assert_eq!(span.bg, None);
    }

    #[test]
    fn a_crossed_out_span_gets_the_strikethrough_component() {
        let row = Row::new(Line::from(Span::styled(
            "gone",
            Style::default()
                .fg(RColor::LightRed)
                .add_modifier(Modifier::CROSSED_OUT),
        )));
        let (mut world, _) = spawn_row_world(&row);
        let span = painted(&mut world, "gone");
        assert_eq!(span.strikethrough, Some(span.fg), "删除线颜色取前景色");
        assert_eq!(span.underline, None);
        assert_eq!(span.bg, None);
    }

    #[test]
    fn a_reversed_span_swaps_the_foreground_and_the_background() {
        let theme = Theme::default();
        // span 自带底色：前景换底色，底色换原本的前景。
        let row = Row::new(Line::from(Span::styled(
            "swap",
            Style::default()
                .fg(RColor::Red)
                .bg(RColor::Blue)
                .add_modifier(Modifier::REVERSED),
        )));
        let (mut world, _) = spawn_row_world(&row);
        let span = painted(&mut world, "swap");
        assert_eq!(span.fg, style::color(Some(RColor::Blue)), "前景 = 原本的底色");
        assert_eq!(
            span.bg,
            Some(style::color(Some(RColor::Red))),
            "底色 = 原本的前景"
        );

        // 没有 span 底色时，前景换成主题 band、底色换成原本的前景。
        let row = Row::new(Line::from(Span::styled(
            "theme swap",
            Style::default()
                .fg(RColor::Red)
                .add_modifier(Modifier::REVERSED),
        )));
        let (mut world, _) = spawn_row_world(&row);
        let span = painted(&mut world, "theme swap");
        assert_eq!(span.fg, style::color(Some(theme.band)));
        assert_eq!(span.bg, Some(style::color(Some(RColor::Red))));
    }

    #[test]
    fn a_plain_span_has_no_background_component() {
        let (mut world, _) = spawn_row_world(&Row::new(Line::from("plain text")));
        let span = painted(&mut world, "plain text");
        assert_eq!(
            span.bg, None,
            "普通 span 不能带 TextBackgroundColor（默认黑底会刷满整行）"
        );
        assert_eq!(span.underline, None);
        assert_eq!(span.strikethrough, None);
    }

    #[test]
    fn row_text_stays_plain_text_after_span_decorations() {
        // 选择/复制读的是 RowText：加了底色/下划线/反色之后它仍必须是纯文本。
        let row = Row::new(Line::from(vec![
            Span::raw("hello "),
            Span::styled(
                "world",
                Style::default()
                    .bg(RColor::Blue)
                    .add_modifier(Modifier::REVERSED | Modifier::UNDERLINED),
            ),
            Span::styled(
                "!",
                Style::default().add_modifier(Modifier::CROSSED_OUT),
            ),
        ]))
        .with_right(Line::from(" PgDn"));
        let (world, row_entity) = spawn_row_world(&row);
        assert_eq!(
            world.get::<RowText>(row_entity).expect("RowText").0,
            "hello world! PgDn"
        );
    }

    #[test]
    fn a_colour_survives_verbatim_when_nothing_is_asked_for() {
        let theme = Theme::default();
        let c = span_style_color(&Style::default().fg(RColor::Red), &theme);
        let want = style::color(Some(RColor::Red)).to_srgba();
        let got = c.to_srgba();
        assert!((want.red - got.red).abs() < 1e-4);
        assert!((want.green - got.green).abs() < 1e-4);
        assert!((want.blue - got.blue).abs() < 1e-4);
    }

    #[test]
    fn reversed_swaps_the_foreground_for_the_background() {
        let theme = Theme::default();
        // 没有 span 底色时换成主题底色（选中就是"挖空"）。
        let reversed = span_style_color(
            &Style::default()
                .fg(RColor::Red)
                .add_modifier(Modifier::REVERSED),
            &theme,
        )
        .to_srgba();
        let band = style::color(Some(theme.band)).to_srgba();
        assert!((band.red - reversed.red).abs() < 1e-4);
        assert!((band.green - reversed.green).abs() < 1e-4);
        assert!((band.blue - reversed.blue).abs() < 1e-4);

        // span 自带底色时换它（终端反色换的就是这个背景）。
        let on_blue = span_style_color(
            &Style::default()
                .fg(RColor::Red)
                .bg(RColor::Blue)
                .add_modifier(Modifier::REVERSED),
            &theme,
        )
        .to_srgba();
        let blue = style::color(Some(RColor::Blue)).to_srgba();
        assert!((blue.red - on_blue.red).abs() < 1e-4);
        assert!((blue.green - on_blue.green).abs() < 1e-4);
        assert!((blue.blue - on_blue.blue).abs() < 1e-4);
    }
}
