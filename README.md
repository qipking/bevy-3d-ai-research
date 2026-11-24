# research

用 **Bevy + rig** 做的 3D 空间 agent 界面原型。目标：把 `rig-harness` 的界面在 3D 空间里重新实现
（不是把终端字符网格变立体，而是换成"卡片/面板在空间里摆"）。

## 目录

```
rig_harness_in_3d/  应用层（harness 装配 + 业务工具 + 演示/巡游）——fork 模板就是它
rig_niri/           视图层：照 rig-tui 的模型重写的前端（面板/状态/输入/picker/窗口逻辑）
feathers_in_3d/     渲染层：把 bevy_ui 子树镜像成 3D 网格 + 字形 + 排序 + 裁剪 + 拾取
bevy_luma/          上游的 SDF 3D UI 材料（vendored，改动要克制）

docs/               文档：design(为什么) / status(现在到哪) / reports(一次性结论) → 先看 docs/README.md
experiments/        一次性探针：spike、occlusion_probe、rig-tui-inspect（不保证能编译）
reference/          上游源码，只读参考：rig-harness-src、niri（GPL-3.0，已 gitignore）
render/             截图证据（每个子目录对应一次验证）
tools/              脚本
```

## 分层

```
rig-ecs      内核：agent / turn / call / Gate / Ask
rig-harness  harness：会话 / 命令 / 设置 / 插件 / services
──────────── 以上是上游，不改 ────────────
rig_harness_in_3d  ← 应用层（等于 plugins/rig-tui 的位置）
rig-feathers       ← 视图层（等于 rig-tui 这个 crate 的位置）
feathers_in_3d     ← 渲染层（上游没有对应物）
```

## 跑

```sh
cd rig_harness_in_3d
export CARGO_HOME=/var/home/lichi/桌面/research/.cargo-home
export BEVY_ASSET_ROOT=$PWD

RIG_HOME=/var/home/lichi/桌面/research/.rig-home cargo run                          # 演示巡游
RIG_HOME=/var/home/lichi/桌面/research/.rig-home cargo run --example harness_boot   # harness 本体自检
```

`RIG_HOME` 必须有：harness 会在里面建会话目录（沙箱下默认的 `~/.rig` 可能只读）。

## 当前状态

见 [`docs/status/HANDOFF.md`](docs/status/HANDOFF.md)（一页速览）和
[`docs/status/PLAN.md`](docs/status/PLAN.md)（进度表）。一句话：
**渲染层与视图层可用；harness 本体已装上；布局已改成 niri 分裂树（`rig_niri`）；
当前在做「工作区 = niri 条带 × 绝对空间坐标」（见 `docs/status/WORKSPACE_MODEL.md`）。**
