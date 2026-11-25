//! `rig_niri`: rig-tui 的另一个**完整前端**（feathers + 3D 空间），同时是一个可 fork 的通用 app 模板。
//!
//! 分层（详见 `../docs/design/AGENT_3D_PATTERN.md`）：
//!
//! ```text
//! rig-ecs 内核  ->  batch（RenderBatch + look）  ->  transcript（LookLine -> 行）
//!               ->  frame（Placement 帧布局）/ pane（窗格与滚动）
//!               ->  panels（多面板 + 增量 diff）/ status / editor / hints
//!               ->  feathers_in_3d（3D 渲染）
//! ```
//!
//! 这个 crate 只装**前端**：不实现业务工具、不写业务 look。
//! 业务方（见 `src/main.rs` 的 demo）只需要：注册工具 + 写 look + 往 `PanelRows` 塞行。
//!
//! ## 一帧的顺序
//!
//! 和 rig-tui 一样，视图的 `Update` 分成几段（[`FeathersSystems`]），插件往里挂系统：
//!
//! ```text
//! Prepare  按键、面板按内容定尺寸（编辑器行数）
//! Layout   帧布局：每块面板按 Placement 拿区域，写到 Node 尺寸 + 3D 锚点
//! Draw     各模块产出行：look、状态行、输入行（look 的换行宽度来自 Layout）
//! Fit      窗格按区域窗口化：把 Draw 产出的整段行截成这一帧的可见行
//! Render   行同步成 bevy_ui 节点、浮层合成
//! ```
//!
//! 与 rig-tui 的 `TuiSystems` 相比只有一处差别：它的 `Fit` 在 `Draw` 之前
//! （面板自己拿画布，先裁再画）；我们这里行是**值**（`PanelRows`），
//! 所以窗口化必须排在 `Draw` 之后。

pub mod batch;
pub mod camera_rig;
pub mod clipboard;
pub mod complete;
pub mod editor;
pub mod frame;
pub mod front;
pub mod hints;
pub mod history;
pub mod keys;
pub mod layout;
pub mod niri;
pub mod pane;
pub mod panels;
pub mod picker;
pub mod rich;
pub mod spatial;
pub mod selection;
pub mod status;
pub mod style;
pub mod theme;
pub mod title;
pub mod transcript;
pub mod workspaces;

use bevy::prelude::*;

/// 视图的 `Update` 阶段，按 rig-tui `TuiSystems` 的顺序（见 crate 文档）。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeathersSystems {
    /// 布局前：按键、面板按内容定尺寸。
    Prepare,
    /// 帧布局：每块面板按 [`frame::Placement`] 拿区域。
    Layout,
    /// 行产出之后：窗格按拿到的区域窗口化（见 crate 文档的顺序说明）。
    Fit,
    /// 各模块产出行。
    Draw,
    /// 行同步成节点、浮层合成。
    Render,
}

/// 条带滚动（L2 几何 + L3 交互）：滚动量、夹紧、把列滚进视口。
pub use workspaces::{
    ColumnBounds, WorkspaceScroll, WorkspaceScrollFocus, clamp_scroll, column_bounds, scroll_by,
    scroll_to_visible,
};

/// 业务代码只需要从这里拿东西。
pub mod prelude {
    pub use crate::batch::{
        BatchPlugin, CompletedCalls, ReplyStream, dialogue_batch, dialogue_batch_of, live_additions,
        source_rows,
    };
    pub use crate::editor::{Editor, PromptSubmitted, SubmittedPrompts};
    pub use crate::frame::{
        CellRect, FramePanel, FrameSize, FrameGeometry, FramePlugin, Look, Paint, PanelChrome,
        Placement,
    };
    pub use crate::frame::FrameTree;
    pub use crate::frame::{add_split_key, split_binding, split_transcript};
    pub use crate::layout::{
        Child, Node, PRESET_COLUMN_FRACTIONS, Size, Split, default_tree, next_preset_fraction,
        previous_preset_fraction,
    };
    pub use crate::pane::{
        ActiveTranscript, PanePlugin, PaneScroll, TranscriptPane, TranscriptRows,
    };
    pub use crate::keys::{
        AppKeyBindingsExt, InputAction, KeyBinding, KeyFocus, KeyPress, KeyPressed, KeyScope,
        Keys, Modifiers, PendingInput, RefusedKeys, action_for, run_command,
    };
    pub use crate::picker::{
        ChoiceAction, ChoiceKey, ChoiceKeyPressed, ChoiceKeys,
    };
    pub use crate::complete::{
        Candidate, Completion, CompletionKind, CompletionPlugin, CompletionSource, CompletionState,
        CompletionTrigger, COMPLETION_HINT, TILE_ROWS, completion_hint_rows, completion_rows,
        completion_tile_height, completion_tile_rows, draw_completion, sync_completion_tile,
    };
    pub use crate::history::{
        HISTORY_FILE, HistoryPlugin, HistorySystems, PromptHistory, Sent,
    };
    pub use crate::selection::{ClickKind, Point, SELECTED_MODIFIER, Selection};
    pub use crate::clipboard::{
        ClipboardText, Copied, attach_image_paths, copy_selection, copy_text, dropped_file_text,
        image_paths, paste_from_clipboard,
    };
    pub use crate::rich::{
        diff_rows, highlight_rows, markdown_rows, tool_result_rows, upstream_theme,
    };
    pub use crate::panels::{KeyedRow, PanelId, PanelRows, PanelRoots};
    pub use crate::workspaces::{
        ColumnBounds, WORKSPACE_GAP, Workspace, WorkspaceDrag, WorkspacePanel, WorkspacePanels,
        WorkspaceScroll, WorkspaceScrollFocus, WorkspaceSet, WorkspacesPlugin, active_panel,
        arrange_workspaces, clamp_scroll, column_bounds, drag_workspace, drag_world_delta,
        focus_workspace, place_workspace, scroll_by, scroll_to_visible, spawn_workspace,
        spawn_workspace_with_tree, workspace_step, workspace_strip_height,
    };
    pub use crate::transcript::{Below, BelowSeen, BlockId, Slot, TextId, Transcript};
    pub use crate::batch::{CallFact, CallHistory};
    pub use crate::niri::{Column, Layer, PresetSize, Tile, TileRect, Tiling, TilingPlugin};
    pub use crate::spatial::{Card, CardContent, CardLook, Funnel, SpatialFocus, spawn_card};
    pub use crate::status::{
        AppStatusPartsExt, StatusPart, StatusParts, StatusPlugin, StatusSettings,
    };
    pub use crate::theme::Theme;
    pub use crate::title::WindowTitle;
    pub use crate::transcript::{Row, RowText, TranscriptFont};
    pub use crate::{FeathersFrontPlugin, FeathersSystems, FeathersViewPlugin};

    // rig-tui 的 look 契约（重新导出，业务不用直接依赖 rig-tui）
    pub use rig_tui::{
        CallState, LookLine, RenderBatch, RenderInput, RenderKey, RenderOutput, RenderRequest,
        RenderSystems, ToolRender, TranscriptRender,
    };
}

/// 视图插件（与 `plugins/rig-tui` 的 `TuiViewPlugin` 同形）。
///
/// `build` 装绘制/输入各系统；`finish` 在所有插件 `build` 之后做只能那时做的事
/// （注册 `Agent` 的 `StatusItems`、给已存在的 editor 补键位提示等），
/// 这样插件添加顺序不影响结果——和 rig-tui 的约定一致。
pub struct FeathersViewPlugin;

impl Plugin for FeathersViewPlugin {
    fn build(&self, app: &mut App) {
        // feathers 的 `UiTheme`：面板 chrome（底/边框）的颜色来源。
        //
        // `bevy_feathers::FeathersPlugins` 会 `init_resource::<UiTheme>()` 插一个
        // **空** `ThemeProps`（所有 token 查不到 → fuchsia）。这里只在"缺失或
        // 空"时换成 `dark_theme::create_dark_theme()`：调用方自己配好的主题
        // 不会被覆盖；没装 UI 库的最小 App 也有一份能查的暗色主题。
        let configured = app
            .world()
            .get_resource::<bevy_feathers::theme::UiTheme>()
            .is_some_and(|ui_theme| !theme::is_unconfigured(ui_theme));
        if !configured {
            app.insert_resource(theme::dark_ui_theme());
        }

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
        )
        .insert_resource(theme::Theme::from_env())
        .init_resource::<selection::Selection>()
        .add_message::<clipboard::Copied>()
        .add_systems(
            Update,
            clipboard::show_copied.in_set(FeathersSystems::Prepare),
        )
        .add_plugins((
            frame::FramePlugin,
            panels::PanelsPlugin,
            pane::PanePlugin,
            status::StatusPlugin,
            editor::EditorPlugin,
            hints::HintsPlugin,
            picker::PickerPlugin,
            spatial::SpatialPlugin,
            niri::TilingPlugin,
            title::TitlePlugin,
            front::FrontPlugin,
            complete::CompletionPlugin,
            history::HistoryPlugin::new(),
            selection::SelectionPlugin,
            camera_rig::CameraRigPlugin,
        ));
        // 第 2 批：每个工作区一套面板状态。单独一行是因为 bevy 的
        // `Plugins` 元组最多 15 个，也避免重排上面的插件列表。
        app.add_plugins(workspaces::WorkspacesPlugin);
    }

    fn finish(&self, app: &mut App) {
        // 与 rig-tui 一样在 finish 里补：别的插件可能已 spawn 了 editor / agent。
        // turn 也要求 `StatusItems`（上游 `status::add` 同样注册 `TurnOf`），
        // 这样状态行能直接读到运行中 turn 自己的条目。
        let world = app.world_mut();
        world.register_required_components::<rig_ecs::prelude::Agent, rig_tui::StatusItems>();
        world.register_required_components::<rig_ecs::agent::TurnOf, rig_tui::StatusItems>();
    }
}

/// 旧名字，保留以免破坏已有调用。
pub type FeathersFrontPlugin = FeathersViewPlugin;

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_feathers::theme::UiTheme;

    /// 只跑 `Plugin::build`，不 `update`：这条测试只关心 `UiTheme` 的装配。
    fn view_app(ui_theme: Option<UiTheme>) -> App {
        let mut app = App::new();
        if let Some(ui_theme) = ui_theme {
            app.insert_resource(ui_theme);
        }
        app.add_plugins(FeathersViewPlugin);
        app
    }

    /// F1：空/缺失的 `UiTheme` 由插件补成 feathers 暗色主题，面板 token 查得到颜色。
    #[test]
    fn the_view_plugin_installs_the_feathers_dark_theme() {
        // 缺失：插一份 dark theme。
        let app = view_app(None);
        let theme = app.world().resource::<UiTheme>();
        assert!(!crate::theme::is_unconfigured(theme));

        // `FeathersPlugins` 插的空主题：也补成 dark theme（否则 token 返回 fuchsia）。
        let app = view_app(Some(UiTheme::default()));
        let theme = app.world().resource::<UiTheme>();
        assert!(!crate::theme::is_unconfigured(theme));
        assert_eq!(
            theme.color(&crate::theme::PANEL_BG),
            crate::theme::dark_ui_theme().color(&crate::theme::PANEL_BG)
        );
    }

    /// 调用方自己配好的 `UiTheme` 不被插件覆盖。
    #[test]
    fn the_view_plugin_keeps_a_configured_ui_theme() {
        let custom = Color::srgb(0.13, 0.57, 0.91);
        let mut ui = crate::theme::dark_ui_theme();
        ui.0.semantic_base
            .insert(bevy_feathers::tokens::semantic::SURFACE_PANE_BODY, custom);
        let app = view_app(Some(ui));
        assert_eq!(
            app.world()
                .resource::<UiTheme>()
                .color(&crate::theme::PANEL_BG),
            custom,
            "调用方配好的主题不能被 dark theme 盖掉"
        );
    }
}
