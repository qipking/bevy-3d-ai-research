//! `rig_harness_in_3d`：把 rig 的**内核 + harness + 前端 + 3D 渲染**装成一个可直接 fork 的 app 骨架。
//!
//! 对应 rig 官方分层里的 **harness 层**（`rig-harness` 之于 `plugins/rig-tui`）：
//!
//! ```text
//! rig-ecs（内核）  ← AgentPlugin、agent/turn/call 效应、Gate、Ask
//! rig-harness（harness）← 会话、slash 命令、Ask/Pick、启动器协议、服务
//! rig-feathers（视图）← RenderBatch+look、多面板、状态、输入、picker、Front
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
//! * [`HarnessExtPlugin`]：`/hello` 命令样板、输入框 `/命令` 到
//!   [`RunCommand`] 的桥、[`Notice`] 到转写行的桥；
//! * `seed_status_items`：演示用的状态条目（真应用里由各插件自己
//!   `StatusItems::show`）。
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
//! ## fork 之后你要改什么
//!
//! | 改什么 | 在哪 |
//! |---|---|
//! | 业务工具（`Tool` 实现 + `add_tool`） | 你的 `kernel.rs` |
//! | 工具卡片（look） | 你的 `looks.rs`，注册到 `RenderSystems::Render` |
//! | 面板内容（状态/输入/额外面板） | 写 `PanelRows`（见 `rig_feathers::status/editor`） |
//! | 相机（手柄/键盘/陀螺仪） | `rig_feathers::camera_rig` |
//! | 更多 slash 命令 | `.add_command("name", "帮助", 你的系统)` |
//!
//! ## 不要改什么
//!
//! `feathers_in_3d`（渲染）与 `rig_feathers`（视图）是上游跟踪层：它们的 API 与
//! rig 上游对齐，改动会破坏"随上游升级"的承诺。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use bevy::text::FontSource;
use feathers_in_3d::FeathersIn3dPlugin;
use rig_ecs::prelude::{Agent, Notice, NoticeLevel};
use rig_feathers::{editor, pane, panels, transcript};
use rig_harness::commands::{AppCommandsExt, NoArgs, RunCommand, SlashCommand};
use rig_harness::harness_protocol::Home;
use rig_harness::main_agent::{MainAgentQuery, main_agent};
use rig_harness::prelude::SessionPaths;
use rig_tui::ratatui::style::{Color as RColor, Style};
use rig_tui::ratatui::text::{Line, Span};

pub use rig_feathers::prelude::*;

/// 装好"真 harness + 视图 + 3D 渲染"的 App（不含业务工具）。
///
/// 会话目录从 `RIG_HOME`（[`Home::from_env`]）来；没有可写目录时直接 panic，
/// 并提示把 `RIG_HOME` 指到可写位置。
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
pub fn app(title: &str) -> App {
    // 会话目录：SessionPlugin 只读不建（正常由 launcher 建），嵌入时自己开一个。
    let home = Home::from_env();
    let (_id, dir) = home.new_session().unwrap_or_else(|failure| {
        panic!(
            "cannot create a session in {}: {failure}（把 RIG_HOME 指向可写目录再跑）",
            home.root().display()
        )
    });

    let mut app = App::new();
    // 系统出错按 rig-harness 的方式记成数据，而不是让 winit 停下来。
    app.set_error_handler(rig_harness::error::warn);
    app.insert_resource(SessionPaths(dir));

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
    app.add_plugins(rig_harness::windowed(DefaultPlugins).set(WindowPlugin {
        primary_window: Some(Window {
            title: title.to_string(),
            resolution: (1280, 800).into(),
            ..default()
        }),
        ..default()
    }));
    app.add_plugins(bevy_feathers::FeathersPlugins);
    // 渲染层
    app.add_plugins(FeathersIn3dPlugin);
    // harness 本体：session / commands / launcher / clock / watch / ask / pick / run / progress
    app.add_plugins(rig_harness::prelude::RigHarnessPlugins);
    // 视图层：替代 plugins/rig-tui 的 TuiViewPlugin
    app.add_plugins(rig_feathers::FeathersViewPlugin);
    // 本模板的扩展：/hello、输入框命令桥、Notice -> 转写
    app.add_plugins(HarnessExtPlugin);
    // 2 + 3. 会话目录宣示与视图字体（字体必须在 PreStartup，否则视图系统被跳过）
    app.add_systems(PreStartup, (announce_session, insert_transcript_font).chain())
        .add_systems(Startup, seed_status_items);
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

/// 给状态行放几个 rig 风格的条目（真应用里由插件各自 `StatusItems::show`）。
fn seed_status_items(mut app_status: ResMut<rig_tui::AppStatus>) {
    use rig_tui::{Side, StatusItem, Tone};
    app_status
        .0
        .show(StatusItem::new("model", Side::Left, 10, 5).says("no model", Tone::Bold));
    app_status
        .0
        .show(StatusItem::new("tokens", Side::Left, 20, 4).says("312 tokens", Tone::Dim));
    app_status
        .0
        .show(StatusItem::new("context", Side::Right, 40, 5).says("ctx 12%", Tone::Green));
    app_status
        .0
        .show(StatusItem::new("agents", Side::Right, 50, 3).says("1 agent", Tone::Dim));
}

/// harness 层的扩展点：会话与 slash 命令。
///
/// 这两个问题都直接问 [`App`] 的 world，而不是模板自己记一份：
/// `session_dir` 读真实的 [`SessionPaths`] 资源，`commands` 读出上游
/// [`AppCommandsExt::add_command`] 注册在 world 里的 [`SlashCommand`] 实体。
/// 因此业务插件后加的 `/命令` 也会出现在这里。
pub trait HarnessExt {
    /// 会话目录（对应 rig-harness 的 session store）；没有会话时为 `None`。
    fn session_dir(&self) -> Option<PathBuf>;
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

/// 通知在转写里的历史（真实数据：每条 [`Notice`] 的文本行）。
///
/// 上游 `rig-tui` 把 `Notice` 长期留在转写里；这里同样保留，面板的窗口化
/// （[`pane::TranscriptRows`] -> `PanelRows`）每帧从这段历史尾部取可见窗口。
#[derive(Resource, Default)]
pub struct NoticeLog {
    rows: Vec<panels::KeyedRow>,
    /// 下一个行 key。用高位段，和 batch 里 `DefaultHasher` 生成的 key 分开，
    /// 增量 diff 时不会把通知行和工具行对错。
    next_key: u64,
}

impl NoticeLog {
    /// 行 key 的高位段（"notice" 的 ASCII 起头，便于看日志时辨认）。
    const KEY_BASE: u64 = 0x6e6f_7469_0000_0000;

    /// 历史里的行数。
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 历史里的行（只读，测试断言用）。
    pub fn rows(&self) -> &[panels::KeyedRow] {
        &self.rows
    }

    /// 把一条通知按行记进历史；每种级别用上游 `notice_lines` 的同一种颜色。
    fn remember(&mut self, notice: &Notice) {
        let style = match notice.level {
            NoticeLevel::Info => Style::default().fg(RColor::Magenta),
            NoticeLevel::Warn => Style::default().fg(RColor::Yellow),
            NoticeLevel::Error => Style::default().fg(RColor::Red),
        };
        let mut lines: Vec<&str> = notice.text.lines().collect();
        if lines.is_empty() {
            lines.push("");
        }
        for line in lines {
            let row = transcript::Row::new(Line::from(Span::styled(line.to_string(), style)));
            let hash = hash_row(&row);
            debug_assert!(self.next_key < 1 << 32, "通知行 key 的计数器溢出低位段");
            let key = Self::KEY_BASE | self.next_key;
            self.next_key += 1;
            self.rows.push(panels::KeyedRow { key, hash, row });
        }
    }
}

/// 行内容的 hash（和 `rig_feathers::batch` 里增量刷新用的是同一套判定：
/// 文本 + 样式都变才算变）。
fn hash_row(row: &transcript::Row) -> u64 {
    let mut hasher = DefaultHasher::new();
    for span in &row.line.spans {
        span.content.hash(&mut hasher);
        format!("{:?}", span.style).hash(&mut hasher);
    }
    for span in row.right.iter().flat_map(|right| right.spans.iter()) {
        span.content.hash(&mut hasher);
        format!("{:?}", span.style).hash(&mut hasher);
    }
    hasher.finish()
}

/// 本模板的 harness 扩展插件。
///
/// * 注册 `/hello`（上游 [`AppCommandsExt`]）；
/// * 输入框里以 `/` 开头的提交 -> [`RunCommand`]（真命令链路，不伪造结果）；
/// * [`Notice`] -> [`NoticeLog`] -> [`pane::TranscriptRows`]：形成
///   "命令 → 通知 → 转写面板行"的链路。
///
/// 通知行的合成放在 [`rig_feathers::FeathersSystems::Fit`]，且排在
/// [`pane::window_transcript`] 之前：先把通知接在整段转写末尾，再由窗格按面板高度
/// 窗口化写入 `PanelRows`。批处理插件（demo 的 `BatchPlugin`）每帧在 `Draw`
/// 里重建 `TranscriptRows`，所以这里每帧都把完整通知历史重新接一遍，并先剥掉
/// 上一帧接的那段（key 完全一致才剥），两种来源都不会重复。
pub struct HarnessExtPlugin;

impl Plugin for HarnessExtPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NoticeLog>()
            // 视图层通常已经由 PanePlugin 初始化；单独用本插件时也要能跑。
            .init_resource::<pane::TranscriptRows>()
            .add_command("hello", "Say hello", hello)
            .add_systems(
                Update,
                run_typed_prompts.run_if(resource_exists::<Messages<PromptSubmitted>>),
            )
            .add_systems(
                Update,
                notices_to_transcript
                    .in_set(rig_feathers::FeathersSystems::Fit)
                    .before(pane::window_transcript),
            );
    }
}

/// 输入框提交以 `/` 开头的文本时，交给上游命令系统跑（`/hello` 就是这样到
/// [`hello`] 的）。发给哪个 agent 用 harness 的 `main_agent` 规则。
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

/// 收集新通知，并把完整通知历史接在转写末尾（`Fit`，窗口化之前）。
fn notices_to_transcript(
    mut notices: MessageReader<Notice>,
    agents: Query<(), With<Agent>>,
    mut log: ResMut<NoticeLog>,
    mut transcript: ResMut<pane::TranscriptRows>,
    // 上一帧接在 TranscriptRows 尾部的通知行 key。
    mut appended: Local<Vec<u64>>,
) {
    for notice in notices.read() {
        // 关于某个已经不在的 agent 的通知不显示（上游同样的过滤）。
        if let Some(agent) = notice.agent
            && !agents.contains(agent)
        {
            continue;
        }
        log.remember(notice);
    }

    // 先剥掉上一帧接的那段（只有 key 对得上才剥：批处理插件这一帧重建过转写，
    // 尾巴就对不上，整段都是新的基础行）。
    if !appended.is_empty() {
        let start = transcript.0.len().saturating_sub(appended.len());
        let is_ours = transcript.0.len() >= appended.len()
            && transcript.0[start..]
                .iter()
                .map(|row| row.key)
                .eq(appended.iter().copied());
        if is_ours {
            transcript.0.truncate(start);
        }
    }

    transcript.0.extend(log.rows.iter().cloned());
    appended.clear();
    appended.extend(log.rows.iter().map(|row| row.key));
}

/// 业务插件都实现它，方便在 `main` 里一行装配。
pub trait BusinessPlugin: Plugin {}

/// 前端模块的再导出，业务代码只从这里拿。
pub mod prelude {
    pub use crate::{HarnessExt, HarnessExtPlugin, NoticeLog, hello};
    pub use rig_feathers::{
        batch::{BatchPlugin, CompletedCalls},
        editor::{Editor, PromptSubmitted, SubmittedPrompts},
        frame::{
            CellRect, Extent, FrameGeometry, FramePanel, FrameSize, Placement, PanelArea,
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
    //! `PanelRows` 的行数/行文本），这是链路本身，而不是画面。

    use super::*;
    use bevy::app::TaskPoolPlugin;
    use rig_ecs::prelude::{AgentId, spawn_root};
    use rig_feathers::FeathersSystems;
    use rig_feathers::frame::{CellRect, PanelArea, PanelAreas, PanelInset, PanelRoots};
    use rig_feathers::pane::TranscriptRows;
    use rig_feathers::panels::{PanelId, PanelRows};
    use rig_harness::harness_protocol::SessionDir;

    /// 内核 + 本模板扩展（含 `/hello`），不带视图/窗口。
    fn headless_app() -> App {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.add_plugins(rig_ecs::AgentPlugin);
        app.add_plugins(HarnessExtPlugin);
        app
    }

    /// 内核 + 命令 + 通知 + 窗格窗口化；不要 UI/渲染/输入插件。
    ///
    /// 面板根与 10 行区域是手工放的：`window_transcript` 只依赖这两个资源，
    /// 这样测试断言的是"通知 -> 转写 -> 可见面板行"的数据链，而不是布局。
    fn transcript_app() -> App {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.add_plugins(rig_ecs::AgentPlugin);
        app.add_plugins(rig_feathers::pane::PanePlugin);
        app.add_plugins(HarnessExtPlugin);
        // 窗格插件不装输入/面板插件，补它校验的这两个资源。
        app.insert_resource(bevy::input::ButtonInput::<KeyCode>::default());
        app.insert_resource(PanelRows::default());

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

    #[test]
    fn harness_ext_reads_the_real_session_and_the_upstream_command_registry() {
        let mut app = headless_app();
        let dir = std::env::temp_dir().join(format!(
            "rig-harness-in-3d-session-{}",
            std::process::id()
        ));
        app.insert_resource(SessionPaths(SessionDir::at(dir.clone())));

        assert_eq!(app.session_dir(), Some(dir));
        assert!(
            app.commands().iter().any(|name| name == "/hello"),
            "commands() 应该读出 add_command 注册的 /hello，实际 {:?}",
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
                && notice.level == NoticeLevel::Info
                && notice.text.contains("Hello")),
            "命令应写出 info Notice，实际 {notices:?}"
        );
    }

    #[test]
    fn a_command_reaches_the_transcript_panel_as_a_notice_row() {
        let mut app = transcript_app();
        let agent = spawn_root(app.world_mut(), AgentId("main".into())).id();
        app.update();
        assert!(
            transcript_rows(&app).is_empty(),
            "还没有通知时转写面板应为空"
        );

        // 真命令：RunCommand -> /hello -> Notice -> NoticeLog -> TranscriptRows -> PanelRows
        app.world_mut().trigger(RunCommand::new(agent, "hello"));
        app.update();

        let rows = transcript_rows(&app);
        assert!(!rows.is_empty(), "命令后转写面板应该有通知行");
        let text = panel_text(&rows);
        assert!(text.contains("Hello"), "面板行里应有通知文本，实际 {text:?}");
        let shown = rows.len();

        // 通知历史不是每帧重新追加的假数据：再跑两帧行数不变。
        app.update();
        app.update();
        assert_eq!(
            transcript_rows(&app).len(),
            shown,
            "通知行不能每帧重复追加"
        );
        assert_eq!(app.world().resource::<NoticeLog>().len(), shown);
    }

    #[test]
    fn a_slash_prompt_from_the_editor_runs_the_command_and_reaches_the_panel() {
        let mut app = transcript_app();
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
    }

    #[test]
    fn a_rebuilt_transcript_keeps_the_notice_history() {
        // 模拟 demo 的 BatchPlugin：它每帧在 Draw 里把 TranscriptRows 整个重建。
        fn rebuild_like_batch(mut transcript: ResMut<TranscriptRows>) {
            transcript.0 = Vec::new();
        }

        let mut app = transcript_app();
        // 没有 FeathersViewPlugin 时自己把 Draw -> Fit 的顺序配上；
        // 生产环境里这条链由 FeathersViewPlugin 配。
        app.configure_sets(
            Update,
            (FeathersSystems::Draw, FeathersSystems::Fit).chain(),
        )
        .add_systems(Update, rebuild_like_batch.in_set(FeathersSystems::Draw));

        let agent = spawn_root(app.world_mut(), AgentId("main".into())).id();
        app.update();
        app.world_mut().trigger(RunCommand::new(agent, "hello"));
        for _ in 0..3 {
            app.update();
            let text = panel_text(&transcript_rows(&app));
            assert!(
                text.contains("Hello"),
                "重建转写后通知行仍在（实际 {text:?}）"
            );
        }
        assert_eq!(app.world().resource::<NoticeLog>().len(), 1);
    }
}
