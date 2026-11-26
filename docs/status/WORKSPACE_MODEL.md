# 工作区模型：niri 条带 × 绝对空间坐标（2026-11-26 复核）

> 用户原话：「我们的 3d 空间是给 niri 形式的每个独立工作区绝对空间坐标，让工作区可以在空间内
> 拖拽 放置 自动整理（niri 排列工作区的方式重置工作区空间 pos 坐标）聚焦（无特殊动作只是隐藏
> 其他工作区）」
>
> 这份文档把这句话翻译成数据结构与不变量。**它是"工作区层"的唯一目标**。
> 条带**内部**的列宽/横向滚动几何按 [`NIRI_SCROLL.md`](../design/NIRI_SCROLL.md)：
> **L1–L4 已落地，L5（scroll 插值/居中）未做**；§5 的第 4–5 条不变量已按当前实现改写。

## 1. 两个层级，各管一件事

```
世界（3D 空间，绝对坐标）          工作区内部（niri 无限条带）
┌───────────────┐               ┌──────────────────────────────────────┐
│ 工作区 A  ●────┼── 位置/朝向    │ … [转写列 0.62][右列 0.38] [picker 列] → 往右超出视口 │
│      ╲        │               └──────────────────────────────────────┘
│       ╲ 相机自由│               列宽 = 视口的 Fraction；列内竖分；**没有浮层**；
│ 工作区 B  ●────┤               条带内**所有列**一起横向滚，Hints/Status 固定
└───────────────┘
```

- **世界**负责：每个工作区（= 一个 agent 会话）放在哪、朝哪、是否可见。
- **条带内部**负责：这个会话的界面怎么排——niri 的列 + 列内竖直分裂树，全部是 tile；
  列宽/滚动见 `NIRI_SCROLL.md`。

## 2. 数据结构（`rig_niri`）

```rust
/// 一个工作区 = 一个 agent 会话的完整界面 + 它在世界里的绝对位姿。
pub struct Workspace {
    pub session: u64,              // 会话 id 的占位（接真会话时换成 rig-harness 的类型）
    pub tree: layout::Node,        // 内部 niri 条带（列）：Split/Leaf，列宽是 Size::Fraction
    pub pose: Transform,           // 绝对世界坐标（不是相对相机！）
    pub order: usize,              // niri 的工作区顺序（自动整理用）
}

/// 每个工作区自己的横向滚动量（组件，挂在工作区实体上）。
pub struct WorkspaceScroll {
    pub scroll: f32,               // UI 像素，>= 0；正值把条带内容往左推
}

/// 所有工作区。`focused` 决定内容写哪条，不决定位置/可见性。
pub struct WorkspaceSet {
    pub items: Vec<Entity>,        // 工作区实体（`Workspace` 是 Component）
    pub focused: Option<Entity>,
}

/// 每个工作区自己的面板状态（挂在 `Workspace` 实体上）：
pub struct WorkspacePanels {
    pub rows: PanelRows,           // 每块面板的行
    pub roots: PanelRoots,         // 每块面板的根实体
    pub spawned: PanelSpawned,     // 增量 diff 用
    pub areas: HashMap<Entity, PanelArea>, // 这一帧的面板区域（帧内坐标）
}
```

证据：`rig_niri/src/workspaces.rs:87`（`Workspace`）、`:110`（`WorkspaceScroll`）、
`:734`（`WorkspaceSet`）、`:758`（`WorkspacePanels`）；`layout.rs:99`（`Size`，`Fraction` 在
`:105`）；`pane.rs:613`（每块窗格自己的 `TranscriptPane`）。

- 每个工作区有自己的面板根：`spawn_workspace_with_tree`（`workspaces.rs:826`）调
  `frame::default_screen_panels` 生成一套根/行，每块面板挂 `WorkspacePanel(workspace)`
  （`workspaces.rs:792`）；转写窗格的来源/标题/滚动/选中/`below` 都在它自己的
  `TranscriptPane`（`pane.rs:613`）上，所以"每个工作区各一份"包括窗格状态。
- **面板不存位置**，位置永远由「树 + 位姿」推导（`PanelArea` 每帧由 `lay_out_frame` 算，
  `frame.rs:586`）。树里的列 x 由前面列宽累加、再减 `WorkspaceScroll.scroll`
  （`layout.rs:302` 的 `placements_scrolled`）。
- 全局 `FrameTree`/`PanelRows`/`PanelRoots`/`PanelSpawned`/`PanelAreas` 是**聚焦工作区的工作副本**：
  `apply_focus`（`workspaces.rs:961`）切换时把组件的 rows/roots/spawned/tree/areas 装进全局，
  `commit_focused_panels`（`:898`）每帧把全局行/根/树写回组件。树的权威在 `Workspace.tree`。

## 3. 四个操作（这是用户要的全部）

| 操作 | 行为 | 不变量 |
|---|---|---|
| **拖拽** `drag_workspace` | 指针在某个工作区上拖动 → `pose.translation` 跟着动（`workspaces.rs:1268`） | 只动被拖的那一个；其它工作区的 `pose` 一个字节不动 |
| **放置** `place_workspace` | 显式设置某个工作区的位姿（拖拽落点/命令，`:1255`） | 同上 |
| **自动整理** `arrange_workspaces`（= niri 排列工作区的方式） | 按 `order` **重算所有位姿**，覆盖手放的位置（`:1228`） | niri 的工作区是**垂直堆叠**（见 §4），所以整理后顺序 = `order` 的垂直序列；同一次整理里每个工作区的间距一致 |
| **聚焦** `focus_workspace` | 只决定**内容写哪个工作区**（输入/转写进哪条）；**不隐藏任何工作区**、不动相机、不做动画、不改几何（`:870`） | 焦点变化前后：所有面板根仍是 `Visibility::Inherited`、所有 `pose` 与面板锚点不变（测试 `focus_changes_nothing_visible` `:2800`、`focus_does_not_move_the_camera` `:2838`） |

**"无特殊动作"按字面实现**：聚焦不移动相机、不缩放、不淡入淡出、**也不隐藏**任何工作区。
它只决定"新内容写进哪条条带"。相机怎么走到那儿是相机的事（自由相机 + 导航）。
（2026-11-25 修正：曾经实现成"聚焦隐藏其它"，用户否掉了——niri 没有隐藏工作区这回事。）

> 注意：`workspaces.rs:49`–`:52` 的模块文档与 `:868` 的函数注释还残留"聚焦只改可见性 /
> 把其它工作区设成 Hidden"的旧说法；实现里没有任何 `focus` 路径写 `Visibility`，
> 测试 `focus_changes_nothing_visible` 断言两边都是 `Inherited`。**以实现与测试为准。**

## 4. 自动整理为什么是"垂直堆叠"

niri 的原话（`README.md`）：*Windows are arranged in columns on an infinite strip going to the
right. Workspaces are dynamic and arranged vertically.* —— 窗口往右排，**工作区竖着排**。

所以 `arrange_workspaces` 把 `order` 映射成一条竖直的序列：

```
      [工作区 0]   y = 0
      [工作区 1]   y = -(高 + 间距)
      [工作区 2]   y = -2(高 + 间距)   …
```

- **高度取帧高**：`workspace_strip_height(frame) = FrameSize::rows × ROW_HEIGHT × UNITS_TO_WORLD`
  （`workspaces.rs:1203`），不是"输入 + 状态 + 最小转写"那种估算——条带内部怎么分是分裂树的事。
- 间距取 niri 默认 gap 16 px 折算的世界单位（`16 × 0.0018 = 0.0288 m`，`WORKSPACE_GAP`
  `workspaces.rs:1196`）；步长 `workspace_step = 条带高度 + 间距`（`:1208`）。x/z 归 0，
  朝向统一 `Quat::IDENTITY`。
- **帧高变了要按新帧高重排**：应用层 `restack_on_resize`（`rig_harness_in_3d/src/workspaces_demo.rs:557`）
  在 `FrameSize` 变化时只重排**自动位姿**（x=z=0、朝向单位阵、y=-index×旧步长的那几个），
  用户手放的工作区留在原地；这样相邻条带不会因为旧帧高而竖直重叠。
  测试：`resizing_the_frame_restacks_workspaces_so_strips_do_not_overlap`（`:654`）、
  `a_hand_placed_workspace_survives_a_frame_resize`（`:692`）、
  `resizing_restacks_only_the_untouched_workspaces`（`:730`）。

## 5. 不变量（拿它们当测试）

1. 新工作区出现 **不能** 改变已有工作区的位姿或尺寸（niri 第一原则；测试
   `a_new_workspace_does_not_touch_the_existing_ones`，`workspaces.rs:2740`）。
2. 聚焦 / 取消聚焦 **不改变任何位姿、也不改任何 `Visibility`**，只决定"新内容写哪条"；
   非聚焦工作区只是不再接受拖拽（抓取区域，`workspace_under_pointer`，`workspaces.rs:1462`），
   画面照常显示、点选照常。测试 `focus_changes_nothing_visible`（`:2800`）。
3. `arrange_workspaces` 之后：顺序 = `order`、间距一致（= 条带高度 + 16px gap）、x/z 为 0、
   朝向单位阵；步长随当前帧高变化（测试 `arrange_stacks_workspaces_vertically_in_order`（`:2584`）、
   `arrange_uses_the_frame_height_and_the_niri_gap`（`:2678`））。
4. **列宽是视口的比例，不是同层份额**（L1）：`Size::Fraction(f)` 的宽度恒为 `f × 视口宽`
   （`layout.rs:99`/`:105`）；默认树转写 0.62、右列 0.38（`:74`/`:80`、`main_row` `:1137`）。
   开新列只 `push` 一个 `Child::fraction(0.5, …)`，**已有列的宽度与 x 一个字节都不动**
   （`layout.rs:739`，测试 `appending_a_column_keeps_existing_columns_and_overflows_the_area` `:1745`、`a_fraction_column_keeps_its_viewport_width_when_siblings_grow` `:1781`）。
5. **条带内一切列都滚、Hints/Status 固定**（L2/L3）：横向滚动把含转写的最外层横向 split 里的
   **所有列**（转写、picker/追加列、右列的 Spacer/Confirm/Complete/Input）一起平移；
   只有 Hints 与 Status 两行不动——它们在根竖 split 里、条带的纵向范围之外
   （判定循环 `layout.rs:322`–`:327`；测试
   `scrolling_moves_the_whole_strip_but_not_hints_or_status`（`:1958`）、
   `with_the_picker_open_the_whole_strip_scrolls_together`（`:2011`））。
   滚动只改 `WorkspaceScroll`（`workspaces.rs:110`/`:406`），树的列宽/列 x 一个字节不变；
   超出视口的面板仍在树上，由 `WorldPanel::clip` 裁剪（`frame.rs:411`、`world3d.rs:79`）。
   > `layout.rs:292`–`:300` 的函数 doc 还残留"右列是固定 chrome"的旧说法，与判定循环和
   > 上面两个测试相反；**以代码与测试为准**。
6. 相机在空间里任意移动，所有工作区位姿保持不变（世界坐标，不是相对相机；测试
   `moving_the_camera_does_not_move_the_workspaces`，`:3023`）。
7. 拖拽中其它工作区不动（`drag_workspace_moves_only_the_target`，`:2720`）；拖拽结束位置被记住
   （`releasing_a_drag_keeps_the_dropped_position`，`:2950`，不会自动弹回，除非调用整理）。
8. 帧高变化只重排自动位姿，手放的不动；重排后相邻条带不重叠（§4 的 `restack_on_resize` 测试）。

## 6. 对现有代码的影响（2026-11-26 复核：都已落地）

- crate 名已定 `rig_niri`，不再改名。
- 每工作区一份面板状态：`WorkspacePanels` 存 rows/roots/spawned/areas（`workspaces.rs:758`），
  树存 `Workspace.tree`；全局那几份是聚焦工作区的代理，`apply_focus`（`:961`）装载、
  `commit_focused_panels`（`:898`）写回。横向滚动量是每工作区的 `WorkspaceScroll`（`:110`）。
- `Placement::Over`/`Within` 已全部删掉（`grep -rE 'Placement::(Over|Within)' rig_niri/src`
  为空），picker/确认/补全都是条带里的 tile；`Placement` 现在只剩 `Tree`/`Rest`
  （`frame.rs:309`）。
- 应用层 `rig_harness_in_3d` 是"装一个 `WorkspaceSet` + 相机 + 导航手势"
  （`workspaces_demo.rs`，它自己的 F1–F8 聚焦、F10 整理；**这是 `workspaces_demo` 的编号，
  和 `FEATHERS_UI.md` 的 F1–F10 无关**），并负责 `restack_on_resize`（`:557`）。

## 7. 分批（每批都能单独验收）

> 2026-11-26 复核：五批均已完成（`WorkspacePanels` = 批 2，四个操作 = 批 3，
> picker/确认 tile = 批 4，`workspaces_demo` 的 F1–F8/F10 = 批 5）；布局的 L1–L4 也已落地。
> 下表保留当时的分批与验收标准；“255 个测试”是当时的基线，现在 `rig_niri` 是 **606**
> （2026-11-26 08:2x，`cargo test --workspace --offline` 全绿）。

| 批 | 内容 | 验收 |
|---|---|---|
| 1 | 改名 `rig_niri`；`Workspace`/`WorkspaceSet` 落地 | 全仓编译；255 个测试保持全过 |
| 2 | 每个工作区一份面板状态（`rows`/`roots`/`spawned`/`areas` + `tree`） | 两个假工作区各自显示不同内容（headless 断言） |
| 3 | 四个操作 + 不隐藏 | §5 的 6 条不变量逐条测试 |
| 4 | 删 `Placement::Over`，picker/补全/确认 tile 化 | 无浮层：`grep -r 'Placement::Over' src` 为空 |
| 5 | 应用层：3 个假工作区 + 拖拽/整理/聚焦按键 | 跑起来能拖、能整理、能聚焦 |

## 8. 条带里没有"非 tile"的东西（已完成）

niri 的设计原则（`Development:-Design-Principles.md`）：
> Fullscreen windows are a normal part of the scrolling layout. They aren't on some
> "special" layer that covers everything; instead, they are normal tiles that you can
> switch away from.

所以条带里**不存在浮层**。当前状态：

| 东西 | 现在是 | 目标 |
|---|---|---|
| 转写 | tile（左列，视口宽 0.62） | ✅ |
| 输入框 | tile（右列，贴底；树里初始 4 行 72px，运行时按内容 3–10 行，`editor::sync_editor_tile` 改高度、顶部 Spacer 吸收差额） | ✅ |
| 状态行 | tile（右列，固定 2 行 = 36px） | ✅ |
| 补全 | tile（右列，打开时 6 行） | ✅ |
| 右列顶部留白 | `PanelId::Spacer`（只吃空间，不画） | ✅ |
| 键位提示行 | ✅ 独立 1 行 tile（`PanelId::Hints`，18px，无内边距否则 1 行放不下 1 行字） | — |
| 确认行（Ask） | ✅ 独立 tile（`PanelId::Confirm`，无 Ask 时 0 高度） | — |
| picker | ✅ 条带**最右边新加一列**：宽 = 0.5 × 视口，**已有列的宽度/x 一个字节不动**（`layout.rs:739`、`picker.rs:868`；测试 `opening_a_picker_appends_a_half_viewport_column_at_the_right`，`picker.rs:2222`）。关掉后整列摘除、树逐字节复原 | 已完成 |

**状态行/提示行横跨整条**（2026-11-25 晚）：树的根是竖 split——

```
Vertical
├── Horizontal (Proportion 1)   ← 主体（niri 条带；内部所有列一起横向滚）
│   ├── Transcript (Fraction 0.62 视口宽)
│   └── Vertical (Fraction 0.38 视口宽)
│       ├── Spacer (Proportion 1)
│       ├── Confirm (Fixed 0 px)
│       ├── Complete (Fixed 0 px)
│       └── Input (Fixed 72 px = 4 行，初始值；运行时由 `editor::sync_editor_tile` 改成 3–10 行)
├── Hints  (Fixed 18 px = 1 行)   ← 条带之外，**不滚**
└── Status (Fixed 36 px = 2 行)   ← 条带之外，**不滚**
```

树里再追加的 picker/插件列都进主体那条 `Horizontal`：它们是条带内容，跟着一起滚
（`layout.rs:739` 的 `append_column`、`:302` 的 `placements_scrolled`）。

**教训（写下来避免再犯）**：`Size::Fixed` 的单位是 **UI 像素**，`walk()` 里只乘一次
`PX_TO_METERS` 换成米。所以树里写 `Child::fixed(4.0 * ROW_HEIGHT, ...)`（=72 px），
**不要**再乘 `PX_TO_METERS`——乘了会得到 0.06 px，固定子塌掉，同层的"第一个子吃剩余"
就会把位置全打乱，看起来像兄弟重叠。`layout::walk` 里已经加了运行时告警
（`Fixed` 在 (0, 1) 之间就 warn），下一个人踩到时能立刻看见。

（此前把原因误判成"`rects()` 嵌套二次换算"，实测不是：`walk()` 每层只换算一次；
故障来自树里写错了单位。`remove_column` 倒是真有 bug——它只在根的直接子里找列，
列挂进嵌套的横向 split 之后会把整个主体行删掉，已改成递归，`layout.rs:827`。）

验收标准：`grep -rE 'Placement::(Over|Within)' rig_niri/src` 为空；
`cargo test --workspace --offline` 全绿（`rig_niri` 606）。
