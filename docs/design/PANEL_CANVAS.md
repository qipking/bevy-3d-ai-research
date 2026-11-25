# 开放面板画布（F6）：插件面板注册、条带列与行读回

> **状态（2026-11-26）**：**切片 1 已实现**——插件能注册面板、把它作为 niri 列放进
> 指定工作区、拿到 `bevy_ui` 根并 spawn 自己的子树（3D 镜像自动可见）、按
> `RowLayout` + `PanelAreas` 读回“第 n 行的世界矩形”。内置 `PanelId` 面板的行为与
> 几何不变。**`StatusLine` 组件留给切片 2**（设计见 §3.5，尚未写代码）。
>
> 落点：`rig_niri/src/panel.rs`（新）、`rig_niri/src/panels.rs` 的
> `PluginPanel` / `PanelId::Plugin`。行号是本次快照；函数名是稳定锚。
>
> 配套：[`NIRI_SCROLL.md`](NIRI_SCROLL.md)（列宽/滚动）、
> [`FEATHERS_UI.md`](FEATHERS_UI.md)（视觉 token）、
> [`../status/WORKSPACE_MODEL.md`](../status/WORKSPACE_MODEL.md)（工作区模型）。

---

## 1. 一句话

**插件面板就是“条带里的一列 + 一个普通 `bevy_ui` 根”**：注册拿句柄，按
`Fraction × 视口宽` 追加到某工作区的横向条带，拿根实体随便 spawn `Node`/`Text`/
`ImageNode`；3D 镜像本来就会把根下面的整棵子树镜像出来。读回不是另存一份
“画了什么”，而是从这一帧的 `PanelArea` + 行实体上的 `RowLayout` 反算世界矩形。

**不引入第二套画布**：没有 `PanelCanvas` 的 `Buffer`、没有 `RequestRedraw`
门控——`bevy_ui` 每帧重排、`feathers_in_3d::sync_mirror` 每帧（`PostUpdate`）
重新镜像，终端那套“脏区/重绘请求”在这里没有收益。

---

## 2. 现状与缺口（切片 1 之前）

| 位置 | 现状 | 缺口 |
|---|---|---|
| `panels.rs:66` `enum PanelId` | 封闭枚举：`Transcript`/`Status`/`Input`/`Picker`/`List`/`Preview`/`Pane2`/`Spacer`/`Hints`/`Confirm`/`Complete` | 插件加一块 tile 必须改核心枚举 + 所有 match |
| `panels.rs:123` `PanelRows` | `HashMap<PanelId, Vec<KeyedRow>>` | 同上；键空间封闭 |
| `panels.rs:159` `PanelRoots` | `HashMap<PanelId, Entity>`，`lay_out` 按它把树叶子映射到根 | 没有“插件把根登记进来”的入口 |
| `panels.rs:298` `sync_panels` | 按工作区拿 `WorkspacePanels`，逐面板做行前缀 diff，生成 `RowLayout` | 只服务内置键；插件没有行归属 |
| `panels.rs:109` `RowLayout` | `{panel, panel_entity, index}`，行实体上有 | 有行身份，但没有“换算世界矩形”的公开函数 |
| `frame.rs:1053` `spawn_panel` | 一次装好 `Node`/chrome/`WorldPanel`/`MirrorRoot`/`PanelId` | 是 `pub` 的，但插件没有合法 `PanelId` 可传 |
| `frame.rs:586` `lay_out_frame` | 每个工作区用自己的 `tree` + `roots` 算 `PanelAreas` | 树叶子只能是内置 `PanelId` |
| `frame.rs:327` `PanelArea` | 树给的精确 `size`/`center`、格子近似 `rect`、行列数 | 足够做行读回的几何源 |
| `layout.rs:99` `Size::Fraction` | 列宽 = 视口宽的比例；`append_column_at`（`:779`）追加到含转写的横向条带 | 没有“插件列 + 任意 fraction”的便捷入口 |
| `layout.rs:912` `set_column_fraction` | 只改目标列的 `Fraction`，其它列一个字节不动 | 可复用（切片 1 就是 `append_column_at` 后调它） |
| `workspaces.rs:732` `WorkspacePanels` | 每工作区 `rows`/`roots`/`spawned`/`areas` 权威快照 | 插件列必须同时更新“组件 + 聚焦全局代理” |
| `status.rs:382`（上游 `reference/rig-tui-inspect/rig-tui-0.47.0/src/status.rs`）`StatusLine` | 上游是挂任意实体的组件，在它的一行画状态 | 我们只有固定 `PanelId::Status`；`status_rows`（`status.rs:186`）每帧现算一行 |

**3D 侧本来不是缺口**：`feathers_in_3d` 不认识 rig，`MirrorRoot`（`world3d.rs:126`）
下面的整棵 `bevy_ui` 子树都会被镜像（`Node`/文字/`ImageNode`/`Outline`，见
`FEATHERS_UI.md` §2.6）。所以插件只要拿到根、在下面 spawn，就能在 3D 里看见；
缺的只是**注册 + 放进条带 + 读回**。

---

## 3. 推荐 API（切片 1 已按此实现）

```rust
// 1) 注册（插件 build 里一次；同名幂等）
let panel: PluginPanel = app.register_panel("git");

// 2) 放进某个工作区的条带：新列宽 = fraction × 视口宽，已有列不变
let root: Entity = add_panel_column(world, workspace, panel, 0.28)?;

// 3) 画：root 就是普通 bevy_ui 根（文字/图片/自定义子树都行）
world.entity_mut(root).with_children(|p| {
    p.spawn((Node { width: percent(100.0), height: px(24.0), ..default() },
             Text::new("git status")));
});

// 4) 可选：追加列通常在视口右边，滚它进来
let _ = reveal_panel_column(world, workspace, panel);

// 5) 读回：某块面板第 n 行这一帧在世界里的矩形
let row: Option<DrawnRow> = drawn_row(world, workspace, panel.id(), 2);
```

### 3.1 身份：注册表 + 句柄，`PanelId::Plugin(..)`

- `PanelRegistry::register(name) -> PluginPanel`（`panel.rs:65` / `:75`）：名字幂等，
  同名重复注册返回同一句柄；空名字生成 `plugin-panel-<n>`，不 panic。
- `PanelId::Plugin(PluginPanel)`（`panels.rs:66`）：句柄是 `u64` 的薄包装
  （`Copy + Eq + Hash`），内置变体保持原样。
- **为什么是枚举变体而不是“实体 → 画布”**：`layout::Node` 的叶子、
  `FrameTree`、`PanelRows`、`PanelRoots`、`PanelAreas`、`WorkspacePanels` 全都以
  `PanelId` 为键。只加一个 `Plugin(..)` 变体，这些结构**一个类型都不用改**；
  改键空间会牵动 `layout`/`frame`/`workspaces` 的公开类型（L 成本）。
- 代价：`PanelId` 从 1 字节变成 16 字节（判别式 + `u64`），哈希表的键大一点；
  行为、分支和内置几何不变。这是有意换“最小改动面”。
- 句柄只是身份，不持有实体；换工作区/重建工作区后同一个句柄仍指同一块插件面板，
  根从 `WorkspacePanels::roots` 按 `PanelId::Plugin` 查。

### 3.2 放进条带：`add_panel_column`

`add_panel_column(world, workspace, panel, fraction) -> Option<Entity>`（`panel.rs:141`）：

1. 校验句柄已注册、工作区有 `Workspace` + `WorkspacePanels`；
2. 在树的**工作副本**上 `append_column_at(PanelId::Transcript, leaf)`（新列接在
   转写/右列之后的横向条带末尾），再 `set_column_fraction(panel, fraction)`
   （非法值退回 `DEFAULT_COLUMN_FRACTION`）；失败不留实体、不改原树；
3. 用**内置同一个** `frame::spawn_panel` 生成根（chrome/`WorldPanel`/`MirrorRoot`/
   `PanelId` 组件一次装好），挂 `WorkspacePanel(workspace)`；
4. 更新该工作区组件（`tree`/`roots`/`rows`/`spawned`）；如果它是聚焦工作区，
   同时更新全局 `FrameTree`/`PanelRoots`/`PanelRows`/`PanelSpawned`——否则下一次
   `commit_focused_panels`（`workspaces.rs:872`）会把组件写回成没有新列的旧全局副本；
5. 返回根实体，失败返回 `None`（只 `warn!`，不 panic）。

**列宽/位置规则**（niri 第一原则）：新列是 `Size::Fraction(fraction)`，宽度
相对**视口**，已有列的宽度和 x 一个字节都不改。默认树的两列已经占满一屏
（`TRANSCRIPT_COLUMN_FRACTION` 0.62 + `RIGHT_COLUMN_FRACTION` 0.38，`layout.rs:74`/`:80`），
所以追加列通常落在视口右边；`reveal_panel_column`（`panel.rs:234`）用
`column_bounds` + `scroll_to_visible` 把它滚进来（只写 `WorkspaceScroll`）。

> 切片 1 **不自动抢滚动**：`scroll_active_column` 在“活动列变化 / 窗口 resize”时
> 仍会把活动转写列滚进视口。插件在启动第一帧之后放面板/调用 reveal 最稳；
> 若以后要“新列自动获得焦点”，属于切片 2 的活动列扩展。

### 3.3 插件怎么画：两条路，可混用

1. **拿根 spawn 子树（推荐，任意 UI）**：
   `world.entity_mut(root).with_children(..)`，`Node`/`Text`/`TextSpan`/
   `ImageNode`/`Outline` 都可以。3D 镜像不挑组件，`feathers_in_3d` 支持什么就能
   看见什么（图片/SVG 边界见 `FEATHERS_UI.md` §2.6）。
   注意面板根是布局给出的**固定尺寸**框：子节点自己决定内部排版；超出部分由
   面板自身矩形 + 工作区视口裁剪。
2. **给“行数组”（要读回时用）**：把 `KeyedRow` 写进
   - 聚焦工作区：全局 `PanelRows`（下一帧 `commit_focused_panels` 写回组件）；
   - 任意工作区：`WorkspacePanels::rows`。
   `sync_panels` 会和内置面板一样做前缀 diff，并给每行挂 `RowLayout`；
   文字样式/选区/点击都复用现有行管线。**只有这条路生成的行**能被
   `drawn_row` 读到（它按 `RowLayout` 确认行身份）。

### 3.4 读回：`DrawnRow` / `drawn_row` / `drawn_rows`

```rust
pub struct DrawnRow { pub panel: PanelId, pub index: usize, pub world_rect: Rect }
pub fn drawn_row(world: &World, workspace: Entity, panel: PanelId, index: usize) -> Option<DrawnRow>;
pub fn drawn_rows(world: &World, workspace: Entity, panel: PanelId) -> Vec<DrawnRow>;
```

`world_rect` 是**世界坐标、轴对齐**矩形（米），和 `WorldPanel::clip` 同口径
（`world3d.rs:46`/`:66`）。算法（`panel.rs::row_world_rect`）：

```text
内容区 = PanelArea.size − FramePanel.inset.total()   // total() 已经是上下/左右之和
起始 y = inset.total() / 2（FlexStart）/ 居中 / 贴底（读根 Node.justify_content）
行中心 = 面板中心 + (0, size.y/2 − 起始y − index·ROW_HEIGHT − ROW_HEIGHT/2)
行尺寸 = (内容区宽, ROW_HEIGHT) × WorldPanel.units_to_world
四角经 WorldPanel.anchor 变换后取 AABB（工作区只平移时就是行自身矩形）
```

行身份来自 `RowLayout`（`panels.rs:109`）：`drawn_row` 先按
`WorkspacePanels::spawned[panel].entities[index]` 找到行实体，要求
`RowLayout.panel/index/panel_entity` 三者都对得上，再用面板根的 `FramePanel.inset`
/ `Node.justify_content` / `WorldPanel` 算几何。没面板、行还没同步、区域为空时
返回 `None`。

> 够不够？**够**。`PanelAreas`（树给的精确 size/center）+ `RowLayout` 已经覆盖
> “某块面板第 n 行”；不需要再存 `TuiFrame` 那种“每个面板的屏幕矩形”表。
> 读回的是**几何**，不是行文本/高亮（文本在行实体上，需要时再读 `RowText`）。

### 3.5 `StatusLine`（切片 2，设计定稿、未实现）

目标：任意面板底部画一条状态行，内容仍来自 `AppStatus` / `StatusItems` /
`StatusParts`，不复制状态行逻辑。

```rust
/// 挂在面板根上：这块面板底部画一条状态行。
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct StatusLine {
    /// 画哪个 agent 的条目；`None` = 当前 `ShownAgent`（上游同义）。
    pub agent: Option<Entity>,
}
```

实现要点（切片 2）：

- 从 `status_rows`（`status.rs:186`）里抽出“来源合并 + `[status] hide` +
  `no agent` + `rig_tui::status::line` 裁剪”的纯函数
  `status_line_row(agent, width) -> (Line, Option<Line>)`；`status_rows` 继续用
  它写 `PanelId::Status`，行为不变。
- 新系统排在 `PanelsPlugin` 的 `sync_panels` 之后（`FeathersSystems::Render`）：
  对每个 `(StatusLine, PanelAreas)` 面板，宽度取面板 `area.cols`，生成一行，
  spawn/更新一个**绝对定位的底部子节点**（`position_type: Absolute`, `bottom: 0`,
  左右贴内容区），里面放和普通行一样的 `Text`/`TextSpan`。
- 用绝对定位而不是硬塞进 `PanelRows`：插件自己的 flex 子树和内置行 diff 都不被
  打乱；面板变高变矮时状态行自动贴底。3D 镜像对绝对定位子树同样生效。
- `StatusLine` 只影响挂了它的面板；`PanelId::Status` 的现有状态行一字节不变。
- 测试：挂到插件面板 → 底部子节点存在、世界 y 在面板底；宽度变化重新裁剪；
  没主题/没 agent 不 panic。

---

## 4. 明确不做

| 不做 | 为什么 |
|---|---|
| 弹层 / 浮窗 / 自由坐标画布 | 条带里只有列；浮动会破坏 niri 的 tiling、滚动与工作区剪裁（和 `FEATHERS_UI.md` §6、`WORKSPACE_MODEL.md` §8 一致） |
| `PanelCanvas` 的 `Buffer` / 逐格绘制 | 我们的行是**值**，`bevy_ui` 自己排版；再包一层 buffer 等于重写渲染 |
| `RequestRedraw` / `RedrawArea` / 脏区门控 | 每帧重排 + 每帧 `sync_mirror`，没有“省一帧重绘”的收益；插件改数据下一帧自然可见 |
| `TuiFrame`/`TerminalWrite` 的屏幕格坐标 | 终端专有；3D 里世界矩形由 `WorldPanel::anchor` 给出，读回用 `DrawnRow` |
| 插件改内置面板的尺寸/位置 | 内置 `PanelId` 的面板行为不变；开新列不改已有列宽（niri 不变量） |
| 行内容/高亮/滚动状态的完整 `DrawnTranscript` | 切片 1 只读几何；文本读 `RowText`，高亮/选区在 `Selection`/行样式里 |
| 自动给“以后创建的工作区”都加插件列 | 切片 2 可加注册策略（`PanelSpec { fraction, all_workspaces }`）；切片 1 是显式放到某个工作区 |

---

## 5. 与 niri 布局 / feathers / 工作区模型的关系（一页内）

### niri 布局

- 插件面板 = 横向条带（含 `Transcript` 的最外层 `Split::Horizontal`，即
  `layout::Node::strip_area`）末尾追加的一个 `Child::fraction(..)`。它和 picker
  列走同一条几何路径（`append_column_at` / `placements_scrolled`）。
- **列宽语义**：`Size::Fraction` 相对视口，不是相对剩余空间；新列不改兄弟宽度
  （`layout.rs:99`、`set_column_fraction` `:912`）。内容超出视口由
  `WorkspaceScroll` 横向滚动，`WorldPanel::clip` 裁剪；插件面板滚出去时实体仍在，
  滚回来就能看见。
- 竖直方向：插件列占条带主体的全高（和转写列/右列同高），Hints/Status 是条带
  之外的 chrome，不受影响。列内要再分上下，插件可以在自己的根里用 `Node` 的
  flex 排，不需要新的布局语义。
- 读回的 `PanelArea.size`/`center` 就是树给的精确值，`rect` 只是格子近似；
  `DrawnRow` 用精确值。

### feathers 视觉

- 插件根默认 `Look::panel(PANEL_BG, PANEL_BORDER)`（`frame.rs:996` 的 token 表），
  和转写面板同一档底/边框/4px 圆角；`sync_panel_chrome`（`frame.rs:1032`）
  按 `UiTheme` 解析。
- 插件子树里可以直接用 `bevy_feathers` 的组件/token；但**不要**用会自己抢输入
  的控件（`TextInput`/`Button` 的交互组件）盖住内置 picking——这和 F2/F5 的
  取舍一致（`FEATHERS_UI.md` §4）。
- 3D 镜像的边界：`Node` 背景/边框/圆角、文字、`ImageNode`、`Outline` 可见；
  SVG 图标仍不支持（没开 `bevy_ui/svg` feature，见 `FEATHERS_UI.md` §2.6）。

### 工作区模型

- 注册表是**全局**的（一块面板一个句柄）；放置是**每工作区**的（每个
  `WorkspacePanels.roots` 各有一个根实体）。同一个句柄在不同工作区可以是不同
  实体、不同列宽、不同行——这正是 niri “一个工作区一条条带”的模型。
- 聚焦工作区的全局 `PanelRows`/`PanelRoots`/`FrameTree`/`PanelAreas` 是工作副本；
  `apply_focus`（`workspaces.rs:935`）在切换时装载组件的根/树/行。
  所以 `add_panel_column` 对聚焦工作区必须同时写全局代理（§3.2 第 4 步），
  非聚焦工作区只写组件，不污染全局。
- 应用层自己 `spawn_workspace`（`workspaces.rs:781`）创建的每个工作区都是独立
  条带；插件显式对某个工作区调用 `add_panel_column` 即可。切片 2 若加“所有
  工作区都装”的策略，落点是 `spawn_workspace_with_tree` 之后统一应用注册表。

### 状态行

`PanelId::Status` 仍是内置固定状态行（`status.rs:186`），插件加**条目**走
`StatusParts`（`status.rs:113`）；给任意面板挂整条状态行是切片 2 的
`StatusLine`（§3.5）。两者共享同一套来源/裁剪逻辑，不复制。

---

## 6. 切片 1 已实现 / 切片 2 待做

### 切片 1（本次）

| 内容 | 落点 |
|---|---|
| `PluginPanel` 句柄 + `PanelId::Plugin(..)` | `panels.rs:40` / `:66` |
| 注册表（名字幂等）+ `App::register_panel` | `panel.rs:65` / `:113` |
| 放进工作区条带、拿根、更新组件 + 聚焦全局代理 | `panel.rs:141` |
| 滚进视口 | `panel.rs:234` |
| `DrawnRow` / `drawn_row` / `drawn_rows` | `panel.rs:263` / `:281` / `:316` |
| 测试（7 个） | `panel.rs` `mod tests`：注册→列→根→区域→镜像根；行世界矩形数值；reveal 后可见；重复/找不到工作区/空面板；非聚焦工作区；句柄可做树叶子 |

回归：`cargo test --workspace --offline` 全绿；`rig_niri` lib 测试 580 → 587，
内置面板的现有测试不变。

### 切片 2（建议顺序）

1. `StatusLine` 组件 + 来源复用 + 底部绝对定位子节点（§3.5）——S/M。
2. `PanelSpec` 注册策略：`fraction`、`all_workspaces`、可选 `Look`，在
   `spawn_workspace`/`bootstrap` 后统一应用——S。
3. 读回扩展：行文本/高亮（`RowText` 已有）、`DrawnRow` 带 panel/workspace 的
   可见性判断（`effective_clip` 非空）——S。
4. 活动列扩展：插件列可以成为 `scroll_active_column` 的目标（新列自动获得焦点）
   ——S/M，依赖 L3 的滚动快照。
5. 插件命令绑定：`InputAction` 已开放前缀/`run_command`，缺“注册表 + 默认键位”
   的公开入口——M（属于 `REMAINING.md` §10 的另一半）。

### 验收口径

- 内置 `PanelId` 面板的几何/行为/现有测试不回归；
- 注册→放置→根存在→面板区域非零→行世界矩形数值可断言；
- 重复注册、重复放置、找不到工作区、未注册句柄、空面板：不 panic；
- 3D 可见性用 `WorldPanel.anchor`、`clip` 与 `panel_world_rect` 的交集断言
  （headless，不跑 GUI）。
