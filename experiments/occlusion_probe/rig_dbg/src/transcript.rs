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
#[derive(Clone, Debug)]
pub struct Row {
    pub line: Line<'static>,
    pub band: Option<RColor>,
}

impl Row {
    pub fn new(line: Line<'static>) -> Self {
        Self { line, band: None }
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

/// Spawns `rows` as a column of UI nodes under `parent`.
pub fn spawn_rows(parent: &mut ChildSpawnerCommands, rows: &[Row], font: &FontSource) {
    let font = font.clone();
    for row in rows {
        let mut entity = parent.spawn(Node {
            width: percent(100.0),
            height: px(ROW_HEIGHT),
            // 行高固定：不让 flex 压缩（否则面板变矮时行会被挤到一起而不是溢出）
            flex_shrink: 0.0,
            align_items: AlignItems::Center,
            border_radius: BorderRadius::all(px(3.0)),
            ..default()
        });
        if let Some(band) = row.band {
            entity.insert(BackgroundColor(color_with_alpha(band, 0.55)));
        }
        let plain: String = row.line.spans.iter().map(|s| s.content.as_ref()).collect();
        entity.insert((RowText(plain), Pickable::default()));
        entity.with_children(|line| {
            line.spawn((
                Text::new(String::new()),
                TextFont {
                    font: font.clone(),
                    font_size: FontSize::Px(FONT_SIZE),
                    ..default()
                },
                TextColor(color_with_alpha(RColor::Reset, 1.0)),
                TextLayout { linebreak: LineBreak::NoWrap, ..default() },
            ))
            .with_children(|text| {
                for span in &row.line.spans {
                    // NOTE: no per-span `TextFont` here. A span that overrides the font
                    // moves the glyph into another font atlas, and the glyph bridge emits
                    // one mesh per atlas — so a mixed-atlas `Text` would only keep the last
                    // atlas. Styling stays color/alpha until the bridge can merge atlases.
                    // The font must be explicit: `TextSpan` *requires* a `TextFont`, and
                    // an implicit one is the 20 px default font, which both changes the
                    // glyph size and moves the glyph into a second atlas.
                    text.spawn((
                        TextSpan::new(span.content.clone()),
                        TextFont {
                            font: font.clone(),
                            font_size: FontSize::Px(FONT_SIZE),
                            ..default()
                        },
                        TextColor(color_with_alpha(
                            span.style.fg.unwrap_or(RColor::Reset),
                            1.0,
                        )),
                    ));
                }
            });
        });
    }
}

fn color_with_alpha(c: RColor, a: f32) -> Color {
    let base = style::color(Some(c));
    base.with_alpha(a)
}
