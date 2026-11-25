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
//! [`Theme::from_env`]（`NO_COLOR` 与 `RF_THEME`）。
//!
//! ## 明暗两套
//!
//! [`ThemeKind`] 是唯一取值：`Dark` 是上表这套默认值，`Light` 是 [`Theme::light`]
//! 的浅色 17 band。[`ThemeKind::props`] 同时给出 feathers 的 `ThemeProps`（面板
//! chrome），[`install_theme`] 把两者一起装进 App；应用层 `/theme` 只改这两个
//! 资源，下一帧 chrome（`UiTheme`）与内容（`Theme`）就都换色。

#[cfg(test)]
use std::ffi::OsStr;
use std::sync::OnceLock;

use bevy::color::{Alpha, Luminance};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_feathers::palette;
use bevy_feathers::theme::{SemanticToken, SurfaceLevel, ThemeProps, ThemeToken, UiTheme};
use bevy_feathers::tokens::semantic;
use bevy_feathers::{dark_theme, tokens};
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

// ---------------------------------------------------------------------------
// 面板 chrome：feathers 的设计 token（F1）
// ---------------------------------------------------------------------------
//
// 17 个语义 band 管的是**内容**（转写行/代码块/diff/调用状态）的颜色，它们继续
// 由 [`Theme`] 提供；这一节管的是**面板表面**（底、边框、圆角）——改用
// `bevy_feathers` 的 `UiTheme`。两套并存的原因见 `docs/design/FEATHERS_UI.md`：
// band 是终端时代的语义色带，feathers token 是标准 UI 的表面层级，各自负责一层。
//
// 取 token 的规则：面板体 = feathers 的 pane body，细条 = pane header（比
// body 深一档），输入框 = text input 的 field 底，确认 tile = subpane header
// （高亮一档），边框 = subpane body border（= feathers 的 `border.default`）。

/// 面板表面：feathers `pane.body.bg`。
pub const PANEL_BG: ThemeToken = tokens::PANE_BODY_BG;
/// 细条（状态行 / 键位提示 / 补全 tile）表面：feathers `pane.header.bg`。
pub const BAR_BG: ThemeToken = tokens::PANE_HEADER_BG;
/// 输入框表面：feathers `textinput.bg`（field 底，比 pane body 亮一档）。
pub const INPUT_BG: ThemeToken = tokens::TEXT_INPUT_BG;
/// 确认/待确认 tile 表面：feathers `subpane.header.bg`（比 body 亮一档）。
pub const CONFIRM_BG: ThemeToken = tokens::SUBPANE_HEADER_BG;
/// 面板边框：feathers `subpane.body.border`（= `border.default`）。
pub const PANEL_BORDER: ThemeToken = tokens::SUBPANE_BODY_BORDER;
/// 输入框的焦点环：feathers `focus.ring`（强调色半透明）。
pub const FOCUS_BORDER: ThemeToken = tokens::FOCUS_RING;

/// 面板圆角（UI 像素）。
///
/// feathers 只有颜色是 design token，圆角/间距在 `bevy_feathers::constants` 里
/// 是普通常量（或者直接写在 `containers::pane`/`group` 的 4px）。这里对齐
/// `bevy_feathers::containers::{pane, group}` 的 4px，集中成**一个**常量：
/// 等 feathers 出圆角 token 时只改这里。
pub const PANEL_RADIUS: f32 = 4.0;

// ---------------------------------------------------------------------------
// 滚动条：feathers `controls::scrollbar` / `listview` 的几何常量
// ---------------------------------------------------------------------------
//
// feathers 的滚动条是**纯 Node**（`BackgroundColor` + `BorderRadius`，没有图片/
// 图标），3D 镜像能画；但它的滑块同步绑在 `ScrollPosition` + `ComputedNode` 的
// 真实内容高度上。picker 的列表是**窗口化**的（只生成可见行，滚动量在
// `PickerInput.scroll`），没有那份真实内容高度，所以 picker 用同一套 token /
// 几何常量自己驱动滑块（见 `picker::sync_picker_scrollbar`）。这几个值照抄
// feathers 的 listview：轨道 6px、离右缘 4px、圆角 3px、滑块最短 8px。

/// 滚动条轨道宽度（feathers `listview` 的 `width: px(6)`）。
pub const SCROLLBAR_WIDTH: f32 = 6.0;
/// 滚动条离列表右缘的距离（feathers `listview` 的 `right: px(4)`）。
pub const SCROLLBAR_INSET: f32 = 4.0;
/// 滚动条（轨道和滑块）的圆角（feathers scrollbar scene 的 3px）。
pub const SCROLLBAR_RADIUS: f32 = 3.0;
/// 滑块最短长度（feathers scrollbar scene 的 `min_thumb_length: 8.0`）。
pub const SCROLLBAR_MIN_THUMB: f32 = 8.0;
/// 列表右边要给滚动条多留的空隙（轨道 + 靠右的间距；左边由面板内边距给）。
///
/// 行是 100% 宽，不 reserve 的话选中行的实心底会盖住滑块；feathers 的
/// `listview` 用 `ScrollbarGutter` 做同一件事。
pub const SCROLLBAR_GUTTER: f32 = SCROLLBAR_WIDTH + SCROLLBAR_INSET;

/// 一份 feathers 标准暗色主题（`dark_theme::create_dark_theme`）。
///
/// `FeathersViewPlugin` 用它初始化 `UiTheme`；没有 UI 库/没插资源的最小 App
/// 也用同一份兜底，保证 headless 测试里 token 查得到颜色而不是 fuchsia。
pub fn dark_ui_theme() -> UiTheme {
    ThemeKind::Dark.ui_theme()
}

/// 一份 feathers 浅色主题（[`create_light_theme`]）。
pub fn light_ui_theme() -> UiTheme {
    ThemeKind::Light.ui_theme()
}

/// 主题明暗：`/theme`、`RF_THEME` 与 [`install_theme`] 的唯一取值。
///
/// 它是 `Resource`，所以 `/theme` 无参数时能报出当前值；没有这个资源的最小
/// App（只用 `Theme` 的 headless 测试）按 `Dark` 处理。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeKind {
    /// 默认：feathers 暗色 theme + 上表那套深色 17 band。
    #[default]
    Dark,
    /// 浅色：feathers 浅色 theme + [`Theme::light`] 的浅色 17 band。
    Light,
}

impl ThemeKind {
    /// 可选值的稳定名字（`/theme` 的参数、错误提示都用它）。
    pub const OPTIONS: [ThemeKind; 2] = [Self::Dark, Self::Light];

    /// `dark` / `light`（不带大小写、首尾空白）。
    pub fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// 给通知用的可读名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "dark（深色）",
            Self::Light => "light（浅色）",
        }
    }

    /// 解析 `/theme` 的参数；大小写不敏感，首尾空白忽略。
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    /// 读 `RF_THEME`：`light` / `dark`，没设或坏值都退回 [`ThemeKind::Dark`]。
    ///
    /// 坏值不是"静默"：和 `RF_FRAME` 一样打一行警告，说明期望值并回退。
    pub fn from_env() -> Self {
        Self::from_env_text(std::env::var("RF_THEME").ok().as_deref())
    }

    /// 不含环境变量读取的解析版本（测试用）。
    pub fn from_env_text(value: Option<&str>) -> Self {
        match value {
            None => Self::Dark,
            Some(raw) => match Self::parse(raw) {
                Some(kind) => kind,
                None => {
                    warn!(
                        "[theme] RF_THEME={raw:?} 无法解析；期望 dark 或 light，\
                         本次按 dark 处理"
                    );
                    Self::Dark
                }
            },
        }
    }

    /// 对应的 feathers 面板 chrome 主题。
    pub fn props(self) -> ThemeProps {
        match self {
            Self::Dark => dark_theme::create_dark_theme(),
            Self::Light => create_light_theme(),
        }
    }

    /// 对应的 [`UiTheme`] 资源值。
    pub fn ui_theme(self) -> UiTheme {
        UiTheme(self.props())
    }
}

/// 3D 场景底色跟随主题。
///
/// 不跟随的话，切到浅色主题后浅色面板会漂在近黑背景上（看起来像没切成功）。
/// 深色主题恢复"启动时 `ClearColor` 的原值"（`Local` 第一次记下来），所以默认
/// 路径逐字节不变；浅色用一个和白色面板有对比的浅灰。
pub fn sync_clear_color(
    kind: Option<Res<ThemeKind>>,
    clear: Option<ResMut<ClearColor>>,
    mut base: Local<Option<Color>>,
) {
    let Some(mut clear) = clear else {
        return;
    };
    let kind = kind.map_or(ThemeKind::Dark, |kind| *kind);
    let base = *base.get_or_insert(clear.0);
    let want = match kind {
        ThemeKind::Dark => base,
        ThemeKind::Light => Color::srgb(0.84, 0.85, 0.87),
    };
    if clear.0 != want {
        clear.0 = want;
    }
}

/// 按 `kind` 初始化一个 App 的主题资源（`FeathersViewPlugin` 的启动路径）。
///
/// 语义与原来的 `FeathersViewPlugin` 一致：
/// * `UiTheme` 缺失 / 为空（`FeathersPlugins` 插的空 `ThemeProps`）→ 插入
///   `kind` 的主题；调用方自己配好的 `UiTheme` 不被覆盖；
/// * 记录当前 [`ThemeKind`]（供 `/theme` 无参数时显示）；
/// * 插入 `kind` 的 [`Theme`]，`NO_COLOR` 仍生效（走 [`Theme::from_env_kind`]）。
///
/// 独立成函数是为了让 headless 测试能直接验证 `RF_THEME=light` 的启动结果，
/// 不需要改进程环境。
pub fn install_theme(app: &mut App, kind: ThemeKind) {
    let configured = app
        .world()
        .get_resource::<UiTheme>()
        .is_some_and(|ui_theme| !is_unconfigured(ui_theme));
    if !configured {
        app.insert_resource(kind.ui_theme());
    }
    app.insert_resource(kind);
    app.insert_resource(Theme::from_env_kind(kind));
}

/// 一份 feathers 浅色主题：从暗色主题的 token 分配表出发，把语义 token 换成
/// 浅色表面 + 深色文字。
///
/// 只在 [`ThemeKind::props`] 里用；`UiTheme` 与 `ThemeProps` 的用法和
/// `create_dark_theme` 完全一样。
///
/// 映射原则（为什么对比够）：
/// * 表面：窗口 / 细条用 `LIGHT_GRAY_MIX`，面板体 / 弹层用 `WHITE`，输入框 /
///   确认 tile / group 用 `LIGHT_GRAY_1`；全部是浅色，彼此差一档；
/// * 文字：`TEXT_DEFAULT` / `TEXT_DIM` 用 `GRAY_0` / `GRAY_2`（深灰），
///   `TEXT_ON_ACCENT` 仍是白（只给强调色/主按钮用）；
/// * 边框：`BORDER_DEFAULT` 用 `GRAY_2`，和 `WHITE` 面板体差 0.7 以上；
/// * 暗色主题里 `BUTTON_TEXT` / `TEXT_INPUT_TEXT` / `LISTROW_TEXT` /
///   `MENUITEM_TEXT` 指向 `TEXT_ON_ACCENT`（白）——浅色底上必须改回
///   `TEXT_DEFAULT`，否则白底白字。
pub fn create_light_theme() -> ThemeProps {
    let mut props = dark_theme::create_dark_theme();
    props.semantic_base = light_semantic_base();
    props.semantic_overrides = light_semantic_overrides();

    // 暗色主题为"深底 + 白字"挑的这几处，浅色底上要换成深色文字；面板边框
    // 也从"复用 header 表面色"改成真正的 border token（浅色下前者几乎看不见）。
    for (token, semantic_token) in [
        (tokens::BUTTON_TEXT, semantic::TEXT_DEFAULT),
        (tokens::TEXT_INPUT_TEXT, semantic::TEXT_DEFAULT),
        (tokens::LISTROW_TEXT, semantic::TEXT_DEFAULT),
        (tokens::MENUITEM_TEXT, semantic::TEXT_DEFAULT),
        (tokens::SUBPANE_BODY_BORDER, semantic::BORDER_DEFAULT),
        (tokens::PANE_HEADER_BORDER, semantic::BORDER_DEFAULT),
        (tokens::PANE_HEADER_DIVIDER, semantic::BORDER_DEFAULT),
    ] {
        props.token_assignments.insert(token, semantic_token);
    }
    props
}

/// 浅色主题的语义色。值只从 feathers 的 `palette` 里挑，保证和它的控件同一
/// 色相/明度体系。
fn light_semantic_base() -> HashMap<SemanticToken, Color> {
    HashMap::from([
        // 表面：接近白，层级靠 LIGHT_GRAY_MIX / LIGHT_GRAY_1 拉开。
        (semantic::SURFACE_WINDOW, palette::LIGHT_GRAY_MIX),
        (semantic::SURFACE_PANE_BODY, palette::WHITE),
        (semantic::SURFACE_PANE_HEADER, palette::LIGHT_GRAY_MIX),
        (semantic::SURFACE_SUBPANE_BODY, palette::WHITE),
        (semantic::SURFACE_SUBPANE_HEADER, palette::LIGHT_GRAY_1),
        (semantic::SURFACE_GROUP, palette::LIGHT_GRAY_1),
        (semantic::SURFACE_DIALOG, palette::WHITE),
        (semantic::SURFACE_MENU, palette::WHITE),
        (semantic::BORDER_DEFAULT, palette::GRAY_2),
        // 文字：深灰；dim 仍是深灰但轻一档，在浅底上都够读。
        (semantic::TEXT_DEFAULT, palette::GRAY_0),
        (semantic::TEXT_DIM, palette::GRAY_2),
        (semantic::TEXT_DISABLED, palette::GRAY_2.with_alpha(0.55)),
        (semantic::TEXT_ON_ACCENT, palette::WHITE),
        // 强调色/填充：强调色保持 feathers 的 ACCENT（蓝）；实心控件用浅灰，
        // 文字由上面的 token 重映射保证是深色。
        (semantic::FILL_ACCENT_DEFAULT, palette::ACCENT),
        (semantic::FILL_ACCENT_HOVER, palette::ACCENT.lighter(0.05)),
        (semantic::FILL_ACCENT_PRESSED, palette::ACCENT.lighter(0.1)),
        (
            semantic::FILL_ACCENT_DISABLED,
            palette::LIGHT_GRAY_2.with_alpha(0.7),
        ),
        (semantic::FILL_SOLID_DEFAULT, palette::LIGHT_GRAY_MIX),
        (
            semantic::FILL_SOLID_HOVER,
            palette::LIGHT_GRAY_MIX.lighter(0.05),
        ),
        (
            semantic::FILL_SOLID_PRESSED,
            palette::LIGHT_GRAY_MIX.lighter(0.1),
        ),
        (
            semantic::FILL_SOLID_DISABLED,
            palette::LIGHT_GRAY_MIX.with_alpha(0.6),
        ),
        (semantic::FILL_ITEM_DEFAULT, Color::NONE),
        (semantic::FILL_ITEM_HOVER, palette::LIGHT_GRAY_MIX),
        (semantic::FILL_ITEM_PRESSED, palette::LIGHT_GRAY_1),
        // 输入框：比面板体（白）深一档，边框再深一档。
        (semantic::FILL_FIELD_DEFAULT, palette::LIGHT_GRAY_1),
        (
            semantic::FILL_FIELD_HOVER,
            palette::LIGHT_GRAY_1.lighter(0.05),
        ),
        (
            semantic::FILL_FIELD_PRESSED,
            palette::LIGHT_GRAY_1.lighter(0.1),
        ),
        (semantic::FILL_NONE, Color::NONE),
        (semantic::AXIS_X, palette::X_AXIS),
        (semantic::AXIS_Y, palette::Y_AXIS),
        (semantic::AXIS_Z, palette::Z_AXIS),
        (semantic::AXIS_W, palette::W_AXIS),
        (semantic::FOCUS_RING, palette::ACCENT.with_alpha(0.5)),
    ])
}

/// 浅色主题的 SurfaceLevel 覆盖：group / dialog 这类更高一层的表面，输入框与
/// 可点项要换一档浅灰，别沿用暗色主题里那组深灰。
fn light_semantic_overrides() -> HashMap<SurfaceLevel, HashMap<SemanticToken, Color>> {
    HashMap::from([
        (
            SurfaceLevel::Highest,
            HashMap::from([
                (semantic::FILL_SOLID_DEFAULT, palette::LIGHT_GRAY_MIX),
                (
                    semantic::FILL_SOLID_HOVER,
                    palette::LIGHT_GRAY_MIX.lighter(0.05),
                ),
                (
                    semantic::FILL_SOLID_PRESSED,
                    palette::LIGHT_GRAY_MIX.lighter(0.1),
                ),
                (
                    semantic::FILL_SOLID_DISABLED,
                    palette::LIGHT_GRAY_MIX.with_alpha(0.6),
                ),
                (semantic::FILL_FIELD_DEFAULT, palette::WHITE),
                (
                    semantic::FILL_FIELD_HOVER,
                    palette::LIGHT_GRAY_MIX.lighter(0.1),
                ),
                (
                    semantic::FILL_FIELD_PRESSED,
                    palette::LIGHT_GRAY_MIX,
                ),
                (semantic::FILL_ITEM_HOVER, palette::LIGHT_GRAY_MIX),
                (semantic::FILL_ITEM_PRESSED, palette::LIGHT_GRAY_1),
            ]),
        ),
        (
            SurfaceLevel::Higher,
            HashMap::from([
                (semantic::FILL_FIELD_DEFAULT, palette::LIGHT_GRAY_MIX),
                (
                    semantic::FILL_FIELD_HOVER,
                    palette::LIGHT_GRAY_MIX.lighter(0.05),
                ),
                (
                    semantic::FILL_FIELD_PRESSED,
                    palette::LIGHT_GRAY_MIX.lighter(0.1),
                ),
                (semantic::FILL_ITEM_HOVER, palette::LIGHT_GRAY_MIX),
                (semantic::FILL_ITEM_PRESSED, palette::LIGHT_GRAY_1),
            ]),
        ),
        (
            SurfaceLevel::Floating,
            HashMap::from([
                (semantic::FILL_SOLID_DEFAULT, palette::LIGHT_GRAY_MIX),
                (semantic::FILL_FIELD_DEFAULT, palette::LIGHT_GRAY_MIX),
                (semantic::FILL_ITEM_HOVER, palette::LIGHT_GRAY_MIX),
                (semantic::FILL_ITEM_PRESSED, palette::LIGHT_GRAY_1),
            ]),
        ),
    ])
}

/// 进程内唯一的兜底主题：没有 `UiTheme` 资源时（headless 测试、最小 App）用它。
///
/// `create_dark_theme` 要建几张 HashMap，逐帧重建太浪费；`OnceLock` 只建一次。
/// 测试想验证"没有资源也不 panic"时直接读它。
pub fn default_ui_theme() -> &'static UiTheme {
    static DEFAULT: OnceLock<UiTheme> = OnceLock::new();
    DEFAULT.get_or_init(dark_ui_theme)
}

/// 取当前主题：有 `UiTheme` 资源用资源，没有就退到 [`default_ui_theme`]。
///
/// 这是面板 chrome 的唯一取色入口：面板系统全部经它读 token，所以测试里
/// 没插 `UiTheme` 也只拿到 feathers 的暗色值，不会 panic。
pub fn ui_theme_or_default(theme: Option<&UiTheme>) -> &UiTheme {
    theme.unwrap_or_else(|| default_ui_theme())
}

/// `UiTheme` 是不是"没配过"的空主题。
///
/// `bevy_feathers::FeathersPlugins` 会 `init_resource::<UiTheme>()` 插一个
/// 空 `ThemeProps`（所有 token 查不到、返回 fuchsia）。`FeathersViewPlugin`
/// 只在"缺失或空"时把它换成 [`dark_ui_theme`]，不覆盖调用方自己配好的主题。
pub(crate) fn is_unconfigured(theme: &UiTheme) -> bool {
    theme.0.token_assignments.is_empty()
}

#[derive(Clone, Copy, Debug, Resource, PartialEq, Eq)]
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
    /// 3D 前端没有终端可问底色，所以这里只认两个环境变量：
    /// * `RF_THEME=light` / `dark` 选明暗（见 [`ThemeKind::from_env`]）；
    /// * `NO_COLOR` 非空时把选中的主题变成无色版。
    ///
    /// 应用装配层应该用它（而不是 `Theme::default()`）初始化 resource，两条
    /// 通路才是通的；`FeathersViewPlugin` 经 [`install_theme`] 只读一次
    /// `RF_THEME`，避免两处解析出不同的值。
    pub fn from_env() -> Self {
        Self::from_env_kind(ThemeKind::from_env())
    }

    /// 已知明暗时读 `NO_COLOR`：`RF_THEME` 已经解析过就用这个入口。
    pub fn from_env_kind(kind: ThemeKind) -> Self {
        let theme = Self::from_kind(kind);
        match std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
            true => theme.without_color(),
            false => theme,
        }
    }

    /// 一套明暗主题的完整 [`Theme`]。
    pub fn from_kind(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Dark => Self::default(),
            ThemeKind::Light => Self::light(),
        }
    }

    /// 浅色 17 band：底色接近白，文字深色；added/removed/调用状态用浅色调，
    /// 彼此仍可区分。
    ///
    /// 前景 token 也一起换成深色版——内容行的默认前景取 [`Theme::fg`]
    /// （`transcript::base_fg_color`），只改 band 会得到"浅底浅字"。
    pub fn light() -> Self {
        Self {
            fg: Style::default().fg(RColor::Rgb(0, 0, 0)),
            dim: Style::default().fg(RColor::Rgb(96, 102, 114)),
            accent: Style::default()
                .fg(RColor::Rgb(30, 84, 160))
                .add_modifier(Modifier::BOLD),
            ok: Style::default().fg(RColor::Rgb(22, 110, 60)),
            warn: Style::default().fg(RColor::Rgb(140, 92, 10)),
            err: Style::default().fg(RColor::Rgb(175, 35, 45)),
            assistant: Style::default().fg(RColor::Rgb(20, 95, 140)),
            band: RColor::Rgb(252, 252, 254),
            prompt: Style::default()
                .fg(RColor::Rgb(20, 115, 70))
                .add_modifier(Modifier::BOLD),

            // 17 个 band 都是浅色底：蓝 / 中性 / 绿 / 粉 / 黄各成一组，同组
            // 的整行色与改词色差一档，保证 diff 的增行/改词、调用状态之间
            // 仍能一眼分开。
            user: band(222, 235, 245),
            tool: band(236, 238, 242),
            added: band(215, 240, 220),
            added_word: band(170, 220, 180),
            removed: band(248, 218, 220),
            removed_word: band(240, 175, 180),
            selected: band(250, 240, 170),
            panel_surface: band(238, 240, 244),
            raised_surface: band(225, 229, 236),
            input: band(232, 238, 246),
            call_waiting: band(228, 232, 240),
            call_running: band(246, 242, 205),
            call_done: band(220, 240, 230),
            call_failed: band(248, 220, 222),
            code: band(235, 238, 243),
            note: band(222, 238, 248),
            important: band(250, 240, 195),

            plain: false,
            truecolor: true,
        }
    }

    #[cfg(test)]
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

    /// sRGB 的相对亮度近似（0..=1）；测试只断言方向，不钉具体值。
    fn luma(color: Color) -> f32 {
        let c = color.to_srgba();
        0.2126 * c.red + 0.7152 * c.green + 0.0722 * c.blue
    }

    /// `Theme::light` 的 17 个 band 都有值、都是浅底，且和深色文字对比明显。
    #[test]
    fn light_bands_are_light_and_readable_with_dark_text() {
        let theme = Theme::light();
        let fg = rgb(theme.fg.fg.expect("浅色主题要有主文字色")).expect("RGB");
        assert!(
            lightness(fg) < 0.3,
            "浅色主题的主文字不够深：{fg:?}"
        );
        assert!(
            lightness(rgb(theme.band).expect("band")) > 0.9,
            "浅色主题的场景底色不够浅：{:?}",
            theme.band
        );

        let bands = theme.bands();
        assert_eq!(bands.len(), 17);
        let mut colors = Vec::new();
        for (name, style) in bands {
            assert_eq!(style.fg, None, "{name} 是 band，不该有前景色");
            let bg = rgb(style.bg.unwrap_or_else(|| panic!("{name} 没有底色")))
                .expect("RGB band");
            let bg_lightness = lightness(bg);
            assert!(
                bg_lightness > 0.7,
                "{name} 的浅色 band 不够浅：{bg:?}（{bg_lightness}）"
            );
            assert!(
                bg_lightness - lightness(fg) > 0.35,
                "{name} 的底色 {bg:?} 与文字 {fg:?} 对比不够"
            );
            colors.push(style.bg.expect("band"));
        }
        // 17 个 band 彼此不同：diff 的整行/改词、调用状态不能糊成同一种底。
        colors.sort_by_key(|color| format!("{color:?}"));
        colors.dedup();
        assert_eq!(colors.len(), 17, "light band 有重复");
        // 前景 token 也换成深色版，浅底上不出现浅字。
        for (name, style) in [
            ("fg", theme.fg),
            ("dim", theme.dim),
            ("accent", theme.accent),
            ("ok", theme.ok),
            ("warn", theme.warn),
            ("err", theme.err),
            ("assistant", theme.assistant),
            ("prompt", theme.prompt),
        ] {
            let color = rgb(style.fg.unwrap_or_else(|| panic!("{name} 没有前景色")))
                .expect("RGB");
            assert!(
                lightness(color) < 0.4,
                "{name} 在浅色主题里不够深：{color:?}"
            );
        }
    }

    /// `ThemeKind` 的解析入口与 `Theme::from_kind`：默认仍是 dark，
    /// `RF_THEME` 的坏值退回 dark（不需要改进程环境）。
    #[test]
    fn theme_kind_parses_env_values_and_keeps_dark_as_the_default() {
        assert_eq!(ThemeKind::default(), ThemeKind::Dark);
        assert_eq!(Theme::from_kind(ThemeKind::Dark).band, Theme::default().band);
        assert_eq!(Theme::from_kind(ThemeKind::Light).band, Theme::light().band);
        assert_eq!(ThemeKind::parse("light"), Some(ThemeKind::Light));
        assert_eq!(ThemeKind::parse(" LIGHT "), Some(ThemeKind::Light));
        assert_eq!(ThemeKind::parse("dark"), Some(ThemeKind::Dark));
        assert_eq!(ThemeKind::parse("purple"), None);
        assert_eq!(ThemeKind::from_env_text(None), ThemeKind::Dark);
        assert_eq!(
            ThemeKind::from_env_text(Some("light")),
            ThemeKind::Light
        );
        assert_eq!(
            ThemeKind::from_env_text(Some("light ")),
            ThemeKind::Light
        );
        assert_eq!(ThemeKind::from_env_text(Some("nope")), ThemeKind::Dark);
        assert_eq!(ThemeKind::from_env_text(Some("")), ThemeKind::Dark);
        // 两个名字都能被 `/theme` 打印出来。
        assert_eq!(ThemeKind::Light.name(), "light");
        assert_eq!(ThemeKind::Dark.name(), "dark");
    }

    /// 浅色 feathers 主题：`SURFACE_*` 亮、`TEXT_DEFAULT` 暗、边框与面板体明显
    /// 不同；面板/输入/列表的文字 token 不能还是暗色主题里的白字。
    #[test]
    fn create_light_theme_has_light_surfaces_and_dark_text() {
        let ui = light_ui_theme();
        let error: Color = bevy::color::palettes::basic::FUCHSIA.into();

        for token in [
            tokens::WINDOW_BG,
            tokens::PANE_BODY_BG,
            tokens::PANE_HEADER_BG,
            tokens::SUBPANE_BODY_BG,
            tokens::SUBPANE_HEADER_BG,
            tokens::GROUP_BG,
            tokens::DIALOG_BG,
            tokens::MENU_BG,
        ] {
            let color = ui.color(&token);
            assert_ne!(color, error, "{token} 没有在浅色主题里赋值");
            assert!(
                luma(color) > 0.6,
                "{token} 不是浅色表面：{color:?}（亮度 {}）",
                luma(color)
            );
        }

        assert!(
            luma(ui.color(&tokens::TEXT_MAIN)) < 0.3,
            "TEXT_MAIN 在浅色主题里必须是深色"
        );
        for token in [
            tokens::TEXT_INPUT_TEXT,
            tokens::LISTROW_TEXT,
            tokens::MENUITEM_TEXT,
            tokens::BUTTON_TEXT,
        ] {
            assert!(
                luma(ui.color(&token)) < 0.3,
                "{token} 在浅色底上不能还是白字"
            );
        }

        // 边框必须看得出来：BORDER_DEFAULT 与面板体（白）差一大截。
        let border = ui.color(&tokens::MENU_BORDER);
        let body = ui.color(&tokens::PANE_BODY_BG);
        assert!(
            luma(body) - luma(border) > 0.3,
            "浅色主题的边框不够明显：border={border:?} body={body:?}"
        );
        // 面板自己的 token：底是浅的，边框和底不同，输入/细条/确认各成一档。
        for token in [
            PANEL_BG,
            BAR_BG,
            INPUT_BG,
            CONFIRM_BG,
            PANEL_BORDER,
            FOCUS_BORDER,
        ] {
            assert_ne!(ui.color(&token), error, "{token} 没有在浅色主题里赋值");
        }
        assert!(luma(ui.color(&PANEL_BG)) > 0.9);
        assert!(luma(ui.color(&BAR_BG)) > 0.6);
        assert!(luma(ui.color(&INPUT_BG)) > 0.6);
        assert!(luma(ui.color(&CONFIRM_BG)) > 0.6);
        assert_ne!(ui.color(&PANEL_BG), ui.color(&BAR_BG));
        assert_ne!(ui.color(&PANEL_BG), ui.color(&INPUT_BG));
        assert_ne!(ui.color(&PANEL_BG), ui.color(&PANEL_BORDER));
        assert_ne!(ui.color(&BAR_BG), ui.color(&INPUT_BG));
        // 滚动条轨道/滑块在浅色主题里也查得到且不是一个颜色。
        assert_ne!(ui.color(&tokens::SCROLLBAR_BG), error);
        assert_ne!(ui.color(&tokens::SCROLLBAR_THUMB), error);
        assert_ne!(
            ui.color(&tokens::SCROLLBAR_BG),
            ui.color(&tokens::SCROLLBAR_THUMB)
        );

        // 暗色主题的每个 token 都指向一个语义 token；浅色主题把语义表
        // 整张换掉后，不能漏掉任何一个（漏了会静默退到 fuchsia）。
        for semantic_token in ui.0.token_assignments.values() {
            assert!(
                ui.0.semantic_base.contains_key(semantic_token),
                "浅色主题缺少语义色 {semantic_token:?}"
            );
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

    // -----------------------------------------------------------------------
    // F1：面板 chrome 的 feathers token
    // -----------------------------------------------------------------------

    /// 面板表面/边框用的就是 feathers 的设计 token（不是本地自造的名字）。
    #[test]
    fn panel_chrome_tokens_are_the_feathers_design_tokens() {
        assert_eq!(PANEL_BG, tokens::PANE_BODY_BG);
        assert_eq!(BAR_BG, tokens::PANE_HEADER_BG);
        assert_eq!(INPUT_BG, tokens::TEXT_INPUT_BG);
        assert_eq!(CONFIRM_BG, tokens::SUBPANE_HEADER_BG);
        assert_eq!(PANEL_BORDER, tokens::SUBPANE_BODY_BORDER);
        assert_eq!(FOCUS_BORDER, tokens::FOCUS_RING);
        // 圆角没有 token：对齐 feathers containers::pane/group 的 4px。
        assert_eq!(PANEL_RADIUS, 4.0);
    }

    /// 每个 chrome token 在 feathers 暗色主题里都查得到，不会退到错误色 fuchsia。
    #[test]
    fn every_panel_token_resolves_in_the_feathers_dark_theme() {
        let theme = dark_ui_theme();
        let error: Color = bevy::color::palettes::basic::FUCHSIA.into();
        for token in [
            PANEL_BG,
            BAR_BG,
            INPUT_BG,
            CONFIRM_BG,
            PANEL_BORDER,
            FOCUS_BORDER,
        ] {
            let color = theme.color(&token);
            assert_ne!(color, error, "{token} 没有在主题里赋值");
        }
        // 细条（pane header）与面板体（pane body）不是同一个表面色。
        assert_ne!(theme.color(&BAR_BG), theme.color(&PANEL_BG));
        // 输入框（field 底）比面板体亮一档。
        assert_ne!(theme.color(&INPUT_BG), theme.color(&PANEL_BG));
    }

    /// 滚动条的几何常量照 feathers `listview` / scrollbar scene 的值；
    /// `SCROLLBAR_BG` / `SCROLLBAR_THUMB` 在 feathers 暗色主题里都查得到。
    #[test]
    fn scrollbar_geometry_matches_the_feathers_scene() {
        assert_eq!(SCROLLBAR_WIDTH, 6.0, "feathers listview 的轨道宽");
        assert_eq!(SCROLLBAR_INSET, 4.0, "feathers listview 的 right: 4");
        assert_eq!(SCROLLBAR_RADIUS, 3.0, "feathers scrollbar scene 的圆角");
        assert_eq!(SCROLLBAR_MIN_THUMB, 8.0, "feathers min_thumb_length");
        assert_eq!(SCROLLBAR_GUTTER, SCROLLBAR_WIDTH + SCROLLBAR_INSET);

        let theme = dark_ui_theme();
        let error: Color = bevy::color::palettes::basic::FUCHSIA.into();
        for token in [tokens::SCROLLBAR_BG, tokens::SCROLLBAR_THUMB] {
            assert_ne!(theme.color(&token), error, "{token} 没有在主题里赋值");
        }
        assert_ne!(
            theme.color(&tokens::SCROLLBAR_BG),
            theme.color(&tokens::SCROLLBAR_THUMB),
            "轨道和滑块不是同一个颜色"
        );
    }

    /// 没有 `UiTheme` 时退到 feathers 暗色主题；有资源时用资源的颜色。
    #[test]
    fn theme_lookup_falls_back_to_the_feathers_dark_theme() {
        assert!(!is_unconfigured(default_ui_theme()));
        for token in [PANEL_BG, PANEL_BORDER, BAR_BG] {
            assert_eq!(
                ui_theme_or_default(None).color(&token),
                default_ui_theme().color(&token)
            );
        }
        assert!(!is_unconfigured(&dark_ui_theme()));

        // 空 `ThemeProps`（FeathersPlugins 的 init_resource）会被认成"没配过"。
        let empty = UiTheme::default();
        assert!(is_unconfigured(&empty));

        // 自定义过的主题：按资源里的颜色解析，不被默认值盖掉。
        let custom = Color::srgb(0.12, 0.34, 0.56);
        let mut ui = dark_ui_theme();
        ui.0.semantic_base
            .insert(tokens::semantic::SURFACE_PANE_BODY, custom);
        assert_eq!(ui_theme_or_default(Some(&ui)).color(&PANEL_BG), custom);
        // 17 个 band 的语义不受 chrome token 影响（两套并存，各管一层）。
        assert_eq!(Theme::default().band, RColor::Rgb(20, 26, 38));
        assert_eq!(Theme::default().code, band(39, 44, 55));
    }
}
