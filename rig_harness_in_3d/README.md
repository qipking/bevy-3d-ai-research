# rig_harness_in_3d

**通用 app 的 fork 模板**：按 rig 官方分层把「内核 / 视图 / 渲染」三层装起来，业务只写工具与 look。

对照 rig 自己的分层（见仓库 README）：

| rig 官方 | 这里 | 说明 |
|---|---|---|
| `rig-ecs`（kernel，无界面） | 直接依赖，不改 | agent / turn / call / Gate / Ask |
| `rig-harness`（粘合层：会话、命令、设置、工作区、插件） | **本 crate** | 装配 + 扩展点，面向 3D app 而不是 coding CLI |
| `rig-tui`（终端视图） | `rig_niri` | 另一个前端实现（feathers + 3D） |
| —（终端设备） | `feathers_in_3d` | route B 渲染：SDF 几何 / 字形 / 排序 / 裁剪 / 拾取 |

```
rig_harness_in_3d（本 crate：装配 + 扩展点）
   ├── rig_niri（视图：RenderBatch+look、多面板、状态、输入、picker、Front）
   │        └── feathers_in_3d（渲染）
   └── rig-ecs（内核）+ rig-harness（Ask/Pick 服务）
```

## 快速开始（fork）

```bash
cp -r rig_harness_in_3d my_app && cd my_app
# 1) 改 Cargo.toml 的 package name
# 2) 改 src/kernel.rs：你的工具（实现 rig_core::Tool）+ 闸门策略
# 3) 改 look：每个工具一个 fn(&ToolRender) -> Vec<LookLine>，注册到 RenderSystems::Render
# 4) 改 src/main.rs 的装配（可整段保留，只换 KernelPlugin）
```

跑：

```sh
../tools/fetch_cjk_font.sh        # 字体（3.2 MB 子集）
CARGO_HOME=/var/home/lichi/桌面/research/.cargo-home cargo build
BEVY_ASSET_ROOT=$PWD RIG_HOME=/path/to/rig-home RF_SHOT_DIR=../render/my_app ./target/debug/my_app
```

- `BEVY_ASSET_ROOT` 必须是本 crate（dev 期 Bevy 不去 `target/debug/assets` 找）；
- `assets/` 现在是指向 `../rig_niri/assets` 的符号链接（字体等共享资源）；fork 出去后换成自己的。

## 代码里的扩展点

| 位置 | 作用 |
|---|---|
| `app(title)` | 装**真 harness**：`windowed(DefaultPlugins)` + `RigHarnessPlugins` + `feathers_in_3d` + `rig_niri`；会话由 `Home::new_session()` 建，路径来自 `RIG_HOME`；`RIG_HOME/settings.toml` 在启动时读 |
| `KernelPlugin`（demo） | 注册工具（`add_tool`）、闸门（`Stage::Gate`）、spawn agent 与示例调用 |
| `HarnessExt`（trait） | harness 服务的读取口：`session_dir()` 读真实 `SessionPaths`，`settings_file()` 读真实 `SettingsPath`，`commands()` 扫上游注册的 `SlashCommand` 实体 |
| `HarnessExtPlugin` | `/hello`、`/setting`、输入框 `/命令` 桥、`Notice` → 转写引擎、真实状态行、`[view]` 设置 |
| `prelude` | 业务只用这里的类型（含 rig-tui 的 look 契约，不用直接依赖 rig-tui） |

## 设置（`RIG_HOME/settings.toml`）

路径规则用上游的 `Home::settings()`：只读 `RIG_HOME`，项目目录没有 rig 设置。
本模板读 `[view]` 一段：

```toml
[view]
hide_status = false
```

- `hide_status = true` 会让状态面板整行消失；
- 在输入框里 `/setting hide_status true` 改内存并写回同一个文件（文件里其他表保留）；
- 文件不存在：用默认值；文件读不了 / 不是 TOML / `[view]` 键错：`warn!` 后用默认值继续，
  不 panic，也不会让上游 launcher 的 `Settings::problems` 拒绝启动。

## 通知只有一条路

`Notice` → `NoticeHistory` → 转写引擎的 `TextId::Notice` → `TranscriptRows` → 面板行。
应用层不再自己拼行（上一轮的 `NoticeLog` 已删），所以真 agent 的通知不会画两遍；
顺序、折行、和对话一起回滚都归 `rig_niri::transcript::Transcript` 管。

## 状态行

条目每帧从真实 ECS 资源刷新：`session`（`SessionPaths`）、`agents`、`turn`
（`ActiveTurn`）、`conversation`（消息条数）、`tasks`（`Running`）、`tokens`
（只有 `LastUsage` 真报过才画）。没有 `no model / 312 tokens / ctx 12%` 之类的
演示数字；其他插件放进 `AppStatus` 的条目不被覆盖。

## 示例业务（可整段删掉）

- `search_products`（只读）、`create_order`（写，走闸门）；
- 闸门：`Stage::Gate` 里 spawn `Ask` + `HoldOf`，前端把选项画成可点行，确认/取消写 `Decision`；
- 自动巡游：8 张截图（`01_overview`…`07_gate_confirmed`），作为回归基线。

## 已知限制

- 模型未接：调用由 `ToolRequest` 注入，不是模型发起（没有 API key）；接缝是 `rig_niri::batch::ReplyStream`；
- `rig_niri` 的 `collect_rows` 目前会 `retain_text` 掉 `TextId::Notice`，所以本模板在
  `Fit` 里把通知历史重新交给引擎（见 `src/notice.rs`）；上游保留 Notice 后这段回填可删；
- `/resume` 会话恢复还没接线；`Notice` 目前只进转写，还没进状态行；
- 文字是位图图集，~6× 放大发糊；裁剪只到面板边界；
- 相机只有键盘路径验证过（无实体手柄）。
