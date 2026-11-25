# 界面重组：改用 bevy_feathers 标准 UI（不再追求终端像素还原）

> **状态**：已定方向（用户 2026-11-26）。取代"rig-tui 像素级还原"的口径。
> **原话**：「用不着像素级还原，这样工作量太大，而 rig-tui 本来就只是终端 UI，可修改性很大，
> 用 feathers 的标准 UI 库来重新组装界面就行了，不然成本太大了。」

## 1. 口径

| 层 | 结论 |
|---|---|
| 行为/数据：agent、会话、工具卡、补全、历史、剪贴板、键位语义、选择 | **保留**（`rig_ecs` + `rig_harness` + `rig_niri` 的引擎），测试就是资产 |
| 视觉：面板外观、按钮、列表、输入框、状态行、滚动条、配色/圆角/间距 | **改用 `bevy_feathers` 的标准组件与 token**，不再逐像素对齐终端 |
| 布局 | niri 式（`NIRI_SCROLL.md`），列宽/滚动/聚焦 |
| 3D | `feathers_in_3d` 镜像 bevy_ui（已支持 Node 背景/边框/圆角 + 文字；**不**支持图标图片） |

**明确不做的**：终端格对齐、`keep/min_width` 的逐字节复刻、hints `fit_keys` 的上游怪癖、
`status` 的阶段拆分、任何"和终端像素一致"的验收。

## 2. 保留 vs 替换

| 现在 | 换成 |
|---|---|
| `PanelInset`/`Look` 手写边框、内边距、圆角、底色（`frame.rs`/`panels.rs`） | `bevy_feathers::containers::{Pane, Subpane, Group}` + `theme::UiTheme` token（`palette`/`tokens`/`rounded_corners`） |
| 行文本 + `Row::band` 手绘色带（`transcript.rs`） | 保留行模型（转写是文本流，没有 feathers 等价物），但颜色/字号/行高改用 feathers token |
| `Ask` 选项是"可点击的文本行"（`Row::choice`） | `feathers::controls::button::Button`（文字按钮，不用图标） |
| picker 是手绘行 + 点击（`picker.rs`） | `feathers::controls::listview`（选择行）+ `feathers::controls::scrollbar` |
| 输入框手绘边框/光标（`editor.rs`） | 外壳用 feathers 输入框的外观（圆角/焦点环/token）；**多行编辑内核保留**（feathers `TextInput` 是单行，不换） |
| 转写滚动只有 PgUp/PgDn/滚轮 | 加 `feathers::controls::scrollbar`，绑 `PaneScroll` |
| 状态行/提示行是字符串行（`status.rs`/`hints.rs`） | 保留字符串（信息密度高），外观用 token；不拆阶段 |
| `theme.rs` 的 17 个语义 band | 保留 band 语义，但**取值来自 feathers palette**，两套并存会让颜色打架 |

## 3. 落地顺序（每步独立提交）

| 步 | 内容 | 文件 | 工作量 |
|---|---|---|---|
| **F1** | 引入 feathers theme/token：面板底/边框/圆角/间距/字号/选中色全部走 token；删掉手写常量 | `frame.rs`、`panels.rs`、`transcript.rs`、`theme.rs` | S |
| **F2** | `Ask` 选项与确认 tile 改成 feathers `Button`（保留 `Row::choice` 数据，交互不变） | `panels.rs`、`batch.rs` 的 confirm 行、app 的 `answer_ask` 观察者 | M |
| **F3** | picker 用 `ListView` + `Scrollbar`（过滤/多选/preview 的**数据**不动） | `picker.rs` | M |
| **F4** | 转写加 `Scrollbar`（绑 `PaneScroll`） | `pane.rs`、`panels.rs` | S |
| **F5** | 输入框外壳换 feathers 外观（多行内核不动） | `editor.rs` | S |

顺序理由：F1 是纯视觉、零交互风险；F2/F3 动交互但数据模型不变；F4/F5 是补件。

## 4. 验收（不求像素，求"是 feathers 的样子"）

- 面板/按钮/列表的尺寸、圆角、间距、颜色全部来自 `UiTheme`/token：**测试里不出现硬编码 RGB/px**
  （可以断言"面板圆角 == token.radius"，而不是"== 8.0"）；
- F2/F3 的交互与现在等价（点击/键盘/Alt+1..9 都能选，行数据与 `Pick` 形状不变）；
- 现有 471+ 测试不因换皮而红；`cargo test --workspace --offline` 全绿；
- 3D 镜像里不出现图标（镜像不支持 image），按钮一律文字。

## 5. 明确不做

- 不引入弹层/对话框（`feathers::controls::dialog` 不用，我们的闸门是 tile）；
- 不做主题切换 UI（`/theme` 以后再说）；
- 不追 rig-tui 的 status 裁剪、hints fit_keys、终端格对齐；
- 动画/补间不在本阶段（niri 的滚动插值另算，见 `NIRI_SCROLL.md` L5）。
