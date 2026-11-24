# 把 rig-harness 的 UI 完整换成 3D（实施计划）

> **2026-11-24 方向会议**：目标从"把 TUI 搬进 3D"上调为"**visionOS 级空间 UI 的 Bevy 实现**"。
> 终端**交互**（键位、整屏切块）降级/丢弃，终端编码的**领域模型**（turn/call/Ask/Pick/StatusItems/
> RenderBatch）全部保留；`frame.rs` 从"整个界面"降级为"卡片内部排版 + flat 模式"。
> 纲领、HIG 检查表、路线图 P0–P4 见 [`../SPATIAL_UI.md`](../design/SPATIAL_UI.md)。
> 下面 §3 的顺序在 P1（卡片化）之后重排：补全→composer 候选 chips，通知→卡片，编辑器→composer 卡。

## 0. 关键发现（读源码后）

`crates/rig-harness/src/lib.rs` 原文：

> Everything else, the terminal view, `--print`, `/reload` and the built-in tools included,
> is a plugin crate in the repository's `plugins/` folder, built only on this crate's public API.

所以官方结构是：

```
crates/rig-ecs        kernel
crates/rig-harness    服务 + 装配（会话/启动器/命令/设置/Ask/Pick/per-call…），不含视图
plugins/rig-tui       ← 终端视图，一个**插件**（crates.io 上的 rig-tui 0.47）
plugins/rig-basics    审批/工具等
crates/rig-cli + rig  生成工程、按 settings.toml 选插件、跑 App
```

**结论**：要"把 rig-harness 换成 3D UI"，正确做法是写一个**与 plugins/rig-tui 同构的插件**
（同样只用 rig-harness 公开 API），而不是另起一个 app。我们现有的三个 crate 对应关系不变，
但 `rig_niri` 必须按"插件"的形态组织：一个 `FeathersViewPlugin`，注册 Front、消费
`Ask`/`Picker`/`StatusItems`/`Notice`/`RenderBatch`，把自己的绘制系统挂进 `App`。

## 1. 插件对照表（plugins/rig-tui → rig_niri）

> 2026-11-25 按 [`PARITY.md`](PARITY.md) 的逐行审计修正过：原先 6 处 ✅ 属于乐观估计。
> 真实对位移植覆盖率 ≈ 25%（对位 3558 / 上游 14749 行），公开名字 104 个里 80 个没有对应物。

| rig-tui 模块 | 作用 | rig_niri 现状 | 下一步 |
|---|---|---|---|
| `lib.rs` (`TuiViewPlugin`, `Front="tui"`) | 插件入口 + Front | ✅ `FeathersViewPlugin`（同形：build + finish 注册 `StatusItems`）+ `front.rs` | — |
| `panel.rs` (`TuiPanel`, `Placement`, `lay_out`) | 可放置面板（四边、尺寸、浮层、剩余区、列） | ✅ `frame.rs`：`Placement`/`Extent`/`CellRect`/`lay_out`（逐条照抄，附 7 个单测） | `fits`（编辑器按内容收行）接上 |
| `render.rs` (1543 行) 的帧布局部分 | 面板 → 面板区 → 剩余区 | ✅ `frame.rs`：`FrameSize` + `lay_out_frame` + `place_panels`（格子 → Node 尺寸 + 3D 锚点） | — |
| `screen.rs` | 默认屏幕（输入/状态行/转写） | ✅ `frame.rs::spawn_default_screen`（Bottom 4 / Bottom 2 / Rest） | — |
| `pane.rs` | 转写窗格：窗口化 + 滚动 | ⚠️ 部分 `pane.rs`：`TranscriptRows` → `PaneScroll`(follow) → `PanelRows`；PgUp/PgDn | "↓ more below" 标记、pane 级 sticky、多 pane |
| `transcript.rs` | 条目 → 行、滚动、sticky | ❌ **未开始**（只有 `LookLine`→节点适配；上游 1783 行的对话引擎没搬） | 见 `PARITY.md` 缺口 1 |
| `status.rs` | `StatusItems`/`AppStatus` → 状态行 | ⚠️ 部分（无 `styled` 条目的颜色） 直接调 `rig_tui::status::line`（keep/min_width 裁剪就是 rig 的），左/右两段由 `SpaceBetween` 对齐 | `styled` 条目、`[status] hide` |
| `hints.rs` | 键位提示 + 宽度裁剪 | ⚠️ 部分 `hints.rs`（tables + fit 有；与运行时状态联动没有） | 与 turn running 联动 |
| `title.rs` | 终端标题 + OSC 133 标记 | ✅ `title.rs`：窗口标题 = `基础标题 · 状态`（working/needs you/error/idle，规则照抄） | OSC 133 只对终端有意义，不做 |
| `editor.rs`/`input.rs`/`keys.rs` | 多行编辑器、键位、补全、历史 | ⚠️ 基础（单行、光标、↑↓ 历史、PgUp/PgDn 归 pane） | 多行、补全、粘贴/图片 |
| `complete.rs` | 补全弹窗 | ❌ | `Over`/`BelowInput` 面板 + 候选源 |
| `pick.rs` (视图侧) | picker 浮层：列表 + preview + keys 行、树形折叠 | ⚠️ 大部分 `picker.rs`：容器 `Over` + 列表列 `Within` + preview 列 `Within`、打字过滤、`free_text`/`secret`、多选打勾、树折叠、`PickPreview::Text`、`Highlighted`、点击行 | `more`（`←` 展开完整说明）、`PickPreview::Conversation`（要值转写） |
| `theme.rs` | 调色（面/条/浮层） | ✅（自有 Theme） | 对齐 rig 的 band 比例（曾改过被回退，需与用户确认） |
| `markdown.rs`/`highlight.rs`/`diff.rs` | 富文本 look | ❌ | 业务 look 里补 |
| `clipboard.rs`/`selection.rs` | 选择与复制 | ❌ **未开始**（`bevy_clipboard` 有依赖但 `src/` 零使用） | 字符级 hit-test + 复制 |
| `screen.rs`/`terminal.rs` | 终端会话 | — 由 `feathers_in_3d` 取代 | — |

## 2. rig-harness 侧 UI 需要的服务（已下载到 `rig-harness-src/`）

| 模块 | 前端要不要管 | 说明 |
|---|---|---|
| `ask.rs` | ✅ 已接 | `Ask`/`Answer`/`Answerer`/`Answerers` |
| `pick.rs` | ✅ 基础 | `Picker`/`Chosen`/`Pick::person`/`ShowsPickers` |
| `commands.rs` | ⚠️ | slash 命令：输入 `/` 时列候选（配合 complete） |
| `describe.rs` | ⚠️ | 会话/agent 描述行 |
| `progress.rs` | ❌ | 进度（可做状态条目） |
| `per_call.rs` | ❌ | 每个调用的工具环境 |
| `host/session.rs` | ❌ | 会话存储/恢复（由 harness 负责） |
| `load.rs` | ❌ | 插件加载 |
| `settings.rs` | ❌ | 配置 |
| `secrets.rs` | ❌ | 密钥（无 key 阶段跳过） |

## 3. 实施顺序（每步都出对照截图）

1. ✅ **插件化**：`rig_niri` 暴露 `FeathersViewPlugin`（与 `TuiViewPlugin` 同形），
   `rig_harness_in_3d::app()` 只做装配；
2. ✅ **帧布局**：`Placement` + 面板区 + 剩余区，替换 3 块固定面板
   （`frame.rs` + `pane.rs`，见 §5 的验收截图）；
3. 🚧 **picker 浮层**：列表 + keys 行已做（`Over` 面板），preview 列与过滤未做；
4. **补全弹窗**：输入框上方 raised 面；
5. **标题/通知/队列**：`title.rs` + `Notice` + 队列行；
6. **编辑器**：多行 + `/` 命令 + `@` 文件 + 历史文件；
7. **富文本**：markdown/diff/highlight 的 look（业务可替换）。

## 4. 验收

- 同一份会话内容，`rig`（TUI）与我们的 app 各出一张截图，逐行比对文本与行序；
- 每个 UI 元素标注来源（rig 类型 / 自有类型），写进 README 的对照表；
- **2026-11-24 起**：每个 UI 改动还要过 [`SPATIAL_UI.md`](../design/SPATIAL_UI.md) §8 的 HIG 检查表
  （角尺寸、卡片距离、不依赖 hover、键盘可完成同一件事…）。

## 5. 第 2 步做到了什么（2026-11-24）

**帧模型**（`rig_niri/src/frame.rs`）：

| rig-tui | rig_niri | 备注 |
|---|---|---|
| `TerminalSize` | `FrameSize { cols, rows }` | 默认 96×28，`RF_FRAME=96x16` 可改 |
| `Placement::{Top,Bottom,Left,Right,BelowInput,Over,Rest,Within}` | 同名同义 | `ratatui::Constraint` 收窄成 `Extent::{Cells,Percent,Rest}` |
| `panel::lay_out` | `frame::lay_out` | 同样的 5 步、同样的 `MIN_TRANSCRIPT=(20,3)`、同样的"放不下就这一帧没有地方" |
| `stacking()` | `Placement::stacking` → `feathers_in_3d::WorldLayer` | 层 = 3D 深度 + 绘制顺序，浮层永远在上 |
| `PanelCanvas::reset(area)` | `place_panels` 写 `Node` 尺寸 + `WorldPanel.anchor` | 格子 → 像素（`CELL = 7.8×18`）→ 世界米 |
| `PanelCanvas` 的文字区 | `PanelArea { rect, cols, rows }` | 减掉面板装饰，`cols` 就是 look 的换行宽度 |

**窗格**（`pane.rs`）：`TranscriptRows`（整段）→ `PaneScroll { top, follow }` → `PanelRows`（这一帧的可见行）。
滚到底跟随新行；PgUp/PgDn 一页；滚轮按行滚。

**验收图**（`render/rf_frame/`、`render/rf_frame_small/`、`render/rf_overlay/`）：

| 图 | 证明了什么 |
|---|---|
| `rf_frame/01_overview.png` | 三块面板由 `Placement` 摆好：转写 `Rest`、状态行 `Bottom(2)`、输入 `Bottom(4)` |
| `rf_frame/03b_oblique45.png` | 面板在同一个 3D 平面里，45° 下文字与面板一起倾斜 |
| `rf_frame_small/01_overview.png` | `RF_FRAME=96x16` 时转写只剩 8 行：窗口化取的是**末尾**（跟着新行） |
| `rf_frame_small/05_scrolled.png` | `scroll.up(4)` 后窗格上移，看到更早的行 |
| `rf_overlay/08_overlay.png` | `Placement::Over` 浮层面板盖在转写上面（`WorldLayer` 1） |
| `rf_frame/08_overlay.png` | 没有浮层时同一格是空的（对照） |

**已知缺口**（下一步）：

- 编辑器 `fits`（按内容长高）还没接 `FramePanel.fits`；
- `complete.rs`（补全弹窗）见 §3 第 4 步；
- 通知/队列行（rig 的 `Notice`）。

（状态行裁剪、滚动标记、窗口标题在 §7 补齐了。）

## 6. 第 3 步做到哪了：picker 浮层（2026-11-24）

`rig_niri/src/picker.rs`，用帧布局的 `Placement::Over` 画 rig-harness 的 `Picker`：

| rig-tui `pick.rs` | rig_niri | 状态 |
|---|---|---|
| 浮层占整屏（终端里没别的地方） | 居中浮层 `Over { Percent(84), Percent(74) }` | ✅（3D 里是一块对话框） |
| 标题 + 说明 + 列表 | 同左，标题 `▌` + 黄色粗体 | ✅ |
| 选中行高亮 + 标记 | `> ` 标记 + `theme.band` 色带 + 青色粗体 | ✅（用 ASCII：字体子集没有 `▸`） |
| 键位行 | 列表下方一行 | ✅ |
| ↑↓ 选、PgUp/PgDn 翻页 | 同左（`PAGE = 10`） | ✅ |
| 回车确认、Esc 关闭 | `Pick::person(entity, Some(Chosen::item(i)))` / `None` | ✅ |
| 点击行 | `PickerChoice` + 拾取 | ✅ |
| `Highlighted`（给插件换 preview） | 选中变化时插入 | ✅ |
| 过滤输入 | 打字 → `rows_for`（每个词都要命中，标签命中在前） | ✅ |
| `free_text` / `secret` | 末尾一行"输入的就是答案"，回车 `Chosen::text`；secret 显示成 `*` | ✅ |
| 多选打勾（`Ticked`/`TickBox`） | Tab 打勾，`[x]/[-]/[ ]` 用 rig 的 `TickBox::of` 算；分支**一次整枝切换** | ✅（与 rig 的两段式不同，见 §8） |
| 树形折叠（`parent`） | ←/→ 折叠展开，`+`/`-` 标记（字体子集没有 ▸/▾） | ✅ |
| `PickPreview` 预览列 | `Within { part: 1, of: 2 }` 一列，`rig_tui::wrap` 折行 | ✅ `Text`（`Conversation` 待做） |

同时立了两件事：

- picker 打开时，输入框与转写窗格不抢方向键（`picker::no_picker` 作为 `run_if`，
  对应 rig-tui 的 `KeyScope`）；
- 浮层用 `WorldLayer(1)`（`Placement::stacking`），斜着看也盖在转写上面
  （`render/rf_picker/10b_picker_oblique.png`）。

验收图（`render/rf_picker/`）：

| 图 | 证明了什么 |
|---|---|
| `09_picker.png` | 浮层出现，第一项选中（`>` + 色带 + 青色粗体） |
| `10_picker_selected.png` | 三次 ↓ 后选中 `google/gemini-2.5-pro`（键位走的是真 `KeyboardInput`） |
| `10b_picker_oblique.png` | 45° 下浮层仍在转写上面：层排序不是距离排序 |
| `11_picker_closed.png` | 回车 → harness 跑命令并 despawn picker → 浮层消失、转写恢复 |

## 7. 第 5 步（部分）：状态行裁剪 / 滚动标记 / 窗口标题（2026-11-24 晚）

| rig-tui | rig_niri | 做法 |
|---|---|---|
| `status::line(items, width)`（公开函数，按 `keep` 丢、按 `min_width` 缩成 `…`） | `status.rs` **直接调用它** | 不重写规则：状态行行为与终端逐字一致 |
| 左一条、右一条分两个区域画 | `Row::with_right(line)` | bevy_ui 的 `SpaceBetween` 负责右对齐（不算空格，CJK 也不会错位） |
| `pane::draw_below`：滚上去时右下角 `↓ more below (PgDn)` | `pane::BELOW_MARKER` | 最后一行的右端；占掉一行，和 rig 占的是同一行 |
| `title.rs` 的 OS133/标题状态机 | `title.rs`：`基础标题 · {working,needs you,error,idle}` | 规则照抄（picker/未答 Ask → needs you；有 turn 在跑 → working；上个 turn 失败 → error） |

验收：

| 图 / 日志 | 证明了什么 |
|---|---|
| `rf_picker2/01_overview.png` 的状态行 | 左边 `no model  312 tokens`，右边 `ctx 12% 1 agent`（右对齐） |
| `rf_frame_small/05_scrolled.png` | 滚上去后最后一行右边是 `↓ more below (PgDn)` |
| `rf_picker2/10c_picker_filtered.png` | 打 "deep" 过滤到只剩 `deepseek/deepseek-chat`，输入行 `> deep▏` |
| `render/tour.log` 的 `[title]` | `idle → needs you`（闸门 Ask）→ `idle`（答完）→ `needs you`（picker 开）→ `idle`（picker 关） |

## 8. 第 3 步收尾：picker 的多选 / 树 / 预览列（2026-11-24 夜）

**结构**：rig-tui 的 picker 是"容器 + 左列表 + 右预览 + 底部键位行"。用帧布局表达：

```text
Overlay  Over { 84%, 74% }                     容器（底色 + 边）
List     Within { panel: Overlay, part: 0, of }  列表列；of = 1 没预览，2 有预览
Preview  Within { panel: Overlay, part: 1, of: 2 }  预览列（插件挂 PickPreview::Text 时才有）
```

有预览时把 List 的 `of` 从 1 改成 2（改的是 `FramePanel.placement`，帧布局每帧重读），
列表的列宽因此真的变成一半——键位行用 `hints::fit_keys` 按半宽丢低优先级，
说明文字用 `rig_tui::wrap::wrap_all` 按列宽折行。**预览列不再盖住列表**。

**多选**：Tab 打勾；显示用 `rig_tui` 那条规矩算的 `TickBox::of`（`[x]` / `[-]` / `[ ]`）；
回车在"动过勾"时交 `Chosen::items(ticked)`，否则交选中的那一个。

> 与 rig 的一处**有意偏差**：rig 的分支勾是两段式（第一次只勾中分支、第二次才勾叶子，
> 所以第一次按完分支仍显示 `Off`）；3D 里做成一次整枝切换。`TickBox::of` 仍然用 rig 的，
> 只是不会出现"分支勾了但显示 Off"的中间态。

**树**：`rows_for` 用 rig 的 `shown()`（展开状态 + 过滤命中），←/→ 用 rig 的 `unfold()` 规则
（打开分支 / 落到第一个子项 / 关上 / 回到父项）。折叠标记用 ASCII `+`/`-`。

**预览**：`PickPreview::Text` → 预览列；高亮变化时插件更新它（demo 里是一个假插件
`demo_preview`，用 `Highlighted` 生成文本——rig 里这是插件自己的事）。

**顺手修的一个真 bug**：`PanelSpawned` 记着每块面板行实体的 id，浮层面板被 despawn 后没清，
下一块同名面板出现时行同步会去动已经死掉的行实体（`Entity despawned` panic）。
加了 `PanelSpawned::forget(id)`，浮层关闭时清掉。

验收图（`render/rf_tree/`）：

| 图 | 证明了什么 |
|---|---|
| `12_tree.png` | 树初始：`> + [ ] sessions`（选中、折叠、未勾），右边 preview 跟着高亮 |
| `12b_tree_open.png` | → 展开后子项出现（`-` 标记），说明文字折行在左半列 |
| `12c_tree_ticked.png` | ↓↓ + Tab：`[x] yesterday`，preview 换成 yesterday |
| `13_tree_closed.png` | 回车交 `Chosen::items([2])` → harness 收掉 picker，浮层与预览列都消失 |
| `09_picker.png` | 单选 picker（没有 preview）时列表占满容器（`of = 1`） |

## 9. Niri 窗口逻辑：仅作布局参考（2026-11-25 归档环/总览线）

> **范围收窄**：`niri` 只用于 `rig_niri` 移植 `rig-tui` 时组件/窗口的**布局位置设计**参考
> （源码 `reference/niri/niri-main`）。环模式、圆柱体总览、两级视角不再推进；原设计已归档 →
> [`GEOMETRY_HISTORY.md`](../archive/GEOMETRY_HISTORY.md)。

**代码现状（保留，不再扩线）**（`rig_niri/src/niri.rs`，40 个单测，语义照 niri，独立实现）：

- 列宽预设 / 前缀和 / `view_offset` 居中且夹取 / 列内竖分 / 指数平滑滚动；（布局参考）
- `Tiling::ring`：总览把各层贴到圆柱内壁（半径 = 最长带鱼屏/2π，短的不绕圈）；**（已归档，不推进）**
- 环上卡片**首尾相接无缺口**（无缝隙边界 + 弦中点定位）；**（已归档，不推进）**
- 总览自由相机（WASD/QE + 鼠标右键/方向键）；**（已归档，不推进）**
- **`Tiling::tile_cells(layer, column, tile)`**：平铺格子 → 文字列数/行数 ——
  这是旧面板（转写/输入/状态）接上 Niri 的那把钥匙。

**下一步（按顺序，每步都能单独验收；只做布局参考部分）**：

1. `pane.rs::pane_rows` 增加 Niri 来源：转写面板挂 `Tile` 后，行数 = `tile_cells` 的行，
   `frame::PanelAreas` 只作为旧路径保留；
2. `frame::spawn_default_screen` 改成"三块真面板 = 三个 tile"：
   转写 `2/3` 列、composer `1/3` 列（hints 作为它的第二个 tile）、状态行做底部 ornament；
3. ~~弹窗改平铺（picker/补全/确认 → tile），去掉 `Placement::Over` 的浮层路径；~~
   **（已归档，不推进）**
4. ~~底部层指示条（当前层高亮）+ 远层只留标题条；~~ **（已归档，不推进）**
5. ~~转写分列，把带鱼屏拉长到 ≥ 8.8 m，让首尾相连自然成立。~~ **（已归档，不推进）**

> 当前实际推进方向见 §10（harness 装配与功能面），与环/总览无关。

## 10. harness 本体终于装上了（2026-11-25 夜）

**发现**：`rig_harness_in_3d::app()` 里只有 `rig_ecs::AgentPlugin`——那是内核。属于 harness 的
13 个插件（services / session / commands / launcher / clock / watch / run / progress …）**一个都没装**，
所以这个 app 三周来只能跑手写巡游：它没有 harness 可以驱动。这是"进度推不动"的真正原因，
和渲染、布局都无关。

**装配**（`rig_harness_in_3d/examples/harness_boot.rs`，已跑通）：

```rust
app.add_plugins((LogPlugin, TaskPoolPlugin { task_pool_options: rig_harness::task_pools() },
                 TimePlugin, FrameCountPlugin));           // 必须最先：build 阶段就要 IoTaskPool
app.add_plugins(rig_harness::windowed(DefaultPlugins)…);    // 剥掉 headless 已提供的那几样
app.add_plugins((RigHarnessPlugins, FeathersViewPlugin, FeathersIn3dPlugin));
```

三个必须自己补的洞：

1. `windowed()` 假定先装了 `HeadlessPlugins`（自带 log/任务池/帧计数/时钟/无窗口 runner）。
   窗口版要自己补回来，**任务池必须排最前**，否则 `IoTaskPool has not been initialized` panic；
2. `SessionPlugin` **只读目录不建目录**（正常由 `rig` 启动器建）。嵌入时用
   `Home::new_session()` 建好再插 `SessionPaths`，否则 `could not mark the session started: ENOENT`；
3. `TranscriptFont` 原来只放在 demo 的 `main.rs` 里，视图插件自己起不来。

**验证**（非视觉）：`RIG_HOME=.rig-home cargo run --example harness_boot` →
会话目录建出、`[title] idle`、面板出真实行、无参数校验告警、20 s 后正常退出。

**下一步（接真实链路，验收靠日志/测试）**：把这套装进 `lib.rs::app()`；命令回复进转写；
`Notice` → 转写行；`Ask` 闸门可回答；设置与会话恢复。
