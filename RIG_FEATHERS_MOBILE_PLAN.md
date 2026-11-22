# rig-tui × feathers × 移动端：改造方案（无 VR）

> 范围收敛：**feathers（3D 空间面板）+ rig-tui 内核 + feathers UI 适配 + 移动端陀螺仪相机**。不做 VR。
> 业务：大模型导购助手，6 个业务工具，支付前闸门。
> 日期：2026-11-23　｜　源码证据：`rig-tui-inspect/`（rig-tui / rig-ecs / rig-harness / rig-core / rig-tools 0.47 源码）

---

## 0. 先核对你的三条判断（全部成立，附源码锚点）

| 你的判断 | 源码证据 | 结论 |
|---|---|---|
| 工具用 `add_tool` 注册，HTTP 在任务池跑 | `rig-ecs/src/tools.rs:271` `AppToolsExt::add_tool`；文档原文 *"Tool futures run on Bevy's async compute pool"* | ✅ 成立 |
| 支付闸门在 Gate 阶段拦调用，rig-ecs 现成 | `rig-ecs/src/stage.rs:119` `Stage::Gate`；`Decision::decide`（`stage.rs:413`）；`rig-harness/src/ask.rs` 的 `Ask`/`Answer`/`Answerer`/`HoldOf` | ✅ 成立，而且是**官方推荐姿势**（`ask.rs` 模块文档直接给了这个用法） |
| 工具卡片用 `RenderSystems::Render` 注册 look | `rig-tui/src/renderers.rs:644` `TranscriptRender` 调度；`:649` `RenderSystems::{Render,Decorate,Edit}`；`:776` `RenderBatch::tools_mut(name)` → `(&ToolRender, &mut RenderOutput)`；`RenderOutput.lines: Vec<LookLine>` | ✅ 成立 |

**两个必须先知道的事实**（会影响动手方式）：

1. **`renderers::add` 是 `pub(crate)`**（`renderers.rs:835`），`mod renderers` 也是私有的 → **外部 crate 不能只依赖 rig-tui 就复用 look 机制**，必须 **fork rig-tui**（MIT 许可，允许）或者 vendor 这几个模块。
2. **`LookLine` 里面是 ratatui 的 `Line<'static>`**（`renderers.rs:496`，字段 `pub(crate)`）→ "行"的样式仍是 ratatui 的 `Style/Span/Color`。所以 feathers 适配层要做一层 **`Line → bevy_ui 文本 span`** 的转换（约 100–150 行），这不是坏事：look 的写法在终端和 feathers 两端完全一致。

---

## 1. 总体结构

```
┌──────────────────────────────────────────────────────────────┐
│ rig-feathers（fork 自 rig-tui 0.47，MIT）                       │
│                                                              │
│  保留：renderers / view / pane / transcript / editor / keys    │
│        input / pick / status / history / complete             │
│  替换：render.rs（终端 Buffer 布局）→ feathers 布局             │
│        panel.rs（PanelCanvas）     → feathers 面板             │
│        screen/terminal.rs          → 删除（窗口替代）           │
│        theme.rs（ratatui 颜色）    → feathers tokens           │
│  新增：feathers_draw（LookLine → 节点树）                       │
│        world3d（路线 B：面板 → 3D 镜面，已实测）                 │
│        gyro（移动端相机）                                       │
│                                                              │
│  内核不动：rig-ecs 0.47（agent 回合/工具/Gate/会话）            │
│  业务新增：6 个工具 + 1 个 Gate + 3 个 look                     │
└──────────────────────────────────────────────────────────────┘
```

**关键点：内核一行不用改。** 你要加的东西只有 `add_tool` / `Stage::Gate` / `RenderSystems::Render` 三处，都是"插件式"接入点。

---

## 2. 工具层：6 个业务工具

### 2.1 接口形状（rig-core 0.47 的 `Tool` trait）

```rust
// rig-core/src/tool/contextual.rs:32
pub trait Tool: Sized + Send + Sync {
    const NAME: &'static str;
    type Args: DeserializeOwned + Send + Sync;
    type Output: IntoToolOutput;          // String / serde 值都行
    type Error: std::error::Error + Send + Sync + 'static;

    fn description(&self) -> String;
    fn parameters(&self) -> serde_json::Value;   // JSON Schema
    fn annotations(&self) -> ToolAnnotations { .. }  // 只读/写，决定并发
    fn call(&self, ctx: &mut ToolContext, args: Self::Args)
        -> impl Future<Output = Result<Self::Output, Self::Error>> + Send;
}
```

注册（`rig-ecs/src/tools.rs:271`）：

```rust
app.add_tool(SearchProducts { http: client.clone() })
   .add_tool(GetProduct { .. })
   .add_tool(Compare { .. })
   .add_tool(CartAdd { .. })
   .add_tool(CreateOrder { .. })      // ← 受闸门保护
   .add_tool(CheckPayment { .. });
```

### 2.2 六个工具的参数与返回

| 工具 | Args | Output（给模型看的） | annotations | 备注 |
|---|---|---|---|---|
| `search_products` | `query, category?, budget_max?, limit?` | 商品列表（id/名称/价格/店铺/评分） | `read_only` | 并发跑 |
| `get_product` | `product_id` | 详情 + SKU/库存/规格 | `read_only` | 并发跑 |
| `compare` | `product_ids: Vec<String>, aspects?` | 对比表（价格/参数/店铺/运费） | `read_only` | 并发跑 |
| `cart_add` | `product_id, qty` | 购物车行 + 小计 + 合计 | 写（串行） | 幂等键防重 |
| `create_order` | `cart_id, address_id, coupon?` | 订单草稿（**商品/单价/数量/合计/店铺**） | 写 + **需审批** | **闸门在这里拦** |
| `check_payment` | `order_id` | 支付状态 + 二维码 payload | `read_only` | 轮询用 |

### 2.3 HTTP 怎么跑在任务池里

源码已确认：`add_tool` 的 future 在 **Bevy 的 AsyncComputeTaskPool** 上跑（`tools.rs` 文档）。所以：

```rust
impl Tool for SearchProducts {
    const NAME: &'static str = "search_products";
    type Args = SearchArgs;
    type Output = String;                 // 或 serde_json::Value
    type Error = ToolHttpError;

    fn description(&self) -> String {
        "Search the catalog. Use it before compare/cart_add.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        schemars::schema_for!(SearchArgs).into()      // rig-ecs 已依赖 schemars 1
    }
    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations::default().read_only(true)    // 只读 → 同回复内可并发
    }
    async fn call(&self, _ctx: &mut ToolContext, args: Self::Args)
        -> Result<String, ToolHttpError>
    {
        // 阻塞型 HTTP 必须离开异步线程：
        let url = format!("{}/products?q={}", self.base, args.query);
        let body = rig_tools::blocking(move || ureq::get(&url).call()?.into_string())
            .await?;                                   // rig-tools/src/blocking.rs:15
        Ok(body)
    }
}
```

> 为什么不用 `reqwest::Client`（async）：它需要 tokio reactor，而 rig 的 future 跑在 bevy 的任务池里（没有 tokio runtime）。两条正路：
> **① `rig_tools::blocking(...)` + 阻塞 HTTP（ureq/reqwest::blocking）** ← 推荐，零 runtime；
> ② 自建 `tokio::runtime` 句柄 + `oneshot` 通道桥接。
>
> 别在 `call` 里直接 `block_on` —— 会把任务池线程卡住（池是共享的）。

### 2.4 错误与重试

- `map_error` 默认已把 `Error` 归类；业务错误（商品下架/超出预算）建议返回 **结构化 JSON** 而不是 `Err`，让模型能自我纠正（例如 `{"ok":false,"reason":"out_of_stock","alternatives":[...]}`）。
- 只有网络/鉴权失败才 `Err`，交由 rig 的重试/上报。

---

## 3. 支付闸门（Gate）

### 3.1 机制（rig 自带，不需要新造）

`rig-harness/src/ask.rs` 模块文档给出的就是这条链：

```
工具调用 effect  ──Admit──►  Stage::Gate
                              │  你的 gate 系统看到 ReadyToStart 且未 decided 的 create_order
                              │  ① 生成 Ask（商品/单价/数量/合计/店铺）
                              │  ② spawn(Ask) + HoldOf(effect)   ← 把 effect 按住
                              ▼
                     前端渲染 Answerer（feathers 模态面板）
                              │  用户点"确认支付"
                              ▼
                     Answer::choice("confirm").outcome() → Decision::decide(Decider::Person, …)
                              ▼
                     Stage::Gate 看完 → 进 Start → 工具真正执行
```

拒绝路径：`Outcome::refused("用户取消支付")` → 模型收到的是"工具被拒绝"，会改口术/换方案。

### 3.2 Gate 系统骨架

```rust
// 与 rig-basics 的 protected-paths 同一姿势
fn gate_payments(
    mut effects: Query<(&EffectRequest, &mut Decision, Entity),
                       (ReadyToStart, PluginEffects)>,
    answerers: Answerers,
    mut commands: Commands,
) {
    for (request, mut decision, effect) in &mut effects {
        if decision.is_decided() { continue; }               // 别的 gate 先决定了
        let name = request.kind.name();
        if !matches!(name, "create_order" | "pay") { continue; }
        if !answerers.can_answer(None) {                     // 没有前端能问 → 保守拒绝
            decision.decide(Decider::plugin("pay-gate"),
                            Outcome::refused("no front to confirm payment"));
            continue;
        }
        // 把订单草稿变成人话；商品/单价/数量/合计/店铺 ← 来自 create_order 的 args
        let ask = Ask::yes_no("确认下单并支付？")
            .body(order_summary(name, request))
            .choice("confirm", "确认支付")
            .choice("cancel", "取消");
        commands.spawn((ask, HoldOf(effect)));               // 按住 effect，等回答
    }
}
// 注册
app.add_systems(AgentPass, gate_payments.in_set(Stage::Gate));
```

要点：
- **`Decision` 先到先得**（`stage.rs:415`），所以多个 gate 可以并存而不打架；
- `Ask` 是 **open effect**，会被记录进会话（可审计：谁在何时批准了哪笔订单，`EffectRecord::decided_by = Person`）；
- 用 `HoldOf` 而不是忙等 —— 停止/取消回合时，hold 会连带关闭问题（`ask.rs` 文档明确）。

### 3.3 前端（feathers）侧

```rust
// 每帧把未回答的 Ask 渲染成模态面板
fn draw_asks(asks: Query<(Entity, &Ask), Without<Outcome>>, mut panels: Query<&mut AskPanel>) {
    for (entity, ask) in &asks {
        // ask.title / ask.body / ask.choices → 商品卡 + 单价 + 数量 + 合计 + 店铺
        // 按钮：确认 / 取消
    }
}
// 点击 → 回答
fn answer(choice: &str, ask: Entity, mut decisions: Query<&mut Decision>) {
    let answer = Answer::choice(choice);
    if let Ok(mut d) = decisions.get_mut(ask) {
        d.decide(Decider::Person, answer.outcome());
    }
}
```

> `Decider::Person` 是给"人"的：会话记录里能区分"插件策略拒绝"和"用户拒绝"。

---

## 4. 展示层：三个 look

### 4.1 注册姿势（与 `render_build_failed` 同款）

```rust
app.add_systems(TranscriptRender, product_card.in_set(RenderSystems::Render))
   .add_systems(TranscriptRender, order_status.in_set(RenderSystems::Render))
   .add_systems(TranscriptRender, payment_qr .in_set(RenderSystems::Render));

fn product_card(mut batch: ResMut<RenderBatch>) {
    for (call, out) in batch.tools_mut("search_products") {
        // call: &ToolRender { call, result, state, theme, facts, took, output, .. }
        out.lines = product_lines(call);   // Vec<LookLine>，LookLine::from(ratatui Line)
    }
}
```

`ToolRender` 提供的东西足够做富卡片：`call.call.function.arguments`（流式中）、`result`、`state: CallState`、`output`（工具边跑边打的字）、`took`（耗时）、`facts`（结构化发布值，比如订单号）。

三个 look 的建议：

| look | 输入 | 输出行（示例） |
|---|---|---|
| 商品卡 | `search_products` 的 result JSON | 标题行 + 价格/店铺/评分 + 3–5 条卖点（等宽对齐） |
| 订单状态 | `create_order` / `check_payment` | 状态徽标（草稿/待付/已付/超时）+ 合计 + 店铺 + 单号 |
| 二维码 | `check_payment` 的 qr payload | ASCII/半块字符二维码（`qrcode` crate → 矩阵 → `▀▄█ `），等宽字体两端都能显示 |

### 4.2 feathers 适配（新增模块）

```rust
// LookLine → bevy_ui：一行一个 Node（Text），span 逐个转 TextSpan
fn line_to_spans(line: &ratatui::text::Line) -> Vec<(String, TextFont, TextColor)> {
    line.spans.iter().map(|s| (
        s.content.to_string(),
        TextFont { font: fonts::MONO.into(), font_size: FontSize::Px(14.0), ..default() },
        TextColor(ratatui_color_to_bevy(s.style.fg)),
    )).collect()
}
```

- 每行一个 `Node`，行内 `TextSpan`s → **保留 rig 的 look 语义，不重写业务格式化**；
- 代码块/二维码用 `MONO`（feathers 自带 FiraMono）；
- 面板本身走**路线 B**（已实测）：`ComputedNode + UiGlobalTransform → Mesh3d + UNodeMaterial3d`，`units_to_world` 每面板可调。

---

## 5. 移动端：陀螺仪控制相机

### 5.1 现成 crate 不可用（要自己写薄封装）

| 方案 | 状态 |
|---|---|
| `bevy_gyroscope 0.1.0` | ❌ 钉 `bevy ^0.14`，且只有 iOS（objc2-core-motion） |
| Bevy 内置 | ❌ 没有惯性传感器 API |
| **Android** | ✅ 用 `ndk::sensor::SensorManager`（`ndk` crate 的 `sensor` feature）或 JNI 直调 `SensorManager`；配 `android-activity`（Bevy 已在用） |
| **iOS** | ✅ `objc2-core-motion`（CMMotionManager）——和 bevy_gyroscope 同款，自己接 |
| **Web/wasm** | ✅ `web-sys` 的 `DeviceOrientationEvent`（最省事的验证路径） |
| 桌面 | 鼠标拖拽 + 键盘（调试用） |

### 5.2 设计（抽象成一个资源，三端一个接口）

```rust
#[derive(Resource, Default)]
pub struct DeviceOrientation {
    pub quat: Option<Quat>,      // 设备姿态（世界系）
    pub available: bool,
}

// 相机系统：设备姿态 → 相机朝向（相对"归零"姿态 + 平滑 + 死区）
fn gyro_camera(
    time: Res<Time>,
    orient: Res<DeviceOrientation>,
    mut cam: Single<&mut Transform, With<Camera3d>>,
    mut zero: Local<Option<Quat>>,
) {
    let Some(raw) = orient.quat else { return };
    let base = *zero.get_or_insert_with(|| raw);           // 启动时归零，用户朝哪看就以哪为前
    let rel = (raw * base.inverse()).to_euler(/*YXZ*/);
    let target_yaw = -rel.y;                                // 设备水平转动 → 相机 yaw
    let target_pitch = -rel.x.clamp(-0.5, 0.5);             // 限幅 ±28°，防眩晕
    let k = (time.delta_secs() * 12.0).min(1.0);            // 低通，抑制抖动
    cam.rotation = cam.rotation.slerp(
        Quat::from_euler(EulerRot::YXZ, target_yaw, target_pitch, 0.0), k);
}
```

细节（都是产品体验的关键）：
- **归零/复位**：双击屏幕或长按 → `zero = raw`；
- **死区**：`|Δ| < 0.01 rad` 不更新，避免静息漂移；
- **只映射 yaw/pitch**，不映射 roll（roll 会让人晕、也会让面板倾斜）；
- **不移动相机位置**（用户说"调用陀螺仪控制相机位置"——实际只需要朝向；真要位移用摇杆/拖拽，否则走动感会晕）；
- **轨迹球兜底**：手指拖拽 = 增量 yaw/pitch，陀螺仪不可用时自动启用。

### 5.3 构建

- Android：`cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build --release` + `android-activity` 的 `NativeActivity`；传感器需要 `android.permission.*` 无需（惯性传感器免权限）。
- iOS：`cargo lipo`/Xcode 工程 + `CMMotionManager`；需要 `NSMotionUsageDescription`（iOS 11+ 访问运动数据）。
- 性能：移动端 60 fps 目标；3D 面板合批（每面板 1 mesh + 1 material），2D 覆盖层关掉（路线 B 的做法）。

---

## 6. 改造清单（文件级）

| 文件（rig-tui） | 行数 | 处理 |
|---|---|---|
| `renderers.rs` | 1068 | **保留**（look 机制 + `LookLine`），新增 3 个业务 look |
| `view.rs` / `pane.rs` / `transcript.rs` | 865/392/1783 | **保留**（视图模型/滚动/转写状态） |
| `editor.rs` / `keys.rs` / `input.rs` / `complete.rs` | 770/751/1026/681 | **保留**（输入状态机；键位映射换成 bevy `KeyCode`） |
| `pick.rs` / `status.rs` / `history.rs` | 1098/605/223 | **保留**（选择器/状态行；PickItem 直接复用） |
| `render.rs`（终端 Buffer 布局） | 1543 | **替换** → feathers 布局（`LookLine` 行 → Node） |
| `panel.rs`（PanelCanvas） | 344 | **替换** → feathers 面板容器 |
| `theme.rs`（ratatui 颜色） | 337 | **替换** → feathers tokens + bevy Color |
| `terminal.rs` / `screen.rs` / `clipboard.rs` | 620/46/340 | **替换**（窗口/clipboard 用 bevy/arboard） |
| `lib.rs`（插件装配） | 422 | **改**：删 `FRONT="tui"` 的终端装配，加 feathers/world3d/gyro 插件 |
| 新增 `tools/business.rs` | ~600 | 6 个工具 |
| 新增 `gate/payment.rs` | ~150 | Gate 系统 |
| 新增 `looks/{product,order,qr}.rs` | ~300 | 3 个 look |
| 新增 `feathers_draw.rs` | ~400 | LookLine → 节点树、面板容器 |
| 新增 `world3d.rs` | ~300 | 路线 B 镜面（直接搬 `spike/src/main.rs` 的 sync_mirror） |
| 新增 `gyro.rs` | ~200 | 三端传感器 + 相机 |

**合计**：替换 ~2900 行、新增 ~1950 行、保留 ~7000 行。

---

## 7. 里程碑

| 阶段 | 内容 | 验收 | 估 |
|---|---|---|---|
| **M0** | fork rig-tui → 去掉终端、上窗口；`LookLine → feathers` 渲染一条 mock 会话；面板走 3D 镜面 | 窗口里能滚动一段带颜色/等宽的转写 | **1 周** |
| **M1** | 6 个工具（接真后端）+ 3 个 look + 输入框 | 说一句"帮我找 500 内的跑鞋"，出商品卡，点选加入购物车 | **2 周** |
| **M2** | 支付闸门（Ask + HoldOf + 模态）+ 订单/二维码 | 下单前必弹确认，拒绝后模型改口术 | **1 周** |
| **M3** | 移动端：Android 传感器 + 陀螺仪相机 + 触屏兜底 | 手机上转头看面板、点按钮、下单全链路 | **2 周** |

---

## 8. 风险

| # | 风险 | 缓解 |
|---|---|---|
| R1 | **fork** rig-tui 的维护成本（上游 0.47 迭代快） | 只 fork 一次，业务代码全在插件层；用 `[patch]` 或 vendor 目录，升级时 diff |
| R2 | `LookLine` 依赖 ratatui `Line`（样式语义是终端的：16 色/修饰符） | 转换层把 `ratatui::Style` 映射到 bevy Color；真彩色直接用 `Color::Rgb` |
| R3 | HTTP 在任务池里的阻塞 | 统一用 `rig_tools::blocking()` 包住阻塞 IO；禁止 `block_on` |
| R4 | 网关/鉴权 | 工具内做重试 + 幂等键（`cart_add`/`create_order` 必须带 `idempotency_key`） |
| R5 | 闸门被绕过（模型直接调 `pay`） | Gate 按**名字前缀**拦（`create_order`/`pay*`），并对 `cart_add` 之外的所有写工具都要求审批 |
| R6 | 移动端性能 | 面板合批；限制面板数；关闭 bevy_ui 的 2D 覆盖层；纹理/字体图集控制 |
| R7 | 陀螺仪漂移/晕动 | yaw/pitch only + 死区 + 低通 + 归零 + 触屏兜底 |
| R8 | 条码/二维码在等宽字体里失真 | 用半块字符（`▀▄█`）而不是彩色方块；二维码尺寸取奇数倍 |

---

## 9. 本机能验证什么

| 项 | 本机 | 说明 |
|---|---|---|
| fork 后的编译 | ✅ | bevy 0.20.1 工具链已就绪（`spike/` 已编译过整棵树，增量很快） |
| `LookLine → feathers` 渲染 | ✅ | llvmpipe 出图即可 |
| 6 个工具 + Gate（**不调真模型**） | ✅ | 直接 spawn 一个 `create_order` 的 ToolCall effect，观察 Gate 是否弹 Ask、回答后是否执行 |
| 后端 HTTP | ✅（本地 mock） | 起一个 `python -m http.server` 或 axum mock |
| 陀螺仪 | ⚠️ | 桌面无传感器，但可用"鼠标拖拽"跑同一套相机逻辑；Android 构建需要 NDK/SDK（本机未装） |
| 真机 | ❌ | 需要一台 Android 手机 |

---

## 10. 建议的下一步（按依赖顺序）

1. **fork + 去终端**（M0 的前半）：`rig-feathers` crate，删 `terminal/screen`，把 `TuiPlugin` 的终端装配换成 `DefaultPlugins`。
2. **`LookLine → feathers` 画一条 mock 会话**：先不做工具，验证文本/颜色/等宽/滚动。
3. **接 6 个工具（mock 后端）+ 3 个 look**：不调模型，用手工构造的 `ToolCall` 事件驱动。
4. **Gate + Ask 模态**：同上，手工构造 `create_order` effect，验证按住→确认→执行。
5. **接真模型** + **移动端**.

> 第 3、4 步可以在**没有 API key** 的情况下完整验证——这正好是本机现在能做的部分。


---

# 实施进展（2026-11-23）

## A. `rig-feathers` 依赖树已验证（重要修正）

我建了 `research/rig-feathers/` 骨架并**编译通过**（2m52s）：

```toml
bevy = "=0.20.1"          # ← 不是 0.20.0
bevy_feathers = "=0.20.1"
bevy_ui/bevy_ui_render/bevy_ui_widgets/bevy_text = "=0.20.1"
bevy_window/bevy_winit = { version = "=0.20.1", features = ["custom_cursor"] }
rig-tui = "=0.47.0"
bevy_luma = { path = "../bevy_luma" }
```

**结论**：`rig-harness 0.47` 依赖 `bevy = "=0.20.1"`（精确钉），因此整个应用必须 **Bevy 0.20.1**。
好消息：feathers 有 0.20.1、luma 的 `bevy ^0.20.0` 也兼容 0.20.1 —— **整套栈在 0.20.1 对齐**，
`spike/` 里的 0.20.0 钉法对本项目不再是正确选择（那是为了对齐 luma 的旧假设）。

> 之前文档说"M0 走 0.20.0"，这里修正为 **0.20.1**。

## B. 3D 文本桥已跑通（`spike/src/main.rs`，约 190 行）

**做法**：不写自定义 shader，不重新整形。

1. `bevy_text` 已经给出每个字形的**位置**与**图集矩形**：
   `TextLayoutInfo.glyphs[i] = { position: Vec2, atlas_info: { texture: AssetId<Image>, rect: Rect, is_alpha_mask } , section_index }`
2. 每个字形 → 两个三角形（面板局部坐标，`content_box().min + position`，Y 翻转，乘 `units_to_world`）；
   UV 由 `rect / image.size()`；颜色写到 `ATTRIBUTE_COLOR`。
3. 材质：**每种图集一个 `StandardMaterial`**（`unlit: true`、`alpha_mode: Blend`、`cull_mode: None`、
   `base_color_texture = 图集`）。字体图集是 `Rgba8UnormSrgb`，顶点色 × 图集 = 带色文字。
4. `AssetId<Image> → Handle<Image>` 用 `Assets::get_strong_handle`。

实测日志：

```
[spike] text3d entity=455v0 glyphs=14 atlas=AssetId{ index: 6 }
[spike] U4 skipped (not mirrored, e.g. text): 455v0 size_px=(168.0,24.0)
[spike] U4 button 456v0: delta=(0.00000,0.00000,0.00000)
```

- `05_medium.png`（0.62 m）：文字清晰、等宽、位置正确；
- `06_text_zoom.png`（约 6.7×）：**明显发糊** —— 位图图集的固有极限（方案 A 的已知代价）。
- 顺带验证：字体加载后标题高度从 0 变成 24 px，U4 断言仍然 delta=0（布局驱动的映射成立）。

## C. 对"feathers UI 适配层"的直接含义

1. **文字不用另起炉灶**：feathers 的 `Text` 节点同样产出 `TextLayoutInfo`，所以
   `LookLine → bevy_ui Text/TextSpan → 这套字形桥` 是一条直路，**富文本（高亮/diff 颜色）也能过**：
   `PositionedGlyph.section_index` 指向 `ComputedTextBlock.entities()[i]`，取该 section 的 `TextColor` 即可逐字着色。
2. **面板内所有节点共用一套坐标**：镜像（背景/边框/圆角）+ 字形桥（文字）都用
   `ComputedNode + UiGlobalTransform + units_to_world`，Z 用 `depth * 1mm` 分层。
3. **下一步就是 `LookLine → feathers` 的转换**：
   `LookLine::colored() -> ratatui::text::Line<'static>`（公开 API）→ `Vec<(String, TextColor)>` →
   一个 `Text` + N 个 `TextSpan` 的 feathers 节点 → 自动被字形桥画进 3D。**不需要 fork 就能先验证这一层。**

## D. 已知缺口（按优先级）

| 缺口 | 影响 | 方案 |
|---|---|---|
| 字形桥只在 `Changed<TextLayoutInfo>` 时重建 | 面板缩放/移动后文字不跟随 | 增加 `Changed<ComputedNode>` / 面板锚点变化触发重建 |
| 高倍放大文字发糊 | 近看/大 FOV 不达标 | ① 图集按 2× 字号光栅 + 四边形缩半（1 行改动，先试）② SDF 文本（终局） |
| 每 text 节点 1 mesh/1 draw call | 转写几百行时 draw call 多 | 面板级合批（同一图集的字形合并成一个 mesh） |
| `LookLine` 转换层未写 | 还不能显示 rig-tui 的真实转写 | 下一里程碑 |

## E. 下一步（按依赖）

1. `feathers_draw`：`LookLine.colored()` → feathers `Text`/`TextSpan` 节点树（**先不 fork**）。
2. 面板级合批 + 缩放跟随。
3. 用 mock 的 `ToolRender`（商品卡/订单卡/二维码）验证 look 的三种卡片。
4. 再决定是否 fork rig-tui 以复用它的 `transcript/pane` 管线（取决于第 1 步是否够用）。


---

# 进展二：feathers 适配层跑通（rig-tui look → feathers 节点 → 3D 面板）

`research/rig-feathers/` 现在是一个**可运行的适配层**（4 个模块，编译通过）：

```
src/transcript.rs  rig-tui LookLine::colored() -> feathers Node/Text/TextSpan
src/world3d.rs     路线 B 镜像（每个节点给世界 Transform + 有填充的加 mesh/material）
src/text3d.rs      bevy_text 字形 -> 3D 四边形（按 section_index 逐 span 着色）
src/style.rs       ratatui Style/Color -> bevy Color
```

mock 转写（商品列表 + 工具卡 + 状态行）已经在 3D 面板里正确显示：
`render/rf_shots/01_overview.png`（正视）、`02_near.png`（近看）、`04_text_zoom.png`（文字特写）。

## 这一轮踩到的两个真坑（都是"看起来对、其实错"的那种）

1. **`TextSpan` 会隐式要求 `TextFont`**（`#[require(TextFont, TextColor, ...)]`），默认是 **20 px 的默认字体**。
   后果：同一行里没有显式 `TextFont` 的 span 字号突然变大，而且**字形落进另一个字体图集**；
   我的字形桥"每个图集一个 mesh"会把先前的 mesh 覆盖掉 → 那一整段文字消失。
   **修法**：每个 span 都显式给 `TextFont`（同一字体族、同一字号），在字形桥支持多图集合并之前，**一个 Text 里不要混字体**。

2. **没有背景色的节点拿不到 Transform**：镜像最初只给"有填充"的节点设置世界坐标，
   而文字是这些行节点的**子节点** → 所有无底色行的文字塌到面板原点，叠成一团。
   **修法**：面板内**每个节点**都先落位（`Transform`），只有有填充的才加 mesh/material。

## 仍未解决（按优先级）

| 问题 | 现象 | 方向 |
|---|---|---|
| **强斜视下文字与色带错位** | 45° 时同一行的文字看起来"往下漂"，正视完全正常 | 三个候选：① 文字与色带共面时的绘制顺序 ② 每层 z 偏置的视差错觉 ③ 一个 Text 跨多图集导致 mesh 覆盖。下一步做**每个 Text 一个合并 mesh + 文字与所在行共用 z** |
| **CJK 缺字** | `¥ ● · ›` 显示成方块；中文完全没有 | 系统里只有 `NotoSansCJK-VF.ttc`，而 Bevy 的字库加载器只认 `ttf`/`otf`（`font_loader.rs:38`）。需要从 TTC 里抽一个 face 成 OTF，或引入 Noto Sans SC 的 OTF/TTF |
| 字库体积/回退 | 一个中文字库 10–30 MB | 子集化（只带常用 3500 字 + 符号）或按需加载 |
| 每 Text 一次 draw call | 长转写 draw call 多 | 面板级合批（同图集合并成一个 mesh） |

## 下一步

1. **修强斜视错位**：每个 Text 生成**一个**合并 mesh（多图集时按图集拆成子实体，而不是覆盖同一实体）；
   文字 z 改为"所在行的 z + 0.1 mm"。
2. **CJK 字库**：从 `NotoSansMonoCJK-VF.ttc` 抽 SC face（fonttools）转 OTF；验证 `¥ 中文` 在 3D 里显示。
3. 面板级合批 + 面板随相机距离缩放（移动端陀螺仪之前先把静态质量做对）。
4. 然后接 rig-tui 的真实转写（`transcript/pane`）：mock 换成 `RenderBatch` 驱动。
