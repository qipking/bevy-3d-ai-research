//! `/theme`：在 dark / light 之间切换面板 chrome 与内容色。
//!
//! 主题有两个资源，缺一不可：
//!
//! * [`UiTheme`]：面板 / 输入框 / 滚动条 / 按钮的 chrome（feathers token），
//!   `frame::sync_panel_chrome`、`picker::sync_picker_scrollbar`、
//!   `editor::sync_input_field` 每帧按它解析颜色；
//! * [`Theme`]：转写 / 工具卡 / diff / 代码的 17 个语义 band，`batch` 每帧用
//!   `upstream_theme(&Theme)` 建批。
//!
//! 所以 `/theme light` 两个一起换，下一帧两边都跟着变；只换一个会出现
//! "白面板 + 深色转写"或反过来。启动时的同一条路径是
//! [`rig_niri::theme::install_theme`]（读 `RF_THEME`）。
//!
//! `/theme` 无参数只报当前值与可选值（一条 notice），不做 picker：切换只有两个
//! 值，打一行的成本比弹窗低。

use bevy::prelude::*;
use bevy_feathers::theme::UiTheme;
use rig_ecs::prelude::Notice;
use rig_harness::commands::{AppCommandsExt, CommandArgs};
use rig_niri::theme::{Theme, ThemeKind};

/// 注册 `/theme`。和 [`crate::show::ShowPlugin`] 一样由
/// [`HarnessExtPlugin`](crate::HarnessExtPlugin) 加进命令表。
pub struct ThemePlugin;

impl Plugin for ThemePlugin {
    fn build(&self, app: &mut App) {
        app.add_command(
            "theme",
            "Show or change the theme: /theme [dark | light]",
            theme,
        );
    }
}

/// `/theme`：解析参数，成功就同时替换 [`Theme`] 与 [`UiTheme`]。
///
/// 非法参数只写一条可见的 warn notice，不改任何主题资源——包括 `ThemeKind`，
/// 所以 `/theme` 无参数时报的仍是切换前的值。
///
/// 为什么非法参数用 [`Notice::warn`] 而不是 [`Notice::error`]：上游
/// `rig_harness::commands` 会把命令运行期间写出的 **Error** 当成"拒绝"，转成
/// `Recalled` 退回输入框（用户看不到转写里的那行）；Warn 在转写里同样醒目，
/// 又不会被撤回。
pub fn theme(
    In(args): In<CommandArgs>,
    mut commands: Commands,
    current: Option<Res<ThemeKind>>,
    mut notices: MessageWriter<Notice>,
) {
    let current = current.as_deref().copied().unwrap_or_default();
    let trimmed = args.args.trim();

    if trimmed.is_empty() {
        notices.write(Notice::info(
            args.agent,
            format!(
                "当前主题：{}。可选：dark、light。用法：/theme dark | /theme light",
                current.label()
            ),
        ));
        return;
    }

    let Some(kind) = ThemeKind::parse(trimmed) else {
        notices.write(Notice::warn(
            args.agent,
            format!(
                "未知主题 \"{trimmed}\"；可选 {}，主题未改变。",
                ThemeKind::OPTIONS
                    .iter()
                    .map(|kind| kind.name())
                    .collect::<Vec<_>>()
                    .join("、")
            ),
        ));
        return;
    };

    // 两个资源一起换；`Commands::insert_resource` 在本命令系统结束时生效，
    // 下一帧 chrome 与内容都按新主题解析。
    commands.insert_resource(kind);
    commands.insert_resource(Theme::from_kind(kind));
    commands.insert_resource(UiTheme(kind.props()));
    notices.write(Notice::info(
        args.agent,
        format!("主题已切换为 {}。", kind.label()),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::TaskPoolPlugin;
    use rig_ecs::prelude::{AgentId, spawn_root};
    use rig_harness::commands::RunCommand;
    use rig_harness::prelude::Settings;

    use crate::HarnessExt;
    use rig_niri::theme::{
        BAR_BG, CONFIRM_BG, FOCUS_BORDER, INPUT_BG, PANEL_BG, PANEL_BORDER, dark_ui_theme,
        light_ui_theme,
    };

    /// 内核 + 本模板扩展（含 `/theme`）；主题按 `kind` 预装，不建窗口。
    fn theme_app() -> App {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.add_plugins(rig_ecs::AgentPlugin);
        app.insert_resource(Settings::from_toml(""));
        app.insert_resource(crate::SettingsPath(std::env::temp_dir().join(
            format!("rig-theme-cmd-settings-{}.toml", std::process::id()),
        )));
        // 和生产装配同一条初始化路径（`FeathersViewPlugin` 也调它）。
        rig_niri::theme::install_theme(&mut app, ThemeKind::Dark);
        app.add_plugins(crate::HarnessExtPlugin);
        app
    }

    fn run(app: &mut App, agent: Entity, line: &str) {
        app.world_mut().trigger(RunCommand::new(agent, line));
        app.update();
    }

    fn notices(app: &App) -> Vec<Notice> {
        app.world()
            .resource::<Messages<Notice>>()
            .iter_current_update_messages()
            .cloned()
            .collect()
    }

    fn panel_colors(ui: &UiTheme) -> Vec<Color> {
        [
            PANEL_BG,
            BAR_BG,
            INPUT_BG,
            CONFIRM_BG,
            PANEL_BORDER,
            FOCUS_BORDER,
        ]
        .iter()
        .map(|token| ui.color(token))
        .collect()
    }

    #[test]
    fn the_theme_command_is_registered() {
        let app = theme_app();
        assert!(
            app.commands().iter().any(|name| name == "/theme"),
            "commands() 应该读出 /theme，实际 {:?}",
            app.commands()
        );
    }

    /// 无参数：一条 notice 报当前主题和可选值，不改任何东西。
    #[test]
    fn bare_theme_reports_the_current_and_available_values() {
        let mut app = theme_app();
        let main = spawn_root(app.world_mut(), AgentId("main".into())).id();
        run(&mut app, main, "theme");

        let text = notices(&app)
            .iter()
            .map(|notice| notice.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("dark"), "无参数 /theme 应报当前值：{text:?}");
        assert!(text.contains("light"), "无参数 /theme 应列出可选值：{text:?}");
        assert_eq!(
            *app.world().resource::<ThemeKind>(),
            ThemeKind::Dark,
            "无参数 /theme 不能改主题"
        );
        assert_eq!(
            app.world().resource::<Theme>().band,
            Theme::default().band
        );
    }

    /// `/theme light`：`ThemeKind`、`Theme`、`UiTheme` 三个一起换成浅色；
    /// `UiTheme` 的面板 token 解析值 == `light_ui_theme()`；`/theme dark` 回得去。
    #[test]
    fn switching_to_light_replaces_both_resources_and_dark_switches_back() {
        let mut app = theme_app();
        let main = spawn_root(app.world_mut(), AgentId("main".into())).id();
        let dark = dark_ui_theme();

        // 初始就是原有的 dark（默认不回归）。
        assert_eq!(
            panel_colors(app.world().resource::<UiTheme>()),
            panel_colors(&dark)
        );
        assert_eq!(app.world().resource::<Theme>().band, Theme::default().band);

        run(&mut app, main, "theme light");
        assert_eq!(
            *app.world().resource::<ThemeKind>(),
            ThemeKind::Light,
            "/theme light 应更新当前主题"
        );
        let light = light_ui_theme();
        assert_eq!(
            panel_colors(app.world().resource::<UiTheme>()),
            panel_colors(&light),
            "/theme light 的 chrome 应解析成浅色 token"
        );
        // 内容色也换了：17 个 band 与前景色都是浅色主题的值。
        let theme = app.world().resource::<Theme>();
        assert_eq!(theme.band, Theme::light().band);
        assert_eq!(theme.fg.fg, Theme::light().fg.fg);
        for ((name, style), (light_name, light_style)) in
            theme.bands().iter().zip(Theme::light().bands())
        {
            assert_eq!(*name, light_name);
            assert_eq!(*style, light_style, "{name} band 没换成浅色");
        }

        // 再切回去：dark 的 token/band 都回来。
        run(&mut app, main, "theme dark");
        assert_eq!(
            *app.world().resource::<ThemeKind>(),
            ThemeKind::Dark
        );
        assert_eq!(
            panel_colors(app.world().resource::<UiTheme>()),
            panel_colors(&dark)
        );
        assert_eq!(app.world().resource::<Theme>().band, Theme::default().band);
    }

    /// 非法参数：写一条可见 notice，三个资源都不变（包括 ThemeKind）。
    #[test]
    fn an_unknown_theme_is_reported_and_changes_nothing() {
        let mut app = theme_app();
        let main = spawn_root(app.world_mut(), AgentId("main".into())).id();
        // 先切到 light，确认非法参数不会把它打回默认值。
        run(&mut app, main, "theme light");
        let before_theme = *app.world().resource::<Theme>();
        let before_ui = panel_colors(app.world().resource::<UiTheme>());
        let before_kind = *app.world().resource::<ThemeKind>();

        run(&mut app, main, "theme purple");

        assert_eq!(*app.world().resource::<ThemeKind>(), before_kind);
        assert_eq!(app.world().resource::<Theme>().band, before_theme.band);
        assert_eq!(
            app.world().resource::<Theme>().fg.fg,
            before_theme.fg.fg
        );
        assert_eq!(
            panel_colors(app.world().resource::<UiTheme>()),
            before_ui
        );
        let visible = notices(&app);
        assert!(
            visible.iter().any(|notice| notice.text.contains("未知主题")
                && notice.text.contains("purple")
                && notice.text.contains("light")),
            "非法参数应有一条可见错误：{:?}",
            visible.iter().map(|notice| &notice.text).collect::<Vec<_>>()
        );
    }
}
