//! `rig_harness_in_3d`：把 rig 的**内核 + harness + 前端 + 3D 渲染**装成一个可直接 fork 的 app 骨架。
//!
//! 对应 rig 官方分层里的 **harness 层**（`rig-harness` 之于 `plugins/rig-tui`）：
//!
//! ```text
//! rig-ecs（内核）  ← AgentPlugin、agent/turn/call 效应、Gate、Ask
//! rig-harness（harness）← 会话、slash 命令、Ask/Pick、启动器协议、服务
//! rig_niri（视图）← RenderBatch+look、多面板、状态、输入、picker、Front
//! feathers_in_3d（渲染）← route B：SDF 几何 + 字形 + 排序 + 裁剪 + 拾取
//! ───────────────────────────────────────────────────────────────
//! 本 crate（harness 模板）= 把上面三层装配起来 + 插件/业务扩展点
//! ```
//!
//! ## 装了什么
//!
//! [`app`] 装的是**真的 harness 本体**（不是只有内核）：会话目录、slash 命令、
//! Ask/Pick、启动器协议、时钟、watch、run、progress 全部来自
//! [`rig_harness::prelude::RigHarnessPlugins`]；本 crate 额外装：
//!
//! * [`HarnessExtPlugin`]：`/hello` 与 `/setting` 命令、输入框 `/命令` 到
//!   [`RunCommand`] 的桥、[`Notice`] 到转写引擎的桥、真实状态行、`[view]` 设置；
//! * [`settings`]：启动时按上游路径规则读 `RIG_HOME/settings.toml` 的 `[view]`
//!   表，坏文件降级成默认值并 `warn!`，可用 `/setting` 改并写回磁盘；
//! * 状态条目全部来自 ECS（会话、agent 数、turn、对话消息数、后台任务、真实
//!   token），没有演示数字。
//!
//! 嵌入式 harness 必须自己做、少一件就起不来的三件装配（照 `examples/harness_boot.rs`）：
//!
//! 1. `DefaultPlugins` 的 log / task pool / time / frame count 被
//!    [`rig_harness::windowed`] 剥掉了，要按**任务池在最前**的顺序补回来
//!    （后面的插件在 build 阶段就会用 `IoTaskPool`）；
//! 2. 会话目录：`SessionPlugin` 只读不建，用
//!    [`Home::from_env`](rig_harness::harness_protocol::Home::from_env) +
//!    `new_session()` 建好，再插入 `SessionPaths`；路径只从 `RIG_HOME` 来；
//! 3. `TranscriptFont`：视图层的行同步系统要它，必须在 `PreStartup` 插入，
//!    否则那些系统会被参数校验跳过。
//!
//! ## 通知只有一条路
//!
//! `Notice` -> [`notice::NoticeHistory`] -> 转写引擎的 `TextId::Notice` -> `TranscriptRows`。
//! 应用层不再自己拼行塞 `TranscriptRows`（上一轮的 `NoticeLog` 已删）：顺序、
//! 折行、回滚都归引擎，和真对话一致。
//!
//! ## fork 之后你要改什么
//!
//! | 改什么 | 在哪 |
//! |---|---|
//! | 业务工具（`Tool` 实现 + `add_tool`） | 你的 `kernel.rs` |
//! | 工具卡片（look） | 你的 `looks.rs`，注册到 `RenderSystems::Render` |
//! | 面板内容（状态/输入/额外面板） | 写 `PanelRows`（见 `rig_niri::status/editor`） |
//! | 相机（手柄/键盘/陀螺仪） | `rig_niri::camera_rig` |
//! | 更多 slash 命令 | `.add_command("name", "帮助", 你的系统)` |
//! | 更多设置 | `[view]` 之外新开一段，照 [`settings`] 的 `add_setting` 写 |
//!
//! ## 不要改什么
//!
//! `feathers_in_3d`（渲染）与 `rig_niri`（视图）是上游跟踪层：它们的 API 与
//! rig 上游对齐，改动会破坏"随上游升级"的承诺。

use std::path::PathBuf;

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use bevy::text::FontSource;
use feathers_in_3d::FeathersIn3dPlugin;
use rig_ecs::prelude::Notice;
use rig_niri::{editor, pane, transcript};
use rig_harness::commands::{AppCommandsExt, NoArgs, RunCommand, SlashCommand};
use rig_harness::harness_protocol::Home;
use rig_harness::main_agent::{MainAgentQuery, main_agent};
use rig_harness::prelude::SessionPaths;

pub use rig_niri::prelude::*;

pub mod notice;
pub mod workspaces_demo;
pub mod settings;
pub mod status;

pub use notice::NoticeHistory;
pub use settings::{SettingsPath, ViewSettings};

/// 装好"真 harness + 视图 + 3D 渲染"的 App（不含业务工具）。
///
/// 会话目录与设置都从 `RIG_HOME`（[`Home::from_env`]）来；没有可写目录时直接
/// panic，并提示把 `RIG_HOME` 指到可写位置。设置文件不存在或读不了不 panic：
/// 用默认值继续（见 [`settings`]）。
///
/// ```no_run
/// use bevy::prelude::*;
///
/// #[derive(Default)]
/// struct MyBusinessPlugin;
///
/// impl Plugin for MyBusinessPlugin {
///     fn build(&self, _app: &mut App) {}
/// }
///
/// let mut app = rig_harness_in_3d::app("my agent");
/// app.add_plugins(MyBusinessPlugin);
/// app.run();
/// ```
/// 找到 `assets/` 目录：环境变量 → 当前目录 → 可执行文件旁边 → 往上找几层。
///
/// bevy 默认把资源根取成 `BEVY_ASSET_ROOT`，没有的话取编译期的 `CARGO_MANIFEST_DIR`。
/// 从别的目录启动二进制时这两条都可能失效，字体加载不到，界面上就一个字都没有。
fn assets_dir() -> std::path::PathBuf {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(root) = std::env::var_os("BEVY_ASSET_ROOT") {
        candidates.push(std::path::PathBuf::from(root).join("assets"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("assets"));
        candidates.push(cwd.join("rig_harness_in_3d/assets"));
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|d| d.to_path_buf());
        for _ in 0..5 {
            let Some(current) = dir else { break };
            candidates.push(current.join("assets"));
            candidates.push(current.join("rig_harness_in_3d/assets"));
            dir = current.parent().map(|d| d.to_path_buf());
        }
    }
    candidates
        .iter()
        .find(|path| path.join("fonts").is_dir())
        .cloned()
        // 一个都不像：交给 bevy 自己的默认值（并用启动日志里的 `[assets]` 提醒）。
        .unwrap_or_else(|| std::path::PathBuf::from("assets"))
}

pub fn app(title: &str) -> App {
    // 会话目录：SessionPlugin 只读不建（正常由 launcher 建），嵌入时自己开一个。
    //
    // 不 panic：`~/.rig` 在只读环境里（沙箱、容器、只读家目录）建不出会话，
    // 这时退到 `./.rig-home`——否则程序一启动就退出，看起来就是"窗口什么都没出现"。
    let (home, dir) = match Home::from_env().new_session() {
        Ok((_id, dir)) => (Home::from_env(), dir),
        Err(failure) => {
            let fallback = std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join(".rig-home");
            warn!(
                "[session] 默认 RIG_HOME 建不出会话（{failure}），改用 {}",
                fallback.display()
            );
            std::fs::create_dir_all(&fallback).ok();
            let home = Home::at(fallback);
            match home.new_session() {
                Ok((_id, dir)) => (home, dir),
                Err(failure) => {
                    error!(
                        "两条路都建不出会话（{failure}）：把 RIG_HOME 指向一个可写目录再跑"
                    );
                    std::process::exit(1);
                }
            }
        }
    };

    let mut app = App::new();
    // 系统出错按 rig-harness 的方式记成数据，而不是让 winit 停下来。
    app.set_error_handler(rig_harness::error::warn);
    app.insert_resource(SessionPaths(dir));
    // 设置：RIG_HOME/settings.toml（上游路径规则）；缺失/坏文件在插件 build 里降级。
    settings::install(&mut app, home.settings());
    // 输入历史落盘：`RIG_HOME/data/tui/history.jsonl`（跨会话按 ↑ 能翻出来）。
    // `FeathersViewPlugin` 装的是内存版插件，这里把文件路径交给它读。
    let history_file = home.data().join(rig_niri::history::HISTORY_FILE);
    app.insert_resource(rig_niri::history::PromptHistory::load(history_file));

    // 1. windowed() 假定 HeadlessPlugins 已经装过这几样；任务池必须最前：
    //    后面的插件在 build 阶段就会用 IoTaskPool。
    app.add_plugins((
        bevy::log::LogPlugin::default(),
        bevy::app::TaskPoolPlugin {
            task_pool_options: rig_harness::task_pools(),
        },
        bevy::time::TimePlugin,
        bevy::diagnostic::FrameCountPlugin,
    ));
    // 资源根：字体在 `assets/fonts/`，路径找不到就只有面板没有字——窗口看起来是空的。
    // 不再依赖 `BEVY_ASSET_ROOT` 环境变量：自己找一个能用的。
    let assets = assets_dir();
    info!("[assets] {}", assets.display());
    app.add_plugins(rig_harness::windowed(DefaultPlugins)
        .set(WindowPlugin {
            primary_window: Some(Window {
                title: title.to_string(),
                resolution: (1280, 800).into(),
                ..default()
            }),
            ..default()
        })
        .set(bevy::asset::AssetPlugin {
            file_path: assets.to_string_lossy().into_owned(),
            ..default()
        }));
    app.add_plugins(bevy_feathers::FeathersPlugins);
    app.add_plugins(workspaces_demo::WorkspacesDemoPlugin::default());
    // 渲染层
    app.add_plugins(FeathersIn3dPlugin);
    // harness 本体：session / commands / launcher / clock / watch / ask / pick / run / progress
    app.add_plugins(rig_harness::prelude::RigHarnessPlugins);
    // 视图层：替代 plugins/rig-tui 的 TuiViewPlugin
    app.add_plugins(rig_niri::FeathersViewPlugin);
    // 本模板的扩展：/hello、/setting、输入框命令桥、Notice -> 引擎、状态行、设置
    app.add_plugins(HarnessExtPlugin);
    // 2 + 3. 会话目录宣示与视图字体（字体必须在 PreStartup，否则视图系统被跳过）
    app.add_systems(PreStartup, (announce_session, insert_transcript_font).chain());
    app
}

/// 把当前会话目录写进日志：验收时 `[session]` 这一行就是会话真的开了的证据。
fn announce_session(paths: Res<SessionPaths>) {
    info!("[session] {}", paths.0.path().display());
}

/// 视图层的字体资源（Latin 等宽 + 中文回退）。
fn insert_transcript_font(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(transcript::TranscriptFont(FontSource::List(vec![
        asset_server
            .load(bevy_feathers::constants::fonts::MONO)
            .into(),
        asset_server
            .load::<Font>("fonts/NotoSansCJKsc-Subset.otf")
            .into(),
    ])));
}

/// harness 层的扩展点：会话、命令与设置文件。
///
/// 这些都直接问 [`App`] 的 world，而不是模板自己记一份：
/// `session_dir` 读真实的 [`SessionPaths`] 资源，`commands` 读出上游
/// [`AppCommandsExt::add_command`] 注册在 world 里的 [`SlashCommand`] 实体。
/// 因此业务插件后加的 `/命令` 也会出现在这里。
pub trait HarnessExt {
    /// 会话目录（对应 rig-harness 的 session store）；没有会话时为 `None`。
    fn session_dir(&self) -> Option<PathBuf>;
    /// `RIG_HOME/settings.toml` 的路径（本 app 保存设置写它）。
    fn settings_file(&self) -> Option<PathBuf>;
    /// 已注册的 slash 命令名，含前导 `/`（与上游 `Name` 一致），按名字排序；
    /// 被 `Disabled` 的命令不算。
    fn commands(&self) -> Vec<String>;
}

impl HarnessExt for App {
    fn session_dir(&self) -> Option<PathBuf> {
        self.world()
            .get_resource::<SessionPaths>()
            .map(|paths| paths.0.path().to_path_buf())
    }

    fn settings_file(&self) -> Option<PathBuf> {
        self.world()
            .get_resource::<SettingsPath>()
            .map(|path| path.0.clone())
    }

    fn commands(&self) -> Vec<String> {
        let world = self.world();
        let mut names: Vec<String> = world
            .iter_entities()
            .filter(|entity| entity.contains::<SlashCommand>())
            .filter(|entity| !entity.contains::<Disabled>())
            .filter_map(|entity| entity.get::<Name>().map(|name| name.as_str().to_owned()))
            .collect();
        names.sort();
        names
    }
}

/// `/hello` 命令的样板：收到命令后写一条 info [`Notice`]。
///
/// 这就是命令系统的全部契约：参数是 [`NoArgs`]（多打的文字会先被上游拒掉），
/// 回复就是命令运行期间写出的通知。
pub fn hello(In(args): In<NoArgs>, mut notices: MessageWriter<Notice>) {
    notices.write(Notice::info(args.agent, "Hello from rig_harness_in_3d."));
}

/// 本模板的 harness 扩展插件。
///
/// * 注册 `/hello`（上游 [`AppCommandsExt`]）与 `/setting`；
/// * 输入框里以 `/` 开头的提交 -> [`RunCommand`]（真命令链路，不伪造结果）；
/// * [`Notice`] -> [`NoticeHistory`] -> 转写引擎（`TextId::Notice`）：形成
///   "命令 → 通知 → 转写面板行"的链路，只有这一条；
/// * 状态行每帧从真实 ECS 资源刷新；
/// * `[view] hide_status` 真的隐藏状态面板。
///
/// 通知回填放在 [`rig_niri::FeathersSystems::Fit`]，排在
/// [`rig_niri::batch::collect_rows`] 之后、[`pane::window_transcript`] 之前：
/// 先让引擎拿到这一帧的对话行，再把通知历史交给引擎，最后才窗口化。
pub struct HarnessExtPlugin;

impl Plugin for HarnessExtPlugin {
    fn build(&self, app: &mut App) {
        // `[view]`：路径/默认值/降级都在 settings 模块里；必须在其他系统之前。
        settings::register(app);

        app.init_resource::<NoticeHistory>()
            // 视图层通常已经初始化；单独用本插件时也要能跑。
            .init_resource::<pane::TranscriptRows>()
            .init_resource::<rig_niri::panels::PanelRows>()
            .init_resource::<transcript::Transcript>()
            .init_resource::<rig_tui::AppStatus>()
            .add_command("hello", "Say hello", hello)
            .add_command(
                "setting",
                "Show or change a view setting: /setting hide_status <true|false>",
                settings::setting,
            )
            .add_systems(
                Update,
                run_typed_prompts.run_if(resource_exists::<Messages<PromptSubmitted>>),
            )
            .add_systems(
                Update,
                notice::notices_to_transcript
                    .in_set(rig_niri::FeathersSystems::Fit)
                    .after(rig_niri::batch::collect_rows)
                    .before(pane::window_transcript),
            )
            .add_systems(
                Update,
                status::sync_status_items
                    .in_set(rig_niri::FeathersSystems::Prepare)
                    .before(rig_niri::status::status_rows),
            )
            .add_systems(
                Update,
                status::apply_view_settings
                    .in_set(rig_niri::FeathersSystems::Draw)
                    .after(rig_niri::status::status_rows),
            );
    }
}

/// 输入框提交以 `/` 开头的文本时，交给上游命令系统跑（`/hello`、`/setting`
/// 就是这样到的）。发给哪个 agent 用 harness 的 `main_agent` 规则
/// （`CurrentAgentPlugin` 让它跟随 `ShownAgent`，也就是当前聚焦工作区的 agent）。
///
/// 不以 `/` 开头的提交不经过这里：视图层（[`rig_niri::editor::keys`]）直接
/// `Deliver` 给 `ShownAgent`，这里只保留斜杠命令这一条桥——所以命令不会变成
/// 一条普通消息。
fn run_typed_prompts(
    mut submitted: MessageReader<editor::PromptSubmitted>,
    agents: MainAgentQuery,
    mut commands: Commands,
) {
    for editor::PromptSubmitted(text) in submitted.read() {
        let trimmed = text.trim();
        let Some(line) = trimmed.strip_prefix('/') else {
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Some(agent) = main_agent(&agents) else {
            continue;
        };
        commands.trigger(RunCommand::new(agent, line.trim()));
    }
}

/// 业务插件都实现它，方便在 `main` 里一行装配。
pub trait BusinessPlugin: Plugin {}

/// 前端模块的再导出，业务代码只从这里拿。
pub mod prelude {
    pub use crate::{HarnessExt, HarnessExtPlugin, NoticeHistory, SettingsPath, ViewSettings, hello};
    pub use rig_niri::{
        batch::{BatchPlugin, CompletedCalls},
        editor::{Editor, PromptSubmitted, SubmittedPrompts},
        frame::{
            CellRect, FrameGeometry, FramePanel, FrameSize, Placement, PanelArea,
        },
        pane::{PaneScroll, TranscriptRows},
        panels::{KeyedRow, PanelId, PanelRows, PanelRoots},
        picker::PickerPlugin,
        status::StatusPlugin,
        theme::Theme,
        transcript::{Row, RowText, TranscriptFont},
    };
    pub use rig_tui::{
        CallState, LookLine, RenderBatch, RenderInput, RenderKey, RenderOutput, RenderRequest,
        RenderSystems, ToolRender, TranscriptRender,
    };
}

#[cfg(test)]
mod tests {
    //! 无窗口测试：`Minimal` 的插件集合里跑真内核 + 命令 + 通知 + 面板窗口化。
    //!
    //! 不建窗口、不渲染、不截图：断言全部落在 ECS 资源上（`Notice` 消息、
    //! `TranscriptRows`/`PanelRows` 的行数/行文本、`AppStatus` 的条目），这是
    //! 链路本身，而不是画面。

    use super::*;
    use bevy::app::TaskPoolPlugin;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyboardInput};
    use rig_core::conversation::Conversation as Talk;
    use rig_core::message::{AssistantContent, AssistantMessage, Message};
    use rig_ecs::prelude::{
        AgentId, Conversation, Deliver, DeliveryMode, Recalled, TurnOf, spawn_root,
    };
    use rig_niri::batch::BatchPlugin;
    use rig_niri::editor::SubmittedPrompts;
    use rig_niri::frame::{CellRect, PanelArea, PanelAreas, PanelInset, PanelRoots};
    use rig_niri::pane::PanePlugin;
    use rig_niri::panels::{PanelId, PanelRows};
    use rig_harness::harness_protocol::SessionDir;
    use rig_harness::main_agent::ShownAgent;
    use rig_harness::prelude::Settings;
    use rig_tui::{AppStatus, Side, StatusItem, Tone};

    /// 内核 + 本模板扩展（含 `/hello`、`/setting`），不带视图/窗口。
    fn headless_app() -> App {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.add_plugins(rig_ecs::AgentPlugin);
        // 测试不读真实 RIG_HOME：显式给空设置和一个临时路径。
        app.insert_resource(Settings::from_toml(""));
        app.insert_resource(SettingsPath(
            std::env::temp_dir().join("rig-harness-in-3d-headless-settings.toml"),
        ));
        app.add_plugins(HarnessExtPlugin);
        app
    }

    /// 内核 + 真对话引擎（`BatchPlugin`）+ 窗格 + 本模板扩展；不建窗口。
    ///
    /// 面板根与 10 行区域是手工放的：`window_transcript` 只依赖这两个资源，
    /// 这样测试断言的是"命令/通知 -> 引擎 -> 可见面板行"的数据链，而不是布局。
    fn engine_app() -> App {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.add_plugins(rig_ecs::AgentPlugin);
        app.add_plugins(PanePlugin);
        // 和生产顺序一致：HarnessExtPlugin 先于 main.rs 里的 BatchPlugin。
        app.insert_resource(Settings::from_toml(""));
        app.insert_resource(SettingsPath(
            std::env::temp_dir().join("rig-harness-in-3d-engine-settings.toml"),
        ));
        app.add_plugins(HarnessExtPlugin);
        app.add_plugins(BatchPlugin);
        // 窗格/批处理要的输入与资源；`SubmittedPrompts` 平时由 EditorPlugin 初始化。
        app.insert_resource(bevy::input::ButtonInput::<KeyCode>::default());
        app.init_resource::<PanelRows>()
            .init_resource::<SubmittedPrompts>();

        let root = app.world_mut().spawn(Name::new("panel:Transcript")).id();
        let mut roots = PanelRoots::default();
        roots.0.insert(PanelId::Transcript, root);
        app.insert_resource(roots);
        let mut areas = PanelAreas::default();
        areas.0.insert(
            root,
            PanelArea::new(CellRect::new(0, 0, 80, 10), PanelInset::default()),
        );
        app.insert_resource(areas);
        app
    }

    /// 面板行的文本（`KeyedRow` 里 `Line` 的 spans 拼起来）。
    fn panel_text(rows: &[KeyedRow]) -> String {
        rows.iter()
            .flat_map(|keyed| keyed.row.line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("")
    }

    /// 面板这一帧的行。
    fn transcript_rows(app: &App) -> Vec<KeyedRow> {
        app.world()
            .resource::<PanelRows>()
            .get(PanelId::Transcript)
            .to_vec()
    }

    /// 引擎写出的整段转写文本（未窗口化）。
    fn transcript_text(app: &App) -> String {
        app.world()
            .resource::<pane::TranscriptRows>()
            .0
            .iter()
            .map(|keyed| panel_text(std::slice::from_ref(keyed)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn status_text(app: &App, name: &str) -> Option<String> {
        app.world()
            .resource::<AppStatus>()
            .0
            .iter()
            .find(|item| item.name == name)
            .map(|item| item.text.clone())
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rig-harness-in-3d-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn settings_app(path: &std::path::Path) -> App {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.add_plugins(rig_ecs::AgentPlugin);
        settings::install(&mut app, path.to_path_buf());
        app.add_plugins(HarnessExtPlugin);
        app
    }

    #[test]
    fn harness_ext_reads_the_real_session_and_the_upstream_command_registry() {
        let mut app = headless_app();
        let dir = std::env::temp_dir().join(format!(
            "rig-harness-in-3d-session-{}",
            std::process::id()
        ));
        app.insert_resource(SessionPaths(SessionDir::at(dir.clone())));
        app.insert_resource(SettingsPath(dir.join("settings.toml")));

        assert_eq!(app.session_dir(), Some(dir.clone()));
        assert_eq!(app.settings_file(), Some(dir.join("settings.toml")));
        assert!(
            app.commands().iter().any(|name| name == "/hello"),
            "commands() 应该读出 add_command 注册的 /hello，实际 {:?}",
            app.commands()
        );
        assert!(
            app.commands().iter().any(|name| name == "/setting"),
            "commands() 应该读出 add_command 注册的 /setting，实际 {:?}",
            app.commands()
        );
    }

    #[test]
    fn the_hello_command_writes_an_info_notice_for_its_agent() {
        let mut app = headless_app();
        let agent = spawn_root(app.world_mut(), AgentId("main".into())).id();
        app.update();

        app.world_mut().trigger(RunCommand::new(agent, "hello"));
        app.world_mut().flush();

        let notices: Vec<Notice> = app
            .world()
            .resource::<Messages<Notice>>()
            .iter_current_update_messages()
            .cloned()
            .collect();
        assert!(
            notices.iter().any(|notice| notice.agent == Some(agent)
                && notice.level == rig_ecs::prelude::NoticeLevel::Info
                && notice.text.contains("Hello")),
            "命令应写出 info Notice，实际 {notices:?}"
        );
    }

    #[test]
    fn a_command_reaches_the_transcript_panel_as_a_notice_row() {
        let mut app = engine_app();
        let agent = spawn_root(app.world_mut(), AgentId("main".into())).id();
        app.update();
        assert!(
            transcript_rows(&app).is_empty(),
            "还没有通知时转写面板应为空"
        );

        // 真命令：RunCommand -> /hello -> Notice -> 引擎 TextId::Notice -> 面板行
        app.world_mut().trigger(RunCommand::new(agent, "hello"));
        app.update();

        let rows = transcript_rows(&app);
        assert!(!rows.is_empty(), "命令后转写面板应该有通知行");
        let text = panel_text(&rows);
        assert!(text.contains("Hello"), "面板行里应有通知文本，实际 {text:?}");
        assert_eq!(
            text.matches("Hello from rig_harness_in_3d.").count(),
            1,
            "通知只应出现一次，实际 {text:?}"
        );
        assert_eq!(app.world().resource::<NoticeHistory>().len(), 1);

        // 通知历史不是每帧重新追加的假数据：再跑两帧行数不变。
        app.update();
        app.update();
        let text = panel_text(&transcript_rows(&app));
        assert_eq!(
            text.matches("Hello from rig_harness_in_3d.").count(),
            1,
            "通知不能每帧重复追加，实际 {text:?}"
        );
        assert_eq!(app.world().resource::<NoticeHistory>().len(), 1);
    }

    #[test]
    fn a_notice_is_drawn_once_and_after_the_conversation_it_follows() {
        let mut app = engine_app();
        let agent = spawn_root(app.world_mut(), AgentId("main".into())).id();
        let mut talk = Talk::new();
        talk.push(Message::user("question from the conversation"));
        talk.push(Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::text("reply from the conversation"),
        ])));
        app.world_mut()
            .entity_mut(agent)
            .insert(Conversation(talk));
        app.insert_resource(ShownAgent(Some(agent)));
        app.update();

        app.world_mut()
            .write_message(Notice::info(Some(agent), "notice from the harness"));
        for _ in 0..3 {
            app.update();
            let text = transcript_text(&app);
            assert_eq!(
                text.matches("notice from the harness").count(),
                1,
                "构造的 Notice 只能出现一次，实际 {text:?}"
            );
            assert!(
                text.find("reply from the conversation")
                    .zip(text.find("notice from the harness"))
                    .is_some_and(|(reply, notice)| reply < notice),
                "通知应排在它跟随的对话之后，实际 {text:?}"
            );
        }
        assert_eq!(app.world().resource::<NoticeHistory>().len(), 1);
    }

    #[test]
    fn a_slash_prompt_from_the_editor_runs_the_command_and_reaches_the_panel() {
        let mut app = engine_app();
        // 输入面板（EditorPlugin）会在 build 里注册这条消息；这里只测桥本身，
        // 所以直接注册并写一条"用户提交了 /hello"的消息。
        app.add_message::<editor::PromptSubmitted>();
        spawn_root(app.world_mut(), AgentId("main".into()));
        app.update();
        assert!(transcript_rows(&app).is_empty());

        app.world_mut()
            .write_message(editor::PromptSubmitted("/hello".into()));
        // 命令由 Commands 队列在 Update 边界执行；通知可能下一帧才被桥读到。
        app.update();
        app.update();

        let rows = transcript_rows(&app);
        assert!(
            panel_text(&rows).contains("Hello"),
            "输入框 /hello 应经命令系统落到转写面板，实际 {:?}",
            panel_text(&rows)
        );
        assert_eq!(panel_text(&rows).matches("Hello").count(), 1);
    }

    #[test]
    fn a_missing_settings_file_uses_defaults() {
        let dir = temp_dir("settings-missing");
        let app = settings_app(&dir.join("settings.toml"));
        assert!(!app.world().resource::<ViewSettings>().hide_status);
        assert!(
            app.world().resource::<Settings>().problems().is_empty(),
            "缺失文件是空设置，不是错误"
        );
    }

    #[test]
    fn a_bad_settings_file_degrades_to_defaults_without_a_problem() {
        let dir = temp_dir("settings-bad");
        let path = dir.join("settings.toml");
        std::fs::write(&path, "this is not = [ toml").unwrap();
        let app = settings_app(&path);
        assert!(
            !app.world().resource::<ViewSettings>().hide_status,
            "坏文件应降级为默认值"
        );
        assert!(
            app.world().resource::<Settings>().problems().is_empty(),
            "降级后不能把 problems 留给 launcher 去拒绝启动"
        );
    }

    #[test]
    fn a_mistyped_view_table_degrades_to_defaults() {
        let dir = temp_dir("settings-typo");
        let path = dir.join("settings.toml");
        std::fs::write(&path, "[view]\nhide_status = \"maybe\"\n").unwrap();
        let app = settings_app(&path);
        assert!(
            !app.world().resource::<ViewSettings>().hide_status,
            "[view] 类型错应降级为默认值"
        );
        assert!(
            app.world().resource::<Settings>().problems().is_empty(),
            "降级后不能把 problems 留给 launcher"
        );
    }

    #[test]
    fn the_setting_command_changes_and_saves_the_file() {
        let dir = temp_dir("settings-command");
        let path = dir.join("settings.toml");
        std::fs::write(&path, "[view]\nhide_status = false\n").unwrap();
        let mut app = settings_app(&path);
        let agent = spawn_root(app.world_mut(), AgentId("main".into())).id();
        app.update();

        app.world_mut()
            .trigger(RunCommand::new(agent, "setting hide_status true"));
        app.update();

        assert!(
            app.world().resource::<ViewSettings>().hide_status,
            "/setting 应改内存里的设置"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("hide_status = true"),
            "设置应写回磁盘，实际 {text:?}"
        );
        let loaded = Settings::read(&path);
        assert!(
            loaded
                .get::<ViewSettings>("view")
                .expect("[view] 应能读回")
                .hide_status,
            "重新读文件应得到 true"
        );
    }

    #[test]
    fn saving_view_settings_preserves_other_tables() {
        let dir = temp_dir("settings-save");
        let path = dir.join("settings.toml");
        std::fs::write(
            &path,
            "[colour]\nname = \"x\"\n\n[view]\nhide_status = false\n\n[tail]\nn = 1\n",
        )
        .unwrap();

        settings::save(&path, &ViewSettings { hide_status: true }).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[colour]\nname = \"x\""), "其他表要保留：{text:?}");
        assert!(text.contains("[tail]\nn = 1"), "后面的表要保留：{text:?}");
        assert_eq!(text.matches("[view]").count(), 1, "不能写两份 [view]：{text:?}");
        assert!(text.contains("hide_status = true"));
        assert!(!text.contains("hide_status = false"));

        let loaded = Settings::read(&path);
        assert!(loaded.problems().is_empty(), "保存后的文件应是合法 TOML");
        assert!(loaded.get::<ViewSettings>("view").unwrap().hide_status);
    }

    #[test]
    fn saving_settings_creates_a_missing_file() {
        let dir = temp_dir("settings-create");
        let path = dir.join("settings.toml");
        assert!(!path.exists());

        settings::save(&path, &ViewSettings { hide_status: true }).unwrap();

        let loaded = Settings::read(&path);
        assert!(loaded.problems().is_empty(), "新建的文件应是合法 TOML");
        assert!(loaded.get::<ViewSettings>("view").unwrap().hide_status);
    }

    #[test]
    fn hide_status_setting_really_hides_the_status_panel() {
        let dir = temp_dir("settings-hide-status");
        let path = dir.join("settings.toml");
        std::fs::write(&path, "[view]\nhide_status = true\n").unwrap();
        let mut app = settings_app(&path);
        app.add_plugins(rig_niri::status::StatusPlugin);
        app.init_resource::<PanelRows>()
            .init_resource::<PanelAreas>()
            .init_resource::<PanelRoots>();
        spawn_root(app.world_mut(), AgentId("main".into()));

        app.update();
        assert!(
            app.world()
                .resource::<PanelRows>()
                .get(PanelId::Status)
                .is_empty(),
            "hide_status = true 时状态面板应为空"
        );

        app.world_mut()
            .resource_mut::<ViewSettings>()
            .hide_status = false;
        app.update();
        assert!(
            !app.world()
                .resource::<PanelRows>()
                .get(PanelId::Status)
                .is_empty(),
            "hide_status = false 时状态面板应重新出现"
        );
    }

    #[test]
    fn status_items_come_from_real_ecs_resources() {
        let mut app = headless_app();
        let dir = temp_dir("status").join("1700000000-42");
        app.insert_resource(SessionPaths(SessionDir::at(dir)));
        let agent = spawn_root(app.world_mut(), AgentId("main".into())).id();

        let mut talk = Talk::new();
        talk.push(Message::user("one"));
        talk.push(Message::user("two"));
        app.world_mut()
            .entity_mut(agent)
            .insert(Conversation(talk));
        // 真 `TurnOf`：关系目标 `ActiveTurn` 会插到 agent 上。
        app.world_mut().spawn(TurnOf(agent));
        // 别的插件已有的条目不能被状态同步覆盖。
        app.world_mut().resource_mut::<AppStatus>().0.show(
            StatusItem::new("other", Side::Right, 99, 10).says("other plugin", Tone::Plain),
        );

        app.update();

        let session = status_text(&app, "session").expect("session 条目");
        assert!(session.contains("1700000000-42"), "会话 id 来自 SessionPaths：{session:?}");
        assert_eq!(status_text(&app, "agents").as_deref(), Some("1 agent"));
        assert_eq!(status_text(&app, "turn").as_deref(), Some("turn running"));
        assert_eq!(
            status_text(&app, "conversation").as_deref(),
            Some("2 msgs")
        );
        assert_eq!(status_text(&app, "tasks").as_deref(), Some("0 tasks"));
        assert_eq!(
            status_text(&app, "tokens"),
            None,
            "没有模型的真实 token 时不画假数字"
        );
        assert_eq!(
            status_text(&app, "other").as_deref(),
            Some("other plugin"),
            "别的插件的 AppStatus 条目不能被覆盖"
        );

        let all: String = app
            .world()
            .resource::<AppStatus>()
            .0
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        for fake in ["312 tokens", "ctx 12%", "no model"] {
            assert!(!all.contains(fake), "演示数据 `{fake}` 不能再出现：{all:?}");
        }
    }

    // ---- 输入框 -> 内核：Deliver / RunCommand / AppExit ----

    /// 视图输入层（真按键表 + 发送/排队/停止/退出）+ 内核 + 本模板扩展；
    /// 不建窗口、不渲染。
    fn typing_app() -> App {
        let mut app = headless_app();
        app.add_plugins(rig_niri::editor::EditorPlugin);
        // EditorPlugin 的输入行系统要 Theme；生产里由 `FeathersViewPlugin` 初始化。
        app.init_resource::<rig_niri::theme::Theme>()
            .insert_resource(bevy::input::ButtonInput::<KeyCode>::default())
            .add_message::<KeyboardInput>();
        // AgentPlugin 会装 TimePlugin，消息的 `update` 不再每帧清（`Messages` 的
        // 两帧窗口因此不可靠）；用 `MessageReader` 收 `AppExit`，和真实插件一样。
        app.init_resource::<Exits>()
            .add_systems(Update, watch_exits);
        app
    }

    /// 收到过的 `AppExit`。
    #[derive(Resource, Default)]
    struct Exits(Vec<AppExit>);

    fn watch_exits(mut exits: MessageReader<AppExit>, mut seen: ResMut<Exits>) {
        seen.0.extend(exits.read().cloned());
    }

    /// 观察到的 `Deliver`。
    #[derive(Resource, Default)]
    struct Delivered(Vec<(Entity, String, DeliveryMode)>);

    fn record_deliver(deliver: On<Deliver>, mut delivered: ResMut<Delivered>) {
        delivered.0.push((
            deliver.entity,
            deliver.message.text().to_owned(),
            deliver.message.mode,
        ));
    }

    /// 给 app 挂上 `Deliver` 观察者。
    fn watching(app: &mut App) {
        app.init_resource::<Delivered>()
            .add_observer(record_deliver);
    }

    fn key(app: &mut App, code: KeyCode, key: Key, text: Option<&str>) {
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

    /// 像用户那样一个字符一个字符打，最后按 Enter。
    fn type_prompt(app: &mut App, text: &str) {
        for character in text.chars() {
            let typed = character.to_string();
            key(
                app,
                KeyCode::KeyA, // 测试只关心字符，不关心物理键
                Key::Character(typed.clone().into()),
                Some(&typed),
            );
        }
        key(app, KeyCode::Enter, Key::Enter, None);
    }

    /// 这几帧里内核写出的通知。
    fn said(app: &mut App, frames: usize) -> Vec<Notice> {
        let mut notices: Vec<Notice> = Vec::new();
        for _ in 0..frames {
            app.update();
            notices.extend(
                app.world()
                    .resource::<Messages<Notice>>()
                    .iter_current_update_messages()
                    .cloned(),
            );
        }
        notices
    }

    #[test]
    fn a_typed_prompt_reaches_the_kernel_and_comes_back_as_recalled() {
        let mut app = typing_app();
        let agent = spawn_root(app.world_mut(), AgentId("main".into())).id();
        app.insert_resource(ShownAgent(Some(agent)));
        app.update();

        type_prompt(&mut app, "  hello from the editor  ");

        // 没有接模型：这一轮在 Start 失败，内核把用户这条消息作为 `Recalled` 还
        // 回来。它出现说明 Deliver 真的进了内核——不只是被测试的观察者看见。
        let mut recalled: Vec<Recalled> = Vec::new();
        for _ in 0..6 {
            app.update();
            recalled.extend(
                app.world()
                    .resource::<Messages<Recalled>>()
                    .iter_current_update_messages()
                    .cloned(),
            );
        }
        assert!(
            recalled
                .iter()
                .any(|recall| recall.agent == agent && recall.text == "hello from the editor"),
            "输入框的文本应经 Deliver 进入内核（没接模型时以 Recalled 返回），实际 {recalled:?}"
        );
    }

    #[test]
    fn a_typed_slash_command_runs_and_is_not_delivered() {
        let mut app = typing_app();
        let agent = spawn_root(app.world_mut(), AgentId("main".into())).id();
        app.insert_resource(ShownAgent(Some(agent)));
        watching(&mut app);
        app.update();

        type_prompt(&mut app, "/hello");

        let notices = said(&mut app, 4);
        assert!(
            notices
                .iter()
                .any(|notice| notice.agent == Some(agent) && notice.text.contains("Hello")),
            "斜杠命令应仍走 RunCommand（/hello 写通知），实际 {notices:?}"
        );
        assert!(
            app.world().resource::<Delivered>().0.is_empty(),
            "斜杠命令不能再额外送一条普通消息，实际 {:?}",
            app.world().resource::<Delivered>().0
        );
        assert_eq!(
            app.world().resource::<SubmittedPrompts>().0,
            ["/hello"],
            "本地回显照旧"
        );
    }

    #[test]
    fn ctrl_d_on_an_empty_input_quits_the_app() {
        let mut app = typing_app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ControlLeft);
        key(&mut app, KeyCode::KeyD, Key::Character("d".into()), None);
        app.update();
        assert_eq!(
            app.world().resource::<Exits>().0,
            [AppExit::Success],
            "空输入 Ctrl+D 应触发 AppExit"
        );

        // 有字时不退。
        app.world_mut().resource_mut::<Editor>().set("draft");
        key(&mut app, KeyCode::KeyD, Key::Character("d".into()), None);
        app.update();
        assert_eq!(
            app.world().resource::<Exits>().0,
            [AppExit::Success],
            "有字时 Ctrl+D 不能再写一条 AppExit"
        );
        assert_eq!(
            app.world().resource::<Editor>().text,
            "draft",
            "有字时 Ctrl+D 也不清空"
        );
    }
}
