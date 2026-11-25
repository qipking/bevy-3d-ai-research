# 界面重组：改用 bevy_feathers 标准 UI（不再追求终端像素还原）

> **状态**：**F1–F5 已完成**（2026-11-26，提交 `5bee819`、`8c78488`；`rig_niri` **526** 个
> lib 测试、`cargo test --workspace --offline` 全绿——这是 F1–F5 的基线；写文档期间 `niri-l1`
> 并行改 `layout.rs`/`frame.rs`/`picker.rs`，工作树已到 **528**，以复跑为准）。方向由用户 2026-11-26 定调，取代
> "rig-tui 像素级还原"的口径。
> **原话**：「用不着像素级还原，这样工作量太大，而 rig-tui 本来就只是终端 UI，可修改性很大，
> 用 feathers 的标准 UI 库来重新组装界面就行了，不然成本太大了。」
> **配套**：布局按 [`NIRI_SCROLL.md`](NIRI_SCROLL.md)；**L1（列宽模型）正在进行中，不算完成**。
> 行号是 2026-11-26 00:5x 的快照，函数名/常量名是稳定锚。

## 1. 口径

| 层 | 结论 |
|---|---|
| 行为/数据：agent、会话、工具卡、补全、历史、剪贴板、键位语义、选择 | **保留**（`rig_ecs` + `rig_harness` + `rig_niri` 的引擎），测试就是资产 |
| 视觉：面板外观、按钮、列表、输入框、状态行、滚动条、配色/圆角/间距 | **改用 `bevy_feathers` 的标准组件与 token**，不再逐像素对齐终端 |
| 布局 | niri 式（`NIRI_SCROLL.md`），列宽/滚动/聚焦；L1 进行中 |
| 3D | `feathers_in_3d` 镜像 bevy_ui（Node 背景/边框/圆角 + 文字 + **Outline**；**不**支持图标图片） |

**明确不做的**：终端格对齐、`keep/min_width` 的逐字节复刻、任何"和终端像素一致"的验收；
hints `fit_keys` 的裁剪细节与 `status` 的阶段拆分也不再作为验收项（代码现在恰好对齐，但不为它返工）。
具体见 §6。

## 2. 已完成：F1–F5 实际用了什么

| 步 | 结果 | 主要落点 |
|---|---|---|
| **F1** | 面板 chrome 全 token 化 | `theme.rs` 的 token 常量（`:98`–`:141`）、`frame::Look::panel`（`frame.rs:926`）、`sync_panel_chrome`（`frame.rs:962`）；测试 `frame.rs:2042`/`:2110`、`theme.rs:629` |
| **F2** | Ask/picker 行按钮化 | `transcript.rs::RowButton`（`:105`）、`update_row_button_styles`（`:176`）、`update_row_focus_rings`（`:211`）；测试 `transcript.rs:1841`/`:1900`/`:1927` |
| **F3** | picker 面板 token 化 + 纯 Node 滚动条 | `picker.rs:861`（容器 `Look::panel`）、`spawn_scrollbar`（`:719`）、`sync_picker_scrollbar`（`:1340`）；测试 `picker.rs:2634`/`:2767`（picker 正被 L1 并行改，行号会漂） |
| **F4** | 转写滚动条（含拖拽） | `pane.rs::ScrollbarModel`（`:248`）、`sync_transcript_scrollbars`（`:460`）、拖拽观察者（`:527`/`:569`/`:592`）；测试 `pane.rs:2982`/`:3085`/`:3110`/`:3170` |
| **F5** | 输入框外壳换 feathers 外观 | `editor.rs::sync_input_field`（`:1494`）、`sync_input_row_height`（`:1557`）、常量 `:154`–`:168`；测试 `editor.rs:3601` |
| 附加 | `feathers_in_3d` 支持 `Outline` 镜像（焦点环） | `world3d.rs:109`/`:127`/`:166`/`:279`–`:356` |

### 2.1 F1：面板 chrome 的 token

`theme.rs` 把 feathers 的颜色 token 收成命名常量（面板/细条/输入/确认各有底色）：

| 常量 | feathers token | 用在哪 |
|---|---|---|
| `PANEL_BG`（`theme.rs:98`） | `PANE_BODY_BG` | 转写、`Pane2`（`frame.rs:1045`） |
| `BAR_BG`（`:100`） | `PANE_HEADER_BG` | 状态行、键位提示、补全 tile |
| `INPUT_BG`（`:102`） | `TEXT_INPUT_BG` | 输入框 |
| `CONFIRM_BG`（`:104`） | `SUBPANE_HEADER_BG` | 确认 tile |
| `PANEL_BORDER`（`:106`） | `SUBPANE_BODY_BORDER` | 所有面板边框 |
| `FOCUS_BORDER`（`:108`） | `FOCUS_RING` | 输入框焦点态 |

- 面板根挂 `ThemeBackgroundColor`/`ThemeBorderColor`（`frame.rs:1024`–`:1029`），
  `sync_panel_chrome` 每帧按 `UiTheme` 解析成真实颜色（没有资源时用 feathers 暗色主题兜底，
  值不变不写组件）。`FeathersViewPlugin` 只在 `UiTheme` 缺失/为空时插暗色主题
  （`lib.rs:151`–`:159`），不覆盖调用方配置。
- 默认屏幕的六块面板全部 `Look::panel(...)`（`frame.rs:775`–`:813`）。

### 2.2 F2：可点击行借 feathers 按钮的视觉

- **没有**换成 `feathers::controls::button::Button`（它自带 `Button`/`Hovered`/`Pressed`/`TabIndex`，
  会和现有 picking、选区、键盘冲突）。做法是一个纯视觉组件 `RowButton`：
  `kind`（`Choice`/`Picker`）+ `selected`（`transcript.rs:105`），每帧把
  `Interaction` + `PickingInteraction` 折成 pressed > hovered > none（`:176`），底色取
  `BUTTON_BG*` / `BUTTON_PLAIN_BG*` / `BUTTON_PRIMARY_BG*` token，圆角 4px、水平内边距 8px
  （`:84`–`:88`）。
- picker 当前项用 `BUTTON_PRIMARY_*`；键盘选中的条目（Alt+↑↓）加 feathers 同值的
  `Outline`（2px 宽 + 2px 偏移，`FOCUS_RING` 色，`:211`）。
- 交互零变化：点击、Alt+1..9、过滤、多选、preview 都照旧，回归测试钉住
  （`transcript.rs:1841`/`:1900`/`:1927`）。

### 2.3 F3：picker 的 token 与滚动条

- 容器与 preview 面板从硬编码深蓝/黄边换成 `Look::panel(PANEL_BG, PANEL_BORDER)`
  （`picker.rs:861`/`:976`）；列表 tile 仍然是透明 `Look::new(Color::NONE, ...)`（`:868`），
  底色由容器给。
- 滚动条是**纯 `Node`**：轨道 6px、离右缘 4px、圆角 3px、滑块最短 8px
  （`theme.rs:130`–`:141`），颜色取 feathers `SCROLLBAR_BG`/`SCROLLBAR_THUMB`
  （`picker.rs:720`–`:721`）。滑块长度/位置按 `PickerInput::scroll` 与可见行算，
  支持滚轮与拖滑块，PgUp/PgDn/↑↓ 同帧跟随，内容放得下时收起
  （`sync_picker_scrollbar` `:1340`）。

### 2.4 F4：转写滚动条

- 每个 `TranscriptPane`（主转写与 `Pane2`）挂轨道 + 滑块，观察者自动覆盖拆出来的窗格
  （`pane.rs:398`、`:1245`–`:1248`）。
- `ScrollbarModel` 按 总行/可见行/`top`/`follow` 算（`:248`）；拖拽观察者
  `PointerDragStart/Drag/End` 改 `top` 并夹紧，拖到底恢复 follow（`:527`–`:592`）；
  滚轮落在滑块上归它服务的窗格。
- 滚动条不改 `↓ more below` / `↓ new output` 的判定（测试 `pane.rs:3170`）。

### 2.5 F5：输入框外壳

- 底色 `TEXT_INPUT_BG`、圆角 4（`editor.rs:168`）、左右内边距 3px（`:154`）、行高
  `feathers::constants::size::ROW_HEIGHT` = 24（`:160`）、字号 `COMPACT_FONT` = 13（`:163`）。
- 焦点在输入框时边框用 `FOCUS_RING`，焦点在别的面板时用 `SUBPANE_BODY_BORDER`
  （`sync_input_field` `:1494`）；每个工作区只有聚焦的那个是焦点态。
- 多行仍是**一块框**：`sync_input_row_height`（`:1557`）只把输入行的高度换成 24px，
  编辑器行模型不动；空输入保底 3 行 = 72px，10 行上限 260px（`input_tile_height` `:176`）。

## 3. 仍是常量 / 直接颜色的地方

feathers 只有**颜色**是 design token；间距、边框宽度、圆角、字号/行高在
`bevy_feathers::constants` 里是普通常量。所以下面这些是本仓命名的常量，不是漏网的硬编码：

| 常量 | 值 | 为什么还是常量 |
|---|---|---|
| `PANEL_PADDING` / `PANEL_BORDER`（`frame.rs:60`/`:61`） | 8 / 2 | feathers 没有 spacing token；对齐 `containers::pane` 的观感 |
| `THIN_PADDING` / `THIN_BORDER`（`frame.rs:64`/`:65`） | 4 / 1 | 同上，细条（状态/提示） |
| `PANEL_RADIUS`（`theme.rs:116`） | 4 | feathers 圆角不是 token；对齐 pane/group/text input 的 4px |
| `ROW_BUTTON_RADIUS`/`PADDING`/`ROW_FOCUS_WIDTH`（`transcript.rs:84`–`:88`） | 4 / 8 / 2 | 照抄 Button scene 与 `FocusIndicator` |
| `SCROLLBAR_WIDTH`/`INSET`/`RADIUS`/`MIN_THUMB`（`theme.rs:130`–`:141`） | 6 / 4 / 3 / 8 | 照抄 feathers listview/scrollbar scene |
| `INPUT_PADDING_X`（`editor.rs:154`） | 3 | 照抄 `FeathersTextInputContainer` 的 padding |
| `ROW_HEIGHT` / `FONT_SIZE`（`transcript.rs:54`/`:55`） | 18 / 13 | 转写是行模型，不是 feathers 组件；内容行高自成一套 |

**内容色仍走 `Theme` 的 17 个语义 band**（`theme.rs:59` 起的 `UPSTREAM_BANDS`），不换成
feathers token：转写/diff/工具卡的颜色是 rig-tui 的语义，feathers token 管的是面板层级，
两套各管一层（见 `theme.rs:95`–`:101` 的注释）。batch 现在用 `upstream_theme(&Theme)`
（`batch.rs:628`/`:915`/`:1530`/`:1782`），不再是写死的 `RTheme::default()`，`NO_COLOR`
由 `Theme::from_env`（`theme.rs:420`）接进整条管线。

**还没迁移的直色**：

- `RF_NIRI_LAYOUT=1` 的 niri 默认屏幕仍用 `Look::new(Color, Color, radius)` 硬编码深底/亮边
  （`niri.rs:498`–`:522`）；默认屏幕走 `frame::default_screen_panels`，不受影响。等 L1 一起换。
- 透明叠加面板（picker 列表 `picker.rs:868`、测试）保持 `Color::NONE`。
- 输入框的焦点环是**换边框色**而不是 `Outline`（`editor.rs:1485`）：3D 镜像的边框更稳，
  且不会因为加环让行内容跳位。

## 4. 为什么不用 feathers 的现成组件

| feathers 组件 | 我们的替代 | 原因（可复现） |
|---|---|---|
| `controls::text_input::TextInput` | 保留多行 `Editor` 内核 + 输入框外壳 | `TextInput` 是单行、自己持有值/光标；我们需要折行、10 行滚动、CJK/emoji 光标、历史与内联作答（`editor.rs:667`/`:1447`），换掉等于重写编辑器。只借它的 token/常量 |
| `controls::button::Button` | `RowButton` 纯视觉 + 现有 observer/键盘 | `Button` 会装自己的 `Button`/`Hovered`/`Pressed`/`TabIndex`，与 picking、文本选区、Alt+1..9 冲突（`transcript.rs:62`–`:72`） |
| `controls::listview::ListView` | picker 自己的窗口化行 + 纯 Node 滚动条 | 列表是**窗口化**的（只生成可见行，滚动量在 `PickerInput::scroll`），`ListView` 依赖真实内容高度/`ScrollPosition`；换成它要重做过滤/树/多选/preview 的数据链 |
| `controls::scrollbar::Scrollbar` | 同 token 同几何的纯 Node 轨道+滑块 | feathers 的滑块 `ScrollbarThumb` **没有 `Node` 组件**，3D 镜像画不出来；`Scrollbar` 又绑 `ScrollPosition` + `ComputedNode`，我们没有那份高度（`picker.rs:208`–`:213`、`pane.rs:230`–`:238`） |
| `containers::{Pane, Subpane, Group}` | 自己的 `Look` + `PanelInset` | 面板根是 3D 镜像实体，不是 feathers scene；`Pane`/`Group` 是 `bsn!` 装配的 scene（`bevy_feathers-0.20.1/src/containers/pane.rs:22` 起），会把面板树绑定到 bevy_ui 层级。我们只取它们的 token 与 4px 圆角 |
| `controls::dialog` | 不用 | 闸门是条带里的确认 tile，没有弹层（`WORKSPACE_MODEL.md` §8） |

## 5. 验收（不求像素，求"是 feathers 的样子"）

- 面板/按钮/列表的尺寸、圆角、间距、颜色全部来自 `UiTheme`/token 或 §3 的命名常量：
  **测试里不出现硬编码 RGB/px**（断言"面板圆角 == `PANEL_RADIUS`"而不是 `== 8.0`）；
- F2/F3 的交互与换皮前等价（点击/键盘/Alt+1..9 都能选，`Row`/`Pick` 形状不变）；
- `cargo test --workspace --offline` 全绿：`rig_niri` **526**（F1–F5 基线；L1 并行时 528）+ app lib 20
  + app bin 5 + `feathers_in_3d` 5（2026-11-26 00:5x 实测）；
- 3D 镜像里不出现图标（镜像不支持 image），按钮一律文字。

## 6. 明确不做

- 不追终端格对齐、`keep`/`min_width` 的逐字节复刻、`status` 的阶段拆分、
  hints `fit_keys` 的裁剪细节（状态行/hints 现在**行为恰好已经对齐**——`hints.rs:73` 按上游
  的宽/权重规则、`status.rs:186` 直接调 `rig_tui::status::line`——但以后不把这些当验收项返工）；
- 不引入弹层/对话框（`feathers::controls::dialog` 不用，闸门是 tile）；
- **本阶段不做主题切换 UI**（`/theme`、浅色主题；`UiTheme` 已可换，见 `REMAINING.md` §11 F7）；
- 动画/补间不在本阶段（niri 的滚动插值另算，见 `NIRI_SCROLL.md` L5）；
- 不替换转写行模型、不重写多行编辑器、不给 picker 换 `ListView`（理由见 §4）；
- `RF_NIRI_LAYOUT=1` 的 niri 默认屏幕不在这五步里（仍是直色，等 L1）；
- 不做图标：3D 镜像不镜像 `ImageNode`，feathers 组件里的图标一律不显示，按钮只用文字。
