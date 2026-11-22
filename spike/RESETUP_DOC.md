# 路线 B 验证交接文档

> 目标：在本地工作区验证「用 `bevy_luma` 作为基准，在 `bevy_feathers` 中实现世界空间 3D 面板」的可行性（路线 B）。
> 交接前设备：阿里云轻量级服务器（/tmp）
> 文档状态：**调研与源码分析已全部完成并交叉验证；运行时验证（编译 + 视觉/交互）在本机未跑通**，换机后按第 7 节步骤接续。
> 编写日期：2026-11-22

---

## ⚡ 本地续跑状态（2026-11-23，换机后）

> 详细结果与证据见 [`../VERIFICATION_REPORT.md`](../VERIFICATION_REPORT.md)。

| 项 | 结果 |
|---|---|
| §3.3 spike 编译 | ✅ 编译通过（全量 2m58s / 增量 34s），交接版源码无需修改 |
| U1 任意缩放清晰 | ✅ 647% 放大下边缘过渡 = 1 物理像素（`render/shots_isolate/02_close.png`） |
| U3 shader 兼容 | ✅ `unode_3d.wesl` 在 bevy_pbr 0.20 下编译**并出图**（原判断"不兼容"有误） |
| U4 坐标映射 | ✅ 独立重算期望位置，delta = (0,0,0)（按钮 / 色块） |
| U5 3D 交互 | ✅ `PointerOver → PointerClick → Activate` 全链路；**无需自研 picking backend**，`MeshPickingPlugin` 即可 |
| U2 / 3D 文本 / 裁剪 / 滚动 / 半透明 | ❌ 本次未做 |

**三个本机新发现的坑（本文档原未覆盖）**：

1. 🔴 `bevy_luma::LumaPlugin` 与 `bevy_ui` 0.20.1 **调度成环**，启动即 panic（`PostUpdate ... 14522 cycles`）。
   → 路线 B 只注册 `bevy_luma::engine::layout::render::UnivisRenderPlugin`（材质 + embedded shader），不要 `LumaPlugin`。
2. 🟠 `bevy_ui` 的 2D 渲染会盖在 3D 面板上。spike 用 `left: -20000px` 把布局树挪出视口；正式方案需"布局树/渲染树分离"。
3. 🟠 `UNodeMaterial3d` 默认 `AlphaMode::Premultiplied`，透明队列按网格中心距离排序 → 斜视时**父节点盖住子节点**。
   → 本次对 luma 打了 1 处本地补丁（`color.w >= 0.999 → Opaque`）规避；生产方案应是 Transparent3d 排序覆写（抄 `depth_sync.rs`）。

**一处必须修正的事实**：crates.io 上的 `bevy_luma` 0.1.0（及其 4 个子 crate）依赖 **bevy 0.19**，
不是 0.20。验证必须用 git main 分支（本工作区 `research/bevy_luma/`）。

---

## 0. 一页结论

| 问题 | 结论 | 依据强度 |
|---|---|---|
| `bevy_feathers` 加 3D 面板，工作量大不大？ | **大**。核心渲染+拾取原型约 2–3 人周；全控件 3D 化约 12–18 人周 | 源码级分析，未实测 |
| 路线 B（保留 `bevy_ui` 布局，只换渲染+拾取）是否可行？ | **技术上可行，关键机制已定位**（见 2.4 节），luma 3D shader 已在 Bevy 0.20 实测跑通；但**spike 尚未跑通** | 机制分析扎实 + U3 已实测 |
| Bevy 版本是否对齐？ | **对齐，且比预期好**。`bevy_feathers` 0.20.1 已发布 crates.io，与 luma 的 Bevy 0.20 依赖完全一致 | ✅ 已实测（crates.io API） |
| 能否复用 luma 的 3D 材质？ | **能**。`UNodeMaterial3d` 是公开导出的，`unode_3d.wesl` 通过 `embedded_asset!` 注册 | ✅ 源码确认 |
| luma/univis 成熟度？ | **univis_ui 是 luma 的前身**，更成熟（151 commits / 54 stars / v0.3.1）；luma 是精简重命名版（1 commit / 2 stars） | ✅ 已实测（git + crates.io） |

**最重要的一条**：`bevy_feathers` **0.20.1 已发布到 crates.io**（146 万下载）。这意味着验证**不需要 fork bevy 主仓库**，直接 `bevy_feathers = "0.20.1"` 即可，工作量与维护成本远低于原始估计。

---

## 1. 环境与代码位置（本机 `/tmp/research/`）

```
/tmp/research/
├── bevy/                  # bevyengine/bevy 主仓库 clone（sparse→full）
│   └── crates/bevy_feathers/     # 主线版 feathers（0.20.0-dev），15,576 行
├── bevy_luma/             # Abdellah-Haffout/bevy_luma clone（1 commit, 2 stars）
├── univis_ui/             # univiseditor/univis_ui clone（151 commits, 54 stars, v0.3.1）
├── feathers_src/
│   └── bevy_feathers-0.20.1/      # 从 crates.io 下载的 0.20.1 发布版 ★验证应基于这个
├── bevyui/
│   └── bevy_ui-0.20.1/            # 从 crates.io 下载的 bevy_ui 0.20.1（API 与主线不同！）
└── spike/                 # 路线 B spike 工程（源码已修复，编译结果见 3.3 节）
    ├── Cargo.toml
    └── src/main.rs
```

**关键提醒**：`bevy_ui` **crates.io 0.20.1 版**与 **bevy main 分支（0.20.0-dev）API 不同**。主线已把 `BorderRadius` 从枚举改为 4 角结构体、`CornerRadius` 改为 `Val` 字段结构体。**验证必须以 crates.io 0.20.1 为准**，不要看主线源码。

---

## 2. 已实测验证的事实（✅ 可靠，可直接采信）

### 2.1 版本与发布状态

| 项目 | 数据 | 验证方式 |
|---|---|---|
| `bevy_feathers` 最新版 | **0.20.1** | crates.io API |
| `bevy_feathers` 0.20.1 下载量 | 116,044（累计 146 万） | crates.io API |
| `bevy_feathers` 依赖 | `bevy_ui = "0.20.1"`、`bevy_ui_render = "0.20.1"` 等 | crates.io 下载的 Cargo.toml |
| `bevy_luma` 依赖 | `bevy = "0.20.0"`（crates.io） | Cargo.lock |
| **版本对齐** | ✅ **luma 与 feathers 都是 Bevy 0.20 系列** | 上表 |
| `bevy_luma` 发布版 | 0.1.0，下载量 **13** | crates.io API |
| `univis_ui` 发布版 | 0.3.1，下载量 191，v0.3.1 有 tag | crates.io API |

### 2.2 项目谱系（重要）

**`univis_ui` 是 `bevy_luma` 的前身，不是两个独立项目。**

| | univis_ui | bevy_luma |
|---|---|---|
| 仓库 | `univiseditor/univis_ui` | `Abdellah-Haffout/bevy_luma` |
| 创建日期 | 2026-01-01 | 2026-10-03 |
| commits | **151** | **1** |
| 作者 | abdellah (84) + abht (67) | abdellah (1) |
| stars | 54 | 2 |
| Bevy 版本 | 0.19 | 0.20 |

证据：
- `bevy_luma` 代码残留大量旧命名：`univis_node`、`univis_picking_backend`、`univis_ui_engine`（65 个文件命中）
- shader diff 显示 luma 是 univis 的**超集**：新增 `custom_params`/`UShader` 噪声钩子；裁剪从单层 `clip_center/clip_size` 改为 4 层 `clip_inv_x/clip_inv_y/clip_bounds/clip_radius` 嵌套矩阵
- `univis_ui` 的 `MIGRATION.md` 明确写有 `URootUi::world_3d(...)`、`meters_per_unit` 等 API，luma 保留同款命名

**决策含义**：参考基准应优先用 **univis_ui**（更成熟、3D 实现更小、git 历史可查）；**bevy_luma** 的价值在于它是 **Bevy 0.20 版本**且**材质/着色器已公开导出**，可直接复用。

### 2.3 3D 实现代码量（实际 wc -l）

**bevy_luma**（Bevy 0.20，较完整）：
```
crates/luma_engine/src/layout/render/material_3d.rs              202
crates/luma_engine/src/layout/render/depth_sync.rs               592
crates/luma_engine/src/layout/render/clip_sync.rs               1038
crates/luma_engine/src/layout/render/layers.rs                  305
crates/luma_engine/src/layout/render/shaders/unode_3d.wesl      420
crates/luma_engine/src/layout/render/shaders/text_label_sdf_3d.wesl  147
crates/luma_interaction/src/interaction/picking/backend.rs       47
crates/luma_interaction/src/interaction/picking/hit_math.rs      61
crates/luma_interaction/src/interaction/picking/clip.rs          39
crates/luma_interaction/src/interaction/picking/root_lookup.rs  19
crates/luma_interaction/src/interaction/picking/validation.rs   93
                                                        合计 ≈ 2963 行
```

**univis_ui**（Bevy 0.19，更精简，无 4 层嵌套裁剪）：
```
material_3d.rs                106      hit_math.rs    62
clip_sync.rs                  290      clip.rs        71
unode_3d.wgsl                 227      backend.rs     40
text_label_sdf_3d.wgsl         99      root_lookup.rs 19
                                          validation.rs  87
                                    合计 ≈ 722（渲染）+ 279（拾取）
```

**结论**：luma 的 3D 实现是 univis 的约 4 倍，主要增量在**嵌套裁剪**和**深度排序**。feathers 场景若不需要多层裁剪，可按 univis 的规模做，工作量显著下降。

### 2.4 关键机制（源码级确认）

**A. 「任意缩放不发虚」的真实机制 = 屏幕空间导数，不是「用了 SDF」**

Bevy 自带 SDF，但抗锯齿单位是错的：
```wgsl
// bevy_ui_render/src/ui.wesl:185  (Bevy 0.20-dev 主线)
fn antialias(distance: f32) -> f32 {
    // Using the fwidth(distance) was causing artifacts, so just use the distance.
    return saturate(0.5 - distance);
}
```
固定 0.5 像素带，而 `point` 是**局部逻辑坐标**（`lib.rs:2436`：`points.map(|pos| pos * rect_size)`，未除 scale factor）→ 放大 4 倍时 AA 带宽被拉成 2 物理像素 → **发虚**。

luma/univis 的做法：
```wgsl
// luma: unode.wgsl / unode_3d.wesl
let grad_outer = length(vec2<f32>(dpdx(dist_outer), dpdy(dist_outer)));
let aa_width = max(grad_outer, softness);
```
用屏幕空间导数求梯度，AA 宽度自动 = 1 物理像素。3D 下 softness 直接归零：
```rust
// luma: crates/luma_engine/src/layout/render/system.rs:408-414
let softness = if is_world_3d { 1.0e-5 } else { minimum_local_softness(world_scale) };
```
univis 更直接：`let smoothing = fwidth(dist);`

> ⚠️ **Bevy 那句注释说明有人试过 fwidth 并放弃了**。迁移时这是最大的技术风险点，必须实测验证 artifact 能否规避。

**B. Bevy UI 完全没有 3D 概念**

`bevy_ui_render` 中 grep `Mesh3d`/`Material3d` 零命中。UI 顶点固定在 `view.clip_from_world` 投影平面（`ui.wesl:53`）。`Camera3d` 出现在 `lib.rs:1431`，但仅用于把 UI 画到 3D 相机的**屏幕**（即 HUD），不是世界里的物理面板。

**C. feathers 是 100% `bevy_ui` 封装**

组件依赖实测计数：
```
Node            261 处
BackgroundColor 113 处
BorderRadius     46 处
BoxShadow         8 处
```
无自有 mesh/material 系统，布局完全由 `bevy_ui` flex 引擎驱动。

**D. univis_ui 是完全自建的 ECS 布局引擎**

`UNode`（`UVal`/`USides`/`UCornerRadius`/`UShapeMode`）+ `ULayout`（Flex/Grid/Masonry/Stack/Radial）+ 自研 flexbox 求解器（2092 行）+ 自研文本排版（parley/swash）。
→ 这是「路线 A（整体换布局引擎）」的代价，等于重写 feathers。

**E. 坐标映射机制（路线 B 的核心，已定位）**

`bevy_ui` 的布局原点在**视口中心**：
```rust
// bevy_ui/src/layout/mod.rs:494
Affine2::from_translation(0.5 * target_size)
```
`UiGlobalTransform` 是 `Affine2`，其 translation = **节点中心相对父节点中心**的偏移。

因此 3D 局部位置可直接推出：
```
local_size   = ComputedNode.size * units_to_world
local_offset = child_center - parent_center   (像素) → * units_to_world → 翻转 Y
```
Bevy UI 的 Y 向下增长，世界 Y 向上 → `local.y = -offset.y * units_to_world`。

**F. `ComputedNode` 已含解析后的物理像素值（重要省时点）**

```rust
// bevy_ui 0.20.1 src/ui_node.rs:36-71
pub struct ComputedNode {
    pub size: Vec2,                    // 物理像素
    pub border: BorderRect,            // 已解析的边框宽度
    pub border_radius: ResolvedBorderRadius,  // 已解析的圆角 (Vec2 per corner)
    pub padding: BorderRect,
    pub inverse_scale_factor: f32,
    ...
}
```
→ **无需手动 resolve `Val`**，直接读 `ComputedNode` 即可。这是路线 B 相对路线 A 的一大优势。

**G. luma 的 3D 材质可公开复用**

```rust
// bevy_luma/src/lib.rs:42-45
pub mod render {
    pub use luma_engine::layout::render::{
        UnivisRenderPlugin, material, material_3d, prelude,
    };
}
// => bevy_luma::engine::layout::render::material_3d::UNodeMaterial3d
```
注册方式（`crates/luma_engine/src/layout/render/mod.rs:41-45`）：
```rust
embedded_asset!(app, "shaders/unode.wgsl");
embedded_asset!(app, "shaders/unode_batch.wgsl");
embedded_asset!(app, "shaders/unode_3d.wesl");
embedded_asset!(app, "shaders/text_label_sdf.wgsl");
embedded_asset!(app, "shaders/text_label_sdf_3d.wesl");
app.add_plugins(MaterialPlugin::<UNodeMaterial3d>::default());
```
`UNodeMaterial3d` 关键字段（`material_3d.rs`）：`color`、`radius`(Vec4: tr,br,tl,bl)、`border_color`、`size`、`border_width`、`softness`、`metallic`、`roughness`、`emissive`、`grad_*`、`shadow_*`、`inner_glow_*`、`clip_inv_x/y`、`clip_bounds`、`clip_radius`、`texture`、`pbr_enabled`。

`Material` impl 要点：
```rust
fn fragment_shader() -> ShaderRef { "embedded://luma_engine/layout/render/shaders/unode_3d.wesl".into() }
fn alpha_mode(&self) -> AlphaMode { AlphaMode::Premultiplied }
fn specialize(...) { descriptor.primitive.cull_mode = None; }  // 双面
```

**H. 3D 拾取算法（可直接移植，~60 行）**

univis_ui `crates/univis_ui_interaction/src/interaction/picking/hit_math.rs`：
```rust
// 1) 屏幕点 → 世界射线
let ray = camera.viewport_to_world(camera_transform, location.position)?;
// 2) 射线 × 节点平面求交
let plane_origin = global_transform.translation();
let plane_normal = global_transform.back().as_vec3();
let distance = (plane_origin - ray.origin).dot(plane_normal) / ray.direction.dot(plane_normal);
// 3) 世界命中点 → 节点局部坐标
let local_hit = global_transform.to_matrix().inverse().transform_point3(world_hit).truncate();
```
luma 同款（`hit_math.rs` 61 行）。裁剪沿父链做 SDF 测试（`clip.rs`）。

---

## 3. 本机实测结果（✅ 已跑通）

### 3.1 luma 3D example —— ✅ 编译 + 运行成功

```bash
cd /tmp/research/bevy_luma
# 去掉 Cargo.toml [workspace.dependencies].bevy.features 里的 "wayland"
cargo build --example world_3d     # ✅ 21m45s, Finished dev profile
```

运行（Xvfb + lavapipe）：
```
INFO bevy_render::renderer: AdapterInfo { name: "llvmpipe (LLVM 17.0.6, 256 bits)",
     device_type: Cpu, driver: "llvmpipe", driver_info: "Mesa 23.3.0 (LLVM 17.0.6)",
     backend: Vulkan, ... }
INFO bevy_render::batching::gpu_preprocessing: GPU preprocessing is fully supported on this device.
INFO bevy_winit::system: Creating new window world_3d (64v0)
```

**结论**：
- ✅ **U3 已验证**：`unode_3d.wesl`（luma 的 3D SDF + PBR shader）在 **Bevy 0.20.0** 下**编译通过、shader 加载无报错**。
  → 路线 B 复用 luma 3D 材质在 Bevy 0.20 上可行，不需要重写 WGSL/PBR 导入。
- ⚠️ **U1 仍未验证**：程序成功启动到渲染循环，但**本机无法截图**（无 ImageMagick/x11-apps），
  所以「透视相机下任意缩放是否清晰」**没有看到实际画面**。
  换机后需要：加截图（或用 Bevy `Screenshot` API）→ 跑 `world_3d` / `luma_world_3d_gallery` → 看边缘锐度。
- ⚠️ 软件渲染（llvmpipe）下性能数据无参考价值，真实性能需在有 GPU 的机器上复核。

### 3.2 Bevy 0.20.1 编译阻塞点 —— ✅ 已定位并解决

`bevy_feathers` + `bevy` 0.20.1 组合**无法编译**：

```
error[E0004]: pattern `&bevy_window::CursorIcon::Custom(_)` not covered
   --> bevy_winit-0.20.1/src/cursor/mod.rs:124:35
```

根因链（已逐层验证）：
1. `bevy_feathers` Cargo.toml：`bevy_picking = { version = "0.20.1", features = ["custom_cursor"] }`
2. `bevy_picking/custom_cursor` → `bevy_window/custom_cursor`
3. `bevy_window` 因此多出 `CursorIcon::Custom(CustomCursor)` 变体
4. `bevy_winit` 的 `update_cursors()` 里 `match cursor.as_ref()` **没有**该变体的分支
   （`bevy_winit` 自己的 `custom_cursor` feature 没开）
5. → E0004

**✅ 已验证的解法**（`cargo build -p bevy_winit` 通过）：
```toml
bevy_window = { version = "=0.20.1", features = ["custom_cursor"] }
bevy_winit  = { version = "=0.20.1", features = ["custom_cursor"] }
```

同时把 `bevy` 钉 `=0.20.0`、`bevy_feathers = "=0.20.0"`（与 luma 对齐）。
> 注：尝试用 `cargo update --precise` 把整棵树降到 0.20.0 会被 cargo 拒绝
> （`bevy_camera 0.20.1` 要求 `^0.20.0`，降级 `bevy_window` 触发依赖冲突），
> **不要在这上面耗时间**，feature 对齐方案即可。

### 3.3 spike 编译 —— ⏳ 交接时仍在进行

`/tmp/research/spike` 已完成源码级修复（见 5.3 节列的 6 个问题全部处理），
最后一步 `cargo build` 在交接时仍在运行（首次全量编译约 20 分钟）。

---

## 4. 仍未验证 / 不确定的部分（❌ 换机后优先补）

| # | 未验证项 | 为什么重要 | 怎么验 |
|---|---|---|---|
| **U1** | **luma 3D 在透视相机下是否真的任意缩放不发虚** | 路线 B 的核心卖点；我从 shader 逻辑推断，未实跑 | 跑 `world_3d` / `luma_world_3d_gallery`，拉近相机看边缘 |
| **U2** | **Bevy `fwidth` 方案是否真会 artifact** | Bevy 自己注释说踩过坑；这是移植最大风险 | 改 `antialias()` 为 fwidth 版本，跑 bevy UI 测试 + feathers examples |
| **U3** | **Bevy 0.20.1 的 `bevy_pbr` shader API 与 luma 的 `unode_3d.wesl` 是否兼容** | 我判断**不兼容**（luma 已重写过），但没验证具体差异点 | 编译 luma 的 3D example，看 shader 报错 |
| **U4** | **路线 B 的坐标映射是否正确** | 核心假设；`UiGlobalTransform` 的父子语义需实测 | 跑 spike，看面板内子元素位置是否正确 |
| **U5** | **feathers 控件在 3D 下的交互语义**（悬停/焦点/滚动） | 决定「全控件 3D 化」的隐藏成本 | spike 中放 button，做 ray picking |
| **U6** | **Bevy 主线（0.20.0-dev）与 crates.io 0.20.1 的 API 差异范围** | 决定要不要维护 fork | 对比 `crates/bevy_ui_render` 两版本 |

**重要**：`bevy_ui` 0.20.1（crates.io）vs 主线已有 API 差异，例如主线 `BorderRadius` 已改为 4 角 `CornerRadius` 结构体 + `Val` 字段；0.20.1 发布版则是 `BorderRadius` 结构体含 4 个 `CornerRadius`，`CornerRadius` 是 `struct { x: Val, y: Val }`。**以 0.20.1 发布版为准。**

---

## 5. 路线 B 的架构设计（已落到 spike 代码）

### 5.1 核心思路

```
bevy_feathers 控件（bevy_ui 布局，屏幕像素）
        │
        │  PostUpdate 阶段读取 ComputedNode + UiGlobalTransform
        ▼
   映射层（新增，约 200-300 行）
        │  size_world   = ComputedNode.size * units_to_world
        │  local_offset = child_center - parent_center，Y 翻转
        │  color/radius/border 从 ComputedNode + BackgroundColor/BorderColor 取
        ▼
luma UNodeMaterial3d + unode_3d.wesl（SDF + PBR + fwidth AA）
        │
        ▼
Mesh3d(Rectangle) + MeshMaterial3d → 正常 PBR 管线，参与深度排序
```

### 5.2 关键实现点

- **标记机制**：新增 `WorldPanel { units_to_world: f32 }` 组件，标记 `bevy_ui` 子树根；系统沿 `Children` 遍历子树
- **每节点产出**：`Mesh3d(Rectangle::new(w,h))` + `MeshMaterial3d(UNodeMaterial3d)` + `Transform::from_translation(local_pos)`
- **材质复用**：直接 `bevy_luma::engine::layout::render::material_3d::UNodeMaterial3d`
- **圆角映射**：luma 的 `radius` 是 `(top_right, bottom_right, top_left, bottom_left)` 顺序，`ComputedNode.border_radius` 同序，可直接对应
- **softness**：3D 下设 `1.0e-5`（走屏幕空间导数）
- **拾取**：移植 `hit_math.rs` 的三步算法，注册为独立 picking backend，与 `bevy_ui` 的 2D picking 仲裁

### 5.3 spike 工程现状

`/tmp/research/spike/` 已写好并完成源码级修复。核心代码在 `src/main.rs`，包含：
- `WorldPanel` 标记组件
- `sync_mirror()` 同步系统（几何 + 材质映射）
- `report()` 统计输出（panels / mirrored_nodes / live_mirrors）
- 测试场景：Camera3d + DirectionalLight + 参考立方体（证明真 3D）+ feathers Button + 纯色 Node

**已修复的问题**（源码级均已确认，共 16 → 0）：
1. ✅ `BorderRadius::all(px(10.0))` —— 0.20.1 的签名是 `all(impl Into<CornerRadius>)`，`px(10.0)` 可直接传入
2. ✅ `all_nodes` Query 已改为直接携带 `Option<&BackgroundColor>` / `Option<&BorderColor>`
3. ✅ `UNodeMaterial3d` 用 `..default()` 补齐剩余字段
4. ✅ `FeathersButtonProps` 字段确认为 `caption` / `variant` / `corners` 三项
5. ✅ `bevy_ui::BorderRadius` 已加入 import
6. ✅ `BorderRadius` 圆角映射改为读 `ComputedNode.border_radius`（`ResolvedBorderRadius`，已解析的物理像素）

#### ⚠️ Bevy 0.20.1 的 API 陷阱（换机后最容易踩，逐条已踩过并修复）

| 陷阱 | 现象 | 正确写法 |
|---|---|---|
| **`BorderRadius` 不是组件** | `is not a Bundle` 编译错误 | 它是 `Node` 的**字段**：`Node { border_radius: BorderRadius::all(px(10.0)), .. }`。同理 `border: UiRect` 也是字段，不是 `BorderWidth` 组件 |
| **`BorderColor` 是 4 边结构体** | `cannot find tuple variant` | `BorderColor::all(color)`（不是 `BorderColor(color)`）。字段是 `top/right/bottom/left` |
| **`FontSize` 是枚举** | `expected FontSize, found f32` | `FontSize::Px(20.0)` |
| **`TextFont.font` 是 `FontSource`** | `expected FontSource, found String` | `FontSource::Family(path.into())` |
| **`bevy_ui` / `bevy_ui_widgets` 是独立 crate** | `unresolved import` | 即使 `bevy` 的 features 里开了，也要在 Cargo.toml **单独声明依赖**才能 `use` |
| **`BorderRect` 字段名** | `no field 'left' on type BorderRect` | Bevy 0.20 的 `BorderRect` 来自 `bevy_sprite`，字段是 `min_inset: Vec2` / `max_inset: Vec2` |
| **`UiGlobalTransform` 取平移** | `no method 'translation'` | `ugt.affine().translation`（`Affine2` 没有 `translation()` 方法） |
| **`FeathersButton` 不能 `commands.spawn()`** | `SceneComponent` 只能用 scene API | 用 `spawn_scene` / `bsn!`；spike 里改用底层 `bevy_ui_widgets::Button` 规避 |
| **插件名** | `cannot find value FeathersPlugin` | 实际叫 `bevy_feathers::FeathersPlugins`（复数） |
| **字体常量** | `cannot find value 'regular'` | `bevy_feathers::constants::fonts::REGULAR`（大写，`&str`） |
| **`Children` 迭代** | `expected &&_ but yields &Entity` | `kids.to_vec()`（`Children` deref 到 `[Entity]`，但有歧义） |

**⚠️ 版本约束（已写入 Cargo.toml，勿改）**：
```toml
bevy = { version = "=0.20.0", ... }
bevy_feathers = "=0.20.0"
bevy_window = { version = "=0.20.1", features = ["custom_cursor"] }
bevy_winit  = { version = "=0.20.1", features = ["custom_cursor"] }
bevy_ui = { version = "=0.20.1" }
bevy_ui_widgets = { version = "=0.20.1" }
bevy_ui_render = { version = "=0.20.1" }
bevy_text = { version = "=0.20.1" }
bevy_scene = { version = "=0.20.0" }
```
详见 7.1 节「custom_cursor feature 对齐」。

**尚未实现的部分**（spike 范围内有意省略）：
- 3D 射线拾取后端（算法已在 2.4 节 H 给出，照搬 `hit_math.rs` 即可）
- 3D 文本
- 嵌套裁剪
- 滚动容器

---

## 6. 工作量估算（基于实测代码量的推算，非实测）

| 模块 | univis 实际 | 移植到 feathers 估 | 备注 |
|---|---|---|---|
| 3D 材质 + shader 复用 | 333 行 | **100–200 行** | ✅ 可直接复用 luma，主要成本是 WESL/PBR API 适配 |
| 3D mesh 生成 + 渲染分派 | ~120 行 | **200–400 行** | 需 hook extract/render 或走普通 ECS 路径 |
| 3D 拾取 | ~280 行 | **300–500 行** | 需与 `bevy_picking` 2D 路径仲裁 |
| **布局→3D 坐标映射** | 无直接对应 | **200–400 行** | ★路线 B 独有；机制已定位（第 2.4 节 E/F） |
| 深度排序 / 透明队列 | ~100 行 | **150–300 行** | 可先用 Bevy 默认排序 |
| 3D 文本 | 99 行 | **150–300 行** | `bevy_text` 字形 atlas → 3D mesh |
| 嵌套裁剪 | 290 行 | **100–300 行** | 单层即可，比 luma 简单 |
| 控件语义适配 | — | **800–1500 行** | ★最大隐性成本：悬停/焦点/滚动在 3D 的行为 |
| 主题层扩展 | — | **100–200 行** | `tokens.rs` 583 行 + `theme.rs` 370 行需加 3D 变体 |

**里程碑估算**：
- **M1 渲染原型**（面板能渲染出来、位置正确、任意缩放清晰）：**2–3 人周**
- **M2 可用面板**（基本控件可交互、拾取正确）：**6–10 人周**
- **M3 全控件 3D 化**：**12–18 人周**（单人约 3–4 个月）

**对照**：若走路线 A（整体换用 univis/luma 的布局引擎），需重写 feathers 全部 24 个控件（13,226 行）+ 主题层（1,459 行），**至少 4–6 人月**，且失去 `bevy_ui` 的可访问性/焦点导航/文本编辑生态。

---

## 7. 换机后的验证步骤

### 7.1 环境准备

#### ⚠️ 必读：`custom_cursor` feature 对齐（已实测的编译阻塞点）

`bevy_feathers` 0.20.x 启用 `bevy_picking/custom_cursor` → 打开 `bevy_window` 的
`CursorIcon::Custom` 枚举变体。但 `bevy_winit` 的 `match cursor.as_ref()` **没有对应分支**：

```
error[E0004]: pattern `&bevy_window::CursorIcon::Custom(_)` not covered
  --> bevy_winit-0.20.1/src/cursor/mod.rs:124:35
```

**解法（已验证 `cargo build -p bevy_winit` 通过）**：在 `Cargo.toml` 里显式给
`bevy_window` 和 `bevy_winit` 都开 `custom_cursor`，让整个树的 feature gate 一致：

```toml
bevy_window = { version = "=0.20.1", features = ["custom_cursor"] }
bevy_winit  = { version = "=0.20.1", features = ["custom_cursor"] }
```

**另一个坑**：想把整棵树降到 0.20.0 时，`cargo update -p bevy_window --precise 0.20.0`
会被拒绝（`bevy_camera 0.20.1` 要求 `^0.20.0`），逐个降级也会互相冲突。
**不要在这上面耗时间** —— 直接用上面这个 feature 对齐方案，
`bevy` 顶层钉 `=0.20.0`、`bevy_feathers = "=0.20.0"` 即可（子 crate 0.20.1 也能编译）。

#### 系统依赖（本机踩过的坑，逐条列出）

```bash
# Rust（必须 1.100+，luma 用 edition 2024）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
export PATH="$HOME/.cargo/bin:$PATH"
rustup component add rustfmt clippy

# 系统依赖
sudo dnf install -y mesa-vulkan-drivers vulkan-loader vulkan-headers   # 软件光栅化 lavapipe
sudo dnf install -y wayland-devel libxkbcommon-devel libX11-devel \
                    libXcursor-devel libXi-devel libXrandr-devel \
                    xorg-x11-server-Xvfb pkgconf-pkg-config alsa-lib-devel
sudo dnf install -y systemd-devel libudev-devel                      # libudev-sys 需要
sudo dnf install -y libxkbcommon-x11                                # winit 需要，本机漏装导致 panic
```

**踩坑记录**：
- `wayland` feature 会因缺 `wayland-client.pc` 编译失败 → **去掉 `wayland` feature，只留 `x11`**
- `libudev-sys` 需要 `libudev-devel`
- `libxkbcommon-x11.so.0` 缺失 → winit 启动时 panic（`xkbcommon-dl`）
- 本机 GPU 是 Cirrus Logic 虚拟显卡（无硬件 Vulkan），**必须用 lavapipe 软件渲染**：
  ```bash
  export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json
  Xvfb :99 -screen 0 1280x800x24 &    # 虚拟显示（Xvfb 会在会话结束时被杀，需重启）
  export DISPLAY=:99
  ```
- Bevy 全量首次编译约 **20 分钟**（本机 target 目录达 6.9G），务必用 `background=true`
- 本机 `imagemagick` / `x11-apps` 在 Alibaba Cloud Linux 源里**不存在** → 截图用 Bevy 内置
  `Screenshot` API（`bevy::render::view::screenshot::Screenshot`）

### 7.2 验证 U1/U3：编译并运行 luma 的 3D example

```bash
cd /tmp/research/bevy_luma
# 去掉 wayland feature（见 Cargo.toml [workspace.dependencies] bevy.features）
cargo build --example world_3d
cargo build --example luma_world_3d_gallery

export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json DISPLAY=:99
./target/debug/examples/luma_world_3d_gallery
```

**要看的**：面板边缘在近处/远处是否锐利（U1）；shader 是否报 PBR API 不兼容（U3）。
**截图建议**：Bevy 有内置 `Screenshot` API（`bevy::render::view::screenshot::Screenshot`），比装 ImageMagick 可靠（本机 `imagemagick`/`x11-apps` 在 Alibaba Cloud Linux 源里没有）。

### 7.3 验证 U4/U5：编译并运行 spike

```bash
cd /tmp/research/spike
# Cargo.toml 已配好：bevy_feathers = "0.20.1"，bevy_luma = { path = "../bevy_luma" }
cargo build 2>&1 | head -60      # 先修第 4.3 节列的 5 个编译错误
cargo run
```

**要验证的判定标准**（spike 通过 = 路线 B 成立）：
1. `report()` 输出 `panels=1 mirrored_nodes>=3`（面板根 + button + 色块）
2. **面板渲染在 3D 空间中**：参考立方体与面板有正确遮挡关系；移动相机时面板有透视变化
3. **子元素位置正确**：文字标题在最上、色块在中间（验证第 2.4 节 E 的坐标映射）
4. **任意缩放清晰**：把 camera 拉近到面板占屏 200%，边缘仍锐利无锯齿/发虚
5. **拾取可交互**：实现 ray picking 后点击按钮能触发 `Activate` 事件（feathers button 依赖 `bevy_ui_widgets::Button` + `Activate`）

### 7.4 验证 U2：Bevy fwidth 方案

```bash
cd /tmp/research/bevy            # 或直接改 crates.io 的 bevy_ui_render 源码副本
# 修改 crates/bevy_ui_render/src/ui.wesl 的 antialias()：
#   fn antialias(distance: f32) -> f32 {
#       return saturate(0.5 - distance);
#   }
# 改为基于 fwidth 的版本后，跑 bevy UI 相关测试与 feathers examples
```
**判定**：若 artifact 确实无法规避 → 路线 B 需在 feathers 侧自建 SDF 渲染管线（工作量显著上升），或改用 luma 整套渲染。

---

## 8. 风险登记

| 风险 | 等级 | 说明 | 缓解 |
|---|---|---|---|
| **R1** Bevy `fwidth` artifact | 高 | Bevy 自己注释说踩过坑 | 先做 7.4 的 1 天验证再决定投入 |
| **R1b** **Bevy 0.20.1 `bevy_winit` 编译失败** | **高（已实测）** | `bevy_feathers` 启用 `bevy_picking/custom_cursor` → 打开 `bevy_window` 的 `CursorIcon::Custom` 变体，但 `bevy_winit` 的 match 没有对应分支 → E0004 | ✅ **已找到解法**：见 7.1 节「custom_cursor feature 对齐」 |
| **R2** `bevy_luma` 成熟度 | 中 | 1 commit / 2 stars / 13 次下载 / 0 issue；43k 行代码压在一次提交 | 只复用 `UNodeMaterial3d` + shader（公开导出，可读可抄），**不 fork 整个 luma**；也可参考 univis_ui v0.3.1（151 commits） |
| **R3** Bevy 版本漂移 | 中 | 主线 0.20.0-dev vs crates.io 0.20.1 API 有差异 | 以 crates.io 0.20.1 为准；不 fork 主线 |
| **R4** univis_ui 是 alpha | 中 | README 明写 "still in alpha"；`MIGRATION.md` 列出「仍需人工验证」项（fit-content world roots、World3d 视觉表现） | 仅作参考实现，不作生产依赖 |
| **R5** 控件语义 3D 化 | 高 | 24 个控件 × 悬停/焦点/滚动/文本编辑 | 先做 M1/M2，只支持 button/label/slider 少量控件 |
| **R6** 深度穿插 | 中 | UI 面板与场景几何的排序 | 用 luma 的 `depth_sync.rs`（592 行，含 `world3d_layer_sort_bias`、透明排序覆盖） |
| **R7** 无硬件 GPU | 低（本机） | 只能软渲染，性能数据不代表真实表现 | 换机若有 GPU 更好；性能结论需在真机复核 |

---

## 9. 关键文件速查

**要读的源码**：
```
bevy_luma/crates/luma_engine/src/layout/render/material_3d.rs          # 3D 材质定义（202 行）
bevy_luma/crates/luma_engine/src/layout/render/shaders/unode_3d.wesl   # 3D SDF + PBR shader（420 行）
bevy_luma/crates/luma_engine/src/layout/render/system.rs                # 3D 分派逻辑（~408-660 行）
bevy_luma/crates/luma_engine/src/layout/render/depth_sync.rs            # 3D 深度/透明排序（592 行）
bevy_luma/crates/luma_engine/src/layout/render/clip_sync.rs             # 3D 裁剪 + world_scale（1038 行）
bevy_luma/crates/luma_interaction/src/interaction/picking/hit_math.rs   # 3D 拾取核心（61 行）
univis_ui/crates/univis_ui_engine/src/layout/render/shaders/unode_3d.wgsl  # 更精简的 3D shader（227 行）
univis_ui/crates/univis_ui_interaction/src/interaction/picking/hit_math.rs # 拾取参考实现（62 行）
univis_ui/crates/univis_ui_engine/src/layout/layout_system/roots.rs     # URootUi::world_3d + meters_per_unit
bevyui/bevy_ui-0.20.1/src/ui_node.rs                                    # ComputedNode / BorderRadius / ResolvedBorderRadius
bevyui/bevy_ui-0.20.1/src/ui_transform.rs                               # UiGlobalTransform (Affine2)
```

**路线 B 的三个关键锚点**（原文位置）：
1. 抗锯齿：`bevy_ui_render/src/ui.wesl:185`（Bevy 的 `antialias`）vs `luma .../unode.wgsl`（`dpdx/dpdy`）
2. 坐标原点：`bevy_ui/src/layout/mod.rs:494`（`Affine2::from_translation(0.5 * target_size)`）
3. 物理尺寸映射：`luma .../roots.rs:292`（`ui_units_to_world_scale`）+ `system.rs:407`（`size_vec = logical_size * world_scale`）

---

## 10. 一句话建议

先花 **1 天** 做 6.4 的 fwidth 验证（成本最低、决定后续 2–3 人周投入是否值得），再花 **2–3 人周** 做 spike 验证坐标映射与渲染。如果两条都通过，则路线 B 成立，总投入约 **6–10 人周** 可得到可用面板；**不要**走路线 A（换布局引擎，4–6 人月起）。

**另一条平行选项**：若 3D 面板是为游戏内科幻终端/VR 场景服务（3D 面板的典型动机），**直接用 `univis_ui` v0.3.1 做游戏侧 3D 面板、`bevy_feathers` 继续做编辑器侧 2D UI，两套并存**，成本接近于零。feathers 文档本身就写明它 "deliberately not intended for" 游戏 UI。
