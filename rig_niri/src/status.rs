//! 状态行：画 rig 自己的 `StatusItems` / `AppStatus`（与 rig-tui 同一份数据）。
//!
//! rig-tui 的 `status.rs` 把条目裁成两行：左边一串、右边一串，行太窄时先丢
//! `keep` 最低的，能缩的条目缩到 `min_width` 再丢（`status::line`，公开函数）。
//! 这里**直接调用它**，不重写裁剪规则——状态行的行为因此和终端一模一样；
//! 我们只负责把这两行摆成一行内的两端（`Row::with_right`，bevy_ui 的
//! `SpaceBetween` 负责右对齐，不靠算空格）。
//!
//! 和上游 `rig-tui-0.47.0/src/status.rs` 的对应关系：
//!
//! | 上游 | 这里 | 说明 |
//! |---|---|---|
//! | `StatusSources::line`（`:400`） | [`status_rows`] | 合并顺序 `AppStatus`、agent 自己的、运行中 turn 的（上游 `:413`），`[status] hide` 过滤（`:409`），没有 agent 时补 `no agent`（`:406`） |
//! | `no agent` 占位（`:406`） | [`no_agent_item`] | 名字 `agent`、左、`order` 30（model 那一格）、`keep` `u8::MAX`、`Tone::Dim` |
//! | turn 的 items（`:404`） | [`status_rows`] | 从 `ShownAgent` 指向的 agent 上的 `ActiveTurn` 找到 turn 实体，再取它的 `StatusItems` |
//! | `StatusSettings.hide`（`:272`） | [`StatusSettings`] | `[status] hide`；只在装配层已有 `rig_harness::prelude::Settings` 时读（可选依赖），没有就全显示 |
//! | `StatusParts` / `AppStatusPartsExt`（`:329`/`:347`） | [`StatusParts`] / [`AppStatusPartsExt`] | 最小可用版：资源里存画出来的条目，同名替换、空文本隐藏；不逐字照抄上游“只记名字给 `/plugins`”的元数据语义 |
//! | `StatusLine` 组件（`:382`） | 不做 | 见下面“没做的部分” |
//! | `StatusSystems::{Show, Edit}`（`:216`） | 不做 | 见下面“没做的部分” |
//!
//! ## 没做的部分
//!
//! * **`StatusLine` 组件**：上游它是 `TuiPanel` 的兄弟组件，任意实体挂上就在自己
//!   `PanelCanvas` 的一行里画一条（可以是别的 agent 的）。我们的面板不是“实体 +
//!   画布”，而是 [`PanelRows`] 里按 [`PanelId`]（固定枚举）索引的行值，再由
//!   `panels` 同步成 `bevy_ui` 节点；要支持任意实体自己画一条，得让面板行归属
//!   从 `PanelId` 扩成“实体 → 画布”，也就是改 `panels.rs` / `frame.rs` 的面板
//!   画布契约（本轮不归这里改）。所以状态行仍然只有 [`PanelId::Status`] 一条，
//!   也没有 `StatusLine.agent` 的“画别的 agent”入口。
//! * **`StatusSystems::{Show, Edit}` 阶段**：上游的状态行条目是**缓存**在
//!   `StatusItems` 组件里、由插件在 `PostUpdate` 按阶段写，视图最后再画；我们的
//!   状态行每帧在 [`crate::FeathersSystems::Draw`] 里从 `AppStatus` / agent / turn
//!   / [`StatusParts`] 资源**现算**一行（没有缓存阶段）。插件加条目走
//!   [`AppStatusPartsExt`] 或直接写 `AppStatus`，先后顺序不影响结果，因此不需要
//!   两个阶段。等以后有插件需要在“所有 Show 之后统一 Edit”时再加也不迟。

use bevy::prelude::*;
use rig_ecs::agent::ActiveTurn;
use rig_harness::prelude::{AppSettingsExt, Settings, ShownAgent};
use rig_tui::ratatui::style::Style;
use rig_tui::ratatui::text::Line;
use rig_tui::{AppStatus, Side, StatusItem, StatusItems, Tone};
use serde::Deserialize;

use crate::frame::{PanelAreas, PanelRoots};
use crate::panels::{KeyedRow, PanelId, PanelRows};
use crate::transcript::Row;

/// `[status]` 在 `settings.toml` 里的段名。
pub const SECTION: &str = "status";

/// 没有帧布局（`PanelAreas`/`PanelRoots`）时状态行按多少列裁：沿用旧代码的 96 列。
const DEFAULT_WIDTH: u16 = 96;

/// `[status]` 段：状态行读的那张表（上游 `status.rs:272`）。
///
/// 上游在 `StatusLinePlugin::build` 里 `add_setting::<StatusSettings>("status")`。
/// 这里同样是“装配层给了 [`Settings`] 才读”：`rig_harness_in_3d` 的
/// `settings::install` 在插件之前插入 `Settings`，所以 app 里这张表会被读到、
/// 也会被记为已读；单独用 `rig_niri`（没有 `Settings` 资源）时不读文件、
/// `hide` 是空表——全显示。
#[derive(Resource, Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct StatusSettings {
    /// 不画的状态条目名：按 [`StatusItem::name`] 过滤**所有来源**的条目
    /// （和上游一样，条目本身还在，别的插件仍然读得到）。
    pub hide: Vec<String>,
}

impl StatusSettings {
    /// 名字 `name` 的条目是否不画。
    pub fn hidden(&self, name: &str) -> bool {
        self.hide.iter().any(|hidden| hidden == name)
    }
}

/// 没有显示中的 agent 时，占 model 那一格的条目（上游 `status.rs:406`）。
///
/// 上游原文：`StatusItem::new("agent", Side::Left, 30, u8::MAX).says("no agent",
/// Tone::Dim)`。`order` 30 就是默认插件里 `model` 的位置，`keep = u8::MAX`
/// 表示状态行再窄也留着它。
pub fn no_agent_item() -> StatusItem {
    StatusItem::new("agent", Side::Left, 30, u8::MAX).says("no agent", Tone::Dim)
}

/// 状态行的一部分：画出来的条目 + 给 `/plugins` 之类看的短标签。
///
/// 上游的 [`rig_tui::status::StatusPart`] 只记名字/标签/位置，画什么由插件的
/// 系统每帧往 `StatusItems` 里写；这里是“最小可用版”：条目连同文字一起放在
/// [`StatusParts`] 资源里，每帧并进状态行（见 [`AppStatusPartsExt`]）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusPart {
    /// 它画出来的条目；名字是 `item.name`，也是“同名替换/隐藏”的身份。
    pub item: StatusItem,
    /// 给界面列表用的短标签，比如 `Cost`；上游 `/plugins` 用的是它。
    pub label: &'static str,
}

impl StatusPart {
    /// 条目的名字（[`StatusItem::name`]）。
    pub fn name(&self) -> &'static str {
        self.item.name
    }
}

/// 插件往状态行加的条目（上游 `StatusParts`，`:296`，最小可用版）。
///
/// 每帧由 [`status_rows`] 并进状态行：和 `AppStatus` / agent / turn 上的同名
/// 条目**替换**（上游 `StatusItems::show` 的语义），文字为空则把同名条目藏起来。
/// 顺序按 (侧, `order`, 名字) 排，和上游 `StatusParts::note` 一致，方便以后
/// 给 `/plugins` 列出来。
#[derive(Resource, Clone, Debug, Default)]
pub struct StatusParts(Vec<StatusPart>);

impl StatusParts {
    /// 加上（或替换）`item`，标签 `label`；同名旧条目不再画。
    pub fn show(&mut self, item: &StatusItem, label: &'static str) {
        self.0.retain(|part| part.item.name != item.name);
        self.0.push(StatusPart {
            item: item.clone(),
            label,
        });
        self.0.sort_by_key(|part| {
            (
                part.item.side == Side::Right,
                part.item.order,
                part.item.name,
            )
        });
    }

    /// 去掉名字 `name` 的部分。
    pub fn hide(&mut self, name: &str) {
        self.0.retain(|part| part.item.name != name);
    }

    /// 各部分，按 (侧, `order`, 名字)。
    pub fn iter(&self) -> impl Iterator<Item = &StatusPart> {
        self.0.iter()
    }

    /// 名字 `name` 的部分。
    pub fn get(&self, name: &str) -> Option<&StatusPart> {
        self.0.iter().find(|part| part.item.name == name)
    }
}

/// 声明/加入状态行的一部分（上游 `AppStatusPartsExt`，`:329`）。
///
/// 上游的 `add_status_part` 只声明“这个插件会画这个名字”，给 `/plugins` 用；
/// 这里把条目本身也收下，所以加进去就连文字一起出现在状态行上（见
/// [`StatusParts`]）。在插件的 `build` 里调用即可。
pub trait AppStatusPartsExt {
    /// 加入（或替换）`item`，标签 `label`。
    fn add_status_part(&mut self, item: &StatusItem, label: &'static str) -> &mut Self;

    /// 去掉名字 `name` 的部分。
    fn remove_status_part(&mut self, name: &str) -> &mut Self;
}

impl AppStatusPartsExt for App {
    fn add_status_part(&mut self, item: &StatusItem, label: &'static str) -> &mut Self {
        self.init_resource::<StatusParts>();
        self.world_mut()
            .resource_mut::<StatusParts>()
            .show(item, label);
        self
    }

    fn remove_status_part(&mut self, name: &str) -> &mut Self {
        if let Some(mut parts) = self.world_mut().get_resource_mut::<StatusParts>() {
            parts.hide(name);
        }
        self
    }
}

/// 把 `AppStatus`、显示中的 agent 和它运行中 turn 的 `StatusItems`、以及
/// [`StatusParts`] 合起来，按 [`StatusSettings`] 过滤，按面板宽度裁好，写进
/// 状态面板（左边一条、右边一条）。
///
/// 合并顺序照上游 `StatusSources::line`（`:400`）：`AppStatus`、agent 自己的、
/// 运行中 turn 的；没有 agent 时最后补 [`no_agent_item`]（上游 `:406`）。
/// `[status] hide` 在补 `no agent` 之前过滤条目（上游 `:409`）——占位不受
/// `hide` 影响，和上游一致。
pub fn status_rows(
    mut panels: ResMut<PanelRows>,
    app_status: Option<Res<AppStatus>>,
    shown: Option<Res<ShownAgent>>,
    status_items: Query<&StatusItems>,
    turns: Query<&ActiveTurn>,
    parts: Option<Res<StatusParts>>,
    settings: Option<Res<StatusSettings>>,
    areas: Option<Res<PanelAreas>>,
    roots: Option<Res<PanelRoots>>,
) {
    // 显示的 agent：上游 `status.agent.or(self.shown.0)`；我们没有 `StatusLine`
    // 组件，所以只有 `ShownAgent` 一个来源。
    let agent = shown.as_ref().and_then(|shown| shown.0);

    let mut items: Vec<StatusItem> = Vec::new();
    if let Some(app) = &app_status {
        items.extend(app.0.iter().cloned());
    }
    if let Some(agent) = agent {
        // agent 自己的 items，再是它运行中 turn 的（上游 `:402`-`:404`）。
        if let Ok(own) = status_items.get(agent) {
            items.extend(own.iter().cloned());
        }
        if let Ok(turn) = turns.get(agent) {
            if let Ok(turn_items) = status_items.get(turn.turn()) {
                items.extend(turn_items.iter().cloned());
            }
        }
    }
    // `StatusParts` 覆盖同名条目（上游 `StatusItems::show`：先 hide 再 push；
    // 空文字只 hide 不画）。放在 `hide` 过滤之前，所以被设置隐藏的部分也不画。
    if let Some(parts) = &parts {
        for part in parts.iter() {
            items.retain(|item| item.name != part.name());
            if !part.item.text.is_empty() {
                items.push(part.item.clone());
            }
        }
    }
    if let Some(settings) = &settings {
        items.retain(|item| !settings.hidden(item.name));
    }
    // 没有 agent 时在 model 那一格画 `no agent`（上游 `:406`）。
    if agent.is_none() {
        items.push(no_agent_item());
    }
    if items.is_empty() {
        panels.set(PanelId::Status, Vec::new());
        return;
    }
    let width = roots
        .as_ref()
        .and_then(|roots| roots.entity(PanelId::Status))
        .and_then(|root| areas.as_ref().and_then(|areas| areas.get(root)))
        .map_or(DEFAULT_WIDTH, |area| area.cols);
    // rig-tui 的裁剪：左边一条、右边一条，丢/缩按 keep 与 min_width。
    let (left, right) = rig_tui::status::line(items.iter(), width);
    let mut row = Row::new(left);
    if right.width() > 0 {
        row = row.with_right(right);
    }
    let line = row.line.clone();
    let right = row.right.clone();
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        format!("{line:?}").hash(&mut hasher);
        if let Some(right) = &right {
            format!("{right:?}").hash(&mut hasher);
        }
        hasher.finish()
    };
    panels.set(
        PanelId::Status,
        vec![KeyedRow { key: 1, hash, row }],
    );
}

/// 注册状态行：`AppStatus` / `StatusParts` / `[status]` 设置资源 + 每帧把条目
/// 画成一行。
///
/// `Agent`（以及 `TurnOf`）上的 `StatusItems` 由 [`crate::FeathersViewPlugin`]
/// 在 `finish` 里注册（和 rig-tui 的 `TuiViewPlugin` 一样，插件添加顺序因此
/// 不影响结果）。
pub struct StatusPlugin;

impl Plugin for StatusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppStatus>()
            .init_resource::<StatusParts>();
        // 可选依赖：装配层给了 `Settings`（`rig_harness_in_3d` 的 `install`）才读
        // `[status]`；没有就全是默认值（什么都不隐藏），也不去碰真实的
        // settings.toml。
        if app.world().contains_resource::<Settings>() {
            app.add_setting::<StatusSettings>(SECTION);
        } else {
            app.init_resource::<StatusSettings>();
        }
        app.add_systems(Update, status_rows.in_set(crate::FeathersSystems::Draw));
    }
}

/// 状态行的一行样式（给业务画自己的状态条目时用）。
pub fn plain(text: impl Into<String>) -> Line<'static> {
    Line::from(rig_tui::ratatui::text::Span::styled(
        text.into(),
        Style::default(),
    ))
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;
    use rig_ecs::agent::TurnOf;
    use rig_harness::prelude::{Settings, ShownAgent};
    use rig_tui::ratatui::text::Line;
    use rig_tui::{AppStatus, Side, StatusItem, StatusItems, Tone};

    use super::{AppStatusPartsExt, StatusParts, StatusPlugin, StatusSettings, no_agent_item};
    use crate::frame::{CellRect, PanelArea, PanelAreas, PanelRoots};
    use crate::panels::{PanelId, PanelRows};

    /// 只有状态行系统的 headless app：状态面板宽 `width` 列。
    ///
    /// `settings` 是真实的 `Settings`（TOML 文本），在插件之前插入——和 app 里
    /// `settings::install` 的位置一样，`[status]` 因此会被 `add_setting` 读到。
    fn app_with(settings: Option<&str>, width: u16) -> App {
        let mut app = App::new();
        app.init_resource::<PanelRows>()
            .init_resource::<PanelAreas>()
            .init_resource::<PanelRoots>();
        if let Some(text) = settings {
            app.insert_resource(Settings::from_toml(text));
        }
        app.add_plugins(StatusPlugin);
        set_width(&mut app, width);
        app
    }

    fn app(width: u16) -> App {
        app_with(None, width)
    }

    /// 状态面板的根 + 区域（宽 `width` 列）。
    fn set_width(app: &mut App, width: u16) {
        let root = match app.world().resource::<PanelRoots>().entity(PanelId::Status) {
            Some(root) => root,
            None => {
                let root = app.world_mut().spawn_empty().id();
                app.world_mut()
                    .resource_mut::<PanelRoots>()
                    .0
                    .insert(PanelId::Status, root);
                root
            }
        };
        app.world_mut().resource_mut::<PanelAreas>().0.insert(
            root,
            PanelArea {
                rect: CellRect::new(0, 0, width, 1),
                size: Vec2::ZERO,
                cols: width,
                rows: 1,
                center: None,
            },
        );
    }

    /// 状态面板这一帧的左半、右半文本。
    fn status_line(app: &App) -> (String, String) {
        let rows = app.world().resource::<PanelRows>();
        let Some(keyed) = rows.get(PanelId::Status).first() else {
            return (String::new(), String::new());
        };
        (
            line_text(&keyed.row.line),
            keyed.row.right.as_ref().map(line_text).unwrap_or_default(),
        )
    }

    fn line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn show_in_app_status(app: &mut App, item: StatusItem) {
        app.world_mut().resource_mut::<AppStatus>().0.show(item);
    }

    fn agent_with(app: &mut App, items: &[StatusItem]) -> Entity {
        let mut status = StatusItems::default();
        for item in items {
            status.show(item.clone());
        }
        app.world_mut().spawn(status).id()
    }

    #[test]
    fn without_a_shown_agent_the_line_says_no_agent_in_the_models_place() {
        let mut app = app(80);
        show_in_app_status(
            &mut app,
            StatusItem::new("title", Side::Left, 20, 14).says("my title", Tone::Bold),
        );
        show_in_app_status(
            &mut app,
            StatusItem::new("conversation", Side::Left, 40, 13).says("3 msgs", Tone::Dim),
        );
        app.update();
        let (left, right) = status_line(&app);
        // 没有 ShownAgent：占位在 `order` 30（model 那一格），夹在 20 与 40 之间。
        assert_eq!(left, "my title  no agent  3 msgs");
        assert!(right.is_empty(), "{right:?}");

        // 有显示中的 agent（哪怕它还没有 items）就不画占位。
        let agent = agent_with(&mut app, &[]);
        app.insert_resource(ShownAgent(Some(agent)));
        app.update();
        let (left, right) = status_line(&app);
        assert_eq!(left, "my title  3 msgs");
        assert!(right.is_empty(), "{right:?}");
    }

    #[test]
    fn the_no_agent_placeholder_is_in_the_models_slot() {
        let item = no_agent_item();
        assert_eq!(item.name, "agent");
        assert_eq!(item.text, "no agent");
        assert_eq!(item.tone, Tone::Dim);
        // 上游 `status.rs:406`：左、`order` 30（默认插件的 `model` 位置）、
        // `keep` `u8::MAX`（再窄也留）。
        assert_eq!(item.side, Side::Left);
        assert_eq!(item.order, 30);
        assert_eq!(item.keep, u8::MAX);
    }

    #[test]
    fn a_running_turns_items_join_the_line() {
        let mut app = app(100);
        let agent = agent_with(
            &mut app,
            &[StatusItem::new("model", Side::Left, 30, u8::MAX).says("acme-model", Tone::Bold)],
        );
        app.insert_resource(ShownAgent(Some(agent)));
        app.update();
        let (left, _) = status_line(&app);
        assert!(left.contains("acme-model"), "{left:?}");
        assert!(!left.contains("turn 3s"), "{left:?}");

        // turn 跑起来（`ActiveTurn` 由 `TurnOf` 关系自动挂到 agent 上），
        // 它的 items 由 `status_rows` 自己按 `ActiveTurn` 找到。
        let mut turn = StatusItems::default();
        turn.show(StatusItem::new("turn", Side::Left, 80, 11).says("turn 3s", Tone::Yellow));
        turn.show(StatusItem::new("tool", Side::Left, 81, 11).says("grep", Tone::Dim));
        app.world_mut().spawn((TurnOf(agent), turn));
        app.update();
        let (left, _) = status_line(&app);
        assert!(left.contains("turn 3s"), "{left:?}");
        assert!(left.contains("grep"), "{left:?}");
        assert!(
            left.find("acme-model") < left.find("turn 3s"),
            "turn 的条目按 order 排在 model 之后：{left:?}"
        );
    }

    #[test]
    fn status_hide_keeps_the_items_out_of_the_line() {
        let mut app = app_with(Some("[status]\nhide = [\"cost\", \"turn\"]\n"), 100);
        // 真实的 `Settings` 反序列化：不是默认值，也没有 problem。
        let problems = app.world().resource::<Settings>().problems().to_vec();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            app.world().resource::<StatusSettings>().hide,
            ["cost".to_string(), "turn".to_string()]
        );

        show_in_app_status(
            &mut app,
            StatusItem::new("cost", Side::Right, 30, 3).says("$0.12", Tone::Dim),
        );
        show_in_app_status(
            &mut app,
            StatusItem::new("tokens", Side::Right, 20, 4).says("12k tok", Tone::Cyan),
        );
        let agent = agent_with(
            &mut app,
            &[
                StatusItem::new("model", Side::Left, 30, u8::MAX).says("acme-model", Tone::Bold),
                StatusItem::new("turn", Side::Left, 80, 11).says("turn 3s", Tone::Yellow),
            ],
        );
        app.insert_resource(ShownAgent(Some(agent)));
        // 插件自己的部分也按同一条规则隐藏。
        app.add_status_part(
            &StatusItem::new("cost", Side::Right, 30, 3).says("part $0.5", Tone::Dim),
            "Cost",
        );
        app.update();
        let (left, right) = status_line(&app);
        assert!(left.contains("acme-model"), "{left:?}");
        assert!(!left.contains("turn 3s"), "{left:?}");
        assert_eq!(right, "12k tok");
        // 隐藏只影响画：条目还在 `AppStatus` 里，别的插件照读（上游语义）。
        assert!(
            app.world()
                .resource::<AppStatus>()
                .0
                .iter()
                .any(|item| item.name == "cost")
        );
    }

    #[test]
    fn a_part_appears_and_replaces_an_item_of_its_name() {
        let mut app = app(100);
        show_in_app_status(
            &mut app,
            StatusItem::new("cost", Side::Right, 30, 3).says("$9", Tone::Plain),
        );
        // 声明即画：`AppStatusPartsExt` 连同文字一起收下（上游只记名字/标签）。
        app.add_status_part(
            &StatusItem::new("cost", Side::Right, 30, 3).says("$0.12", Tone::Dim),
            "Cost",
        );
        // 声明了但没文字的部分不画（上游“declared yet not drawn”）。
        app.add_status_part(&StatusItem::new("budget", Side::Right, 35, 6), "Budget");
        app.update();
        let (_, right) = status_line(&app);
        assert_eq!(right, "$0.12");
        assert!(!right.contains("$9"), "{right:?}");

        // 再声明同名的一项：旧的不再出现，部分本身也不重复。
        app.add_status_part(
            &StatusItem::new("cost", Side::Right, 30, 3).says("$0.10", Tone::Dim),
            "Cost",
        );
        app.update();
        let (_, right) = status_line(&app);
        assert_eq!(right, "$0.10");
        assert!(!right.contains("$0.12"), "{right:?}");

        let parts = app.world().resource::<StatusParts>();
        let declared: Vec<(&str, &str)> =
            parts.iter().map(|part| (part.name(), part.label)).collect();
        assert_eq!(declared, [("cost", "Cost"), ("budget", "Budget")]);

        // 撤掉声明：状态行不再由它提供条目（`budget` 本来也没画，所以没有变化）。
        app.remove_status_part("budget");
        assert_eq!(app.world().resource::<StatusParts>().iter().count(), 1);
        app.remove_status_part("cost");
        app.update();
        let (_, right) = status_line(&app);
        assert_eq!(right, "$9", "撤掉部分后回到 `AppStatus` 里的条目");
    }

    #[test]
    fn an_empty_part_hides_an_item_of_its_name() {
        let mut app = app(100);
        show_in_app_status(
            &mut app,
            StatusItem::new("cost", Side::Right, 30, 3).says("$9", Tone::Plain),
        );
        // 声明了但没文字：`StatusItems::show` 的语义（上游 `:153`），把同名条目藏起来。
        app.add_status_part(&StatusItem::new("cost", Side::Right, 30, 3), "Cost");
        app.update();
        let (_, right) = status_line(&app);
        assert!(right.is_empty(), "{right:?}");
        assert_eq!(
            app.world()
                .resource::<StatusParts>()
                .get("cost")
                .map(|part| part.label),
            Some("Cost"),
            "声明的部分本身还记着标签"
        );
    }

    #[test]
    fn a_narrow_line_still_drops_by_keep_and_cuts_by_min_width() {
        let mut app = app(20);
        // 有显示的 agent，免得 `no agent` 占位挤进这一行。
        let agent = agent_with(&mut app, &[]);
        app.insert_resource(ShownAgent(Some(agent)));
        show_in_app_status(
            &mut app,
            StatusItem::new("session", Side::Left, 10, 2).says("my session", Tone::Dim),
        );
        show_in_app_status(
            &mut app,
            StatusItem::new("model", Side::Left, 30, u8::MAX).says("model", Tone::Bold),
        );
        show_in_app_status(
            &mut app,
            StatusItem::new("goal", Side::Left, 60, 20)
                .says("goal: ship the parser", Tone::Plain)
                .shrinks_to(8),
        );
        app.update();
        // `keep` 最低的 session 先丢；goal 缩到 min_width 之上。
        let (left, _) = status_line(&app);
        assert_eq!(left, "model  goal: ship t…");

        // 再窄：缩到 min_width 也放不下就丢，`keep` 最高的 model 留着。
        set_width(&mut app, 14);
        app.update();
        let (left, _) = status_line(&app);
        assert_eq!(left, "model");
    }
}
