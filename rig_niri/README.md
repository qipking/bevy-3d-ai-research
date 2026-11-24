# rig_niri

**rig-tui 的另一个完整前端**：同样的内核（rig-ecs / rig-core）、同样的 look 管线（`RenderBatch` + `RenderSystems::Render`），
但把"终端格子"换成 **feathers UI + 世界空间 3D 面板**。同时，它本身就是**通用 app 的 fork 模板**。

```
              ┌─────────────── rig（内核）───────────────┐
              │ agent / turn / 工具 effect / Gate / Ask  │
              └───────────────┬──────────────────────────┘
                              │  同一个 bevy world（rig-ecs 是 bevy 0.20.1 插件）
        ┌─────────────────────▼──────────────────────┐
        │ rig_niri（本 crate，lib）              │
        │  batch   : 效应 -> RenderBatch -> look     │
        │  transcript : LookLine -> feathers 行      │
        │  frame   : Placement 帧布局（rig 的算法）  │
        │  pane    : 窗格窗口化 + 滚动               │
        │  panels  : PanelId + 增量 diff             │
        │  status / editor / hints / theme / front   │
        └─────────────────────┬──────────────────────┘
                              │
                       feathers_in_3d（3D 渲染）
```

## 定位：它是"另一个前端"，不是业务 app

| | rig-tui | rig_niri |
|---|---|---|
| 前端身份 | `Front::new("tui")` | `Front::new("feathers")` |
| 设备 | 终端（crossterm/ratatui） | 窗口 + 3D 世界（feathers/bevy） |
| 布局 | 终端格子（`TuiPanel`/`Placement`） | 同一套 `Placement`（`frame.rs`），格子换成像素 + 世界锚点 |
| 转写 | `transcript`/`pane` 滚动缓冲区 | `pane.rs` 窗口化（同一段行，按窗高截取） |
| 渲染 | 画到 `Buffer` | `feathers_in_3d`（SDF + 字形四边形 + 排序键） |
| 输入 | 终端按键事件 | bevy 输入 + 射线拾取 + 编辑器 |
| look 管线 | **同一份**（`RenderBatch` + `RenderSystems::Render`） | **同一份** |
| 闸门/Ask | 同一套 `Ask`/`Decision` | 同一套 |

> 两个前端可以**并存**：`Front` 机制就是为此设计的；本仓库只跑 feathers 前端。

## 和 rig-tui 的功能对照（`UPSTREAM.md` 有完整表）

| rig-tui | rig_niri | 状态 |
|---|---|---|
| `panel.rs`（`TuiPanel`/`Placement`/`lay_out`） | `frame.rs`（同名同义的 `Placement`/`lay_out`，格子版算法） | ✅ |
| `render.rs` 的帧布局（面板 → 面板区 → 剩余区） | `frame.rs` 的 `lay_out_frame` + `place_panels` | ✅ |
| `pane.rs`（窗格窗口化 + 滚动） | `pane.rs`（`TranscriptRows` → `PaneScroll` → `PanelRows`） | ✅ 主窗格 |
| `screen.rs`（默认屏幕：输入/状态行/转写） | `frame.rs::spawn_default_screen` | ✅ |
| renderers（look 调度） | `batch.rs`（自建驱动） | ✅（rig-tui 是 `pub(crate)`） |
| status / hints | `status.rs` 直接调 `rig_tui::status::line`（`keep`/`min_width` 裁剪是 rig 本来的规则）+ `hints.rs` | ✅ |
| title（终端标题 / OSC 133） | `title.rs`：窗口标题 `基础 · {working,needs you,error,idle}`（OSC 133 只对终端有意义，不做） | ✅ |
| input / editor / keys / history | `editor.rs` | ✅ 基础键位 |
| theme | `theme.rs` | ✅ |
| `pick.rs`（列表 + preview + 折叠） | `picker.rs`：容器 `Over` + 列表 `Within` + preview `Within`、打字过滤、`free_text`/`secret`、多选打勾、树折叠、`PickPreview::Text`、点击 | ⚠️ 缺 `more`、`PickPreview::Conversation` |
| `complete.rs`（补全弹窗） | — | ⬜ |
| clipboard / selection | `bevy_clipboard` + 行级选择 | ✅ 行级 |
| markdown/highlight/diff | 业务 look（demo 里只做了两种卡片） | 由业务决定 |

## 一帧的顺序

`Update` 里按 `FeathersSystems` 分段（对应 rig-tui 的 `TuiSystems`）：

```text
Prepare  按键、面板按内容定尺寸（`FitRows`）
Layout   `lay_out_frame` 算每块面板的格子区域 → `place_panels` 写 Node 尺寸 + 3D 锚点
Draw     look / 状态行 / 输入行产出行（look 的换行宽度 = 面板文字列数）
Fit      `window_transcript`：把整段行截成这一帧的可见行
Render   `sync_panels`：行同步成 bevy_ui 节点（增量 diff）
```

> 和 rig-tui 的唯一顺序差别：它的 `Fit` 在 `Draw` 之前（面板自己拿画布先裁再画）；
> 我们这里行是**值**（`PanelRows`），所以窗口化必须排在 `Draw` 之后。

## 帧布局：`Placement` 就是 3D 面板的"放哪"

一块面板 = 一个 `FramePanel`（`Placement` + 顺序 + 装饰）。默认屏幕是：

| 面板 | Placement | 说明 |
|---|---|---|
| 输入 | `Bottom(Cells(4))` | 最下面，提示行 + 键位提示行 |
| 状态行 | `Bottom(Cells(2))` | 输入上面，细条 |
| 转写 | `Rest` | 剩下的全部 |

其它可用的放置：`Top` / `Left` / `Right`（侧栏、对比视图）、`BelowInput`（建议行）、
`Over`（浮层，picker/补全）、`Within`（picker 的 preview、左右分栏）。

picker 就是 `Over` 的现成例子（`picker.rs`）：有 `Picker` 就开一块浮层，
列表的行由 `picker_rows` 写进 `PanelRows[Overlay]`，回车走 `Pick::person`。

```rust
// 业务插件加一块自己的面板：位置交给帧布局，行自己写
let panel = frame::spawn_panel(&mut commands, PanelId::Overlay,
    FramePanel::new(Placement::Over {
        width: Extent::Percent(52),
        height: Extent::Cells(9),
    }),
    frame::Look::new(Color::srgb(0.10, 0.12, 0.18), Color::srgb(0.95, 0.82, 0.35), 10.0));
roots.0.insert(PanelId::Overlay, panel);
rows.set(PanelId::Overlay, my_rows);
```

层（`Placement::stacking` → `WorldLayer`）同时决定 3D 深度和绘制顺序：
浮层永远盖在底下的面板上，跟相机角度、底下有多少节点都无关
（`feathers_in_3d::LAYER_BIAS`）。

## 作为 fork 模板：你只需要改三处

1. **工具**：实现 `rig_core::Tool`，`app.add_tool(...)`；
2. **look**：每个工具一个 `fn(&ToolRender) -> Vec<LookLine>`，注册到 `RenderSystems::Render`；
3. **面板内容**：往 `PanelRows` 里写行（`status.rs`/`editor.rs` 是样板）；
   要一块新面板就 spawn 一个 `FramePanel`（见上）。

其余（3D 渲染、排序、裁剪、拾取、增量刷新、闸门问答、相机）都在 lib 里，不用动。
示例业务在 `src/main.rs` + `src/kernel.rs`（导购/支付闸门）——**可以整段删掉**换成你的。

## 跑

```sh
../tools/fetch_cjk_font.sh          # 中文字体（含子集化，3.2 MB）
export CARGO_HOME=/var/home/lichi/桌面/research/.cargo-home

cd ../rig_harness_in_3d             # 装配层（本 crate 是 lib，demo binary 在那里）
cargo build
BEVY_ASSET_ROOT=$PWD RF_SHOT_DIR=../render/rf_shots ./target/debug/rig_harness_in_3d
```

- 必须给 `BEVY_ASSET_ROOT`；
- 窗口标题会跟着状态变成 `… · working / needs you / error / idle`（`title.rs`）；
- `RF_RIG=1` 用手柄/键盘驱动相机（默认关闭，巡游用固定机位）；
- `RF_FRAME=96x28` 改帧的格子数（默认 96×28，16 行时转写会窗口化滚动）；
- `RF_OVERLAY=1` 额外生成一块 `Placement::Over` 浮层面板（验证层级）；
- 运行约 30 秒自动截图后退出：`01_overview`…`08_overlay`。

## 上游跟进

见 [`UPSTREAM.md`](UPSTREAM.md)：锁版本、每次升级要重跑的检查点、哪些 API 是边界。

## 已知限制

- 文字是位图图集，~6× 放大发糊（要 SDF 文本）；
- 裁剪到面板边界，没有嵌套裁剪（`Within` 的 preview 也只能裁到自己的面板）；
- 帧的格子数是**固定**的（3D 里它是一块屏，物理尺寸不随窗口变）；
- 模型未接（接缝 `ReplyStream`）；
- bevy_text 对 CJK 会打印 `ICU4X data error: No segmentation model for complex script`，
  换行分段退化；当前行都是 `NoWrap`，影响有限；
- 无实体手柄/真机，相机只验证了编译与键盘路径。
