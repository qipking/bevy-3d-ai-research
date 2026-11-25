//! 主题：我们的调色板 + 上游 `rig_tui::theme::Theme` 的 17 个语义 band。
//!
//! 上游的主题不是"前景色 token"，而是**每种东西的底色**：用户写的、工具调用、
//! diff 的增删行与改动词、选中项、面板/弹层、输入行、调用的四种状态、代码块、
//! 提示块。`rich::upstream_theme` 上一版拿我们的前景色往底色里混，混出来的
//! 色相和上游语义对不上；现在 17 个 band 都收进本结构，成为唯一来源。
//!
//! band 用 `Style::bg` 存放（和本文件的其他 token 一样是 `Style`），这样
//! [`Theme::without_color`] 能把所有前景/底色一起清掉；`rig_tui` 要的是
//! `Color`，由 `rich::upstream_theme` 取 `.bg` 直接映射。
//!
//! 默认值就是上一版混色的结果（3D 深色底 `(20, 26, 38)` 上混出来的那组颜色），
//! 所以视觉上不回退；区别只在于现在是**显式值**，不再依赖前景色和混合比例。
//!
//! | 上游 `rig_tui::theme::Theme` | 这里 | 默认值 |
//! |---|---|---|
//! | `user` | [`Theme::user`] | `(18, 38, 49)` |
//! | `tool` | [`Theme::tool`] | `(31, 37, 49)` |
//! | `added` | [`Theme::added`] | `(17, 42, 32)` |
//! | `added_word` | [`Theme::added_word`] | `(12, 65, 24)` |
//! | `removed` | [`Theme::removed`] | `(58, 22, 32)` |
//! | `removed_word` | [`Theme::removed_word`] | `(109, 16, 24)` |
//! | `selected` | [`Theme::selected`] | `(91, 95, 27)` |
//! | `panel_surface` | [`Theme::panel_surface`] | `(29, 35, 47)` |
//! | `raised_surface` | [`Theme::raised_surface`] | `(69, 74, 84)` |
//! | `input` | [`Theme::input`] | `(60, 65, 75)` |
//! | `call_waiting` | [`Theme::call_waiting`] | `(44, 49, 60)` |
//! | `call_running` | [`Theme::call_running`] | `(53, 58, 33)` |
//! | `call_done` | [`Theme::call_done`] | `(18, 36, 34)` |
//! | `call_failed` | [`Theme::call_failed`] | `(76, 20, 29)` |
//! | `code` | [`Theme::code`] | `(39, 44, 55)` |
//! | `note` | [`Theme::note`] | `(17, 42, 52)` |
//! | `important` | [`Theme::important`] | `(62, 67, 31)` |
//! | `plain` | [`Theme::plain`] | `false` |
//! | `background`（私有） | [`Theme::band`] | `(20, 26, 38)` |
//! | `truecolor`（私有） | [`Theme::truecolor`] | `true` |
//!
//! 上游还有 `on`/`shade`/`without_color`/`selection`/`changed_word`/`panel`/
//! `raised`/`of_terminal`。这里对应 [`Theme::shade`]、[`Theme::without_color`]、
//! [`Theme::selection`]、[`Theme::changed_word`]、[`Theme::panel`]、
//! [`Theme::raised`]；`on`/`of_terminal` 要问终端，3D 前端没有终端，换成
//! [`Theme::from_env`]（只认 `NO_COLOR`）。

use std::ffi::OsStr;

use bevy::prelude::*;
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style, Stylize};
use rig_tui::ratatui::text::{Line, Span};
use rig_tui::ratatui::widgets::{Block, Padding};

use crate::style::rgb;

/// 上游 `rig_tui::theme::Theme` 的 17 个 band 字段名，顺序同上游文档。
///
/// [`Theme::bands`] 按同一顺序返回本结构的字段，测试用它做一一对应；不用反射。
pub const UPSTREAM_BANDS: [&str; 17] = [
    "user",
    "tool",
    "added",
    "added_word",
    "removed",
    "removed_word",
    "selected",
    "panel_surface",
    "raised_surface",
    "input",
    "call_waiting",
    "call_running",
    "call_done",
    "call_failed",
    "code",
    "note",
    "important",
];

/// 一个只有一个底色的 band。
const fn band(r: u8, g: u8, b: u8) -> Style {
    Style::new().bg(RColor::Rgb(r, g, b))
}

#[derive(Clone, Copy, Debug, Resource)]
pub struct Theme {
    // ---- 我们的调色板（前景 token） ----
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
    /// 面板色带 / 3D 深色底（上游的 `background`）
    pub band: RColor,
    /// 输入行提示符
    pub prompt: Style,

    // ---- 上游 `rig_tui::theme::Theme` 的 17 个语义 band ----
    /// 用户写的那块（上游 `user`）
    pub user: Style,
    /// 工具调用/结果、插件送进对话的文字（上游 `tool`）
    pub tool: Style,
    /// diff 的增行整行（上游 `added`）
    pub added: Style,
    /// diff 增行里改动的词（上游 `added_word`）
    pub added_word: Style,
    /// diff 的删行整行（上游 `removed`）
    pub removed: Style,
    /// diff 删行里改动的词（上游 `removed_word`）
    pub removed_word: Style,
    /// 选中项（上游 `selected`）
    pub selected: Style,
    /// 转写旁边面板的表面（上游 `panel_surface`）
    pub panel_surface: Style,
    /// 盖在屏幕上的弹层表面（上游 `raised_surface`）
    pub raised_surface: Style,
    /// 输入行（上游 `input`）
    pub input: Style,
    /// 调用的排队/被按住状态（上游 `call_waiting`）
    pub call_waiting: Style,
    /// 调用运行中（上游 `call_running`）
    pub call_running: Style,
    /// 调用已完成（上游 `call_done`）
    pub call_done: Style,
    /// 调用失败（上游 `call_failed`）
    pub call_failed: Style,
    /// 回复里的代码：围栏块与行内 code（上游 `code`）
    pub code: Style,
    /// 提示块 `> [!NOTE]`（上游 `note`）
    pub note: Style,
    /// `> [!IMPORTANT]`（上游 `important`）
    pub important: Style,

    /// 上游 `plain`：`NO_COLOR` 的无色版；不画任何前景/底色，用修饰符表达语义。
    pub plain: bool,
    /// 上游私有的 `truecolor`：默认 `true`（我们的 token 是 RGB/命名色，不退到
    /// 256 色板；这是 3D 深色主题，不改成 ANSI 16 色）。
    pub truecolor: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            // 原有可见前景色：逐项保持（测试里钉了旧值）。
            fg: Style::default().fg(RColor::Rgb(210, 216, 228)),
            dim: Style::default().fg(RColor::Rgb(120, 128, 145)),
            accent: Style::default()
                .fg(RColor::Yellow)
                .add_modifier(Modifier::BOLD),
            ok: Style::default().fg(RColor::Green),
            warn: Style::default().fg(RColor::LightYellow),
            err: Style::default().fg(RColor::LightRed),
            assistant: Style::default().fg(RColor::Cyan),
            band: RColor::Rgb(20, 26, 38),
            prompt: Style::default()
                .fg(RColor::Green)
                .add_modifier(Modifier::BOLD),

            // 17 个 band：上一版 `rich::upstream_theme` 在原底色上混出的值，
            // 现在固化成显式默认，观感不回退。
            user: band(18, 38, 49),
            tool: band(31, 37, 49),
            added: band(17, 42, 32),
            added_word: band(12, 65, 24),
            removed: band(58, 22, 32),
            removed_word: band(109, 16, 24),
            selected: band(91, 95, 27),
            panel_surface: band(29, 35, 47),
            raised_surface: band(69, 74, 84),
            input: band(60, 65, 75),
            call_waiting: band(44, 49, 60),
            call_running: band(53, 58, 33),
            call_done: band(18, 36, 34),
            call_failed: band(76, 20, 29),
            code: band(39, 44, 55),
            note: band(17, 42, 52),
            important: band(62, 67, 31),

            plain: false,
            truecolor: true,
        }
    }
}

impl Theme {
    /// 17 个 band，按 [`UPSTREAM_BANDS`] 的顺序给出 `(上游字段名, 本结构的值)`。
    ///
    /// 给测试和"一一对上游"的检查用，也是这一组字段的公开清单。
    pub fn bands(&self) -> [(&'static str, Style); 17] {
        [
            ("user", self.user),
            ("tool", self.tool),
            ("added", self.added),
            ("added_word", self.added_word),
            ("removed", self.removed),
            ("removed_word", self.removed_word),
            ("selected", self.selected),
            ("panel_surface", self.panel_surface),
            ("raised_surface", self.raised_surface),
            ("input", self.input),
            ("call_waiting", self.call_waiting),
            ("call_running", self.call_running),
            ("call_done", self.call_done),
            ("call_failed", self.call_failed),
            ("code", self.code),
            ("note", self.note),
            ("important", self.important),
        ]
    }

    /// 所有 `Style` 字段（调色板 + 17 个 band），可变，供 [`Theme::without_color`]
    /// 和测试统一遍历。新增 token 时记得加进来。
    fn styles_mut(&mut self) -> [&mut Style; 25] {
        [
            &mut self.fg,
            &mut self.dim,
            &mut self.accent,
            &mut self.ok,
            &mut self.warn,
            &mut self.err,
            &mut self.assistant,
            &mut self.prompt,
            &mut self.user,
            &mut self.tool,
            &mut self.added,
            &mut self.added_word,
            &mut self.removed,
            &mut self.removed_word,
            &mut self.selected,
            &mut self.panel_surface,
            &mut self.raised_surface,
            &mut self.input,
            &mut self.call_waiting,
            &mut self.call_running,
            &mut self.call_done,
            &mut self.call_failed,
            &mut self.code,
            &mut self.note,
            &mut self.important,
        ]
    }

    /// 无色版（上游 `without_color`，`:191`）。
    ///
    /// 清掉所有 `Style` 的 fg/bg，并把 [`Theme::plain`] 置真；`bold`/`underline`/
    /// `reverse` 这些修饰符保留，语义改由 [`Theme::selection`]、
    /// [`Theme::changed_word`] 和上游 markdown 的 `plain` 分支表达。
    ///
    /// [`Theme::band`] 是 3D 场景的底色（上游私有的 `background`），不是文字色；
    /// 上游的 `without_color` 也保留它，这里同样保留——渲染层要靠它画反色/面板。
    pub fn without_color(mut self) -> Self {
        for style in self.styles_mut() {
            *style = colorless(*style);
        }
        self.plain = true;
        self
    }

    /// 选中项的样子：`selected` band；无色版用反色（上游 `selection`，`:200`）。
    pub fn selection(&self) -> Style {
        match self.plain {
            true => Style::new().add_modifier(Modifier::REVERSED),
            false => Style::new().bg(self.selected.bg.unwrap_or(self.band)),
        }
    }

    /// diff 里改动的词：`tint` 底色；无色版用下划线（上游 `changed_word`，`:210`）。
    pub fn changed_word(&self, tint: RColor) -> Style {
        match self.plain {
            true => Style::new().add_modifier(Modifier::UNDERLINED),
            false => Style::new().bg(tint),
        }
    }

    /// 从 [`Theme::band`] 朝远端明度走 `by`（0..=1）的一级灰
    /// （上游 `shade`，`:175`）；`truecolor=false` 时落到 256 色的灰阶。
    pub fn shade(&self, by: f32) -> RColor {
        let background = rgb(self.band).unwrap_or((20, 26, 38));
        let toward = match lightness(background) < 0.5 {
            true => (0xff, 0xff, 0xff),
            false => (0, 0, 0),
        };
        let (r, g, b) = blend(background, toward, by.clamp(0.0, 1.0));
        if self.truecolor {
            return RColor::Rgb(r, g, b);
        }
        // 256 的灰阶是 232（8）到 255（238），间隔 10。
        let level = lightness((r, g, b)) * 255.0;
        let step = ((level - 8.0) / 10.0).round().clamp(0.0, 23.0) as u8;
        RColor::Indexed(232 + step)
    }

    /// 一块面板：`panel_surface` 平涂 + 首行粗体标题，留出一格边框的 padding
    /// （上游 `panel`，`:221`）。
    pub fn panel<'a>(&self, title: impl Into<Line<'a>>) -> Block<'a> {
        surface(self.surface_band(self.panel_surface), title.into(), self.plain)
    }

    /// 盖在屏幕上的弹层：同 [`Theme::panel`]，表面是 `raised_surface`
    /// （上游 `raised`，`:227`）。
    pub fn raised<'a>(&self, title: impl Into<Line<'a>>) -> Block<'a> {
        surface(self.surface_band(self.raised_surface), title.into(), self.plain)
    }

    /// 一块表面的底色：无色版用 `Reset`（终端默认底 = 不画），和上游在 plain
    /// 时统一 `strip_colors` 的效果一致。
    fn surface_band(&self, style: Style) -> RColor {
        match self.plain {
            true => RColor::Reset,
            false => style.bg.unwrap_or(self.band),
        }
    }

    /// 上游 `of_terminal`（`:234`）的对应物。
    ///
    /// 3D 前端没有终端可问底色，所以只认 `NO_COLOR`：设了非空值就是无色版；
    /// 否则默认的 3D 深色主题。应用装配层应该用它（而不是 `Theme::default()`）
    /// 初始化 resource，`NO_COLOR` 的通路才是通的。
    pub fn from_env() -> Self {
        Self::from_no_color(std::env::var_os("NO_COLOR").as_deref())
    }

    fn from_no_color(value: Option<&OsStr>) -> Self {
        match value.is_some_and(|value| !value.is_empty()) {
            true => Self::default().without_color(),
            false => Self::default(),
        }
    }
}

/// 清掉一个样式的颜色，保留修饰符。
fn colorless(style: Style) -> Style {
    Style {
        fg: None,
        bg: None,
        ..style
    }
}

/// 一块平涂表面：标题两侧各留一格，下面留一行 padding（上游 `surface`）。
fn surface(color: RColor, mut title: Line<'_>, plain: bool) -> Block<'_> {
    title.spans.insert(0, Span::from(" "));
    title.spans.push(Span::from(" "));
    let title = match plain {
        true => title.bold().reversed(),
        false => title.bold(),
    };
    Block::new()
        .bg(color)
        .title(title)
        .padding(Padding::new(1, 1, 0, 1))
}

/// `from` 朝 `to` 走 `by`（0..=1）。
fn blend(from: (u8, u8, u8), to: (u8, u8, u8), by: f32) -> (u8, u8, u8) {
    let channel = |from: u8, to: u8| {
        let mixed = f32::from(from) + (f32::from(to) - f32::from(from)) * by;
        mixed.round().clamp(0.0, 255.0) as u8
    };
    (
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

/// 看起来有多亮，0..=1（上游 `lightness`）。
fn lightness((r, g, b): (u8, u8, u8)) -> f32 {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个 band 的默认底色（上游字段名 → 旧混色结果）。
    const DEFAULT_BANDS: [(&str, (u8, u8, u8)); 17] = [
        ("user", (18, 38, 49)),
        ("tool", (31, 37, 49)),
        ("added", (17, 42, 32)),
        ("added_word", (12, 65, 24)),
        ("removed", (58, 22, 32)),
        ("removed_word", (109, 16, 24)),
        ("selected", (91, 95, 27)),
        ("panel_surface", (29, 35, 47)),
        ("raised_surface", (69, 74, 84)),
        ("input", (60, 65, 75)),
        ("call_waiting", (44, 49, 60)),
        ("call_running", (53, 58, 33)),
        ("call_done", (18, 36, 34)),
        ("call_failed", (76, 20, 29)),
        ("code", (39, 44, 55)),
        ("note", (17, 42, 52)),
        ("important", (62, 67, 31)),
    ];

    #[test]
    fn every_upstream_band_has_its_own_field() {
        let theme = Theme::default();
        let bands = theme.bands();
        // 清单与上游字段名一一对应、顺序一致（不用反射）。
        assert_eq!(
            bands.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            UPSTREAM_BANDS.to_vec()
        );
        assert_eq!(bands.len(), 17);
        for (name, style) in bands {
            assert_eq!(style.fg, None, "{name} 是 band，不该有前景色");
            assert!(style.bg.is_some(), "{name} 没有底色");
        }
        // 默认的 17 个 band 两两不同（上游同名测试的本地版）。
        let mut colors = theme
            .bands()
            .iter()
            .map(|(_, style)| style.bg.expect("band"))
            .collect::<Vec<_>>();
        colors.sort_by_key(|color| format!("{color:?}"));
        colors.dedup();
        assert_eq!(colors.len(), 17, "band 默认值有重复");
    }

    #[test]
    fn default_keeps_the_existing_visible_colors() {
        let theme = Theme::default();
        // 3D 深色底上的旧前景 token：一个都不能变。
        assert_eq!(theme.fg.fg, Some(RColor::Rgb(210, 216, 228)));
        assert_eq!(theme.dim.fg, Some(RColor::Rgb(120, 128, 145)));
        assert_eq!(theme.accent.fg, Some(RColor::Yellow));
        assert!(theme.accent.add_modifier.contains(Modifier::BOLD));
        assert_eq!(theme.ok.fg, Some(RColor::Green));
        assert_eq!(theme.warn.fg, Some(RColor::LightYellow));
        assert_eq!(theme.err.fg, Some(RColor::LightRed));
        assert_eq!(theme.assistant.fg, Some(RColor::Cyan));
        assert_eq!(theme.band, RColor::Rgb(20, 26, 38));
        assert_eq!(theme.prompt.fg, Some(RColor::Green));
        assert!(theme.prompt.add_modifier.contains(Modifier::BOLD));
        assert!(!theme.plain);
        assert!(theme.truecolor);
        // 17 个 band 的默认值 = 旧 `rich::upstream_theme` 混出来的颜色。
        for ((name, style), (expected_name, color)) in
            theme.bands().iter().zip(DEFAULT_BANDS)
        {
            assert_eq!(*name, expected_name);
            assert_eq!(style.bg, Some(RColor::Rgb(color.0, color.1, color.2)));
        }
    }

    #[test]
    fn a_plain_theme_has_no_foreground_or_background_colors() {
        let mut plain = Theme::default().without_color();
        assert!(plain.plain);
        for style in plain.styles_mut() {
            assert_eq!(style.fg, None, "无色版还有前景色：{style:?}");
            assert_eq!(style.bg, None, "无色版还有底色：{style:?}");
        }
        // `band` 是场景底色（上游私有的 `background`），不是文字色；保留。
        assert_eq!(plain.band, Theme::default().band);
        // 默认主题本身不受影响。
        assert!(!Theme::default().plain);
    }

    #[test]
    fn no_color_is_set_only_when_nonempty() {
        assert!(!Theme::from_no_color(None).plain);
        assert!(!Theme::from_no_color(Some(OsStr::new(""))).plain);
        assert!(Theme::from_no_color(Some(OsStr::new("1"))).plain);
        assert!(Theme::from_no_color(Some(OsStr::new("anything"))).plain);
    }

    #[test]
    fn plain_marks_selection_and_changed_words_with_modifiers() {
        let theme = Theme::default();
        // 有色版：选中 = selected 底色；改动词 = 给定 tint。
        assert_eq!(theme.selection().bg, theme.selected.bg);
        assert_eq!(
            theme.changed_word(theme.added_word.bg.expect("band")).bg,
            theme.added_word.bg
        );
        // 无色版：反色 / 下划线，且没有底色。
        let plain = theme.without_color();
        let selection = plain.selection();
        assert!(selection.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(selection.bg, None);
        let word = plain.changed_word(theme.added_word.bg.expect("band"));
        assert!(word.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(word.bg, None);
    }

    #[test]
    fn shade_walks_from_the_scene_background_toward_white() {
        let theme = Theme::default();
        // 3D 深色底：越走越亮，0 是底色本身。
        assert_eq!(theme.shade(0.0), theme.band);
        let RColor::Rgb(r, _, _) = theme.shade(0.5) else {
            panic!("truecolor 主题的 shade 应该给 RGB");
        };
        assert!(r > 100, "深色底上的 shade(0.5) 太暗：{r}");
        // 无色版只影响前景/底色，不影响 shade 的数学。
        assert_eq!(theme.without_color().shade(0.0), theme.band);
    }

    #[test]
    fn panel_and_raised_surfaces_keep_the_border_padding() {
        let area = rig_tui::ratatui::layout::Rect::new(0, 0, 20, 10);
        let bordered = Block::bordered().inner(area);
        let theme = Theme::default();
        assert_eq!(theme.panel("files").inner(area), bordered);
        assert_eq!(theme.raised("").inner(area), bordered);
        // 表面底色来自对应 band；无色版退到 `Reset`（终端默认底 = 不画）。
        assert_eq!(
            theme.surface_band(theme.panel_surface),
            RColor::Rgb(29, 35, 47)
        );
        let plain = theme.without_color();
        assert_eq!(
            plain.surface_band(plain.raised_surface),
            RColor::Reset,
            "无色版的面板表面不该有底色"
        );
    }
}
