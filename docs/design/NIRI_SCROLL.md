# 布局优化：niri 式滚动平铺（横向无限延伸）

> **状态**：计划。用户 2026-11-25 指示：**功能移植做完之后**执行这一步，严格按 niri 的
> 滚动平铺哲学。
> **依据**：`reference/niri/niri-main/docs/wiki/Development:-Design-Principles.md`
> （三条基本原则）、`Configuration:-Layout.md`（列宽/间隙/居中策略）。
> **对象**：`rig_niri` 的条带内部布局（`layout.rs` / `frame.rs` / `panels.rs` /
> `workspaces.rs`）。工作区层面的模型（一条工作区 = 一条条带、竖直堆叠、绝对空间
> 坐标）不变。

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

## 1. 现在错在哪

niri 的第一原则（`Development:-Design-Principles.md:7`）：

> Opening a new window should not affect the sizes of any existing windows.

我们现在的布局是**分割树按比例分剩余空间**：

- `layout::Size::Proportion(w)` 是"同层比例"（`rig_niri/src/layout.rs:60-68`）：权重 /
  同层权重和。开新列时 `append_column`（`layout.rs:475` 一带）把新列设成 1/3、已有列
  按同一因子**等比缩小**——正是 niri 要避免的行为。
- 条带宽度恒等于 `FrameSize.cols`（`frame.rs:321` 的 `screen` → `TileRect`），没有
  "内容比视口宽、横向滚动"的概念。
- 于是 niri 的第二原则（`:24`，"focused window 不该自己移动"）也没法成立：任何开列
  都会挤动聚焦列。

---

## 2. 目标模型（照 niri）

```
一条工作区条带 = 一个视口（viewport，宽 = FrameSize.cols 格）
条带内容     = 一串列（columns），每列有**自己的宽度**，从 x=0 向右无限排
视口         = 内容的 [scroll, scroll + viewport) 窗口，scroll 可移动
列内         = 竖直分割树（现在的 Split::Vertical 直接沿用）
```

四条不变量（每条都要有测试）：

1. **开新列不改已有列宽度**：新列取 `default_column_width`（niri 默认
   `proportion 0.5`，即视口一半），已有列的宽度、x 坐标一个字节都不变。
2. **列宽是视口的比例/固定格数，不是"同层兄弟的份额"**：`0.5` 永远是视口的一半，
   不随兄弟数量变化。预设宽度照 niri：`1/3`、`1/2`、`2/3`（`Configuration:-Layout.md:17-19`）。
3. **聚焦列自动滚进视口**，且只移动 `scroll`，不改任何列的位置/宽度。
4. **动作立即生效**：加列、滚、改列宽、关列都是当帧改完（不做补间；补间最多是
   `scroll` 的视觉插值，见 §5 L5）。

### 2.1 条带里的一切都滚（包括输入/确认）

niri 里没有"常驻输入框"，一切皆列。我们一开始想给聊天留个例外（把输入/状态/提示/确认钉在
视口右缘），实现后发现两处坏处：追加的 picker 列会落在右列**之后**、把右列挤出视口；
要真正钉住就得把条带拆成"可滚动子区域 + 固定 chrome"两个容器，属于结构改动。

**最终决定（与 niri 一致）**：

- 条带 = 根竖 split 的中间那行；行里的**所有列**（转写、picker/追加列、右列的输入/确认/补全）
  一起横向滚动；
- 唯一的固定元素是**横跨整条的两行**：Hints 与 Status——它们在根竖 split 里、条带的纵向范围
  之外，本来就不属于滚动内容；
- 于是打开 picker 时输入框会跟着左移（仍在视口内、仍能打字），关掉后跟着滚回来。这不是
  缺陷，是"输入框也是一列"的直接结果；如果以后要钉住它，需要按上面的结构改动重做
  （`docs/status/REMAINING.md` 的 F6 面板画布是同一件事的地基）。

---

## 3. 数据模型（改哪几个类型）

### 3.1 `rig_niri/src/layout.rs`

- `Size::Proportion` 的语义**改掉**：从"同层兄弟份额"改成"**视口的比例**"
  （niri 的 `proportion` 就是相对视口）。旧行为用 `Size::Share(f32)` 保名（如果还有
  地方需要"兄弟份额"，只留给垂直分割内部的窗口高度）。
- 新增 `struct Strip { columns: Vec<Column>, scroll: f32 }`（或直接扩 `Node`）：
  - `Column { width: Size, node: Node }` —— 垂直的 `Node` 复用现有树；
  - `scroll`：以格为单位的水平偏移（`f32`，允许半格，渲染时对齐到像素）。
- `rects` 的新口径：列 x = 前面所有列宽之和 − `scroll`；**不再**按剩余空间等分。
  列可以部分/完全在 `[0, viewport)` 之外（部分可见是 niri 的观感，必须支持）。
- `append_column` 变成：`push(Column::new(default_width, node))`，然后
  `scroll_to_visible(new_index)`；**不动已有列**。
- `remove_column`：摘掉后 `scroll` 夹到新范围（niri 关窗后视口回弹，但聚焦列不移动）。

### 3.2 `rig_niri/src/frame.rs`

- `lay_out_frame` 产出仍然是 `TileRect`（米），但允许矩形**超出**帧：新增
  `FrameRects { rects, clip: TileRect }`——`clip` 是视口（条带矩形）。
- `place_panels` 照旧摆放；**可见性/裁剪**交给渲染层（§3.4）。

### 3.3 `rig_niri/src/panels.rs`

- `PanelRows` 按 `PanelId` 分桶不变；列滚动不影响行内容，只影响面板 rect。

### 3.4 `feathers_in_3d`（裁剪）

现在是"每个面板自己的矩形裁剪"（`world3d.rs` 的 `panel_clip`）。列滚出视口后
面板矩形会伸进隔壁条带，必须再加一层**条带视口裁剪**：

- 给 `WorldPanel` 加 `clip: Option<TileRect>`（条带视口，世界坐标）；
- shader 的裁剪矩形取"面板矩形 ∩ 条带视口"；
- 只有转写列挂这层 clip；chrome 面板不挂（它们本来就在视口内）。

### 3.5 `rig_niri/src/workspaces.rs`

- 条带的世界位姿不变（绝对坐标、竖直堆叠）；
- 每个工作区各自持有自己的 `Strip`（列 + scroll），互不影响。

---

## 4. 键位/交互（照 niri 的语义，键位按 3D 场景调整）

| 动作 | niri | 我们（建议） | 语义要求 |
|---|---|---|---|
| 横向滚动 | Mod+滚轮 / `focus-column-left/right` | `Alt+←/→`、横向滚轮 | 只改 `scroll`；到两端夹住 |
| 聚焦上一/下一列 | `focus-column-left/right` | `Alt+←/→` 的聚焦版（与滚动二选一由 shift 区分） | 聚焦列必须滚进视口 |
| 新列（拆窗） | 开新窗口 | 现有 `Alt+S`（`frame.rs:948` 的 `split_transcript`） | 已有列宽度不变 |
| 关列 | 关窗 | 现有 `remove_column` 入口 | scroll 夹紧，聚焦列不移动 |
| 改列宽 | Mod+R / 预设宽度 | `Alt+Shift+←/→` 循环 `1/3 → 1/2 → 2/3` | 列宽按视口比例；不挤动别人 |
| 移动列 | Mod+Shift+方向 | `Alt+Ctrl+←/→` | 交换位置，宽度跟着列走 |
| 居中策略 | `center-focused-column never/always/on-overflow` | 先固定 `never` | 聚焦列滚进视口即可，不居中 |

**不做的**：niri 的 overview / 手势 / 动画系统；我们不引入弹层或总览几何
（用户早前明确否掉）。

---

## 5. 实施拆分（每步可独立提交 + 测试）

| 步 | 内容 | 文件 | 风险 |
|---|---|---|---|
| **L1** | `Size::Proportion` 语义改成"视口比例"，加 `Strip`/`Column`，`rects` 按累加 x 摆列；旧的分割树只服务列内竖直分割 | `layout.rs` | M（老测试要重写） |
| **L2** | 叠加 `scroll` 与视口裁剪：`FrameRects.clip` + `feathers_in_3d` 的条带 clip | `frame.rs`、`feathers_in_3d/src/world3d.rs` | M |
| **L3** | 聚焦/滚动动作：`scroll_to_visible`、`Alt+←/→`、横向滚轮；Esc/焦点交还沿用现有 | `pane.rs`、`keys.rs`、`workspaces.rs` | M |
| **L4** | 列宽预设与移动列：`Alt+Shift+←/→`、`Alt+Ctrl+←/→` | `layout.rs`、`keys.rs` | S |
| **L5** | （可选）`scroll` 的视觉插值 + `center-focused-column on-overflow` | `pane.rs`/`camera` | S |

### 验收（全部无窗口测试，不做视觉研究）

- **不变量 1**：开列前后，已有列的 `(x, width)` 逐字节相等（`rects()` 直接断言）；
- **不变量 2**：同一列在兄弟数变化时宽度不变（`0.5 * viewport` 恒成立）；
- **不变量 3**：聚焦列滚进视口后 `scroll` 满足 `x ≥ scroll && x + w ≤ scroll + viewport`
      （或列本身比视口宽时左贴边）；
- **不变量 4**：动作当帧生效（`app.update()` 一次后 rect 已变）；
- 裁剪：列完全在视口外时 `clip` 与面板矩形无交集（渲染层测试用纯几何函数）；
- 回归：chrome（输入/状态/提示/确认）的 rect 与改动前逐字节相同。

---

## 6. 与"功能移植"的边界

这一步**只改布局几何与滚动**，不改功能：

- 不做新的数据源（那是 `SHOW_AGENT.md` 的 S1–S6）；
- 不做插件扩展点（PARITY #10）；
- 不动工作区层面（绝对坐标、拖拽、自动整理、全可见）。

功能移植（S1–S3、`in_input`、`ChoiceKeys`）收尾后再开始 L1。L1 是纯几何重构，
可以和功能项并行，但**不要**和 `layout.rs` 的其它改动同时进行。
