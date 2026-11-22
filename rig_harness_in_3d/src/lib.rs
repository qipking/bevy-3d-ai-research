//! `rig_harness_in_3d`：把 rig 的**内核 + 前端 + 3D 渲染**装成一个可直接 fork 的 app 骨架。
//!
//! 对应 rig 官方分层里的 **harness 层**（`rig-harness` 之于 `rig-tui`）：
//!
//! ```text
//! rig-ecs（内核）  ← AgentPlugin、agent/turn/call 效应、Gate、Ask
//! rig-feathers（视图）← RenderBatch+look、多面板、状态、输入、picker、Front
//! feathers_in_3d（渲染）← route B：SDF 几何 + 字形 + 排序 + 裁剪 + 拾取
//! ───────────────────────────────────────────────────────────────
//! 本 crate（harness 模板）= 把上面三层装配起来 + 插件/业务扩展点
//! ```
//!
//! ## fork 之后你要改什么
//!
//! | 改什么 | 在哪 |
//! |---|---|
//! | 业务工具（`Tool` 实现 + `add_tool`） | 你的 `kernel.rs` |
//! | 工具卡片（look） | 你的 `looks.rs`，注册到 `RenderSystems::Render` |
//! | 面板内容（状态/输入/额外面板） | 写 `PanelRows`（见 `rig_feathers::status/editor`） |
//! | 相机（手柄/键盘/陀螺仪） | `rig_feathers::camera_rig` |
//! | 会话/设置/命令 | 这里留的扩展点（见 [`HarnessExt`]） |
//!
//! ## 不要改什么
//!
//! `feathers_in_3d`（渲染）与 `rig_feathers`（视图）是上游跟踪层：它们的 API 与
//! rig 上游对齐，改动会破坏"随上游升级"的承诺。

use bevy::prelude::*;
use rig_feathers::FeathersViewPlugin;
use feathers_in_3d::FeathersIn3dPlugin;

pub use rig_feathers::prelude::*;

/// 装好"内核 + 视图 + 3D 渲染"的 App（不含任何业务工具）。
///
/// ```no_run
/// let mut app = rig_harness_in_3d::app("my agent");
/// app.add_plugins(MyBusinessPlugin);
/// app.run();
/// ```
pub fn app(title: &str) -> App {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: title.to_string(),
            resolution: (1280, 800).into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(bevy_feathers::FeathersPlugins)
    .add_plugins(FeathersIn3dPlugin)
    .add_plugins(FeathersViewPlugin)
    .add_plugins(rig_ecs::AgentPlugin)
    // 前端要回答 Ask / 展示 Picker；没有这两个，闸门会直接拒绝
    .add_plugins(rig_harness::ask::AskPlugin)
    .add_plugins(rig_harness::pick::PickPlugin)
    .add_systems(Startup, seed_status_items);
    app
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

/// harness 层的扩展点：会话、设置、slash 命令、工作区。
///
/// rig-harness 把这些做成"每个插件共享的服务"；3D app 通常只要其中几项，
/// 所以模板不预置，留成 trait 让业务按需实现。
pub trait HarnessExt {
    /// 会话目录与恢复（对应 rig-harness 的 session store）。
    fn session_dir(&self) -> Option<std::path::PathBuf> {
        None
    }
    /// slash 命令（`/clear`、`/model` …），返回命令名。
    fn commands(&self) -> &'static [&'static str] {
        &[]
    }
}

/// 业务插件都实现它，方便在 `main` 里一行装配。
pub trait BusinessPlugin: Plugin {}

/// 前端模块的再导出，业务代码只从这里拿。
pub mod prelude {
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
