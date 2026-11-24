//! 窗口标题：rig-tui 的 `title.rs` 在终端里做同一件事。
//!
//! rig-tui 往终端写 OSC 133 的提示符标记和标题（`rig · working` 等）；窗口化的
//! 3D app 没有终端，对应物是**窗口标题**。状态怎么算的照抄 `title.rs`：
//!
//! | 状态 | 条件 |
//! |---|---|
//! | `needs you` | 有 picker 打开，或有没回答的 `Ask` |
//! | `working` | 有 turn 在跑（不是子 agent 的） |
//! | `error` | 上一个 turn 失败 |
//! | `idle` | 其它 |
//!
//! 只在这些输入变化时写窗口标题（写标题会触发窗口事件，不必每帧写）。

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use rig_ecs::prelude::{Outcome, Settled, SpawnedBy, TurnOf, TurnOutcome};
use rig_harness::pick::Picker;

/// 标题里的时间/状态分隔符。
pub const TITLE_SEPARATOR: &str = " · ";

/// 窗口标题的基础部分（启动时的标题）+ 当前状态。
#[derive(Resource, Default)]
pub struct WindowTitle {
    /// 启动时窗口的标题，标题 = 它 + ` · ` + 状态。
    pub base: String,
    /// 这次写进窗口的状态（没写就是 `None`）。
    pub state: Option<&'static str>,
    /// 上一个结束的 turn 是否失败。
    pub failed: bool,
}

impl WindowTitle {
    /// 完整标题。
    pub fn compose(&self, state: &str) -> String {
        match self.base.is_empty() {
            true => format!("rig{TITLE_SEPARATOR}{state}"),
            false => format!("{}{TITLE_SEPARATOR}{state}", self.base),
        }
    }
}

/// 启动时记下窗口原来的标题。
fn capture_base(
    mut title: ResMut<WindowTitle>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    if title.base.is_empty() {
        title.base = window.title.clone();
    }
}

/// 一个 turn 结束：记下失败，`error` 状态要用。
fn note_outcome(
    settled: On<Add<Settled>>,
    turns: Query<(&rig_ecs::prelude::ForAgent, &TurnOutcome)>,
    subagents: Query<(), With<SpawnedBy>>,
    mut title: ResMut<WindowTitle>,
) {
    let Ok((&rig_ecs::prelude::ForAgent(agent), outcome)) = turns.get(settled.entity) else {
        return;
    };
    if subagents.contains(agent) {
        return;
    }
    title.failed = matches!(outcome, TurnOutcome::Failed(_));
}

/// 每帧算状态；变了才写窗口标题。
pub fn tell_state(
    turns: Query<&TurnOf, Without<Settled>>,
    subagents: Query<(), With<SpawnedBy>>,
    pickers: Query<(), With<Picker>>,
    asks: Query<(), (With<rig_harness::ask::Ask>, Without<Outcome>)>,
    mut title: ResMut<WindowTitle>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    let running = turns.iter().any(|turn| !subagents.contains(turn.0));
    // 有 turn 开跑就把上次的失败清掉（rig-tui 在 `turn_started` 里做同一件事）。
    if running && title.failed {
        title.failed = false;
    }
    let state = if !pickers.is_empty() || !asks.is_empty() {
        "needs you"
    } else if running {
        "working"
    } else if title.failed {
        "error"
    } else {
        "idle"
    };
    if title.state == Some(state) {
        return;
    }
    title.state = Some(state);
    let composed = title.compose(state);
    if window.title != composed {
        window.title = composed;
    }
    info!("[title] {state}");
}

pub struct TitlePlugin;

impl Plugin for TitlePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WindowTitle>()
            .add_systems(Startup, capture_base)
            .add_observer(note_outcome)
            .add_systems(Update, tell_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_is_the_base_and_the_state() {
        let mut title = WindowTitle::default();
        title.base = "my agent".to_string();
        assert_eq!(title.compose("working"), "my agent · working");
        assert_eq!(title.compose("needs you"), "my agent · needs you");
    }

    #[test]
    fn without_a_base_it_still_says_rig() {
        let title = WindowTitle::default();
        assert_eq!(title.compose("idle"), "rig · idle");
    }
}
