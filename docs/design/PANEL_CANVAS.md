# 开放面板画布（F6）：插件面板注册、条带列与行读回

> **状态（2026-11-26 核对）：切片 1、切片 2 已实现；切片 3 未实现。**
>
> | 切片 | 内容 | 状态 | 证据 |
> |---|---|---|---|
> | 1 | 插件面板注册、`PanelId::Plugin`、放进条带、行世界矩形读回 | ✅ 已实现 | `panel.rs:145`（`register`）、`panel.rs:271`（`add_panel_column`）、`panel.rs:704`（`DrawnRow`）、`panel.rs:722`（`drawn_row`）、`panels.rs:100`（`PanelId::Plugin`） |
> | 2 | `StatusLine`、`PanelAuto::AllWorkspaces`、移除/注销、`ActivePanelColumn` | ✅ 已实现 | `status.rs:114`/`:361`、`panel.rs:65`（`PanelAuto`）、`:413`（`apply_auto_panels`）、`:440`/`:521`/`:541`（移除/注销）、`:600`（`ActivePanelColumn`） |
> | 3 | 插件命令绑定、`DrawnRow` 文本/高亮读回、redraw 门控 | ⬜ 未实现（redraw 门控**明确不做**） | `DrawnRow` 只有几何字段（`panel.rs:704`–`:713`）；没有插件命令绑定入口；全仓没有 `RequestRedraw`/门控 |
>
> 落点：`rig_niri/src/panel.rs`（新）、`rig_niri/src/status.rs`（`StatusLine`）、
> `rig_niri/src/panels.rs` 的 `PluginPanel` / `PanelId::Plugin`。行号是本次快照；
> 函数名是稳定锚。
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

## 2. 历史：切片 1 之前的缺口（已由切片 1/2 填上，保留作决策记录）

| 位置 | 现状 | 缺口 |
|---|---|---|
| `panels.rs:66` `enum PanelId` | 封闭枚举：`Transcript`/`Status`/`Input`/`Picker`/`List`/`Preview`/`Pane2`/`Spacer`/`Hints`/`Confirm`/`Complete` | 插件加一块 tile 必须改核心枚举 + 所有 match |
| `panels.rs:123` `PanelRows` | `HashMap<PanelId, Vec<KeyedRow>>` | 同上；键空间封闭 |
| `panels.rs:159` `PanelRoots` | `HashMap<PanelId, Entity>`，`lay_out` 按它把树叶子映射到根 | 没有“插件把根登记进来”的入口 |
| `panels.rs:298` `sync_panels` | 按工作区拿 `WorkspacePanels`，逐面板做行前缀 diff，生成 `RowLayout` | 只服务内置键；插件没有行归属 |
| `panels.rs:109` `RowLayout` | `{panel, panel_entity, index}`，行实体上有 | 有行身份，但没有“换算世界矩形”的公开函数 |
| `frame.rs:1053` `spawn_panel` | 一次装好 `Node`/chrome/`WorldPanel`/`MirrorRoot`/`PanelId` | 是 `pub` 的，但插件没有合法 `PanelId` 可传 |
| `frame.rs:593` `lay_out_frame` | 每个工作区用自己的 `tree` + `roots` 算 `PanelAreas` | 树叶子只能是内置 `PanelId` |
| `frame.rs:327` `PanelArea` | 树给的精确 `size`/`center`、格子近似 `rect`、行列数 | 足够做行读回的几何源 |
| `layout.rs:99` `Size::Fraction` | 列宽 = 视口宽的比例；`append_column_at`（`:779`）追加到含转写的横向条带 | 没有“插件列 + 任意 fraction”的便捷入口 |
| `layout.rs:912` `set_column_fraction` | 只改目标列的 `Fraction`，其它列一个字节不动 | 可复用（切片 1 就是 `append_column_at` 后调它） |
| `workspaces.rs:758` `WorkspacePanels` | 每工作区 `rows`/`roots`/`spawned`/`areas` 权威快照 | 插件列必须同时更新“组件 + 聚焦全局代理” |
| `status.rs:382`（上游 `reference/rig-tui-inspect/rig-tui-0.47.0/src/status.rs`）`StatusLine` | 上游是挂任意实体的组件，在它的一行画状态 | **切片 2 已补**：`status.rs` 的 `StatusLine` 挂在任意面板根上，在面板底部画一行；`PanelId::Status` 的现有状态行不变 |

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

// 6) 切片 2：注册时就自动装进所有工作区（之后新建的也带）
let panel: PluginPanel = app.register_panel_with("git", PanelSpec::everywhere(0.28));
// 或者运行时打开/关闭：
add_panel_everywhere(world, panel, 0.28);
focus_panel_column(world, workspace, panel);      // 成为活动列并滚进视口
remove_panel_everywhere(world, panel);            // 从所有工作区摘掉（旧列宽不变）
unregister_panel(world, panel);                   // 连注册一起删

// 7) 任意面板底部状态行（内置/插件面板都可以）
world.entity_mut(root).insert(StatusLine::default());
```

### 3.1 身份：注册表 + 句柄，`PanelId::Plugin(..)`

- `PanelRegistry::register(name) -> PluginPanel`（`panel.rs:145` / `:153`）：名字幂等，
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

`add_panel_column(world, workspace, panel, fraction) -> Option<Entity>`（`panel.rs:271`）：

1. 校验句柄已注册、工作区有 `Workspace` + `WorkspacePanels`；
2. 在树的**工作副本**上 `append_column_at(PanelId::Transcript, leaf)`（新列接在
   转写/右列之后的横向条带末尾），再 `set_column_fraction(panel, fraction)`
   （非法值退回 `DEFAULT_COLUMN_FRACTION`）；失败不留实体、不改原树；
3. 用**内置同一个** `frame::spawn_panel` 生成根（chrome/`WorldPanel`/`MirrorRoot`/
   `PanelId` 组件一次装好），挂 `WorkspacePanel(workspace)`；
4. 更新该工作区组件（`tree`/`roots`/`rows`/`spawned`）；如果它是聚焦工作区，
   同时更新全局 `FrameTree`/`PanelRoots`/`PanelRows`/`PanelSpawned`——否则下一次
   `commit_focused_panels`（`workspaces.rs:898`）会把组件写回成没有新列的旧全局副本；
5. 返回根实体，失败返回 `None`（只 `warn!`，不 panic）。

**列宽/位置规则**（niri 第一原则）：新列是 `Size::Fraction(fraction)`，宽度
相对**视口**，已有列的宽度和 x 一个字节都不改。默认树的两列已经占满一屏
（`TRANSCRIPT_COLUMN_FRACTION` 0.62 + `RIGHT_COLUMN_FRACTION` 0.38，`layout.rs:74`/`:80`），
所以追加列通常落在视口右边；`reveal_panel_column` 用
`column_bounds` + `scroll_to_visible` 把它滚进来（只写 `WorkspaceScroll`）。

> `add_panel_column` 本身**不抢滚动**：`scroll_active_column` 仍按活动列（转写窗格
> 或切片 2 的 `ActivePanelColumn`）走。要让插件列获得焦点/自动滚进来，用
> `focus_panel_column`（§3.6）。

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

### 3.5 `StatusLine`（切片 2，已实现）

**任意面板根挂上就在它的底部画一条状态行**，内容与内置 `PanelId::Status` 同源：
`AppStatus` / agent / 运行中 turn 的 `StatusItems` / `StatusParts` / `[status] hide`
/ `no agent` / `rig_tui::status::line` 宽度裁剪。挂哪块画哪块，多个面板各画各的；
不挂就不画。

```rust
/// 挂在面板根上：这块面板底部画一条状态行。
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct StatusLine {
    /// 画哪个 agent 的条目；`None` = 当前 `ShownAgent`（上游同义）。
    pub agent: Option<Entity>,
}
```

落点（`rig_niri/src/status.rs`）：

- `merged_status_items(agent, app_status, own, turn, parts, settings)` 是唯一的
  “来源合并 + 覆盖 + `[status] hide` + `no agent`”纯函数；
  `status_line_row(items, width)` 直接调 `rig_tui::status::line` 裁成左右两条。
  `status_rows`（写 `PanelId::Status`）和 `StatusLine` 都走它们，规则不复制。
- `draw_status_lines` 排在 `publish_focused_spawned` 之后（
  `FeathersSystems::Render`；它是 `PanelsPlugin` 行同步链的末尾），所以本帧的
  `PanelArea` / 行都已在位。
- 每块面板生成/更新一个**绝对定位的底部子节点**：`position_type: Absolute`、
  `bottom: 0`、左右贴内容区、高 `ROW_HEIGHT`；宽度取面板这一帧的 `area.cols`，
  宽度变化时重新裁剪。内容 hash 没变不重建文字。
- **不写 `PanelRows`、不进行 diff**：插件自己的 flex 子树、内置行同步、`drawn_row`
  都不受影响。3D 镜像对绝对定位子树同样生效。
- **输入框（`PanelId::Input`）**：上游语义是“编辑器在自己的底行画状态行”，不额外
  占一行。我们不改 `editor.rs`，用同一个绝对定位底部行盖在输入框底行上——不占
  flex 行、不改 `PanelArea`/面板行数，视觉上就是编辑器底行。
- `StatusLine.agent = Some(e)` 画指定 agent（`None` 跟 `ShownAgent`）；没有 agent
  时补 `no agent`（上游 `status.rs:406`）。
- `PanelId::Status` 的主状态行仍由 `status_rows` 写，行为一个字节不变。

测试（`panel.rs` 的 `a_status_line_on_a_plugin_panel_is_drawn_at_the_bottom` /
`a_status_line_on_the_input_panel_stays_in_its_bottom_row` /
`status_lines_are_per_panel_and_clip_to_the_panel_width`）：挂/不挂、输入框底行、
多面板独立、宽度裁剪、换 agent。

### 3.6 生命周期与活动列（切片 2，已实现）

**注册策略**：

```rust
pub enum PanelAuto { Manual, AllWorkspaces }

pub struct PanelSpec { pub fraction: f32, pub auto: PanelAuto }
impl PanelSpec { pub fn everywhere(fraction: f32) -> Self; }

app.register_panel_with("git", PanelSpec::everywhere(0.28));
```

`PanelRegistry` 为每块面板存一份 `PanelSpec`。`PanelAuto::AllWorkspaces` 由
`apply_auto_panels`（`panel.rs:413`，`WorkspacesPlugin` 的 `Prepare`，exclusive system）补齐：

- 注册时已有 N 个工作区 → 第一帧每个工作区各加一列；
- 之后 `spawn_workspace` / `spawn_workspace_with_tree` 新建的工作区 → 下一帧自动带上；
- 已有该列的工作区不重复加（`add_panel_column` 幂等）。

运行时也可以 `add_panel_everywhere(world, panel, fraction)`：把规格改成
`AllWorkspaces` 并立刻补齐所有工作区。

**移除/注销**（都对树用 `Node::remove_column`，只摘包含这块面板的那一列）：

- `remove_panel_column(world, workspace, panel)`：摘一个工作区；同步该工作区的
  根/行/区域表，聚焦时同帧同步全局 `FrameTree` / `PanelRoots` / `PanelRows` /
  `PanelSpawned`，并 despawn 面板根；
- `remove_panel_everywhere(world, panel)`：摘所有工作区，并把规格降回
  `PanelAuto::Manual`（否则 `apply_auto_panels` 下一帧会加回来）；
- `unregister_panel(world, panel)`：注册表 + 所有工作区一起删。

**旧列宽不动**：`remove_column` 只删根条带里包含该面板的那一个直接子节点；其它列
仍是 `Size::Fraction(fraction × 视口宽)`，宽度、x 一个字节不改。测试
`removing_an_auto_panel_restores_every_workspace_tree` 断言“加列 → 摘列”之后每个
工作区的树逐字节复原、根实体已 despawn、自动策略已停。

**活动列**：`ActivePanelColumn { workspace, panel }` 是 `ActiveTranscript` 之外的
另一个焦点（插件列不在转写窗格模型里）：

- `focus_panel_column(world, workspace, panel)`：设为活动列并立即 `reveal`；
- `scroll_active_column` / `column_workspace_keys` 经
  `active_panel_with_plugin` 解析活动列——**picker 打开时 picker 列优先**，否则
  插件列（根还在时）→ `ActiveTranscript` → 主转写；改宽/移动列键因此也作用于
  插件列；
- `clear_panel_column_focus(world)` 交回转写窗格。

picker 开列/关列只摘它自己的 `PanelId::List` 那列，插件列留在条带里；测试
`a_plugin_column_can_be_revealed_alongside_an_open_picker_column` 覆盖“开着
picker 时 reveal 插件列、关 picker 后插件列宽不变”。

---

## 4. 明确不做

| 不做 | 为什么 |
|---|---|
| 弹层 / 浮窗 / 自由坐标画布 | 条带里只有列；浮动会破坏 niri 的 tiling、滚动与工作区剪裁（和 `FEATHERS_UI.md` §6、`WORKSPACE_MODEL.md` §8 一致） |
| `PanelCanvas` 的 `Buffer` / 逐格绘制 | 我们的行是**值**，`bevy_ui` 自己排版；再包一层 buffer 等于重写渲染 |
| `RequestRedraw` / `RedrawArea` / 脏区门控 | 每帧重排 + 每帧 `sync_mirror`，没有“省一帧重绘”的收益；插件改数据下一帧自然可见 |
| `TuiFrame`/`TerminalWrite` 的屏幕格坐标 | 终端专有；3D 里世界矩形由 `WorldPanel::anchor` 给出，读回用 `DrawnRow` |
| 插件改内置面板的尺寸/位置 | 内置 `PanelId` 的面板行为不变；开新列不改已有列宽（niri 不变量） |
| 行内容/高亮/滚动状态的完整 `DrawnTranscript` | 读回只读几何；文本读 `RowText`，高亮/选区在 `Selection`/行样式里 |

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
  `apply_focus`（`workspaces.rs:961`）在切换时装载组件的根/树/行。
  所以 `add_panel_column` 对聚焦工作区必须同时写全局代理（§3.2 第 4 步），
  非聚焦工作区只写组件，不污染全局。
- 应用层自己 `spawn_workspace`（`workspaces.rs:807`）创建的每个工作区都是独立
  条带；插件显式对某个工作区调用 `add_panel_column` 即可。切片 2 的
  `PanelAuto::AllWorkspaces` 由 `apply_auto_panels` 在每帧 `Prepare` 统一补齐：
  已有工作区 + 之后新建的工作区都覆盖，注册表是唯一策略来源。

### 状态行

`PanelId::Status` 仍是内置固定状态行（`status.rs` 的 `status_rows`），插件加
**条目**走 `StatusParts`；给任意面板挂整条状态行是 `StatusLine`（§3.5）。两者
共享 `merged_status_items` + `status_line_row` 同一套来源/裁剪逻辑，不复制。

---

## 6. 已实现（切片 1 + 切片 2）

### 切片 1

| 内容 | 落点 |
|---|---|
| `PluginPanel` 句柄 + `PanelId::Plugin(..)` | `panels.rs` |
| 注册表（名字幂等）+ `App::register_panel` | `panel.rs` |
| 放进工作区条带、拿根、更新组件 + 聚焦全局代理 | `panel.rs::add_panel_column` |
| 滚进视口 | `panel.rs::reveal_panel_column` |
| `DrawnRow` / `drawn_row` / `drawn_rows` | `panel.rs` |

### 切片 2（2026-11-26 完成）

| 内容 | 落点 |
|---|---|
| `StatusLine` 组件 + 底部绝对定位行（任意面板；输入框落在编辑器底行） | `status.rs::StatusLine` / `draw_status_lines` |
| 来源复用：`merged_status_items` / `status_line_row`（`status_rows` 与 `StatusLine` 共用） | `status.rs` |
| 注册策略 `PanelSpec { fraction, auto }` + `PanelAuto::AllWorkspaces` | `panel.rs` |
| 自动补齐已有/新工作区 | `panel.rs::apply_auto_panels`（`WorkspacesPlugin` 的 `Prepare`） |
| `add_panel_everywhere` / `remove_panel_column` / `remove_panel_everywhere` / `unregister_panel` | `panel.rs` |
| 插件列活动焦点 `ActivePanelColumn` + `focus_panel_column` / `clear_panel_column_focus` | `panel.rs` / `workspaces.rs::active_panel_with_plugin` |
| picker 与本策略共存 | `panel.rs` 测试 `a_plugin_column_can_be_revealed_alongside_an_open_picker_column` |

测试：`panel.rs` 的 14 个测试覆盖切片 1 的注册/列/根/区域/镜像根/行世界矩形/
reveal/重复输入/非聚焦工作区/queue/树叶子，以及切片 2 的 StatusLine（插件面板、
输入框底行、多面板/宽度裁剪）、自动工作区（已有 + 新建）、移除后树逐字节复原、
picker 共存、插件列活动焦点。`cargo test --workspace --offline` 全绿（`rig_niri` lib **606**，2026-11-26 实测）。

### 切片 3：未实现（见 `REMAINING.md` §10）

- 插件命令绑定入口（注册表 + 默认键位；`InputAction` / `run_command` 已有）；
- `DrawnRow` 的可见性辅助（`effective_clip` 非空）与行文本/高亮读回扩展；
  `DrawnRow` 现在只有 `panel`/`index`/`world_rect`（`panel.rs:704`–`:713`），没有文本/高亮字段；
- `RequestRedraw` / 脏区门控：**明确不做**（每帧重排 + 每帧 `sync_mirror`，见 §1/§4）。

### 验收口径

- 内置 `PanelId` 面板的几何/行为/现有测试不回归；
- 注册→放置→根存在→面板区域非零→行世界矩形数值可断言；
- 重复注册、重复放置、找不到工作区、未注册句柄、空面板：不 panic；
- 3D 可见性用 `WorldPanel.anchor`、`clip` 与 `panel_world_rect` 的交集断言
  （headless，不跑 GUI）。
