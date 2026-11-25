# ShowAgent：看别的会话（子 agent / 已保存会话 / 被压缩条目 / 重启前的调用事实）

> 用户原话（`docs/status/REMAINING.md` §6「看不了别的会话」，本文写作时在 `:147`）：「看不了别的会话：转写永远只显示当前 agent
> 的当前对话：不能切到子 agent 看它干了什么，不能把已保存的会话读进来对照，压缩（compaction）
> 移出对话的条目直接消失，重启后旧调用卡片没有时长/调用树，拆窗（Alt+S）只是把同一份转写复制
> 成两列。」
>
> **这份文档是纯设计：只定方案，不改任何代码。** 落地按 §4 的小步提交。
>
> 必须同时成立的产品约束（违反即作废）：

1. **条带里没有弹层。** 一切可见物都是 tile（`docs/status/WORKSPACE_MODEL.md` §8）；
2. **一个工作区 = 一个 agent 会话 = 一条 niri 条带**，多个工作区在 3D 里垂直堆叠、绝对坐标；
3. 聚焦**不隐藏**其它工作区，不做斗兽场/圆柱体总览；
4. 3D 相机自由，几何不跟相机；
5. 能复用上游 `rig_tui::` 的 `pub` 类型/函数就复用，不为"自研"重写。

> **行号口径**：本文所有 `文件:行` 按 2026-11-25 22:50 的工作树核对。写这份文档期间有并行改动
> 正在落 `REMAINING.md` §8/§9（`pane.rs` 的 `↓ new output`、`picker.rs` 的选项键、`batch.rs`/
> `transcript.rs` 的 `[stopped]` 等），它们会推动行号；**函数名/类型名才是稳定锚**，
> 行号对不上时按名字找。上游 `reference/` 里的 0.47 源码不在改动范围，行号固定。

---

## 0. 结论（TL;DR）

1. **来源从"全局一个 `ShownAgent`"改成"每个转写 tile 自带 `rig_tui::TranscriptSource`"**。
   `Shown` = 这条工作区的当前来源（默认 = 它绑定的 agent），`Agent(e)` = 某个活 agent，
   `Value(c)` = 一份存档对话（只读）。这正是上游 `TranscriptPane` 的三态（`pane.rs:84`–`:95`），
   类型可直接用，不必自造枚举。
2. **子 agent 用"条带内换来源"，不新建工作区。** 主窗格换来源 = 移动 `ShownAgent`
   （上游 `ShowAgent` 语义，`view.rs:49`），于是**整条 look/调用树/流式管线原样复用**；
   侧窗格（`Alt+S` 拆出来的）带一个独立来源，可以同时看父子。
3. **已保存会话只进侧窗格。** 数据链是公开 API：`Home::saved_sessions()`
   （`rig-harness-protocol-0.47.0/src/lib.rs:592`）→ `StoreOpener::open`
   （`rig-harness-0.47.0/src/host/session.rs:112`）→
   `rig_memory::listing::read_conversation`（`rig-memory-0.47.0/src/listing.rs:20`）。
4. **压缩条目：类型复用上游 `rig_tui::Replaced`，数据不自己造**——存档侧读
   `rig_memory::compaction::summarized_in_log`（`compaction.rs:227`），活的会话侧用帧间条目 id
   差集。上游把行生成函数 `compacted_line`/`ShownEntries::archive` 都关在 `pub(crate)`
   （`render.rs:797`、`transcript.rs:749`/`:786`），这两处只能复刻 ~40 行。
5. **重启前的时长/调用树：上游 `PastEffects` 从 crate 外用不了**（内部 `Settled` 与
   `note_effects` 都是 `pub(crate)`，且依赖里没有任何东西把 `EffectRecorded` 落盘）。
   要自己把 `CallFact` 写进会话的 `PluginData`，并用 `Effects::continue_after` 续上 effect id。
6. **最危险的一步是 S6**（落盘 + effect id 续号）：它动内核资源初始化顺序与会话写入。

---

## 1. 现状：拿得到什么、拿不到什么

### 1.1 三个名字的实际位置（先纠正一处）

任务描述把 `WorkspaceAgents`/`show_focused_agent`/`sync_workspace_transcripts` 记在
`rig_niri/src/workspaces.rs`。**复核：不在那里**，它们在应用层 `rig_harness_in_3d/src/workspaces_demo.rs`；
`rig_niri::workspaces` 只管面板状态的按工作区拆分，**完全不认识 agent**：

| 名字 | 真实位置 | 它做什么 |
|---|---|---|
| `WorkspaceAgents` | `rig_harness_in_3d/src/workspaces_demo.rs:150` | `HashMap`：工作区实体 ↔ agent 实体（双向，`agent_of`/`workspace_of`） |
| `bind_workspace_agents` | 同上 `:219` | 每个未绑定工作区绑一个**真** agent；session 0 收编主 agent（`primary_root_agent` `:296`） |
| `show_focused_agent` | 同上 `:310` | 焦点变化时把 `ShownAgent` 指向该工作区的 agent（自带 `Local<Option<Entity>>` 去抖，`:325`） |
| `sync_workspace_transcripts` | 同上 `:365` | 非聚焦工作区：`dialogue_batch`（`:408`）+ `live_additions`（`:413`）→ 自己的 `WorkspacePanels.rows[Transcript]` |

而 `rig_niri/src/workspaces.rs` 里只有**几何与面板状态**：

```rust
// rig_niri/src/workspaces.rs:79
pub struct Workspace {
    pub session: u64,      // :81  会话 id 的 u64 占位
    pub tree: layout::Node,// :83  niri 分裂树
    pub pose: Transform,   // :85  绝对世界位姿
    pub order: usize,      // :87  整理顺序
}
// :119 WorkspacePanels { rows, roots, spawned, areas } —— 没有 agent / 没有来源
// :321 apply_focus：聚焦切换时把组件状态装回全局工作副本
```

**结论**：`rig_niri` 里"工作区 → 它显示谁的对话"这件事**根本不存在**。它是应用层的一张私有表
（`WorkspaceAgents`）+ 一个全局资源（`ShownAgent`）。这就是本次要补的地基：来源必须变成
**rig_niri 认识的一等值**，而且必须挂在 tile 上，而不是资源上。

### 1.2 转写管线现在怎么选 agent（`rig_niri/src/batch.rs`）

| 环节 | 位置 | 选择口径 |
|---|---|---|
| `rebuild_batch` | `:504` | `let shown = turn.agent();`（`:528`）→ `batch.agent = shown`（`:529`） |
| `ShownTurn::agent` | `:353`–`:357` | **只读全局 `ShownAgent`**：`self.shown.0`，且该实体真有 `Conversation` |
| 可见性过滤 | `focused_visible` `:307` | `owner == shown`，或 owner 为 `None`（app 级/更早历史） |
| 调用树/时长 | `CallHistory` `:92`、`CallFact` `:63` | 只来自**本进程** `record_calls`（`BatchPlugin` `:1875` 挂在 `Stage::Review`） |
| 聚焦建行 | `collect_rows` `:1207` → `TranscriptRows`（`pane.rs:61`） | 全局一份，`PanelId::Transcript` 专用 |
| 非聚焦建行 | `dialogue_batch` `:734`、`live_additions` `:1491` | 纯函数/独占系统，参数里**明确带 agent**（可复用！） |
| 重启后 | `:89` `TODO(past)` | 自认"旧的 `took` 与调用树是空的" |
| 压缩说明 | 测试 `:2163`–`:2199` | 明确断言**不画** `N messages above were compacted`（摘要本身照常画成 entry） |

`dialogue_batch` 的签名是 `(&Conversation, agent: Entity, width)`（`:734`），但它内部调
`conversation_frame(conversation: &talk::Conversation, …)`（`:768`）——**内层类型已经是
`rig_core` 的 `Conversation`，不是 ECS 组件**。这意味着"给一份存档对话建行"只差一层包装
（见 §2.3，改动约 6 行）。

### 1.3 子 agent 关系怎么查（上游确实有，但本 app 没有生产者）

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

**硬限制**：本 app **没有任何地方造子 agent**——`grep -rn "Origin::" rig_niri/src rig_harness_in_3d/src`
为空（`spawn::Spawner` 也从没被 import；`grep "Spawner"` 的命中全是 bevy_ui 的
`ChildSpawnerCommands`），只有 `spawn_root` 被用来造 root agent；`kernel.rs` 只注册
`search_products`/`create_order` 两个工具。
`rig_niri/src/title.rs:58` 与 `:73` 用 `With<SpawnedBy>` 判断"这是子 agent 的 turn"，
说明**关系读得到，但今天没有子 agent 可读**。所以"看子 agent"的对象要靠插件/未来的
`/subagent` 命令产生；本次设计只负责"有了就能看"，测试里用 `rig_ecs::spawn::spawn` 直接造。

### 1.4 已保存会话：公开服务齐全，且不用重开会话

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

### 1.5 压缩（`Replaced`）：数据在磁盘上，类型能复用，行生成不能

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

### 1.6 重启前的调用事实（`PastEffects`）：类型够不着，数据也没人落盘

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

### 1.7 拆窗（Alt+S）现在只是镜像

| 事实 | 位置 |
|---|---|
| `PanelId::Pane2` 的注释就写着"每帧显示与 `Transcript` 完全相同的可见行" | `rig_niri/src/panels.rs:46`–`:48` |
| `mirror_pane2_rows` 把 `PanelRows[Transcript]` 整份复制给 `Pane2` | `panels.rs:150`–`:157`（全局调用 `:146`，每个工作区调用 `:296`） |
| `split_transcript` 只拆树 + spawn 面板 + 建行槽 | `rig_niri/src/frame.rs:948`–`:970` |
| 键位：`Alt+S` 已注册（`FramePlugin` 自己装：`frame.rs:1032`） | `frame.rs:975`–`:979`、`:996` |
| 滚动状态只有**一份**全局 `PaneScroll` | `rig_niri/src/pane.rs:130`–`:137`；`window_transcript` `:273` 只写 `PanelId::Transcript`（`:315`） |

**结论**：拆窗没有"每个窗格各自的来源/滚动/选中"，因为 `PaneScroll` 是全局 `Resource`，
而 `Pane2` 的行是抄来的。这两件要一起改（§2.7）。

### 1.8 需求 → 数据源 → 现状（一张表看完）

| 用户要的 | 数据拿得到吗 | 卡在哪 |
|---|---|---|
| 切到子 agent 看它干了什么 | ✅ `Conversation`/`CallHistory`/`live_additions` 都按 agent 参数化 | 来源是全局的，没有入口；且本 app 没生产者 |
| 已保存会话读进来对照 | ✅ `Home::saved_sessions` + `StoreOpener` + `read_conversation` | 没有 `Value` 渲染路径，没有 picker，读 store 会阻塞帧 |
| 被压缩掉的条目还在 | ✅ `summarized_in_log` → `BTreeMap<MessageId, Vec<Entry>>` | 本 app 不压缩；`compacted_line` 私有；`Value` 不画 |
| 重启后有 `[took]`/调用树 | ❌ 没有任何落盘 | `PastEffects` 不可用，要自建 |
| 拆窗各有各的来源 | ✅ `PanelId`/`PanelRoots` 已就绪 | `PaneScroll` 全局 + `mirror_pane2_rows` |

---

## 2. 设计方案（推荐）

### 2.1 核心：来源是 tile 的属性，复刻上游的 `TranscriptSource`

新增一个组件（**只在 `rig_niri`**，不碰上游），挂在转写面板的根实体上（根实体由
`frame::spawn_panel` `:887` 创建，每个工作区一份 `WorkspacePanels.roots`）：

```rust
// 新增：rig_niri/src/panes.rs（或 pane.rs 里，与 PaneScroll 同文件）
use rig_tui::TranscriptSource; // rig-tui-0.47.0/src/pane.rs:84，pub 且已重导出（lib.rs:87）

#[derive(Component, Clone, Debug)]
pub struct TranscriptPane {
    /// 这块 tile 画谁。语义与上游同名概念一一对应，类型直接复用。
    pub source: TranscriptSource,
    /// tile 内第一行的标题（= 上游 `TranscriptPane::title`，`pane.rs:63`）。
    /// `None` = 不占行，行为与今天完全一致。
    pub title: Option<String>,
    /// `Value` 来源的压缩替换表（活的 agent 从它自己的组件读，见 §2.5）。
    pub replaced: Option<Arc<rig_tui::Replaced>>,
    /// 这块 tile 自己的滚动（把全局 `PaneScroll` 降级成组件）。
    pub scroll: PaneScroll,
    /// 这块 tile 自己的条目选中（`Selection::item` 的块序号）。
    pub selected: Option<u32>,
}
```

三个变体与产品语义的映射：

| 上游变体（`pane.rs:84`–`:95`） | 在我们的世界里什么意思 | 渲染路径 |
|---|---|---|
| `Shown` | "这条工作区的会话"：默认 = `WorkspaceAgents` 绑的 agent；若用户在主窗格换成了子 agent，则 = 那个子 agent（因为 `ShownAgent` 被移动了） | 聚焦工作区：**真管线**（`rebuild_batch`→`run_looks`→`collect_rows`）；非聚焦：`dialogue_batch`/`live_additions` |
| `Agent(e)` | 明确的某个活 agent（子 agent、或别的工作区的 agent），可并发看两份 | `dialogue_batch`/`live_additions`（不跑 look） |
| `Value(c)` | 已保存会话的对话，**只读快照**，没有活动部分 | `dialogue_batch_of` + 可选 `Replaced` |

**为什么复用上游类型而不是自造枚举**：它是 `pub`、`Clone`、已从 `rig_tui` 根导出
（`lib.rs:87`），三态语义与我们要的一字不差；自造会让以后对齐上游多一层翻译。
`PartialEq` 没有，所以"变了吗"用 `Changed<TranscriptPane>` 判定，和上游用
`Changed<TranscriptPane>` / `Changed<Replaced>` 同构（`pane.rs:213`）。

### 2.2 子 agent：条带内换来源，**不**新建工作区

三个候选，取舍如下：

| 方案 | 怎么做 | 优点 | 致命点 |
|---|---|---|---|
| **A（推荐）条带内换来源** | 主窗格的 `TranscriptSource::Agent(sub)` → 设 `ShownAgent = sub`；侧窗格随便挂 | 整条 look/调用树/流式管线**原样**复用（就是上游 `ShowAgent`）；与 niri 的"列"语义一致（父子并排 = 一次 split） | 输入框跟着走（`editor.rs:1096`/`:1177` 把 `Deliver` 送给 `ShownAgent`），打字会发给子 agent；`CurrentAgent` 会被 harness 拉回根（`main_agent.rs:101`） |
| B 新工作区显示子 agent | `spawn_workspace` + 绑 `sub` | "一条条带一个 agent"听起来干净 | **破坏不变量**：`bind_workspace_agents`（`:219`）给每个未绑定工作区 `spawn_root` 一个**新** root agent，子 agent 会被自己的新 root 顶掉；子 agent 短命，为它生/灭 3D 条带会打断空间记忆；`arrange_workspaces` 会把世界重排 |
| C 只读第二窗格，主窗格永不切 | 侧窗格 `Agent(sub)`，主窗格恒为 `Shown` | 输入永远在"你正在谈的那个" | 子 agent 的卡片**没有业务 look**（`dialogue_batch` 明确不跑 look，`batch.rs:720`–`:721`），也没有 look 的调用树；主窗格永远看不到子 agent 的全貌 |

**选 A，并把 C 作为 A 的补充**：主窗格换来源给你"完整看一个 agent"，侧窗格给你"同时对照两个"。
B 只保留一个例外（本次不做）：把某个子 agent **钉成常驻工作区**是另一个产品决定，见 §5。

**A 的代价要写明**：

- 打字发给子 agent 是**上游行为**（`ShowAgent` 后 `ShownAgent` 就是它），不是 bug；退出时
  必须回到本工作区的 agent，否则用户会一脸问号。→ tile 标题行常驻显示"在看谁"（§2.4）。
- `CurrentAgent` 仍是根（`follow_shown` 取 `iter_ancestors` 的最后一个），所以 `/fork`、rewind
  的操作对象不会跑到子 agent 上——这是好事，别去改。

### 2.3 渲染路径（谁画什么）

```
聚焦工作区的 Transcript 窗格
  source = Shown          -> 真管线（今天的 batch.rs 全链，不动）
  source = Agent(e)       -> 先设 ShownAgent = e —— 仍然走真管线（等价于上游 ShowAgent）
  source = Value(c)       -> 不允许（见下）

任何其它转写窗格（Pane2 / 非聚焦工作区的 Transcript）
  source = Shown, 该工作区聚焦   -> 直接窗口化全局 TranscriptRows（已带 look 的整段行流，零额外渲染）
  source = Shown, 该工作区非聚焦 -> source_rows(bound agent)（与今天 sync_workspace_transcripts 同一条）
  source = Agent(e)             -> batch::source_rows(world, &Agent(e), …)（dialogue_batch + live_additions）
  source = Value(c)             -> batch::source_rows(world, &Value(c), …)（只有对话 + 可选 Replaced）
```

规则一句话：**主窗格永远是"活的、你在谈的"那个；存档只能进侧窗格。** 这条规则保护
"输入框发去哪"不被存档搞乱，也让 `Value` 不需要"没有 agent 的 look 缓存"这种麻烦事。

**"设 `ShownAgent`"由谁做**：不是渲染系统，而是一个 `Prepare` 里的解析系统
（`resolve_shown_agent`，替换今天的 `show_focused_agent` `workspaces_demo.rs:310`），
输入是"聚焦工作区的主窗格来源"，输出是 `ShownAgent`：

| 聚焦工作区主窗格的来源 | `ShownAgent` | 其余窗格看到的 `Shown` |
|---|---|---|
| `Shown` | `WorkspaceAgents.agent_of(focused)` | 主窗格在看的那份（= 本工作区 agent） |
| `Agent(e)` | `e`（上游 `ShowAgent` 语义） | 主窗格在看的那份（= `e`） |
| `Value(c)` | 不允许（主窗格不能选存档项） | — |
| 非聚焦工作区 | 不参与（`ShownAgent` 永远指向聚焦那条） | 该工作区绑定的 agent |

它必须**同时**在"焦点变了"和"`Changed<TranscriptPane>`"两种情况下重算（今天只在焦点变化时
算，`workspaces_demo.rs:325` 的 `Local` 去抖就是那个限制）。

需要的三个小改动（都可单独提交、都是纯函数级）：

1. `dialogue_batch` 拆成两层（~6 行）：
   ```rust
   pub fn dialogue_batch(conversation: &Conversation, agent: Entity, width: u16)
       -> (RenderBatch, Vec<(TextId, Vec<Line<'static>>)>)
   { dialogue_batch_of(&conversation.0, Some(agent), None, width) }

   /// 存档/子 agent 用：内层 `rig_core::Conversation`，可选压缩替换表。
   pub fn dialogue_batch_of(
       conversation: &rig_core::conversation::Conversation,
       agent: Option<Entity>,
       replaced: Option<&rig_tui::Replaced>,
       width: u16,
   ) -> (RenderBatch, Vec<(TextId, Vec<Line<'static>>)>)
   ```
   （`conversation_frame` 本来就收内层类型：`batch.rs:768`；`rig_ecs::agent::Conversation`
   是 `pub` 的元组结构体 `agent.rs:209`，取 `.0` 合法。存档/侧窗格内部照旧传
   `&CallHistory::default()` / `&CallTree::default()`，和今天的 `dialogue_batch`
   `batch.rs:740`–`:744` 一模一样——存档没有本进程的调用事实。）
2. `source_rows`：把 `workspaces_demo.rs:365`–`:444` 里"按 agent 拼行 + 窗口化"的那段
   **搬进 `rig_niri::batch`**，参数从 `agent: Entity` 换成 `&TranscriptSource`。应用层只剩
   "遍历工作区、读自己的面积"。
3. 每个窗格的窗口化：`window_transcript`（`pane.rs:273`）从"只写 `PanelId::Transcript`"改成
   "对每个带 `TranscriptPane` 的根各写一份"，高度用 `pane_size` 的同一套（`pane.rs:217`；它由 `pane_rows` 改名而来，现在连列数一起返回）。

**为什么侧窗格不跑 look**：`run_looks` 跑的是整个 world 的 `TranscriptRender`，而
`RenderBatch` 全局只有**一份**（`batch.rs:1851` `init_resource::<RenderBatch>()`）。给侧窗格也
跑 look 就要把管线从"单例资源"改成"每来源一份"，那是 §4 的 S2 里最贵的一档。取舍：侧窗格
用默认行 + 调用树（`live_additions` 已带 `ForAgent` 过滤与树根判断，`:1573`–`:1590`），
**够看清楚子 agent 干了什么**，只是卡片长得朴素。

### 2.4 进入 / 退出

| 键 | 作用 | 落在哪 |
|---|---|---|
| `Alt+S` | 拆窗（已有，`frame.rs:975`） | `Pane2` 出现，`source = Shown`，并被设为**活动窗格** |
| **`Alt+A`（新）** | 给**活动窗格**换来源：打开 picker（picker 是条带最右一列，不是弹层） | 新模块 `rig_niri::panes` + 应用层 provider |
| **`Alt+W`（新）** | 关掉活动的侧窗格（`Pane2`）；没有侧窗格时把主窗格来源复位成 `Shown` | 同上 |
| `Esc`（已有） | 只保留今天的三个含义：关补全 / 面板作用域把键交还输入框（`editor.rs:1272`）/ 输入框里停 turn；`keys.rs:650` 是转写区 Esc 的 `Deselect`。**不加第四个含义** | 不动 |
| 鼠标点某窗格的行 | 把该窗格设为活动窗格 | 用 `RowLayout.panel_entity`（`panels.rs:68`–`:75`）；`selection.rs:684` 已有"行 → 工作区"的同款解析 |

**活动窗格**：一个 `ActiveTranscript(Option<Entity>)` 资源。优先级 = 最近一次拆窗/点击/在该窗格
里用 `Alt+↑↓` 的那个 → 否则 `PanelRoots[PanelId::Transcript]`。这样"不点也能用"，且没有新弹层。

**picker 的内容**（复用 `Picker` + `PickerQueue`，`picker.rs:104`、`:551`）：

```
标题：看谁的对话
  0  这条工作区（返回）                      command: show workspace
  1  helper        agent · 3 次调用 · 正在跑   command: show agent 42
  2  reviewer      agent · 已结束             command: show agent 57
  ── 已保存的会话（只读） ──
  3  昨天的比价 — 42 msgs · 2h 前             command: show session 1764...-1
  4  更多会话…                                command: show sessions
```

- 条目 `command` **不需要我们执行**：rig-harness 的 `pick` 观察者会把命中的条目命令按
  `run_chosen` 跑（`rig-harness-0.47.0/src/pick.rs:433`、`commands.rs:264`），
  所以"picker 项 → `/show …` 命令"这条链今天就是通的（这正是上游 `/agents` 的姿势）。
  注意 item 的 `command` 字段**不带前导 `/`**（`pick.rs:384` 里是 `format!("/{}", …)`）。
- 斜杠命令（应用层注册，`AppCommandsExt::add_command`，如 `lib.rs:307`/`:308` 的 `/hello`）：
  ```
  /show workspace            主窗格/活动窗格回到本工作区绑定的 agent
  /show agent <AgentId|short> 切到某个活 agent（子 agent 或别的工作区）
  /show session <id>         打开一个已保存会话（只读，且只能在侧窗格）
  /show close                关掉活动的侧窗格
  ```
  命令参数用 `In<CommandArgs>`（`commands.rs:49`），`args` 已 trim。
- **命令不认"窗格 id"**（上游命令是 agent 作用域的）。所以：`Alt+A` 在打开 picker 时把目标
  窗格记进 `ShowTarget(Entity)`；`/show` 解析顺序 = `ShowTarget`（还活着）→ 活动窗格 →
  主窗格。手打 `/show agent x` 时就是"活动窗格"。
- **目标是主窗格时，picker 不列存档那一组**（主窗格只能 `Shown`/`Agent`，见 §2.3）；
  `/show session` 落在主窗格时产生的是一条 `Notice`（"存档只能进侧窗格：先 Alt+S"），
  不是静默忽略。

**标题行**（无弹层的表达方式）：tile 内第一行，dim，例如

```
 helper · shown agent · 3 calls running        (source = Agent)
 workspace 2 · read-only · 42 msgs · 2026-11-24 20:10   (source = Value)
```

实现 = `window_transcript`（`pane.rs:273`）在窗口顶部插一行（和 `below_marker`
（`pane.rs:249`）/`panels.set`（`:315`）同一套做法），
行 key 用 `u64::MAX - 2`（`BELOW_MARKER_KEY` 是 `u64::MAX`，见 `pane.rs:242`；
`workspace_label_row` 用 `u64::MAX - 1`，`workspaces_demo.rs:457`），`item_rows` 跳过它。
**`title = None` 时不占行**，所以默认行为与今天逐字节一致。

### 2.5 被压缩的条目（`Replaced`）：数据两条，画法一处

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

### 2.6 重启前的调用事实：自己落盘（`PastEffects` 的替代）

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

### 2.7 多窗格各带来源（不再镜像）

> 落地顺序见 §4：**S1 只做 2 的组件与活动窗格（`Pane2` 仍镜像，行为零变化）**；
> 1/3/4 的切来源与独立窗口化在 S2 一起做。

1. 删掉 `mirror_pane2_rows`（`panels.rs:150`–`:157`）与两处调用（`:146`、`:296`）。
2. `Pane2` 的根实体挂 `TranscriptPane { source: Shown, … }`；`split_transcript`
   （`frame.rs:948`）在 spawn 时挂上，并把 `ActiveTranscript` 指向它。
3. `window_transcript` 变成"遍历所有带 `TranscriptPane` 的转写根"：每个根各自算高度
   （`pane_size` 泛化成 `pane_size_of(root)`，`pane.rs:217`）、各自窗口化、各自写
   `PanelRows[这块的 PanelId]`。
4. 每窗格自己的 `scroll`/`selected` 在组件里（§2.1），`pane_keys`/`pane_wheel`/
   `select_pane_item` 从"全局 `PaneScroll`"改成"活动窗格的那一份"：
   - `pane_keys`（`pane.rs:395`）→ 目标 = 活动窗格；
   - `pane_wheel`（`:593`）→ 目标 = 指针下的那个窗格（`wheel_panel` `:568` 已能把指针换算成
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
4. 主窗格换到 `Agent(sub)` 后，`ShownAgent == sub`、`CurrentAgent == 根`、输入框的 `Deliver`
   目标是 `sub`（上游语义，写测试锁死，别"修"它）。
5. 关掉侧窗格后，`PanelRows`/`PanelRoots`/`FrameTree` 里不再有 `Pane2`，且主窗格状态不变
   （与 `picker` 摘列的既有约定一致，`picker.rs:559`）。
6. 压缩条目只能在"它对应的摘要条目仍存在"时画出（上游同上：`Replaced` 的条目没了就不画，
   `transcript.rs:717`–`:722`）。
7. 没有任何弹层：本设计新增的可见物只有 tile 内的行（标题行）+ 条带最右的 picker 列。

---

## 4. 实施拆分（每步可独立提交）

| 步 | 内容 | 主要文件 | 预计行数 | 风险 |
|---|---|---|---|---|
| **S1** | 纯重构：`TranscriptPane` 组件 + `ActiveTranscript` + `PaneScroll` 从资源降级成组件字段；`Pane2` **仍镜像**、`title` 字段还没人读 → 行为零变化 | `rig_niri/src/pane.rs`、`panels.rs`、`frame.rs` | ~250 | M |
| **S2** | 来源渲染：`dialogue_batch_of` 拆层 + `batch::source_rows`（从 `workspaces_demo` 搬入）+ 每个窗格各自窗口化；`Pane2` 按来源出内容；删 `mirror_pane2_rows` | `rig_niri/src/batch.rs`、`pane.rs`、`panels.rs`；`rig_harness_in_3d/src/workspaces_demo.rs`（瘦身） | ~350 | M（结构最大） |
| **S3** | 进入/退出：`/show` 命令 + `Alt+A` picker + `Alt+W` + `ShowTarget`/活动窗格 + 标题行；`show_focused_agent` 改成"按来源解析，`Changed<TranscriptPane>` 也生效" | `rig_harness_in_3d/src/show.rs`（新）、`panes.rs`；`keys.rs`；`workspaces_demo.rs` | ~380 | M |
| **S4** | 已保存会话：异步 `saved_sessions` + `SessionMeta` 列表 + `open`/`read_conversation` + `Value` 来源 + 标题行 | `rig_harness_in_3d/src/show.rs`、`session_view.rs`（新） | ~300 | M |
| **S5** | 压缩条目：`Replaced` 组件（live 差集）+ `summarized_in_log`（存档）+ 两个渲染路径的展开 + `compacted_line` 复刻 | `rig_niri/src/batch.rs`、`transcript.rs`（引擎失效） | ~260 | M |
| **S6** | 重启前的调用事实：`CallFact` → `plugins/rig_niri/calls.jsonl`、装载、`Effects::continue_after`、`owner` 按 `AgentId` 补 | `rig_niri/src/calls.rs`（新）+ `batch.rs`；应用装配 | ~420 | **L** |

### S1 详细（纯重构，不改行为）
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

### S2 详细（这一步才改变画面）
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

### S3 详细
- `/show` 用 `In<CommandArgs>`（`commands.rs:49`）；解析 `args` 第一词 = 子命令。
- `Alt+A` 的 picker 由应用层 spawn（agent 列表需要 `SpawnedBy`/`AgentId`，会话列表需要
  `Home`）；`rig_niri` 只提供"给活动窗格设来源"的 API（`panes::set_source(world, pane, source)`）。
- picker 里的子 agent 列表 = `Query<&SpawnedBy>` 过滤 `parent == 本工作区 agent`（一层足够；
  多层用 `iter_ancestors`，`relationship_query.rs:135`）。
- 测试：
  - `Alt+S` 后 `Alt+A` 选第二个 agent → `Pane2.source == Agent(sub)` 且主窗格不变；
  - `/show agent <id>` → `ShownAgent == sub`，**且焦点切换回来时被 tile 的 source 覆盖回来**
    （回归测试 `show_focused_agent` 的 `Local` 去抖，`workspaces_demo.rs:325`）；
  - `Alt+W` → `Pane2` 消失、`PanelRows` 无 `Pane2`、主窗格行不变；
  - `/show workspace` → `ShownAgent == WorkspaceAgents.agent_of(focused)`。

### S4 详细
- 读会话列表在 command/startup 里做（上游明说会阻塞帧，`host/session.rs:82`）：用
  `AsyncComputeTaskPool` 拿 `Arc<dyn Store>`，回来后 `write_message(SavedSessionsLoaded(..))`。
- 预览只用 `SessionMeta`（`meta.rs:32`）+ 第一条消息的截断，不 fold 全日志。
- 测试：临时目录建 `DirStore` + 写两条消息 → `/show session <id>` → 侧窗格文本含那两条、
  且 `ShownAgent` **不变**（只读不抢输入）。

### S5 详细
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

### S6 详细（L，最危险）
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
| 聚焦即隐藏其它工作区 | 产品约束 3；`focus_workspace` 只决定内容写哪（`workspaces.rs:230`） |
| 为子 agent 新建常驻工作区 | 见 §2.2 方案 B；会破坏"一条条带 = 一个会话"的绑定 |
| 第 3、第 4 个转写窗格（N panes） | `PanelId::Pane2` 是单值；N 窗格要重做 `PanelId` 的键空间（`panels.rs:33`），不是这次的风险预算 |
| 侧窗格跑业务 look | 需要把 `RenderBatch`/look 缓存从单例改成每来源一份（`batch.rs:1851`），L 级，留到"多来源完整管线" |
| 压缩之后的 rewind / undo / fork | 只做"看得见"，不做"改得动"（archived 条目不可选中，对齐上游 `transcript.rs:1483`） |
| 由本前端**产生**子 agent（`/subagent` 工具/命令） | 本次是"看得见"；生产者属于业务工具层。测试用 `rig_ecs::spawn::spawn` 造 |
| 由本前端**执行**压缩（`/compact`） | 上游 `prepare_compaction`/`apply_summary` 在 `rig-memory`，接命令是另一件事；本设计只消费它留下的记录 |
| 存档会话里的时长/调用树 | 存档里没有 effect 记录（§2.6），不画假数字 |
| 跨会话 diff / 合并 | 只做"并排对照"，不做对齐/合并算法 |

---

## 6. 验收标准（全部无窗口，可用 ECS 断言）

1. **来源在 tile 上**：拆窗后 `app.world().get::<TranscriptPane>(pane2_root).source` 等于设进去的值；
   `Resource` 里没有任何"全局来源"（`ShownAgent` 只是 `Shown` 的解析结果）。
2. **主窗格换来源 = 移 `ShownAgent`**：`/show agent sub` 后 `ShownAgent.0 == Some(sub)`，
   `CurrentAgent.0 == <根 agent id>`（`follow_shown` 的证据，`main_agent.rs:101`）。
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
11. **关窗复原**：`Alt+W` 后 `PanelRoots`/`PanelRows`/`FrameTree` 里没有 `Pane2`，
    且 `PanelRows[Transcript]` 的行序列与拆窗前相等。
12. **没有弹层**：全仓库 `grep -r "Placement::Over\|Placement::Within" rig_niri/src` 仍为空
    （沿用 `WORKSPACE_MODEL.md` §8 的验收口径）。

---

## 7. 开放问题（不阻塞本次落地，但要有人拍板）

1. **主窗格显示子 agent 时，输入框要不要变？** 本设计按上游：照常发给子 agent，标题行写明
   "shown agent: helper"。另一种选择是"子 agent 视图锁输入"（灰掉输入 tile），但那会新增一个
   "为什么打不了字"的问题。
2. **`/show session` 打开存档后，`Ctrl+S`/复制是否要写回？** 本设计：不写回，存档纯只读。
3. **一个子 agent 结束后 Pane2 该显示什么？** 本设计：保持来源不变，标题行加 `(ended)`，
   内容留在原地（空间记忆：用户没关就不动）。不自动切回、不自动关窗。
4. **effect 日志的量**：`calls.jsonl` 每结算一次追加一行，长会话可能上万行；S6 里要先做
   "只读最后 N 天/最多 N 行"的上限，再谈全量。
