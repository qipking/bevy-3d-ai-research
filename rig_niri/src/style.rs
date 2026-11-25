//! ratatui style/color -> Bevy color, so rig-tui's looks keep their semantics.

use bevy::prelude::*;
use rig_tui::ratatui::style::Color as RColor;

/// ratatui 颜色 → xterm 的 RGB。
///
/// 这条通路和 [`color`] 分开：`color` 是"给 Bevy 画布挑一个好看的近似色"，
/// 命名色用的是本前端自己的调色板；这里要的是"终端里那个色相"，命名色按
/// xterm 的 16 基色取值，供主题明暗判断、混色和 `NO_COLOR` 的明度计算用。
/// `Reset` 没有色相，返回 `None`（调用方保留自己的兜底）。
pub fn rgb(color: RColor) -> Option<(u8, u8, u8)> {
    // xterm 的 16 个基础色（0–15），和 [`color`] 的 `indexed` 同一张表。
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match color {
        RColor::Reset => None,
        RColor::Black => Some(BASE[0]),
        RColor::Red => Some(BASE[1]),
        RColor::Green => Some(BASE[2]),
        RColor::Yellow => Some(BASE[3]),
        RColor::Blue => Some(BASE[4]),
        RColor::Magenta => Some(BASE[5]),
        RColor::Cyan => Some(BASE[6]),
        RColor::Gray => Some(BASE[7]),
        RColor::DarkGray => Some(BASE[8]),
        RColor::LightRed => Some(BASE[9]),
        RColor::LightGreen => Some(BASE[10]),
        RColor::LightYellow => Some(BASE[11]),
        RColor::LightBlue => Some(BASE[12]),
        RColor::LightMagenta => Some(BASE[13]),
        RColor::LightCyan => Some(BASE[14]),
        RColor::White => Some(BASE[15]),
        RColor::Rgb(r, g, b) => Some((r, g, b)),
        RColor::Indexed(index) => Some(match index {
            0..=15 => BASE[usize::from(index)],
            16..=231 => {
                // 6×6×6 立方体，每级 0/95/135/175/215/255。
                const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
                let n = index - 16;
                (
                    STEPS[usize::from(n / 36)],
                    STEPS[usize::from(n / 6 % 6)],
                    STEPS[usize::from(n % 6)],
                )
            }
            // 232–255 是 8 起步、间隔 10 的灰阶。
            index => {
                let level = 8 + 10 * (index - 232);
                (level, level, level)
            }
        }),
    }
}

pub fn color(c: Option<RColor>) -> Color {
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
