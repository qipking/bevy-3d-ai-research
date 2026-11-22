# 路线 B 验证报告（本地机续跑）

> 承接：`spike/RESETUP_DOC.md`（阿里云服务器交接文档，2026-11-22）
> 本机：Fedora Silverblue 45 / Ryzen 7 7840H (8C16T) / 58 GB / Rust 1.100.0
> 日期：2026-11-23
> 结论：**路线 B 的关键机制在本地全部跑通并被量化验证**；同时发现 4 个交接文档没覆盖的运行时新问题（其中 2 个已给出规避方案）。

---

## 0. 一页结论（更新交接文档第 0 节）

| 问题 | 交接文档状态 | 本地续跑结果 | 依据 |
|---|---|---|---|
| spike 能否编译？ | ⏳ 交接时未完成 | ✅ **通过**（全量 2m58s / 增量 34s，387 个 crate） | `spike/target/debug/spike` |
| U1 任意缩放是否发虚？ | ❌ 未看到画面 | ✅ **通过（定量）**：**647% 放大下边缘过渡仅 1 物理像素** | `render/shots_isolate/02_close.png` 扫描线实测 |
| U3 luma 3D shader 与 Bevy 0.20 是否兼容？ | 只跑到"加载无报错" | ✅ **通过**：`unode_3d.wesl` 在 0.20 下编译**并实际出图** | `render/shots/*.png` |
| U4 坐标映射对不对？ | ❌ 未实测 | ✅ **通过（定量）**：两个子节点的三维位置 **delta = (0,0,0)** | `.tmp/final1.log` 的 U4 行 |
| U5 3D 下 feathers 控件还有交互语义吗？ | ❌ 未实测，估算 300–500 行 | ✅ **通过**：`PointerOver → PointerClick → Activate` 全链路触发，**零自定义拾取代码** | `.tmp/final1.log` 的 `ACTIVATE` 行 |
| U2 Bevy 自带 UI 的 `fwidth` 是否会 artifact？ | ❌ 未验证 | ❌ 本次未做（已被路线 B 绕过，不影响结论） | — |
| U6 0.20.0 / 0.20.1 API 差异范围？ | ❌ | 🟡 部分：**混树可编译并运行**（`bevy 0.20.0` + 子 crate `0.20.1`） | `Cargo.lock` |
| 新增：`LumaPlugin` 能否与 feathers 共存？ | 未提及 | ❌ **不能**：调度环 panic；只取 `UnivisRenderPlugin` 即解决 | 见 §3.1 |
| 新增：3D 下父子绘制顺序？ | 未提及 | ⚠️ **透明队列会反序**；实心面板可走 Opaque 规避 | 见 §3.3 |

**一句话**：路线 B（保留 `bevy_ui` 布局，只换渲染与拾取）**已在本地跑通并量化验证**；拾取成本比原估算低得多（复用 `mesh_picking`，不需要自研 300–500 行 backend），但**新增了"深度/绘制顺序"和"布局树与渲染树分离"两项之前没算到的成本**。

---

## 1. 环境与源码获取（与交接文档的差异）

### 1.1 本机环境

| 项 | 值 | 影响 |
|---|---|---|
| OS / CPU | Fedora Silverblue 45 / Ryzen 7 7840H 8C16T | 编译比服务器快得多：全量 **2m58s** |
| 内存 | 58 GiB | 无压力 |
| GPU | 会话中 `/dev/dri` 不可见 → **llvmpipe（Mesa 26.2.7 软件 Vulkan）** | 画面正确性可用；**性能数据无参考价值** |
| Vulkan ICD | `radeon/lvp/...` 均在，但 `powervr` ICD 枚举失败（`-3`） | 仅日志噪音，不影响运行 |

> 交接文档 §7.1 里"Xvfb + lavapipe"的一整套在本机**不需要**：有真实 Wayland 会话，Bevy 直接开窗；截图用 Bevy 内置 `Screenshot` API 落盘。

### 1.2 ⚠️ 重要修正：crates.io 上的 `bevy_luma` 是 Bevy **0.19**，不是 0.20

交接文档 §2.1 写"`bevy_luma` 依赖 `bevy = "0.20.0"`"——这来自 **git 仓库的 Cargo.lock**，没错；但**crates.io 发布的 0.1.0 是 Bevy 0.19**：

```toml
# static.crates.io 上 bevy_luma-0.1.0/Cargo.toml
[dependencies.bevy]
version = "0.19.0"
```

它的 4 个子 crate（`luma_engine` / `luma_layout` / `luma_style` / `luma_interaction`）也都是 0.1.0 / Bevy 0.19。

**结论**：验证**必须用 git 仓库 main 分支**（`Cargo.toml` 里 `bevy = "0.20.0"`），不能用 crates.io 包。交接文档 §7.2 的 `cargo build --example world_3d` 如果照 crates.io 版本做会得到 0.19 的树。

### 1.3 网络受限下的源码拉取

本机 `github.com` / `codeload.github.com` **TCP 被重置**，但 `api.github.com` 与 `raw.githubusercontent.com` 可用，于是：

```bash
# 1) 列文件树
curl -s "https://api.github.com/repos/Abdellah-Haffout/bevy_luma/git/trees/main?recursive=1"
# 2) 196 个 blob 走 raw 并行下载（实测 3 个文件会因重置失败，需重试）
# 3) Cargo.toml / Cargo.lock 用 api.github.com/contents/<path>?ref=main 补下
```

结果：`research/bevy_luma/`（196 文件、13 MB、main 分支）与交接文档 §1 的 `/tmp/research/bevy_luma` 等价。

---

## 2. 编译（U3 的前置）

```bash
cd research/spike
export CARGO_HOME=$PWD/../.cargo-home   # 本机会话沙箱里 ~/.cargo 只读，CARGO_HOME 放到工作区
cargo build
#   Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m58s
```

- 交接文档 §3.2 的 **`custom_cursor` feature 对齐方案有效**：`bevy 0.20.0` + 子 crate 0.20.1 混树没有 E0004。
- 交接文档 §3.3「spike 编译仍在进行」→ **已闭环**：源码未改动即可编译通过（后续改动均为功能增强）。
- 增量编译 34s，改一行就能跑一轮验证，迭代成本很低。

---

## 3. 运行时新发现（交接文档没有的）

### 3.1 🔴 `LumaPlugin` 与 `bevy_ui` 不能共存 → 调度环 panic

直接按交接版写 `.add_plugins(bevy_luma::LumaPlugin)` 会在**第一帧之前** panic：

```
Error when initializing schedule PostUpdate: schedule has 14522 before/after cycle(s):
cycle 1: system `... (in set Propagate)` must run before itself
 ... (in set EditableTextSystems) ... (in set Content) ... (in set Layout) ...
```

成因：`LumaPlugin` = style + **engine(layout/render/text)** + **interaction** 三套系统，其 `UiSettlementSchedule`/`Layout` 排序约束和 `bevy_ui` 0.20.1 的 `Propagate / Content / EditableTextSystems` 互相成环。

**规避（已验证）**：路线 B **只需要 luma 的材质与 shader**，不注册它的布局/交互插件：

```rust
use bevy_luma::engine::layout::render::UnivisRenderPlugin;
...
.add_plugins(UnivisRenderPlugin)   // embedded_asset! + MaterialPlugin::<UNodeMaterial3d>
```

`UnivisRenderPlugin::build` 正是 `embedded_asset!(unode_3d.wesl)` + `MaterialPlugin::<UNodeMaterial3d>` + depth_sync 安装点，够用且不碰 `bevy_ui` 的调度。

> 这条对路线 B 的架构含义：**"复用 luma 的材质"和"复用 luma 的 UI 引擎"是两件事**，前者成本低，后者会与 bevy_ui 正面冲突。

### 3.2 🟠 2D UI 必须移出视口，否则盖在 3D 面板上

`bevy_ui` 节点仍然会被 `bevy_ui_render` 画到相机视口上。spike 的做法：

```rust
Node { position_type: PositionType::Absolute, left: px(-20000.0), .. }
```

布局照常计算（`ComputedNode`/`UiGlobalTransform` 都正确），2D 渲染落在视口外。
**但这只是 spike 的取巧**：正式路线 B 需要"布局树 / 渲染树分离"（或 `UiTargetCamera` 指向一个不渲染的相机），这是交接文档工作量表里没有的一项。

（试过 `Visibility::Hidden`：会顺着 `ChildOf` 把挂在同一批实体上的 3D mesh 一起隐藏，不可用。）

### 3.3 🟠 3D 下的绘制顺序：透明队列按"网格中心距离"排序，会反序

luma 的 `UNodeMaterial3d` 默认 `AlphaMode::Premultiplied`（透明队列）。透明队列的排序键是**网格中心到相机的距离**，不是深度：

- 正对相机（`04_far`、`05_medium`）：子节点略近 → 画在后 → 正确；
- 斜视（`03_oblique`，相机在右前方 45°）：面板中心比左侧子节点**更近** → 面板后画 → **把按钮和色块整块盖住**。

实测证据：`render/shots/03_oblique.png` 在**修补前**只看到面板背景+边框，子节点全部消失。

**两个可选解**：

| 方案 | 做法 | 代价 |
|---|---|---|
| A. 实心节点走 Opaque 队列（本次采用） | 覆写 `Material::alpha_mode`：`color.w >= 0.999 → Opaque`，其余仍 Premultiplied | 失去"半透明面板"；AA 依赖 MSAA（实测仍然锐利） |
| B. 覆写 Transparent3d 排序（正解） | 仿 luma `depth_sync.rs`：给同一面板内所有节点同一个 `mesh_center`（面板根中心）+ 单调递增 `depth_bias` | 需要碰 render phase（`TransparentSortingInfo3d`），约 50–100 行；luma 原版 592 行可抄 |

本次为了把"布局→3D 映射"验证干净，采用 A（对 luma 源码做了 1 处本地补丁，见 §5.2），并在报告里标注 B 是生产方案。

---

## 4. 验证结果

### 4.1 U1 任意缩放不发虚 —— ✅ 通过（定量）

- 场景：面板 340×200 逻辑 px，`units_to_world = 0.002` → 0.68 m × 0.40 m；
- 相机 `z = 0.30 m` 时，1 逻辑 px ≈ **6.47 屏幕 px**（即 **647% 放大**）；
- 实测（`render/shots_isolate/02_close.png`，y=200 扫描线）：

```
x=849 (42,58,83)      <- 色块内部
x=850 (185,49,145)    <- 交界只在 1 个像素内完成
x=851 (198,124,170)   <- 面板背景
```

**边缘过渡 = 1 物理像素**，没有 2–6 px 的模糊带。机制上这是 `unode_3d.wesl` 的
`let grad_outer = length(vec2(dpdx(dist_outer), dpdy(dist_outer)))`（屏幕空间导数）在起作用，
与交接文档 §2.4-A 的推断一致。

> 说明：本次因走 Opaque 队列（§3.3 方案 A），边缘锐利由 **MSAA 4× + SDF** 共同保证；换回 Premultiplied + 方案 B 时 AA 由 SDF 的 `smoothstep(aa_width)` 保证，同样不会随缩放变宽。

### 4.2 U3 shader 兼容性 —— ✅ 通过

`unode_3d.wesl`（14.8 KB，导入 `bevy_pbr::render::{mesh_view_bindings, pbr_types, pbr_functions, forward_io}`）
在 bevy_pbr 0.20 下编译、绑定、出图全部正常（`05_medium.png` 的圆角、边框、多层色块均正确）。
交接文档 §4 的"我判断不兼容" → **判断有误，实际兼容**。

### 4.3 U4 布局→3D 坐标映射 —— ✅ 通过（delta = 0）

判据不依赖 `UiGlobalTransform` 自身（避免自证）：用 `ComputedNode.size` + CSS 常量
（border 2 / padding 14 / row_gap 8 / column 排列）**独立重算**期望位置，再和 3D 实际位置比：

```
[spike] U4 button 455v0: actual=(-0.1880,0.1200,0.0010) expected=(-0.1880,0.1200,0.0010) delta=(0.00000,0.00000,0.00000)
[spike] U4 node   456v0: actual=(-0.1280,0.0440,0.0010) expected=(-0.1280,0.0440,0.0010) delta=(0.00000,0.00000,0.00000)
```

即 `local_size = ComputedNode.size * units_to_world`、`local_offset = child_center - parent_center`（Y 取反）
这套映射**精确成立**（0.0001 mm 级）。

> 踩坑记录（供复现者）：首次实现 delta = 0.052 m，原因有二——
> ① 期望值忘了算 `border: 2px`（content box 起点 = -W/2 + border + padding）；
> ② 文本节点不镜像时仍要占一个 flex 槽位（`row_gap` 对所有子项生效）。
> 这两条都是"bevy_ui 语义 → 3D 映射"必须注意的细节。

### 4.4 U5 3D 下的交互语义 —— ✅ 通过（且比原估算便宜得多）

**结论先行**：只要打开 `MeshPickingPlugin`，**不需要写任何自定义 picking backend**：

```rust
.add_plugins(MeshPickingPlugin)   // 矩形本来就有 Mesh3d + Aabb，直接可被射线命中
```

合成一个 `PointerId::Touch(7)` 指针（`PointerLocation` + `PointerInput` 消息），把按钮的三维中心
投影到屏幕坐标后"悬停—按下—抬起"，走的是**真实** `bevy_picking` 管线：

```
[spike] probe: button 455v0 world=(-0.188,0.120,0.001) screen=(345.02,211.86)
[spike] PointerOver  entity=455v0            # 按钮
[spike] PointerClick entity=455v0 pointer=Touch(7) button=Primary depth=0.9682
[spike] ACTIVATE entity=455v0  <-- widget fired
```

`bevy_ui_widgets::Button` 的 observer 在 3D 场景里照常工作（`PointerPress/Click → Pressed → Activate`），
说明 **feathers 控件的语义层与"它被画在哪"是解耦的**——这对路线 B 是重大利好。

> 与交接文档 §2.4-H 的对比：原方案假设要移植 `hit_math.rs` 写 300–500 行 backend；
> 实测**这条成本可以省掉**。剩下的差异只有：圆角命中（mesh 拾取命中矩形，不认圆角）、
> 遮挡仲裁（若同时想保留 `bevy_ui` 的 2D 拾取）、滚动/裁剪需要额外做。

### 4.5 画面证据

| 文件 | 说明 |
|---|---|
| `render/shots/01_overview.png` | 两块面板 + 参考立方体；立方体被面板 A 正确遮挡（真 3D 深度） |
| `render/shots/03_oblique.png` | 斜视 45°：透视、圆角、边框、子节点顺序全部正确 |
| `render/shots/04_far.png` | 2.4 m 远：面板 B 的 60×60 px 圆角 30 → 正好是圆 |
| `render/shots/05_medium.png` | 0.62 m：**近看依然锐利**（U1 的直观证据） |
| `render/shots_isolate/02_close.png` | 647% 放大 + 品红面板（隔离模式）：测边缘过渡用的图 |
| `render/contact_sheet.png` | 四图拼版 |

### 4.6 未验证 / 明确不在本次范围

| # | 项 | 状态 |
|---|---|---|
| U2 | Bevy 自带 UI 的 `fwidth` artifact | 未做（路线 B 不用 bevy_ui 渲染，优先级下降） |
| — | 3D 文本 | 未做（spike 里文本节点被跳过；本 spike 未提供字体资产，标题文本实测只占一个 flex 槽位、高度按 0 计——由 U4 的 delta=0 反推） |
| — | 嵌套裁剪 / 滚动容器 | 未做（交接文档已声明出范围） |
| — | 半透明面板 | 本次走 Opaque 规避，**半透明场景未验证**（需方案 B） |
| — | 性能 | llvmpipe 软件渲染，数据无意义；需真机（本机有 Radeon 7600，但会话看不到 `/dev/dri`） |
| — | 与 `bevy_ui` 2D 拾取的仲裁 | 未验证（本 spike 把 UI 挪出视口，天然不冲突） |

---

## 5. spike 相对交接版的改动

### 5.1 `spike/src/main.rs`（345 → 852 行，重写）

| 改动 | 原因 |
|---|---|
| `LumaPlugin` → `UnivisRenderPlugin` | §3.1 调度环 |
| UI 根节点 `left: -20000px` | §3.2 避免 2D 覆盖 |
| `WorldPanel { units_to_world, anchor: Transform }` | 原版面板只能放在世界原点；现支持任意锚点/旋转/多面板 |
| 每层 `z += 0.001` 的 Z 偏置 | 同面板内子节点必须"在父节点前面" |
| `PanelNode` 缓存 mesh/material 句柄 | 原版每帧 `meshes.add` + `materials.add`，**每秒泄漏 60×N 个资产** |
| 材质只在样式变化时改写 | 否则 hover 高亮会被下一帧覆盖 |
| 截图巡游（`Runner`，5 个机位） | 无人值守产出证据 |
| 合成指针（Touch 7）hover/click | 无鼠标也能验证 U5；且让截图可复现 |
| U4 量化断言（独立重算期望位置） | 把"看起来对"变成"delta=0" |
| `SPIKE_ISOLATE=1` 隔离模式 | 单独看面板 A、品红底，便于像素级测边缘 |

### 5.2 对 `bevy_luma` 的**唯一**本地补丁（1 处，已注释）

`bevy_luma/crates/luma_engine/src/layout/render/material_3d.rs`：

```rust
fn alpha_mode(&self) -> AlphaMode {
    // ROUTE-B SPIKE PATCH: 全不透明节点走 opaque 队列，用深度测试解决父子顺序
    if self.color.w >= 0.999 { AlphaMode::Opaque } else { AlphaMode::Premultiplied }
}
```

**这只是 spike 的权宜之计**；正式实现应改为 §3.3 方案 B（Transparent3d 排序覆写），保留半透明能力。

---

## 6. 对交接文档工作量估算的修订

| 模块 | 交接估算 | 本地实测后修订 | 说明 |
|---|---|---|---|
| 3D 材质 + shader 复用 | 100–200 行 | **≈0 行**（直接可用） | 只要 `UnivisRenderPlugin`；唯一要改的是 `alpha_mode` |
| 布局→3D 坐标映射 | 200–400 行 | **≈150 行**（已实现） | 机制成立，含脏检查/句柄缓存 |
| 3D 拾取 | 300–500 行 | **≈50 行**（合成指针 + mesh_picking） | 自研 backend 可省；圆角/裁剪命中另算 |
| 深度排序 / 透明队列 | 150–300 行 | **150–300 行（未省）** | 原估算偏乐观，实为**必须做**（方案 B） |
| 布局树/渲染树分离 | 未列 | **100–300 行** | 新增项：2D overlay 必须关掉 |
| 3D 文本 | 150–300 行 | 未动（仍估 150–300） | — |
| 控件语义适配 | 800–1500 行 | **可能显著下降** | `bevy_ui_widgets` 的 observer 在 3D 下直接生效；悬停/焦点仍需按控件验证 |
| **M1（渲染原型）** | 2–3 人周 | **维持 2–3 人周**，但风险从"机制未知"降为"工程细节" | 机制已全部落地 |

**关键判断**：路线 B **技术可行且已验证**。真正的工作量重心从"能不能渲染/能不能点"转移到
**"多面板绘制顺序 + 布局树与渲染树解耦 + 3D 文本 + 控件语义"** 四件事上。

---

## 7. 复现步骤

```bash
cd research

# 1) 源码（github.com 被墙时走 api + raw，见 §1.3）
ls bevy_luma/Cargo.toml bevy_luma/crates/luma_engine/src/layout/render/material_3d.rs

# 2) 编译
cd spike
export CARGO_HOME=$PWD/../.cargo-home     # 本机会话沙箱 ~/.cargo 只读
cargo build                               # 全量 ~3 min（增量 ~35 s）

# 3) 主巡游：5 机位截图 + 合成指针 hover/click + U4 断言
SPIKE_SHOT_DIR=$PWD/../render/shots ./target/debug/spike

# 4) 隔离模式（品红面板，测边缘锐度）
SPIKE_ISOLATE=1 SPIKE_SHOT_DIR=$PWD/../render/shots_isolate ./target/debug/spike

# 5) 只看断言
grep -E 'U4 |panels=|ACTIVATE' ../.tmp/final1.log
```

运行约 35 秒（软件渲染下单帧 ~60 ms），退出码 0；结束时打印巡游截图清单。

---

## 8. 下一步建议（按性价比排序）

1. **做方案 B（Transparent3d 排序覆写）**，把半透明面板解禁——这是目前唯一挡住"可用面板"的渲染问题；可直接抄 luma `depth_sync.rs` 的 `world3d_layer_sort_bias`。
2. **设计布局树/渲染树分离**（`UiTargetCamera` 到不渲染相机 / 专用 UI 世界），替换 spike 的 `left:-20000` 取巧。
3. **3D 文本**：先接 `bevy_text` 的 glyph atlas 到 luma 的 `text_label_sdf_3d.wesl`（luma 已有 147 行参考）。
4. **控件语义抽样验证**：button（已过）、slider（拖拽）、scrollarea（滚轮 + 裁剪）、text input（焦点/输入法）。
5. **U2 可以降级**：路线 B 不走 bevy_ui 渲染，`fwidth` artifact 只影响"如果哪天回退到 bevy_ui 渲染"的分支。

---

## 附：产物清单

```
research/
├── VERIFICATION_REPORT.md            # 本文件
├── bevy_luma/                        # git main（Bevy 0.20）+ 1 处 alpha_mode 补丁
├── spike/
│   ├── Cargo.toml                    # 交接版原样（版本钉法有效）
│   ├── src/main.rs                   # 重写，含巡游/U4 断言/合成指针
│   ├── RESETUP_DOC.md                # 交接文档（顶部加了续跑状态）
│   └── target/debug/spike            # 可执行
├── render/
│   ├── shots/01..05*.png             # 主巡游
│   ├── shots_isolate/01..05*.png     # 隔离模式
│   ├── contact_sheet.png
│   └── isolate_sheet.png
├── .cargo-home/                      # 工作区内的 CARGO_HOME（沙箱原因）
└── .tmp/                             # 各轮运行日志（run1..run8 / final1..2）
```
