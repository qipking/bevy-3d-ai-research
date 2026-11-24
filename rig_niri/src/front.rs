//! 前端身份：rig-tui 用 `Front::new("tui")` 声明自己；这里是 `Front::new("feathers")`。
//!
//! 作用：同一个 world 里可以并存多个前端（TUI / feathers / print），由 CLI 选谁跑；
//! 另外两件前端必须做的事：spawn `Answerer`（回答 Ask）、`ShowsPickers`（展示列表选择器）。

use bevy::prelude::*;
use rig_harness::prelude::{Answerer, ShowsPickers};

/// 本前端在 rig 里的名字。
pub const FRONT: &str = "feathers";

pub struct FrontPlugin;

impl Plugin for FrontPlugin {
    fn build(&self, app: &mut App) {
        // 声明前端身份（与 rig-tui 的 `app.add_front(Front::new(FRONT)...)` 同一套机制）
        app.register_feathers_front();
        app.add_systems(Startup, spawn_front_markers);
    }
}

/// 在启动时声明"这个前端能回答 Ask、能展示 picker"。
fn spawn_front_markers(mut commands: Commands) {
    // agent: None = 同时服务所有 agent
    commands.spawn((Answerer { agent: None }, ShowsPickers));
}

/// 把注册动作包一层，便于以后加 flag（`--feathers`）。
pub trait AddFeathersFront {
    fn register_feathers_front(&mut self) -> &mut Self;
}

impl AddFeathersFront for App {
    fn register_feathers_front(&mut self) -> &mut Self {
        use rig_harness::prelude::{AppCliExt, Front};
        self.add_front(Front::new(FRONT));
        self
    }
}
