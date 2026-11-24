//! ratatui style/color -> Bevy color, so rig-tui's looks keep their semantics.

use bevy::prelude::*;

pub fn color(c: Option<rig_tui::ratatui::style::Color>) -> Color {
    use rig_tui::ratatui::style::Color as R;
    let c = match c {
        Some(c) => c,
        None => R::Reset,
    };
    match c {
        R::Reset => Color::srgb(0.88, 0.90, 0.94),
        R::Black => Color::srgb(0.05, 0.05, 0.06),
        R::Red => Color::srgb(0.90, 0.35, 0.35),
        R::Green => Color::srgb(0.40, 0.85, 0.45),
        R::Yellow => Color::srgb(0.95, 0.82, 0.35),
        R::Blue => Color::srgb(0.40, 0.62, 0.95),
        R::Magenta => Color::srgb(0.85, 0.45, 0.90),
        R::Cyan => Color::srgb(0.40, 0.85, 0.88),
        R::Gray => Color::srgb(0.55, 0.57, 0.62),
        R::DarkGray => Color::srgb(0.33, 0.35, 0.40),
        R::LightRed => Color::srgb(1.0, 0.55, 0.55),
        R::LightGreen => Color::srgb(0.65, 0.95, 0.65),
        R::LightYellow => Color::srgb(1.0, 0.93, 0.60),
        R::LightBlue => Color::srgb(0.62, 0.80, 1.0),
        R::LightMagenta => Color::srgb(1.0, 0.70, 1.0),
        R::LightCyan => Color::srgb(0.65, 0.98, 1.0),
        R::White => Color::srgb(0.97, 0.98, 1.0),
        R::Rgb(r, g, b) => Color::srgb_u8(r, g, b),
        R::Indexed(i) => indexed(i),
    }
}

fn indexed(i: u8) -> Color {
    // xterm 256 的 16 基色 + 6x6x6 立方体 + 灰阶
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0), (128, 0, 0), (0, 128, 0), (128, 128, 0),
        (0, 0, 128), (128, 0, 128), (0, 128, 128), (192, 192, 192),
        (128, 128, 128), (255, 0, 0), (0, 255, 0), (255, 255, 0),
        (0, 0, 255), (255, 0, 255), (0, 255, 255), (255, 255, 255),
    ];
    let (r, g, b) = if i < 16 {
        BASE[i as usize]
    } else if i < 232 {
        let n = i - 16;
        let steps = [0u8, 95, 135, 175, 215, 255];
        (steps[(n / 36) as usize], steps[((n / 6) % 6) as usize], steps[(n % 6) as usize])
    } else {
        let v = 8 + 10 * (i - 232);
        (v, v, v)
    };
    Color::srgb_u8(r, g, b)
}
