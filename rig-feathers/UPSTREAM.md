# 跟进 rig 上游（rig-tui / rig-ecs / rig-core）

**目标**：rig-feathers 是 rig-tui 的并行前端，必须能随上游升级。原则是——
**只依赖公开 API，不 fork**；上游改了内部（`renderers::add` 那种），我们自己补那一层。

## 当前锁定

```toml
bevy = "=0.20.1"        # rig-harness 精确钉 0.20.1
bevy_feathers = "=0.20.1"
rig-tui = "=0.47.0"
rig-core = "=0.47.0"
rig-ecs = "=0.47.0"
rig-harness = "=0.47.0"
bevy_luma = { path = "../bevy_luma" }   # 上游 git main（0.20）
```

升级顺序：**先把五个 rig 版本一起抬**（它们同版本发布），再抬 bevy（如果上游抬了）。
`bevy_luma` 只用于 `UNodeMaterial3d` + `UnivisRenderPlugin`，bevy 变了要跟着重新拉源码。

## 哪些是我们的边界（上游变动最可能撞到这里）

| 我们用到的上游公开 API | 用途 | 上游若改 |
|---|---|---|
| `RenderBatch` / `RenderInput` / `RenderKey` / `RenderRequest` / `RenderOutput` | 自建 look 驱动 | 改 `batch.rs` 的构造 |
| `RenderSystems::{Render,Decorate,Edit}` / `TranscriptRender` | 注册 look 的调度 | 改 `BatchPlugin::build` |
| `ToolRender` / `CallState` / `LookLine::colored()` | look 的输入与输出 | 改 `batch.rs` 的 look |
| `rig_core::message::{ToolCall, ToolFunction, ToolName, CallId, ToolResult}` | 构造会话数据 | 改 `batch.rs` / demo |
| `rig_ecs::{AgentPlugin, spawn_root, ToolRequest, ForAgent, Decision, Outcome, Stage, HoldOf, Holds}` | 内核与闸门 | 改 `kernel.rs`（demo） |
| `rig_harness::{Ask, Answer, Answerer, Answerers, Pick, Chosen, Picker, Front}` | 问答与选择器 | 改 `front.rs` / `picker.rs` / demo |
| `bevy_text::{TextLayoutInfo, ComputedTextBlock}` | 3D 字形 | 改 `feathers_in_3d/text3d.rs` |
| `bevy_ui::{ComputedNode, UiGlobalTransform}` | route B 坐标 | 改 `feathers_in_3d/world3d.rs` |

**不属于边界**：rig-tui 的 `transcript`/`pane`/`render`/`screen`/`terminal`（我们不用）。

## 升级检查清单

1. `cargo update -p rig-tui -p rig-ecs -p rig-core -p rig-harness --precise <新版本>`（五个一起）；
2. `cargo build` —— 先看边界表里的类型是否还在；`LookLine::colored()` 是否仍是公开的；
3. 跑 `cargo run --bin demo`（`BEVY_ASSET_ROOT=$PWD`），检查巡游 8 张图：
   - 转写里工具卡片/闸门选项仍在；
   - `[gate] holding ... → person answered "confirm" → order placed` 日志还在；
4. `render/rf_shots/*.png` 与上一版对比（作为回归基线）；
5. 上游若新增前端能力（新的面板/状态项/选择器类型），按 `docs/design/AGENT_3D_PATTERN.md` 的扩展点补。

## 上游还没给的能力（我们自己补的）

| 缺什么 | 我们的做法 |
|---|---|
| `renderers::add` 是 `pub(crate)`，无法注册 look 驱动 | `batch.rs` 自建 `RenderBatch` + 手动 `world.run_schedule(TranscriptRender)` |
| 没有"feathers 前端"样例 | 本仓库 |
| 效应实体结算后回收 | `record_calls` 在 `Stage::Review` 抄历史 |
| picker 没有前端实现参考（只有 TUI 的） | `picker.rs`：可点行 → `Pick::person` |
