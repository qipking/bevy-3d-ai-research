# 文档索引

> **niri 的唯一用途**：`rig_niri` 移植 `rig-tui` 时，组件/窗口的**布局位置设计**参考
> `reference/niri/niri-main`。除此之外（斗兽场、圆柱体总览、环、两级视角）全部不再推进；
> 相关设计已归档到 [`archive/`](archive/)，只有历史价值，不要照着做。
>
> 目录约定：**`design/` = 设计与决策（看"为什么"）、`status/` = 当前状态与计划（看"现在到哪"）、
> `reports/` = 一次性排查/验证报告（看"当时查到了什么"）、`archive/` = 历史记录（只有历史价值，不要照着做）。**
> 每份文档顶部都有状态行：`当前` / `历史` / `已暂缓`，先看那个再读。

## 先读这三份（按顺序）

| 文档 | 一句话 | 状态 |
|---|---|---|
| [`status/HANDOFF.md`](status/HANDOFF.md) | 交接页：分层、目录、怎么跑、踩过的坑 | **当前**，先读 |
| [`status/PLAN.md`](status/PLAN.md) | 把 rig-harness 的 UI 换成 3D 的实施计划与进度 | 当前（进度表） |
| [`design/SPATIAL_UI.md`](design/SPATIAL_UI.md) | 空间 UI 纲领：3D 相对终端哪些赢面是真的、该丢什么 | 当前（规矩） |

## status/ —— 现在到哪

| 文档 | 一句话 | 状态 |
|---|---|---|
| [`HANDOFF.md`](status/HANDOFF.md) | 交接页（含 jj 事故记录与规则） | 当前 |
| [`PLAN.md`](status/PLAN.md) | 实施顺序、每步做到哪、下一步（环/总览已归档，仅留布局参考） | 当前 |
| [`PARITY.md`](status/PARITY.md) | 逐模块移植审计：谁已移植、谁部分、谁没开始、缺哪个公开 API | **当前，查进度看它** |

## design/ —— 为什么这么做

| 文档 | 一句话 |
|---|---|
| [`SPATIAL_UI.md`](design/SPATIAL_UI.md) | 设计纲领 §1–§10：§1 是"3D 赢面"判断表、该丢什么、ECS 理由、HIG 清单、验收检查表（§11 起的几何已归档） |
| [`AGENT_3D_PATTERN.md`](design/AGENT_3D_PATTERN.md) | 分层范式：内核 / 视图 / 渲染 / 应用各管什么，扩展点在哪 |
| [`RIG_NIRI_MOBILE_PLAN.md`](design/RIG_NIRI_MOBILE_PLAN.md) | 移动端方案（陀螺仪相机，无 VR） |
| [`VR_ASSISTANT_STACK.md`](design/VR_ASSISTANT_STACK.md) | 技术栈研究：feathers + rig + 路线 B 的可行性 |

## archive/ —— 历史记录

> **只有历史价值，不要照着做。** 这些是被取代或暂缓的设计线；保留原文是为了不丢决策过程与当时的实测数据。

| 文档 | 一句话 | 状态 |
|---|---|---|
| [`GEOMETRY_HISTORY.md`](archive/GEOMETRY_HISTORY.md) | 原 SPATIAL_UI.md §11–§21：土楼圆柱 / 斗兽场漏斗 / 两级视角 / niri 列带 / 环实现，原文整段保留 | 历史（已归档，不再推进） |
| [`WATCHWALL.md`](archive/WATCHWALL.md) | S1「监看多个 agent」的可读性测算与三层表征（原 status/） | 已暂缓（归档留存，不做视觉研究） |

## reports/ —— 一次性结论

| 文档 | 一句话 |
|---|---|
| [`VERIFICATION_REPORT.md`](reports/VERIFICATION_REPORT.md) | 路线 B（UI 镜像成 3D 网格）的验证报告与实测数据 |
| [`occlusion_report.md`](reports/occlusion_report.md) | "面板被清空导致立方体像浮到前面"的排查结论（`unhover_row` 缺判断） |
| [`PUBLISH.md`](reports/PUBLISH.md) | 三个 crate 的发布准备（仓库布局、版本策略） |

## 写新文档的规矩

1. 放对目录，并在本索引登记一行；
2. 顶部写状态行（`当前` / `历史` / `已暂缓`）和一句话结论；
3. 结论要能被验证——给命令、给数字、给日志，而不是"应该可以"；
4. 旧文档被取代时**不要删**，把状态改成 `历史` 并写清被谁取代；整条设计线停掉时移到 `archive/`，原文保留。

## 目录结构
```
research/
├── README.md            ← 新的总览：分层、目录、怎么跑
├── docs/                ← 所有文档
│   ├── README.md          索引（先读哪个、哪份是历史）
│   ├── design/            为什么这么做：SPATIAL_UI（§1–§10）、AGENT_3D_PATTERN、移动端方案、技术栈
│   ├── status/            现在到哪：HANDOFF（交接页）、PLAN（进度表）
│   ├── reports/           一次性结论：VERIFICATION_REPORT、occlusion_report、PUBLISH
│   └── archive/           历史：GEOMETRY_HISTORY（旧几何）、WATCHWALL（S1 监看墙，已暂缓）
├── rig_harness_in_3d/   应用层（= plugins/rig-tui 的位置）
├── rig_niri/        视图层（= rig-tui 这个 crate 的位置）
├── feathers_in_3d/      渲染层
├── bevy_luma/           上游 SDF 材料（vendored）
├── experiments/         一次性探针：spike、occlusion_probe、rig-tui-inspect
├── reference/           上游源码只读：rig-harness-src、niri（仍 gitignore）
├── render/              截图证据 + 新增 README 说明哪张还有效
└── tools/               脚本
```
