# ShowAgent：看别的会话（子 agent / 已保存会话 / 被压缩条目 / 重启前的调用事实）

> **状态（2026-11-26 核对）：S1/S2 已实现，S3/S4 已实现，S5/S6 未做。**
>
> | 步 | 内容 | 状态 | 证据 |
> |---|---|---|---|
> | S1 | 来源挂到转写窗格（`TranscriptPane`）+ `ActiveTranscript` + 每窗格自己的 `PaneScroll` | ✅ | `pane.rs:613`（组件）、`:662`（`ActiveTranscript`）、`:816`（`window_transcript` 逐窗格窗口化） |
> | S2 | `dialogue_batch_of` + `batch::source_rows`；`Pane2` 不再镜像、按来源各画各的 | ✅ | `batch.rs:929`/`:2044`；`pane.rs:905`/`:921`；`mirror_pane2_rows` 已不存在 |
> | S3 | `/show` 入口 + picker + 标题行；主/侧窗格各带来源 | ✅ | `show.rs:43`–`:47`、`:77`（`show`）、`:293`（`source_picker`）；`pane.rs:757`（`title_row`）、`:952`（`window_pane`） |
> | S4 | 存档会话：`saved_sessions` + `read_conversation` → `TranscriptSource::Value` | ✅ | `show.rs:252`（`open_session`）、`:326`（`saved_sessions`）、`:696`（临时 home 的端到端测试） |
> | S5 | 压缩条目 `Replaced` 的显示 | ⬜ 未做 | 全仓没有 `Replaced` 消费路径；`batch.rs:2643` 的测试仍明确断言**不画** `N messages above were compacted` |
> | S6 | 重启前的调用事实落盘/续号 | ⬜ 未做 | 没有 `calls.jsonl`/`continue_after` 调用点；`batch.rs:106` 仍是 `TODO(past)` |
>
> **入口与原文不同（按实际实现）**：S3 用的是斜杠命令 `/show`（无参数弹 picker），
> **没有** `Alt+A` / `Alt+W`；`/show shown` 复位来源，但**没有**"关掉侧窗格"的键。
> 标题行是窗格第一行 dim 文字，`title = None` 时不占行（`pane.rs:954` 起）。
>
> **已删除的早期判断**：原文把"本 app 没有子 agent 生产者"当成缺口。现在不这么写——
> "看别的会话"不依赖本前端造子 agent：`/show` 能看任何活 agent（包括别的工作区）和存档会话；
> 以后有 `/subagent` 或插件造出子 agent，同一个入口直接可用。
>
> 用户原话（`docs/status/REMAINING.md` §6「看不了别的会话」，本文写作时在 `:147`）：「看不了别的会话：转写永远只显示当前 agent
> 的当前对话：不能切到子 agent 看它干了什么，不能把已保存的会话读进来对照，压缩（compaction）
> 移出对话的条目直接消失，重启后旧调用卡片没有时长/调用树，拆窗（Alt+S）只是把同一份转写复制
> 成两列。」
>
> 本文 §1 保留了 **S1 之前**的检查记录（作决策历史），其中与现状不符的段落已按实现
> 补注/改写；当前实现以顶部状态表、§2 的"已实现"标注和代码为准。行号口径 2026-11-26；
> 函数名/类型名是稳定锚。

> 必须同时成立的产品约束（违反即作废）：

1. **条带里没有弹层。** 一切可见物都是 tile（`docs/status/WORKSPACE_MODEL.md` §8）；
2. **一个工作区 = 一个 agent 会话 = 一条 niri 条带**，多个工作区在 3D 里垂直堆叠、绝对坐标；
3. 聚焦**不隐藏**其它工作区，不做斗兽场/圆柱体总览；
4. 3D 相机自由，几何不跟相机；
5. 能复用上游 `rig_tui::` 的 `pub` 类型/函数就复用，不为"自研"重写。

> **行号口径**：`rig_*` 的 `文件:行` 按 2026-11-26 工作树核对；函数名/类型名是稳定锚。
> 上游 `reference/` 里的 0.47 源码不在改动范围，行号固定。

---

## 0. 结论（TL;DR）

1. ✅ **已实现**：来源从"全局一个 `ShownAgent`"改成"每个转写 tile 自带
   `rig_tui::TranscriptSource`"。`Shown` = 这条工作区的当前来源，`Agent(e)` = 某个活
   agent，`Value(c)` = 一份存档对话（只读）。组件是 `TranscriptPane`（`pane.rs:613`），
   三态直接用上游 `TranscriptSource`，没有自造枚举。
2. ⚠️ **实现了"条带内换来源"，但没有"移动 `ShownAgent`"**：S2/S3 之后 `/show` 只改活动
   窗格的 `source`/`title`，**不移动全局 `ShownAgent`**（`show.rs:16`–`:20` 的模块注释、
   测试 `show.rs:514`）。`Shown` 来源仍走全局真管线，`Agent`/`Value` 走 `source_rows`
   （`batch.rs:2044`）。侧窗格（`Alt+S` 拆出的 `Pane2`）带独立来源，可同时看两份。
3. ✅ **已实现（S4）**：已保存会话读成 `Value`。数据链就是原文列的公开 API：
   `Home::saved_sessions()` → `StoreOpener::open` → `read_conversation`；入口
   `show.rs:252 open_session`，picker 的会话列表 `show.rs:326 saved_sessions`。
   端到端测试用临时 `RIG_HOME` 建会话再 `/show session <id>`（`show.rs:696`）。
4. ⬜ **未实现（S5）**：压缩条目仍不画。上游的 `Replaced`/`summarized_in_log`/`compacted_line`
   路径没有接；`batch.rs:2643` 的测试仍断言**不凭空画** `N messages above were compacted`。
5. ⬜ **未实现（S6）**：重启前的时长/调用树仍没有。`PastEffects` 从 crate 外用不了，
   自建 `calls.jsonl` + `Effects::continue_after` 也没做；`batch.rs:106` 仍是 `TODO(past)`。
6. **S6 仍是本设计最危险的一步**（落盘 + effect id 续号）：它动内核资源初始化顺序与会话写入。

---

## 1. 现状：拿得到什么、拿不到什么

### 1.1 名字的实际位置（S1 之后要分开看）

任务描述把 `WorkspaceAgents`/`show_focused_agent`/`sync_workspace_transcripts` 记在
`rig_niri/src/workspaces.rs`。**复核：不在那里**，它们在应用层
`rig_harness_in_3d/src/workspaces_demo.rs`；`rig_niri::workspaces` 的 `Workspace` 只装
几何/会话 id（`workspaces.rs:87`–`:97`）。

但"S1 之前 `rig_niri` 完全不认识来源"这句话**现在已经不成立**：来源是挂在转写根上的
`TranscriptPane`（`pane.rs:613`），`Pane2` 也带自己的来源。工作区 ↔ agent 的绑定仍留在
应用层：

| 名字 | 真实位置 | 它做什么 |
|---|---|---|
| `WorkspaceAgents` | `rig_harness_in_3d/src/workspaces_demo.rs:158` | `HashMap`：工作区实体 ↔ agent 实体（双向，`agent_of`/`workspace_of`） |
| `bind_workspace_agents` | 同上 `:227` | 每个未绑定工作区绑一个**真** agent；session 0 收编主 agent |
| `show_focused_agent` | 同上 `:318` | 焦点变化时把 `ShownAgent` 指向该工作区的 agent（自带 `Local` 去抖） |
| `sync_workspace_transcripts` | 同上 `:373` | 非聚焦工作区：`dialogue_batch` + `live_additions` → 自己的 `WorkspacePanels.rows[Transcript]` |

`rig_niri/src/workspaces.rs` 的 `Workspace` 仍然只有几何与面板状态，没有 agent 字段：

```rust
// rig_niri/src/workspaces.rs:87
pub struct Workspace {
    pub session: u64,       // :89  会话 id 的 u64 占位
    pub tree: layout::Node, // :91  niri 分裂树
    pub pose: Transform,    // :93  绝对世界位姿
    pub order: usize,       // :95  整理顺序
}
// :758 WorkspacePanels { rows, roots, spawned, areas }
// :961 apply_focus：聚焦切换时把组件状态装回全局工作副本
```

**结论（S1 之后）**：`rig_niri` 认识"这个窗格画谁的对话"（`TranscriptPane.source`），
但"一个工作区绑定哪个 agent"仍是应用层的 `WorkspaceAgents` + 全局 `ShownAgent`。S1–S4
没有把 `WorkspaceAgents` 搬进 `rig_niri`，也不需要搬。

### 1.2 转写管线现在怎么选 agent（`rig_niri/src/batch.rs`）

| 环节 | 位置 | 选择口径 |
|---|---|---|
| `rebuild_batch` | `:599` | `let shown = turn.agent();` → `batch.agent = shown` |
| `ShownTurn::agent` | `:407` 起 | **只读全局 `ShownAgent`**：`self.shown.0`，且该实体真有 `Conversation` |
| 可见性过滤 | `focused_visible` `:369` | `owner == shown`，或 owner 为 `None`（app 级/更早历史） |
| 调用树/时长 | `CallHistory` `:112`、`CallFact` `:68` | 只来自**本进程** `record_calls`（挂在 `Stage::Review`） |
| 聚焦建行 | `collect_rows` `:1513` → `TranscriptRows` | 全局一份；`window_transcript` 再按窗格窗口化（`pane.rs:816`） |
| 非聚焦/非 Shown 建行 | `dialogue_batch` `:911`、`live_additions` `:1813`、`source_rows` `:2044` | 参数里**明确带 agent/source**（S2 已复用） |
| 重启后 | `:106` `TODO(past)` | 仍是空的：S6 未做 |
| 压缩说明 | 测试 `:2643` 起 | 仍明确断言**不画** `N messages above were compacted`（摘要本身照常画成 entry） |

S2 已经把它拆成 `dialogue_batch`（`:911`）和 `dialogue_batch_of`（`:929`）：
内层直接收 `rig_core::Conversation` + 可选 `Replaced`，所以存档/侧窗格建行走的
就是 `source_rows`（`:2044`）→ `dialogue_batch_of`。

### 1.3 子 agent 关系怎么查（API 在；不影响本设计）

| 查什么 | API | 位置 |
|---|---|---|
| 子 → 父 | `SpawnedBy::parent()` | `rig-ecs-0.47.0/src/agent.rs:149`、`:157` |
| 父 → 子们 | `Spawned`（relationship target，`pub`；`RelationshipTarget::collection` 读） | `agent.rs:166` |
| 祖先链 | `Query<&SpawnedBy>::iter_ancestors(entity)` | `bevy_ecs-0.20.1/src/relationship/relationship_query.rs:135`（上游这样用：`rig-harness-0.47.0/src/main_agent.rs:110`） |
| 造子 agent | `Spawner::spawn(Origin::ChildOf(agent), AgentSpec{..})` / `spawn(world, …)` | `rig-ecs-0.47.0/src/spawn.rs:37`（`Origin`）、`:133`（`Spawner::spawn`）、`:227`（`spawn`） |
| "允许子 agent"开关 | `Subagents` 资源标记 | `spawn.rs:72` |
| 调用属于谁 | `ForAgent(pub Entity)` | `rig-ecs-0.47.0/src/stage.rs:324` |
| agent 名字 | `AgentId(pub String)` + `short()` | `agent.rs:76`、`:92` |
| turn 属于谁 | `TurnOf(pub Entity)` / `Turn` | `agent.rs:493`、`:504` |
| 谁在显示 | `ShownAgent(pub Option<Entity>)`；`CurrentAgent` **强制跟随 shown 的根** | `rig-harness-0.47.0/src/main_agent.rs:62`、`:101`（`follow_shown`）、`:89`（`keep_shown`） |

**现状**：看谁不依赖本前端造子 agent——S1–S4 的"看别的会话"已经能看任何活 agent
（`/show agent <id>`，`show.rs:208` 起）和存档会话（`/show session`，`show.rs:252`）。
以后有 `/subagent` 或插件造出子 agent，同一个入口直接可用；测试里需要子 agent 时用
`rig_ecs::spawn::spawn` 造。

### 1.4 已保存会话：公开服务齐全，且不用重开会话（S4 已实现）

| 需要 | API | 位置 |
|---|---|---|
| 列会话 | `Home::saved_sessions() -> Vec<(SessionId, SessionDir)>`（内部按 `SessionDir::is_saved` 过滤） | `rig-harness-protocol-0.47.0/src/lib.rs:592`、`:1001` |
| 按名字/id 找 | `Home::find(&str)` | 同上 `:612` |
| 打开某个会话的 store | `StoreOpener::open(&SessionDir) -> Arc<dyn Store>` | `rig-harness-0.47.0/src/host/session.rs:112` |
| 读某个 agent 的对话（不写、不折全量日志） | `read_conversation(&dyn Store, Option<&str>) -> (String, Conversation)` | `rig-memory-0.47.0/src/listing.rs:20` |
| 列表里显示什么 | `SessionMeta { title, updated, messages, model, … }::read` | `rig-memory-0.47.0/src/meta.rs:32`、`:53` |
| 运行中会话的 store（无需再 open） | `SessionStore(pub Arc<dyn Store>)` 资源 | `rig-ecs-0.47.0/src/session_log.rs:184` |
| 会话级插件文件（落盘用） | `PluginData::of(&StoreOpener, &SessionDir)`；`PluginData` 每 run 自动插入 | `rig-harness-0.47.0/src/plugin_data.rs:73`、`host/session.rs:319`–`:345` |
| 当前会话路径 | `SessionPaths(pub SessionDir)` | `host/session.rs:55` |

**注意**（上游原文 `host/session.rs:82`–`:83`）：*"Opening, reading and writing a store may block
the frame: do it in commands and at startup, as `/resume` does."* 所以列表/预览必须**异步**读，
不能在每帧系统里同步 `open()`。

**S4 实际做法**：`/show` 是斜杠命令（`show.rs:77`），列表/读取都在命令系统里同步做完
（一次 `/show` 只读最近 8 个会话的 `SessionMeta` + 被选中的一个 store，不在每帧系统里）；
端到端测试见 `show.rs:696`。没有做后台任务/`SavedSessionsLoaded` 消息——只在命令里做。

### 1.5 压缩（`Replaced`）：数据在磁盘上，类型能复用，行生成不能（S5 未实现）

- 上游类型：`pub struct Replaced(pub BTreeMap<MessageId, Vec<conversation::Entry>>)`，是**挂在
  agent 实体上的组件**（`rig-tui-0.47.0/src/transcript.rs:723`；重导出 `lib.rs:105`）。
  上游把它当"这个 agent 的转写要额外画的条目"（`render.rs:522` 的 `LayoutSources.agents`
  查询 `Option<&Replaced>`；`pane.rs:213` 用 `Changed<Replaced>` 触发重排）。
- 数据源（存档/历史）：`rig_memory::compaction::summarized_in_log(&dyn Store, log) ->
  BTreeMap<MessageId, Vec<Entry>>`（`rig-memory-0.47.0/src/compaction.rs:227`–`:256`）——
  **产出的就是 `Replaced` 的载荷**，键是摘要条目的 id，值是被它换掉的条目。
- 行生成：上游 `render::compacted_line` 是**私有** `fn`（`render.rs:797`），
  `ShownEntries`/`archive` 是 `pub(crate)`（`transcript.rs:749`、`:786`）。→
  只能复刻那 3 行 dim 文本 + 递归展开（~40 行，见 §2.5）。
- **硬限制**：上游对 `TranscriptSource::Value` 的窗格**不画** archived 条目
  （`render.rs:463`–`:470` 的 `Showing::Value` 走 `ShownConversation::value`，`:352`–`:378`
  里 `replaced` 只有 agent 路径才填）。所以"存档里也能看见被压缩掉的条目"是**我们要比上游多做
  的一点**，不是抄不到。
- **另一个硬限制**：本 app **从不压缩**——`grep -rn "compact" rig_niri/src rig_harness_in_3d/src`
  只命中 `transcript.rs:960`/`:978` 的 notice 分类与 `batch.rs` 的测试（含 `:2164` 那条
  "压缩摘要本身照常画"的新测试）；没有 `/compact` 命令，也没有 `apply_summary` /
  `prepare_compaction` 的调用点。所以活的 `Replaced` 今天**产生的机会为零**；测试必须自己构造
  `Conversation::splice`（`summarized_in_log` 那条路）或直接插一个 `Replaced` 组件。

### 1.6 重启前的调用事实（`PastEffects`）：类型够不着，数据也没人落盘（S6 未实现）

| 事实 | 证据 |
|---|---|
| `PastEffects(Arc<Settled>)` 只有 `new/len/is_empty` 三个 `pub` 方法 | `rig-tui-0.47.0/src/transcript.rs:434`–`:457` |
| 载荷 `Settled` 是 `pub(crate)`，字段私有 | `transcript.rs:271` |
| 唯一消费它的 `note_effects` 是 `pub(crate)` | `transcript.rs:462` |
| 所以 crate 外**构造了也用不上**：读不到事实，也交不进调度 | 上面三条合起来 |
| `EffectRecorded(pub EffectRecord)` 是**消息**，只在内存 | `rig-ecs-0.47.0/src/stage.rs:599`、`:693` |
| `EffectRecord` 是公开的，字段齐全（`id`/`kind`/`outcome`/`parent`/`started_ms`/`call`/`reviewed`/`turn`） | `rig-core-0.47.0/src/effect/mod.rs:798`、`:829`、`:846`、`:851` |
| 依赖树里**没有** `rig-basics`（上游唯一写 effect log 的 `telemetry` 在它里面） | `Cargo.lock` 的 `rig-*` 只有 core/derive/ecs/harness/harness-protocol/http/markdown/memory/reqwest/tools/tui/tungstenite |
| effect id **不跨 run**（"Not saved: effect ids do not outlive a run"） | `rig-ecs-0.47.0/src/agent.rs:171` |
| 续号入口现成：`Effects::continue_after(last: Option<EffectId>)` | `rig-ecs-0.47.0/src/effects.rs:55` |

**结论**：`PastEffects` 这条上游路**彻底不通**。要"重启后有 `took` 和调用树"，只能自己把
`CallFact`（`batch.rs:63`，已含 `effect`/`parent`/`owner`/`agent`/`call`/`result`/`took`）落盘，
并用 `continue_after` 保住 id 单调（否则新 run 从 0 开始，会和旧记录撞）。

### 1.7 拆窗（Alt+S）：S1/S2 后已不是镜像

| 事实 | 位置 |
|---|---|
| `PanelId::Pane2` 的根也挂 `TranscriptPane`（默认 `source = Shown`、`title = None`） | `frame.rs:1107`–`:1109`、`pane.rs:613` |
| `mirror_pane2_rows` **已删除**；`Pane2` 不再抄 `PanelRows[Transcript]` | 全仓 grep 为空 |
| `window_transcript` 遍历所有带 `TranscriptPane` 的转写根，每块用自己的 `scroll`/`below`/`source` 窗口化 | `pane.rs:816`–`:936`；`window_pane` `:952` |
| `split_transcript` 拆树 + spawn 面板 + 建行槽，并把 `ActiveTranscript` 指过去 | `frame.rs:1136`–`:1156` |
| 键位：`Alt+S` 已注册；重复拆是空操作 | `frame.rs:1160`–`:1186` |
| 滚动/选中状态是每窗格一份 `TranscriptPane` 字段 | `pane.rs:613`–`:626` |

**结论**：拆窗后两列可以各有来源/滚动/选中；`title = None` 时行流与拆窗前逐行一致
（测试 `pane.rs:2928`/`:3022`）。**没有做**的是 `Alt+W` 关窗（见 §2.4）。

### 1.8 需求 → 数据源 → 现状（一张表看完）

| 用户要的 | 数据拿得到吗 | S1–S4 之后 |
|---|---|---|
| 切到子 agent 看它干了什么 | ✅ `Conversation`/`CallHistory`/`live_additions` 都按 agent 参数化 | ✅ `/show agent <id>` + picker（`show.rs:208`/`:293`）；可看任何活 agent（包括别的工作区） |
| 已保存会话读进来对照 | ✅ `Home::saved_sessions` + `StoreOpener` + `read_conversation` | ✅ `/show session <id 或 name>` → `Value`（`show.rs:252`）；只读 |
| 被压缩掉的条目还在 | ✅ `summarized_in_log` → `BTreeMap<MessageId, Vec<Entry>>` | ⬜ S5 未做；`Value` 不画 archived，仍不画压缩标记 |
| 重启后有 `[took]`/调用树 | ❌ 没有任何落盘 | ⬜ S6 未做；`batch.rs:106` 仍是 `TODO(past)` |
| 拆窗各有各的来源 | ✅ `PanelId`/`PanelRoots` + `TranscriptPane` 已就绪 | ✅ S1/S2 已做：每窗格独立 `source`/`scroll`/`below`；镜像已删 |

---

## 2. 设计方案（S1–S4 已按此落地，S5/S6 仍是计划）

> 下面保留原设计文字，但**已实现的部分按代码修正**；入口与原文的 `Alt+A`/`Alt+W`
> 不同，实际是 `/show`（§2.4）。

### 2.1 核心：来源是 tile 的属性，复刻上游的 `TranscriptSource`（已实现）

组件实际放在 `pane.rs`（不是新模块），挂在转写面板的根实体上（根实体由
`frame::spawn_panel`（`frame.rs:1053`）创建，每个工作区一份 `WorkspacePanels.roots`）：

```rust
// 实际：rig_niri/src/pane.rs:613
use rig_tui::TranscriptSource; // rig-tui-0.47.0/src/pane.rs:84，pub 且已重导出（lib.rs:87）

#[derive(Component, Clone, Debug)]
pub struct TranscriptPane {
    /// 这块 tile 画谁。语义与上游同名概念一一对应，类型直接复用。
    pub source: TranscriptSource,
    /// 窗格第一行的标题；`None` = 不占行。
    pub title: Option<String>,
    /// 这块窗格自己的滚动（窗顶 + 是否跟随末尾）。
    pub scroll: PaneScroll,
    /// 这块窗格自己的条目选中（`Selection::item` 的块序号）。
    pub selected: Option<u32>,
    /// 这块窗格自己的"下面还有"状态（Pane2 与主窗格各记各的）。
    pub below: BelowSeen,
}
```

**与原文的差异**：没有 `replaced` 字段（S5 未做，压缩替换表还没地方放）；另加了
`below`（每窗格自己的 `↓ more below`/`↓ new output` 状态，`pane.rs:622`）。

三个变体与产品语义的映射：

| 上游变体（`pane.rs:84`–`:95`） | 在我们的世界里什么意思 | 渲染路径 |
|---|---|---|
| `Shown` | "这条工作区的会话"：默认 = `WorkspaceAgents` 绑的 agent；`/show` **不移动 `ShownAgent`**，所以主窗格选 `Agent(e)` 时不再是 `Shown` | 聚焦工作区：**真管线**（`rebuild_batch`→`run_looks`→`collect_rows`）；非聚焦：`dialogue_batch`/`live_additions` |
| `Agent(e)` | 明确的某个活 agent（子 agent、或别的工作区的 agent），可并发看两份 | `dialogue_batch`/`live_additions`（不跑 look） |
| `Value(c)` | 已保存会话的对话，**只读快照**，没有活动部分 | `source_rows` → `dialogue_batch_of`（无 `Replaced`，S5 未做） |

**为什么复用上游类型而不是自造枚举**：它是 `pub`、`Clone`、已从 `rig_tui` 根导出
（`lib.rs:87`），三态语义与我们要的一字不差；自造会让以后对齐上游多一层翻译。
`PartialEq` 没有，所以"变了吗"用 `Changed<TranscriptPane>` 判定，和上游用
`Changed<TranscriptPane>` / `Changed<Replaced>` 同构（`pane.rs:213`）。

### 2.2 子 agent：条带内换来源，**不**新建工作区（已实现，与原文 A 有差异）

实际选的是"条带内换来源"，但 **`/show` 不移动 `ShownAgent`**：它只改活动窗格的
`source`/`title`（`show.rs:77`–`:135`，测试 `show.rs:514` 明确断言"不移动 `ShownAgent`"）。
因此原文方案 A 的代价（输入框跟着发给子 agent、`Deliver` 改道）**没有发生**：
`Shown` 来源的窗格仍走全局管线，输入框仍发给本工作区的 agent。

| 方案 | 现状 |
|---|---|
| **A 条带内换来源** | ✅ 采用，但改的是 `TranscriptPane.source`，不是 `ShownAgent`。主窗格可以看 `Agent(sub)`，输入框不受影响 |
| B 新工作区显示子 agent | ❌ 仍否：`bind_workspace_agents` 会给未绑定工作区 `spawn_root` 一个新 root；子 agent 短命，为它生/灭条带会打断空间记忆 |
| C 只读第二窗格，主窗格永不切 | ✅ 现在的推荐用法之一：`Alt+S` 拆出 `Pane2`，`/show agent <id>` 只改活动窗格；主窗格保持 `Shown` |

结论：A 与 C 现在可以并用——想完整看一个 agent 就把主窗格切到 `Agent`，想保留
"当前会话"就把侧窗格切过去。`CurrentAgent` 仍跟随根（上游 `main_agent.rs:101`），
`/fork`、rewind 的操作对象不变。

### 2.3 渲染路径（谁画什么）——已实现

```
任一转写窗格（Transcript / Pane2，每个根带 TranscriptPane）：
  source = Shown          -> 全局 TranscriptRows（真管线已写好的整段；有工作区时只取当前工作副本）
  source = Agent(e)       -> batch::source_rows(world, &Agent(e), width)
  source = Value(c)       -> batch::source_rows(world, &Value(c), width)
然后 window_pane 按**这块窗格自己的**面积/scroll/below 窗口化，title 有值时插第一行。
```

- **范围**：`window_transcript` 只处理**当前工作副本**的根（聚焦工作区）；非聚焦工作区由
  应用层 `sync_workspace_transcripts`（`workspaces_demo.rs:373`）用 `dialogue_batch` +
  `live_additions` 直接写自己的 `WorkspacePanels.rows[Transcript]`，只有 `Transcript` 一块，
  没有 `Pane2`/`Agent`/`Value`。
- 落地位置：`window_transcript`（`pane.rs:816`–`:936`）遍历所有带 `TranscriptPane` 的根；
  `source_rows`（`batch.rs:2044`）按 `TranscriptSource` 取整段；`dialogue_batch_of`
  （`batch.rs:929`）是存档/侧窗格的内层纯函数；`window_pane`（`pane.rs:952`）做窗口化。
- 原文"三个小改动"都已完成：`dialogue_batch_of` 拆层（`:929`）、`source_rows` 搬进
  `rig_niri::batch`（`:2044`）、每窗格窗口化（`pane.rs:816`）。
- **没有 `resolve_shown_agent`**：`/show` 不移动 `ShownAgent`，"Shown 解析"这件事不存在。
- 侧窗格不跑 look（`RenderBatch` 全局只有一份）；`Agent`/`Value` 来源用
  `live_additions` 的实时部分（`:1813` 起），卡片朴素但能看到在飞调用。

### 2.4 进入 / 退出——已实现（只有 `/show`）

| 入口 | 作用 | 落在哪 |
|---|---|---|
| `Alt+S` | 拆窗（已有） | `Pane2` 出现，默认 `source = Shown`、无标题，并被设为活动窗格（`frame.rs:1136`–`:1156`） |
| `/show` | 无参数：弹 picker（`shown` + 每个活 agent + 最近 8 个存档会话） | `show.rs:77`–`:100`；picker `show.rs:293` |
| `/show shown`（也认 `workspace`/`default`/`current`） | 恢复本工作区的默认来源并清标题 | `show.rs:114`–`:118` |
| `/show agent <id>` / `/show <id>` | 看某个活 agent（完整 id / 短 id / 唯一名字） | `show.rs:208`–`:232` |
| `/show session <id 或 name>` | 读存档会话为 `TranscriptSource::Value`（只读） | `show.rs:252`–`:290` |
| `Esc` | **不参与退出**：仍是关补全 / 交还输入框 / 停 turn | `show.rs:22`–`:23` 的模块注释 |

原文计划的 `Alt+A`（picker）与 `Alt+W`（关侧窗格）**都没有实现**。现在：
- 打开 picker 就是 `/show`（无参数），不需要新键位；
- 没有"关掉 `Pane2`"的命令/键；`/show shown` 只复位来源，不删面板；
- 原文说"存档只能进侧窗格"，**实际没有这个限制**：`apply_source` 对活动窗格不区分
  主/侧，`/show session` 也能落在主窗格；它只把该窗格变只读，输入框仍走 `Shown`；
- picker 项的命令由 rig-harness 的 pick 观察者执行（item `command` 不带前导 `/`），
  所以 `show agent <id>`/`show session <id>` 和手打 `/show` 走同一条命令系统。
- **标题行**：`TranscriptPane.title = Some(..)` 时 `window_pane` 在窗口第一行插 dim 标题
  （`pane.rs:757` `title_row`、`:991`–`:992` 插入、`:954` 起说明），`item_rows` 跳过它，
  正文少放一行；`title = None` 时行数与 S2 逐行一致（测试 `pane.rs:3022`）。

### 2.5 被压缩的条目（`Replaced`）：数据两条，画法一处（S5 未实现，下面是计划）

> 本节是**未实现计划**；里面的行号/上游结构是设计时的锚，不保证与当前代码仍对得上。

**数据从哪来**

| 场景 | 来源 | 时机 |
|---|---|---|
| 活的 agent（将来有 `/compact` 时） | 帧间差集：上一帧 `conversation.entries()` 的 `id` 集合 − 这一帧的，配上新出现的摘要条目 id | `rebuild_batch` 里顺手做（~25 行），写 `Replaced` 组件到 agent |
| 已保存会话（`Value`） | `summarized_in_log(&*store, &log)` | 打开会话时（和 `read_conversation` 同一个异步任务，共用一次 `open`） |
| 重启后（resume） | 同上，`log` = 该 agent 的 `AgentId` | Startup 异步 |

`summarized_in_log` 的返回值类型**就是** `Replaced.0` 的类型，直接 `Replaced(map)`，不转换。

**画在哪、怎么画**：在 `conversation_frame`（`batch.rs:759`）里，遍历 `conversation.entries()`
之前插一段"先画被替换的条目，再画摘要条目"的展开，等价于上游 `ShownEntries::new/archive`
（`transcript.rs:760`–`:817`）的复刻。上游是 `pub(crate)`，我们照语义重写：

```
for entry in conversation.entries():
    stands_for = archive(replaced, entry.id)   // 递归：被替换的条目里还可能有更早的摘要
    if stands_for > 0: 先 push 归档条目，再 push 一行 dim "── N message(s) above ... ──"
    push entry 本身
```

- 文本照抄上游 `compacted_line`（`render.rs:797`–`:806`）：
  `── 1 message above was compacted ──` / `── N messages above were compacted ──`。
- 行的 key 用 `row_hash`（`rig_niri/src/transcript.rs:745`）即可，归档条目不需要"可选中/可 rewind"
  （上游也说了 archived 没有 item：`transcript.rs:1483` 注释）。本次不做 archived 的选中。
- 失效：`Changed<Replaced>` → 清 `Transcript` 引擎的对应块（引擎已有 `remove` `:587`、
  `retain_text` `:607`）。

**不做的**：不画"被压缩条目的调用树"（`transcript::PastEffects` 那条还是空的），
不做 rewind/undo 到 archived 条目（需要 `MessageId` 与对话修改权限，超出只读视图）。

### 2.6 重启前的调用事实：自己落盘（`PastEffects` 的替代，S6 未实现）

> 本节同样是**未实现计划**，行号只作历史参考。

**格式**：一行一个 JSON 对象，落在会话的 `PluginData.session` 里（`plugins/` 之下，
`plugin_data.rs:73`；`PluginData` 每个 run 自动插入，`host/session.rs:319`）：

```jsonc
// plugins/rig_niri/calls.jsonl
{"v":1,"effect":17,"parent_effect":9,"parent_call":"call_abc","call":"call_def",
 "name":"search_products","agent":"helper","owner_agent_id":"helper",
 "started_ms":1764...,"took_ms":1834,"ok":true,"args":"{...}"}
```

- **为什么存 `parent_call`**：`EffectId` 不跨 run（`agent.rs:171`），跨 run 还原调用树只能靠
  `CallId`（`ToolCall.id`，稳定、进对话、进日志）。`parent_effect` 只在本 run 内有意义，
  供同一 run 内的重启补齐。
- **为什么存 `owner_agent_id`**：`owner: Option<Entity>` 是实体，不跨 run；重启后按
  `AgentId` 反查实体（`Query<(Entity, &AgentId)>`）。
- **续号**：Startup 读完日志后，若 app 自己构造 `Effects`，用
  `Effects::continue_after(Some(max_effect))`（`effects.rs:55`）。否则新 run 的 id 从 0 起，
  和日志里的旧 id 撞，`CallTree` 会把两棵树接错。**这一条是 S6 的风险点**，要单独测。

**装载**：Startup（或第一次需要时）异步读完全部行 → `CallHistory.0` 前置（旧的在前，保持
`ordinal`/结算顺序），并给每个 fact 把 `owner` 补成重启后的实体。

**存档会话（`Value`）没有效果可读**：`read_conversation` 只还原对话，没有 effect 记录。
存档里每张卡片只能画对话里已有的 `ToolCall`/`ToolResult`（今天就是这样），**没有 `took`**。
这一点必须在 UI 上诚实：存档的卡片不画时长，而不是画 `0ms`。

### 2.7 多窗格各带来源（已不再镜像，S1/S2 已实现）

> 实际落地：S1 就把 `PaneScroll` 收进 `TranscriptPane` 并加了 `ActiveTranscript`；
> S2 同时删掉 `mirror_pane2_rows`、改成每窗格独立窗口化。所以下面 1–4 都已完成。

1. 删掉 `mirror_pane2_rows` 与两处调用（已完成：全仓 grep 为空）。
2. `Pane2` 的根实体挂 `TranscriptPane { source: Shown, … }`；`split_transcript`
   （`frame.rs:1136`）在 spawn 时挂上，并把 `ActiveTranscript` 指向它（`frame.rs:1153`–`:1156`）。
3. `window_transcript` 变成"遍历所有带 `TranscriptPane` 的转写根"：每个根各自算高度
   （`pane_size` 泛化成 `pane_size_of(root)`，`pane.rs:711`）、各自窗口化、各自写
   `PanelRows[这块的 PanelId]`。
4. 每窗格自己的 `scroll`/`selected` 在组件里（§2.1），`pane_keys`/`pane_wheel`/
   `select_pane_item` 从"全局 `PaneScroll`"改成"活动窗格的那一份"：
   - `pane_keys`（`pane.rs:1081`）→ 目标 = 活动窗格；
   - `pane_wheel`（`:1320`）→ 目标 = 指针下的那个窗格（`wheel_panel` `:1283` 已能把指针换算成
     `PanelId`，加一档 `Pane2` 即可）；
   - `Selection` 已经按 `PanelId` 分桶（`selection.rs:122`–`:141`），不用改结构。
5. 对齐不变量：**主窗格的 `PanelId::Transcript` 行为逐字节不变**（`title = None` 时不插标题行）。

### 2.8 复用清单（约束 5 的交代）

| 上游 API | 能复用吗 | 证据 |
|---|---|---|
| `rig_tui::TranscriptSource` | ✅ 直接当来源类型 | `pane.rs:84`，重导出 `lib.rs:87` |
| `rig_tui::Replaced` | ✅ 直接当组件/载荷 | `transcript.rs:723`，重导出 `lib.rs:105` |
| `rig_tui::ShowAgent` | ✅ 语义照搬（我们自己的事件/直接设资源） | `view.rs:49`；上游处理 `view.rs:736`–`:760` |
| `rig_memory::compaction::summarized_in_log` | ✅ 返回值即 `Replaced` 载荷 | `compaction.rs:227` |
| `rig_memory::listing::read_conversation` | ✅ 存档对话 | `listing.rs:20` |
| `Home::saved_sessions` / `find` | ✅ 会话列表 | protocol `lib.rs:592`/`:612` |
| `StoreOpener::open` / `PluginData::of` | ✅ store 与插件文件 | `host/session.rs:112`、`plugin_data.rs:73` |
| `Effects::continue_after` | ✅ 续 effect id | `effects.rs:55` |
| `rig_tui::text_rows` | ✅（已在用） | `batch.rs:802` 已在用 |
| `render::compacted_line` | ❌ 私有 `fn` | `render.rs:797` |
| `transcript::ShownEntries`/`archive` | ❌ `pub(crate)` | `transcript.rs:749`/`:786` |
| `transcript::PastEffects`/`note_effects` | ❌ 载荷与消费者都 `pub(crate)` | `transcript.rs:271`/`:434`/`:462` |
| `pane::TranscriptPane` 的绘制 | ❌ 绑死 ratatui `TuiPanel`/`TuiSystems::Draw` | `pane.rs:57`–`:64`、`:210`（`lay_out_panes`）、`render.rs:482` |

---

## 3. 不变量（写测试就照抄这几条）

1. 一个工作区的 `Transcript` 窗格**任何时候**都只有一个来源；换来源只换"行从哪来"，
   **不改任何 `pose`、不改任何 `Node` 尺寸、不新增/删除任何面板实体**（拆窗除外）。
2. `title = None` + `source = Shown` 时，`PanelRows[Transcript]` 与改动前**逐行相等**。
3. 存档来源（`Value`）**只读**：不产生 `Deliver`、不写 `Conversation`、不产生 `CallHistory`。
4. 主窗格换到 `Agent(sub)` 后，`ShownAgent` **不变**、`CurrentAgent` 仍是根、输入框的
   `Deliver` 目标仍是本工作区 agent（S3 的实现口径，测试 `show.rs:514` 锁死）。
5. 关侧窗格：**未实现**。`/show shown` 只复位来源；没有"删 `Pane2`"的命令/键，所以没有
   "关掉后 `PanelRows`/`FrameTree` 里不再有 `Pane2`"这条回归可写。
6. 压缩条目：**S5 未实现**。以后实现时也只能在"它对应的摘要条目仍存在"时画出（上游同上：
   `Replaced` 的条目没了就不画，`transcript.rs:717`–`:722`）。
7. 没有任何弹层：本设计新增的可见物只有 tile 内的行（标题行）+ 条带最右的 picker 列。

---

## 4. 实施拆分（每步可独立提交）

| 步 | 内容 | 状态 | 与原文的差异 |
|---|---|---|---|
| **S1** | `TranscriptPane` + `ActiveTranscript` + `PaneScroll` 降级成每窗格字段 | ✅ 已实现 | `Pane2` 没有停在镜像：S2 一起删了镜像 |
| **S2** | `dialogue_batch_of` + `batch::source_rows` + 每窗格窗口化；删 `mirror_pane2_rows` | ✅ 已实现 | `source_rows` 在 `batch.rs:2044`；`dialogue_batch_of` 在 `:929` |
| **S3** | 入口 + picker + 标题行 + 活动窗格 | ✅ 已实现 | 入口是 `/show`（`show.rs:43`），**没有 `Alt+A`/`Alt+W`/`ShowTarget`**；标题行在 `pane.rs:757`/`:952` |
| **S4** | 存档会话 → `Value` 来源 | ✅ 已实现 | 没有 `session_view.rs`；列表/读取都在 `show.rs` 的命令系统里同步做（`show.rs:252`/`:326`） |
| **S5** | `Replaced` 压缩条目 | ⬜ 未实现 | 无 `Replaced` 消费路径；测试仍断言不画压缩标记（`batch.rs:2643`） |
| **S6** | `CallFact` 落盘 + `continue_after` | ⬜ 未实现 | 无 `calls.jsonl`/`continue_after`；`batch.rs:106` 仍 `TODO(past)` |

### S1 详细（✅ 已实现；"Pane2 仍镜像"是当时的中间状态，S2 已删）
- 组件与 `ActiveTranscript` 进 `pane.rs`（与 `PaneScroll` 同文件，避免新模块太早）。
- `frame::spawn_panel` 给 `Transcript`/`Pane2` 挂默认 `TranscriptPane`；`split_transcript`
  挂 `Pane2` 的默认值并设活动。
- 主窗格的 `window_transcript` 改成读写"主窗格那一份 `scroll`"；`Pane2` 仍然由
  `mirror_pane2_rows`（`panels.rs:150`）填——所以这一步跑完，画面上一个像素都不该变。
- 测试（无窗口）：
  - 拆窗后两列**逐行相等**（镜像语义没被破坏）；
  - 主窗格 PgDn 后 `PanelRows[Transcript]` 变、`PanelRows[Pane2]` 跟着变（同一份）；
  - `title = None` 时行数/行 key 与改动前相等；
  - `拆窗幂等`（连按 Alt+S 仍是两个窗格，`frame.rs:953` 已有守卫）。

### S2 详细（✅ 已实现）
- 顺序：先拆 `dialogue_batch_of`（纯函数，测试直接喂 `rig_core::Conversation`）→ 再搬
  `source_rows`（搬完 `workspaces_demo::sync_workspace_transcripts` 变成 ~40 行的循环）→
  接 `Pane2`（挂来源 + 独立窗口化）→ 最后删 `mirror_pane2_rows` 与两处调用。
- 测试：
  - `source_rows(&Value(c))` 的文本等于 `c` 里所有用户/assistant 文字的拼接，且**没有**
    `CallState` 行（没有活动部分）；
  - `source_rows(&Agent(sub))` 含子 agent 的在飞调用（借用 `workspaces_demo` 的
    `NeverReturns` 测试夹具，`workspaces_demo.rs:573`）；
  - `Pane2.source = Shown` 时 `Pane2` 行 == 主窗格**整段**行流（`TranscriptRows`）按它自己
    高度窗口化的结果——拆窗后不滚动时与主窗格逐行相等（证明镜像→独立窗口化不改变内容），
    滚动后与主窗格不同（证明它真的独立了）；
  - `Pane2` 与非聚焦工作区的行**互不影响**（两个 agent 各一条，文本不串）。

### S3 详细（✅ 已实现；入口是 `/show`）
- `/show` 用 `In<CommandArgs>`；解析 `args` 第一词 = 子命令（`show.rs:77`–`:135`）。
- 无参数时由 `show` 系统直接 `commands.spawn(picker)`（`show.rs:94`–`:99`），picker 在
  `source_picker`（`:293`）里组装：`shown` + 每个活 agent + 最近 8 个存档会话。
- picker 项的命令由 rig-harness 的 pick 观察者执行，不需要应用层自己接；条目
  `command` 不带前导 `/`。
- 设来源只改活动窗格的 `TranscriptPane`（`apply_source`/`apply_agent`，`show.rs:208`/`:234`），
  不移动 `ShownAgent`。
- 测试：`show.rs:479`（命令注册 + 无参 picker）、`:514`（选 agent 改窗格、`shown` 恢复）、
  `:565`（两窗格各带来源独立窗口化）、`:638`（标题行）、`:696`（存档会话）。
- **没做**：`Alt+A`/`Alt+W`/`ShowTarget`/`resolve_shown_agent`；关侧窗格没有入口。

### S4 详细（✅ 已实现；列表/读取同步在命令里做，没有 `AsyncComputeTaskPool`）
- 读会话列表在**命令系统**里同步做（上游明说会阻塞帧，`host/session.rs:82`；这里接受
  一次命令的延迟）：`saved_sessions`（`show.rs:326`）读 `SessionMeta`，被选中的会话才
  `open` + `read_conversation`（`show.rs:266` 起）。没有 `AsyncComputeTaskPool` /
  `SavedSessionsLoaded` 消息。
- 标签只用 `SessionMeta`（标题/消息数/更新时间）+ 目录名，不 fold 全日志。
- 测试：临时 `RIG_HOME` 建会话 + 写消息 → `/show session <id>` → 侧窗格文本含那两条、
  且 `ShownAgent` **不变**（`show.rs:696`，只读不抢输入）。

### S5 详细（⬜ 未实现）
- `Replaced` 挂 agent 实体（与上游同构，`transcript.rs:723`）；`Value` 的来源把它放在
  `TranscriptPane.replaced`（因为 `Value` 没有 agent 实体可挂）。
- **不推翻**并行落地的那条测试 `batch.rs:2163`–`:2199`
  （`a_compaction_summary_gets_no_invented_message_count`）：没有 `Replaced` 时仍然一条标记
  都不画——这条测试正是本设计要保住的"不凭空编数"。
- 测试：
  - 构造 `Conversation` + `Replaced{BTreeMap}` → 行流里出现 `── 2 messages above were compacted ──`
    且归档文本在摘要**上方**、顺序为归档→空行→标记→摘要（对齐 `render.rs:780`–`:790`）；
  - 摘要条目被删（`Replaced` 里的键不存在了）→ 一行都不画；
  - 递归：归档条目里还有摘要 → 计数是 `max(1, 内层)` 的和（`transcript.rs:797`–`:810`）。

### S6 详细（⬜ 未实现，L，最危险）
- **为什么 L**：它要 ① 在 `Startup` 抢先于内核用 `Effects::continue_after` 换资源；
  ② 在 `Stage::Review` 的 `record_calls`（`batch.rs:1875` 注册、`:405` 定义）旁边追加写盘；③ 定义 `PluginData`
  路径与格式；④ 处理"写盘失败只降级、不 panic"。任何一条顺序错都表现为"调用树接错/时长丢失"。
- 测试：
  - 结算一次调用 → `files.read_file("rig_niri/calls.jsonl")` 有一行，`took_ms` 非空；
  - 新建一个 App、把文件塞进 `MemoryStore` 造的 `PluginData` → `CallHistory` 有同一 `CallId`
    的 fact，`parent_call` 关系还原；
  - `Effects::last_id` 续到日志最大值（否则断言"新调用 id > 旧最大 id"）；
  - 日志坏行/截断 → 跳过该行，其余照常（不 panic）。

---

## 5. 这次不做

| 不做 | 为什么 |
|---|---|
| 3D 总览（斗兽场、圆柱、环、两级视角） | `docs/design/SPATIAL_UI.md` 已明确废弃；本设计不引入任何新几何 |
| 任何弹层/浮窗/overlay | 产品约束 1；picker 是条带最右列（`picker.rs` 的既有做法），标题是 tile 里的一行 |
| 聚焦即隐藏其它工作区 | 产品约束 3；`focus_workspace` 只决定内容写哪（`workspaces.rs:870`） |
| 为子 agent 新建常驻工作区 | 见 §2.2 方案 B；会破坏"一条条带 = 一个会话"的绑定 |
| 第 3、第 4 个转写窗格（N panes） | `PanelId::Pane2` 是单值；N 窗格要重做 `PanelId` 的键空间（`panels.rs:66`），不是这次的风险预算 |
| 侧窗格跑业务 look | 需要把 `RenderBatch`/look 缓存从单例改成每来源一份（`batch.rs:2318`），L 级，留到"多来源完整管线" |
| 压缩之后的 rewind / undo / fork | 只做"看得见"，不做"改得动"（archived 条目不可选中，对齐上游 `transcript.rs:1483`） |
| 由本前端**产生**子 agent（`/subagent` 工具/命令） | 本次是"看得见"；生产者属于业务工具层。测试用 `rig_ecs::spawn::spawn` 造 |
| 由本前端**执行**压缩（`/compact`） | 上游 `prepare_compaction`/`apply_summary` 在 `rig-memory`，接命令是另一件事；本设计只消费它留下的记录 |
| 存档会话里的时长/调用树 | 存档里没有 effect 记录（§2.6），不画假数字 |
| 跨会话 diff / 合并 | 只做"并排对照"，不做对齐/合并算法 |

---

## 6. 验收标准（全部无窗口，可用 ECS 断言）

> **状态**：1–6、11、12 已满足（S1–S4）；**7–10 未满足**（S5/S6 未实现）。
> 第 2 条按实际实现改过：`/show` **不移动 `ShownAgent`**。

1. **来源在 tile 上**：拆窗后 `app.world().get::<TranscriptPane>(pane2_root).source` 等于设进去的值
   （测试 `pane.rs:2978`、`show.rs:565`）；`ShownAgent` 只在 `Shown` 来源的解析里用。
2. **`/show` 只改活动窗格**：`/show agent <id>` 后该窗格 `source = Agent(entity)`，
   `ShownAgent.0` **不变**（测试 `show.rs:514` 明确断言）；`CurrentAgent.0` 仍是根
   （`follow_shown`，`main_agent.rs:101`）。
3. **子 agent 的行**：`Pane2.source = Agent(sub)` 后，`WorkspacePanels.rows[Pane2]` 的文本含
   sub 的对话与在飞调用名，**不含**父 agent 的调用名。
4. **存档只读**：`/show session <id>` 后侧窗格文本含存档消息；`ShownAgent` 不变；
   `Conversation` 的 `messages().len()` 不变；没有 `Deliver` 消息产生。
5. **存档无 live 部分**：`Value` 行流里 `CallState::Running` 计数为 0，`Ask` 行计数为 0。
6. **逐字节回归**：`title = None` + `source = Shown` 时，一个有三条消息 + 一次调用的 App，
   `PanelRows[Transcript]` 的 `(key, hash)` 序列与改动前的快照相等（`hash` 就是增量 diff
   用的那个 `SpawnedRows::hashes`，`panels.rs:124`；比它不比像素）。
7. **压缩标记**：构造 `Conversation` 前两条 + 摘要 + `Replaced` → 行流里恰好一行 dim 的
   `── 2 messages above were compacted ──`，且它在归档文本之后、摘要之前。
8. **压缩缺席**：删掉摘要条目 → 行流里既没有归档文本也没有那行标记。
9. **重启事实**：结算一次调用 → `PluginData.session` 的 `calls.jsonl` 有一行；
   用同一份文件新建 App → `CallHistory` 有同一 `CallId`、`took.is_some()`、
   `parent` 关系与落盘一致。
10. **续号**：装载过日志的 App 里，下一次结算的 `EffectId` 大于日志中的最大值
    （`Effects::continue_after` 生效，`effects.rs:55`）。
11. **关窗复原**：原文的 `Alt+W` **没有实现**（没有关 `Pane2` 的键/命令）。
    `/show shown` 只把来源复位成 `Shown` 并清标题，不删面板；`title = None` 时
    `PanelRows[Transcript]` 的行序列与拆窗前相等（测试 `pane.rs:3022`）。
12. **没有弹层**：全仓库 `grep -r "Placement::Over\|Placement::Within" rig_niri/src` 仍为空
    （沿用 `WORKSPACE_MODEL.md` §8 的验收口径）。

---

## 7. 开放问题（不阻塞本次落地，但要有人拍板）

1. **主窗格显示子 agent 时，输入框要不要变？** 已按实现回答：`/show` 不移动 `ShownAgent`，
   所以输入框**不变**，仍发给本工作区的 agent；窗格标题写明在看谁。不需要灰掉输入。
2. **`/show session` 打开存档后，`Ctrl+S`/复制是否要写回？** 本设计：不写回，存档纯只读。
3. **一个子 agent 结束后 Pane2 该显示什么？** 本设计：保持来源不变，标题行加 `(ended)`，
   内容留在原地（空间记忆：用户没关就不动）。不自动切回、不自动关窗。
4. **effect 日志的量**：`calls.jsonl` 每结算一次追加一行，长会话可能上万行；S6 里要先做
   "只读最后 N 天/最多 N 行"的上限，再谈全量。
