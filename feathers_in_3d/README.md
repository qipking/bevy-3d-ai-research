# feathers_in_3d

把一棵 **`bevy_ui` / `bevy_feathers` 的 UI 子树**渲染成 **世界空间里的 3D 面板**（路线 B）：
布局仍然由 `bevy_ui` 算，几何/文字/排序/拾取换到 3D 里做。

```
feathers/bevy_ui 布局（屏幕像素，可以放在视口外）
        │  ComputedNode + UiGlobalTransform
        ▼
feathers_in_3d
  ├─ world3d  每节点 → 3D 四边面 + bevy_luma SDF 材质（圆角/边框/AA/裁剪）
  ├─ text3d   bevy_text 的排版结果 → 字形四边形（每面板每图集 1 个 mesh）
  ├─ sort     透明队列排序覆写（面板内按 UI 画笔顺序）
  └─ pick     射线 → 面板平面 → 节点矩形 → bevy_picking 事件
```

能力与实测结果见 `../docs/design/RIG_NIRI_MOBILE_PLAN.md`。

---

## 快速上手

```toml
[dependencies]
feathers_in_3d = { path = "../feathers_in_3d" }
bevy = "=0.20.1"
bevy_feathers = "=0.20.1"
```

```rust
use feathers_in_3d::{FeathersIn3dPlugin, MirrorRoot, WorldPanel};

App::new()
    .add_plugins(DefaultPlugins)
    .add_plugins(bevy_feathers::FeathersPlugins)
    .add_plugins(FeathersIn3dPlugin)   // 内含 luma 的 UnivisRenderPlugin
    .add_systems(Startup, setup);
```

面板 = 一个普通的 UI 子树根节点 + 两个组件：

```rust
commands.spawn((
    Node {
        // 关键：把布局挪出视口，否则 bevy_ui 的 2D 渲染会盖在 3D 面板上
        position_type: PositionType::Absolute,
        left: px(-20000.0),
        width: px(780.0),
        height: px(380.0),
        flex_direction: FlexDirection::Column,
        overflow: Overflow::scroll_y(),      // 滚动容器（可选）
        ..default()
    },
    BackgroundColor(Color::srgb(0.045, 0.055, 0.08)),
    WorldPanel { units_to_world: 0.0018, anchor: Transform::default() },
    MirrorRoot,
    ScrollPosition::default(),               // 有 overflow 时配这个
))
.with_children(|panel| { /* feathers / bevy_ui 节点 */ });
```

`anchor` 就是面板在世界里的位置/朝向；`units_to_world` 是「1 逻辑像素 = 多少米」。

---

## 坐标映射（route B 的核心）

| 2D（bevy_ui） | 3D |
|---|---|
| `ComputedNode.size`（物理像素） | `size * units_to_world` |
| `UiGlobalTransform`（中心相对父中心的偏移，Y 向下） | 逐跳累加，`x * u`、`-y * u`（Y 向上） |
| 节点有 `BackgroundColor`/边框 | 镜像成 mesh + `UNodeMaterial3d` |
| 节点是 `Text`/`TextSpan` | 由 `TextLayoutInfo` 的字形生成四边形 |
| 绘制顺序 = 树的前序 | `PanelSort` 的 `bias`（DFS 前序 × 0.5 mm） |

> 之前踩过的坑：父节点没有填充时也必须**落位**（给它 `Transform`），否则它的文字子节点会全部塌到面板中心。

---

## 各模块要点

### `world3d` — 几何与材质

- `UNodeMaterial3d` 是 bevy_luma 公开导出的 SDF 3D 材质：圆角、边框、`dpdx/dpdy` 屏幕空间导数 AA；
- 面板边界裁剪用材质自带的 4 层 clip uniform：填**面板世界→局部矩阵的行** + `half * units`。
  ⚠️ `clip_bounds` 的单位是**米**（shader 在局部世界空间求 SDF），不是 UI 像素；
- 无填充的节点不建 mesh，但一定落位（见上）。

### `text3d` — 3D 文字

- 数据来源：`bevy_text` 已经完成整形/换行，`TextLayoutInfo.glyphs` 给出每个字形的
  `position` 与 `atlas_info { texture, rect }`；`section_index` 指向 `ComputedTextBlock` 的
  section 实体，用来取该 span 的 `TextColor`（富文本逐字着色）；
- 一个面板汇总所有 Text 的字形，**按图集分组，每组 1 个 mesh**（实测 11 行 584 字形 → 2 draw call）；
  中英混排会落进 2 个图集（FiraMono + Noto CJK），这也是必须按图集分组的原因；
- 材质：`StandardMaterial { unlit, alpha_mode: Blend, base_color_texture: 图集 }`，
  颜色走顶点色（`ATTRIBUTE_COLOR`）；
- 重建由 `Changed<TextLayoutInfo>` + 面板尺寸/偏移签名触发；字形按面板框 CPU 剔除
  （`StandardMaterial` 没有 clip uniform，只能剔除；边界行会有 2 px 余量）。

### `sort` — 画对顺序（照抄 luma 的做法）

面板内所有节点共面，所以顺序必须由渲染排序键给：

```rust
item.sorting_info = TransparentSortingInfo3d::Sorted {
    mesh_center: <面板中心>,   // 同面板同中心 → 排序键只剩 bias
    depth_bias: <DFS 前序 bias>, // 越大越后画（Transparent3d 按 distance+bias 升序）
};
```

在 `RenderSystems::PhaseSort` 里、`sort_phase_system::<Transparent3d>` 之前覆写。
参考：`bevy_luma/crates/luma_engine/src/layout/render/depth_sync.rs`
（`WORLD3D_SORT_BIAS_BAND = 0.02`、`apply_world3d_ui_transparent_sort_overrides`）。

### `pick` — 射线到控件

和 luma 的 `hit_math.rs` 同三步：射线 × 面板平面 → 命中点转到面板局部 → 命中节点矩形。
不实现自己的 picking backend 接口，而是发 `bevy_picking` 的 `PointerHits`，
这样 `bevy_ui` 控件、`PointerClick` observer 都不用改。

- 只有**显式带 `Pickable`** 的节点参与拾取（否则面板背景会抢行的点击）；
- 共面节点深度相同，按 `bias` 破平（后画的优先），保证子节点赢过父背景；
- 用法：给可交互的行/卡片加 `Pickable::default()`，然后 `app.add_observer(|click: On<PointerClick>| ...)`。

---

## 运行示例（本仓库的 `rig_niri`）

```sh
# 1) CJK 字体：Bevy 只认 .ttf/.otf，系统里是 .ttc，需要抽一个 face
./tools/fetch_cjk_font.sh

cd rig_niri
CARGO_HOME=../.cargo-home cargo build
BEVY_ASSET_ROOT=$PWD RF_SHOT_DIR=../render/rf_shots ./target/debug/rig_niri
```

产出：`01_overview` / `02_near` / `03a_mild_oblique` / `03b_oblique45` / `04_text_zoom`
/ `05_scrolled`（滚动 + 裁剪）/ `06_selected`（合成指针点击选中）。

> 开发期一定要给 `BEVY_ASSET_ROOT`，否则 Bevy 会去 `target/debug/assets` 找字体。

---

## 已知限制 / 待办

| 项 | 说明 |
|---|---|
| 文字是位图图集 | 面板被放大到 ~6× 以上会糊（`04_text_zoom`）。要任意距离都清晰需要 SDF/MSDF 文本 |
| 裁剪只到面板边界 | 嵌套裁剪（面板里的滚动区再裁剪）未做；转写目前是"面板即视口" |
| 字形剔除不是真裁剪 | 边缘的字形是被剔除而不是被切一半，滚动到一半的行可能整行消失 |
| 选择是行级 | 字符级选择/拖拽未做；`Ctrl+C` 走 `bevy_clipboard`，需要开 `system_clipboard` feature 才写系统剪贴板 |
| 暂无 rig-tui 的 `RenderBatch` 接入 | 现在由应用层把 `LookLine::colored()` 转成 feathers 节点；直接吃 `RenderBatch` 需要构造 `ToolCall`
（`CallId`/`ToolName`）并驱动 `TranscriptRender` 调度 |
| 每个字形网格是独立 draw call | 每面板每图集 1 次已够用；面板很多时可再跨面板合批 |
| 与不透明场景几何的遮挡 | ✅ 已定位：**不是本 crate 的问题**。是应用层的 `PointerOut` 冒泡（见 `../occlusion_report.md`）把面板根的 `BackgroundColor` 写成 alpha=0，SDF body 全部 discard，背后的立方体才"透"出来。修复在 `rig_niri` |

## 性能（llvmpipe 上无参考价值）

软件渲染下仍可跑完整巡游；真实性能需要 GPU 机器。设计上每面板的 draw call =
填充节点数 + 图集数，节点数上百时值得进一步合批。
