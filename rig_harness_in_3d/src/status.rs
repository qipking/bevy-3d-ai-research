//! 状态行：每帧从 ECS 的真实资源写 `AppStatus`，不再有演示数字。
//!
//! 条目和来源：
//!
//! | 名字 | 来源 |
//! |---|---|
//! | `session` | [`SessionPaths`] 里的会话目录 id（`RIG_HOME/sessions/<id>`） |
//! | `agents` | `With<Agent>` 的实体数 |
//! | `turn` | agent 上有没有 `ActiveTurn`（`TurnOf` 的关系目标） |
//! | `conversation` | `Conversation` 的消息条数 |
//! | `tasks` | `Running` 的后台任务数 |
//! | `tokens` | `LastUsage` 里模型真报过的 token；没有就不画 |
//!
//! `AppStatus` 里别的插件放的条目不动（只按名字替换本 app 自己的）。
//! `[view] hide_status = true` 时，在状态行系统之后把 `PanelId::Status` 清空。

use bevy::prelude::*;
use rig_ecs::prelude::{ActiveTurn, Agent, Conversation, LastUsage, Running};
use rig_niri::panels::{PanelId, PanelRows};
use rig_harness::prelude::SessionPaths;
use rig_tui::{AppStatus, Side, StatusItem, Tone};

use crate::settings::ViewSettings;

/// 从真实资源刷新状态条目；`Prepare`（状态行绘制之前）。
pub fn sync_status_items(
    paths: Option<Res<SessionPaths>>,
    agents: Query<(
        &Agent,
        Option<&ActiveTurn>,
        Option<&LastUsage>,
        Option<&Conversation>,
    )>,
    running: Query<(), With<Running>>,
    mut status: ResMut<AppStatus>,
    mut logged: Local<bool>,
) {
    let count = agents.iter().count();
    let turns = agents.iter().filter(|(_, turn, ..)| turn.is_some()).count();
    let messages: usize = agents
        .iter()
        .filter_map(|(_, _, _, conversation)| conversation)
        .map(|conversation| conversation.messages().len())
        .sum();
    let tasks = running.iter().count();
    let tokens = agents
        .iter()
        .filter_map(|(_, _, last, _)| last.and_then(LastUsage::usage))
        .filter_map(|usage| usage.total_tokens.or_else(|| usage.context_tokens()))
        .max();

    let session = paths.as_deref().and_then(|paths| {
        paths
            .0
            .id()
            .map(|id| id.to_string())
            .or_else(|| paths.0.path().file_name().map(|name| name.to_string_lossy().into_owned()))
    });

    if !*logged {
        *logged = true;
        info!(
            "[status] session={} agents={count} turns={turns} msgs={messages} tasks={tasks} tokens={tokens:?}",
            session.as_deref().unwrap_or("-")
        );
    }

    match session {
        Some(session) => show(
            &mut status,
            StatusItem::new("session", Side::Left, 10, 2)
                .says(format!("session {session}"), Tone::Dim),
        ),
        None => hide(&mut status, "session"),
    }

    let agents_text = match count {
        1 => "1 agent".to_owned(),
        count => format!("{count} agents"),
    };
    show(
        &mut status,
        StatusItem::new("agents", Side::Left, 20, 3).says(agents_text, Tone::Bold),
    );

    let (turn_text, turn_tone) = match turns {
        0 => ("idle".to_owned(), Tone::Green),
        1 => ("turn running".to_owned(), Tone::Yellow),
        turns => (format!("{turns} turns running"), Tone::Yellow),
    };
    show(
        &mut status,
        StatusItem::new("turn", Side::Left, 30, 5).says(turn_text, turn_tone),
    );

    let messages_text = match messages {
        1 => "1 msg".to_owned(),
        messages => format!("{messages} msgs"),
    };
    show(
        &mut status,
        StatusItem::new("conversation", Side::Left, 40, 4).says(messages_text, Tone::Dim),
    );

    let tasks_text = match tasks {
        1 => "1 task".to_owned(),
        tasks => format!("{tasks} tasks"),
    };
    show(
        &mut status,
        StatusItem::new("tasks", Side::Right, 10, 4).says(tasks_text, Tone::Dim),
    );

    match tokens {
        Some(tokens) => show(
            &mut status,
            StatusItem::new("tokens", Side::Right, 20, 4).says(format!("{tokens} tokens"), Tone::Cyan),
        ),
        // 没接过模型、模型没报过 token 时不画——不留假数字。
        None => hide(&mut status, "tokens"),
    }
}

/// 状态行之后：`hide_status` 时把这一帧画出来的状态面板清空。
pub fn apply_view_settings(settings: Res<ViewSettings>, mut panels: ResMut<PanelRows>) {
    if settings.hide_status {
        panels.set(PanelId::Status, Vec::new());
    }
}

/// 只在文字或色调变了时 `show`，避免每帧都把 `AppStatus` 标记成 changed。
fn show(status: &mut AppStatus, item: StatusItem) {
    let unchanged = status
        .0
        .iter()
        .find(|old| old.name == item.name)
        .is_some_and(|old| old.text == item.text && old.tone == item.tone);
    if !unchanged {
        status.0.show(item);
    }
}

fn hide(status: &mut AppStatus, name: &str) {
    if status.0.iter().any(|item| item.name == name) {
        status.0.hide(name);
    }
}
