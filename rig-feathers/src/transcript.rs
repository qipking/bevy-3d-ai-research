//! The feathers side of the rig-tui adapter: `LookLine` rows -> bevy_ui nodes.
//!
//! rig-tui's looks produce `Vec<LookLine>`; `LookLine::colored()` is the public
//! API that gives back a styled `ratatui::text::Line` (spans with `Style`).
//! This module turns those lines into a column of `Node`s with a `Text` +
//! `TextSpan` chain, which the 3D mirror / glyph bridge then places in the world.

use bevy::prelude::*;
use bevy::picking::Pickable;
use bevy::text::{FontSource, LineBreak, TextLayout};
use bevy_ui::{BorderRadius, Node, percent, px};
use rig_tui::LookLine;
use rig_tui::ratatui::style::Color as RColor;
use rig_tui::ratatui::text::Line;

use crate::style;

/// The row's plain text, for selection/copy.
#[derive(Component, Clone, Debug)]
pub struct RowText(pub String);

pub const ROW_HEIGHT: f32 = 18.0;
pub const FONT_SIZE: f32 = 13.0;

/// One transcript row: the styled line plus an optional band color behind it.
///
/// `right` is a second line aligned to the row's right edge (rig-tui draws the
/// status line's right-hand items, and a scrolled pane's "more below" marker,
/// the same way: at the right of a row).
#[derive(Clone, Debug)]
pub struct Row {
    pub line: Line<'static>,
    /// A line at the right end of the row, on the same row.
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

    /// The same row, `right` aligned to its right edge.
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
    /// From rig-tui's look output — this is the adapter seam.
    pub fn from_look(line: &LookLine) -> Self {
        Self::new(line.colored())
    }
}

/// 在一行节点里（重新）生成文字节点与 span —— 流式更新时只换这一行的内容。
pub fn fill_row_text(
    commands: &mut Commands,
    row_entity: Entity,
    row: &Row,
    font: &FontSource,
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
        spawn_text_line(parent, &row.line, &font);
        if let Some(right) = &row.right {
            spawn_text_line(parent, right, &font);
        }
    });
}

/// 转写行用的字体（FiraMono + Noto CJK 回退链）。
#[derive(Resource, Clone)]
pub struct TranscriptFont(pub FontSource);

/// 生成一行 UI 节点，返回该行实体（增量更新需要它）。
pub fn spawn_row(parent: &mut ChildSpawnerCommands, row: &Row, font: &FontSource) -> Entity {
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
        plain.push_str(&right.spans.iter().map(|s| s.content.as_ref()).collect::<String>());
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
        spawn_text_line(parent, &row.line, &font);
        if let Some(right) = &row.right {
            spawn_text_line(parent, right, &font);
        }
    });
    entity.id()
}

/// Spawns one styled line's text node (an empty `Text` with one `TextSpan` per run).
fn spawn_text_line(parent: &mut ChildSpawnerCommands, line: &Line<'static>, font: &FontSource) {
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
                // 字体必须显式给：TextSpan 隐式要求的 TextFont 是 20px 默认字体，
                // 不显式给会换字体、换图集（见 feathers_in_3d/README.md）。
                text.spawn((
                    TextSpan::new(span.content.clone()),
                    TextFont {
                        font: font.clone(),
                        font_size: FontSize::Px(FONT_SIZE),
                        ..default()
                    },
                    TextColor(color_with_alpha(span.style.fg.unwrap_or(RColor::Reset), 1.0)),
                ));
            }
        });
}

/// Spawns `rows` as a column of UI nodes under `parent`.
pub fn spawn_rows(parent: &mut ChildSpawnerCommands, rows: &[Row], font: &FontSource) -> Vec<Entity> {
    rows.iter().map(|row| spawn_row(parent, row, font)).collect()
}

fn color_with_alpha(c: RColor, a: f32) -> Color {
    let base = style::color(Some(c));
    base.with_alpha(a)
}
