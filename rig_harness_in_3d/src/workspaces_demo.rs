//! 演示：世界层放多个工作区（= 多个 agent 会话），每个工作区是独立的 niri 条带。
//!
//! 设计见 `docs/status/WORKSPACE_MODEL.md`：**工作区内部**是 niri 分裂树，
//! **工作区之间**是绝对空间坐标。这个模块只做"摆几个工作区出来"，
//! 拖拽/整理/聚焦由 `rig_niri::workspaces` 提供。
//!
//! ```sh
//! RF_WORKSPACES=3 cargo run      # 竖直摆 3 个会话
//! ```
//!
//! 默认（不设或设 1）只跑单工作区，行为和以前完全一样。

use bevy::prelude::*;
use rig_niri::layout;
use rig_niri::panels::{KeyedRow, PanelId};
use rig_niri::transcript::Row;
use rig_niri::workspaces::{
    arrange_workspaces, focus_workspace, spawn_workspace, Workspace, WorkspacePanels, WorkspaceSet,
};
use rig_tui::ratatui::style::{Color as RColor, Style};
use rig_tui::ratatui::text::{Line, Span};

/// 一个会话条带在世界里的高度（米）：帧高 × 行高 × 单位换算。
const STRIP_HEIGHT_M: f32 = 28.0 * 18.0 * 0.0018;
/// 首帧摆放用的粗略间距；真正的间距由 `arrange_workspaces` 按帧高算（见 `arrange_once`）。
const STRIP_GAP_M: f32 = 0.03;

pub struct WorkspacesDemoPlugin;

impl Plugin for WorkspacesDemoPlugin {
    fn build(&self, app: &mut App) {
        let count = std::env::var("RF_WORKSPACES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        app.insert_resource(DemoWorkspaces(count))
            .add_systems(Startup, spawn_extra_workspaces)
            .add_systems(Update, (label_workspaces, demo_keys).chain())
            .add_systems(Update, (show_all_once, arrange_once));
    }
}

#[derive(Resource)]
struct DemoWorkspaces(usize);

/// 第 0 个由默认屏幕收编，这里只补 1..count。
fn spawn_extra_workspaces(mut commands: Commands, count: Res<DemoWorkspaces>, mut set: ResMut<WorkspaceSet>) {
    for index in 1..count.0 {
        let pose = Transform::from_xyz(0.0, -(index as f32) * (STRIP_HEIGHT_M + STRIP_GAP_M), 0.0);
        spawn_workspace(&mut commands, &mut set, index as u64, pose, index);
    }
    if count.0 > 1 {
        info!("[workspaces] 摆了 {} 个工作区（竖直间距 {:.2} m）", count.0, STRIP_HEIGHT_M + STRIP_GAP_M);
    }
}

/// 给每个工作区的转写面板写一行标题，说明"这是第几个会话"。
///
/// 没有它，非聚焦工作区是空的，看不出多条条带各自独立。
fn label_workspaces(mut workspaces: Query<(&Workspace, &mut WorkspacePanels)>) {
    for (workspace, mut panels) in &mut workspaces {
        // 第 0 个是默认屏幕收编来的真界面，一行都不要碰。
        if workspace.session == 0 {
            continue;
        }
        let want = format!(
            "session {} · workspace {} · {}",
            workspace.session, workspace.session, if workspace.order == 0 { "0" } else { "n" }
        );
        let same = panels
            .rows
            .get(PanelId::Transcript)
            .first()
            .map(|row| row.row.line.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .is_some_and(|text| text == want);
        if same {
            continue;
        }
        let row = Row::new(Line::from(Span::styled(
            want.clone(),
            Style::default().fg(RColor::LightBlue),
        )));
        let keyed = KeyedRow {
            key: 0,
            hash: simple_hash(&want),
            row,
        };
        // 只在**完全空**时兜底一次：真内容进来以后不再经过这里。
        if panels.rows.get(PanelId::Transcript).is_empty() {
            panels.rows.set(PanelId::Transcript, vec![keyed]);
        }
    }
}

fn simple_hash(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// 演示用按键（真应用里这些是导航手势/命令）：
///
/// | 键 | 作用 |
/// |---|---|
/// | `F1`..`F8` | 聚焦第 n 个工作区（其它隐藏） |
/// | `F9` | 取消聚焦（全部可见） |
/// | `F10` | 自动整理：按 niri 的垂直堆叠重算所有位姿 |
fn demo_keys(world: &mut World) {
    let Some(keys) = world.get_resource::<ButtonInput<KeyCode>>() else {
        return;
    };
    let pressed: Vec<KeyCode> = keys.get_just_pressed().copied().collect();
    for key in pressed {
        match key {
            KeyCode::F1 | KeyCode::F2 | KeyCode::F3 | KeyCode::F4 | KeyCode::F5 | KeyCode::F6
            | KeyCode::F7 | KeyCode::F8 => {
                let index = match key {
                    KeyCode::F1 => 0,
                    KeyCode::F2 => 1,
                    KeyCode::F3 => 2,
                    KeyCode::F4 => 3,
                    KeyCode::F5 => 4,
                    KeyCode::F6 => 5,
                    KeyCode::F7 => 6,
                    _ => 7,
                };
                let target = world
                    .get_resource::<WorkspaceSet>()
                    .and_then(|set| set.get(index));
                if let Some(workspace) = target {
                    focus_workspace(world, workspace);
                    info!("[workspaces] 聚焦 F{}", index + 1);
                }
            }
            KeyCode::F9 => {
                if let Some(mut set) = world.get_resource_mut::<WorkspaceSet>() {
                    set.focused = None;
                    info!("[workspaces] 取消聚焦（内容不再写任何条带；所有条带仍然可见）");
                }
            }
            KeyCode::F10 => {
                arrange_workspaces(world);
                info!("[workspaces] 自动整理（niri 垂直堆叠）");
            }
            _ => {}
        }
    }
}

/// 首帧整理一次：让间距/朝向由 `arrange_workspaces` 统一算（和按 F10 的结果一致），
/// 避免"spawn 时的间距"和"整理后的间距"不一样。
fn arrange_once(mut done: Local<bool>, set: Res<WorkspaceSet>, mut commands: Commands) {
    if *done {
        return;
    }
    *done = true;
    if set.items.len() > 1 {
        commands.queue(|world: &mut World| {
            arrange_workspaces(world);
        });
    }
}

/// `RF_SHOW_ALL=1`：启动后取消聚焦，一次看到所有条带。
fn show_all_once(mut shown: Local<bool>, set: Res<WorkspaceSet>) {
    if *shown || std::env::var("RF_SHOW_ALL").is_err() {
        return;
    }
    *shown = true;
    info!("[workspaces] 共 {} 个工作区，竖直排列，全部可见", set.items.len());
}

/// 默认树（单工作区时由 bootstrap 收编默认屏幕，这里只给测试用）。
pub fn demo_tree() -> layout::Node {
    layout::default_tree()
}
