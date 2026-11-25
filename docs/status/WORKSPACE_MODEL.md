# 工作区模型：niri 条带 × 绝对空间坐标（2026-11-25 定案）

> 用户原话：「我们的 3d 空间是给 niri 形式的每个独立工作区绝对空间坐标，让工作区可以在空间内
> 拖拽 放置 自动整理（niri 排列工作区的方式重置工作区空间 pos 坐标）聚焦（无特殊动作只是隐藏
> 其他工作区）」
>
> 这份文档把这句话翻译成数据结构与不变量。**它是当前唯一的布局目标**，比 `PLAN.md` 的旧条目优先。

## 1. 两个层级，各管一件事

```
世界（3D 空间，绝对坐标）          工作区内部（niri 无限条带）
┌───────────────┐               ┌──────────────────────────────────────┐
│ 工作区 A  ●────┼── 位置/朝向    │ … [转写块][转写块] [补全][输入][状态] → 往右无限 │
│      ╲        │               └──────────────────────────────────────┘
│       ╲ 相机自由│               列宽用 niri 预设；列内竖分；**没有浮层**
│ 工作区 B  ●────┤
└───────────────┘
```

- **世界**负责：每个工作区（= 一个 agent 会话）放在哪、朝哪、是否可见。
- **条带内部**负责：这个会话的界面怎么排——niri 的分裂树，全部是 tile。

## 2. 数据结构（`rig_niri`）

```rust
/// 一个工作区 = 一个 agent 会话的完整界面 + 它在世界里的绝对位姿。
pub struct Workspace {
    pub session: SessionId,        // 来自 rig-harness，用于接真会话
    pub tree: layout::Node,        // 内部 niri 分裂树（Split/Leaf）
    pub pose: Transform,           // 绝对世界坐标（不是相对相机！）
    pub order: usize,              // niri 的工作区顺序（自动整理用）
}

/// 所有工作区。`focused` 决定可见性，不影响位置。
pub struct WorkspaceSet {
    pub items: Vec<Workspace>,
    pub focused: Option<SessionId>,
}
```

每个 `Leaf(PanelId)` 的几何 = `tree.rects(area)` 算出的帧内矩形 → 乘 `pose` 变成世界坐标。
**面板不存位置**，位置永远由「树 + 位姿」推导。

## 3. 四个操作（这是用户要的全部）

| 操作 | 行为 | 不变量 |
|---|---|---|
| **拖拽** `drag_workspace` | 指针在某个工作区上拖动 → `pose.translation` 跟着动 | 只动被拖的那一个；其它工作区像素级不动 |
| **放置** `place_workspace` | 显式设置某个工作区的位姿（拖拽落点/命令） | 同上 |
| **自动整理** `arrange_workspaces`（= niri 排列工作区的方式） | 按 `order` **重算所有位姿**，覆盖手放的位置 | niri 的工作区是**垂直堆叠**（见 §4），所以整理后顺序 = `order` 的垂直序列；同一次整理里每个工作区的间距一致 |
| **聚焦** `focus_workspace` | 只决定**内容写哪个工作区**（输入/转写进哪条）；**不隐藏任何工作区**、不动相机、不做动画、不改几何 | niri 的工作区是竖着摆的，靠视口移动去看，不靠隐藏（`README.md`：Workspaces are dynamic and arranged vertically） |

**"无特殊动作"按字面实现**：聚焦不移动相机、不缩放、不淡入淡出、**也不隐藏**任何工作区。
它只决定"新内容写进哪条条带"。相机怎么走到那儿是相机的事（自由相机 + 导航）。
（2026-11-25 修正：曾经实现成"聚焦隐藏其它"，用户否掉了——niri 没有隐藏工作区这回事。）

## 4. 自动整理为什么是"垂直堆叠"

niri 的原话（`README.md`）：*Windows are arranged in columns on an infinite strip going to the
right. Workspaces are dynamic and arranged vertically.* —— 窗口往右排，**工作区竖着排**。

所以 `arrange_workspaces` 把 `order` 映射成一条竖直的序列：

```
      [工作区 0]   y = 0
      [工作区 1]   y = -(高 + 间距)
      [工作区 2]   y = -2(高 + 间距)   …
```

间距取 niri 默认 gap 16 px 折算的世界单位（`16 × 0.0018 = 0.0288 m`），高度取条带的
实际高度（输入+状态+最小转写高度）。x/z 归 0（同一条竖线上），朝向统一朝相机默认方向。

## 5. 不变量（拿它们当测试）

1. 新工作区出现 **不能** 改变已有工作区的位姿或尺寸（niri 第一原则）。
2. 聚焦 / 取消聚焦 **不改变任何位姿**，只改可见性。
3. `arrange_workspaces` 之后：顺序 = `order`、间距一致、x/z 为 0、朝向一致。
4. 工作区内部：`rects()` 的叶子面积之和 = 条带面积（已有 15 个测试保证）。
5. 相机在空间里任意移动，所有工作区位姿保持不变（世界坐标，不是相对相机）。
6. 拖拽中其它工作区不动；拖拽结束位置被记住（不会自动弹回，除非调用整理）。

## 6. 对现有代码的影响

- `rig_niri` → **`rig_niri`**（改名）：这个 crate 就是"条带内部那套"，名字要对得上。
- `PanelRows`/`PanelRoots`/`FrameTree` 现在是全局单例 → 要变成 **每个工作区一份**（否则两条条带
  共用一个转写）。这是第 1 个要做的结构改动。
- `Placement::Over`（picker/确认/补全浮层）全部删掉，改成条带里的 tile。
- 应用层 `rig_harness_in_3d` 不再"装一个界面"，而是"装一个 `WorkspaceSet` + 相机 + 导航手势"。

## 7. 分批（每批都能单独验收）

| 批 | 内容 | 验收 |
|---|---|---|
| 1 | 改名 `rig_niri`；`Workspace`/`WorkspaceSet` 落地 | 全仓编译；255 个测试保持全过 |
| 2 | 每个工作区一份 `PanelRows`/`PanelRoots`/`FrameTree` | 两个假工作区各自显示不同内容（headless 断言） |
| 3 | 四个操作 + 可见性 | §5 的 6 条不变量逐条测试 |
| 4 | 删 `Placement::Over`，picker/补全/确认 tile 化 | 无浮层：`grep -r 'Placement::Over' src` 为空 |
| 5 | 应用层：3 个假工作区 + 拖拽/整理/聚焦按键 | 跑起来能拖、能整理、能聚焦 |

## 8. 条带里没有"非 tile"的东西（进行中）

niri 的设计原则（`Development:-Design-Principles.md`）：
> Fullscreen windows are a normal part of the scrolling layout. They aren't on some
> "special" layer that covers everything; instead, they are normal tiles that you can
> switch away from.

所以条带里**不存在浮层**。当前状态：

| 东西 | 现在是 | 目标 |
|---|---|---|
| 转写 | tile（左列） | ✅ |
| 输入框 | tile（右列，固定 4 行，贴底） | ✅ |
| 状态行 | tile（右列，固定 2 行） | ✅ |
| 补全 | tile（右列，打开时 6 行） | ✅ |
| 右列顶部留白 | `PanelId::Spacer`（只吃空间，不画） | ✅ |
| 键位提示行 | ✅ 独立 1 行 tile（`PanelId::Hints`，无内边距否则 1 行放不下 1 行字） | — |
| 确认行（Ask） | ✅ 独立 tile（`PanelId::Confirm`，无 Ask 时 0 高度） | — |
| picker | ✅ 条带**最右边新加一列**（打开时已有列等比缩到 2/3，关掉后逐字节复原） | — |

**状态行/提示行已横跨整条**（2026-11-25 晚）：树的根改成竖 split——

```
Vertical
├── Horizontal (Proportion 1)   ← 主体
│   ├── Transcript (Proportion 2)
│   └── Vertical (Proportion 1)
│       ├── Spacer (Proportion 1)
│       ├── Confirm (Fixed 0 px)
│       ├── Complete (Fixed 0 px)
│       └── Input (Fixed 72 px = 4 行)
├── Hints  (Fixed 18 px = 1 行)
└── Status (Fixed 36 px = 2 行)
```

**教训（写下来避免再犯）**：`Size::Fixed` 的单位是 **UI 像素**，`walk()` 里只乘一次
`PX_TO_METERS` 换成米。所以树里写 `Child::fixed(4.0 * ROW_HEIGHT, ...)`（=72 px），
**不要**再乘 `PX_TO_METERS`——乘了会得到 0.06 px，固定子塌掉，同层的"第一个子吃剩余"
就会把位置全打乱，看起来像兄弟重叠。`layout::walk` 里已经加了运行时告警
（`Fixed` 在 (0, 1) 之间就 warn），下一个人踩到时能立刻看见。

（此前把原因误判成"`rects()` 嵌套二次换算"，实测不是：`walk()` 每层只换算一次；
故障来自树里写错了单位。`remove_column` 倒是真有 bug——它只在根的直接子里找列，
列挂进嵌套的横向 split 之后会把整个主体行删掉，已改成递归。）

验收标准：`grep -r 'Placement::Over\|Placement::Within' rig_niri/src` 为空。
