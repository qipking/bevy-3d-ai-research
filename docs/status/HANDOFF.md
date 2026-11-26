# 交接文档：rig_niri / feathers_in_3d

> **状态**：当前（2026-11-26 复跑）。这页是给**新会话的第一个读者**的：读完它就能接手，
> 细节再去 [`ARCHITECTURE.md`](ARCHITECTURE.md)。仓库根 `/var/home/lichi/桌面/bevy_3d_ai_research`
> （jj 仓库；仓库里也有 `.git`，但提交用 jj，不要 `git commit`）。

---

## 0. 一句话与三层边界

**用 `bevy_feathers` 做视觉、niri 做布局、rig 做内核，做一个带 3D 空间界面的 agent 助手**
（先桌面，后移动端）。不是"把终端界面画到 3D 里"。

| 层 | 听谁的 | 判据 |
|---|---|---|
| **内容/行为** | `rig-tui` | 行文本/颜色/折行、键位语义、turn/tool/Ask/Picker/状态行、`RenderBatch` + look |
| **视觉** | `bevy_feathers` | token、按钮/输入框/滚动条外观；**不追终端像素** |
| **布局** | niri | 列宽 = 视口比例、横向滚动条带、工作区竖直堆叠、**没有浮层** |
| 3D 呈现 | `feathers_in_3d` | 把 bevy_ui 子树镜像成世界空间面板；这是工程层，不是第四种设计权威 |

口径冲突时按上表从上往下判：内容必须像 rig-tui；视觉/布局按 feathers+niri，像素级一致不是验收项。

---

## 1. 五层架构：先认 crate，再读 ARCHITECTURE

五层（外加 feathers 工具包）；视图层模块数有据：`ls rig_niri/src/*.rs | wc -l` = 26。

| 层 | crate | 干什么 | 关键入口 |
|---|---|---|---|
| 内核 | `rig-ecs` / `rig-core` 0.47 | agent、turn、effect、工具、`Deliver`、`Ask`/`Decision` | `AgentPlugin` |
| 胶水 | `rig-harness` 0.47 | 会话/日志、主 agent、picker、通知、进度、插件 | `RigHarnessPlugins`、`ShownAgent`、`Picker` |
| 应用 | `rig_harness_in_3d` | 装配 + 演示工具 + `/show`、`/theme`、多工作区 | `app(title)`、`KernelPlugin`、`WorkspacesDemoPlugin` |
| 视图 | `rig_niri`（**26 个模块**） | 行模型、转写引擎、niri 布局、工作区、输入、选择、主题 | `FeathersViewPlugin` |
| 渲染桥 | `feathers_in_3d` | bevy_ui → 世界空间面板/文字/图片/拾取（**不认识 rig**） | `FeathersIn3dPlugin`、`WorldPanel` |
| UI 工具包 | `bevy_feathers` 0.20 | `UiTheme`、token、控件样式、`Outline` | `FeathersPlugins` |

依赖方向：`rig_niri → rig-tui/rig-harness/feathers_in_3d`；
`rig_harness_in_3d → rig_niri + rig-harness + feathers_in_3d`；`feathers_in_3d` 只依赖 bevy。
**读代码顺序**：本页 → `ARCHITECTURE.md` §1（依赖）→ §3（模块地图）→ §4（四条数据链）→
§5（帧内顺序）→ §6（状态归属）。改代码前先认这五个锚，别从 `main.rs` 顺着读。

---

## 2. 怎么构建、跑、看画面

日常用 `run.sh`（它已经设好 `CARGO_HOME`、`BEVY_ASSET_ROOT`、`RIG_HOME`，并在 workspace 根构建）：

```sh
cd /var/home/lichi/桌面/bevy_3d_ai_research
./tools/fetch_cjk_font.sh     # 只在字体缺失时跑一次（Bevy 只认 .ttf/.otf）
./run.sh                      # 单会话 + 自动巡游
./run.sh ws3                  # 3 条工作区（每条一个真 agent）
./run.sh niri                 # 旧的 niri 平面演示（RF_NIRI_LAYOUT=1），不是当前主链路
./run.sh shots                # 巡游并把截图写到 render/rf_niri_app/，约 30 秒后自动退出
```

手工跑时要给三样东西，缺一样会"窗口打开了但什么都没看见"：
`CARGO_HOME=$PWD/.cargo-home`、可写的 `RIG_HOME`、能找到 `assets/fonts` 的资源根
（应用自己会找 `BEVY_ASSET_ROOT`/当前目录/可执行文件附近，见 `rig_harness_in_3d/src/lib.rs::assets_dir`）。

环境变量（只列真正有读取方的）：

| 变量 | 作用 | 证据 |
|---|---|---|
| `RF_SHOT_DIR` | 截图输出目录；默认 `shots` | `rig_harness_in_3d/src/main.rs:63` |
| `RF_WORKSPACES` | 工作区条数；默认 1 | `workspaces_demo.rs:142` |
| `RF_SHOW_ALL` | 启动后取消聚焦，一次看全所有条带 | `workspaces_demo.rs:609` |
| `RF_FRAME` | 钉死帧尺寸（如 `96x28`），窗口 resize 不再改 | `frame.rs:140`/`:217` |
| `RF_THEME` | 启动主题 `dark` / `light` | `theme.rs:210` |
| `RF_RIG` | 相机交给手柄/键盘操作架 | `camera_rig.rs:33` |
| `RF_MARKERS` | 加几何参照方块 | `main.rs:108` |
| `RF_NIRI_LAYOUT` | 打开旧的 niri 平面演示 | `niri.rs:459` |

`RF_OVERLAY` 只在旧注释里出现过（`grep -rn RF_OVERLAY rig_niri/src rig_harness_in_3d/src` 只剩 `workspaces.rs:1058` 一句注释），**当前没有读取方**，别照旧文档用。
截图：巡游实际拍 **17 张**（`main.rs:495` 的 `run_tour`，帧 54 退出）；`render/rf_niri_app/`
里另有早期的 `08_overlay.png`，不属于当前巡游。先看 `01_overview.png`、`04_text_zoom.png`、
`06_gate_ask.png`；浅色主题的同一组在 `render/light/`。

测试（全部无窗口，离线）：

```sh
CARGO_HOME=$PWD/.cargo-home cargo test --workspace --offline
```

---

## 3. 当前进度（2026-11-26 实测）

| 套件 | 测试数 | 说明 |
|---|---|---|
| `rig_niri` lib | **606** | 视图层全部 |
| `rig_harness_in_3d` lib | **36** | 装配 + 业务演示 |
| `rig_harness_in_3d` bin | **5** | 巡游/机位 |
| `feathers_in_3d` | **15** | 镜像/文字/图片/裁剪 |
| 合计 | **662** | 另有 3 个 doctest 通过、2 个 ignored |

已完成的大块（都有测试；详细落点见 ARCHITECTURE §3–§7）：

| 块 | 落点 |
|---|---|
| 行模型 / 转写引擎（块缓存、折行、流式让位） | `rig_niri/src/transcript.rs`、`batch.rs` |
| niri 列布局：`Size::Fraction` + `WorkspaceScroll` + 视口裁剪 | `layout.rs:99`、`workspaces.rs:110`、`frame.rs:586`/`:691` |
| 多来源：`TranscriptSource::{Shown,Agent,Value}` + `/show` | `pane.rs:613`、`batch.rs:2044`、`show.rs` |
| 插件面板画布：`PanelId::Plugin`、`drawn_row(s)`、`StatusLine` | `panels.rs:100`、`panel.rs:271`/`:722`、`status.rs:114` |
| feathers 视觉：token 化面板/按钮/滚动条/输入框 | `frame.rs` 的 `sync_panel_chrome`、`theme.rs` 的 token |
| 主题：`/theme dark\|light` + `RF_THEME` | `theme_cmd.rs`、`theme.rs:282` |
| `ImageNode` 镜像（F8） | `feathers_in_3d/src/world3d.rs:317`/`:589` |
| 应用层：多工作区、演示工具 + Ask 闸门 | `workspaces_demo.rs`、`kernel.rs:191` |

最近提交（`git log --oneline`；jj 上是同一串）：

`b22d674` 截图 · `2840745` 架构文档 · `db0b214` 主题 · `05d2ca1` F6 切片 2 ·
`bb71cbd` F10 复制 · `8a13230` F6 切片 1 · `e66e936` S3/S4 + F8 · `6ff1f50` 审查修复

---

## 4. 剩什么（按价值排的前 5 件，编号见 REMAINING）

> 编号与状态以 [`REMAINING.md`](REMAINING.md) 为准（2026-11-26 已按当前实现重排）；
> 测试数字同时见 [`ARCHITECTURE.md`](ARCHITECTURE.md) §8。

| 排序 | 未完成项 | 落点 / 证据 | 量 |
|---|---|---|---|
| 1 | **S5–S6**：压缩条目（`Replaced`）与重启前调用事实（`CallFact`/`PastEffects`） | `REMAINING.md` §6；`grep -rnE 'Replaced|PastEffects' rig_niri/src` 只剩注释/TODO | L |
| 2 | **对话图片接线**：F8 只做了 `ImageNode` 镜像；消息/转写里的图片还没有挂节点 | `REMAINING.md` §4/§11 F8；`grep -rn ImageNode rig_harness_in_3d/src` 为 0 | M（估） |
| 3 | **F6 切片 3**：插件命令绑定（注册自己的命令 + 默认键） | `REMAINING.md` §10/§11 F6；`InputAction`/`run_command` 已有，缺注册 API | M |
| 4 | **主题收尾**：浅色对比度打磨 + `[view] theme` 持久化 | `REMAINING.md` §11 F7；`[view]` 只有 `hide_status`（`settings.rs:33`），主题读回未做 | S |
| 5 | **布局/平台收尾**：niri consume/expel 与鼠标拖列（键盘移列已有；`grep -rnE 'fn (consume|expel)' rig_niri/src` 无输出）、L5（scroll 插值 / center-focused）、Android feature 条件化 | [`NIRI_SCROLL.md`](../design/NIRI_SCROLL.md) §5；[`ARCHITECTURE.md`](ARCHITECTURE.md) §10；`grep -rn interpolat rig_niri/src` 为 0 | S–M + 真机 |

更完整的条目、依赖和工作量都在 [`REMAINING.md`](REMAINING.md)；模块级审计看
[`PARITY.md`](PARITY.md)。**不要重排 REMAINING 的编号**，别的文档按 `#1`–`#10` 引用它。

---

## 5. 约束与红线

1. **不追终端像素**：内容/行为对 rig-tui；视觉/布局按 feathers+niri。不要为了"像终端"回退 token。
2. **没有弹层/浮窗**：闸门是确认 tile，picker 是条带列，补全是右列 tile；不要加 modal。
3. **工作区全可见**：聚焦只决定内容写哪条，不隐藏、不移动相机、不做动画（`focus_changes_nothing_visible`，
   `workspaces.rs:2800`；键位 F1..F8 聚焦、F9 取消、F10 整理，见 `workspaces_demo.rs:58`）。
   同一处注释里的"其它隐藏"是旧话，**不要照它改实现**。
4. **条带内所有列一起滚**（转写/追加列/picker/输入/确认/补全）；只有条带外的 Hints/Status 固定
   （`workspaces.rs:108`，测试 `:3165`）。输入框跟着滚是 niri 的直接结果，不是 bug。
5. **`Pane2` 不再是主窗格的镜像**：每块窗格有自己的 `TranscriptPane.source`/`scroll`；
   `/show` 只改活动窗格，**不移动 `ShownAgent`**（`pane.rs:613`、`show.rs:14`）。
6. **方向不能反**：`feathers_in_3d` 不认识 rig，是 `rig_niri` 依赖它；不要为了业务把 rig 类型塞进渲染桥。
7. **`vendor/bevy_luma` 的 alpha_mode=Premultiplied 是有意改动，别回滚**。
8. **永不裸跑 `jj restore`**（无路径 = 丢弃整个 change）。回退文件写路径；出事用
   `jj op log` / `jj op restore <op-id>` / `jj undo`。`tools/jjx` 只在 `~/.config` 只读的沙箱里用。
9. **不要换控件内核**：输入框只借 feathers 外观（不换 `TextInput`）；不做 `RequestRedraw`/脏区门控
   （bevy_ui 每帧重排 + 每帧镜像，没有省一帧的收益）。
10. **Android 先做平台条件化**：`rig_niri/Cargo.toml:43` 无条件开 `system_clipboard`/`wayland`，
    `arboard` 3.7 把 android 排除在所有后端外；先条件化 feature、留内存替身，再 NDK 实测。

---

## 6. 常用命令 / 目录速查

```sh
./run.sh [ws3|ws5|niri|shots]                    # 跑 app（见 §2）
CARGO_HOME=$PWD/.cargo-home cargo test --workspace --offline
CARGO_HOME=$PWD/.cargo-home cargo test -p rig_niri --offline
jj st / jj log / jj new / jj describe            # 版本管理
git log --oneline -12                            # 只看提交历史（不要 git commit）
grep -rn "关键字" rig_niri/src                    # 找实现
```

| 路径 | 是什么 |
|---|---|
| `rig_niri/src/` | 视图层 26 个模块（`batch`/`transcript`/`layout`/`frame`/`workspaces`/`panel`/`pane`/`editor`/`theme`…） |
| `rig_harness_in_3d/src/` | 应用装配 + 演示业务（`lib.rs::app`、`kernel.rs`、`workspaces_demo.rs`、`show.rs`、`theme_cmd.rs`） |
| `feathers_in_3d/src/` | `world3d.rs` 镜像、`text3d.rs` 字形、`pick.rs` 拾取、`sort.rs` 排序 |
| `docs/status/` | `HANDOFF`（本页）、`ARCHITECTURE`、`REMAINING`、`PARITY`、`WORKSPACE_MODEL` |
| `docs/design/` | `SPATIAL_UI`、`FEATHERS_UI`、`NIRI_SCROLL`、`PANEL_CANVAS`、`SHOW_AGENT` |
| `render/rf_niri_app/`、`render/light/` | 当前截图证据（巡游 17 张 / 浅色同组） |
| `reference/`、`vendor/` | 上游只读源码 / vendored `bevy_luma` |
| `tools/` | `fetch_cjk_font.sh`、`jjx`、字体子集脚本 |

**新会话第一步**：读 `ARCHITECTURE.md` §1/§3–§6 → 跑一遍全量测试 → `./run.sh shots` 并看
`render/rf_niri_app/01_overview.png` → 从 §4 挑一项开工。不要先改代码再补测试。
