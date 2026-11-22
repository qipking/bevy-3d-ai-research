//! 验证：**这个 app 到底是不是一个 rig harness**。
//!
//! 现在 `rig_harness_in_3d::app()` 只装了 `rig_ecs::AgentPlugin`，等于只装了内核，
//! 没装 harness（services / session / commands / launcher / clock / watch / run / progress）。
//! 这个例子按上游推荐的方式装：`rig_harness::windowed(DefaultPlugins)` + `RigHarnessPlugins`
//! + 我们自己的视图（`FeathersViewPlugin`）和渲染（`FeathersIn3dPlugin`）——
//! 也就是用我们的视图**替掉** `plugins/rig-tui` 的 `TuiViewPlugin`。
//!
//! 嵌入式 harness 必须自己做的三件装配（缺一件就起不来或报错）：
//!
//! 1. `windowed()` 剥掉了 log / task pool / frame count / time（它假定先装了
//!    `HeadlessPlugins`），winit 版要按**任务池在最前**的顺序补回来；
//! 2. 会话目录：`SessionPlugin` 只读 `SessionPaths` / `RigHome`，**不建目录**
//!    （正常由启动器建）。嵌入时用 `Home::new_session()` 建好再插进去；
//! 3. `TranscriptFont`：视图层要它，之前只有 demo 的 `main.rs` 放，
//!    所以插件本身跑不起来。
//!
//! 跑法（在 `rig_harness_in_3d` 目录）：
//!
//! ```sh
//! BEVY_ASSET_ROOT=$PWD timeout 20 cargo run --example harness_boot
//! ```
//!
//! 成功的标准不是画面：能启动、会话建好、命令/闸门可用、没有参数校验告警。

use bevy::prelude::*;
use bevy::text::FontSource;
use rig_feathers::transcript::TranscriptFont;
use rig_harness::harness_protocol::Home;
use rig_harness::prelude::SessionPaths;

fn main() -> AppExit {
    let mut app = App::new();
    app.set_error_handler(rig_harness::error::warn);

    // 2. 会话目录：嵌入式 harness 得自己开一个 session。
    let home = Home::from_env();
    let (_id, dir) = match home.new_session() {
        Ok(session) => session,
        Err(failure) => {
            eprintln!("cannot create a session in {}: {failure}", home.root().display());
            return AppExit::error();
        }
    };
    println!("session dir = {}", dir.path().display());
    app.insert_resource(SessionPaths(dir));

    // 1. `windowed()` 假定你已经装了 `HeadlessPlugins`（它自带这些，外加无窗口 runner）。
    //    我们装的是 winit 的 runner，所以这几样要自己补回来；任务池用 harness 的尺寸，
    //    并且必须排在最前：后面的插件在 build 阶段就会用到 IoTaskPool。
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
            title: "rig harness (3D view)".to_string(),
            resolution: (1280, 800).into(),
            ..default()
        }),
        ..default()
    }));
    app.add_plugins(bevy_feathers::FeathersPlugins);
    // 渲染层
    app.add_plugins(feathers_in_3d::FeathersIn3dPlugin);
    // harness 本体
    app.add_plugins(rig_harness::prelude::RigHarnessPlugins);
    // 视图层：替代 plugins/rig-tui 的 TuiViewPlugin
    app.add_plugins(rig_feathers::FeathersViewPlugin);
    // 3. 视图层的字体资源
    app.add_systems(PreStartup, insert_font);
    app.run()
}

fn insert_font(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(TranscriptFont(FontSource::List(vec![
        asset_server.load(bevy_feathers::constants::fonts::MONO).into(),
        asset_server.load::<Font>("fonts/NotoSansCJKsc-Subset.otf").into(),
    ])));
}
