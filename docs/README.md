# 文档索引

> 目录约定：**`design/` = 设计与决策（看"为什么"）、`status/` = 当前状态与计划（看"现在到哪"）、
> `reports/` = 一次性排查/验证报告（看"当时查到了什么"）。**
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
| [`PLAN.md`](status/PLAN.md) | 实施顺序、每步做到哪、下一步五条 | 当前 |
| [`WATCHWALL.md`](status/WATCHWALL.md) | S1「监看多个 agent」的可读性测算与几何（三层表征） | **已暂缓**（用户指示不做视觉研究，留作决策记录） |

## design/ —— 为什么这么做

| 文档 | 一句话 |
|---|---|
| [`SPATIAL_UI.md`](design/SPATIAL_UI.md) | 设计纲领。§1 是"3D 赢面"的判断表，§11–§21 是历代几何（土楼/斗兽场/两级/niri），**均已退为历史** |
| [`AGENT_3D_PATTERN.md`](design/AGENT_3D_PATTERN.md) | 分层范式：内核 / 视图 / 渲染 / 应用各管什么，扩展点在哪 |
| [`RIG_FEATHERS_MOBILE_PLAN.md`](design/RIG_FEATHERS_MOBILE_PLAN.md) | 移动端方案（陀螺仪相机，无 VR） |
| [`VR_ASSISTANT_STACK.md`](design/VR_ASSISTANT_STACK.md) | 技术栈研究：feathers + rig + 路线 B 的可行性 |

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
4. 旧文档被取代时**不要删**，把状态改成 `历史` 并写清被谁取代。
