# 空间 UI 设计纲领（会议记录 + 决策）

> **状态（2026-11-26 核对）：已废弃（作为路线图）；取代它的是下面这些文档，不要按这份做。**
>
> - **取代它的文档**：布局 → [`NIRI_SCROLL.md`](NIRI_SCROLL.md)；视觉 → [`FEATHERS_UI.md`](FEATHERS_UI.md)；
>   工作区 → [`../status/WORKSPACE_MODEL.md`](../status/WORKSPACE_MODEL.md)；插件面板 →
>   [`PANEL_CANVAS.md`](PANEL_CANVAS.md)；转写来源 → [`SHOW_AGENT.md`](SHOW_AGENT.md)。
> - **与当前口径直接冲突**：§5/§6 的 P1"卡片化/自由摆放/可拖动缩放"、§4 的 ornament/浮层、
>   §8 的"卡片在 0.6–1.0 m 前弧、可拖动可缩放"、§6 的 P2/P3 凝视/手势路线图。当前实现是
>   **niri 条带里只有列**：没有自由浮动卡片、没有弹层、不做斗兽场/圆柱体/环/两级视角
>   （用户已明确否掉）。
> - **仍可参考（不是验收项）**：§1–§3 的"丢交互不丢模型""为什么是 Bevy ECS"
>   "RealityKit 范式同构"这些判断没有变；§4/§7/§8 的 HIG 数值与无障碍要求是远期背景。
>
> **niri 的唯一用途**：`rig_niri` 移植 `rig-tui` 时，组件/窗口的**布局位置设计**参考
> `reference/niri/niri-main`。除此之外（斗兽场、圆柱体总览、环、两级视角）全部不再推进。
>
> 本文件保留 §1–§10（另含开头 §0 结论）：3D 相对终端的赢面判断、该丢什么、为什么用 Bevy ECS、
> HIG 落地清单、验收检查表。原 §11–§21 的历代几何（土楼圆柱 / 斗兽场漏斗 / 两级视角 /
> niri 绝对列带 / 环实现）已归档 → [`GEOMETRY_HISTORY.md`](../archive/GEOMETRY_HISTORY.md)，
> 只作决策历史，不再作为当前路线。
>
> 2026-11-24 计划讨论会的产出。这份文档的作用不是"介绍 3D"，而是**给重构定规矩**：
> 哪些终端设计要丢、哪些必须留、为什么用 Bevy ECS、以及拿 visionOS HIG 当标尺怎么落地。
> 以后每个 UI 改动都要过 §8 的检查表；违反时要写"例外说明"。

相关：[`PLAN.md`](../archive/PLAN.md)（实施顺序）、[`AGENT_3D_PATTERN.md`](AGENT_3D_PATTERN.md)（分层）、
[`HANDOFF.md`](../status/HANDOFF.md)（怎么跑）、[`GEOMETRY_HISTORY.md`](../archive/GEOMETRY_HISTORY.md)（已归档几何）。

---

## 0. 结论（TL;DR）

1. **丢交互，不丢模型。** 终端的键位表（PgUp/PgDn、↑↓ 历史、Tab、Esc 语义）和"整屏 = 一个界面"
   的假设要丢掉；但 rig-tui 编码的**领域模型**（turn / call / Ask / Picker / StatusItems /
   RenderBatch+look）不是"终端设计"，它是 agent 运行时的状态机——换成空间 UI 一模一样，不能丢。
2. **Bevy ECS + 3D 的理由不是"3D 好看"，而是"UI 和 agent 在同一个 World 里"。** rig-ecs 本身是
   Bevy 插件；RealityKit 用 ECS + 声明式 UI 表达的架构，Bevy 有同构的对应物（§3）。
3. **visionOS 路线可行，但要说得准确：** 我们做的是**visionOS 级设计规范的 Bevy 实现**，
   不是"visionOS 应用"（Bevy 现在没有 visionOS 目标，Swift/Metal 桥超出范围）。
   借的是它的**交互规则（HIG）和架构范式（ECS + RealityView + 空间锚点）**，不是它的 API。

---

## 1. 3D 相对终端：哪些优势是真的

| 优势 | 是不是真的 | 说明 |
|---|---|---|
| 按**注意力**分配空间，而不是按行数挤 | ✅ 真的 | 终端里转写/输入/状态抢同一个 96×28 矩形；空间里卡片各有位置与大小，深度 = 层级 |
| **直接操作**（点、拖、缩放）替代背键位 | ✅ 真的 | 终端键位是专家界面；空间 UI"看得见就能动"，工作记忆负担低 |
| 多卡片**并排比较**（多个 agent、多个工具结果） | ✅ 真的 | 终端只能滚动；空间里可以并置（这正是 rig 多 agent 的天然形态） |
| **空间记忆**：卡片留在世界里的固定位置 | ✅ 真的 | HIG 的 spatial layout 原则；终端历史只存在滚动缓冲里 |
| **多模态**：凝视 + 捏合 + 语音 + 陀螺仪 | ✅ 真的 | 头显/手机没有物理键盘，键位表在这里根本不存在 |
| 材质/光照/动效传达状态 | ✅ 但易滥用 | 玻璃、发光能表达"浮层/焦点"，但滥用会变成"科幻电影道具" |
| 沉浸/专注（dimming、只留相关卡片） | ✅ 真的 | 对应 visionOS 的 immersion 分级 |
| **长文本/代码 diff/日志**更好读 | ❌ 反了 | 这类内容 2D 面板最优，终端/编辑器更快；空间 UI 要做的是**把它包进卡片**，而不是把它 3D 化 |
| **精确打字**更快 | ❌ 反了 | 头显里打字不如物理键盘；语音适合短指令，不适合长 prompt |
| 阅读舒适度全面更好 | ❌ 不一定 | 距离、字号、位图字体放大发糊都是真问题（§7） |

**结论**：3D 的赢面是"**布局与操作**"，不是"文字渲染"。所以别把力气花在"把终端字符网格变立体"，
要花在"卡片怎么摆、怎么动手"。这也是为什么我们已有的 `frame.rs`（行列布局、折行、截断、keep 优先级）
不是废料——它是**卡片内部的排版引擎**，而卡片本身才是新的东西。

---

## 2. 该丢什么、留什么

| 层 | 终端的做法 | 空间 UI 的做法 | 处置 |
|---|---|---|---|
| **键位/操作** | PgUp/PgDn 滚动、↑↓ 历史、Tab 打勾、Esc 关闭 | 凝视+捏合、拖拽、点击、语音 | **丢**（降级为辅助输入，见下） |
| **屏幕假设** | 一个 96×28 的整屏，所有面板都是它切出来的 | 空间里 N 张卡片，各有位置/朝向/大小 | **丢**，`Placement` 降级为"flat 模式" |
| **状态行** | 屏幕底部一行文本 | 卡片 footer / ornament | 改造（数据仍是 `StatusItems`） |
| **输入框** | 3 行编辑器 + 键位提示 | composer 卡片（文本 + 附件 + 语音 + 发送） | 改造（`Editor` 状态复用） |
| **转写** | 一整块滚动缓冲 | 转写卡片（可折叠成摘要卡、可展开成阅读卡） | 改造（`transcript`/`pane` 的内容逻辑复用） |
| **picker** | 盖住整屏的列表 + preview | 浮在空中的选择卡（列表卡 + 预览卡），或 radial/菜单 | 改造（数据模型 `Picker`/`Chosen`/`Ticked` 复用） |
| **补全** | 输入框上方的弹窗 | composer 卡片上方的候选 chips | 改造 |
| **领域模型** | turn/call/Ask/Picker/Notice/status | **一模一样** | **留** |
| **look 管线** | `RenderBatch` + `RenderSystems::Render` → `LookLine` | 卡片内容的生成器 | **留** |
| **内容排版** | 行列、折行、截断、keep/min_width | 卡片内部排版 | **留**（`frame.rs` 降级为卡片内部布局） |
| **键位提示** | "Enter sends · / commands · @ files" | "捏合选择 · 拖动移动 · 语音输入" | 改造（文案按输入模态变） |

**为什么"留"这么重要**：丢掉 `Ask/Picker/StatusItems/RenderBatch` 等于丢掉唯一的、被真实终端验证过的
行为规格。空间 UI 最难的不是"画得科幻"，而是"状态机不出错"——这部分 rig 已经写好了，我们只换外观和输入。

**键盘怎么办**：不删，**降级为辅助输入**。理由：
1. visionOS 本身就支持物理键盘/切换控制，无障碍要求有替代输入；
2. 我们现在的开发机没有头显，键盘是唯一能自动测的输入（巡游截图全靠它）；
3. 桌面 3D 窗口模式下，键盘+鼠标是主输入。
实现上：所有键位处理保留成 `KeyboardFallback`，主输入路径换成"指针/凝视 → 卡片 → 动作"。

---

## 3. 为什么是 Bevy ECS + 3D

### 3.1 真正的理由：UI 和 agent 在同一个 World

- rig-ecs 是 Bevy 0.20 插件；agent / turn / call / gate 都是实体。**"工具调用变成一张卡片"就是 spawn 一个实体**，
  不需要 IPC、不需要跨语言、不需要把状态序列化过去再序列化回来。
- 同一个 `Update` 里：工具 effect 结算 → look 生成行 → 卡片内容更新 → 布局 → 镜像成 3D。
  数据只有一份，变更检测（`Changed<T>`）就是"UI 什么时候要重画"的答案。
- 这正是 rig-tui 的架构（immediate-mode 终端渲染同一个 World），我们只是把"渲染器"换成"保留模式 ECS UI + 3D"。

### 3.2 RealityKit 的范式与 Bevy 的对应

Apple 在 [Understanding the modular architecture of RealityKit](https://developer.apple.com/documentation/visionos/understanding-the-realitykit-modular-architecture)
里明确说 RealityKit 基于 **Entity Component System**；[WWDC23 10080《使用 RealityKit 构建空间体验》](https://developer.apple.com/cn/videos/play/wwdc2023/10080/?time=576)
从 9:36 讲 entities/components、23:00 讲 custom systems。它的空间 UI 架构是：

```text
SwiftUI（声明式 2D 视图/窗口）
  └─ RealityView（把 3D 内容挂进视图）
       └─ RealityKit（ECS：Entity / Component / System、空间锚点、动画、音频）
```

Bevy 侧的同构物（我们已经在用）：

| RealityKit / visionOS | 我们的对应物 | 现状 |
|---|---|---|
| `Entity` | `Entity` | ✅ 同构 |
| `Component`（含 `ModelComponent`、`Transform`） | `Component` + `Transform`/`Node` | ✅ |
| `System`（`SceneUpdateContext`, custom systems） | Bevy `System` + `Update`/`PostUpdate` 调度 | ✅ |
| `RealityView` + attachments | `MirrorRoot` + `WorldPanel`（bevy_ui 子树镜像成 3D） | ✅ 已实现（route B） |
| 空间锚点（`AnchorEntity`、`SpatialTrackingSession`） | 我们的 `FrameGeometry`/卡片 `Pose`（世界坐标） | ⚠️ 只有"一帧平面"，没有 per-card 锚点 |
| 声明式状态（`@State`/`@Observable`） | `Resource` + `Component` + 变更检测 | ✅ |
| `Model3D` / `ModelEntity` | `Mesh3d` + `UNodeMaterial3d`（bevy_luma SDF） | ✅ 部分 |
| glass 材质、vibrancy | `UNodeMaterial3d` 半透明 + 背板 | ⚠️ 要调 |
| 手势/凝视（eyes & hands） | `bevy_picking` + 自定义 gaze/pinch | ❌ 还没有 |
| 空间音频 | `bevy_audio`（`SpatialListener`/`AudioSink`) | ❌ 未接 |

**所以"迁移经验"具体是迁移三样东西**：
1. **ECS 分层**：内容（model）→ 组件（pose/size/state）→ 系统（布局、交互、动画）分离；
2. **声明式 UI 数据流**：状态变 → 组件变 → 系统重排，而不是"命令式地往画布上画"；
3. **空间规则**（HIG）：布局、视线、舒适度、材质、层级——这是"工业标准"，照它做就不会飘。

### 3.3 为什么不是 Unity/Unreal

不是"Bevy 更好"，而是**我们的 agent 运行时本来就在 Bevy 里**。用 Unity 意味着把 rig-ecs 的实体世界
和引擎世界做双份同步，把最贵的状态机部分变成跨引擎桥——那就本末倒置了。RealityKit 能做到
"iOS 开发者不用学 Unity"，靠的也是"UI 和内容在同一个声明式/ECS 体系里"；Bevy 对我们是同一个道理。

---

## 4. visionOS HIG → Bevy 的落地清单

> HIG 的官方页是前端渲染的，数值以 [Designing for visionOS](https://developer.apple.com/design/human-interface-guidelines/designing-for-visionos)
> 为准；下面标"待复核"的是二手整理里的常见数值，落地前用官方页/真机校准。

| HIG 原则 | 具体做法 | 我们的落地 |
|---|---|---|
| **Spatial layout**：窗口有物理感、位置有含义 | 卡片放在"前弧"里，离用户 0.6–1.0 m；不要放在身后/视野边缘 | `card.rs` 的 `Pose` + `space.rs` 的放置规则 |
| **Ergonomics**：可点元素够大 | 常见指引：眼睛目标 ≥ 60pt（**待复核**）。我们换算成**视角**：最小可点 **2.5°**（1.45 m 处 ≈ 6.3 cm；0.8 m 处 ≈ 3.5 cm） | 卡片交互行高度、按钮尺寸；当前行高 18px ≈ 1.28°，**卡片模式要放大到 ≥ 2.5°** |
| **Eyes & hands**：凝视选择、捏合确认；不要依赖 hover 才可发现 | 悬停高亮可以有（gaze highlight），但**功能不能只在悬停时出现**；提供可见 affordance | `input.rs`：gaze 高亮 + pinch 触发；每个卡片有可见按钮/手柄 |
| **Windows**：窗口承载"以 UI 为主"的内容，保持窗口感（标题栏、边框、尺寸） | 卡片 = 窗口：标题、边框、可拖动、可缩放（ornament 手柄） | `card.rs` 的 chrome + `ornament.rs` |
| **Ornaments**：控件挂在窗口边缘、浮在窗口外（常见：底边重叠 ~20pt，**待复核**） | 每张卡片的操作（关闭/展开/状态）做成 ornament，不塞进内容区 | `ornament.rs`：卡片底部/侧边挂件 |
| **Materials & depth**：用系统 glass 表达层级；不要硬阴影 | 半透明背板 + 细边 + 轻微深度（`WorldLayer`），不用投影 | `bevy_luma` 的 SDF 半透明 + 边框高光 |
| **Comfort**：避免在余光里做大幅运动；避免 0.2 Hz 附近的振荡（**待复核**） | 卡片入场用短、缓的动画；相机不做急加速；缩小/展开在同一位置 | 动画系统（待做）+ `camera_rig` 限速 |
| **Immersive space**：按需沉浸，不是一上来全屏 | 默认"窗口空间"；工具卡可展开成沉浸式阅读视图 | `space.rs` 的 immersion 分级 |
| **Accessibility**：键盘/VoiceOver/动态字号/高对比 | 保留键盘路径；字号可缩放；颜色不只靠色相 | `KeyboardFallback` + 主题 token |

**我们的设计 token（按视角定，不按像素定）**：

| token | 值 | 换算（1.45 m 处） |
|---|---|---|
| 正文最小字号 | 0.8° | ≈ 2.0 cm（我们现在的 13px ≈ 0.92°，够） |
| 最小可点目标 | 2.5° | ≈ 6.3 cm |
| 卡片舒适宽度 | 25–35° | ≈ 0.65–0.9 m |
| 卡片距离 | 0.6–1.0 m | — |
| 层间距（深度） | 1–3 cm | 我们的 `layer_depth = 2 cm` 正好 |
| 动画时长 | 0.2–0.35 s，缓动 | — |

---

## 5. 架构：在现有四层上加"空间层"

```text
rig-ecs           内核（agent/turn/call/gate/ask）         不变
rig-harness       服务（session/commands/settings/Ask/Pick）不变
rig_niri      空间视图层：
                    content/  转写、composer、chooser 的内容（复用现有 transcript/pane/look）
                    card/     卡片 = 内容 + 姿态 + chrome + ornament
                    space/    卡片放在哪（前弧、墙、桌）、沉浸分级
                    input/    凝视/捏合/拖拽/缩放 + 键盘兜底
feathers_in_3d    渲染（Mirror + 文字 + 排序 + 裁剪 + 拾取）  不变
```

模块计划（都放 `rig_niri`，不新开 crate）：

| 模块 | 职责 | 复用 |
|---|---|---|
| `card.rs` | `Card { kind, pose, size, chrome }`；拖动/缩放/吸附；内容仍是"行"（`PanelRows`） | `panels`/`transcript` |
| `space.rs` | 卡片放置规则（前弧、距离带、避让）、沉浸分级、焦点与 dim | `FrameGeometry` |
| `input.rs` | 拾取 → 卡片；gaze 高亮；pinch/select；drag；双指缩放；键盘兜底路由 | `feathers_in_3d::pick` |
| `ornament.rs` | 卡片边缘控件（关闭、展开、状态条目） | `status::line` |
| `composer.rs` | 输入卡片：文本 + 附件 + 语音 + 发送；补全 chips | `editor` |
| `chooser.rs` | picker → 选择卡（列表卡 + 预览卡） | `picker` |

**`frame.rs` 的新定位**：不再是"整个界面"，而是**卡片内部排版 + flat 模式**。
保留它的原因：折行、截断、keep/min_width、`CELL` 度量都是"内容排版"的正确答案，且已有 12 个单测。
`Placement` 在空间模式下只用于卡片内部（或者 flat 演示模式，`RF_FLAT=1`）。

---

## 6. 路线图

| 阶段 | 目标 | 验收 |
|---|---|---|
| **P0**（已做） | 视图插件同形、帧布局、窗格、picker（列表/预览/多选/树） | 截图 + 21 个单测 |
| **P1 卡片化** | 内容 → `Card`：转写卡、composer 卡、chooser 卡；拖动/缩放/吸附；键盘降级为辅助 | 一个房间里 3 张卡能拖、能叠、能缩放；无键盘也能完成"提问 → 看工具卡 → 确认" |
| **P2 空间布局** | 前弧/距离带放置、ornament、焦点 dim、卡片动画 | HIG 检查表 §8 全绿；卡片距离/角尺寸在 token 范围内 |
| **P3 凝视与手势** | gaze 高亮 + pinch 选择 + 拖拽 + 双指缩放；手机陀螺仪 3DoF 相机 | 无鼠标完成一次完整对话（模拟器里用鼠标当凝视） |
| **P4 沉浸与多 agent** | 沉浸式阅读卡、多 agent 卡片并置、语音输入、空间音频 | 两个 agent 并排工作，各自卡片不互相遮挡 |

**P1 是分水岭**：P1 之后，"终端帧布局"变成"卡片内部排版"，界面的主语从"屏幕"换成"卡片"。

---

## 7. 风险与硬约束（必须承认）

| 约束 | 影响 | 对策 |
|---|---|---|
| **Bevy 不能产出 visionOS 应用** | 不能上 Apple Vision Pro | 明确"设计迁移"定位；目标平台 = 桌面 3D 窗口 → OpenXR（Quest / Android XR） |
| 本机无头显、无 OpenXR 运行时 | 凝视/手势无法真机验证 | 设备抽象 + 鼠标模拟凝视/捏合；真机阶段补测 |
| 无 `/dev/dri`，LLVMpipe 软渲染 | 3D 卡片多了会掉帧 | 卡片数上限、绘制调用预算、按距离降级（LOD） |
| 位图字放大 ~6× 发糊 | 空间里近看图糊 | 换 MSDF/SDF 字，或按距离切图集 |
| `bevy_ui` 是 2D 布局 | 我们用 Mirror 绕开 | 保持 seam 小（`WorldPanel`），别让业务碰 bevy_ui |
| HIG 数值是二手整理 | 可能不准 | 用官方页/真机校准；标"待复核" |

---

## 8. 验收检查表（每个 UI 改动都过一遍）

- [ ] 交互元素在卡片里，而不是"整屏切块"
- [ ] 可点目标视角 ≥ 2.5°（换算表见 §4）
- [ ] 卡片在 0.6–1.0 m 距离带、正面 ±60° 前弧内
- [ ] 功能不依赖"悬停才出现"；有可见 affordance
- [ ] 层级用深度/材质表达，不用硬阴影
- [ ] 卡片可拖动/可缩放；缩放不破坏内部排版（折行重算）
- [ ] 键盘能完成同一件事（无障碍/桌面模式）
- [ ] 状态行/ornament 不遮挡内容
- [ ] 动画 0.2–0.35 s，无 0.2 Hz 振荡
- [ ] 帧内绘制调用与文本网格在预算内（软渲染下 ≥ 30 fps）

违反任一条时，在 PR/提交说明里写"例外 + 理由"，否则退回。

---

## 9. 决策记录

| # | 决策 | 理由 | 代价 |
|---|---|---|---|
| D1 | **丢交互，不丢模型**：键位降级为辅助输入 | 领域模型是行为规格；交互是外观 | 键位代码不删，但不再是主路径 |
| D2 | **Bevy ECS 是核心**，不引入游戏引擎桥 | agent 运行时已在同 World；RealityKit 的 ECS 范式同构 | 要自己补空间 UI 生态缺口 |
| D3 | **HIG 当标尺**，每个 UI 改动过 §8 检查表 | 避免"科幻幻想"式设计 | 有约束、迭代慢一点 |
| D4 | **`frame.rs` 降级为卡片内部排版 + flat 模式** | 折行/截断/优先级规则已被验证 | 需要"卡片"这层新抽象 |
| D5 | **不承诺 visionOS 应用**，只做设计迁移 | Bevy 无 visionOS 目标 | 对外表述要克制 |

---

## 10. 待决问题（需要拍板）

1. **P1 的卡片模型**：卡片是"自由摆放 + 吸附"，还是"固定槽位（前弧 3 个位置）"？
   前者自由但易乱，后者秩序好但不像空间 UI。倾向：自由摆放 + 轻吸附（对齐 + 距离带），HIG 允许。
2. **内容密度**：转写卡默认显示"最近 N 条 + 摘要"，还是一整段可滚动？前者更像空间 UI，后者更像终端。
3. **多 agent 的呈现**：一个 agent 一张卡（可并排/堆叠），还是主卡 + 侧栏？牵涉 rig 的 `ShownAgent`。
4. **输入**：composer 卡片是否常驻？还是"点一下才浮出"（visionOS 的 progressive disclosure）？
5. **验证手段**：在真机之前，是否接受"鼠标模拟凝视 + 键盘模拟捏合"作为 P3 的验收方式？

---

> §11–§21（土楼圆柱 / 斗兽场漏斗 / 两级视角 / 环）已归档 → docs/archive/GEOMETRY_HISTORY.md
