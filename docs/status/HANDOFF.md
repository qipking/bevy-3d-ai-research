# 交接文档：rig_niri / feathers_in_3d

> 写于 2026-11-23，目的是**上下文超限时也能接着干**。先读这一页，再按需展开其它文档。
> 工作区：`/var/home/lichi/桌面/bevy_3d_ai_research`（jj 仓库）

---

## 0. 事故记录（已按 jj 官方文档修正）

**发生的事**：我执行了不带路径的 `jj restore`。官方帮助原文：

> When neither `--from` nor `--into` is specified, the command restores into the working copy from
> its parent(s). `jj restore` without arguments is similar to `jj abandon`, except that it leaves
> an empty revision with its description and other metadata preserved.

**`jj restore` 无参数 = 丢弃整个 change 的全部改动**，不是"回退某几个文件"。
（`snapshot.max-new-file-size` 默认 4MiB 的警告只是拒绝快照 `tools/pylib/*.so`，快照本身是成功的。）

**恢复方法（已用它救回全部代码）**：

```sh
jj op log                 # 找到 restore 之前那条 snapshot 操作
jj op restore <op-id>     # 整个仓库（含工作区）回到那一刻
jj undo                   # 或撤销上一条操作
jj evolog -r @            # 工作区 commit 的历次版本
```

**规则**：
1. 只回退部分文件 → `jj restore <paths> [--from <rev>]`，**永不裸跑** `jj restore`；
2. 出错先 `jj op log` / `jj undo`，操作日志能救回来；
3. 大文件先 `.gitignore` 再下载；需要放宽用 `jj config set --repo snapshot.max-new-file-size <N>`；
4. `jj st` 里的 `A/M` 是工作区相对父提交的差异，属正常。

## 0.1 一页速览

**目标**：用 **feathers 做前端 + rig 做内核**，做一个带 3D 空间界面的助手（先桌面，后移动端陀螺仪相机）。
**当前原则（2026-11-25 重定）**：`niri` 只用于 `rig_niri` 移植 `rig-tui` 时组件/窗口的
**布局位置设计**参考（源码 `reference/niri/niri-main`）。斗兽场漏斗、圆柱体总览、环、两级视角
全部不再推进，相关设计已归档 → [`GEOMETRY_HISTORY.md`](../archive/GEOMETRY_HISTORY.md)、
[`WATCHWALL.md`](../archive/WATCHWALL.md)。当前推进的仍是功能面（会话、命令、设置、闸门、picker，
用测试验收），与这些旧几何无关。

| 目录 | 是什么 | 状态 |
|---|---|---|
| `feathers_in_3d/` | 把 bevy_ui/feathers 子树渲染成世界空间 3D 面板（路线 B）：几何/文字/排序/裁剪/拾取 | ✅ 可用，有 README |
| `rig_niri/` | **rig-tui 的另一个完整前端**（纯 lib，视图层） | ✅ 见 `README.md` / `UPSTREAM.md` |
| `rig_harness_in_3d/` | **通用 app 的 fork 模板**（harness 层：装配 + 扩展点 + 示例业务） | ✅ 见 `README.md` |
| `bevy_luma/` | 上游源码（git main，0.20）+ 1 处本地改动（alpha_mode 固定 Premultiplied） | 依赖 |
| `spike/` | 最早的路线 B 验证工程（0.20.0 树），留作证据 | 归档 |
| `occlusion_probe/` | 遮挡异常的复现/对照工具（子代理产物） | 归档 |
| `render/rf_shots/`、`render/rf_300/`、`render/probe/` | 截图证据 | 证据 |
| `VERIFICATION_REPORT.md` / `RIG_NIRI_MOBILE_PLAN.md` / `VR_ASSISTANT_STACK.md` | 早期调研与验证 | 参考 |
| `occlusion_report.md` | 遮挡异常结案报告 | 参考 |

---

## 0.9 方向（2026-11-24 会议）

目标不是"把 TUI 画到 3D"，而是 **visionOS 级空间 UI 的 Bevy 实现**：
终端交互降级为辅助输入，领域模型全保留。纲领、HIG 检查表、路线图 P0–P4 在
[`SPATIAL_UI.md`](../design/SPATIAL_UI.md)——**每个 UI 改动都要过它的 §8 检查表**。

---

## 1. 怎么跑

```sh
cd research

# 中文字体（Bevy 只认 .ttf/.otf，系统里是 .ttc 集合）
./tools/fetch_cjk_font.sh            # -> rig_niri/assets/fonts/NotoSansCJKsc-Regular.otf

cd rig_harness_in_3d          # 装配层；rig_niri 是纯 lib
export CARGO_HOME=/var/home/lichi/桌面/bevy_3d_ai_research/.cargo-home   # 本机 ~/.cargo 只读
cargo build
BEVY_ASSET_ROOT=$PWD RF_SHOT_DIR=../render/rf_shots ./target/debug/rig_harness_in_3d
```

- **必须**给 `BEVY_ASSET_ROOT`，否则 Bevy 去 `target/debug/assets` 找字体；
- 运行约 30 秒，自动截图后退出：`01_overview` / `02_near` / `03a_mild_oblique` /
  `03b_oblique45` / `04_text_zoom` / `05_scrolled` / `06_gate_ask` /
  `07_gate_confirmed` / `08_overlay`；
- 环境变量：`RF_FRAME=96x28`（帧的格子数，16 行时转写会窗口化）、`RF_OVERLAY=1`（加一块
  `Placement::Over` 浮层）、`RF_RIG=1`（手柄/键盘相机）、`RF_MARKERS=1`（几何参照方块）。
- 帧布局的现状与缺口见 `rig_harness_in_3d/PLAN.md` §5。

---

## 2. 版本与依赖（不要随意动）

| crate | 版本 | 说明 |
|---|---|---|
| bevy | **=0.20.1** | rig-harness/rig-ecs 精确钉 0.20.1，全栈必须这个版本 |
| bevy_feathers | =0.20.1 | |
| bevy_window / bevy_winit | =0.20.1 + `custom_cursor` | 不加会 E0004（见 `spike/RESETUP_DOC.md` §7.1） |
| rig-tui / rig-core | =0.47.0 | rig-tui 提供 `RenderBatch`/`LookLine`/look 调度 |
| bevy_luma | path `../bevy_luma` | 只用于 `UNodeMaterial3d` + `UnivisRenderPlugin` |

⚠️ `rig-harness 0.47` 依赖 `bevy = "=0.20.1"`，所以 `bevy = "=0.20.0"`（spike 里的老钉法）**不能**用在这里。
⚠️ 不能用 `LumaPlugin`（与 bevy_ui 调度成环），只能用 `UnivisRenderPlugin`。

---

## 2.5 定位（2026-11-23 调整）

`rig_niri` 不再当"业务 app"，而是：

1. **rig-tui 的并行前端**（同样吃 `RenderBatch`/look，前端身份 `Front::new("feathers")`）；
2. **通用 app 的 fork 模板**：业务只改「工具 / look / 面板行来源」三处；
3. **紧跟上游**：只依赖公开 API，升级检查清单见 `rig_niri/UPSTREAM.md`。

三个 crate 对齐 rig 官方分层：`rig-ecs`（上游内核）→ `rig_harness_in_3d`（harness/模板，含示例业务）
→ `rig_niri`（视图）→ `feathers_in_3d`（3D 渲染）。

## 3. 架构与数据流

```
rig-ecs（agent / turn / 工具调用 effect）
   │  事件与组件
   ▼
rig_niri/src/batch.rs     ← 从 effect 读取状态，构造 rig-tui 的 RenderBatch
   │  RenderSystems::Render 上的 look：fn(&ToolRender) -> Vec<LookLine>
   ▼
rig_niri/src/transcript.rs  LookLine::colored() -> Row -> feathers Node/Text/TextSpan
   │  （行带 RowText + Pickable）
   ▼
feathers_in_3d
   ├─ world3d.rs  每节点 → Mesh3d + UNodeMaterial3d（SDF 圆角/边框/AA/裁剪）
   ├─ text3d.rs   bevy_text 字形 → 四边形，每（面板×图集）1 mesh
   ├─ sort.rs     Transparent3d 排序覆写（面板内按 UI 画笔顺序）
   └─ pick.rs     射线 → 面板平面 → 节点矩形 → bevy_picking PointerHits
```

关键约束/坑（都踩过）：
1. **UI 子树必须挪出视口**（`position_type: Absolute; left: -20000px`），否则 2D 渲染盖在 3D 上；
2. **共面节点的绘制顺序只能靠排序键**（luma 的做法），不能再靠 z 偏移；
3. **`PointerOut` 会沿 `ChildOf` 冒泡**：监听器必须判断"是不是我要处理的实体"，否则会把祖先的背景写成透明（曾导致"面板消失、背后立方体透出"）；
4. **`TextSpan` 隐式要求 `TextFont`**（默认 20px 默认字体），不显式给会换字体/换图集；
5. **每个 Text 的字形可能落在多个字体图集**（Latin + CJK），必须按图集分组建 mesh，否则后一个覆盖前一个；
6. `clip_bounds` 单位是**米**不是 UI 像素；
7. 面板内**没有填充的节点也要落位**（给 Transform），否则它的文字子节点会塌到面板中心。

---

## 4. 目前实现到哪

| 能力 | 位置 | 状态 |
|---|---|---|
| LookLine → feathers 行 | `rig_niri/src/transcript.rs` | ✅ |
| rig-tui look（`RenderSystems::Render`） | `rig_niri/src/batch.rs` | ✅ 两个示例 look |
| **RenderBatch 驱动** | `rig_niri/src/batch.rs` | ✅ 手工构造 `RenderBatch`（因为 `renderers::add` 是 `pub(crate)`） |
| 滚动 | `main.rs::scroll_panel` + `Overflow::scroll_y` | ✅ |
| 射线拾取 / 悬停 / 选择 / 复制 | `feathers_in_3d/src/pick.rs` + `main.rs` observers | ✅ |
| **rig-ecs 真实 agent/turn 数据** | `rig_niri/src/kernel.rs` + `batch.rs` | ✅ |
| **真实闸门（Ask/Decision）** | `kernel.rs::payment_gate` + `answer_to_decision`/`answer_ask` | ✅ |
| **增量刷新 / 流式** | `batch.rs::Transcript`(keyed) + `main.rs::sync_rows` | ✅ 前缀原地改、只重建后缀 |
| **相机操作架（手柄模拟移动端）** | `camera_rig.rs`（`RF_RIG=1`） | ✅ 手柄+键盘，无实体手柄待真机验证 |
| **CJK 子集化** | `tools/make_cjk_charset.py` + `fetch_cjk_font.sh` | ✅ 30.7 MB → 3.2 MB |
| **真实模型接入** | — | ❌ 未做（本机无 API key；接缝已留：`ReplyStream`） |
| **模板化（多面板/状态/输入/主题）** | `panels.rs`/`status.rs`/`editor.rs`/`theme.rs` | ✅ 本轮完成 |
| **开发范文档** | `../AGENT_3D_PATTERN.md` | ✅ 本轮完成 |

流程（已实现）：

```
rig_ecs::AgentPlugin（同一个 bevy world）
  ├─ spawn_root(AgentId("main"))            → 一个 root agent
  ├─ add_tool(SearchProducts / CreateOrder) → 真工具（async，跑在 bevy 任务池）
  └─ spawn ToolRequest{call} + ForAgent     → 内核推进 Admit→Prepare→Gate→Start→…

Stage::Gate
  ├─ payment_gate：create_order 未决 → spawn Ask(商品/单价/数量/合计/店铺) + HoldOf(effect)
  └─ answer_to_decision：Ask 有 Outcome → 确认则放行（hold 释放），取消则 Outcome::refused

batch.rs
  ├─ record_calls（Stage::Review）：结算前把 (ToolCall, ToolResult) 抄进 CompletedCalls 历史
  ├─ rebuild_batch：历史(Completed) + 现场(Waiting/Running/Held) → RenderBatch
  ├─ run_looks：world.run_schedule(TranscriptRender) 跑 look
  └─ collect_rows：LookLine → Row，并把未回答的 Ask 追加成可点行（AskChoice）
```

**两个必须知道的坑**：
1. 效应实体**结算后会被回收**——转写必须自己在 `Stage::Review` 把结果抄走（`CompletedCalls`），否则历史消失；
2. `ReadyToStart` 是**过滤类型别名**（不是组件），`Has<ReadyToStart>` 会编译不过；
3. Ask 只问一次：给效应打 `AskAsked` 标记，否则每个 pass 都会再问一遍。

---

## 5. 本轮任务的两个卡点与思路

### 5.1 真实 agent/turn 数据

- rig-ecs 的 `AgentPlugin` 与我们的 app **同为 bevy 0.20.1**，可以直接 `add_plugins(AgentPlugin)` 共享 world；
- 不需要大模型就能跑工具链路：spawn 一个 `ToolRequest { call: ToolCall }`（`#[require(Effect)]`）+ `ForAgent(agent)`，
  内核会把它推进 `Admit → Prepare → Gate → Start → Arrive → Review → Commit` 各阶段；
- 工具用 `AppToolsExt::add_tool` 注册（`rig-ecs/src/tools.rs:271`），future 跑在 Bevy 的 async compute pool；
- 前端每帧从效应实体读状态构造 `ToolRender`：
   - 有 `Outcome` → `Done`（+ 结果文本）；
   - `ReadyToStart` 且未决 → `Waiting`；
   - 被 `Holds` 按住 → `Held(原因)`；
   - 其余 → `Running(None)`。

### 5.2 真实闸门

- 在 `Stage::Gate` 加系统：对 `create_order`（或所有写工具）spawn `Ask::yes_no(...).body(订单摘要)` + `HoldOf(effect)`；
- 前端需要 spawn 一个 `Answerer`（`rig_harness::pick`/`ask` 的 front 约定），把"未回答的 Ask"画成模态；
- 用户点确认/取消 → `Answer::choice("confirm").outcome()` 写 `Decision::decide(Decider::Person, …)`；
- `ask.rs` 文档明确这是官方姿势；`Decision` 先到先得。

---

## 6. 版本管理与提交

jj 仓库（`research/`）。本会话 `~/.config` 只读，用包装脚本：

```sh
./jjx st / describe -m "..." / new / log
```

`bevy_luma` 的本地改动（alpha_mode 固定 Premultiplied）是有意的，别回滚。

---

## 7. 接续清单（按顺序）

1. **真实模型**：`rig-core` 的 provider（需要 API key）+ 把流式 delta 写进 `ReplyStream`；
   服务端把 token 流经模型调用接进 `RenderBatch` 的 assistant 条目；
2. **移动端**：`camera_rig` 的 yaw/pitch 换成设备姿态（Android `ndk::sensor`、iOS `objc2-core-motion`、
   wasm `DeviceOrientationEvent`），键盘/手柄作为兜底；
3. **真机验证**：手柄（本机无设备）与 90 Hz 性能；
4. 字库再瘦身（按业务词表子集）/ 或换 SDF 文本解决放大发糊；
5. 性能：面板级合批、`text3d` 只在可见区间重建。

## 7.5 rig（真 TUI 基线）已装好

```
<repo>/.cargo-home/bin/rig    # rig 0.47.0（cargo install rig --locked）
```

用途：做"完整替换 rig-harness UI"的**对照基线**。

```sh
export RIG_HOME=$PWD/.rig-home          # 隔离会话目录
export PATH=$PWD/.cargo-home/bin:$PATH
rig --version                            # rig 0.47.0
rig                                      # 需要真 TTY；首次会 rig build（编译 agent 工程）
script -qec "rig" /dev/null | cat -v     # 无 TTY 时抓 ANSI 输出，可当文本基线
rig -p --json "hello"                    # headless（无 provider 时会报缺 key，正好确认接缝）
```

注意：`rig` 首次运行会**生成并编译 agent 工程**（`RIG_HOME` 下），需要几分钟；
没有 provider/key 时它仍能启动 TUI 并显示未配置状态——这正是我们要对齐的界面。

## 8. 已知限制（不要重复怀疑）

- 位图文字约 6× 放大后发糊（`04_text_zoom.png`），要任意距离清晰需 SDF 文本；
- 裁剪只到面板边界（面板即视口），没有嵌套裁剪；
- 边缘字形是**剔除**不是切割，滚到一半的行可能整行消失；
- 剪贴板默认走进程内缓冲，写系统剪贴板要开 `bevy_clipboard/system_clipboard` feature；
- 软件渲染（llvmpipe），性能数字无参考价值。
