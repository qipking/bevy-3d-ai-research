//! 主题 token：界面里所有颜色都从这里取，换主题只改这一处。
//!
//! rig-tui 有 `theme.rs`（ratatui 颜色 + 面板底色）；这里对应的是一组
//! `Style` 常量，look 和模板面板都用它们，保证终端观感一致。

use bevy::prelude::*;
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};

#[derive(Clone, Copy, Debug, Resource)]
pub struct Theme {
    /// 主文字
    pub fg: Style,
    /// 次要文字（列名、说明）
    pub dim: Style,
    /// 强调（工具名、标题）
    pub accent: Style,
    /// 成功/价格
    pub ok: Style,
    /// 警告/评分
    pub warn: Style,
    /// 错误
    pub err: Style,
    /// 助手段落
    pub assistant: Style,
    /// 用户段落
    pub user: Style,
    /// 面板色带
    pub band: RColor,
    /// 输入行提示符
    pub prompt: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            fg: Style::default().fg(RColor::Rgb(210, 216, 228)),
            dim: Style::default().fg(RColor::Rgb(120, 128, 145)),
            accent: Style::default()
                .fg(RColor::Yellow)
                .add_modifier(Modifier::BOLD),
            ok: Style::default().fg(RColor::Green),
            warn: Style::default().fg(RColor::LightYellow),
            err: Style::default().fg(RColor::LightRed),
            assistant: Style::default().fg(RColor::Cyan),
            user: Style::default().fg(RColor::Cyan),
            band: RColor::Rgb(20, 26, 38),
            prompt: Style::default()
                .fg(RColor::Green)
                .add_modifier(Modifier::BOLD),
        }
    }
}
