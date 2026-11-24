# rig_harness_in_3d

**通用 app 的 fork 模板**：按 rig 官方分层把「内核 / 视图 / 渲染」三层装起来，业务只写工具与 look。

对照 rig 自己的分层（见仓库 README）：

| rig 官方 | 这里 | 说明 |
|---|---|---|
| `rig-ecs`（kernel，无界面） | 直接依赖，不改 | agent / turn / call / Gate / Ask |
| `rig-harness`（粘合层：会话、命令、设置、工作区、插件） | **本 crate** | 装配 + 扩展点，面向 3D app 而不是 coding CLI |
| `rig-tui`（终端视图） | `rig-feathers` | 另一个前端实现（feathers + 3D） |
| —（终端设备） | `feathers_in_3d` | route B 渲染：SDF 几何 / 字形 / 排序 / 裁剪 / 拾取 |

```
rig_harness_in_3d（本 crate：装配 + 扩展点）
   ├── rig-feathers（视图：RenderBatch+look、多面板、状态、输入、picker、Front）
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
BEVY_ASSET_ROOT=$PWD RF_SHOT_DIR=../render/my_app ./target/debug/my_app
```

- `BEVY_ASSET_ROOT` 必须是本 crate（dev 期 Bevy 不去 `target/debug/assets` 找）；
- `assets/` 现在是指向 `../rig-feathers/assets` 的符号链接（字体等共享资源）；fork 出去后换成自己的。

## 代码里的扩展点

| 位置 | 作用 |
|---|---|
| `app(title)` | 装**真 harness**：`windowed(DefaultPlugins)` + `RigHarnessPlugins` + `feathers_in_3d` + `rig_feathers`；会话由 `Home::new_session()` 建，路径来自 `RIG_HOME` |
| `KernelPlugin`（demo） | 注册工具（`add_tool`）、闸门（`Stage::Gate`）、spawn agent 与示例调用 |
| `HarnessExt`（trait） | harness 服务的读取口：`session_dir()` 读真实 `SessionPaths`，`commands()` 扫上游注册的 `SlashCommand` 实体 |
| `HarnessExtPlugin` | 把 `/hello` 注册成样板命令，并把 `Notice` 桥进转写面板（命令 → 通知 → 行，真链路） |
| `prelude` | 业务只用这里的类型（含 rig-tui 的 look 契约，不用直接依赖 rig-tui） |

## 示例业务（可整段删掉）

- `search_products`（只读）、`create_order`（写，走闸门）；
- 闸门：`Stage::Gate` 里 spawn `Ask` + `HoldOf`，前端把选项画成可点行，确认/取消写 `Decision`；
- 自动巡游：8 张截图（`01_overview`…`07_gate_confirmed`），作为回归基线。

## 已知限制

- 模型未接：调用由 `ToolRequest` 注入，不是模型发起（没有 API key）；接缝是 `rig_feathers::batch::ReplyStream`；
- 设置（`Settings`）与 `/resume` 会话恢复还没接线；`Notice` 目前只进转写，还没进状态行；
- 文字是位图图集，~6× 放大发糊；裁剪只到面板边界；
- 相机只有键盘路径验证过（无实体手柄）。
