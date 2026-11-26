# 界面重组：改用 bevy_feathers 标准 UI（不再追求终端像素还原）

> **状态（2026-11-26 核对）：F1–F5、F7、F8 全部完成；本文没有 F6（F6 是插件面板画布，
> 见 [`PANEL_CANVAS.md`](PANEL_CANVAS.md)，切片 1/2 已实现、切片 3 未做）。**
> 方向由用户 2026-11-26 定调，取代"rig-tui 像素级还原"的口径。
> **原话**：「用不着像素级还原，这样工作量太大，而 rig-tui 本来就只是终端 UI，可修改性很大，
> 用 feathers 的标准 UI 库来重新组装界面就行了，不然成本太大了。」
>
> | 项 | 状态 | 证据 |
> |---|---|---|
> | F1 面板 chrome token 化 | ✅ | `theme.rs:110`–`:148`、`frame.rs:996`/`:1032` |
> | F2 Ask/picker 行按钮化 | ✅ | `transcript.rs:105`/`:176`/`:211` |
> | F3 picker token + 纯 Node 滚动条 | ✅ | `picker.rs:880`/`:737`/`:1363` |
> | F4 转写滚动条（含拖拽） | ✅ | `pane.rs:249`/`:461`/`:531`/`:573`/`:596` |
> | F5 输入框外壳 | ✅ | `editor.rs:159`–`:181`、`:1638`/`:1701` |
> | F7 `/theme dark / light` | ✅（持久化未做） | `theme.rs:282`/`:309`；`theme_cmd.rs:47` |
> | F8 `ImageNode` 镜像 | ✅（SVG 未做） | `world3d.rs:186`/`:209` |
> | `NIRI_SCROLL` L1–L4 | ✅（L5 未做） | 见 [`NIRI_SCROLL.md`](NIRI_SCROLL.md) 顶部状态表 |
>
> **配套**：布局按 [`NIRI_SCROLL.md`](NIRI_SCROLL.md)（L1–L4 已实现，L5 未做）。
> **已知限制**：浅色主题可用，但 3D 小字号对比偏弱，默认仍是 dark，见 §5.5。
> 行号是 2026-11-26 的快照；函数名/常量名是稳定锚。
> 复跑基线：`cargo test --workspace --offline` → `rig_niri` **606**、app lib **36**、
> app bin **5**、`feathers_in_3d` **15**、doctests 3（2026-11-26 实测，全绿）。

## 1. 口径

| 层 | 结论 |
|---|---|
| 行为/数据：agent、会话、工具卡、补全、历史、剪贴板、键位语义、选择 | **保留**（`rig_ecs` + `rig_harness` + `rig_niri` 的引擎），测试就是资产 |
| 视觉：面板外观、按钮、列表、输入框、状态行、滚动条、配色/圆角/间距 | **改用 `bevy_feathers` 的标准组件与 token**，不再逐像素对齐终端 |
| 布局 | niri 式（`NIRI_SCROLL.md`），列宽/滚动/聚焦；**L1–L4 已实现**，L5（插值/居中）未做 |
| 3D | `feathers_in_3d` 镜像 bevy_ui（Node 背景/边框/圆角 + 文字 + **Outline** + **`ImageNode`**；SVG 图标未做） |

**明确不做的**：终端格对齐、`keep/min_width` 的逐字节复刻、任何"和终端像素一致"的验收；
hints `fit_keys` 的裁剪细节与 `status` 的阶段拆分也不再作为验收项（代码现在恰好对齐，但不为它返工）。
具体见 §6。

## 2. 已完成：F1–F5 + F7/F8 实际用了什么

| 步 | 结果 | 主要落点 |
|---|---|---|
| **F1** | 面板 chrome 全 token 化 | `theme.rs` 的 token 常量（`:110`–`:148`）、`frame::Look::panel`（`frame.rs:996`）、`sync_panel_chrome`（`frame.rs:1032`）；测试 `frame.rs:2131`/`:2199`/`:2230`、`theme.rs` 的浅色/暗色 token 测试 |
| **F2** | Ask/picker 行按钮化 | `transcript.rs::RowButton`（`:105`）、`update_row_button_styles`（`:176`）、`update_row_focus_rings`（`:211`）；测试 `transcript.rs:1852`/`:1911`/`:1938` |
| **F3** | picker 面板 token 化 + 纯 Node 滚动条 | `picker.rs:880`（容器 `Look::panel`）、`spawn_scrollbar`（`:737`）、`sync_picker_scrollbar`（`:1363`）；测试 `picker.rs:3330`/`:3503` |
| **F4** | 转写滚动条（含拖拽） | `pane.rs::ScrollbarModel`（`:249`）、`sync_transcript_scrollbars`（`:461`）、拖拽观察者（`:531`/`:573`/`:596`）；测试 `pane.rs:3235`/`:3338`/`:3363`/`:3423` |
| **F5** | 输入框外壳换 feathers 外观 | `editor.rs::sync_input_field`（`:1638`）、`sync_input_row_height`（`:1701`）、常量 `:159`–`:181`；测试 `editor.rs:4010`/`:4121`/`:4185` |
| **F7** | dark/light 主题切换 | `theme.rs::ThemeKind`（`:173`）/`create_light_theme`（`:309`）/`install_theme`（`:282`）；app 侧 `theme_cmd` 的 `/theme`（`theme_cmd.rs:47`）、`RF_THEME`；测试 `theme.rs`、`theme_cmd.rs`、`lib.rs:257` |
| **F8** | `ImageNode` 镜像 | `world3d.rs::PanelImageNode`（`:186`）、`image_uv_rect`（`:209`）；测试 `world3d.rs:1114` 起 |
| 附加 | `feathers_in_3d` 支持 `Outline` 镜像（焦点环） | `world3d.rs::PanelOutlineNode`（`:154`） |

### 2.1 F1：面板 chrome 的 token

`theme.rs` 把 feathers 的颜色 token 收成命名常量（面板/细条/输入/确认各有底色）：

| 常量 | feathers token | 用在哪 |
|---|---|---|
| `PANEL_BG`（`theme.rs:110`） | `PANE_BODY_BG` | 转写、`Pane2`（`frame.rs:1118`） |
| `BAR_BG`（`:112`） | `PANE_HEADER_BG` | 状态行、键位提示、补全 tile |
| `INPUT_BG`（`:114`） | `TEXT_INPUT_BG` | 输入框 |
| `CONFIRM_BG`（`:116`） | `SUBPANE_HEADER_BG` | 确认 tile |
| `PANEL_BORDER`（`:118`） | `SUBPANE_BODY_BORDER` | 所有面板边框 |
| `FOCUS_BORDER`（`:120`） | `FOCUS_RING` | 输入框焦点态 |

- 面板根挂 `ThemeBackgroundColor`/`ThemeBorderColor`（`frame.rs:1097`–`:1101`），
  `sync_panel_chrome` 每帧按 `UiTheme` 解析成真实颜色（没有资源时用 feathers 暗色主题兜底，
  值不变不写组件）。`FeathersViewPlugin` 只在 `UiTheme` 缺失/为空时插暗色主题
  （`lib.rs:166`–`:169`），不覆盖调用方配置。
- 默认屏幕的六块面板全部 `Look::panel(...)`（`frame.rs:845`–`:882`）。

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
  （`transcript.rs:1852`/`:1911`/`:1938`）。

### 2.3 F3：picker 的 token 与滚动条

- 容器与 preview 面板从硬编码深蓝/黄边换成 `Look::panel(PANEL_BG, PANEL_BORDER)`
  （`picker.rs:880`/`:999`）；列表 tile 仍然是透明 `Look::new(Color::NONE, ...)`（`:887`），
  底色由容器给。
- 滚动条是**纯 `Node`**：轨道 6px、离右缘 4px、圆角 3px、滑块最短 8px
  （`theme.rs:142`–`:148`），颜色取 feathers `SCROLLBAR_BG`/`SCROLLBAR_THUMB`
  （`picker.rs:738`–`:739`）。滑块长度/位置按 `PickerInput::scroll` 与可见行算，
  支持滚轮与拖滑块，PgUp/PgDn/↑↓ 同帧跟随，内容放得下时收起
  （`sync_picker_scrollbar` `:1363`）。

### 2.4 F4：转写滚动条

- 每个 `TranscriptPane`（主转写与 `Pane2`）挂轨道 + 滑块，观察者自动覆盖拆出来的窗格
  （`pane.rs:397`–`:404` 的 `Add<TranscriptPane>` 观察者）。
- `ScrollbarModel` 按 总行/可见行/`top`/`follow` 算（`:249`）；拖拽观察者
  `PointerDragStart/Drag/End` 改 `top` 并夹紧，拖到底恢复 follow（`:531`–`:599`）；
  滚轮落在滑块上归它服务的窗格。
- 滚动条不改 `↓ more below` / `↓ new output` 的判定（测试 `pane.rs:3423`）。

### 2.5 F5：输入框外壳

- 底色 `TEXT_INPUT_BG`、圆角 4（`editor.rs:173`）、左右内边距 3px（`:159`）、行高
  `feathers::constants::size::ROW_HEIGHT` = 24（`:171`）、字号 `COMPACT_FONT` = 13（`:168`）。
- 焦点在输入框时边框用 `FOCUS_RING`，焦点在别的面板时用 `SUBPANE_BODY_BORDER`
  （`sync_input_field` `:1638`）；每个工作区只有聚焦的那个是焦点态。
- 多行仍是**一块框**：`sync_input_row_height`（`:1701`）只把输入行的高度换成 24px，
  编辑器行模型不动；空输入保底 3 行 = 72px，10 行上限 260px（`input_tile_height` `:181`）。

### 2.6 F8：`ImageNode` 镜像（SVG 图标未做）

`feathers_in_3d` 现在把 `bevy_ui::widget::ImageNode` 镜像成一张**贴图四边形**
（`world3d::PanelImageNode`），feathers 控件里的勾/展开箭头/icon button 和转写里贴的截图
在 3D 里都能画出来：

- **独立 quad**：图片不是塞进节点自己的填充材质，而是节点下另挂一张同尺寸 quad。这样
  "透明底图标按钮"和"有底色行里的截图"都正确；quad 继承节点的 Transform，材质复用
  `UNodeMaterial3d` 的 `texture` + 新增的 `uv_rect`，并和填充/Outline/文字共用同一套
  `clip_*` uniform——`WorldPanel::clip`（工作区条带视口）与面板自身裁剪对图片同样生效。
- **UV/flip**：`flip_x`/`flip_y` 只交换 `uv_rect` 的 U/V 两端（`(1,0,0,1)` / `(0,1,1,0)`），
  shader 做 `mix(xy, zw, body_uv)`，几何（SDF）不受影响；`ImageNode::rect` 按纹理像素
  归一到 0..1，可裁子图。
- **tint**：`ImageNode::color` 转线性后作为材质 `color`，shader 里与 texel 逐分量相乘。
- **加载**：`Assets<Image>` 里还没有的图片先跳过；加载后的下一帧 `sync_mirror` 检查到就会
  补上（`sync_mirror` 每帧跑，不需要 AssetEvent）。资产卸载、`ImageNode` 移除、节点被裁掉
  或尺寸为 0 时 quad 收起；零尺寸/离屏不生成、不 panic。
- **层级**：文字仍由 `text3d` 按面板批量画（在整块面板之后），所以同一节点里**文字总在
  图片之上**；"图片盖在文字上"这种真正按 UI 树的交错需要把字形拆到节点层，本轮没做。
- **未覆盖**：`ImageNode::image_mode`/`visual_box` 的 fit/cover/content-box 规则没抄，
  图片按节点世界矩形拉伸（图标/截图通常节点尺寸就是图片尺寸，够用）；`texture_atlas`
  没接（`ImageNode::rect` 子图已支持）。
- 测试：`feathers_in_3d` **9 → 15**（UV/flip/rect 纯函数、tint 乘法、空矩形/零尺寸/
  离屏、资产缺失→加载→卸载的 ECS 用例）。

**SVG 图标未做**：SVG 需要 `bevy_ui` 的 `svg` feature（当前 `bevy` 依赖没开；注意本仓锁定
的 bevy 0.20.1 的 `bevy_ui/Cargo.toml` 里没有 `svg` feature，真要支持得先确认上游的
SVG loader/feature 形态）。本轮只处理已栅格化、在 `Assets<Image>` 里的 `Handle<Image>`。

### 2.7 F7：主题切换（dark / light）

**主题切换已做，持久化未做。** 面板 chrome 与内容色是两套资源，所以切换一次换两个：

- `ThemeKind { Dark, Light }` 是唯一入口；`Theme::light()` 给 17 个语义 band 的浅色版
  （浅底 + 深色前景 token），`create_light_theme()` 从 `create_dark_theme()` 的 token
  分配表换语义色：表面 = `WHITE`/`LIGHT_GRAY_MIX`/`LIGHT_GRAY_1`，文字 =
  `GRAY_0`/`GRAY_2`，边框 = `GRAY_2`；并把暗色主题里指向 `TEXT_ON_ACCENT`（白）的
  `BUTTON_TEXT`/`TEXT_INPUT_TEXT`/`LISTROW_TEXT`/`MENUITEM_TEXT` 改回 `TEXT_DEFAULT`，
  否则浅底白字。
- `install_theme(app, kind)` 是启动装配：`UiTheme` 缺失/为空时插 `kind` 的 chrome
  （调用方自配的不覆盖），记录 `ThemeKind`，并插入 `kind` 的 `Theme`（`NO_COLOR` 仍生效）。
  `FeathersViewPlugin` 用 `ThemeKind::from_env()` 调它，即 `RF_THEME=light`；
  和 `RF_FRAME` 一样，坏值警告后回退 `dark`。
- `/theme`（`rig_harness_in_3d/src/theme_cmd.rs`，由 `HarnessExtPlugin` 注册）：
  无参数报当前值与可选值；`/theme dark` / `/theme light` 同时
  `insert_resource(Theme::from_kind(..))` 与 `insert_resource(UiTheme(kind.props()))`，
  下一帧 `frame::sync_panel_chrome` / `sync_picker_scrollbar` / `sync_input_field` 与
  `batch` 的 `upstream_theme` 都按新值解析；非法参数写一条 warn notice，不改主题。
- 测试：`Theme::light` 的 17 band 亮度方向/对比、`create_light_theme` 的
  `SURFACE_*`/`TEXT_DEFAULT`/`BORDER`、`/theme` 双向切换与非法参数、`install_theme`
  的 light 启动（都断言资源/token，不跑 GUI）。

**未做**：`[view] theme` 持久化（下一轮，`Settings`/`add_setting` 已有先例）、
高对比/自定义主题、图标色（`feathers_in_3d` 的图标仍取图像自身颜色）。

## 3. 仍是常量 / 直接颜色的地方

feathers 只有**颜色**是 design token；间距、边框宽度、圆角、字号/行高在
`bevy_feathers::constants` 里是普通常量。所以下面这些是本仓命名的常量，不是漏网的硬编码：

| 常量 | 值 | 为什么还是常量 |
|---|---|---|
| `PANEL_PADDING` / `PANEL_BORDER`（`frame.rs:62`/`:63`） | 8 / 2 | feathers 没有 spacing token；对齐 `containers::pane` 的观感 |
| `THIN_PADDING` / `THIN_BORDER`（`frame.rs:66`/`:67`） | 4 / 1 | 同上，细条（状态/提示） |
| `PANEL_RADIUS`（`theme.rs:128`） | 4 | feathers 圆角不是 token；对齐 pane/group/text input 的 4px |
| `ROW_BUTTON_RADIUS`/`PADDING`/`ROW_FOCUS_WIDTH`（`transcript.rs:84`–`:88`） | 4 / 8 / 2 | 照抄 Button scene 与 `FocusIndicator` |
| `SCROLLBAR_WIDTH`/`INSET`/`RADIUS`/`MIN_THUMB`（`theme.rs:142`–`:148`） | 6 / 4 / 3 / 8 | 照抄 feathers listview/scrollbar scene |
| `INPUT_PADDING_X`（`editor.rs:159`） | 3 | 照抄 `FeathersTextInputContainer` 的 padding |
| `ROW_HEIGHT` / `FONT_SIZE`（`transcript.rs:54`/`:55`） | 18 / 13 | 转写是行模型，不是 feathers 组件；内容行高自成一套 |

**内容色仍走 `Theme` 的 17 个语义 band**（`theme.rs:71` 起的 `UPSTREAM_BANDS`），不换成
feathers token：转写/diff/工具卡的颜色是 rig-tui 的语义，feathers token 管的是面板层级，
两套各管一层（见 `theme.rs` 里 `PANEL_*` 常量上的注释）。batch 现在用 `upstream_theme(&Theme)`
（`batch.rs:630`/`:935`/`:1550`/`:1817`），不再是写死的 `RTheme::default()`，`NO_COLOR`
由 `Theme::from_env`（`theme.rs:727`）接进整条管线。

**还没迁移的直色**：

- `RF_NIRI_LAYOUT=1` 的旧 `niri::spawn_niri_screen` 仍用 `Look::new(Color, Color, radius)`
  硬编码深底/亮边（`niri.rs:498`–`:536`）。这是**旧演示**，不是默认屏幕；默认屏幕走
  `frame::default_screen_panels` + `layout::default_tree`，F1 已 token 化。没有"等 L1 再换"
  这件事——L1–L4 已经实现，旧演示不在当前路线里。
- 透明叠加面板（picker 列表 `picker.rs:887`、测试）保持 `Color::NONE`。
- 输入框的焦点环是**换边框色**而不是 `Outline`（`sync_input_field`，`editor.rs:1638` 起，
  颜色分支 `:1682`）：3D 镜像的边框更稳，且不会因为加环让行内容跳位。

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
- `cargo test --workspace --offline` 全绿：`rig_niri` **606** + app lib **36**
  + app bin **5** + `feathers_in_3d` **15** + doctests **3**（2026-11-26 实测）；
- 3D 镜像支持 `ImageNode`（图标/截图，见 §2.6）；**SVG 图标仍未做**（当前依赖没开对应 feature）。

## 5.5 浅色主题对比度限制（已知限制）

`/theme light` 功能可用（`UiTheme` + 17 band 一起换、面板/按钮/滚动条都跟着走），但**3D 里
小字号对比偏弱**：3D 文本是字形网格，13px 在 3D 缩放后笔画覆盖约 60%，深色在浅底上会糊成
中灰；深色主题"亮字暗底"看起来更清楚。**这是浅色主题的对比度限制，不是 token 配错**；
当前默认仍是 dark。改进方向（未做）：浅色主题下加大字号 / 换 bold 字面、
或给文本材质单独做不透明处理。

## 6. 明确不做

- 不追终端格对齐、`keep`/`min_width` 的逐字节复刻、`status` 的阶段拆分、
  hints `fit_keys` 的裁剪细节（状态行/hints 现在**行为恰好已经对齐**——`hints.rs:73` 按上游
  的宽/权重规则、`status.rs:186` 直接调 `rig_tui::status::line`——但以后不把这些当验收项返工）；
- 不引入弹层/对话框（`feathers::controls::dialog` 不用，闸门是 tile）；
- **主题切换已做（F7）**：`/theme dark|light` + `RF_THEME`，见 §2.7；**持久化
  （`[view] theme`）与高对比/自定义主题仍未做**（`REMAINING.md` §11 F7）；
- 动画/补间不在本阶段（niri 的滚动插值另算，见 `NIRI_SCROLL.md` L5）；
- 不替换转写行模型、不重写多行编辑器、不给 picker 换 `ListView`（理由见 §4）；
- `RF_NIRI_LAYOUT=1` 的旧 `niri.rs` 演示不在这五步里（仍是直色；它不是默认屏幕，也不在当前路线上）；
- 不做 SVG 图标：`ImageNode` 已镜像（§2.6），但 SVG 需要 `bevy_ui` 的 `svg` feature
  （当前没开），feathers 的 SVG 图标仍不显示；位图图标/截图可以。
