# rig_niri ↔ rig-tui 移植核对（2026-11-26，F1–F10 / L1–L4 / S1–S4（功能）/ F6 切片 1–2 之后）

> **状态**：当前（2026-11-26 08:2x 快照）。取代 2026-11-26 01:0x（F1–F5 之后）那一版；
> 差异清单见 §5。本文只改结论与证据指针，审计方法沿用：逐文件读两边源码。
>
> **口径**：**内容/行为对照 rig-tui；视觉与布局以 `bevy_feathers` + niri 为准**
> （[`../design/FEATHERS_UI.md`](../design/FEATHERS_UI.md)、
> [`../design/NIRI_SCROLL.md`](../design/NIRI_SCROLL.md)），**像素级一致性不再是验收标准**。
> 面板外观/按钮/输入框/滚动条这类"长得不一样"不算缺口；只有"行内容、交互语义、公开能力"
> 的差异才进 🟡/❌。布局 **L1–L4 已落地，L5（scroll 插值）未做**；本文件按
> `NIRI_SCROLL.md` §0 的现行状态记。
>
> **基准**：上游 `rig-tui 0.47.0`（`.cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/rig-tui-0.47.0/src/`，下称"上游"）
> vs 我们 `rig_niri 0.1.0`（`rig_niri/src/`）。上游源码不变，所以上游行号是固定的。
> **方法**：结论只来自代码（`grep`/`read`），数字来自 `wc -l`、`grep -c '#\[test\]'`、`cargo test`
> （复现命令见 §6）。不沿用旧文档的任何结论。
> **状态标记**：
> ✅ **已移植** = 该模块对外的职责已完整落地，实现可以换（视觉按 feathers 不算差异）；
> 🟡 **部分** = 主路径在，但公开能力有明确缺口（§3 逐条写"缺什么、缺在哪个函数"）；
> ❌ **未开始**；
> 🔄 **有意替换** = 终端管道，用 bevy 窗口/渲染取代；
> ➕ **自有** = 上游没有对应职责，不是移植目标（§2 单列，不计入状态分布）。
>
> **快照警告**：本文件写于 **2026-11-26 08:2x**，此时 `rig_niri` 有 **606** 个 `#[test]`
> （`cargo test --workspace --offline` 全绿，见 §0）。本轮落地了
> F1–F10、L1–L4、S1–S4（功能）、F6 切片 1/2、`ImageNode` 镜像、键盘复制、`/show`、`/theme`、
> 图片路径粘贴/拖入、free-text 输入框作答、条带竖直重叠修复。
> **行号是这一刻的快照，以函数/类型名为准**；`grep` 命令都能复现结论本身。

---

## 0. 数字

| 指标 | rig-tui 0.47.0 | rig_niri 0.1.0 |
|---|---|---|
| `.rs` 文件 | **46** = 25 个实现文件 + 21 个 `tests.rs` | **26**（测试内联在 `#[cfg(test)] mod tests`，没有独立测试文件；比上一版多 `panel.rs`） |
| 总行数 | **23,711** | **43,162** |
| 实现行（扣掉测试模块） | **14,316** | **21,913** |
| 测试行 | **9,395**（21 个 `tests.rs`） | **21,249**（各文件从 `#[cfg(test)] mod tests` 算到文件尾） |
| `#[test]` 个数 | 246 | **606** |
| `cargo test` | — | `rig_niri` **606 passed**；`rig_harness_in_3d` lib **36**、bin **5**；`feathers_in_3d` **15**；doctest：`rig_niri` 2 passed / 2 ignored，`rig_harness_in_3d` 1 passed，`feathers_in_3d` 0 |

- 本轮实测：**606**（2026-11-26 08:2x，`cargo test --workspace --offline`，exit 0）；历史：
  **528**（L1 并行时）、**526**（F1–F5 基线）、**441**（11-25 22:50）、**423**（11-25 22:40）。
  增长来自 F1–F10、L1–L4、S1–S4（功能）、F6 切片 1/2、主题切换、图片路径、输入框作答等。
- doctest 数字同上：`rig_niri` 2 passed / 2 ignored（`clipboard` 两段 ignored、`history`/`picker` 各一），
  `rig_harness_in_3d` 1 passed，`feathers_in_3d` 0。
- 我们的"自有"行约 **7,808**（`niri.rs` 771、`workspaces.rs` 4,060、`layout.rs` 2,295、
  `spatial.rs` 434、`camera_rig.rs` 128、`style.rs` 120），其中实现 4,038、测试 3,770。
- 行数**不能**当覆盖率：我们的行引擎、工作区、niri 平铺是自己设计的，不比上游少。

**模块状态分布**（§1 的 26 个上游模块）：

| 状态 | 个数 | 是哪些 |
|---|---|---|
| ✅ 已移植 | **12** | `screen` `complete` `diff` `highlight` `hints` `history` `jsonl` `markdown` `panel` `selection` `status` `theme` |
| 🟡 部分 | **11** | `lib` `clipboard` `editor` `input` `keys` `pane` `pick` `render` `renderers` `transcript` `view` |
| ❌ 未开始 | **0** | — |
| 🔄 有意替换 | **2** | `terminal` `title` |
| ➖ 不适用 | **1** | `tests`（上游集成测试，不是移植目标） |

一句话：**25 个上游实现模块全部有落点，没有"未开始"**。这一轮把上一版还标着"部分"的
`panel`（插件画布切片 1/2）和 `status`（`StatusLine` 组件）翻成 ✅，并修正了
`pane`（多来源产行）、`pick`（输入框作答改由 editor 承担）、`editor`（free-text 作答）、
`clipboard`（图片路径解析）的旧结论。剩下的缺口集中在五处：
① 多来源转写的收尾（`Replaced`/`PastEffects`，S5–S6）；
② 插件扩展点的最后一截（插件命令绑定入口）；
③ 图片剪贴板取图（`Paste::Image` 没有生产者）；
④ 公开的 view/选择 API（`TranscriptView`/`Highlight`/`DrawnTranscript`）；
⑤ 终端专有的屏幕导出（`TuiFrame`，有意不移植）。
逐项排在 [`REMAINING.md`](REMAINING.md)；视觉/布局见
[`../design/FEATHERS_UI.md`](../design/FEATHERS_UI.md)（F1–F8/F10 已完成）和
[`../design/NIRI_SCROLL.md`](../design/NIRI_SCROLL.md)（L1–L4 已完成，L5 未做）。


---

## 1. 上游模块总表

**证据写法**：`文件:行` 是我们/上游在快照里的位置；"缺什么"只写**公开能力**的缺口，不列实现差异。

| # | 上游模块（行） | 我们的落点（行） | 状态 | 证据 / 缺什么 |
|---|---|---|---|---|
| 1 | `lib.rs`（422） | `lib.rs` 306、`front.rs` 39 | 🟡 | 有：`FeathersSystems` 五阶段（`lib.rs:61`）、`FeathersViewPlugin` build（`lib.rs:163`）、`ThemeKind` 安装（`lib.rs:170` → `theme.rs:282`）、插件元组（`lib.rs:190`–`:204`）、finish 里给 `Agent` **与 `TurnOf`** 注册 `StatusItems`（`lib.rs:216`–`:217`）、前端身份 `FRONT="feathers"`（`front.rs:10`）+ `Answerer`/`ShowsPickers`（`front.rs:25`）。缺：`TuiPlugin`/`TuiPlugins`（上游 `:118`/`:133`）那种"一键装全套 + 可选 part 去重"的装配器；无终端的 `add_view`（上游 `:260`）；`app.keeps_files("tui")`（上游 `:180`）；`BuildFailedLooks`（上游 `:398`）；`BatchPlugin` 不在视图插件里（应用层手动加，`rig_harness_in_3d/src/main.rs:68`）。 |
| 2 | `terminal.rs`（620） | — | 🔄 | 裸模式/备用屏/鼠标捕获/括号粘贴/OSC 52/`TuiTestScreen` 全部由 bevy 窗口 + `bevy_clipboard` 取代（见 §4）。还有用户价值但没替代的是 `RunInTerminal`（跑 `$EDITOR`，上游 `:57`）。复制已由 `selection.rs` + `clipboard.rs` 直接写系统剪贴板：`copy_text`/`copy_selection`/`copy_selection_keys`（`clipboard.rs:233`/`:244`/`:273`）。 |
| 3 | `screen.rs`（46） | `frame.rs:534`/`:924`、`layout.rs:1111` | ✅ | 默认屏幕 spawn：`DefaultScreen`（`frame.rs:534`）+ `spawn_default_screen`（`:924`）+ `default_screen_panels`（`:826`），确认 tile 高度由 `sync_confirm_tile`（`:904`）管；树来自 `layout::default_tree`（`layout.rs:1111`），7 个叶子：转写/留白/确认/补全/输入/提示/状态（`layout.rs:1123` 的 root + `main_row` `:1137`）。上游的 `InlineStrip`（`pick.rs:1064`）换成 `PanelId::Confirm` tile（`panels.rs:66`）——职责（"工作旁边的问题"）落地，位置按 niri 设计换掉。 |
| 4 | `clipboard.rs`（340） | `clipboard.rs` 1420 | 🟡 | 有：`ClipboardText` 抽象（`:164`）、`copy_text`/`copy_selection`/`copy_selection_keys`（`:233`/`:244`/`:273`）、`Copied N lines` 与状态项（`:81`/`:96`/`:104`）、`read_paste`/`apply_paste`/`paste_text`/`type_path`（`:467`/`:650`/`:663`/`:700`）、`Paste::Image` 的 `@path` 语义（`:452`–`:459`）、`paste_from`（`:726`）。**文本粘贴已接线**：`Do::PasteImage` → `Act::Paste`（`editor.rs:1150`）→ `clipboard::paste_from`（调用点 `editor.rs:1435`），失败写 `PASTE_FAILED`（`clipboard.rs:126`/`:129`）、成功清掉上一次错误（`editor.rs:1442`）。**图片路径已解析**：`image_paths`（`:486`）、`attach_image_paths`（`:499`）、`dropped_file_text`（`:525`）；窗口拖入在应用层 `drop_image_files`（`rig_harness_in_3d/src/lib.rs:389`，注册 `:350`）。缺：**剪贴板里直接是图片**取不到——`bevy_clipboard` 没开 `image` feature，`read_paste` 只读文本（`:467`），`Paste::Image` 没有生产者（模块文档 `:24`–`:27`），应用层也没有把图片存进上游 `Scratch::images`（`rig-harness-0.47.0/src/host/session.rs:170`，本仓 `grep -rn Scratch rig_harness_in_3d/src` 为 0）。 |
| 5 | `complete.rs`（681） | `complete.rs` 1881 | ✅ | 数据模型逐个对齐：`CompletionSource`（`:92`）/`CompletionTrigger`（`:101`）/`CompletionKind`（`:181`）/`Candidate`（`:206`）/`Completion`（`:215`）；`SHOWN=50`（`:69`）；`FileIndex`+`walk`（`:489`/`:588`）、`match_at`/`token`/`path_items`（`:671`/`:677`/`:711`）、`markers`（`:767`）、`Sources`（`:783`，`pub(crate)`）、`typed`（`:805`）、`follow_input`/`receive_paths`（`:919`/`:948`）、`CompletionPlugin`（`:1000`）、候选 tile（`completion_rows` `:1042`、高度 `:1080`、`sync_completion_tile` `:1093`）。键位接线：`editor.rs:1198` 的 `completion_act`（↑↓ 选中、Tab/Enter 接受、Esc 关闭并记住 token）。已知差异：`walk` 不读 `.gitignore`（`complete.rs:29`–`:31`）；上游"粘贴后不弹列表 / 被拒命令放回不弹列表"两条抑制没做（`complete.rs:24`–`:26`）；`InputContext` 没移植（`typed()` 等价，`:805`）。 |
| 6 | `diff.rs`（318） | `rich.rs:124` + 上游 `ToolRender` | ✅ | `diff_rows` 直接调 `rig_tui::diff::unified_lines`（`rich.rs:124`）；工具卡片走 `ToolRender::default_lines`→`result_lines` 的 diff 分支（上游 `renderers.rs:243`），我们的建批用 `batch::default_lines`（`batch.rs:847`）。行级红绿 → `Row.band`，改动词更深一档 → span 底色（渲染层已支持，见 §3 `theme`）。 |
| 7 | `editor.rs`（770） | `editor.rs` 4384 | 🟡 | 有：游标=字节偏移+grapheme 边界（`Editor` `:100`、`set_cursor` `:407`），编辑动作齐全（`act` `:1104` 一带）；**多行显示已完成**：`layout`/`layout_within`（`:672`/`:682`）按面板列宽折行，`MOST_LINES=10`（`:115`）、最少 3 行（`LEAST_ROWS` `:123`）、光标按真实行列反色下划线（`CURSOR` `:266`、`with_cursor` `:299`），`editor_rows`（`:1591`）每行一个 `KeyedRow`，tile 高度随内容 `input_tile_height`（`:181`）+ `sync_editor_tile`（`:1573`）；F5 输入框外壳 `sync_input_field`（`:1638`）与 `sync_input_row_height`（`:1701`）；**输入框内作答已完成**：`first_free_text_ask`（`:978`）→ `answer_with_text`（`:1049`），`AnswerSeeded`（`:1009`）保证 `initial` 只填一次，Enter 分支在 `:1385`–`:1388`；测试 `:2735`/`:2759`/`:2779`/`:2833`/`:2852`/`:2870`/`:2898`。缺：`Editor` 是 `Resource`（`:100`）不是 `Component`（上游 `:61`），`MainEditor`（上游 `:83`）/`MainInput` 不存在；上游 `answer`/`answering`/`take_answer`（上游 `:188`/`:202`/`:208`）没有同名 API（能力由 `AnswerSeeded` + `answer_with_text` 承担）；输入框内按状态换提示（上游 `draw_editors` `:686`）没有。 |
| 8 | `highlight.rs`（444） | `rich.rs:102` | ✅ | `highlight_rows` 调 `rig_tui::highlight::code_rows`（`rich.rs:102`）；围栏代码的语法色由 `markdown::render` 内部的 `Grammars` 带上（`rich.rs:88`，建批的 `batch::reply_lines` 直接调上游 `markdown::render`，`batch.rs:1253`），所以回复里的代码块有语法色。 |
| 9 | `hints.rs`（102） | `hints.rs` 581 | ✅ | 有：两张键位表逐字相同（`keys` `:44`，与上游 `:48`–`:70` 一致）、`fit_keys` **已逐字对齐**（`:73`：`format!(" {} ", …)` 带首尾空格、`unicode_width` 算宽、同权重丢**最后**一个；测试 `:288`/`:302`/`:312`/`:321`）、独立 1 行提示 tile（`hint_rows` `:145`、`hint_tile_rows` `:159`、`hint_panel_rows` `:183`、`PanelId::Hints`）。显示时机也对齐：只在输入为空时显示、非空画一行空行（`:183`，测试 `:432`）；`turn_running` 由 `ShownAgent` + `ActiveTurn` 推（`:183`）。唯一文案差异：`Ctrl+V paste`（`:51`/`:59`）跟实际行为走，等剪贴板图片（REMAINING §4）做完再改回 `image`——这是 clipboard 的缺口，不是 hints 的。 |
| 10 | `history.rs`（223） | `history.rs` 733 | ✅ | `PromptHistory`（`:50`）含 `load`（`:65`）/`record`（`:114`）/`seed`（`:126`）/`rewrite`（`:136`）、`Sent`（`:207`）、`HistorySystems::{Seed,Record,Save}`（`:193`）；系统 `load`（`:214`）/`record`（`:235`）/`save`（`:287`）/`mirror`（`:312`）/`seed_shown`（`:328`）；JSONL 一行一个字符串、坏行跳过、去重按最后位置（测试 `:488`–`:714` 共 12 个）。差异：文件路径由插件参数给（`HistoryPlugin` `:374`），不用上游的 `PluginData`/`Files`；`Editor` 自己是历史持有者，所以多一层 `mirror`（`:312`）。 |
| 11 | `input.rs`（1026） | 分散在 `editor.rs`/`keys.rs`/`pane.rs`/`selection.rs`/`complete.rs` | 🟡 | 有：`PendingInput`（`keys.rs:958`）、按键分派（`editor.rs:1280` `keys`）、作用域/焦点（`keys.rs`）、滚轮 3 行（`pane.rs:1320`，`WHEEL_ROWS` `:153`）、拖选/双击/三击（`selection.rs:944` `pointer_selection`）、补全接受（`editor.rs:1198` `completion_act`）。**"键 → 会话"的动作已接**：Enter → `Deliver`（`DeliveryMode::Steer`，`editor.rs:1157`）、turn 跑着时 Tab → `Queue`（`:1154`）、Esc → `StopTurn`（`:1160` 一带，落点 `:1429`）、空输入 Ctrl+D → `AppExit`（`editor.rs:1427`）、Alt+1..9 → `answer_shown_ask`（`editor.rs:1396` → `:1065`）、Alt+0/Esc → `refuse_shown_ask`（`:1087`）。缺：**细粒度**——`InputSystems::{Filter,Dispatch}` / `TuiKeys`（上游 `:111`/`:116`）、`InputContext`（上游 `:130`）都没有（`grep -rn 'InputSystems\|TuiKeys\|InputContext' rig_niri/src` 只命中注释）；`act` 里的 `SelectUp/SelectDown/Deselect` 仍是空实现（`editor.rs:1136`–`:1138`），转写区的 Alt+↑/↓ 是 `pane_keys` 直接读物理键做的（`pane.rs:1081`），不经过绑定表；`Deliver::literal` 的"命令被拒后当普通消息重发"没有（本 crate 没有 `refused`，`editor.rs:1184`/`:1414`）。 |
| 12 | `jsonl.rs`（75） | `history.rs` | ✅ | "一行一个 JSON 值"的追加/读取落在 `PromptHistory::{load,record,save}`（`history.rs:65`/`:114`/`:287`）里，坏行跳过不 panic。上游的 `tail`（跟随文件）没有；我们没有"跟读历史文件"的需求。 |
| 13 | `keys.rs`（751） | `keys.rs` 1984 | 🟡 | 有：`KeyPress`（`:153`）、`Keys`（`:287`）、`KeyScope`（`:343`）、`KeyBinding`（`:380`）、`KeyPressed`（`:425`）、`KeyFocus`（`:438`）、`AppKeyBindingsExt`（`:441`）、`InputAction` **33 个**（`:512`，上游 32 + F10 的 `CopySelection`）、`built_in()` **43 条**（`:591`）、`add_built_in`（`:701`；另生成 `Ctrl+Shift+C`、转写 Esc、Alt+1..9/Alt+0 共 12 条 → 55 个绑定）、`copy_selection_binding`（`:690`）、`InlineKey`（`:675`）、`arm_inline_keys`（`:829`）、`ask_is_visible`（`:817`）、`workspace_column_bindings`（`:788`）、`Found`/`find` 多键前缀、`released`（`:943`）、冲突拒绝 `RefusedKeys`（`:503`）、**`run_command`**（`:490`，上游 `:455`）。`KeyPressed.agent` 已由 `ShownAgent` 填（测试 `:1887`/`:1925`/`:1967`）。缺：`Reflect` 没有；`tell_refused` 走 `Notice` 的上游行为只降级成 `warn!`（`keys.rs:28`–`:29`）；插件"注册命令 + 默认键"的公开入口没有（见 §3.7 / REMAINING §10）。 |
| 14 | `markdown.rs`（193） | `rich.rs:88` + `batch::reply_lines` | ✅ | `markdown_rows` 调 `rig_tui::markdown::render(text,&theme,Some(width))`（`rich.rs:88`）；回复正文直接走上游 render（`batch.rs:1253` `reply_lines`），标题/列表/表格/引用/提示块都按上游规则。`render_marked` 没用到——它只服务上游的 `TranscriptRender` 查找标记（上游 `renderers.rs:629`），我们没有搜索高亮，见 `view`。 |
| 15 | `panel.rs`（344） | `frame.rs` + `layout.rs` + `panels.rs` + `panel.rs` | ✅ | 有：布局职责——`lay_out`/`lay_out_scrolled`（`frame.rs:395`/`:411`）、`lay_out_frame`（`:586`）、`place_panels`（`:691`）、树叶子/容器矩形（`layout.rs` 的 `Node::rects`/`Node::containers`，`:256`/`:264`）、`PanelRows`/`PanelRoots`/`PanelSpawned`（`panels.rs:123`/`:159`/`:177`）、`sync_panels`（`:298`）、`MIN_TRANSCRIPT`（`frame.rs:76`）。**开放画布已落地**（F6 切片 1/2）：`PanelRegistry`（`panel.rs:131`）、`PanelRegistry::register`/`register_with`（`:145`/`:153`；App 入口 `register_panel`/`register_panel_with` `:234`/`:241`）、`PanelId::Plugin(PluginPanel)`（`panels.rs:66`/`:40`）、`add_panel_column`（`panel.rs:271`）、`remove_panel_column`/`remove_panel_everywhere`/`unregister_panel`（`:440`/`:521`/`:541`）、`ActivePanelColumn`（`:600`）、`focus_panel_column`/`reveal_panel_column`（`:626`/`:675`）、`DrawnRow`/`drawn_row`（`:704`/`:722`）、任意面板 `StatusLine`（`status.rs:114`）。**有意不做**：`RequestRedraw`/`RedrawArea`（上游 `:82`/`:91`）与 `PanelCanvas` 的终端 buffer（`panel.rs:16`、`PANEL_CANVAS.md` §4：3D 每帧重排）。缺：插件命令绑定入口（REMAINING §10）。 |
| 16 | `pane.rs`（392） | `pane.rs` 3453 | 🟡 | 有：`PaneScroll{top,follow}`（`:145`）、窗口化 `window_transcript`（`:816`）、PgUp/PgDn/Shift+↑↓ 在 `pane_keys`（`:1081`）、滚轮（`pane_wheel` `:1320`）、"下面还有"标记（`BELOW_MARKER` `:724`）——两种标记都会画：`BelowSeen` 是资源（`transcript.rs:1047`），每个 `TranscriptPane.below` 每帧 `pane.below.of(...)`（`pane.rs:973`）判定 `Below::{More,New}`，`↓ more below` / `↓ new output` 按宽度裁剪；Alt+↑/↓ 选条目（`select_pane_item` `:1013`；块身份 `item_rows` `:118`，每帧重算 `sync_item_selection` `:1185`），选中后 Esc 清选区并把键交回输入框（`release_transcript_focus` `:1247`，测试 `:2281`/`:2411`）。**S1–S4 功能已落地**：`TranscriptPane` 组件（`:613`，`source/title/scroll/selected/below`）+ `ActiveTranscript`（`:662`）；`batch::source_rows`（`batch.rs:2044`）按 `Shown`/`Agent`/`Value` 产行，`dialogue_batch_of`（`batch.rs:929`）建存档批；`Pane2` 不再镜像（`grep -rn mirror_pane2_rows rig_niri/src` 为 0）；标题行 `TITLE_ROW_KEY`（`:734`）在 `window_transcript`（`:891`/`:906`）画，不参与滚动/条目（测试 `:3022`/`:3134`）。**F4**：每窗格滚动条（`ScrollbarModel` `:249`、`sync_transcript_scrollbars` `:461`，测试 `:3235`/`:3338`/`:3363`/`:3423`）。缺：`PaneKeys` 公开绑定（上游 `:125`）——滚动/选择键是物理 `KeyCode` 直读，插件不能重绑；`DrawnTranscript` 没有；`ShowAgent` 只由应用层 `/show`（`rig_harness_in_3d/src/show.rs:77`）提供（S4 的存档读取目前同步，见 REMAINING §12.3），`rig_niri` 没有这个公开类型（`grep -rn ShowAgent rig_niri/src` 为 0）。 |
| 17 | `pick.rs`（1098） | `picker.rs` 3825 | 🟡 | 有：`PickerInput`（`:164`）、标题/说明/过滤（`picker_rows` `:1442`）、树折叠 `unfold`、多选打勾 `tick`、`free_text`、`secret` 打码、PgUp/PgDn（`PAGE=10`）、`PickPreview` 列（`sync_preview_panel` `:975`）、点击行（`answer_picker` `:1731`）、高亮同步（`mark_highlighted` `:1019`）。**队列已完成**：`PickerQueue`（`:256`）+ `Add<Picker>` 观察者 `enqueue_picker`（`:723`）+ `sync_picker`（`:831`）。**选项键已落地**：Alt+1..9 直接选第 n 个可见项并确认、Alt+0 取消（`picker_keys` `:1102`，测试 `:2955`/`:2976`/`:2991`/`:3003`/`:3014`）；`ChoiceKeys`（`:86`）+ `ChoiceKey`（`:102`）+ `ChoiceAction`（`:124`）+ `ChoiceKeyPressed`（`:140`），测试 `:3090`/`:3123`/`:3156`/`:3186`/`:3210`；`more`/`scroll`/`picked` 都在 `PickerInput`（`:164`–`:191`）。**F3 外观**：容器/preview 走 `Look::panel` token，列表右侧纯 Node 滚动条（`spawn_scrollbar` `:737`、`sync_picker_scrollbar` `:1363`）。缺：上游的 `ChoiceSystems` 调度（上游 `:589`）和 `Tick`/`TickAll`/`TickNone`/`More`/`Answer` 变体（收窄成 `Choose`/`Cancel`/`Message`）；`in_input`/`draft`（上游 `:132`）**有意分叉**——输入框作答由 `editor::first_free_text_ask`/`answer_with_text` 直接接 Enter（REMAINING §5），`grep -rn in_input rig_niri/src` 只命中注释。 |
| 18 | `render.rs`（1543） | `frame.rs` + `pane.rs` + `panels.rs` + `picker.rs` + `complete.rs` + `selection.rs` | 🟡 | **绘制管道整体换成 bevy**（有意）：帧布局 `frame.rs:395`、窗口化 `pane.rs:816`、行→节点 `panels.rs::sync_panels`（`:298`）、补全/选择 tile（`complete.rs:1042`/`picker_rows` `picker.rs:1442`）、选区反色 `selection.rs:1088`。语义侧有：`CallStates`→`batch::live_state`（`batch.rs:571`）、`ShownConversation`→`batch::{conversation_frame,dialogue_batch}`（`batch.rs:967`/`:911`）、`asides`→conversation_frame 的 Slot、`decorate`→`transcript::request_lines` + `batch::child_calls_look`（`batch.rs:1470`）、`cells_in`→`selection::row_text`。缺：搜索高亮 `light`/`lit`（上游 `:848`/`:899`）与 `view::Highlight` 全链路（`grep -rn Highlight rig_niri/src` 只命中 `pick::Highlighted`（`picker.rs:1022`/`:1034`/`:1037`））；`TuiFrame`（上游 `:1196`，`screen_row`/`area`/`covered`）没有；`TranscriptLayout::lay_out`（上游 `:437`，无终端导出）没有；`Paced` 33ms 流式节流与 `needs_redraw`（上游 `:75`）——我们每帧全跑（bevy 自己驱动，属有意）；`inbox_lines`（上游 `:1520`）没有（没有收件箱 UI）；`compacted_line`（上游 `:797`"N messages above were compacted"）不画（`batch.rs:2643` 注释说明它需要上游 `Replaced`/会话计数；测试 `:2646` 断言这行不出现）。 |
| 19 | `renderers.rs`（1068） | `batch.rs` 4885 | 🟡 | 有：五类 `RenderKey`（Tool/Streaming/Entry/Content/Aside）与四类 `RenderInput` 全填（`conversation_frame` `batch.rs:967`/`rebuild_batch` `:599`）；`batch.pane`/`batch.agent` 真的设置；`ToolRender` 的 `theme`/`took`（`record_calls` `:470` 用 `Started` 算）/`output`/`children`（`CallTree` `:232`）/`item` 都填；`RenderOutput.{lines,decorations}`；`RenderSystems::{Render,Decorate,Edit}` 链（`BatchPlugin`，`world.run_schedule(TranscriptRender)`）。`text_rows` 直接复用上游。**`ToolRender.facts` 已接**：`published_facts`（`:541`）把内核 `Published` 的 `ToolResultContext` 填进 `facts`（`:507`/`:666`/`:1879` 三处填充），结果 entry 的 `Entry::facts` 也进同一条。缺：`RenderRevision` 从不推进（`grep -rn RenderRevision rig_niri/src` 为 0）→ 每帧全量重建；模型流参数、调用效应还没出现时的 `RenderKey::Streaming` 预画没做（TODO `:811`）；重启后旧调用的 `Published` 事实读不到（`KeptFacts` 是 rig-ecs 的 `pub(crate)`，见 `:106` TODO(past)）。 |
| 20 | `selection.rs`（54） | `selection.rs` 2459 | ✅ | 语义比上游多得多且对齐：grapheme 列（不是字节）、CJK/emoji ZWJ 只算一列、`Selection::press/drag/release`（`:261`/`:302`/`:315`）、复制文本的 `trim_end` + `join("\n")` + 整体 `trim_end` 与上游 `Selection::mark` 一致（`selected_text` `:324`）、`ranges_in`/`byte_ranges_in`/`all_ranges`（`:356`/`:387`/`:401`）、`SELECTED_MODIFIER=REVERSED`（`:63`）、鼠标系统 `pointer_selection`（`:944`）、反色绘制 `selected_row`（`:1088`）。**条目级选中**：`item`/`select_item`（`:200`/`:213`）；上游没有的超集：双击选词/三击选行（`ClickKind` `:94`、`apply` `:459`）、窗口 resize 清选区（`clear_selection_on_resize` `:788`）、复制失败写状态行）。 |
| 21 | `status.rs`（605） | `status.rs` 815 | ✅ | 有：**直接调** `rig_tui::status::line(items,width)`（`status.rs:177` 调上游 `status::line`，`:182`），所以 keep/`min_width`/"先缩后丢"与上游逐字相同；`status_rows`（`:285`）合并 `AppStatus` + agent 的 `StatusItems` + **运行中 turn 的 `StatusItems`**，后者由 `merged_status_items`（`:135`）读（测试 `:644`）；`no agent` 占位（`:97`/`:135`，测试 `:605`/`:631`）；`StatusSettings` + `[status] hide`（`:79`，测试 `:673`）；`StatusParts`/`AppStatusPartsExt`（`:212`/`:252`，测试 `:720`/`:763`）。**`StatusLine` 组件已补**（`:114`）：挂在任意面板根上，底部画一条与 `PanelId::Status` 同源的行，输入框上落在编辑器底行；`merged_status_items`/`status_line_row`（`:135`/`:177`）是两条路径的唯一来源规则；测试 `panel.rs:1273`/`:1328`/`:1360`。缺（有意）：`StatusSystems::{Show,Edit}` 两阶段不做——每帧在 `Draw` 里现算（`status.rs:41` 自注）。 |
| 22 | `theme.rs`（337） | `theme.rs` 1258 + `rich.rs:54` | ✅ | 有：**上游 17 个 band 全有字段**（`UPSTREAM_BANDS` `theme.rs:71`、`bands()` `:593`、测试 `:871`），默认值就是旧混色结果（测试 `:896`）；`shade`（`:681`）、`without_color`/`plain`（`:655`/测试 `:1098`）、`selection`（`:664`）、`changed_word`（`:672`）、`panel`/`raised`（`:699`/`:705`）、`NO_COLOR` 通路 `from_env`（`:727`/测试 `:1112`）。**明暗两套**：`ThemeKind`（`:173`）、`Theme::light`（`:753`）、`create_light_theme`（`:309`）、`light_ui_theme`（`:164`）、`install_theme`（`:282`），测试 `:929`/`:987`/`:1014`。`upstream_theme`（`rich.rs:54`）逐字段直接映射，不再混色（测试 `rich.rs:257`）；**batch 已跟我们的主题**（`upstream_theme(&Theme)`：`batch.rs:630`/`:935`/`:1550`/`:1817`，测试 `:4182`/`:4249`）。`on`/`of_terminal` 是"问终端背景色"的构造函数，3D 前端没有终端，换成固定的 3D 深底 + `from_env`；feathers 的面板表面 token 另在 `theme.rs:110`–`:120`，两套各管一层。 |
| 23 | `title.rs`（216） | `title.rs` 132 | 🔄 | OSC 133 语义提示符/OSC 2 标题/标题栈只对终端有意义，换成**窗口标题**（`WindowTitle` `title.rs:25`，`compose` `:36`）。状态机照抄：`needs you`（有 picker 或有未回答 `Ask`）/`working`（非子 agent 的 turn 在跑）/`error`（上个 turn 失败）/`idle`（`tell_state` `:71`，与上游 `:184`–`:195` 同条件）。缺：退出时恢复原标题（上游 `:209` `restore_title`）没有——窗口关闭即结束，价值低；`[title] mark_nonce_env`（上游 `:76`）不适用。 |
| 24 | `transcript.rs`（1783） | `transcript.rs` 2027 + `batch.rs` | 🟡 | 有：对话引擎 `Transcript`（`transcript.rs:736`）——按块缓存折行、宽度失效重折（`set_width`/`apply` `:749`/`:767`）、`TextId`（`:621`，含 `Message`/`Notice`/`StreamingReasoning`/`StreamingReply`/`Prompt`/`Inbox`；**没有 `Stopped` 变体**）、`BlockId`、`Slot`（`:650`）、排序键与上游 `Part` 顺序一致（`rank` `:674`）、折行用上游 `rig_tui::wrap::wrap`（`wrap_rows` 一带）、`Below`（`:999`）+ `BelowSeen`（`:1047`）；建块在 `batch::conversation_frame`（`batch.rs:967`）。**`[stopped]` 已画**：不加新变体，用 `TextId::Message { at, block: usize::MAX }`（`batch::STOPPED_BLOCK` `:216`；压入 `:1209`–`:1211`）。缺：`PastEffects`（上游 `:434`，恢复会话时旧调用的事实）——`batch.rs:106` TODO(past)；`Replaced`（上游 `:723`，被压缩移出对话的条目仍显示）——本仓只有 `batch.rs:2643` 注释提到它，没有实现（`grep -rn 'struct Replaced' rig_niri/src` 为 0）；压缩标记 `── N messages above were compacted ──`（上游 `render.rs:797`）不画；`stop_lines` 的第二种文案 `[stopped before it was answered]`（上游 `:1669`）只统一画 `[stopped]`；`TextId::Inbox` 有槽位（`:633`）但没有生产者。 |
| 25 | `view.rs`（865） | `pane.rs` + `selection.rs` + `transcript.rs` | 🟡 | 有：转写位置与宽度（`PaneScroll` + `Transcript::set_width`）、文本选择（`selection.rs:261`）与反色绘制、画出来的行（`PanelRows[Transcript]`，`pane::window_transcript` `:816`）、notice 的落点（`TextId::Notice`，应用层 `rig_harness_in_3d/src/notice.rs` 桥接）。**条目级选择能力有**：`Selection::item`/`select_item`（`selection.rs:200`/`:213`）+ `pane::select_pane_item`（`pane.rs:1013`）。**看别的来源的能力也有**：应用层 `/show`（`rig_harness_in_3d/src/show.rs:77`）改活动窗格的 `TranscriptSource`（`:236`），标题行在 `pane.rs:734`。缺：`TranscriptView`/`TranscriptItem`/`CallItem` 这套**公开 API**（上游 `:143`/`:84`/`:112`）没有——选中态是 `Selection` 里的一个 `u32` 块序号，插件读不到"当前选中哪条/有哪些条"；`Highlight`/`LineSpan`/`TextMark`/`MarkIn`/`RowWrap`（上游 `:224`/`:241`/`:207`/`:174`/`:361`）没有；`ScrollTo`/`ItemRow`（上游 `:259`/`:271`）没有公开入口；`ShowAgent`（上游 `:49`）类型没有（能力在应用层 `/show`）；`DrawnTranscript`/`DrawnRow`（上游 `:316`/`:326`/`:341`）没有（3D 读回是 `panel::DrawnRow`，只读几何）；`Notices` 资源（上游 `:165`）没有；`load_draft`/`keep_draft`（上游 `:723`/`:701`）没有。 |
| 26 | `tests.rs`（433） | — | ➖ | 上游的 TUI 集成测试（`TuiTestScreen` + 模拟按键），不是移植目标。我们的等价可测性在行是**值**：`rig_niri` 606 个内联测试 + headless `App`（`batch` 的端到端、`selection` 的指针事件、`workspaces` 的布局不变量）。 |

---

## 2. 我们的 26 个文件 → 上游

| # | 文件 | 行（实现/测试） | 对应上游 | 状态 | 一句话职责 |
|---|---|---|---|---|---|
| 1 | `batch.rs` | 4885（2361/2524） | `renderers.rs` + `render.rs` 的建批/调度 | 🟡 | 从对话 + 效应建 `RenderBatch`，跑 look，把行交给转写引擎；Ask 单独进确认 tile；`ToolRender.facts` 已接。 |
| 2 | `camera_rig.rs` | 128（128/0） | —— | ➕ | 3D 相机架（手柄/键盘），上游无对应。 |
| 3 | `clipboard.rs` | 1420（758/662） | `clipboard.rs` + `terminal::CopyToClipboard` | 🟡 | 复制选区/条目到系统剪贴板、`Copied N lines`、Ctrl+V 粘文本与图片路径、窗口拖入 `@path`（失败可见）；剪贴板图片本身未接。 |
| 4 | `complete.rs` | 1881（1206/675） | `complete.rs` | ✅ | `/` 命令、`@` 路径、插件 `CompletionSource`；候选 tile + 键位。 |
| 5 | `editor.rs` | 4384（1783/2601） | `editor.rs` + `input.rs` 的编辑动作 | 🟡 | 多行文本模型 + 历史 + grapheme 光标 + 折行显示/光标/高度同步 + F5 外壳 + free-text 输入框作答；缺组件身份与输入框内提示。 |
| 6 | `frame.rs` | 2311（1232/1079） | `panel.rs` + `screen.rs` + `render.rs` 的帧布局 | ✅ | 树叶子 → 帧内格子区域 → UI 尺寸/3D 锚点；默认屏幕 7 个叶子；`FrameSize` 跟窗口；条带视口裁剪；F1 面板 chrome 走 token。 |
| 7 | `front.rs` | 39（39/0） | `lib.rs` 的前端身份 | ✅ | `Front::new("feathers")` + `Answerer` + `ShowsPickers`。 |
| 8 | `hints.rs` | 581（224/357） | `hints.rs` | ✅ | 两张键位表 + `fit_keys` 逐字对齐上游；独立 1 行 tile，输入为空才显示。 |
| 9 | `history.rs` | 733（432/301） | `history.rs` + `jsonl.rs` | ✅ | 跨会话提示词历史（JSONL、去重、seed/record/save/rewrite）。 |
| 10 | `keys.rs` | 1984（1213/771） | `keys.rs` | 🟡 | 键位 IR、43 条内置绑定（+12 条）、33 个动作、作用域/焦点/插件绑定、内联回答键开关、`run_command`、`KeyPressed.agent` 跟 `ShownAgent`；缺 `Reflect`/`tell_refused` 的 Notice 展示/插件命令注册。 |
| 11 | `layout.rs` | 2295（1171/1124） | ——（替代 `panel.rs::Placement`） | ➕ | niri 分裂树 + `Size::Fraction` 列宽 + `placements_scrolled` + 预设宽度/移动列；L1–L4 已落地。 |
| 12 | `lib.rs` | 306（223/83） | `lib.rs` | 🟡 | `FeathersSystems` + `FeathersViewPlugin` + prelude；没有一键全套/headless `add_view`。 |
| 13 | `niri.rs` | 771（575/196） | —— | ➕ | niri 平铺几何：图层/列/瓦片、`place_tiles`、`spawn_niri_screen`（`RF_NIRI_LAYOUT` 旧演示）。 |
| 14 | `panel.rs` | 1653（815/838） | `panel.rs` 的 `TuiPanel`/`PanelCanvas` | ✅ | 插件面板注册表、按工作区加/摘列、自动工作区、活动列、`DrawnRow` 读回；任意面板 `StatusLine`。 |
| 15 | `panels.rs` | 604（380/224） | `panel.rs` 的绘制侧 | ✅ | 行是值：`PanelRows` → bevy_ui 节点（前缀增量 diff）；面板身份是内置变体 + `PanelId::Plugin`。 |
| 16 | `pane.rs` | 3453（1446/2007） | `pane.rs` | 🟡 | 转写窗口化 + 滚动/滚动条 + `↓ more below`/`↓ new output` + 标题行 + Alt+↑↓ 选条目 + 每窗格来源；缺 `PaneKeys` 公开绑定/`DrawnTranscript`。 |
| 17 | `picker.rs` | 3825（1790/2035） | `pick.rs` | 🟡 | 列表选择器（过滤/树/多选/填空/secret/preview）+ 队列 + 内置 Alt+1..9 + `ChoiceKeys` + F3 token/滚动条；缺 `ChoiceSystems`。 |
| 18 | `rich.rs` | 604（206/398） | `markdown.rs` + `highlight.rs` + `diff.rs` | ✅ | 上游三个纯值渲染器的适配层 + `upstream_theme`（给业务/插件用）。 |
| 19 | `selection.rs` | 2459（1209/1250） | `selection.rs` + `render::cells_in` + `input::moused` | ✅ | 鼠标拖选/双击/三击、grapheme 级命中、反色、复制、条目级选中身份。 |
| 20 | `spatial.rs` | 434（365/69） | —— | ➕ | 3D 卡片/漏斗/自由视角（空间 UI 的渲染侧），上游无对应。 |
| 21 | `status.rs` | 815（514/301） | `status.rs` 的裁剪 + 绘制 | ✅ | `status_rows`/`merged_status_items`/`StatusLine` 同一套来源规则；`[status] hide`/`StatusParts`/`no agent` 已有。 |
| 22 | `style.rs` | 120（120/0） | —— | ➕ | ratatui `Color` → bevy `Color` 的桥（含 256 色近似）。 |
| 23 | `theme.rs` | 1258（844/414） | `theme.rs` | ✅ | 17 个语义 band + 明暗两套 + `shade`/`without_color`/`selection`/`changed_word`/`panel`/`raised`/`from_env`/`install_theme`；batch 用 `upstream_theme` 注入。 |
| 24 | `title.rs` | 132（114/18） | `title.rs` | 🔄 | 状态机照抄，输出换成窗口标题。 |
| 25 | `transcript.rs` | 2027（1086/941） | `transcript.rs` | 🟡 | 对话引擎（块/缓存/排序/折行/finalize/notice/`[stopped]` 槽位/`BelowSeen`）+ 行 → 节点；缺 `Replaced`/`PastEffects`。 |
| 26 | `workspaces.rs` | 4060（1679/2381） | —— | ➕ | 工作区（= 会话）的绝对位姿、拖拽/放置/整理/聚焦、每工作区一份面板状态 + 横向 `WorkspaceScroll`。 |

**统计**：✅ 11（`complete` `frame` `front` `hints` `history` `panel` `panels` `rich` `selection` `status` `theme`）｜🟡 8（`batch` `clipboard` `editor` `keys` `lib` `pane` `picker` `transcript`）｜🔄 1（`title`）｜➕ 6
（`camera_rig` `layout` `niri` `spatial` `style` `workspaces`）。
表里行数是 2026-11-26 08:2x 工作树；**行号以函数名为准**。
`layout.rs`/`niri.rs` 不是"上游没有这件事"：它们替代的是 `panel.rs::Placement` + `render::layout` 的
布局职责，所以在 §1 里算进 `panel`/`render` 的"部分"，而不单列状态。

### 2.1 本轮新增的能力（用户可感知，不在上游模块表里）

这些不是新"上游模块"，但都是本轮落地、上一版 PARITY 没有记的能力；逐条给落点与测试。

| 能力 | 落点（当前行号） | 证据 / 测试 |
|---|---|---|
| `/show` 切来源（子 agent / 存档会话）+ 窗格标题行 | `rig_harness_in_3d/src/show.rs:77`（命令）、`:236`（写活动窗格 `source`/`title`）、`:292`（picker）；`batch::source_rows`（`batch.rs:2044`）、`dialogue_batch_of`（`:929`）；标题行 `pane.rs:734`/`:891` | `show.rs:479`（命令注册 + 无参开 picker）、`:514`（选 agent、`shown` 恢复）、`:565`（两窗格各自来源/窗口化）、`:638`（标题行不破坏滚动/条目）、`:696`（存档会话读成 `Value`） |
| `/theme dark\|light` | `rig_harness_in_3d/src/theme_cmd.rs:47`；`theme.rs:173`（`ThemeKind`）、`:282`（`install_theme`）、`:753`（`Theme::light`）、`:309`（`create_light_theme`） | `theme_cmd.rs:150`/`:161`/`:187`/`:237`；`theme.rs:929`/`:987`/`:1014` |
| 插件面板画布（F6 切片 1/2） | `panel.rs:131`（注册表）、`:271`（加列）、`:440`（摘列）、`:626`（活动列）、`:722`（`drawn_row`）；`panels.rs:40`/`:66`（`PluginPanel`/`PanelId::Plugin`）；`status.rs:114`（`StatusLine`） | `panel.rs:956`（注册变列 + 3D 根）、`:1032`（行世界矩形）、`:1273`/`:1328`/`:1360`（`StatusLine`）、`:1430`/`:1480`（自动工作区/整树移除） |
| `ImageNode` 3D 镜像（F8） | `feathers_in_3d/src/world3d.rs:186`（`PanelImageNode`）、`:209`（`image_uv_rect`）、`:241`（`image_tint`）、`:256`（`image_quad_rect`）；镜像在 `sync_mirror`（`:321` 一带） | `world3d.rs:1114`/`:1153`/`:1169`/`:1188`/`:1316`/`:1363` |
| 键盘复制（F10，`Ctrl+Shift+C`） | `clipboard.rs:273`（`copy_selection_keys`）、`keys.rs:690`（`copy_selection_binding`，Global 作用域） | `clipboard.rs:989`（条目）、`:1021`（鼠标选区）、`:1041`（无选中不写剪贴板）、`:1097`（活动窗格） |
| 图片路径粘贴 / 窗口拖入 | `clipboard.rs:486`（`image_paths`）、`:499`（`attach_image_paths`）、`:525`（`dropped_file_text`）、`:663`（`paste_text`）；应用层 `rig_harness_in_3d/src/lib.rs:389`（`drop_image_files`） | `clipboard.rs:1216`/`:1255`/`:1280`/`:1299`/`:1313`/`:1324`/`:1339`/`:1354`；`rig_harness_in_3d/src/lib.rs:932`/`:951` |
| free-text 在输入框作答 | `editor.rs:978`（`first_free_text_ask`）、`:1049`（`answer_with_text`）、`:1009`（`AnswerSeeded`）、`:1385`（Enter 分支） | `editor.rs:2735`/`:2759`/`:2779`/`:2833`/`:2852`/`:2870`/`:2898` |
| 条带竖直重叠修复（间距按帧高） | `workspaces.rs:1203`（`workspace_strip_height`）、`:1208`（`workspace_step`）；应用层 `workspaces_demo.rs:557`（`restack_on_resize`） | `workspaces_demo.rs:654`（resize 后不重叠）、`:692`（手放的不动）、`:730`（只重排未手放的） |

---

## 3. 🟡 项逐条：缺什么、缺在哪个函数

> 只列**公开能力**的缺口。"上游"指 rig-tui 0.47.0 的同名函数/类型。

### 3.1 `lib.rs`（装配）

- **没有 `add_view`**：上游 `add_view`（`lib.rs:260`）在无终端时装配 input/keys/pickers/转写，
  测试用 `TuiTestScreen` 顶替终端。我们只能整装 `FeathersViewPlugin`；headless 测试靠单个插件
  （`PanePlugin`/`BatchPlugin`/`StatusPlugin` 各自能跑，见 `rig_harness_in_3d/src/lib.rs:464` 的 `headless_app` / `:481` 的 `engine_app`）。
- **没有可选 part 的装配器**：上游 `TuiPlugins`（`lib.rs:133`）用 `is_plugin_added` 去重，逐个加
  `StatusLinePlugin`/`KeyHintsPlugin`/`TerminalTitlePlugin`。我们的 `StatusPlugin`/`HintsPlugin`/
  `TitlePlugin` 是固定装进 `FeathersViewPlugin` 的（`lib.rs:190`–`:204`）。
- **`keeps_files("tui")` / `BuildFailedLooks`**：上游 `lib.rs:180`/`:398`。我们没有会话文件登记；
  构建失败的红色提示 look 也没有。
- **`BatchPlugin` 不在视图插件里**：`FeathersViewPlugin::build` 的插件元组没有它
  （`lib.rs:190`–`:204`），应用必须自己加（`rig_harness_in_3d/src/main.rs:68`、`src/lib.rs:492`）。
- **turn 级 `StatusItems` 已注册**（不是缺口）：`finish` 给 `Agent` 与 `TurnOf` 都注册
  （`lib.rs:216`–`:217`），`merged_status_items` 按 `ActiveTurn` 读 turn 的条目（`status.rs:135`）。

### 3.2 `clipboard.rs`（图片剪贴板）

- **文本粘贴已通**：Ctrl+V 绑定是 `InputAction::PasteImage`（`keys.rs:659`），
  `act` 把它变成 `Act::Paste`（`editor.rs:1150`），由 `editor::keys` 调 `clipboard::paste_from`
  （`editor.rs:1435`，实现 `clipboard.rs:726`）；失败写 `PASTE_FAILED`（`clipboard.rs:126`/`:129`），
  成功清掉上一次错误（`editor.rs:1442`）。
- **图片路径已接**：`image_paths`（`clipboard.rs:486`）识别文本里的图片文件路径，
  `attach_image_paths`（`:499`）只在**整段都是图片路径**时换成 `@path`（混有普通文字原样保留，
  和上游一致）；`paste_text`（`:663`）先做这个替换；窗口拖入由应用层 `drop_image_files`
  （`rig_harness_in_3d/src/lib.rs:389`）调 `dropped_file_text`（`:525`）+ `type_path`（`:700`）。
- **图片粘贴取不到图**：上游 `Do::PasteImage` 要 `Scratch::images` + 进程/落盘
  （上游 `input.rs:863`–`:877`）；我们的 `bevy_clipboard` 没开 `image` feature，
  `read_paste` 只读文本（`clipboard.rs:467`），`Paste::Image` 没有生产者（模块文档 `:24`–`:27`），
  应用也没有把图片写进 `Scratch::images`（上游 `host/session.rs:170`）。

### 3.3 `editor.rs`（组件身份与输入框内提示）

- **多行显示已完成**：`layout`（`editor.rs:672`）/`editor_rows`（`:1591`）按列宽折行，
  光标落在真实行列（`CURSOR` `:266`/`with_cursor` `:299`），`MOST_LINES=10`（`:115`）之后随光标滚动，
  tile 高度由 `input_tile_height`（`:181`）+ `sync_editor_tile`（`:1573`）同步；测试
  `:3186`/`:3249`/`:3510`/`:3586`/`:3698`。
- **F5 输入框外壳已完成**：底色/圆角/行高/字号/水平内边距/焦点环来自 feathers token
  （`sync_input_field` `:1638`、`sync_input_row_height` `:1701`、`INPUT_ROW_HEIGHT` `:165`）。
- **在输入框里作答已完成**：free-text `Ask` 时 Enter 用输入框文本作答
  （`first_free_text_ask` `:978` → `answer_with_text` `:1049`，`AnswerSeeded` `:1009` 只填一次
  `initial`）；选项型问题不抢 Enter（测试 `:2779`）。
- **仍缺**：`Editor` 是 `Resource`（`:100`），上游是 `Component` + `#[require(Caret)]`
  （上游 `:61`/`:92`），并有 `MainEditor`（上游 `:83`）/`MainInput` 让插件用
  `Single<&mut Editor, With<MainEditor>>`；上游 `answer`/`answering`/`take_answer`
  （上游 `:188`/`:202`/`:208`）没有同名 API。
- **输入框内的按键提示**：上游在 `draw_editors`（上游 `:686`）里按状态换（recalled / 面板拿走键 /
  正在作答 / 空输入），我们只有独立的 `PanelId::Hints` 一行（`hints.rs:183`），按"输入空 + turn
  是否在跑"切换；这属于小差异，见 `REMAINING.md`。

### 3.4 `hints.rs`（已对齐，不再是缺口）

- `fit_keys` 三处行为**已逐字对齐**（`hints.rs:73`）：整行 `format!(" {} ", …)` 带首尾空格、
  宽度用 `unicode_width`、同权重丢**最后**一个（`Reverse(at)`）；测试 `:288`（首尾空格）、
  `:302`（unicode 宽度）、`:312`（同权重丢最后）、`:321`（能放就全放）。
- 显示时机也对齐：只在输入为空时显示；非空时写一行空行、tile 保持固定 1 行高
  （`hint_tile_rows` `:159`、`hint_panel_rows` `:183`，测试 `:432`/`:451`）。
- 唯一差异是 `Ctrl+V paste` 文案（`:51`/`:59`）：它跟当前实际行为走，剪贴板图片（REMAINING §4）
  做完后改回上游的 `image`。

### 3.5 `input.rs`（键 → 会话：细粒度部分）

- **已接**（不再是缺口）：`Send` → `Deliver::new(agent, text, DeliveryMode::Steer)`（`editor.rs:1157`
  → `:1418`）；turn 跑着时 `Tab` → `Queue`（`editor.rs:1154`）；`Esc` → `StopTurn::new(agent)`
  （`editor.rs:1429`）；空输入 `Ctrl+D` → `AppExit`（`editor.rs:1427`）；Alt+1..9 → `Act::AnswerInline`
  （`editor.rs:1143` → `answer_shown_ask` `:1065`）；Alt+0 → `Act::DismissInline`（`:1144` →
  `refuse_shown_ask` `:1087`）；有未回答 `Ask` 时 Esc 先拒绝（`editor.rs:1364`–`:1368`）。
- **过滤/上下文**：没有 `InputSystems::Filter`（插件在绑定之前读/改/丢键）、`TuiKeys` 每键一次的
  调度（上游 `input.rs:111`–`:116`）、`InputContext`（上游 `:130`）。我们的 `editor::keys`
  （`editor.rs:1280`）一次读完这一帧的 `KeyboardInput` 直接分派。
- **条目选择的键位通道**：转写区的 Alt+↑/↓、Shift+↑/↓、PgUp/PgDn 在 `pane_keys`（`pane.rs:1081`）
  里**直接读物理键**，`InputAction::{SelectUp,SelectDown,Deselect}` 在 `act` 里仍是空实现
  （`editor.rs:1136`–`:1138`）——能力有，插件不能重绑。
- **`Deliver::literal`**：本 crate 没有 `refused` 概念，所以没有"命令被拒后当普通消息重发"
  （`editor.rs:1184`、`:1414`）。

### 3.6 `keys.rs`（插件侧）

- **`run_command` 已移植**：`run_command`（`keys.rs:490`）返回 `KeyPressed` 观察者，按下时对
  `KeyPressed.agent` 跑 `line`；被拒时写一条错误 `Notice`、不回填输入。测试
  `run_command_runs_the_line_for_the_shown_agent`（`:1802`）、
  `run_command_does_nothing_without_a_shown_agent`（`:1830`）、
  `a_refused_run_command_is_an_error_notice_for_the_agent`（`:1850`）。应用层把输入框 `/` 开头的
  提交转成 `RunCommand` 的桥不变（`rig_harness_in_3d/src/lib.rs:362`）。
- **`KeyPressed.agent` 已填显示中的 agent**：`:1887`（跟随切换）、`:1925`（松开也填）、
  `:1967`（没有 `ShownAgent` 时 `None`）。
- **`Reflect` / `tell_refused`**：上游这些组件都 `derive(Reflect)` 供 `/plugins`、场景保存；
  键位冲突我们只 `warn!` + `RefusedKeys`（`keys.rs:503`、`:28`–`:29`），上游还会发 `Notice`。
- **插件命令注册**：仍缺"插件注册自己的命令 + 默认键"的 API（`KeyBinding`/`InputAction`/
  `run_command` 都有，只是没有注册表），见 REMAINING §10。

### 3.7 `panel.rs` / `panels.rs`（开放画布：已落地，剩命令绑定）

- **插件能画自己的面板**（F6 切片 1/2）：`register_panel`/`register_panel_with`
  （`panel.rs:234`/`:241`，内部 `:145`/`:153`）、`add_panel_column`（`:271`）、`add_panel_everywhere`（`:374`）、
  `remove_panel_column`/`remove_panel_everywhere`/`unregister_panel`（`:440`/`:521`/`:541`）、
  `drawn_row`（`:722`）。`PanelId::Plugin`（`panels.rs:66`）只扩了一个变体，内置面板分支不变。
- **任意面板状态行已落地**：`StatusLine`（`status.rs:114`）挂哪块画哪块，输入框上落在编辑器底行
  （测试 `panel.rs:1273`/`:1328`/`:1360`）。
- **有意不做**：`RequestRedraw`/`RedrawArea`（上游 `:82`/`:91`）与 `PanelCanvas` 的终端 buffer
  （`panel.rs:16`、`PANEL_CANVAS.md` §4：`bevy_ui` 每帧重排、`sync_mirror` 每帧镜像，没有省一帧的收益）。
- **仍缺**：插件命令绑定入口（REMAINING §10 切片 3）。

### 3.8 `pane.rs`（多来源：S1–S4 功能已落地）

- **多来源窗格已完成**：`TranscriptPane` 组件（`pane.rs:613`，含 `source`/`title`/`scroll`/`selected`/
  `below`）+ `ActiveTranscript`（`:662`）；`batch::source_rows`（`batch.rs:2044`）按
  `Shown`/`Agent`/`Value` 各取各的行，`window_transcript`（`pane.rs:816`）按每块窗格自己的来源
  窗口化；`Pane2` 不再镜像（`grep -rn mirror_pane2_rows rig_niri/src` 为 0）。
- **入口在应用层**：`/show`（`rig_harness_in_3d/src/show.rs:77`）改**聚焦工作区活动窗格**的
  `source`/`title`，不动 `ShownAgent`（测试 `show.rs:514`/`:565`）；标题行 `TITLE_ROW_KEY`
  （`pane.rs:734`）不参与滚动/条目。
- **公开的滚动/选择键**：Alt+↑/↓ 选条目、Shift+↑/↓ 逐行、PgUp/PgDn 已经实现（`pane_keys`
  `:1081`），但走的是物理 `KeyCode` 而不是 `PaneKeys` 绑定（上游 `:125` 的 6 条绑定），
  所以插件不能改键；`ScrollTo`/`ItemRow` 没有公开入口。
- **两种"下面还有"标记都已画**：`BelowSeen` 是资源（`transcript.rs:1047`），每个窗格的
  `TranscriptPane.below` 每帧 `pane.below.of(...)`（`pane.rs:973`）决定 `Below::{More,New}`，标记文案来自
  `Below::marker`（`transcript.rs:1010`/`:1012`），按窗格宽度裁剪。
- **F4 滚动条**：`ScrollbarModel`（`pane.rs:249`）+ `sync_transcript_scrollbars`（`:461`）+
  拖拽观察者（`:531`/`:573`/`:596`），测试 `:3235`/`:3338`/`:3363`/`:3423`。

### 3.9 `pick.rs` / `picker.rs`（公开选项键；输入框作答已分叉到 editor）

- **队列已完成**：`PickerQueue`（`picker.rs:256`）+ `Add<Picker>` 观察者 `enqueue_picker`（`:723`）
  + `sync_picker`（`:831`）；测试 `:3546`/`:3628`/`:3663`/`:3695`/`:3743`。
- **内置 Alt+1..9 已落地**：`picker_keys`（`:1102`）在 picker 显示时接管；测试 `:2955`/`:2976`/
  `:2991`/`:3003`/`:3014`。
- **`ChoiceKeys`/`ChoiceAction` 已移植（部分）**：`ChoiceKeys`（`:86`）+ `ChoiceKey`（`:102`）+
  `ChoiceAction`（`:124`，收窄成 `Choose(n)`/`Cancel`/`Message(String)`）+ `ChoiceKeyPressed`
  （`:140`）；测试 `:3090`/`:3123`/`:3156`/`:3186`/`:3210`。**还缺**：上游的 `ChoiceSystems`
  调度（上游 `:589`）与 `Tick`/`TickAll`/`TickNone`/`More`/`Answer` 变体（用 `Message` 由插件自己做）；
  树里的 `more` 没键。
- **输入框作答**：有意不在 picker 这条路上——free-text `Ask` 由 `editor.rs` 直接接 Enter
  （REMAINING §5），所以 `in_input` 在 `rig_niri` 里为 0 是分叉，不是缺口。
- **F3 外观**：容器/preview 走 `Look::panel`，列表右侧纯 Node 滚动条（`spawn_scrollbar` `:737`、
  `sync_picker_scrollbar` `:1363`），测试 `:2657`/`:2790`。

### 3.10 `render.rs`（非绘制部分）

- **搜索高亮**：`view::Highlight`/`LineSpan` + `render::Lights`/`light`/`lit`（上游 `render.rs:848`/
  `:899`）全链路 0 命中，`grep -rn "Highlight" rig_niri/src` 只命中 `pick::Highlighted`。
- **帧导出**：`TuiFrame`（上游 `:1196`，`screen_row`/`area`/`covered`）没有；`TranscriptLayout`
  （上游 `:437`，无终端导出转写行）没有——插件拿不到"第 n 行在屏幕哪"（3D 读回是
  `panel::DrawnRow` 的世界矩形）。
- **收件箱/压缩提示**：`inbox_lines`（上游 `:1520`）没有生产者（本前端没有收件箱 UI）；
  `compacted_line`（上游 `:797`）不画（`batch.rs:2643` 注释；测试 `:2646` 断言不出现）。

### 3.11 `renderers.rs`（版本与流式；事实已接）

- **`facts` 已接**：`published_facts`（`batch.rs:541`）把内核 `Published` 的
  `ToolResultContext` 填进 `ToolRender.facts`；结果 entry 的 `Entry::facts` 也进同一条。
  剩余限制：`KeptFacts` 是 rig-ecs 的 `pub(crate)`，重启后旧调用的 `Published` 事实读不到
  （`batch.rs:106` TODO(past)）。
- **`RenderRevision`**：0 命中 → 每帧全量 `rebuild_batch`（`batch.rs:599`）+ `apply`
  （`transcript.rs:767`），diff 靠内容比较兜住，但大对话的 CPU 成本比上游高。
- **流式预画**：模型流参数、调用效应还没出现时，上游先画 `CallState::Streaming` 的卡片；
  我们等效应出现（`batch.rs:811` TODO(streaming)）。

### 3.12 `status.rs`（状态行：`StatusLine` 已补）

- **`StatusLine` 组件 / 任意面板状态行已落地**（`status.rs:114`）：挂在任意面板根上，
  底部画一条与 `PanelId::Status` 同源的行；`merged_status_items`（`:135`）/`status_line_row`
  （`:177`）是唯一来源规则，`StatusParts` 覆盖同名、`[status] hide` 过滤、`no agent` 占位都在。
  测试 `panel.rs:1273`/`:1328`/`:1360`。
- **有意不做**：`StatusSystems::{Show,Edit}` 两阶段（上游 `:216`）——状态行每帧在 `Draw` 里现算
  （`status.rs:41` 自注）；视觉按 feathers token，不再逐字对齐终端。

### 3.13 `theme.rs`（已对齐，不再是缺口）

- **17 个 band + 方法都有**：`UPSTREAM_BANDS`（`theme.rs:71`）、`bands()`（`:593`）、`shade`
  （`:681`）、`without_color`/`plain`（`:655`）、`selection`（`:664`）、`changed_word`（`:672`）、
  `panel`/`raised`（`:699`/`:705`）、`from_env`（`:727`，只认 `NO_COLOR` 与 `RF_THEME`）；
  测试 `:871`/`:896`/`:1098`/`:1112`/`:1120`/`:1139`/`:1152`/`:1177`/`:1190`/`:1234`。
- **明暗两套**：`ThemeKind`（`:173`）、`Theme::light`（`:753`）、`create_light_theme`（`:309`）、
  `install_theme`（`:282`）；测试 `:929`/`:987`/`:1014`。
- **batch 已跟我们的主题**：`upstream_theme(&Theme)`（`rich.rs:54`；batch 侧 `batch.rs:630`/`:935`/`:1550`/`:1817`），`NO_COLOR` 走同一条（测试 `rich.rs:257`）。浅色主题的小字号对比度是已知观感限制
  （`FEATHERS_UI.md` §5.5），不是内容移植缺口。

### 3.14 `transcript.rs`（几处可见缺口）

- **`PastEffects`**（上游 `:434`）：恢复会话时从效应日志重建旧调用的事实（`took`、调用树、
  子 agent 的调用）。`batch.rs:106` TODO(past)；后果：重启后旧卡片没有时长/调用树。
- **`Replaced`**（上游 `:723`）：压缩移出对话的条目仍显示；本仓只有 `batch.rs:2643` 一行注释
  提到它，没有实现（`grep -rn 'struct Replaced' rig_niri/src` 为 0）；压缩摘要也不画。
- **停止标记只画一种**：`[stopped]` 已画（`batch.rs:1209`–`:1211`，用
  `TextId::Message { at, block: usize::MAX }` = `batch::STOPPED_BLOCK` `:216`），上游
  `stop_lines`（`transcript.rs:1669`）还有 `[stopped before it was answered]`。
  **`TextId::Inbox`** 有槽位（`transcript.rs:633`）无生产者。

### 3.15 `view.rs`（公开选择 API / 导出）

- **条目级选择**：能力有（`Selection::item`/`select_item` `selection.rs:200`/`:213` +
  `pane::select_pane_item` `:1013`），但**公开 API 没有**：`TranscriptView::selected` 不存在，
  插件读不到"第 n 个调用/当前选中哪条"（上游 `:143`/`:84`/`:112`）。
- **搜索/标记**：`Highlight`/`LineSpan`/`TextMark`/`MarkIn`/`RowWrap`（上游 `:224`/`:241`/`:207`/
  `:174`/`:361`）没有，配套的 `render::light` 也没有（见 3.10）。
- **`ScrollTo`/`ItemRow`**（上游 `:259`/`:271`）、**`ShowAgent`**（上游 `:49`，能力在应用层
  `/show`）、**`DrawnTranscript`**（上游 `:316`）、**`Notices` 资源**（上游 `:165`）、
  **`load_draft`/`keep_draft`**（上游 `:723`/`:701`）：都没有。notice 的显示是应用层
  `rig_harness_in_3d/src/notice.rs` 桥到 `TextId::Notice`，前端没有 `Notices` 资源。

---

## 4. 有意替换 / 不适用（不是缺口）

### 4.1 `terminal.rs`：整条终端管道换成窗口

| 上游能力 | 终端实现 | 我们的取代 |
|---|---|---|
| 裸模式/备用屏/光标隐藏/鼠标捕获/括号粘贴 | crossterm + `Terminal<CrosstermBackend>` | bevy 窗口 + winit 输入（不需要这些模式） |
| 每帧合成到 `Buffer` | `render.rs` 写格子 | bevy_ui 节点 + `feathers_in_3d`（`panels.rs::sync_panels`） |
| 复制 | OSC 52 + base64 | `bevy_clipboard` 系统剪贴板（`clipboard.rs:233`/`:244`），F10 键盘复制 `copy_selection_keys`（`:273`） |
| 尺寸/焦点事件 | crossterm resize/focus | bevy `Window`：`sync_frame_size_to_window`（`frame.rs:553`）+ 物理→逻辑像素换算（`feathers_in_3d/src/world3d.rs:101`–`:122`） |
| `TuiTestScreen` | 内存 `Buffer` 断言 | 行是值：`PanelRows` + headless `App`（606 个测试） |
| `TerminalWrite`（任意转义序列，如桌面通知） | 帧后写 stdout | 无对应物；插件要通知得走 bevy 消息（可接受） |
| `RunInTerminal`（跑 `$EDITOR`） | 交还 tty 给子进程 | **没有替代**；窗口版要"挂起输入 + 恢复重排 + 退出码事件"，价值低，见 `REMAINING.md` 终端专有 |

### 4.2 `title.rs`：OSC 标题 → 窗口标题

OSC 133 提示符标记、OSC 2 标题、标题栈 push/pop（`PUSH_TITLE`/`POP_TITLE`）只对终端有意义；
窗口前端用 `WindowTitle`（`title.rs:25`）写 `Window::title`，状态机照抄（§1 #23）。
退出恢复原标题（上游 `:209`）不做：窗口关闭即结束。

### 4.3 `tests.rs`：不是移植目标

上游 433 行是 TUI 集成测试（`TuiTestScreen` + 模拟按键）。我们不需要等价物：行/选区/面板区域
都是值，测试直接断言值（`batch` 的 headless 端到端、`selection` 的指针事件、`workspaces` 的布局
不变量）。

---

## 5. 本轮被推翻的旧结论

> 只列**能力级**的翻案；更早两版（11-25 22:50 / 11-26 00:5x）的逐条对照已并入 §1/§3，
> 带旧行号的表不再维护（旧行号全部失效）。

| 旧文（01:0x 快照） | 现在的证据 |
|---|---|
| "`pane`：`TranscriptSource::{Agent,Value}` 没有产行路径，两块窗格 `source` 都是 `Shown`，`Pane2` 仍由 `mirror_pane2_rows` 每帧复制——'看子 agent / 看存档'仍缺" | S1–S4 功能已落地（S4 的异步读取见 REMAINING §12.3）：`batch::source_rows`（`batch.rs:2044`）按三种来源产行、`dialogue_batch_of`（`:929`）建存档批；`/show` 改活动窗格 `source`/`title`（`rig_harness_in_3d/src/show.rs:77`/`:236`）；`mirror_pane2_rows` 0 命中；标题行 `pane.rs:734`/`:891`。测试 `show.rs:514`/`:565`/`:638`/`:696`。 |
| "`clipboard`：拖入/粘贴出来的文件路径没解析——上游 `dropped_images`/`image_file`/`local_url_path`/`percent_decoded` 在 `grep -rn dropped_images rig_niri/src` 是 0" | 已接：`image_paths`（`clipboard.rs:486`）、`attach_image_paths`（`:499`）、`dropped_file_text`（`:525`）；`paste_text`（`:663`）替换纯附件粘贴，窗口拖入 `rig_harness_in_3d/src/lib.rs:389`。测试 `clipboard.rs:1216`/`:1280`/`:1313`/`:1354`、`rig_harness_in_3d/src/lib.rs:932`/`:951`。 |
| "`editor`：在输入框里作答缺 `answer`/`answering`/`take_answer`，`grep -rn 'in_input' rig_niri/src` 为 0" | 能力已落地（换实现）：free-text `Ask` 的 Enter 走 `first_free_text_ask`（`editor.rs:978`）→ `answer_with_text`（`:1049`），`AnswerSeeded`（`:1009`）保证 `initial` 只填一次；测试 `:2735`/`:2759`/`:2833`/`:2852`/`:2870`。`in_input` 仍为 0 是有意分叉，不再是缺口。 |
| "`panel`：插件画不了自己的面板——上游 `TuiPanel`/`PanelCanvas`，我们只有封闭 `PanelId`，加面板要改核心枚举" | F6 切片 1/2 已落地：`PanelRegistry`（`panel.rs:131`）、`add_panel_column`（`:271`）、`drawn_row`（`:722`）；`PanelId::Plugin`（`panels.rs:66`）只扩一个变体；测试 `panel.rs:956`/`:1032`/`:1430`/`:1480`。 |
| "`status`：缺 `StatusLine` 组件，只有固定 `PanelId::Status`" | `StatusLine`（`status.rs:114`）挂在任意面板根上，底部一条与内置状态行同源；测试 `panel.rs:1273`/`:1328`/`:1360`。 |
| "`picker`：不能用 Alt+1..9 选，只能点；没有队列" | `PickerQueue`（`picker.rs:256`）+ `enqueue_picker`（`:723`）+ `sync_picker`（`:831`）；`picker_keys`（`:1102`）接管 Alt+1..9/Alt+0；测试 `:2955`/`:3546`/`:3663`。 |
| "`[stopped]` 不画、`↓ new output` 不画" | `[stopped]`：`batch.rs:1209`–`:1211`（`STOPPED_BLOCK` `:216`），测试 `batch.rs:2576`/`:2606`/`:2626`；`Below::New`：`transcript.rs:1012` + `pane.rs:816`，测试 `pane.rs:1729`/`:1765`。 |
| "F8 `ImageNode` 不镜像；F10 键盘复制没做；F7 主题切换没做；L1 进行中" | `ImageNode`：`world3d.rs:186`/`:209`/`:241`/`:256`，测试 `:1114`/`:1188`；键盘复制：`clipboard.rs:273` + `keys.rs:690`，测试 `:989`/`:1021`/`:1097`；主题：`theme_cmd.rs:47` + `theme.rs:173`/`:282`/`:753`，测试 `theme_cmd.rs:187`；L1–L4：`NIRI_SCROLL.md` 状态表（`layout.rs:99`/`:739`/`:302`，`workspaces.rs:110`/`:406`/`:630`）。 |
| "`rig_niri` 526 个测试（L1 并行时 528）" | 现在 **606**（`cargo test --workspace --offline`，2026-11-26 08:2x；app lib 36 + bin 5、`feathers_in_3d` 15）。见 §0/§6。 |

---

## 6. 复现

```sh
cd /var/home/lichi/桌面/bevy_3d_ai_research
UP=.cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/rig-tui-0.47.0/src

# 文件数 / 行数
find rig_niri/src -name '*.rs' | wc -l                      # 26
find rig_niri/src -name '*.rs' -print0 | xargs -0 wc -l | tail -1   # 43162
find "$UP" -name '*.rs' | wc -l                              # 46（25 实现 + 21 tests.rs）
find "$UP" -name '*.rs' ! -name 'tests.rs' -print0 | xargs -0 wc -l | tail -1  # 14316
find "$UP" -name 'tests.rs' -print0 | xargs -0 wc -l | tail -1        # 9395

# 测试数
grep -rh '#\[test\]' rig_niri/src | wc -l                    # 606
grep -rh '#\[test\]' "$UP" | wc -l                           # 246

# 实现/测试行拆分（从 `#[cfg(test)]` + `mod tests` 算；不要被 `#[cfg(test)] use` 骗到）
for f in rig_niri/src/*.rs; do
  s=$(awk '/^#\[cfg\(test\)\]$/{n=NR; getline; while($0=="") getline; if ($0 ~ /^mod tests/) {print n; exit}}' "$f")
  [ -n "$s" ] && echo "$f impl=$((s-1)) test=$(($(wc -l < "$f")-s+1))" || echo "$f impl=$(wc -l < "$f") test=0"
done

# 测试
export CARGO_HOME=$PWD/.cargo-home
cargo test --workspace --offline      # rig_niri 606、app lib 36、bin 5、feathers 15；doctests：rig_niri 2 passed/2 ignored、app 1、feathers 0

# 本轮新增能力的锚点，逐条可查
grep -n "pub fn show(" rig_harness_in_3d/src/show.rs                   # 77
grep -n "pub fn theme(" rig_harness_in_3d/src/theme_cmd.rs              # 47
grep -n "pub fn source_rows" rig_niri/src/batch.rs                      # 2044
grep -n "pub const TITLE_ROW_KEY" rig_niri/src/pane.rs                  # 734
grep -n "pub struct PanelRegistry\|pub fn add_panel_column\|pub fn drawn_row" rig_niri/src/panel.rs  # 131 / 271 / 722
grep -n "pub struct StatusLine" rig_niri/src/status.rs                  # 114
grep -n "pub fn copy_selection_keys" rig_niri/src/clipboard.rs          # 273
grep -n "pub fn image_paths\|pub fn attach_image_paths\|pub fn dropped_file_text" rig_niri/src/clipboard.rs  # 486 / 499 / 525
grep -n "pub enum ThemeKind\|pub fn install_theme\|pub fn light()" rig_niri/src/theme.rs  # 173 / 282 / 753
grep -n "pub struct PanelImageNode" feathers_in_3d/src/world3d.rs       # 186
grep -n "fn restack_on_resize" rig_harness_in_3d/src/workspaces_demo.rs # 557
grep -n "pub fn first_free_text_ask\|fn answer_with_text" rig_niri/src/editor.rs  # 978 / 1049

# 仍然缺的（应为 0 或只剩 TODO/注释）
grep -rn "struct Replaced" rig_niri/src                        # 0
grep -rn "PastEffects" rig_niri/src                            # 只有 batch.rs:106 TODO
grep -rn "RenderRevision\|TuiFrame\|TranscriptView\|ShowAgent" rig_niri/src  # 0（ShowAgent 只在应用层 show.rs）
grep -rn "fn consume\|fn expel" rig_niri/src                   # 0（niri 列吞并/弹出未做，见 REMAINING §12）
grep -rn "ChoiceSystems" rig_niri/src                          # 0
```

---

## 7. 怎么读这份文档

1. 想知道"某个能力有没有"：先看 §1 的对应上游模块，再看 §3 的同名小节；本轮新增的能力先看 §2.1。
2. 想知道"还差什么、先做哪个"：看 [`REMAINING.md`](REMAINING.md)（按用户能感知的价值排序）。
3. 想改代码前确认边界：§2 的"我们的 26 个文件"给每个文件的职责与上游对应；6 个 ➕ 文件
   是自有设计（`layout`/`niri`/`workspaces`/`spatial`/`camera_rig`/`style`），改它们不算"偏离上游"。
4. 文档里的数字都能用 §6 的命令重新数；`文件:行` 是 2026-11-26 08:2x 快照的位置，
   行号对不上时以函数/类型名为准（见开头的快照警告）。
5. 视觉/布局的下一阶段看 [`../design/FEATHERS_UI.md`](../design/FEATHERS_UI.md)（F1–F8/F10 已完成，
   F9 未做）与 [`../design/NIRI_SCROLL.md`](../design/NIRI_SCROLL.md)（L1–L4 已完成，L5 未做）。
