# agent + 3D 程序设计开发范（rig_niri 模板）

> 定位：`rig_niri` = **rig-tui 的另一个完整前端**（视图层）；
> `rig_harness_in_3d` = **通用 app 的 fork 模板**（harness 层：装配 + 扩展点）。
> 上游跟进策略见 `rig_niri/UPSTREAM.md`；本文讲"怎么用它搭一个新 app"。

> 这份文档把 `rig_niri` 从"一个业务 demo"提炼成**可复制的模板**：
> 用 rig-tui 的 look 管线做"内容长什么样"，用 feathers 做"长出来"，用 `feathers_in_3d` 把 UI 放进 3D 世界。
> 目标是：换一个业务（导购、运维、客服、现场作业），只需要换**工具 + look + 面板内容**，其余不动。

---

## 0. 三个 crate 对应 rig 的三层

| rig 官方层 | 这里的 crate | 职责 |
|---|---|---|
| `rig-ecs`（kernel） | 上游依赖 | agent / turn / call / Gate / Ask |
| `rig-harness`（粘合） | **`rig_harness_in_3d`** | 装配、插件服务、扩展点 |
| `rig-tui`（视图） | **`rig_niri`** | RenderBatch+look、面板、状态、输入、picker、Front |
| （终端设备） | `feathers_in_3d` | 3D 渲染 |

## 0.1 视图内部的分层（每层只依赖下一层）

```
┌─ L4 交互层 ──────────────────────────────────────────────┐
│ camera_rig（手柄/键盘/将来的陀螺仪）  pick（射线）       │
│ editor（输入）  observers（选择/复制/闸门选项）           │
├─ L3 模板层 ──────────────────────────────────────────────┤
│ panels（PanelId + 3D 锚点 + 增量 diff）                   │
│ status（状态行）  theme（token）  editor_rows（输入行）    │
├─ L2 转写层 ──────────────────────────────────────────────┤
│ batch（RenderBatch 驱动）+ looks（fn(&ToolRender)->行）   │
│ transcript（LookLine -> feathers 行）                     │
├─ L1 内核层 ──────────────────────────────────────────────┤
│ rig-ecs：agent / turn / 工具 effect / Stage::Gate / Ask   │
│ rig-core：Tool 契约、ToolCall/ToolResult                  │
└──────────────────────────────────────────────────────────┘
              ↓ 渲染底座
        feathers_in_3d（与业务无关）
        world3d / text3d / sort / pick
```

**依赖方向单向向下**：L1 不知道有 UI；L2 只把状态变成"行"；L3 只把行分配到面板；L4 只处理输入。

---

## 1. 数据流（一帧）

```
rig-ecs 效应实体（ToolRequest / Outcome / Holds / Running）
   │  L2 rebuild_batch：状态 -> CallState，结果 -> 会话形状的 ToolResult
   ▼
RenderBatch（rig-tui 的类型）
   │  looks（注册在 RenderSystems::Render）：ToolRender -> Vec<LookLine>
   ▼
Row（LookLine::colored() -> ratatui Line -> feathers Row）
   │  key/hash
   ▼
PanelRows（按 PanelId 分桶）
   │  L3 sync_panels：前缀 key 相同就原地换文字，只重建变化的后缀
   ▼
feathers 节点树（Node/Text/TextSpan，在视口外）
   │  L0 feathers_in_3d
   ▼
3D：mesh + SDF 材质 + 字形四边形 + 排序键 + 裁剪 + 射线拾取
```

关键：**每一层都只产出"下一层能吃的纯数据"**——`CallState`/`LookLine`/`Row`/`KeyedRow`。
换渲染器（终端 / 2D / 3D）时，L1、L2 完全不用动；rig-tui 自己的 TUI 就是同一份 L2 数据换了个 L0。

---

## 2. 与 rig-tui 的对应关系（模板照它来）

| rig-tui 模块 | 模板里的对应物 | 说明 |
|---|---|---|
| `panel.rs`（`TuiPanel`/`Placement`） | `panels.rs`（`PanelId` + `PanelSpec` + 3D 锚点） | 终端按格子切，这里按世界坐标摆 |
| `pane.rs` / `transcript.rs`（转写滚动） | `panels.rs::sync_panels` + `Overflow::scroll_y` | 增量 diff 代替重排 |
| `renderers.rs`（look 调度） | `batch.rs`（`RenderBatch` + `RenderSystems::Render`） | rig-tui 是 `pub(crate)`，所以自己驱动 |
| `status.rs` | `status.rs`（`StatusBar` + 一行） | 状态条目可扩展 |
| `hints.rs` | `editor.rs` 的第二行 | 键位提示 |
| `title.rs` | 窗口标题（`Window::title`） | 面板标题可做成面板第一行 |
| `pick.rs`（选择器） | 闸门的 `Ask` 行 + `AskChoice` | 选项列表就是可点行 |
| `input.rs` / `editor.rs` / `keys.rs` | `editor.rs` | 状态形状相同，键位收窄 |
| `history.rs` | `Editor.history` + ↑↓ | |
| `complete.rs` | 待做（命令补全） | 建议做成 `CompletionSource` trait |
| `markdown.rs` / `highlight.rs` / `diff.rs` | **looks**（在 `batch.rs`） | 内容表现属于业务 look |
| `theme.rs` | `theme.rs`（`Theme` token） | 换主题只改一处 |
| `render.rs` / `screen.rs` / `terminal.rs` | `feathers_in_3d` | 终端设备换成 3D 渲染 |
| `clipboard.rs` | `bevy_clipboard` | |

---

## 3. 扩展点（新业务只碰这三处）

### 3.1 加一个工具 + 它的卡片

```rust
// 1) 工具：实现 rig-core 的 Tool，注册到内核
app.add_tool(MyTool);

// 2) look：告诉转写"这个调用长什么样"，返回 Vec<LookLine>
fn my_look(mut batch: ResMut<RenderBatch>) {
    for (call, out) in batch.tools_mut("my_tool") {
        out.lines = vec![ /* Line::from(...) */ ];
    }
}
app.add_systems(TranscriptRender, my_look.in_set(RenderSystems::Render));
```

**约定**：look 只读 `ToolRender`（参数、结果、`CallState`），不碰 UI；想要富样式就多几个 `Span`。

### 3.2 加一块面板

```rust
// PanelId 加一项 + default_panels() 加一条 spec（尺寸/锚点/配色）
PanelId::Side,
// 再写一个系统往 PanelRows 里塞行
fn side_rows(mut panels: ResMut<PanelRows>) {
    panels.set(PanelId::Side, vec![KeyedRow { key: 1, hash: 1, row }]);
}
```
面板之间互不影响：每个面板自己的 3D 锚点、自己的增量状态。

### 3.3 加一个交互

```rust
// 让行可点：spawn_row 已经给了 Pickable；点中后拿 RowText / 自定义组件
app.add_observer(|click: On<PointerClick>, q: Query<&MyRowAction>| { ... });
```
射线拾取在 `feathers_in_3d::pick`，**不需要自己写 backend**；靠 `Pickable` 声明"可点"。

---

## 4. 不变量（踩过的坑，违反就出 bug）

1. **UI 子树必须移出视口**（`left: -20000px`），否则 bevy_ui 的 2D 渲染盖在 3D 上；
2. **共面节点的顺序只能靠排序键**：`PanelSort` + `Transparent3d` 覆写，不要用 z 偏移；
3. **`PointerOut` 会冒泡到所有祖先**：picking 监听器必须判断"是不是我要的实体"，
   否则会把祖先背景写成透明（曾导致"面板消失、背后几何透出"）；
4. **`TextSpan` 必须显式给 `TextFont`**：隐式默认是 20px 默认字体，会换字体、换图集；
5. **一个 Text 的字形可能跨多个图集**（Latin + CJK），合批必须按图集分；
6. **`clip_bounds` 的单位是米**，不是 UI 像素；
7. **没有填充的节点也要落位**（给 Transform），否则它的文字子节点塌到面板中心；
8. **效应实体结算后会被回收**：转写要在 `Stage::Review` 抄走结果（`CompletedCalls`）；
9. **`ReadyToStart` 是过滤类型别名**，不能 `Has<ReadyToStart>`；
10. **Ask 只问一次**：给效应打标记，否则每个 pass 都问。

---

## 5. 新应用 checklist

```
[ ] Cargo：bevy =0.20.1 + feathers 0.20.1 + rig-tui/rig-core/rig-ecs 0.47 + feathers_in_3d
[ ] 内核：AgentPlugin + spawn_root（或接真实 daemon/模型）
[ ] 工具：实现 Tool（read_only 的标 annotations），add_tool
[ ] look：每个工具一个 fn(&ToolRender) -> Vec<LookLine>，注册到 RenderSystems::Render
[ ] 面板：PanelId + spec（尺寸/锚点），行来源写进 PanelRows
[ ] 状态：StatusBar 字段改成自己的（模型/费用/任务数…）
[ ] 输入：Editor 的提交接到"发消息给内核"（或有模型后接 ReplyStream）
[ ] 闸门：写工具在 Stage::Gate spawn Ask + HoldOf，前端画选项行
[ ] 相机：camera_rig（桌面手柄/键盘；移动端换设备姿态）
[ ] 字体：CJK 子集化（tools/fetch_cjk_font.sh）
[ ] 证据：自动巡游截图（rf_shots/*.png）当回归基线
```

---

## 6. 反模式

| 反模式 | 为什么不行 |
|---|---|
| 在 look 里直接建 UI 节点 | look 必须与渲染器无关，否则 TUI/2D/3D 三端无法共存 |
| 在 UI 系统里读 rig-ecs 的效应实体 | 效应会被回收；经 L2 的 `CompletedCalls`/`RenderBatch` 这一层 |
| 每个 token 重建整面板 | 用 keyed 行增量 diff |
| 用 z 偏移解决前后遮挡 | 斜视会错位；用排序键 |
| 把业务状态塞进 `Theme`/`PanelSpec` | 模板层要能原样复制到下一个业务 |

---

## 7. 当前模板的完成度

| 能力 | 状态 |
|---|---|
| 多面板（transcript / status / input）+ 3D 锚点 | ✅ |
| 状态行、键位提示 | ✅ |
| 输入编辑器（光标/历史/提交事件） | ✅ 基础键位 |
| look 管线 + 增量刷新 + 流式 | ✅（模型仍是脚本） |
| 闸门 Ask/Decision + 可点选项 | ✅ |
| 射线拾取 / 悬停 / 选择 / 复制 | ✅ |
| 相机操作架（手柄/键盘） | ✅（无实体手柄，未真机验证） |
| 补全、滚动区嵌套、多会话面板 | ⬜ |
| 真实模型 | ⬜（接缝：`ReplyStream`） |
