# 大模型驱动 3D / VR 助手：技术栈与架构研究

> **状态（2026-11-26 核对）：当前（部分实现的研究记录）——M0 已完成，M1 大部分完成，
> M2/M3（VR）未实现，仍是计划。**
>
> - **已落地**：Bevy **0.20.1** + `rig-ecs` 0.47 内嵌（L3）、feathers 视觉、路线 B 的
>   `feathers_in_3d` 3D 面板（`rig_niri/src/frame.rs:691` 的 `place_panels`）、3D 文本桥
>   （`feathers_in_3d/src/text3d.rs`，本文 §6 的方案 A）。
> - **与当前实现的差距**：桌面布局已经不是本文设想的"卡片自由摆放"，而是 **niri 条带**
>   （[`NIRI_SCROLL.md`](NIRI_SCROLL.md)），视觉口径见 [`FEATHERS_UI.md`](FEATHERS_UI.md)；
>   VR/OpenXR（M2/M3）仍未做，本文 §2 的版本三角（OpenXR 只到 Bevy 0.19）仍是真约束。
> - **与当前口径直接冲突**：§8 把 Picker 写成"对话框/菜单（feathers dialog/select）"——
>   当前**没有弹层**，picker 是条带最右一列（`WORKSPACE_MODEL.md` §8、
>   [`PANEL_CANVAS.md`](PANEL_CANVAS.md)）；§7 的"面板可抓取/移动/缩放"未实现
>   （可拖动的是**整个工作区条带**，不是单个面板）。
> - **仍有效**：§2 版本三角、§6 的 3D 文本三方案与验收判据、§10 风险登记、§11 本机能力边界。
>
> 目标：**feathers 做前端 · rig 做内核 · 已验证的 3D feathers（路线 B）做空间面板 · rig 的工具调用取后端服务器数据 · 最终交付一个 VR 应用**
> 本机：Fedora 45 / Ryzen 7 7840H / 无 GPU 直通（llvmpipe）/ **无 OpenXR 运行时**（无 Monado / SteamVR）
> 日期：2026-11-23　｜　前置证据：`VERIFICATION_REPORT.md`（路线 B 实测）、`spike/RESETUP_DOC.md`（luma/feathers 源码级分析）

---

## 0. 一页结论

| 问题 | 结论 |
|---|---|
| 这套栈能不能成立？ | **能，但必须切三个进程边界**：内核（rig）/ 桌面前端（feathers）/ VR 前端（OpenXR）。 |
| 最大的硬约束是什么？ | **版本三角**：`rig-ecs` 精确钉 `bevy 0.20.1`；`bevy_mod_openxr 0.6` 只支持 `bevy 0.19`；两者**无法同进程**。 |
| 推荐的第一步 | **M0：在 Bevy 0.20.1 上做"非 VR 的 3D 助手"**——feathers + 路线 B 面板 + rig 内核内嵌。此组合所有版本刚好对齐，能在无头显的机器上把 90% 的东西跑通。 |
| VR 什么时候上？ | M2 阶段切到 **Bevy 0.19 + bevy_mod_openxr 0.6** 的独立前端进程；内核通过 OpenRig daemon HTTP 或 `rig-agent` sidecar 对接。 |
| 3D 文本怎么办？ | **本次研究定位的最大未解项**。路线 B 与 feathers 都不提供 3D 文本，luma 的 SDF 文本 material 是 `pub(crate)`。建议先做「bevy_text 字形 → 3D 四边形」桥接（约 300 行，图集+顶点色，一次 draw call/面板），VR 阶段再评估 SDF。 |
| 工具调用怎么取后端数据？ | 内核侧工具注册：`rig-mcp-client 0.47`（stdio + streamable HTTP MCP）或自写 HTTP 工具；UI 侧只消费 `tool_start/delta/tool_end` 事件，渲染成卡片。 |

**一句话**：技术上可行，**难点不在"能不能渲染"，而在"版本边界 + 3D 文本 + VR 输入"**；按 M0→M2 的顺序，前两项能在本机验证，第三项需要一台带头显的机器。

---

## 1. 目标拆解：五个技术支柱

| # | 支柱 | 具体要求 | 现状 | 证据 |
|---|---|---|---|---|
| **P1** | 前端框架 | 2D 桌面窗口 + 世界空间面板，一套 UI 定义 | feathers 覆盖 2D；3D 需路线 B | 上一轮报告 |
| **P2** | 空间（3D）UI | 面板在 3D 场景里、任意距离清晰、可交互 | 路线 B 已验证（delta=0、647% 清晰、Activate 触发） | `VERIFICATION_REPORT.md` §4 |
| **P3** | 内核 | agent 回合 / 工具调用 / 流式输出 / 会话 | `rig-ecs` 0.47 = 现成的 Bevy ECS 内核；`rig-agent` = 无 bevy 依赖的经典运行时；OpenRig daemon = 文档化 HTTP 协议 | crates.io 依赖 + OpenRig docs |
| **P4** | 后端数据 | 工具调服务器（HTTP/MCP）、权限确认 | `rig-mcp-client` 0.47（stdio + streamable HTTP，`rmcp 3.9`） | crates.io |
| **P5** | VR | 头显渲染、控制器/手势输入、文本输入 | **生态落后一版**（OpenXR 只到 Bevy 0.19）；本机无运行时 | `bevy_mod_openxr 0.6` 依赖 `bevy ^0.19` |

---

## 2. 版本三角（这是整个架构的骨架）

### 2.1 实测的依赖关系（全部来自 crates.io 的 dependencies 接口）

| 组件 | 版本 | 钉住的 Bevy | 关键点 |
|---|---|---|---|
| `rig-ecs` | 0.47.0 | **`bevy_app`/`bevy_ecs` = 0.20.1（精确）** | "The kernel of the rig coding agent on Bevy and rig-core" |
| `rig-tui` | 0.47.0 | `bevy_app/ecs/... = 0.20.1` | 只是 rig-ecs 的一个 **front**（`Front::new("tui")`） |
| `rig-core` | 0.47.0 | **无 bevy 依赖** | provider/model/tool/streaming 契约 |
| `rig-agent` | 0.47.0 | **无 bevy 依赖** | 经典 agent runtime（rig-core + rig-memory） |
| `rig-mcp-client` | 0.47.0 | `bevy_ecs = 0.20.1` | MCP 工具（stdio + HTTP） |
| `bevy_mod_openxr` | 0.6.0 | **`bevy ^0.19`** | 仓库 main 仍是 0.19；`bevy_oxr 0.3` 是 bevy 0.14 的老分支 |
| `bevy_feathers` | **0.19.0 / 0.20.x** | 双版本都有 | 桌面端可以任选 |
| `univis_ui` | 0.3.1 | `bevy ^0.19` | 路线 B 的 0.19 版材质来源（luma 的前身，更成熟） |
| `bevy_luma` | git main | `bevy 0.20.0` | 路线 B 的 0.20 版材质来源（我们已实测） |

### 2.2 三种组合的可行性

| 组合 | 内核 | 前端 Bevy | 可行？ | 评价 |
|---|---|---|---|---|
| **A. 全 0.20.1（无 VR）** | `rig-ecs` 内嵌 | 0.20.1 + feathers + luma 材质 | ✅ **完全对齐** | M0 首选：复用最多、版本零冲突 |
| **B. VR 0.19** | 独立进程（daemon / sidecar） | 0.19 + feathers 0.19 + univis 0.19 + OpenXR | ✅ 可行 | 客户端全 0.19，内核跨进程；这是**唯一现在能上 VR 的组合** |
| **C. 硬凑同进程** | rig-ecs(0.20.1) + OpenXR(0.19) | 一个二进制里两套 bevy | ⚠️ 理论可行（crate 重命名并存） | 编译时间/二进制翻倍、两套 `bevy_ecs` 类型不能互通，**不建议** |

> **关键结论**：VR 不是"加个插件"就能上的；它决定了**进程边界**。把钱花在进程边界（协议 + 事件模型）上，比把两套 Bevy 塞进一个进程划算得多。
> 另外：`bevy_mod_openxr` 升到 0.20 只是时间问题（0.6 刚跟到 0.19），**M0 走 0.20.1 不会白做**——升级时只换版本号。

---

## 3. 内核怎么接：三个层级（L1 / L2 / L3）

```
┌─────────────────────────────────────────────────────────────┐
│  前端进程（Bevy + feathers + 3D 面板）                        │
│    UI 状态机 ← 事件流 → 渲染                                   │
└───────────────┬─────────────────────────────────────────────┘
                │  ① HTTP/SSE/WS（OpenRig daemon 协议）
                │  ② in-process Rust 调用（同进程）
┌───────────────▼─────────────────────────────────────────────┐
│  内核：agent 回合 · 工具调用 · 会话 · 权限 · MCP                │
└─────────────────────────────────────────────────────────────┘
```

### L1 · OpenRig daemon HTTP（最松耦合）

OpenRig 有**文档化的 daemon 协议**（`openrig.dev/docs/daemon-http-clients`）：

- 发现：`rig status --json` 的 `daemon` 对象（`state/reachable/pid/port`），或 `$OPENRIG_HOME/daemon.json`，或 `OPENRIG_HOST/OPENRIG_PORT`（默认 `127.0.0.1:7433`）
- 健康：`GET /healthz`（`status/pid/eventLoop.lagP99Ms/routeTimings`）
- 鉴权：按路由族校验 bearer token
- **事件流**（这正是流式 UI 要的）
- 文档明确说"未列出的路由形状可能变化"→ 需要锁版本

✅ 优点：前端可以任意 Bevy 版本（VR 用 0.19 也没问题）；不用链接庞大的 rig；daemon 已在管会话/工具/权限。
⚠️ 代价：必须有 daemon 在跑；协议演进要跟。

### L2 · `rig-core` + `rig-agent` 内嵌（无 bevy 依赖）

```
rig-core 0.47   →  Model / Completion / Tool / streaming
rig-agent 0.47  →  经典 agent runtime（turn 循环、工具调度）
```
✅ 任意 Bevy 版本；单进程、无外部依赖。
⚠️ 自己写驱动：把 async stream 桥到 Bevy 事件（`bevy_tasks` + channel），约 300–500 行；会话/权限 UI 要自己做。

### L3 · `rig-ecs` 内嵌（Bevy 原生内核，复用最多）

`rig-ecs` 就是"跑在 Bevy ECS 上的 agent 内核"：agents / turns / tool 调用 / picker 都是组件与消息；`rig-tui` 只是在它上面画终端。
✅ M0 最佳：能把 rig-tui 的**状态机**（editor/keys/pick/pane）直接搬来当 UI 层，只换绘制。
❌ **必须 bevy 0.20.1** → 与 OpenXR(0.19) 冲突。

### 推荐

| 阶段 | 内核 | 理由 |
|---|---|---|
| **M0（现在）** | **L3** `rig-ecs` 内嵌 | 版本恰好对齐，复用 rig-tui 的交互状态机，最快出可用 3D 助手 |
| **M1** | L3 保持 | 同上 |
| **M2（VR）** | **L1**（daemon）或 **L2**（sidecar `rig-agent`） | VR 客户端锁 0.19；内核退到进程外 |

> 进程边界一旦定好，L1/L2 的选择是运维问题：要"持久团队/多客户端"→ L1；要"单机单助手、少依赖"→ L2。

---

## 4. 前端分层：一个 UI 定义，两个渲染前端

### 4.1 桌面端 = feathers（已研究结论）

上一轮的结论不变：**桌面 → feathers**（同版本族、控件/IME/a11y 现成、成熟度可交付）。rig-tui 的 `Front` 抽象允许 `--gui` 与 `--tui` 并存。

### 4.2 VR 端 = 世界空间面板（三种实现路线，见 §5）

### 4.3 为什么两端不能同进程（实测证据）

| 事实 | 证据 |
|---|---|
| `bevy_luma::LumaPlugin` + `bevy_ui` 0.20.1 **调度成环**，第一帧前 panic | `RESETUP_DOC.md` §3.1（14522 cycles） |
| 路线 B 只注册 `UnivisRenderPlugin`（材质+shader）才能与 bevy_ui 共存 | 同上 |
| 两套 Bevy 版本同进程需要 crate 重命名并存 | 编译时间/体积双倍，类型不互通 |

**设计含义**：把"UI 定义（面板/控件/事件）"抽成**与渲染无关的层**，桌面/VR 各自渲染。rig-tui 已经这么做了（`view.rs` 的 `DrawnTranscript` 是纯数据），照抄这个模式即可。

---

## 5. VR 的 3D UI：三条路线

| | V1 · 路线 B（feathers 布局 → 3D 材质镜像） | V2 · univis/luma 原生 world_3d | V3 · UI→纹理面板 |
|---|---|---|---|
| 做法 | bevy_ui 布局树 + 每节点 3D mesh + SDF 材质 | 直接用 univis(0.19)/luma(0.20) 的 `URootUi::world_3d` | bevy_ui 渲染到 `Image`，贴到 3D 四边形 |
| 文字 | **要自己接 3D 文本** | ✅ 自带 SDF 文本（`text_label_sdf_3d.wesl`） | ✅ 全部文字照常 |
| 控件 | ✅ feathers 全套 | ⚠️ 自己的控件集（要重写 pick/status/editor） | ✅ feathers 全套 |
| 清晰度 | ✅ SDF，任意距离 | ✅ SDF + SDF 文本 | ❌ 分辨率绑定（近看糊） |
| 与已验证据一致 | ✅ 已实测 | 未实测（univis 0.19 / luma 0.20） | 未实测 |
| 工作量 | 中（文本桥接 ~300 行） | 中（VR 前端另起，不能与 feathers 同进程） | 低（但 VR 体验打折） |
| 风险 | 3D 文本质量 | 生态 alpha（luma 1 commit；univis 更成熟） | 分辨率/性能 |

**推荐**：
- **M0–M1**：V1（路线 B），与已验证的结论一致，桌面/VR 可共享同一套面板定义；
- **M2**：若 V1 的文本质量在头显里不达标（1.5 m 处 14 px 字号发虚），VR 前端切 **V2**（univis 0.19 自带 SDF 文本），桌面端继续 feathers —— 反正两端本来就是不同进程。

> 注意：`luma` 的 `UTextLabelSdfMaterial3d` 是 **`pub(crate)`**（`crates/luma_engine/src/layout/text/render.rs:91`），**不能直接复用**；要用要么 fork，要么自己实现 SDF 文本。这是 V2 的真实成本。

---

## 6. 3D 文本：本项目的最大未解项

### 6.1 现成的东西（已核实）

```rust
// bevy_text 0.20.1: 布局结果里每个字形的**位置**和**图集信息**都是公开的
pub struct TextLayoutInfo {
    pub glyphs: Vec<PositionedGlyph>,   // position: Vec2, atlas_info: { texture: AssetId<Image>, rect, .. }
    pub run_geometry: Vec<RunGeometry>, // 每行的装饰矩形（背景色/下划线）
    pub size: Vec2,
    pub selection_rects: Vec<Rect>,
}
```

`PositionedGlyph { position, atlas_info: GlyphAtlasInfo { texture: AssetId<Image>, rect, .. }, line_index, section_index }`
→ **bevy_text 已经完成了整形/换行/排版，只差把字形画到 3D 空间里**。

### 6.2 三个实现方案

| 方案 | 做法 | 清晰度 | 工作量 | 备注 |
|---|---|---|---|---|
| **A. 字形四边形**（建议先做） | 每个面板一个 mesh：每字形 2 个三角形，UV 取 `atlas_info.rect`，顶点色取 `TextColor`；材质用 `StandardMaterial { unlit: true, base_color_texture: atlas, alpha_mode: Blend }` | 位图图集，近距离会糊（图集按 1x 光栅） | ~300 行，**无自定义 shader** | 一次 draw call/面板；先用它把"文字真的进 3D"跑通 |
| **B. SDF / MSDF 文本** | swash 光栅 SDF，shader 里 `smoothstep` | ✅ 任意距离清晰 | 800–1500 行（或 fork luma 的 text 栈） | VR 的终局方案；luma 有实现但 `pub(crate)` |
| **C. UI→纹理** | `Camera2d` 渲染到 `Image`，贴 3D | 分辨率绑定 | ~150 行 | 快速 demo 可用，VR 体验差 |

**建议**：M0–M1 用 **A** 跑通全链路，同时在 M1 末尾用一台头显对"1.5 m / 14 px"做可读性验收；不达标再上 **B**。

### 6.3 验收判据（写死，免得自欺）

- 1.5 m 距离、14 px（面板 `units_to_world = 0.0012` → 约 2.5 mm 字高）下，连续阅读 30 秒不头晕、能分辨 `0/O`、`l/1`；
- 面板倾斜 45° 时文字边缘无摩尔纹；
- 单面板 500 字重绘 < 2 ms（CPU）。

---

## 7. VR 交互设计

| 交互 | 方案 | 复用/证据 |
|---|---|---|
| 指向 | 控制器/手射线 → 面板平面求交 → 面板本地 px | 路线 B 的 `intersect_ray_with_node_plane`（luma 的 `clip_sync`，公开） |
| 点击 | 合成 `PointerPress/Release/Click` 打到命中的 UI 实体 | ✅ 已实测（`PointerClick → Activate`） |
| 悬停 | 合成指针 + `PointerOver` | ✅ 已实测 |
| 文本输入 | ① feathers `virtual_keyboard`（现成）渲染成第二块面板 ② 语音（M3）③ 手机端遥控（可选） | feathers 自带虚拟键盘；头显里没有系统 IME |
| 中文输入 | 拼音虚拟键盘 / 语音转写；**不要指望 IME** | — |
| 面板操作 | `WorldPanel.anchor: Transform` 让面板可抓取/移动/缩放（已是字段） | 路线 B 已支持任意锚点/旋转/多面板 |
| 选择复制 | 面板内文本选择（需自研，bevy_ui 只对 `EditableText` 原生支持） | 见风险 R5 |

---

## 8. 数据流与事件模型

```
用户输入(语音/键盘) ──┐
                      ├─► Front(client) ──prompt──► Kernel(rig)
拾取/点击(picker)  ───┘                                 │
                                                        ├─ stream: TextDelta
                                                        ├─ ToolCallStart/ArgsDelta/Result
                                                        ├─ PickerRequest(权限/选择)
                                                        └─ TurnEnd
                      ┌─────────────────────────────────┘
                      ▼
              TranscriptStore（纯数据，复用 rig-tui 的 view/DrawnTranscript 思路）
                      │  dirty 标记
                      ▼
              面板重绘（只在变化时；对应 rig-tui 的 frame_due）
```

**事件约简成 5 类**（建议直接做成 `serde` 的协议，桌面/VR/未来 Web 共用）：

| 事件 | 载荷 | UI 表现 |
|---|---|---|
| `TextDelta` | turn_id, text | 追加到当前消息 |
| `ToolCall` | id, name, args(stream), result | 卡片：标题 → 参数 → 结果（折叠） |
| `Picker` | id, title, options | 对话框/菜单（feathers dialog/select） |
| `Status` | model, tokens, elapsed | 状态行 |
| `Error` | code, message | 红色卡片 + 重试 |

**工具取后端数据**：`rig-mcp-client 0.47` 支持 stdio 与 streamable HTTP 的 MCP server（`rmcp 3.9`），把后端服务器包成 MCP tool 即可；也可以用 `rig-core` 的 `Tool` trait 自写 HTTP 工具（自己控制超时/鉴权/重试）。UI 不关心工具怎么实现，只渲染事件。

---

## 9. 里程碑与工时

| 阶段 | 内容 | 触发判据 | 估时 |
|---|---|---|---|
| **M0 · 3D 桌面助手** | Bevy 0.20.1 + feathers + 路线 B 面板 + rig 内核（L3）+ mock 工具；流式文字进面板；工具卡片 | 一条 prompt 能让面板上出现流式文本 + 一张工具卡 | **2–3 周** |
| **M1 · 可用** | 3D 文本（方案 A）、多面板、真实 MCP 工具、输入框 + 历史、错误处理 | 1000 字 + 5 个工具调用的会话不卡 | **3–5 周** |
| **M2 · VR** | 0.19 + bevy_mod_openxr 0.6 + 面板 + 控制器射线 + 虚拟键盘；内核转 daemon/sidecar | 头显里能读、能点、能打字 | **4–8 周** |
| **M3 · 打磨** | 语音、空间布局、90 Hz 性能、打包分发 | 连续 30 分钟不晕、不掉帧 | **3–6 周** |

> 与"从零写"相比，M0 便宜在 `rig-ecs` 把 agent 回合/工具/权限都做好了；贵在 3D 文本和面板交互。

---

## 10. 风险登记

| # | 风险 | 等级 | 缓解 |
|---|---|---|---|
| **R1** | OpenXR 生态停在 Bevy 0.19 | 高（阻塞 VR） | M0/M1 走 0.20.1 不白做；盯 `bevy_mod_openxr` 0.7；到时只换版本号 |
| **R2** | 3D 文本质量不达标（图集模糊） | 高 | M1 末尾用头显验收；不达标切方案 B（SDF）或 V2（univis） |
| **R3** | 内核跨进程后的流式/工具状态同步 | 中 | 先定事件 schema（§8），桌面/VR 共用；daemon 协议锁版本 |
| **R4** | 90 Hz 性能（每面板 draw call、overdraw） | 中 | 面板级合批（方案 A 的 glyc mesh + 面板 mesh 各 1 次 draw）；限制面板数/面积 |
| **R5** | VR 没有 IME、选择复制要自研 | 中 | 虚拟键盘 + 语音；选择复制优先级降到 M3 |
| **R6** | 路线 B 的透明排序/裁剪（已实测） | 中 | 实心面板走 Opaque（已验证）；半透明面板做 Transparent3d 排序覆写 |
| **R7** | 两套前端（0.19 / 0.20）维护成本 | 中 | UI 定义与渲染分层；共享 crate 只依赖 `bevy_ecs` 之外的东西 |
| **R8** | rig 生态快速演进（0.47，几乎每周发版） | 中 | 锁定 minor；协议层做适配器 |

---

## 11. 本机能验证什么 / 需要什么设备

| 项 | 本机 | 说明 |
|---|---|---|
| 编译、面板渲染、截图 | ✅ | llvmpipe 可出图（已产出 `render/shots`） |
| 3D 文本桥接 | ✅ | bevy_text 的 `TextLayoutInfo` 可读；用 `StandardMaterial(unlit)` 出图 |
| 流式 / 工具调用 | ✅ | mock 工具（不调真模型）即可验证管线 |
| 合成指针 → 控件 | ✅ | 已实测（Touch(7) → Activate） |
| **OpenXR 运行时** | ❌ | 本机无 Monado / SteamVR；`/dev/dri` 也不可见 |
| **90 Hz 性能** | ❌ | 软件渲染无意义 |
| **头显输入（手柄/手/眼动）** | ❌ | 需要真机 |

**需要的最小设备**：一台带 GPU 的 Linux/Windows + Quest 3（ALVR/Link）或 Valve Index；或一台 Windows + WMR（已停更，不推荐）。

---

## 12. 立刻可做的三件事（建议顺序）

1. **3D 文本桥接 spike（1–2 天）**：读 `TextLayoutInfo.glyphs` → 生成面板级 mesh（图集 UV + 顶点色）→ `StandardMaterial(unlit)` → 截图验证。**这一项决定 V1 是否成立**。
2. **rig 内嵌跑通（2–3 天）**：`rig-ecs` + 一个 mock tool → 把 `TextDelta/ToolCall` 变成 Bevy 事件 → 面板上出流式文字。
3. **定事件协议（1 天）**：把 §8 的 5 类事件写成 `serde` schema，桌面/VR 两个前端共用；内核边界（L1/L2/L3）先留接口，M0 用 L3 实现。

> 这三件事做完，M0 的核心风险就清零了；VR 留在 M2，用设备说话。

---

## 附录 A · 版本清单（照抄）

```toml
# M0：全 0.20.1（非 VR）
bevy            = "=0.20.0"      # 子 crate 自动到 0.20.1；见 RESETUP_DOC 的 custom_cursor 修正
bevy_feathers   = "=0.20.0"
bevy_window     = { version = "=0.20.1", features = ["custom_cursor"] }
bevy_winit      = { version = "=0.20.1", features = ["custom_cursor"] }
bevy_luma       = { path = "../bevy_luma" }   # git main（0.20）
rig-ecs         = "=0.47.0"      # 精确钉 bevy 0.20.1
rig-core        = "0.47"
rig-agent       = "0.47"         # 无 bevy 依赖

# M2：VR 前端（独立进程）
bevy            = "0.19"
bevy_feathers   = "0.19"
bevy_mod_openxr = "0.6"          # bevy 0.19
univis_ui       = "0.3.1"        # bevy 0.19（若走 V2）
rig-core        = "0.47"         # 客户端只用事件协议，不链接 rig-ecs
```

## 附录 B · 关键 API 锚点

```
bevy_text-0.20.1/src/pipeline.rs:485   TextLayoutInfo { glyphs, run_geometry, size, selection_rects }
bevy_text-0.20.1/src/glyph.rs:15       PositionedGlyph { position, atlas_info: { texture, rect, .. } }
bevy_text-0.20.1/src/editing.rs:156    EditableText { allow_newlines }  ← 多行输入
bevy_ui_widgets-0.20.1/src/text_input.rs:499  on_ime_input(Ime)         ← IME
bevy_feathers-0.20.0/src/constants.rs:14      fonts::MONO = FiraMono
bevy_feathers-0.20.0/src/controls/virtual_keyboard.rs   ← VR 输入兜底
luma/crates/luma_engine/src/layout/text/render.rs:91    UTextLabelSdfMaterial3d（pub(crate)，不可直接用）
luma/crates/luma_engine/src/layout/render/clip_sync.rs:196  intersect_ray_with_node_plane（公开，可复用）
rig-ecs 0.47 Cargo.toml                bevy_app/bevy_ecs = "=0.20.1"（精确钉）
bevy_mod_openxr 0.6 Cargo.toml         bevy ^0.19
```

## 附录 C · 与既有产物的关系

| 既有产物 | 在本架构中的位置 |
|---|---|
| `VERIFICATION_REPORT.md`（路线 B 实测） | 直接作为 §5 的 V1 依据 |
| `spike/`（feathers → 3D 面板，含截图巡游/合成指针/U4 断言） | M0 的渲染底座 |
| `rig-tui-inspect/`（rig-tui 0.47 源码） | §3 L3 要复用的状态机（editor/keys/pick/view） |
| `spike/RESETUP_DOC.md` | §2/§4.3 的版本与调度冲突证据 |
