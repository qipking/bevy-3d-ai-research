//! `/show`：把**聚焦工作区**的活动转写窗格切到另一份来源。
//!
//! S1/S2 已经把来源放到 tile 上（`TranscriptPane::source`），但还没有入口。
//! 这里补的是应用层的那一个入口：
//!
//! * `/show` 无参数 → 弹一块 [`Picker`] 列出来源（`shown`、每个活 agent、最近
//!   的已保存会话）；
//! * `/show shown` → 恢复本工作区的默认来源（标题清空）；
//! * `/show agent <id>` / `/show <id>` → 看某个活 agent（子 agent、别的工作区
//!   的 agent 都行）；
//! * `/show session <id|name>` → 读一份已保存会话，作为只读的
//!   [`TranscriptSource::Value`]。
//!
//! 命令**不移动** [`ShownAgent`]：来源是窗格自己的属性。主窗格切到
//! `Agent(e)`/`Value(c)` 时，全局 `ShownAgent` 管线仍然只服务 `Shown` 的那个
//! 窗格（`pane::window_transcript` 按 source 取行），两列互不干扰。
//!
//! Esc 不参与退出（它是停 turn / 交还输入框的键）；退出口是 `/show shown` 或
//! picker 里的 `shown` 项。

use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use rig_harness::commands::{AppCommandsExt, CommandArgs};
use rig_harness::prelude::{
    Agent, AgentId, Home, Notice, PickItem, Picker, RigHome, SessionPaths, SpawnedBy, StoreOpener,
};
use rig_harness::rig_memory::{SessionMeta, read_conversation};
use rig_niri::pane::{ActiveTranscript, TranscriptPane};
use rig_niri::panels::{PanelId, PanelRoots};
use rig_tui::TranscriptSource;

/// picker 里最多列几个已保存会话（最近的在前）。
const SAVED_SESSIONS: usize = 8;

/// `/show` 命令、picker 选项的数据来源。
///
/// 注册在 [`HarnessExtPlugin`](crate::HarnessExtPlugin) 里（和 `/hello`、
/// `/setting` 同一个命令表），所以 `HarnessExt::commands()` 能读到 `/show`。
pub struct ShowPlugin;

impl Plugin for ShowPlugin {
    fn build(&self, app: &mut App) {
        app.add_command(
            "show",
            "Show a transcript source: /show [shown | agent <id> | session <id>]",
            show,
        );
    }
}

/// 一个可切换的活 agent。
#[derive(Clone, Debug)]
struct AgentChoice {
    /// 实体：`TranscriptSource::Agent` 要的就是它。
    entity: Entity,
    /// `AgentId` 的完整字符串（picker 的 command 用它，跨实体稳定）。
    id: String,
    /// 可读名字：优先 `Name`，没有/是默认的 `agent` 时退回 id。
    label: String,
    /// 是否是子 agent（`SpawnedBy`）。
    child: bool,
    /// 创建顺序（FIFO；picker 按它排）。
    order: u32,
}

/// 一个已保存会话。
#[derive(Clone, Debug)]
struct SessionChoice {
    id: String,
    label: String,
    detail: String,
}

/// `/show`：解析参数并改**活动转写窗格**的 `source`/`title`。
#[allow(clippy::too_many_arguments)]
pub fn show(
    In(args): In<CommandArgs>,
    mut commands: Commands,
    mut notices: MessageWriter<Notice>,
    roots: Option<Res<PanelRoots>>,
    active: Option<Res<ActiveTranscript>>,
    mut panes: Query<&mut TranscriptPane>,
    agents: Query<(Entity, &AgentId, Option<&Name>, Option<&SpawnedBy>), With<Agent>>,
    home: Option<Res<RigHome>>,
    opener: Option<Res<StoreOpener>>,
    paths: Option<Res<SessionPaths>>,
) {
    let home = home_of(home.as_deref(), paths.as_deref());
    let opener = opener.as_deref().cloned().unwrap_or_default();
    let trimmed = args.args.trim();

    // 无参数：弹 picker。条目命令由 rig-harness 的 pick 观察者执行
    // （`run_chosen`），不需要应用层自己接。
    if trimmed.is_empty() {
        let picker = source_picker(args.agent, &agents, &home, &opener);
        commands.spawn(picker);
        return;
    }

    let Some(root) = resolve_root(active.as_deref(), roots.as_deref()) else {
        notices.write(Notice::error(
            args.agent,
            "没有活动的转写窗格（先 Alt+S 拆一个，或让工作区先绑上 agent）。",
        ));
        return;
    };

    let choices = agent_choices(&agents);
    let (subcommand, rest) = trimmed
        .split_once(char::is_whitespace)
        .map(|(name, rest)| (name, rest.trim()))
        .unwrap_or((trimmed, ""));

    let outcome = match subcommand.to_ascii_lowercase().as_str() {
        "shown" | "workspace" | "default" | "current" => {
            apply_source(root, TranscriptSource::Shown, None, &mut panes)
        }
        "agent" => apply_agent(root, rest, &choices, &mut panes),
        "session" => open_session(root, rest, &home, &opener, &mut panes),
        _ => {
            // `/show <id>`：先按 agent 找（完整 id / 短 id / 唯一名字），找不到
            // 再按已保存会话找（会话 id 或用户给会话起的名字）。
            if let Some(choice) = find_agent(&choices, trimmed) {
                apply_agent_choice(root, &choice, &mut panes)
            } else {
                open_session(root, trimmed, &home, &opener, &mut panes)
            }
        }
    };

    if let Err(why) = outcome {
        notices.write(Notice::error(args.agent, why));
    }
}

/// 本 app 的 `RIG_HOME`：优先资源（测试可指向临时目录），否则从当前会话目录
/// 反推，再不行才问环境。
fn home_of(home: Option<&RigHome>, paths: Option<&SessionPaths>) -> Home {
    if let Some(home) = home {
        return home.0.clone();
    }
    if let Some(root) = paths
        .and_then(|paths| paths.0.path().parent())
        .and_then(|sessions| sessions.parent())
    {
        return Home::at(root);
    }
    Home::from_env()
}

/// 活动转写根：`ActiveTranscript` 有效就用它，否则退回主转写。
fn resolve_root(active: Option<&ActiveTranscript>, roots: Option<&PanelRoots>) -> Option<Entity> {
    let roots = roots?;
    active
        .and_then(|active| active.root(roots))
        .or_else(|| roots.entity(PanelId::Transcript))
}

/// 每个活 agent 一个选项，按实体创建顺序（FIFO）。
fn agent_choices(
    agents: &Query<(Entity, &AgentId, Option<&Name>, Option<&SpawnedBy>), With<Agent>>,
) -> Vec<AgentChoice> {
    let mut choices: Vec<AgentChoice> = agents
        .iter()
        .map(|(entity, id, name, spawned_by)| AgentChoice {
            entity,
            id: id.0.clone(),
            label: display_agent_name(name, &id.0),
            child: spawned_by.is_some(),
            order: entity.index_u32(),
        })
        .collect();
    choices.sort_by_key(|choice| choice.order);
    choices
}

/// 显示名：有 `Name` 且不是默认的 `agent` 就用它，否则用 `AgentId`。
fn display_agent_name(name: Option<&Name>, id: &str) -> String {
    match name.map(Name::as_str) {
        Some(name) if !name.trim().is_empty() && name != "agent" => name.to_owned(),
        _ => id.to_owned(),
    }
}

/// 按完整 id、唯一短 id、唯一显示名找 agent。
fn find_agent(choices: &[AgentChoice], wanted: &str) -> Option<AgentChoice> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return None;
    }
    if let Some(choice) = choices.iter().find(|choice| choice.id == wanted) {
        return Some(choice.clone());
    }
    if let Some(choice) = unique(choices.iter().filter(|choice| choice.id.starts_with(wanted))) {
        return Some(choice.clone());
    }
    unique(choices.iter().filter(|choice| choice.label == wanted)).cloned()
}

/// 迭代器里恰好一个元素时返回它。
fn unique<'a>(mut items: impl Iterator<Item = &'a AgentChoice>) -> Option<&'a AgentChoice> {
    let first = items.next()?;
    items.next().is_none().then_some(first)
}

/// 把一个 agent 选项落到窗格上。
fn apply_agent(
    root: Entity,
    wanted: &str,
    choices: &[AgentChoice],
    panes: &mut Query<&mut TranscriptPane>,
) -> Result<(), String> {
    let Some(choice) = find_agent(choices, wanted) else {
        return Err(format!("没有找到 agent `{wanted}`。"));
    };
    apply_agent_choice(root, &choice, panes)
}

fn apply_agent_choice(
    root: Entity,
    choice: &AgentChoice,
    panes: &mut Query<&mut TranscriptPane>,
) -> Result<(), String> {
    apply_source(
        root,
        TranscriptSource::Agent(choice.entity),
        Some(choice.label.clone()),
        panes,
    )
}

/// 改一个窗格的来源 + 标题。`Shown` 的标题一律清空（退出口）。
fn apply_source(
    root: Entity,
    source: TranscriptSource,
    title: Option<String>,
    panes: &mut Query<&mut TranscriptPane>,
) -> Result<(), String> {
    let mut pane = panes
        .get_mut(root)
        .map_err(|_| format!("{root:?} 不是转写窗格（没有 TranscriptPane）。"))?;
    pane.source = source;
    pane.title = title.filter(|title| !title.trim().is_empty());
    // 换了来源，旧来源的条目身份不再有意义；窗口化下一帧会把 scroll 夹到新
    // 来源的范围里。
    pane.selected = None;
    Ok(())
}

/// 打开一份已保存会话，作为只读 `Value` 来源。
fn open_session(
    root: Entity,
    wanted: &str,
    home: &Home,
    opener: &StoreOpener,
    panes: &mut Query<&mut TranscriptPane>,
) -> Result<(), String> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return Err("用法：/show session <id|名字>。".to_owned());
    }
    let id = home
        .find(wanted)
        .map_err(|why| format!("打不开已保存会话：{why}"))?;
    let dir = home.session(&id);
    let store = opener
        .open(&dir)
        .map_err(|failure| format!("打不开会话 {} 的 store：{failure}", id.as_str()))?;
    let meta = SessionMeta::read(&*store);
    let label = session_label(&dir, meta.as_ref(), id.as_str());
    let (_agent, conversation) = read_conversation(&*store, None)
        .map_err(|failure| format!("读不了会话 {}：{failure}", id.as_str()))?;
    apply_source(root, TranscriptSource::Value(conversation), Some(label), panes)
}

/// 会话的可读名字：用户起的名字 → `SessionMeta.title` → id。
fn session_label(
    dir: &rig_harness::harness_protocol::SessionDir,
    meta: Option<&SessionMeta>,
    id: &str,
) -> String {
    dir.name()
        .ok()
        .flatten()
        .or_else(|| {
            meta.and_then(|meta| (!meta.title.is_empty()).then(|| meta.title.clone()))
        })
        .unwrap_or_else(|| id.to_owned())
}

/// `/show` 的 picker：`shown` + 每个活 agent + 最近 N 个已保存会话。
fn source_picker(
    agent: Entity,
    agents: &Query<(Entity, &AgentId, Option<&Name>, Option<&SpawnedBy>), With<Agent>>,
    home: &Home,
    opener: &StoreOpener,
) -> Picker {
    let mut picker = Picker::new("看谁的对话")
        .body("Enter 切换活动转写窗格的来源；shown 恢复本工作区的默认来源。")
        .for_agent(agent);
    picker.items.push(
        PickItem::new("shown", "show shown").detail("本工作区的默认来源（恢复默认）"),
    );
    for choice in agent_choices(agents) {
        let detail = if choice.child {
            format!("agent · {} · 子 agent", choice.id)
        } else {
            format!("agent · {}", choice.id)
        };
        picker.items.push(
            PickItem::new(choice.label, format!("show agent {}", choice.id)).detail(detail),
        );
    }
    for session in saved_sessions(home, opener, SAVED_SESSIONS) {
        picker.items.push(
            PickItem::new(session.label, format!("show session {}", session.id))
                .detail(session.detail),
        );
    }
    picker
}

/// 最近 N 个已保存会话（按 `SessionMeta.updated` 从新到旧）；打不开 store 的
/// 会话仍按名字列出（时间未知），不因为一个坏目录整份列表消失。
fn saved_sessions(home: &Home, opener: &StoreOpener, limit: usize) -> Vec<SessionChoice> {
    let mut sessions: Vec<(u64, SessionChoice)> = home
        .saved_sessions()
        .into_iter()
        .map(|(id, dir)| {
            let meta = opener
                .open(&dir)
                .ok()
                .and_then(|store| SessionMeta::read(&*store));
            let label = session_label(&dir, meta.as_ref(), id.as_str());
            let updated = meta.as_ref().map_or(0, |meta| meta.updated);
            let messages = meta.as_ref().map_or(0, |meta| meta.messages);
            let detail = format!("saved session · {messages} msgs · {}", age_text(updated));
            (
                updated,
                SessionChoice {
                    id: id.as_str().to_owned(),
                    label,
                    detail,
                },
            )
        })
        .collect();
    sessions.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.id.cmp(&right.1.id)));
    sessions.truncate(limit);
    sessions.into_iter().map(|(_, choice)| choice).collect()
}

/// picker 里显示的"多久以前"；没有时间戳（旧会话/读不到 meta）时说 unknown。
fn age_text(updated: u64) -> String {
    if updated == 0 {
        return "unknown time".to_owned();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let seconds = now.saturating_sub(updated) / 1000;
    match seconds {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    //! 无窗口测试：真命令系统 + 真 picker + 真 `window_transcript`，断言落在
    //! `TranscriptPane`/`PanelRows` 上。

    use super::*;
    use crate::HarnessExt;
    use bevy::app::TaskPoolPlugin;
    use rig_core::conversation::Conversation as TalkConversation;
    use rig_core::message::{AssistantContent, AssistantMessage, Message};
    use rig_ecs::prelude::{Conversation, spawn_root};
    use rig_harness::commands::RunCommand;
    use rig_harness::harness_protocol::SessionDir;
    use rig_harness::pick::PickPlugin;
    use rig_harness::prelude::{Chosen, Pick, ShownAgent};
    use rig_harness::rig_memory::save;
    use rig_niri::FeathersSystems;
    use rig_niri::frame::{CellRect, PanelArea, PanelAreas};
    use rig_niri::pane::{PanePlugin, TranscriptRows};
    use rig_niri::panels::{KeyedRow, PanelRows};
    use rig_niri::transcript::Row;
    use rig_tui::ratatui::text::Line;

    /// `/show` 测试 App：内核 + 窗格 + 命令 + picker（不建窗口、不跑 GUI）。
    fn show_app() -> App {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.add_plugins(rig_ecs::AgentPlugin);
        app.add_plugins(PanePlugin);
        app.add_plugins(PickPlugin);
        app.configure_sets(
            Update,
            (
                FeathersSystems::Prepare,
                FeathersSystems::Layout,
                FeathersSystems::Draw,
                FeathersSystems::Fit,
                FeathersSystems::Render,
            )
                .chain(),
        );
        app.insert_resource(rig_harness::prelude::Settings::from_toml(""));
        app.insert_resource(crate::SettingsPath(std::env::temp_dir().join(
            format!("rig-show-settings-{}.toml", std::process::id()),
        )));
        // 不让 picker 去读真实家目录：指向一个不存在的临时 home。
        app.insert_resource(RigHome(Home::at(
            std::env::temp_dir().join(format!("rig-show-empty-home-{}", std::process::id())),
        )));
        // HarnessExtPlugin 注册 `/show`；`ShowPlugin` 也直接加，测试命令表不依赖
        // 模板插件（两者 idempotent 的 add_command 会对重复名 warn，所以只加模板）。
        app.add_plugins(crate::HarnessExtPlugin);
        // 窗格系统要的资源。
        app.init_resource::<PanelRows>()
            .init_resource::<TranscriptRows>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(ShownAgent(None));
        app
    }

    /// 80 列 × `rows` 行的测试区域（不走 `PanelArea::new` 的像素内缩，测试
    /// 要的是确定的可见行数）。
    fn area(rows: u16) -> PanelArea {
        PanelArea {
            rect: CellRect::new(0, 0, 80, rows),
            size: Vec2::new(80.0, f32::from(rows)),
            cols: 80,
            rows,
            center: None,
        }
    }

    /// 一块主转写 + 80×10 的区域；返回根实体。
    fn spawn_transcript(app: &mut App) -> Entity {
        let root = app.world_mut().spawn(Name::new("panel:Transcript")).id();
        app.world_mut()
            .entity_mut(root)
            .insert((PanelId::Transcript, TranscriptPane::default()));
        let mut roots = PanelRoots::default();
        roots.0.insert(PanelId::Transcript, root);
        app.insert_resource(roots);
        let mut areas = PanelAreas::default();
        areas.0.insert(
            root,
            area(10),
        );
        app.insert_resource(areas);
        root
    }

    fn pane(app: &App, root: Entity) -> &TranscriptPane {
        app.world().get::<TranscriptPane>(root).expect("转写根")
    }

    fn panel_text(rows: &[KeyedRow]) -> String {
        rows.iter()
            .flat_map(|keyed| keyed.row.line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn row_keys(rows: &[KeyedRow]) -> Vec<u64> {
        rows.iter().map(|keyed| keyed.key).collect()
    }

    #[test]
    fn the_show_command_is_registered_and_bare_show_opens_a_picker_with_shown_and_agents() {
        let mut app = show_app();
        let main = spawn_root(app.world_mut(), (AgentId("main".into()), Name::new("main"))).id();
        spawn_root(app.world_mut(), (AgentId("helper".into()), Name::new("helper")));
        app.world_mut().resource_mut::<ShownAgent>().0 = Some(main);
        let _root = spawn_transcript(&mut app);
        app.update();

        assert!(app.commands().iter().any(|name| name == "/show"));

        app.world_mut().trigger(RunCommand::new(main, "show"));
        app.update();

        let mut pickers = app.world_mut().query::<&Picker>();
        let pickers: Vec<&Picker> = pickers.iter(app.world()).collect();
        assert_eq!(pickers.len(), 1, "无参数 /show 应弹一个 picker");
        let labels: Vec<&str> = pickers[0].items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels.first().copied(), Some("shown"), "{labels:?}");
        assert!(labels.contains(&"main"), "picker 要列出主 agent：{labels:?}");
        assert!(labels.contains(&"helper"), "picker 要列出子 agent：{labels:?}");
        assert!(
            pickers[0]
                .items
                .iter()
                .any(|item| item.command == "show agent helper"),
            "helper 的条目命令应回到 /show：{:?}",
            pickers[0]
                .items
                .iter()
                .map(|item| item.command.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn picking_an_agent_sets_the_active_pane_source_and_title_and_shown_restores_it() {
        let mut app = show_app();
        let main = spawn_root(app.world_mut(), (AgentId("main".into()), Name::new("main"))).id();
        let helper =
            spawn_root(app.world_mut(), (AgentId("helper".into()), Name::new("helper"))).id();
        app.world_mut().resource_mut::<ShownAgent>().0 = Some(main);
        let root = spawn_transcript(&mut app);
        app.update();

        // 无参数 /show -> picker；选中 helper 那一条。
        app.world_mut().trigger(RunCommand::new(main, "show"));
        app.update();
        let picker = {
            let mut query = app.world_mut().query::<(Entity, &Picker)>();
            let (entity, picker) = query.iter(app.world()).next().expect("picker").clone();
            (entity, picker.clone())
        };
        let index = picker
            .1
            .items
            .iter()
            .position(|item| item.command == "show agent helper")
            .expect("helper 选项");
        app.world_mut()
            .trigger(Pick::person(picker.0, Some(Chosen::item(index))));
        // Pick -> run_chosen -> RunCommand，命令队列在下一个 Update 边界执行。
        app.update();
        app.update();

        assert!(
            matches!(pane(&app, root).source, TranscriptSource::Agent(agent) if agent == helper),
            "活动窗格应切到 helper，实际 {:?}",
            pane(&app, root).source
        );
        assert_eq!(pane(&app, root).title.as_deref(), Some("helper"));
        assert_eq!(
            app.world().resource::<ShownAgent>().0,
            Some(main),
            "/show 只改窗格来源，不移动 ShownAgent"
        );

        // shown 是退出口：来源回到 Shown、标题清空。
        app.world_mut().trigger(RunCommand::new(main, "show shown"));
        app.update();
        assert!(matches!(pane(&app, root).source, TranscriptSource::Shown));
        assert_eq!(pane(&app, root).title, None);
    }

    /// 两列各画各的：主转写留 `Shown`，Pane2 设 `Agent(helper)`；主转写的全局
    /// `TranscriptRows` 不被 Pane2 带跑，Pane2 也不镜像主窗格。
    #[test]
    fn two_panes_with_different_sources_are_widowed_independently() {
        let mut app = show_app();
        let main = spawn_root(app.world_mut(), (AgentId("main".into()), Name::new("main"))).id();
        let helper =
            spawn_root(app.world_mut(), (AgentId("helper".into()), Name::new("helper"))).id();
        app.world_mut().resource_mut::<ShownAgent>().0 = Some(main);
        let transcript = spawn_transcript(&mut app);

        // Pane2：和主转写一样高，挂在自己的根上。
        let pane2 = app.world_mut().spawn(Name::new("panel:Pane2")).id();
        app.world_mut()
            .entity_mut(pane2)
            .insert((PanelId::Pane2, TranscriptPane::default()));
        let mut roots = app.world().resource::<PanelRoots>().clone();
        roots.0.insert(PanelId::Pane2, pane2);
        app.insert_resource(roots);
        app.world_mut()
            .resource_mut::<PanelAreas>()
            .0
            .insert(pane2, area(10));

        // 主窗口的全局整段（Shown 管线）是 alpha；helper 的对话是 beta。
        app.world_mut().resource_mut::<TranscriptRows>().0 = vec![KeyedRow {
            key: 0,
            hash: 1,
            row: Row::new(Line::from("alpha from the shown agent")),
        }];
        let mut talk = TalkConversation::new();
        talk.push(Message::user("beta from the helper agent"));
        app.world_mut().entity_mut(helper).insert(Conversation(talk));

        // 活动窗格 = Pane2；/show agent helper 只改 Pane2。
        app.world_mut().resource_mut::<ActiveTranscript>().0 = Some(pane2);
        app.world_mut()
            .trigger(RunCommand::new(main, "show agent helper"));
        app.update();

        assert!(matches!(
            pane(&app, transcript).source,
            TranscriptSource::Shown
        ));
        assert_eq!(pane(&app, transcript).title, None);
        assert!(matches!(
            pane(&app, pane2).source,
            TranscriptSource::Agent(agent) if agent == helper
        ));
        assert_eq!(pane(&app, pane2).title.as_deref(), Some("helper"));

        let main_text = panel_text(app.world().resource::<PanelRows>().get(PanelId::Transcript));
        let pane2_text = panel_text(app.world().resource::<PanelRows>().get(PanelId::Pane2));
        assert!(main_text.contains("alpha"), "{main_text:?}");
        assert!(!main_text.contains("beta"), "主列不能被 Pane2 带跑：{main_text:?}");
        assert!(pane2_text.contains("beta"), "{pane2_text:?}");
        assert!(!pane2_text.contains("alpha"), "Pane2 不镜像主列：{pane2_text:?}");

        // 主转写也切到 helper、Pane2 回 Shown：两列反过来各画各的。
        app.world_mut().resource_mut::<ActiveTranscript>().0 = Some(transcript);
        app.world_mut()
            .trigger(RunCommand::new(main, "show agent helper"));
        app.world_mut()
            .get_mut::<TranscriptPane>(pane2)
            .unwrap()
            .source = TranscriptSource::Shown;
        // 主转写的 rows 仍是全局 Shown 的 alpha；它现在画 helper 的 beta。
        app.update();

        let main_text = panel_text(app.world().resource::<PanelRows>().get(PanelId::Transcript));
        let pane2_text = panel_text(app.world().resource::<PanelRows>().get(PanelId::Pane2));
        assert!(main_text.contains("beta"), "{main_text:?}");
        assert!(pane2_text.contains("alpha"), "{pane2_text:?}");
    }

    #[test]
    fn a_title_row_is_on_top_and_does_not_break_scrolling_or_items() {
        let mut app = show_app();
        let main = spawn_root(app.world_mut(), (AgentId("main".into()), Name::new("main"))).id();
        app.world_mut().resource_mut::<ShownAgent>().0 = Some(main);
        let root = spawn_transcript(&mut app);
        // 30 行正文，窗格 10 行。
        app.world_mut().resource_mut::<TranscriptRows>().0 = (0..30)
            .map(|index| KeyedRow {
                // 和引擎同一个约定：高 32 位 = 条目序号，低 32 位 = 块内行号。
                key: (index as u64) << 32,
                hash: index + 1,
                row: Row::new(Line::from(format!("row {index}"))),
            })
            .collect();
        app.world_mut().get_mut::<TranscriptPane>(root).unwrap().title = Some("main".into());
        app.update();

        let rows = app.world().resource::<PanelRows>().get(PanelId::Transcript).to_vec();
        assert_eq!(rows.len(), 10, "标题占一行，正文少一行，总数仍是窗高");
        assert_eq!(rows[0].key, u64::MAX - 2, "标题行 key");
        assert!(
            row_text(&rows[0]).contains("main"),
            "{:?}",
            row_text(&rows[0])
        );
        // 标题钉在顶部，正文仍是最后 9 行。
        let body: Vec<u64> = rows[1..].iter().map(|keyed| keyed.key).collect();
        assert_eq!(
            body,
            (21..30).map(|index| (index as u64) << 32).collect::<Vec<_>>()
        );
        // title 不属于任何条目。
        let items = rig_niri::pane::item_rows(&rows);
        assert_eq!(items.len(), 9, "30 行正文 9 条 == 9 行");
        assert_eq!(items[0].start, 1, "标题跳过，第一条正文从可见窗口第 1 行开始");
        assert!(items.iter().all(|item| item.ordinal != u32::MAX));

        // 清空标题：不占行，回到普通窗口。
        app.world_mut().get_mut::<TranscriptPane>(root).unwrap().title = None;
        app.update();
        let rows = app.world().resource::<PanelRows>().get(PanelId::Transcript).to_vec();
        assert_eq!(
            row_keys(&rows),
            (20..30).map(|index| (index as u64) << 32).collect::<Vec<_>>()
        );
    }

    fn row_text(keyed: &KeyedRow) -> String {
        keyed
            .row
            .line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn a_saved_session_is_read_from_a_temp_home_and_shown_as_a_value_source() {
        let root = temp_dir("session-home");
        let home = Home::at(&root);
        let (id, dir) = home.new_session().expect("临时会话目录");
        let store = StoreOpener::default().open(&dir).expect("打开临时 store");
        let mut conversation = TalkConversation::new();
        conversation.push(Message::user("saved question"));
        conversation.push(Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::text("saved answer"),
        ])));
        save(&*store, "main", &mut conversation).expect("写临时会话");
        dir.mark_saved().expect("标记 saved");

        let mut app = show_app();
        app.insert_resource(RigHome(home.clone()));
        app.insert_resource(SessionPaths(SessionDir::at(dir.path())));
        let main = spawn_root(app.world_mut(), (AgentId("main".into()), Name::new("main"))).id();
        app.world_mut().resource_mut::<ShownAgent>().0 = Some(main);
        let pane_root = spawn_transcript(&mut app);
        app.update();

        // picker 里也要列得出这个会话（S4 的列表来源）。
        app.world_mut().trigger(RunCommand::new(main, "show"));
        app.update();
        let (picker_entity, picker) = {
            let mut query = app.world_mut().query::<(Entity, &Picker)>();
            let (entity, picker) = query.iter(app.world()).next().expect("picker");
            (entity, picker.clone())
        };
        assert!(
            picker
                .items
                .iter()
                .any(|item| item.command == format!("show session {}", id.as_str())),
            "picker 应列出临时 home 里的已保存会话：{:?}",
            picker
                .items
                .iter()
                .map(|item| item.command.as_str())
                .collect::<Vec<_>>()
        );
        app.world_mut().entity_mut(picker_entity).despawn();

        app.world_mut().trigger(RunCommand::new(
            main,
            format!("show session {}", id.as_str()),
        ));
        app.update();

        match &pane(&app, pane_root).source {
            TranscriptSource::Value(saved) => {
                assert!(!saved.messages().is_empty(), "读到的 Conversation 不能是空的");
            }
            other => panic!("应切到 Value 来源，实际 {other:?}"),
        }
        assert_eq!(
            app.world().resource::<ShownAgent>().0,
            Some(main),
            "读存档不能抢 ShownAgent"
        );
        let text = panel_text(app.world().resource::<PanelRows>().get(PanelId::Transcript));
        assert!(text.contains("saved question"), "{text:?}");
        assert!(text.contains("saved answer"), "{text:?}");

        let _ = std::fs::remove_dir_all(&root);
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rig-show-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
