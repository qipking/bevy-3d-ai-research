# 实现架构（当前）

> 快照 2026-11-26。行号会漂，**类型名/函数名才是锚**。
> 口径：内容与行为对照 rig-tui，视觉与布局按 feathers + niri（见 `docs/design/FEATHERS_UI.md`、
> `docs/design/NIRI_SCROLL.md`）；像素级一致性不是验收标准。

---

## 0. 一句话

**上游 `rig-ecs`/`rig-harness` 管内核与会话；`rig_niri` 是视图层（行模型 + niri 布局 + 工作区）；
`feathers_in_3d` 把 bevy_ui 镜像成 3D 面板；`bevy_feathers` 提供视觉语言；
应用层 `rig_harness_in_3d` 负责装配与演示业务。**

---

## 1. 五层栈

| 层 | crate | 谁写的 | 职责 | 关键入口 |
|---|---|---|---|---|
| 内核 | `rig-ecs` 0.47 / `rig-core` | 上游 | agent、turn、effect、工具、inbox、`Deliver`、`Ask`/`Decision` | `AgentPlugin`、`Deliver::new`、`StopTurn` |
| 胶水 | `rig-harness` 0.47 | 上游 | 会话目录/日志、主 agent、picker、通知、进度、插件 | `RigHarnessPlugins`、`ShownAgent`、`Picker`、`Ask` |
| 应用 | `rig_harness_in_3d` | 我们 | 装配、演示工具（搜索/下单闸门）、工作区演示、设置/通知/状态 | `app(title)`、`KernelPlugin`、`WorkspacesDemoPlugin` |
| 视图 | `rig_niri` | 我们 | 行模型、转写引擎、布局、工作区、输入、补全、picker、选择、剪贴板、主题 | `FeathersViewPlugin` |
| 渲染桥 | `feathers_in_3d` | 我们 | bevy_ui → 世界空间面板/文字/拾取 | `FeathersIn3dPlugin`、`WorldPanel` |
| UI 工具包 | `bevy_feathers` 0.20 | 上游 | 视觉语言：`UiTheme`、token、按钮/输入框/滚动条样式、`Outline` | `FeathersPlugins`、`UiTheme` |

依赖方向（按 `Cargo.toml` 核实，没有循环）：

```
rig-core
  ▲   ▲
rig-ecs rig-harness-protocol / http / memory / tools
  ▲        ▲
rig-harness ┘
  ▲   ▲
rig-tui │
  ▲   │
rig_niri
  ▲
rig_harness_in_3d ──> rig-harness（直接，装配用）
  └────────────────> feathers_in_3d ──> bevy / bevy_ui / bevy_text（不认识 rig）
```

直接依赖（只列关键项）：

| crate | 直接依赖 |
|---|---|
| `rig-ecs` | `rig-core`、`rig-memory` |
| `rig-harness` | `rig-core`、`rig-ecs`、`rig-harness-protocol`、`rig-http`、`rig-memory`、`rig-tools` |
| `rig-tui` | `rig-core`、`rig-ecs`、`rig-harness`、`rig-markdown`、`rig-tools` |
| `rig_niri` | `rig-tui`、`rig-harness`、`rig-ecs`、`rig-core`、`feathers_in_3d`、`bevy_feathers` |
| `feathers_in_3d` | `bevy`、`bevy_ui`、`bevy_text`、`bevy_luma`（**不依赖任何 rig**） |
| `rig_harness_in_3d` | `rig_niri`、`rig-harness`、`rig-ecs`、`rig-core`、`rig-tui`、`feathers_in_3d`、`bevy_feathers` |

**容易误读的两点**：

- `rig_niri` 不是 `rig-harness` 的兄弟，而是在它之上：`rig_niri` 直接依赖 `rig-harness`
  （`ShownAgent`、`Answerer`、`ShowsPickers`、`Picker`、`Ask`）。应用层看起来"分叉"，只是因为它
  既装配 `rig_niri` 又直接装配 `rig-harness`。
- `feathers_in_3d` 是最底层的渲染桥，**不认识 rig**；是 `rig_niri` 依赖它，不是反过来。

### 为什么 `rig_niri` 依赖 `rig-tui`

`rig-tui` 里有两层东西：**终端前端**（terminal/input/keys/editor/pick 的绘制）和
**look 协议 + 纯文本渲染器**。`rig_niri` 只用后者（17 个文件、51 处 `use rig_tui::…`）：

| 类别 | 用的东西 |
|---|---|
| look 协议 | `RenderBatch`/`RenderInput`/`RenderOutput`/`RenderRequest`/`RenderKey`、`ToolRender`、`CallState`、`LookLine`、`RenderSystems`、`TranscriptRender`、`text_rows` |
| 主题 | `rig_tui::theme::Theme`（17 个 band） |
| 纯函数 | `wrap::{wrap,wrap_typed,wrap_all}`、`markdown::render`、`highlight::code_rows`、`diff::unified_lines`、`status::line` |
| 值类型 | ratatui 的 `Line`/`Span`/`Style`/`Color`、`TranscriptSource`、`Replaced`、`AppStatus`、`StatusItems` |

一个都没用：`terminal`、`input`、`keys`、`editor`、`pick` 的绘制、`TuiPlugin`。所以两个前端
（终端 / 3D）在**行为上并行**，只是共享的"内容稿件格式"上游没有单独拆 crate，放在了 `rig-tui` 里。
这是**有意保留的成本选择**（用户 2026-11-26 确认）：真要彻底解耦，得把约 3k 行协议+渲染器拷进
workspace 并持续跟上游漂移，收益不抵成本。

---

## 2. 应用装配（`rig_harness_in_3d`）

`lib.rs::app()` 的顺序（有约束，别随意重排）：

1. `Home::from_env().new_session()`（只读家目录时退到 `./.rig-home`）；
2. `TaskPoolPlugin`（**必须最先**，`windowed()` 假定它已装）；
3. `rig_harness::windowed(DefaultPlugins)`；
4. `bevy_feathers::FeathersPlugins`（`UiTheme`）；
5. `workspaces_demo::WorkspacesDemoPlugin`（N 个工作区、F1..F10、每工作区真 agent）；
6. `FeathersIn3dPlugin`（镜像/文字/拾取）；
7. `RigHarnessPlugins`（内核 + 主 agent + picker/ask/进度）；
8. `rig_niri::FeathersViewPlugin`（视图层全部系统）；
9. `HarnessExtPlugin`（本 app 的 `/hello`、`/setting` 等）。

| 文件 | 作用 |
|---|---|
| `main.rs` | 演示 binary：巡游机位（`home_z`/`zoom`）、截图（`RF_SHOT_DIR`）、合成指针 |
| `kernel.rs` | 两个演示工具 + **闸门**（`Ask` 按住调用）+ `answer_ask` 观察者 |
| `workspaces_demo.rs` | 多工作区演示：绑定 agent、聚焦键、非聚焦行同步、`restack_on_resize` |
| `settings.rs` / `notice.rs` / `status.rs` | 设置路径、通知历史、`AppStatus` |
| `show.rs` / `theme_cmd.rs` | `/show`（切转写来源：agent / 存档会话）与 `/theme`（dark/light）命令 |

---

## 3. 视图层内部（`rig_niri`，26 个模块）

### 3.1 输入与键位

| 文件 | 职责 |
|---|---|
| `keys.rs` | `KeyBinding`/`KeyScope`/`KeyFocus`/`InputAction`（32 个）/`KeyPressed`/多键前缀 `find`/`run_command` |
| `editor.rs` | 多行编辑器（`Editor` 资源）：折行、真实光标行列、历史、粘贴、发送；`act()` 把动作变成 `Deliver`/`StopTurn`/`AppExit`；`layout_editor`/`sync_editor_tile` 让输入 tile 随内容长高 |
| `complete.rs` | 补全（`/` 命令、`@` 文件、插件 source、候选 tile、Tab 接受） |
| `picker.rs` | picker 列（FIFO 队列、过滤、多选、树、preview、滚动条、`ChoiceKeys`、Alt+1..9） |
| `pane.rs` | 转写窗格：`TranscriptPane{source,title,scroll,selected,below}`、`ActiveTranscript`、窗口化、滚动条、PgUp/PgDn/Alt+↑↓/Shift+↑↓/Esc |
| `selection.rs` | 行级/条目级选择、拖选、点击、输入面板的透明焦点面 |

### 3.2 布局与工作区

| 文件 | 职责 |
|---|---|
| `layout.rs` | 分裂树：`Node`/`Split`/`Size::{Fraction,Proportion,Fixed}`；`rects`/`placements`；列宽预设、移动列、`append_column` |
| `frame.rs` | `FrameSize`（跟随窗口/`RF_FRAME` 固定）、`FramePlugin`、面板区域 `PanelArea`、3D 锚点 `WorldPanel`、面板 chrome token、确认 tile 高度 |
| `workspaces.rs` | `Workspace{session,tree,pose,order}`、`WorkspaceSet{items,focused}`、`WorkspacePanels{rows,roots,spawned,areas}`、`WorkspaceScroll`、排列/拖拽/聚焦、`clamp_scroll`/`scroll_to_visible`/`workspace_step` |
| `panel.rs` | 插件面板画布（F6）：`PanelId::Plugin(PluginPanel)`、注册/放进条带/自动装进工作区、`drawn_row(s)` 读回 |
| `niri.rs` / `spatial.rs` | 早期实验（RF_NIRI_LAYOUT / 卡片空间），仍在插件里但不在主链路 |

### 3.3 内容管线

| 文件 | 职责 |
|---|---|
| `batch.rs` | 内核 → `RenderBatch`：`rebuild_batch`（聚焦）、`run_looks`、`collect_rows`、`dialogue_batch`（非聚焦）、`live_additions`、`confirm_panel_rows`；调用树/子 agent、facts、流式 |
| `transcript.rs` | 对话引擎：`Transcript`（块缓存/排序/折行）、`Row`/`KeyedRow`、`TextId`/`BlockId`、行→UI 节点（span 底色/下划线/按钮外观） |
| `rich.rs` | markdown/代码高亮/diff → 行（直接调上游 `rig_tui::{markdown,diff,highlight}`） |
| `status.rs` / `hints.rs` | 状态行（items、`no agent`、turn items、`[status] hide`、`StatusParts`）与提示行 |
| `history.rs` | 提示词历史（JSONL 落盘 + `Sent`） |
| `clipboard.rs` | 系统/内存剪贴板、复制选中、Ctrl+V 粘贴 |
| `theme.rs` / `style.rs` | 17 个语义 band + 面板 chrome token + `UiTheme` 兜底；ratatui 色 → bevy 色桥 |

### 3.4 前端身份

`front.rs`（`Answerer`/`ShowsPickers`）、`title.rs`（窗口标题状态机）、`camera_rig.rs`（可选手柄相机）。

---

## 4. 四条主数据链

### A. 输入

```
KeyboardInput ──> keys::find(bindings, KeyFocus/KeyScope)
   ├─ 输入作用域 ──> editor::keys ──> act(action) ─┬─ 改 Editor（光标/文本/历史）
   │                                              ├─ Deliver(agent, text, Steer|Queue)
   │                                              ├─ StopTurn(agent) / AppExit
   │                                              └─ RunCommand（`/` 开头，应用层跑）
   ├─ 转写作用域 ──> pane 的键（滚动/选条目/交还焦点）
   └─ picker 显示时 ──> picker_keys（Alt+1..9/ChoiceKeys/过滤）

鼠标 ──> feathers_in_3d::pick（屏幕→面板局部 UI 像素）
   ├─ 点击行 ──> answer_ask / PickerChoice / 选择
   ├─ 拖选 ──> selection
   └─ 拖滚动条 ──> PaneScroll / PickerInput.scroll
```

### B. 内容（内核 → 行）

```
rig-ecs（Conversation / ToolRequest / Partial / Ask / StatusItems）
        │
        ▼  batch::rebuild_batch（聚焦工作区）
RenderBatch（RenderKey→RenderInput/Output） ──> run_looks（rig-tui 的 TranscriptRender）
        │                                              │ 业务 look（搜索表/下单）
        ▼                                              ▼
collect_rows ──> TranscriptEngine（折行/排序/流式让位）──> PanelRows[PanelId] = Vec<KeyedRow>
非聚焦工作区：dialogue_batch + live_additions ──> 各自 WorkspacePanels.rows
```

### C. 布局（行 → 面板矩形 → 世界）

```
FrameSize（窗口 logical px / CELL，RF_FRAME 可固定）
   └─ layout::Node（根：主体行 | Hints | Status；主体行：转写列 0.62 | 右列 0.38；列可伸出视口）
        └─ frame::lay_out_frame → PanelArea{rect,size,cols,rows,center} × 每个实体
             └─ place_panels → WorldPanel{units_to_world, anchor, clip=工作区视口}
                  └─ WorkspaceScroll 决定横向偏移；chrome（Hints/Status/Input/Confirm）不滚
```

### D. 3D 渲染

```
bevy_ui 子树（Node + Text/TextSpan + BackgroundColor/BorderColor/BorderRadius/Outline）
   ├─ world3d::sync_mirror：面板 → 世界网格；材质裁剪 = 面板 AABB ∩ 视口；Outline → 描边环
   ├─ text3d：TextLayoutInfo → 字形网格（按裁剪矩形剔字形）
   └─ sort：层/深度排序（UI 面板按工作区 pose 摆在世界里）
```

---

## 5. 帧内顺序（`FeathersViewPlugin` 实际链）

```
Prepare  ：按键分派、输入面板焦点面、按内容定面板尺寸（补全 tile/输入 tile）
Layout   ：sync_frame_size_to_window → sync_confirm_tile → lay_out_frame → place_panels
Draw     ：track_reply_stream → rebuild_batch → run_looks → collect_rows → confirm_panel_rows
Fit      ：window_transcript（每窗格窗口化）→ 滚动条同步 → 非聚焦工作区行同步
Render   ：panels::sync_panels（行→节点，按 key 前缀 diff）→ 编辑器行 → 滚动条/按钮外观
```

注意：`Draw` 产行、`Fit` 按区域窗口化、`Render` 才写节点；这个顺序改了会出现"行数对但显示的是一帧前的窗口"。

---

## 6. 状态归属

| 状态 | 类型 | 归属 |
|---|---|---|
| 工作区集合/焦点 | `WorkspaceSet` | 全局资源 |
| 一个工作区 | `Workspace{session,tree,pose,order}` | 实体组件 |
| 该工作区的面板/行/区域 | `WorkspacePanels{rows,roots,spawned,areas}` | 实体组件 |
| 横向滚动 | `WorkspaceScroll{scroll}` | 实体组件 |
| 转写窗格（来源/滚动/选中） | `TranscriptPane` + `ActiveTranscript` | 组件/资源 |
| 聚焦工作区的渲染批 | `RenderBatch`、`Dialogue`、`ReplyStream`、`TranscriptEngine` | 全局资源（跟随 `ShownAgent`） |
| 面板行与根 | `PanelRows`、`PanelRoots`、`PanelAreas` | 全局资源（聚焦工作区） |
| 布局树 | `FrameTree` | 全局资源（当前工作区树的工作副本） |
| 主题 | `Theme`（band）+ `UiTheme`（feathers token） | 全局资源 |
| 补全/picker/历史 | `CompletionState`、`PickerInput`/`PickerQueue`、`PromptHistory` | 全局资源 |

---

## 7. 工作区模型（niri 条带）

- **一个工作区 = 一个 agent 会话 = 一条条带**；条带有绝对世界位姿（`Workspace.pose`），竖直堆叠（`F4`/`arrange_workspaces`）。
- 条带内部是 niri 式**列**：转写列（0.62 视口）、右列（0.38：Spacer/Confirm/Complete/Input 竖直分割）；picker/补全可追加列。
- 列宽用 `Size::Fraction`（相对视口），**开列不改已有列宽**；内容可超出视口，用 `WorkspaceScroll` 横向滚动 + 视口裁剪。
- 聚焦只影响"谁接收键盘/谁用全局管线"，**不隐藏其它工作区**。
- 非聚焦工作区用自己的 `dialogue_batch + live_additions` 出实时行；聚焦工作区走 `ShownAgent` 的全局管线。

---

## 8. 测试与验证

- 全部无窗口 ECS 测试：`rig_niri` 606、app 36、bin 5、`feathers_in_3d` 15（`cargo test --workspace --offline`）。
- 真实渲染验证：`RF_SHOT_DIR=render/rf_niri_app cargo run -p rig_harness_in_3d`（巡游 17 张截图，自动退出）。
- 关键不变量都有测试：列宽不随兄弟变、开列不动旧列、裁剪 = 面板∩视口、行宽 ≤ 面板列数、焦点不会被转写抢走、滚动不改列几何。

---

## 9. 已知边界（有意为之）

- 没有弹层/浮窗：闸门是确认 tile，picker 是条带列，补全是右列 tile。
- `feathers_in_3d` 不镜像图片/图标：按钮一律文字；`Outline` 已支持。
- 输入框只借 feathers 外观，不换 `TextInput`（多行/光标/IME 自己管）。
- 像素级对齐不做；`StatusLine` 组件、开放面板画布（F6）、多来源（S2–S6）未做。

---

## 10. 平台注意事项（Android，2026-11-26 核查）

用户判断 bevy 对 Android 支持较好，适配应该不重。核查结论：**look 协议本身不阻碍 Android**
（纯数据 + 纯函数，零 OS 调用）；真正要注意的是下面这些，按严重程度：

| 项 | 现状 | 是否阻碍 |
|---|---|---|
| `bevy_clipboard` + `arboard` | `Cargo.toml` 无条件开 `system_clipboard`/`wayland`；`arboard` 3.7 的 `src/platform/mod.rs` 把 android 排除在所有平台后端之外 | **编译期硬点**：Android 上要把这两个 feature 平台条件化，先用内存替身（`ClipboardText` 抽象已留口） |
| `rig-tui` 的终端依赖树 | 依赖 `crossterm`/`ratatui`/`syntect`/`two-face` 等；我们运行时不调用 | 潜在构建点：`crossterm` 没有显式 android 支持，需 NDK 实测；若过不去，再把 look 协议抽成 seam/vendored crate（顺带把 syntect 摘出 APK） |
| 会话目录 | `Home::from_env()` 默认 `~/.rig`，`current_dir()` 在 Android 上可能不可用 | 运行期：显式 `RIG_HOME` 指 app data（`app()` 已有 `./.rig-home` 回退） |
| 资源加载 | `BEVY_ASSET_ROOT` 是桌面路径方案 | 运行期：用 `AssetPlugin` 的 APK assets |
| `rig-tools`/`rig-http` | shell 工具在 Android 上没有可执行环境 | 运行期：3D 前端可以不注册这些工具 |
| 输入法/软键盘 | winit 支持 Android；`KeyboardInput`/IME 合成事件没实测 | 需真机验证 |

顺序建议：平台条件化 clipboard feature → NDK `cargo check --target aarch64-linux-android` →
若唯一错误来自 crossterm/syntect，再拆 look 协议 → 最后处理路径、assets、IME。
