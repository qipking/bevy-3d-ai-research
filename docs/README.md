# 文档索引

> **niri 的唯一用途**：`rig_niri` 移植 `rig-tui` 时，组件/窗口的**布局位置设计**参考
> `reference/niri/niri-main`。除此之外（斗兽场、圆柱体总览、环、两级视角）全部不再推进；
> 相关设计已归档到 [`archive/`](archive/)，只有历史价值，不要照着做。
>
> **界面口径（2026-11-26）**：不再追求 rig-tui 终端像素还原，界面用 `bevy_feathers` 的标准 UI
> 重组（[`design/FEATHERS_UI.md`](design/FEATHERS_UI.md)，F1–F5 已完成，F6 切片 1/2、F7、F8、
> F10 也已落地）；布局用 niri 的列/滚动（[`design/NIRI_SCROLL.md`](design/NIRI_SCROLL.md)，
> **L1–L4 已实现，L5 未做**）。内容/行为仍对照 rig-tui。
>
> 目录约定：**`design/` = 设计与决策（看"为什么"）、`status/` = 当前状态与计划（看"现在到哪"）、
> `reports/` = 一次性排查/验证报告（看"当时查到了什么"）、`archive/` = 历史记录（只有历史价值，不要照着做）。**
> 每份文档顶部都有状态行：`当前` / `历史` / `已暂缓` / `已归档`，先看那个再读。

## 先读这几份（按顺序）

| 文档 | 一句话 | 状态 |
|---|---|---|
| [`status/HANDOFF.md`](status/HANDOFF.md) | 交接页：新会话第一页，定位、怎么跑、进度、红线 | **当前**，先读 |
| [`status/ARCHITECTURE.md`](status/ARCHITECTURE.md) | 实现架构：五层栈、四条数据链、帧内顺序、状态归属 | **当前，读完 HANDOFF 读它** |
| [`status/WORKSPACE_MODEL.md`](status/WORKSPACE_MODEL.md) | 工作区模型：niri 条带 × 绝对空间坐标、四个操作、不变量 | 当前（**工作区层**的唯一目标；条带内部几何见 `design/NIRI_SCROLL.md`） |
| [`design/SPATIAL_UI.md`](design/SPATIAL_UI.md) | 空间 UI 纲领：3D 相对终端哪些赢面是真的、该丢什么 | 当前（规矩） |

## status/ —— 现在到哪

| 文档 | 一句话 | 状态 |
|---|---|---|
| [`HANDOFF.md`](status/HANDOFF.md) | 交接页：新会话第一页（定位、怎么跑、进度、剩余项、红线） | 当前 |
| [`ARCHITECTURE.md`](status/ARCHITECTURE.md) | **实现架构**：五层栈、四条数据链、帧内顺序、状态归属 | **当前，读完 HANDOFF 读它** |
| [`PARITY.md`](status/PARITY.md) | 逐模块移植审计：谁已移植、谁部分、谁没开始、缺哪个公开 API | **当前，查进度看它**（口径已改：内容对照 rig-tui，视觉/布局以 feathers+niri 为准） |
| [`REMAINING.md`](status/REMAINING.md) | 还没做的事，按用户能感知的价值排序（含依赖与工作量） | **当前，排期看它** |
| [`WORKSPACE_MODEL.md`](status/WORKSPACE_MODEL.md) | **工作区模型**：niri 条带 × 绝对空间坐标、四个操作、六条不变量 | **当前，工作区层的唯一目标**（条带内部见 `design/NIRI_SCROLL.md`） |

## design/ —— 为什么这么做

| 文档 | 一句话 |
|---|---|
| [`SPATIAL_UI.md`](design/SPATIAL_UI.md) | 设计纲领 §1–§10：§1 是"3D 赢面"判断表、该丢什么、ECS 理由、HIG 清单、验收检查表（§11 起的几何已归档） |
| [`AGENT_3D_PATTERN.md`](design/AGENT_3D_PATTERN.md) | 分层范式：内核 / 视图 / 渲染 / 应用各管什么，扩展点在哪 |
| [`RIG_NIRI_MOBILE_PLAN.md`](design/RIG_NIRI_MOBILE_PLAN.md) | 移动端方案（陀螺仪相机，无 VR） |
| [`VR_ASSISTANT_STACK.md`](design/VR_ASSISTANT_STACK.md) | 技术栈研究：feathers + rig + 路线 B 的可行性 |
| [`FEATHERS_UI.md`](design/FEATHERS_UI.md) | 界面重组：改用 bevy_feathers 标准 UI（不再追求终端像素还原）；F1–F5 已完成，F6 切片 1/2、F7、F8、F10 已落地，记了 token/组件/仍是常量的地方/为什么不用现成组件。**当前（视觉口径）** |
| [`NIRI_SCROLL.md`](design/NIRI_SCROLL.md) | 布局优化：niri 式滚动平铺（列宽/横向滚动/聚焦）；**L1–L4 已实现，L5 未做**（scroll 插值 / center-focused）。**当前（布局计划）** |
| [`PANEL_CANVAS.md`](design/PANEL_CANVAS.md) | 开放面板画布（F6）：插件面板注册、作为 niri 列放进条带、行世界矩形读回、`StatusLine`、自动工作区/活动插件列 | 当前（**切片 1/2 已完成**，剩切片 3 命令绑定） |
| [`SHOW_AGENT.md`](design/SHOW_AGENT.md) | 看别的会话（S1–S6）：子 agent / 已保存会话 / 压缩条目 / 重启前调用事实 | 当前（**S1–S4 已完成**，S5–S6 未做） |

## archive/ —— 历史记录

> **只有历史价值，不要照着做。** 这些是被取代或暂缓的设计线；保留原文是为了不丢决策过程与当时的实测数据。

| 文档 | 一句话 | 状态 |
|---|---|---|
| [`GEOMETRY_HISTORY.md`](archive/GEOMETRY_HISTORY.md) | 原 SPATIAL_UI.md §11–§21：土楼圆柱 / 斗兽场漏斗 / 两级视角 / niri 列带 / 环实现，原文整段保留 | **已归档**，仅历史参考；当前实现见 `status/ARCHITECTURE.md` |
| [`WATCHWALL.md`](archive/WATCHWALL.md) | S1「监看多个 agent」的可读性测算与三层表征（原 status/） | **已归档**，仅历史参考；视觉研究已停，功能面见 `status/REMAINING.md` |
| [`PLAN.md`](archive/PLAN.md) | 旧的实施顺序与进度表（2026-11-24 方向会议） | **已归档**，仅历史参考；进度看 `status/PARITY.md`、`status/REMAINING.md` |
| [`PUBLISH.md`](archive/PUBLISH.md) | 旧的发布准备（workspace 化 + 改名后已过时） | **已归档**，仅历史参考；当前发布记录见 [`reports/PUBLISH_GITHUB.md`](reports/PUBLISH_GITHUB.md) |

## reports/ —— 一次性结论

| 文档 | 一句话 |
|---|---|
| [`VERIFICATION_REPORT.md`](reports/VERIFICATION_REPORT.md) | 路线 B（UI 镜像成 3D 网格）的验证报告与实测数据 |
| [`occlusion_report.md`](reports/occlusion_report.md) | "面板被清空导致立方体像浮到前面"的排查结论（`unhover_row` 缺判断） |
| [`official-tui.md`](reports/official-tui.md) | 官方 `rig 0.47.0` 真实界面抓取（2026-11-25，伪终端 + 去 ANSI）：空会话/运行中长什么样、启动命令、原始字节都在 `reference/rig-official/` |
| [`PUBLISH_GITHUB.md`](reports/PUBLISH_GITHUB.md) | 上传到 GitHub 的三种凭证方式与已上传地址（2026-11-25） |

## 写新文档的规矩

1. 放对目录，并在本索引登记一行；
2. 顶部写状态行（`当前` / `历史` / `已暂缓` / `已归档`）和一句话结论；
3. 结论要能被验证——给命令、给数字、给日志，而不是"应该可以"；
4. 旧文档被取代时**不要删**，把状态改成 `历史` 并写清被谁取代；整条设计线停掉时移到 `archive/`，
   顶部标 `已归档，仅历史参考；当前实现见 <文档>`，原文保留。

## 目录结构
```
bevy_3d_ai_research/
├── README.md            ← 新的总览：分层、目录、怎么跑
├── docs/                ← 所有文档
│   ├── README.md          索引（先读哪个、哪份是历史）
│   ├── design/            为什么这么做：SPATIAL_UI（§1–§10）、AGENT_3D_PATTERN、移动端方案、技术栈、
│   │                      FEATHERS_UI（视觉口径）、NIRI_SCROLL（布局 L1–L5）、PANEL_CANVAS（F6）、SHOW_AGENT（S1–S6）
│   ├── status/            现在到哪：HANDOFF（交接页）、ARCHITECTURE（实现架构）、PARITY（移植审计）、
│   │                      REMAINING（还剩什么）、WORKSPACE_MODEL
│   ├── reports/           一次性结论：VERIFICATION_REPORT、occlusion_report、official-tui、PUBLISH_GITHUB
│   └── archive/           历史：GEOMETRY_HISTORY、WATCHWALL、PLAN、PUBLISH（均已归档，仅历史参考）
├── rig_harness_in_3d/   应用层（= plugins/rig-tui 的位置）
├── rig_niri/            视图层（= rig-tui 这个 crate 的位置，26 个模块）
├── feathers_in_3d/      渲染层（普通 bevy_ui → 3D 面板）
├── vendor/bevy_luma/    上游 SDF 材料（vendored，有本地 alpha_mode 改动）
├── reference/           上游源码只读：rig-official、rig-harness-src、rig-tui-inspect、niri
├── experiments/         一次性探针：spike、occlusion_probe
├── render/              截图证据（rf_niri_app、light）+ README 说明哪张还有效
├── shots/               `RF_SHOT_DIR` 的默认输出（单次巡游的散图）
└── tools/               脚本（字体、jjx、子集化）
```
