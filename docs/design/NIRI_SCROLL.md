# 布局优化：niri 式滚动平铺（横向无限延伸）

> **状态（2026-11-26 核对）**：**L1–L4 已实现，L5 未做**。
>
> | 步 | 状态 | 证据 |
> |---|---|---|
> | L1 列宽模型 | ✅ 已实现 | `layout.rs:99`（`Size::Fraction`）、`layout.rs:739`（`append_column`）、`layout.rs:1745`/`:1782`（不变量测试） |
> | L2 滚动 + 视口裁剪 | ✅ 已实现 | `workspaces.rs:110`（`WorkspaceScroll`）、`layout.rs:302`（`placements_scrolled`）、`frame.rs:674`（`workspace_viewport`）、`world3d.rs:79`（`effective_clip`） |
> | L3 聚焦/滚动键位 | ✅ 已实现 | `workspaces.rs:406`（`scroll_active_column`）、`workspaces.rs:490`（`scroll_workspace_keys`）、`:546`（`scroll_workspace_wheel`） |
> | L4 列宽预设/移动列 | ✅ 已实现 | `layout.rs:912`（`set_column_fraction`）、`layout.rs:944`（`move_column`）、`workspaces.rs:630`（`column_workspace_keys`） |
> | L5 scroll 插值 / `center-focused-column` | ⬜ 未实现 | 全仓无 `WorkspaceScroll` 的补间；`scroll` 当帧写定。`niri.rs` 的 `view_offset` 是 `RF_NIRI_LAYOUT` 旧演示，不是这套实现 |
>
> **依据**：`reference/niri/niri-main/docs/wiki/Development:-Design-Principles.md`
> （三条基本原则）、`Configuration:-Layout.md`（列宽/间隙/居中策略）。
> **对象**：`rig_niri` 的条带内部布局（`layout.rs` / `frame.rs` / `panels.rs` /
> `workspaces.rs`）。工作区层面的模型（一条工作区 = 一条条带、竖直堆叠、绝对空间
> 坐标）不变。
>
> **别和 `RF_NIRI_LAYOUT=1` 的旧演示混了**：`rig_niri/src/niri.rs` 的
> `Tiling`/`Layer`/`Column`/`view_offset` 是更早的独立 3D 列带演示（`run.sh niri`），
> 默认屏幕走的是 `layout::default_tree` + `workspaces`，即本文这套。旧演示既没有
> `WorkspaceScroll`，也没有按工作区裁剪；不要用它的行为判断 L1–L4 是否实现。

---

## 0. 先划清"哪些是照搬、哪些是我们自己的"（三层边界）

"移植"这个词被用得含糊过，这里钉死——**像素级只对第 1 层成立**：

| 层 | 谁说了算 | 判据 | 是不是"像素级还原" |
|---|---|---|---|
| **1. 内容**：一行里每个字符、span 颜色、底色/下划线、折行、状态行条目、hints 文案、picker 行 | `rig-tui` | 同一 terminal 宽度下，我们的行文本与颜色逐项对得上 | ✅ 是。这层错了就是移植错了 |
| **2. 面板**：转写/输入/状态/提示/确认/补全/picker 各自多大、放哪、怎么切分 | 我们（3D 化设计） | 上游用固定 `Placement`（转写左上、输入右下、状态底行）；我们换成分割树 → 现在换 niri 列/滚动 | ❌ 不是。上游的"位置"没有 3D 对应物 |
| **3. 工作区/条带**：一条会话一个条带、绝对空间坐标、竖直堆叠、拖拽/整理 | 我们（用户定的模型） | `WORKSPACE_MODEL.md` | ❌ 不是。上游一个终端只有一个屏 |

所以两件事并不冲突：

- **niri 管第 2 层的横向组织**（列宽、滚动、聚焦列）；
- **rig-tui 管第 1 层的绘制**（同样的行、同样的颜色、同样的折行）。

本文里的"逐字节相同"全部是**第 2 层我们自己的回归不变量**（chrome 的 rect 在 L1–L5
前后不变），不是"与上游终端一致"。之前说"S1 画面一个像素不变"也是这个意思：
**S1 前后我们自己不变**，不是"和 rig-tui 一样"。

如果哪天要求第 2 层也照搬上游，那 niri 滚动就不该做——二者只能选一个。
按用户 2026-11-25 的指示（"布局优化完全按 niri 的滚动平铺哲学"），选的是：
**第 1 层照搬 + 第 2/3 层 niri**。

---

## 1. 当初错在哪（L1 已把根因改掉）

niri 的第一原则（`Development:-Design-Principles.md:7`）：

> Opening a new window should not affect the sizes of any existing windows.

本文 2026-11-25 落笔时，布局是**分割树按比例分剩余空间**：

- `Size::Proportion(w)` 是"同层比例"（权重 / 同层权重和）。开新列时 `append_column`
  把新列设成 1/3、已有列按同一因子**等比缩小**——正是 niri 要避免的行为。
- 条带内容宽度恒等于 `FrameSize.cols`（`frame.rs` 的 `screen()` → `TileRect`），没有
  "内容比视口宽、横向滚动"的概念。

**现在（L1–L4 实现后）**：

- 水平列用 `Size::Fraction(f)`（`layout.rs:99`–`:111`）：长度 = **区域（视口）宽 × f**，
  不参与 leftover 分配、不随兄弟数量变化（`layout.rs:447`–`:489`、测试 `:1745`/`:1782`）。
  默认树的主体行就是 `Transcript Fraction 0.62 | 右列 Fraction 0.38`（`layout.rs:1137`–`:1156`）。
- `append_column` 用 `DEFAULT_COLUMN_FRACTION = 0.5`（`layout.rs:71`、`:739`）：新列接在
  条带末尾，已有列的宽度和 x 一个字节不改（测试 `layout.rs:1745`）。
- 内容可以比视口宽：`walk` 按累计 x 摆列、允许超出右边界（`layout.rs:517`–`:560`），
  `placements_scrolled`（`layout.rs:302`）再把条带里的列统一左移。
- `Size::Proportion` **没有删**，但只服务竖直方向/旧树的高度分配；旧的"同层份额"语义
  仅保留在竖直分割里（`layout.rs:17`–`:19`）。

---

## 2. 目标模型（照 niri）

```
一条工作区条带 = 一个视口（viewport，宽 = FrameSize.cols 格）
条带内容     = 一串列（columns），每列有**自己的宽度**，从 x=0 向右无限排
视口         = 内容的 [scroll, scroll + viewport) 窗口，scroll 可移动
列内         = 竖直分割树（现在的 Split::Vertical 直接沿用）
```

四条不变量（均已落地；测试锚点见 §5 验收）：

1. **开新列不改已有列宽度**：新列取 `DEFAULT_COLUMN_FRACTION`（niri 默认
   `proportion 0.5`，即视口一半），已有列的宽度、x 坐标一个字节都不变。
2. **列宽是视口的比例/固定格数，不是"同层兄弟的份额"**：`0.5` 永远是视口的一半，
   不随兄弟数量变化。预设宽度照 niri：`1/3`、`1/2`、`2/3`（`Configuration:-Layout.md:17-19`）。
3. **聚焦列自动滚进视口**，且只移动 `scroll`，不改任何列的位置/宽度。
4. **动作立即生效**：加列、滚、改列宽、关列都是当帧改完（不做补间；补间最多是
   `scroll` 的视觉插值，见 §5 L5）。

### 2.1 条带里的一切都滚（包括输入/确认）——已实现

niri 里没有"常驻输入框"，一切皆列。我们一开始想给聊天留个例外（把输入/状态/提示/确认钉在
视口右缘），实现后发现两处坏处：追加的 picker 列会落在右列**之后**、把右列挤出视口；
要真正钉住就得把条带拆成"可滚动子区域 + 固定 chrome"两个容器，属于结构改动。

**最终决定（与 niri 一致，已按此实现）**：

- `placements_scrolled`（`layout.rs:302`–`:350`）按条带的纵向范围筛面板，范围内的
  **所有**列（转写、picker/插件列、右列的输入/确认/补全）一起左移 `scroll` 像素；
- Hints/Status 是根竖 split 的兄弟、在条带纵向范围之外，不移动（`layout.rs:337` 一带）；
- 唯一的固定元素是**横跨整条的两行**：Hints 与 Status——它们在根竖 split 里、条带的纵向
  范围之外，本来就不属于滚动内容；
- 于是打开 picker 时输入框会跟着左移（仍在视口内、仍能打字），关掉后跟着滚回来。这不是
  缺陷，是"输入框也是一列"的直接结果；如果以后要钉住它，需要按上面的结构改动重做
  （`docs/status/REMAINING.md` 的 F6 面板画布是同一件事的地基）。
- 注意：该函数的 doc 注释开头仍写着"右列是固定 chrome、不参与滚动"（`layout.rs:293`–`:301`），
  与同函数后半句（`:320`–`:323`）和实际循环（`:330`–`:338`）不一致；**以代码为准**，输入/确认确实跟着滚。

---

## 3. 数据模型（实际改成了什么样）

> 与下面的原始设计有偏差，以本节为准：**没有新增 `Strip`/`Column` 结构，也没有
> `FrameRects`**；列宽挂在现有树的 `Size::Fraction` 上，滚动挂在每个工作区的
> `WorkspaceScroll` 组件上。

### 3.1 `rig_niri/src/layout.rs`（已实现）

- `Size::Fraction(f) = 视口宽 × f`（`layout.rs:99`–`:111`）。旧的 `Size::Proportion`
  **保留**，只用于竖直方向/旧树的高度分配；`Size::Share` 从未加过。
- 列 = 包含转写/活动面板的最外层横向 split 的直接子节点；列宽预设常量
  `PRESET_COLUMN_FRACTIONS = [1/3, 1/2, 2/3]`（`layout.rs:86`）。
- `rects` 的列 x = 前面所有列宽之和（`walk`，`layout.rs:517`–`:560`），列可以部分/
  完全在视口外；`strip_area`（`layout.rs:285`）给出"包含锚点的最外层横向 split"。
- `append_column`（`layout.rs:739`）就是 `push(Child::fraction(0.5, node))`，**不动已有列**；
  `remove_column`（`layout.rs:827`）摘掉整列，剩余列宽不变（`Fraction` 不重算）。
- 列宽/移动列：`column_fraction`（`layout.rs:899`）、`set_column_fraction`（`:912`）、
  `move_column`（`:944`）、`next_preset_fraction`/`previous_preset_fraction`
  （`:1041`/`:1055`）。`set_column_fraction` 拒绝含嵌套 `Fraction` 的列（`:920` 一带）。
- 滚动应用：`placements_scrolled`（`:302`）/`rects_scrolled`（`:362`）把条带里所有列
  左移 `scroll` 像素；面板尺寸不改，只改中心 x。

### 3.2 `rig_niri/src/frame.rs`（已实现，形式与原文不同）

- 没有新增 `FrameRects { rects, clip }`。视口就是条带矩形：
  `layout::Node::strip_area`（`layout.rs:285`）在布局时记录；
  `lay_out_frame`（`frame.rs:593`）读该工作区的 `WorkspaceScroll`；
- `workspace_viewport`（`frame.rs:674`）把帧的四个角经工作区 `pose` 变换成世界 AABB；
  `place_panels`（`frame.rs:691` 起）把它写进每块面板的 `WorldPanel::clip`
  （`frame.rs:761`），**所有**面板都写（交集等于自身时无视觉变化，chrome 也不会画出视口）。
- 没有工作区的旧单屏路径把 `clip` 清成 `None`（`frame.rs:805`），保持旧行为逐字节不变。

### 3.3 `rig_niri/src/panels.rs`（没改）

`PanelRows` 仍按 `PanelId` 分桶（`panels.rs:123`）；列滚动只影响面板 rect，不影响行内容，
所以 **L1–L4 没有改这个文件**。后来 F6 给 `PanelId` 加了 `Plugin` 变体（`panels.rs:100`），
那是插件面板的事（见 `PANEL_CANVAS.md`），不影响列滚动。

### 3.4 `feathers_in_3d`（已实现）

`WorldPanel` 上有 `clip: Option<Rect>`（`world3d.rs:59`）；有效裁剪 =
面板矩形 ∩ 条带视口（`effective_clip`，`world3d.rs:79`，`panel_clip_uniform` 写进 shader
`clip_*` uniform）。比原文更广：**不是只有转写列挂 clip**，工作区里所有面板都挂
（`frame.rs:755`–`:766`），因为跟随滚动的右列也需要它。

### 3.5 `rig_niri/src/workspaces.rs`（已实现）

- 条带的世界位姿不变（绝对坐标、竖直堆叠）；
- 每个工作区各自持有 `WorkspaceScroll { scroll: f32 }`（UI 像素，`workspaces.rs:110`），
  互不影响；`lay_out_frame` 按工作区取各自的 scroll（`frame.rs:593` 的参数）。
- 滚动只写 `WorkspaceScroll`：`clamp_scroll`（`:134`）、`scroll_to_visible`（`:154`）、
  `ColumnBounds`（`:184`）、`scroll_by`（`:211`）、`column_bounds`（`:263`）。
- 聚焦/滚动系统：`scroll_active_column`（`:406`）、`scroll_workspace_keys`（`:490`）、
  `scroll_workspace_wheel`（`:546`）、`column_workspace_keys`（`:630`）。

## 4. 键位/交互（已实现的实际键位）

原文建议的 `Alt+←/→`、`Alt+Shift+←/→`、`Alt+Ctrl+←/→` **没有采用**：
`Alt+←/→` 被内置的 `WordLeft`/`WordRight` 占用（`keys.rs:738`–`:748` 的注释），
`Ctrl+Alt+←/→` 又会被 picker 的方向键顺带展开/收起说明。实际绑定如下：

| 动作 | 实际键位/入口 | 语义 | 证据 |
|---|---|---|---|
| 横向滚动 | `Ctrl+Alt+H/L` 一屏；`Ctrl+Alt+Home/End` 到最左/最右；`Shift+滚轮` 或横向滚轮 | 只改 `scroll`，夹进 `[0, content - viewport]` | `workspaces.rs:490`、`:546`；`keys.rs:752`–`:762` |
| 聚焦上一/下一列 | **没有独立动作**。活动列由 picker / `ActivePanelColumn` / `ActiveTranscript` 决定；活动列变化时自动 `scroll_to_visible` | 聚焦列必须滚进视口 | `workspaces.rs:406`–`:487`、`column_bounds` `:263` |
| 新列（开列） | picker 打开时追加一列（`append_column`）；插件面板用 `add_panel_column`；`Alt+S` 仍是"拆转写窗格"（在转写列**内部**横拆出 `Pane2`） | 已有列宽度不变 | `layout.rs:739`、`panel.rs:271`、`frame.rs:1136` |
| 关列 | picker 关闭 `remove_column`；插件面板 `remove_panel_column` / `unregister_panel` | scroll 夹紧，聚焦列不移动 | `layout.rs:827`、`panel.rs:440`/`:541` |
| 改列宽 | `Alt+Shift+H/L`（H 变窄、L 变宽），按 `1/3 → 1/2 → 2/3` 循环/回绕 | 只改活动列的 `Fraction`，不挤动别人；嵌套 `Fraction` 列拒绝 | `workspaces.rs:630`–`:722`；`keys.rs:788`–`:802` |
| 移动列 | `Ctrl+Alt+Shift+H/L` | 整列（宽度 + 子树）与邻列交换，到边界不动 | `layout.rs:944`、`workspaces.rs:630` |
| 居中策略 | **未实现**，行为固定为 `never` | 聚焦列滚进视口即可 | L5；见 §5 |

**不做的**：niri 的 overview / 手势 / 动画系统；不引入弹层或总览几何（用户早前明确否掉）。
滚动的视觉插值属于 L5，也未做。

## 5. 实施拆分（最终状态）

| 步 | 内容 | 文件 | 状态 | 落地证据 |
|---|---|---|---|---|
| **L1** | `Size::Fraction` 视口比例、累计 x 摆列；旧分割树只服务列内竖直分割 | `layout.rs` | ✅ 已实现 | `layout.rs:99`–`:111`、`:447`–`:560`、`:739`；测试 `:1745`/`:1782` |
| **L2** | `scroll` + 视口裁剪：`strip_area` / `placements_scrolled` + `WorldPanel::clip` | `layout.rs`、`frame.rs`、`world3d.rs` | ✅ 已实现 | `layout.rs:285`/`:302`、`frame.rs:674`/`:761`、`world3d.rs:59`/`:79`；测试 `workspaces.rs:3133`（滚动不动 chrome）/`:3201`（无 scroll 时面板只裁自身） |
| **L3** | 滚动动作：`scroll_to_visible`、`Ctrl+Alt+H/L`、滚轮 | `workspaces.rs`、`keys.rs` | ✅ 已实现 | `workspaces.rs:110`/`:134`/`:154`/`:184`/`:211`/`:263`/`:406`/`:490`/`:546`；测试 `:3095` 起 |
| **L4** | 列宽预设与移动列：`Alt+Shift+H/L`、`Ctrl+Alt+Shift+H/L` | `layout.rs`、`workspaces.rs`、`keys.rs` | ✅ 已实现 | `layout.rs:86`/`:912`/`:944`/`:1041`/`:1055`；`workspaces.rs:630`；测试 `:3805` 一带 |
| **L5** | （可选）`scroll` 的视觉插值 + `center-focused-column on-overflow` | `pane.rs`/`camera` | ⬜ 未实现 | 没有对 `WorkspaceScroll` 做补间；`center-focused-column` 没有对应字段/分支。`niri.rs::Tiling::view_offset` 的平滑动画属于 `RF_NIRI_LAYOUT` 旧演示，不能算 L5 |

**L5 与当前实现的差距（写清楚，别误读）**：滚动是"当帧跳到位"，没有插值动画；
`center-focused-column` 只有 `never` 的语义（`scroll_to_visible` 左边优先，见
`workspaces.rs:154`–`:180`），不居中。要做的话只需在渲染/相机层插值 `scroll`，不
需要改列几何。

### 验收（全部无窗口测试，不做视觉研究）

以下各条都已有测试；行号是 2026-11-26 快照：

- **不变量 1**：开列前后已有列 `(x, width)` 逐字节不变，新列接在旧总宽之后
  （`layout.rs:1745` `appending_a_column_keeps_existing_columns_and_overflows_the_area`）；
- **不变量 2**：同一列在兄弟数 2→3 时宽度不变
  （`layout.rs:1782` `a_fraction_column_keeps_its_viewport_width_when_siblings_grow`）；
- **不变量 3**：聚焦列滚进视口（`workspaces.rs:3097` `scroll_helpers_clamp_to_the_valid_range`、
  `:3442` `opening_an_off_screen_picker_scrolls_it_in_and_closing_scrolls_back`）；
- **不变量 4**：动作当帧生效（`workspaces.rs:3856`
  `keyboard_width_keys_resize_the_transcript_column_in_the_same_frame`、`:3792`
  `keyboard_width_presets_cycle_on_the_active_column_only`、`:3901`
  `keyboard_move_swaps_the_active_column_with_its_neighbour`）；
- 裁剪：列完全在视口外时 `clip` 与面板矩形无交集（`world3d.rs` 的 `effective_clip`
  用例、`workspaces.rs:3201` `without_scroll_every_panel_clips_to_its_own_rect`）；
- 回归：chrome（输入/状态/提示/确认）的 rect 与改动前逐字节相同
  （`frame.rs:2261` `feathers_chrome_does_not_change_panel_geometry`；
  `workspaces.rs:3133` `scrolling_a_workspace_moves_the_strip_but_not_the_chrome_rows`
  断言 Hints/Status 不动）。

## 6. 与"功能移植"的边界

这一步**只改布局几何与滚动**，不改功能：

- 不在本步做新的数据源（那是 `SHOW_AGENT.md` 的 S1–S6；S1–S4 已在别处实现，S5/S6 未做）；
- 不在本步做插件扩展点（那是 F6/`PANEL_CANVAS.md`，切片 1/2 已实现）；
- 不动工作区层面（绝对坐标、拖拽、自动整理、全可见）。

L1–L4 已与功能移植（`SHOW_AGENT.md` 的 S1–S4、`in_input`、`ChoiceKeys`）一起落地；
L5 仍未做，且不是功能项的前置。L1 当时是纯几何重构，现在若再改 `layout.rs`，仍应
避免与其它布局改动同时进行。
