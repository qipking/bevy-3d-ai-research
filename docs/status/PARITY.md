# rig_niri ↔ rig-tui 移植核对（2026-11-25 第二次审计）

> **状态**：当前。本文**整篇重写**，取代 2026-11-23 的首次审计及其后追加的三段进度日志。
> 旧版开头的"4943 行 / 17 个文件 / 只有 1 个模块已移植"和"没有键位机制、没有输入历史、
> 没有补全、没有选字/剪贴板、回复是写死的一句中文"**全部失效**，不要再引用；差异清单见 §5。
>
> **基准**：上游 `rig-tui 0.47.0`（`.cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/rig-tui-0.47.0/src/`，下称"上游"）
> vs 我们 `rig_niri 0.1.0`（`rig_niri/src/`）。
> **方法**：逐文件读两边源码（grep/read），结论只来自代码；数字来自 `wc -l`、`grep -c '#\[test\]'`、`cargo test`
> （复现命令见 §6）。不沿用旧文档的任何结论。
> **口径**：
> ✅ **已移植** = 该模块对外的职责已完整落地，实现可以换；
> 🟡 **部分** = 主路径在，但公开能力有明确缺口（§3 逐条写"缺什么、缺在哪个函数"）；
> ❌ **未开始**；
> 🔄 **有意替换** = 终端管道，用 bevy 窗口/渲染取代；
> ➕ **自有** = 上游没有对应职责，不是移植目标（§2 单列，不计入状态分布）。
>
> **快照警告**：审计期间有并行改动，`batch.rs` / `transcript.rs` / `rich.rs` / `panels.rs` / `lib.rs`
> 在工作区里正被修改。**行号是 2026-11-25 20:10 快照的**；这几个文件请以函数名为准。
> 状态与缺口结论来自逐函数核对，不依赖具体行号。`transcript.rs` 的行号尤其会漂。
> 审计前最后一次提交：`b235640 fix(layout): 状态行/提示行横跨整条…`。

---

## 0. 数字

| 指标 | rig-tui 0.47.0 | rig_niri 0.1.0 |
|---|---|---|
| `.rs` 文件 | **46** = 25 个实现文件 + 21 个 `tests.rs` | **25**（测试内联在 `#[cfg(test)] mod tests`，没有独立测试文件） |
| 总行数 | **23,711** | **22,557** |
| 实现行（扣掉测试） | **14,316** | **13,478** |
| 测试行 | **9,395**（21 个 `tests.rs`） | **9,079**（各文件从第一个 `#[cfg(test)]` 算到文件尾） |
| `#[test]` 个数 | 246 | **317**（本文采用的基线；审计时并行新增到 32x，见下） |
| `cargo test` | — | `rig_niri` **317**；`rig_harness_in_3d` **15**（基线） |

- 测试基线按任务约定记 **317 + 15**。审计时并行改动已把 `rig_niri` 的 `#[test]` 增到 **32x**，
  复跑里失败的用例全部来自**正在改的新测试**（先是 2 个 `transcript::span_style_tests`，后是
  1 个新增的 `batch` 双 agent 用例），不是基线用例。这是并行改动的瞬时状态，**不改变**本文结论。
- `rig_harness_in_3d` 的 15 个测试独立跑过：`15 passed`（审计后并行改动又加了 1 个）。
- 我们的"自有"行约 **5,221**（`niri.rs` 771、`workspaces.rs` 2373、`layout.rs` 1463、`spatial.rs` 433、
  `camera_rig.rs` 128、`style.rs` 53），其中实现约 2,886、测试约 2,335。
- 行数**不能**当覆盖率：我们的行引擎、工作区、niri 平铺是自己设计的，不比上游少。
- 以上都是 20:10 快照；写完后并行改动仍在长（本文落笔时 `wc -l` 已到 22.7k、`#[test]` 328）。

**模块状态分布**（§1 的 26 个上游模块）：

| 状态 | 个数 | 是哪些 |
|---|---|---|
| ✅ 已移植 | **8** | `screen` `complete` `diff` `highlight` `history` `jsonl` `markdown` `selection` |
| 🟡 部分 | **15** | `lib` `clipboard` `editor` `hints` `input` `keys` `panel` `pane` `pick` `render` `renderers` `status` `theme` `transcript` `view` |
| ❌ 未开始 | **0** | — |
| 🔄 有意替换 | **2** | `terminal` `title` |
| ➖ 不适用 | **1** | `tests`（上游集成测试，不是移植目标） |

一句话：**25 个上游实现模块全部有落点，没有"未开始"**；8 个完整，15 个部分——缺口集中在三处：
① 输入到 agent 的通道（`Deliver`/`StopTurn`/排队/转向）② 视图层的选择、回放与多来源转写
③ 插件用的开放画布与导出（`TuiPanel`/`StatusLine`/`TuiFrame`/`DrawnTranscript`）。真正的"没做"
只剩这些，逐项排在 [`REMAINING.md`](REMAINING.md)。

---

## 1. 上游模块总表

**证据写法**：`文件:行` 是我们/上游在快照里的位置；"缺什么"只写**公开能力**的缺口，不列实现差异。

| # | 上游模块（行） | 我们的落点（行） | 状态 | 证据 / 缺什么 |
|---|---|---|---|---|
| 1 | `lib.rs`（422） | `lib.rs` 184、`front.rs` 39 | 🟡 | 有：`FeathersSystems` 五阶段（`lib.rs:60`）、`FeathersViewPlugin` build/finish（`lib.rs:132`）、finish 里给 `Agent` 注册 `StatusItems`（`lib.rs:179`）、前端身份 `FRONT="feathers"` + `Answerer`/`ShowsPickers`（`front.rs:10`–`25`）。缺：`TuiPlugin`/`TuiPlugins` 那种"一键装全套 + 可选 part 去重"的装配器；无终端的 `add_view`（上游 `lib.rs:260`）；`app.keeps_files("tui")`（上游 `:180`）；`BuildFailedLooks`（上游 `:398`）；`StatusItems` 只注册 `Agent` 没注册 `TurnOf`（上游 `status.rs:227`）；`BatchPlugin` 不在视图插件里（应用层手动加，`rig_harness_in_3d/src/main.rs:68`）。 |
| 2 | `terminal.rs`（620） | — | 🔄 | 裸模式/备用屏/鼠标捕获/括号粘贴/OSC 52/`TuiTestScreen` 全部由 bevy 窗口 + `bevy_clipboard` 取代（见 §4）。还有用户价值但没替代的是 `RunInTerminal`（跑 `$EDITOR`，上游 `:57`）。复制已由 `selection.rs:696` + `clipboard.rs:202` 直接写系统剪贴板。 |
| 3 | `screen.rs`（46） | `frame.rs:561`、`layout.rs:723` | ✅ | 默认屏幕 6 块 tile 全部 spawn：转写/输入/状态（`frame.rs:561`–`660`）+ 补全/键位提示/确认行；树来自 `layout::default_tree`（`layout.rs:723`）。上游的 `InlineStrip`（`screen.rs:38`）换成 `PanelId::Confirm` tile（`frame.rs:628` 附近）——职责（"工作旁边的问题"）落地，位置按 niri 设计换掉。 |
| 4 | `clipboard.rs`（340） | `clipboard.rs` 566 | 🟡 | 有：`ClipboardText` 抽象（`:133`）、`copy_text`/`copy_selection`（`:202`/`:213`）、`Copied N lines` 与状态项（`:66`/`:89`/`:112`）、`read_paste`/`apply_paste`/`paste_text`/`type_path`（`:239`–`:268`）、`Paste::Image` 的 `@path` 语义（`:228`）。缺：**文本粘贴没接线**——`paste_from_clipboard`（`:280`）没有任何调用者；Ctrl+V 绑的是 `PasteImage`（`keys.rs:616`）而 `act` 里是空实现（`editor.rs:481`）。**图片剪贴板**取不到（模块文档 `clipboard.rs:11`–`14`：`bevy_clipboard` 没开 image feature）；拖入/粘贴出来的文件路径没解析（上游 `dropped_images` `:227`）。 |
| 5 | `complete.rs`（681） | `complete.rs` 1881 | ✅ | 数据模型逐个对齐：`CompletionSource`（`:92`）/`CompletionTrigger`（`:101`）/`CompletionKind`（`:181`）/`Candidate`（`:206`）/`Completion`（`:215`，`chosen`/`select`/`is_exact` 都在）；`SHOWN=50`（`:69`）；`FileIndex`+`walk`（`:489`/`:588`）、`match_at`/`token`/`path_items`（`:671`/`:677`/`:711`）、`markers`（`:767`）、`Sources`（`:796`）、`typed`（`:805`）、`follow_input`/`receive_paths`（`:919`/`:948`）、`CompletionPlugin`（`:1000`）、候选 tile（`:1071`/`:1172`）。键位接线：`editor.rs:500` 的 `completion_act`（↑↓ 选中、Tab/Enter 接受、Esc 关闭并记住 token）。已知差异：`walk` 不读 `.gitignore`（`:29`）；上游"粘贴后不弹列表 / 被拒命令放回不弹列表"两条抑制没做；`InputContext` 没移植（`typed()` 等价）。 |
| 6 | `diff.rs`（318） | `rich.rs:124` + 上游 `ToolRender` | ✅ | `diff_rows` 直接调 `rig_tui::diff::unified_lines`（`rich.rs:124`）；工具卡片走 `ToolRender::default_lines`→`result_lines` 的 diff 分支（上游 `renderers.rs:234`），我们的建批用 `call.default_lines()`（`batch.rs::default_lines`）。行级红绿 → `Row.band`，改动词更深一档 → span 底色（渲染层已支持，见 §3 `theme`）。 |
| 7 | `editor.rs`（770） | `editor.rs` 1268 | 🟡 | 有：游标=字节偏移+grapheme 边界（`:43`–`:75`），`set/take/clear/sent/recalled/keep/insert_char/word_left/right/home/end/backspace/delete/delete_word_*/delete_to_line_*/up/down/line_up/down` 全部在（`:75`–`:432`），CJK/emoji/ZWJ 有测试。缺：**多行显示**——`editor_rows`（`:685`）只产 **1 行**（整段文本 + `▏` 光标字符），上游 `draw_editors`（上游 `editor.rs:686`）是 `Paragraph::new(layout.rows).scroll(...)` + 光标行列（`Layout` 上游 `:37`）；**输入框内作答**（`answer`/`answering`/`take_answer` 上游 `:188`–`:220`）没有；`Editor` 是 `Resource`（`:43`）不是 `Component`（上游 `:61`，`#[require(Caret)]`），`MainEditor`/`MainInput` 不存在；输入框内的按状态提示（"An earlier prompt"/"A panel has the keys"/"Enter answers"，上游 `:706`–`:733`）没有。 |
| 8 | `highlight.rs`（444） | `rich.rs:106` | ✅ | `highlight_rows` 调 `rig_tui::highlight::code_rows`（`rich.rs:106`）；围栏代码的语法色由 `markdown::render` 内部的 `Grammars` 带上（`batch.rs::reply_lines` 直接调上游 `markdown::render`），所以回复里的代码块有语法色。 |
| 9 | `hints.rs`（102） | `hints.rs` 241 | 🟡 | 有：两张键位表逐字相同（`:26`–`:45`，与上游 `:48`–`:70` 一致）、`fit_keys`（`:50`）、独立提示 tile（`hints.rs:118`、`PanelId::Hints`）。缺口：`fit_keys` **三处行为与上游不同**（`:50`–`:64` vs 上游 `:79`–`:100`）——上游返回 `format!(" {} ", …)` 带首尾空格、按 `unicode_width` 算宽、同权重时丢**最后**一个（`Reverse(at)`）；我们裸 join、`chars().count()`、丢**第一个**。提示**恒显示**（`hint_rows` 不看输入是否为空），上游只在输入为空时显示（上游 `editor.rs` 的 `draw_editors` 里 `!editor.is_empty()` 就清空 keys）；按状态换文案（"An earlier prompt"、"A panel has the keys"、"Enter answers"）也没做。 |
| 10 | `history.rs`（223） | `history.rs` 733 | ✅ | `PromptHistory`（`:50`）含 `load`（`:65`）/`record`（`:114`）/`seed`（`:126`）/`rewrite`（`:136`）、`Sent`（`:207`）、`HistorySystems::{Seed,Record,Save}`（`:193`）；系统 `load`（`:214`）/`record`（`:235`）/`save`（`:287`）/`seed_shown`（`:328`）；JSONL 一行一个字符串、坏行跳过、去重按最后位置（测试 `:489`–`:714` 共 12 个）。差异：文件路径由插件参数给（`HistoryPlugin::at`，`:385`），不用上游的 `PluginData`/`Files`；`Editor` 自己是历史持有者，所以多一层 `mirror`（`:312`）。 |
| 11 | `input.rs`（1026） | 分散在 `editor.rs`/`keys.rs`/`pane.rs`/`selection.rs`/`complete.rs` | 🟡 | 有：`PendingInput`（`keys.rs:766`）、按键分派（`editor.rs:549` `keys`）、作用域/焦点（`keys.rs`）、滚轮 3 行（`pane.rs:271`，`WHEEL_ROWS=3` `:66`）、拖选/双击/三击（`selection.rs:788`）、`accept_completion`（`editor.rs:500`）。缺一整套"键 → 会话"的动作：`send`（上游 `:985`，带 `DeliveryMode::{Queue,Steer}` `:1005`–`:1008`）没有——全仓 `grep Deliver` 为 **0**；Tab 排队（上游 `:831`–`:833`）、Esc 停 turn（上游 `:854`–`:859`，`StopTurn`）、Ctrl+D 退出（上游 `:795`）在我们 `act` 里是空实现（`editor.rs:453`/`:483`/`:493`）；`answer_inline`（上游 `:950`）空实现（`editor.rs:479`），Alt+1..9 的绑定被 `Disabled` 挂起（`keys.rs:641`–`:655`）；条目选择 `select`（上游 `:922`）空实现；`InputSystems::{Filter,Dispatch}` / `TuiKeys` / `InputContext`（上游 `:111`–`:130`）没有等价物。 |
| 12 | `jsonl.rs`（75） | `history.rs` | ✅ | "一行一个 JSON 值"的追加/读取落在 `PromptHistory::{load,record,save}`（`history.rs:65`/`:114`/`:287`）里，坏行跳过不 panic。上游的 `tail`（跟随文件）没有；我们没有"跟读历史文件"的需求。 |
| 13 | `keys.rs`（751） | `keys.rs` 1402 | 🟡 | 有：`KeyPress`（`:140`）、`Keys`（`:274`）、`KeyScope`（`:330`）、`KeyBinding`（`:367`）、`KeyPressed`（`:412`）、`KeyFocus`（`:424`）、`AppKeyBindingsExt`（`:427`）、`InputAction` **32 个**（`:476`）、`built_in()` **43 条**（`:548`）、`add_built_in`（`:628`，另生成转写 Esc + Alt+1..9 共 11 条）、`Found`/`find` 多键前缀（`:673`/`:686`）、`released`（`:751`）、冲突拒绝 `RefusedKeys`（`:468`）。缺：`run_command`（上游 `:455`）没有；`KeyPressed.agent` 恒 `None`（`editor.rs:591`，上游填显示中的 agent）；`Reflect` 没有；`tell_refused` 走 `Notice` 的上游行为只降级成 `warn!`（`keys.rs:21`–`22`）；`KeyScope::Transcript` 有变体但**没有系统会切进去**（`:338` 注释自认）。 |
| 14 | `markdown.rs`（193） | `rich.rs:92` + `batch.rs::reply_lines` | ✅ | `markdown_rows` 调 `rig_tui::markdown::render(text,&theme,Some(width))`（`rich.rs:92`）；回复正文直接走上游 render（`batch.rs::reply_lines`），标题/列表/表格/引用/提示块都按上游规则。`render_marked` 没用到——它只服务上游的 `TranscriptRender` 查找标记（上游 `renderers.rs:629`），我们没有搜索高亮，见 `view`。 |
| 15 | `panel.rs`（344） | `frame.rs` + `layout.rs` + `panels.rs` | 🟡 | 有：布局职责——`lay_out`（`frame.rs:240`）、树叶子/容器矩形（`layout.rs:177`/`:195`）、`PanelRows`/`PanelRoots`（`panels.rs:82`/`:112`）、`MIN_TRANSCRIPT` 常量（`frame.rs:51`）。`Placement` 从上游 7 种收成 `Tree`/`Rest`（`frame.rs:154`），`Over`/`Within` 按工作区模型删掉（有意）。缺：**开放画布**——上游任何实体挂 `TuiPanel`（`panel.rs:106`）就有 `PanelCanvas`（`:160`，`render`/`buffer_mut`/`copy_to`），我们只有封闭枚举 `PanelId`（`panels.rs:33`），插件要加面板必须改核心枚举；`RequestRedraw`/`RedrawArea`（上游 `:82`/`:91`）没有；`TerminalSize`（上游 `:95`）换成 `FrameSize`（`frame.rs:86`–`107`），从 env 读固定格子数，**不跟窗口 resize**；`MIN_TRANSCRIPT` 只剩常量，树布局不再拿它兜底。 |
| 16 | `pane.rs`（392） | `pane.rs` 674 | 🟡 | 有：`PaneScroll{top,follow}`（`:58`）、窗口化 `window_transcript`（`:184`）、PgUp/PgDn（`:219`）、滚轮（`:271`）、"下面还有"标记（`:165`）。缺：`TranscriptPane`/`title`/`TranscriptSource::{Shown,Agent,Value}`（上游 `:57`/`:84`）——我们只有全局 `PanelId::Transcript`，不能"看子 agent"或"看已保存会话"；`MainPane`/`PaneKeys`（上游 `:101`/`:125`）——滚动键是物理 `KeyCode::PageUp/PageDown`（`:219`）而不是绑定表，**Up/Down 逐行滚、Alt+Up/Down 选条目没有**；`Below::New`（上游 `:376`"↓ new output"）没画——`transcript::BelowSeen`（`transcript.rs:815`）写好了但 `window_transcript` 不用它；`DrawnTranscript` 没有。 |
| 17 | `pick.rs`（1098） | `picker.rs` 1518 | 🟡 | 有：标题/说明/过滤（`:629`）、树折叠 `unfold`（`:187`）、多选打勾 `tick`（`:225`）、`free_text`（`:250`）、`secret` 打码（`:693`–`:759`）、PgUp/PgDn（`PAGE=10` `:31`）、`PickPreview` 列（`:469`/`:816`）、点击行（`PickerChoice` `:35`）。缺：**队列**——`sync_picker` 只取 `pickers.iter().next()`（`:376`），上游 `queue_choices`（`:500`）按顺序一次显示一个并交接状态；`Ask` 不进 picker（换成 Confirm tile，`batch.rs::confirm_rows`，这是设计），但顺带丢了 `InlineStrip`（上游 `:1064`）与 `Alt+1..9`；`ChoiceKeys`/`ChoiceAction`/`ChoiceSystems`（上游 `:51`/`:55`/`:589`）没有——插件不能给选项加键；`in_input`（在输入框里作答）、`carried`（换问题时交接）、`more`（展开选项说明）、`picked`（重新填充后选回同一项）、body 滚动（`PickerInput.scroll`）、`draft` 都没有（我们 `PickerInput` 字段见 `picker.rs:42`）。 |
| 18 | `render.rs`（1543） | `frame.rs` + `pane.rs` + `panels.rs` + `picker.rs` + `complete.rs` + `selection.rs` | 🟡 | **绘制管道整体换成 bevy**（有意）：帧布局 `frame.rs:368`、窗口化 `pane.rs:184`、行→节点 `panels.rs::sync_panels`、补全/选择 tile（`complete.rs:1172`/`picker.rs:629`）、选区反色 `selection.rs:912`。语义侧有：`CallStates`→`batch::live_state`、`ShownConversation`→`batch::{ShownTurn,Dialogue}`、`asides`→`batch::conversation_frame`、`decorate`→`RenderOutput.decorations`（`transcript.rs::request_lines`）+ `batch::child_calls_look`（`:1062`）、`cells_in`→`selection::row_text`。缺：搜索高亮 `light`/`lit`（上游 `:848`/`:899`）与 `view::Highlight` 全链路（0 命中）；`TuiFrame`（上游 `:1196`，`screen_row`/`area`/`covered`）没有；`TranscriptLayout::lay_out`（上游 `:442`，无终端导出）没有；`Paced` 33ms 流式节流与 `needs_redraw`（上游 `:70`/`:84`）——我们每帧全跑（bevy 自己驱动，属有意）；`inbox_lines`（上游 `:1520`）没有（没有 `Deliver` 就没有收件箱）；`compacted_line`（上游 `:797`"N messages above were compacted"）不画（`batch.rs` 的测试明确断言压缩摘要不显示）。 |
| 19 | `renderers.rs`（1068） | `batch.rs` 2412 | 🟡 | 有：五类 `RenderKey`（Tool/Streaming/Entry/Content/Aside）与四类 `RenderInput` 全填（`batch.rs::conversation_frame`/`rebuild_batch`）；`batch.pane`/`batch.agent` 真的设置（`:444`/`:445`）；`ToolRender` 的 `theme`/`took`（`record_calls` 用 `Started` 算）/`output`（`CallOutput`）/`children`（`CallTree`，`:196`）/`item` 都填；`RenderOutput.{lines,decorations}`；`RenderSystems::{Render,Decorate,Edit}` 链（`BatchPlugin`，`:1618`）；`text_rows` 直接复用上游。缺：`ToolRender.facts` 恒 `None`（TODO `:547`/`:806`：内核 `Outcome` 没给 `ToolResultContext`）→ 退出码、`OutputCut`"输出被截断"提示、`FilesChanged` 等事实拿不到，上游 `header()` 的调用状态色带/`exit()` 也就少一半输入；`RenderRevision` 从不推进（0 命中）→ 每帧全量重建；模型流参数、调用效应还没出现时的 `RenderKey::Streaming` 预画没做（TODO `:567`）。 |
| 20 | `selection.rs`（54） | `selection.rs` 2141 | ✅ | 语义比上游多得多且对齐：grapheme 列（不是字节）、CJK/emoji ZWJ 只算一列、`Selection::press/drag/release`（`:209`/`:250`/`:263`）、复制文本的 `trim_end` + `join("\n")` + 整体 `trim_end` 与上游 `Selection::mark` 一致（`:273`）、`ranges_in`/`byte_ranges_in`/`all_ranges`（`:304`/`:335`/`:349`）、`SELECTED_MODIFIER=REVERSED`（`:56`）、鼠标系统 `pointer_selection`（`:788`）、反色绘制 `selected_row`（`:912`）。上游没有的超集：双击选词/三击选行（`:87`/`:498`/`:518`）、窗口 resize 清选区（`:733`）、复制失败写状态行。 |
| 21 | `status.rs`（605） | `status.rs` 88 | 🟡 | 有：**直接调** `rig_tui::status::line(items,width)`（`status.rs:44`），所以 keep/`min_width`/"先缩后丢"与上游逐字相同；`AppStatus`+agent 的 `StatusItems` 合并（`:23`–`:38`）。缺：`StatusLine` 组件（上游 `:382`）与 `StatusLinePlugin`（`:243`）——任何面板都能画一条状态行，我们只有固定 `PanelId::Status`；`StatusParts`/`AppStatusPartsExt`（上游 `:296`/`:329`，`/plugins` 的显示/隐藏联动）；`StatusSettings` + `[status] hide`（上游 `:274`）——应用层用自己的 `[view] hide_status` 清面板替代；`StatusSystems::{Show,Edit}`（上游 `:216`）——我们全在 `Draw`；`no agent` 占位（上游 `:406`）没有；turn 级 items（上游 `:404` 读 `ActiveTurn` 的 `StatusItems`）没有，应用层 `sync_status_items` 只读 agent 的 `ActiveTurn` 计数。 |
| 22 | `theme.rs`（337） | `theme.rs` 52 + `rich.rs:63` | 🟡 | 有：10 个前景色 token（`theme.rs:10`–`:31`），`upstream_theme`（`rich.rs:63`）把底色当基底、用我们的语义前景混出 11 个 band（user/tool/added/added_word/removed/removed_word/selected/call_running/call_done/call_failed/note/important），markdown/diff 因此能拿上游颜色（`rich.rs:94`/`:126`）。缺：上游 `Theme` 的 17 个 band 与 `on`/`shade`/`without_color`/`selection`/`changed_word`/`panel`/`raised`/`of_terminal`（上游 `:138`–`:234`）没有对应公开 API，`plain`（NO_COLOR）无通路；**batch 用自己的 `RTheme::default()`**（`batch.rs:446`/`:640`/`:1120`…）而不是 `upstream_theme(我们的 Theme)` → 工具卡片/entry 行的默认底色不跟我们的主题走；`panel_surface`/`raised_surface`/`input`/`call_waiting`/`code` 是 `Theme::on` 的默认值（猜的）。 |
| 23 | `title.rs`（216） | `title.rs` 132 | 🔄 | OSC 133 语义提示符/OSC 2 标题/标题栈只对终端有意义，换成**窗口标题**（`title.rs:25` `WindowTitle`）。状态机照抄：`needs you`（有 picker 或有未回答 `Ask`）/`working`（非子 agent 的 turn 在跑）/`error`（上个 turn 失败）/`idle`（`title.rs:80`–`:120`，与上游 `:187`–`:195` 同条件）。缺：退出时恢复原标题（上游 `:209` `restore_title`）没有——窗口关闭即结束，价值低；`[title] mark_nonce_env`（上游 `:73`）不适用。 |
| 24 | `transcript.rs`（1783） | `transcript.rs` 1515 + `batch.rs` | 🟡 | 有：对话引擎 `Transcript`（`transcript.rs:501`）——按块缓存折行、宽度失效重折（`set_width`/`apply`）、`TextId`（`Message`/`Notice`/`StreamingReasoning`/`StreamingReply`/`Prompt`/`Inbox`，`:386`）、`BlockId`（`:403`）、`Slot`（`Top`/`After`/`Inside`，`:415`）、排序键与上游 `Part` 顺序一致（`rank`）、折行用上游 `rig_tui::wrap::wrap`、`Below`（`:764`）；建块在 `batch::conversation_frame`（用户/assistant 正文/推理/工具/entry/内容块/aside/note）。缺：`PastEffects`（上游 `:434`，恢复会话时旧调用的事实）——`batch.rs:77` TODO；`Replaced`（上游 `:723`，被压缩移出对话的条目仍显示）——0 命中；`stop_lines`（上游 `:1669`，`[stopped]` / `[stopped before it was answered]`）——`batch.rs:862` TODO(stops)；压缩标记 `── N messages above were compacted ──`（上游 `render.rs:797`）不画；`TextId::Inbox` 有槽位但没有生产者（没有 `Deliver`）。 |
| 25 | `view.rs`（865） | `pane.rs` + `selection.rs` + `transcript.rs` | 🟡 | 有：转写位置与宽度（`PaneScroll` + `Transcript::set_width`）、文本选择（`selection.rs:116`）与反色绘制、画出来的行（`PanelRows[Transcript]`，`pane::window_transcript`）、notice 的落点（`TextId::Notice`，应用层 `rig_harness_in_3d/src/notice.rs` 桥接）。缺：`TranscriptView`/`TranscriptItem`/`CallItem` 的**条目级选择**（上游 `:143`/`:84`/`:112`）——Alt+↑↓ 没有目标；`Highlight`/`LineSpan`/`TextMark`/`MarkIn`/`RowWrap`（上游 `:224`/`:241`/`:207`/`:174`/`:361`，搜索/标记）全没有；`ScrollTo`/`ItemRow`（上游 `:259`/`:271`，"滚到某条"）没有；`ShowAgent`（上游 `:49`，切到另一个 agent）没有；`DrawnTranscript`/`DrawnRow`/`DrawnLine`（上游 `:316`/`:326`/`:341`，给插件读"画了什么"）没有；`Notices` 资源（上游 `:165`）没有；`load_draft`/`keep_draft`（上游 `:723`/`:701`，只在 reload 时保留未发送输入）没有。 |
| 26 | `tests.rs`（433） | — | ➖ | 上游的 TUI 集成测试（`TuiTestScreen` + 模拟按键），不是移植目标。我们的等价可测性在行是**值**：`rig_niri` 317 个内联测试 + headless `App`（`batch` 的端到端、`selection` 的指针事件、`workspaces` 的布局不变量）。 |

---

## 2. 我们的 25 个文件 → 上游

| # | 文件 | 行（实现/测试） | 对应上游 | 状态 | 一句话职责 |
|---|---|---|---|---|---|
| 1 | `batch.rs` | 2412（1666/746） | `renderers.rs` + `render.rs` 的建批/调度 | 🟡 | 从对话 + 效应建 `RenderBatch`，跑 look，把行交给转写引擎；Ask 单独进确认 tile。 |
| 2 | `camera_rig.rs` | 128 | —— | ➕ | 3D 相机架（手柄/键盘），上游无对应。 |
| 3 | `clipboard.rs` | 566（309/257） | `clipboard.rs` + `terminal::CopyToClipboard` | 🟡 | 复制选区到系统剪贴板、`Copied N lines`；粘贴语义已写，键位未接。 |
| 4 | `complete.rs` | 1881（1206/675） | `complete.rs` | ✅ | `/` 命令、`@` 路径、插件 `CompletionSource`；候选 tile + 键位。 |
| 5 | `editor.rs` | 1268（762/506） | `editor.rs` + `input.rs` 的编辑动作 | 🟡 | 多行文本模型 + 历史 + grapheme 游标；显示只有 1 行。 |
| 6 | `frame.rs` | 1315（852/463） | `panel.rs` + `screen.rs` + `render.rs` 的帧布局 | 🟡 / `screen` ✅ | 树叶子 → 帧内格子区域 → UI 尺寸/3D 锚点；默认屏幕 6 块 tile。 |
| 7 | `front.rs` | 39 | `lib.rs` 的前端身份 | ✅ | `Front::new("feathers")` + `Answerer` + `ShowsPickers`。 |
| 8 | `hints.rs` | 241（160/81） | `hints.rs` | 🟡 | 两张键位表 + 裁剪成一行；3 处裁剪行为与上游不同。 |
| 9 | `history.rs` | 733（432/301） | `history.rs` + `jsonl.rs` | ✅ | 跨会话提示词历史（JSONL、去重、seed/record/save/rewrite）。 |
| 10 | `keys.rs` | 1402（1021/381） | `keys.rs` | 🟡 | 键位 IR、43 条内置绑定、32 个动作、作用域/焦点/插件绑定；缺 `run_command`。 |
| 11 | `layout.rs` | 1463（774/689） | ——（替代 `panel.rs::Placement`） | ➕ | niri 分裂树：`Split/Size/Child/Node`、`rects`/`placements`、增删列、`default_tree`。 |
| 12 | `lib.rs` | 184 | `lib.rs` | 🟡 | `FeathersSystems` + `FeathersViewPlugin` + prelude；没有一键全套/headless `add_view`。 |
| 13 | `niri.rs` | 771（575/196） | —— | ➕ | niri 平铺几何：图层/列/瓦片、`place_tiles`、`spawn_niri_screen`。 |
| 14 | `pane.rs` | 674（334/340） | `pane.rs` | 🟡 | 转写窗口化 + 滚动（PgUp/PgDn、滚轮）；没有多来源窗格/条目选择。 |
| 15 | `panels.rs` | 612（345/267） | `panel.rs` 的 `PanelCanvas`/绘制侧 | 🟡 | 行是值：`PanelRows` → bevy_ui 节点（前缀增量 diff）；面板身份是封闭枚举。 |
| 16 | `picker.rs` | 1518（900/618） | `pick.rs` | 🟡 | 列表选择器（过滤/树/多选/填空/secret/preview），是条带最右一列而不是浮层。 |
| 17 | `rich.rs` | 563（263/300） | `markdown.rs` + `highlight.rs` + `diff.rs` | ✅ | 上游三个纯值渲染器的适配层 + `upstream_theme`（给业务/插件用）。 |
| 18 | `selection.rs` | 2141（1023/1118） | `selection.rs` + `render::cells_in` + `input::moused` | ✅ | 鼠标拖选/双击/三击、grapheme 级命中、反色、复制。 |
| 19 | `spatial.rs` | 433（364/69） | —— | ➕ | 3D 卡片/漏斗/自由视角（空间 UI 的渲染侧），上游无对应。 |
| 20 | `status.rs` | 88 | `status.rs` 的裁剪 + 绘制 | 🟡 | 合并 `AppStatus`+agent `StatusItems`，调上游 `status::line` 裁成两条。 |
| 21 | `style.rs` | 53 | —— | ➕ | ratatui `Color` → bevy `Color` 的桥（含 256 色近似）。 |
| 22 | `theme.rs` | 52 | `theme.rs` | 🟡 | 10 个前景 token；band 由 `rich::upstream_theme` 混出。 |
| 23 | `title.rs` | 132（114/18） | `title.rs` | 🔄 | 状态机照抄，输出换成窗口标题。 |
| 24 | `transcript.rs` | 1515（842/673） | `transcript.rs` | 🟡 | 对话引擎（块/缓存/排序/折行/finalize/notice 槽位）+ 行 → 节点。 |
| 25 | `workspaces.rs` | 2373（992/1381） | —— | ➕ | 工作区（= 会话）的绝对位姿、拖拽/放置/整理/聚焦、每工作区一份面板状态。 |

**统计**：✅ 5（`complete` `front` `history` `rich` `selection`）｜🟡 13｜🔄 1（`title`）｜➕ 6
（`camera_rig` `layout` `niri` `spatial` `style` `workspaces`）。
`layout.rs`/`niri.rs` 不是"上游没有这件事"：它们替代的是 `panel.rs::Placement` + `render::layout` 的
布局职责，所以在 §1 里算进 `panel`/`render` 的"部分"，而不单列状态。

---

## 3. 🟡 项逐条：缺什么、缺在哪个函数

> 只列**公开能力**的缺口。"上游"指 rig-tui 0.47.0 的同名函数/类型。

### 3.1 `lib.rs`（装配）

- **没有 `add_view`**：上游 `add_view`（`lib.rs:260`）在无终端时装配 input/keys/pickers/转写，
  测试用 `TuiTestScreen` 顶替终端。我们只能整装 `FeathersViewPlugin`；headless 测试靠单个插件
  （`PanePlugin`/`BatchPlugin`/`StatusPlugin` 各自能跑，见 `rig_harness_in_3d/src/lib.rs:419`–`:438`）。
- **没有可选 part 的装配器**：上游 `TuiPlugins`（`lib.rs:138`）用 `is_plugin_added` 去重，逐个加
  `StatusLinePlugin`/`KeyHintsPlugin`/`TerminalTitlePlugin`。我们的 `StatusPlugin`/`HintsPlugin`/
  `TitlePlugin` 是固定装进 `FeathersViewPlugin` 的（`lib.rs:154`–`:173`）。
- **`keeps_files("tui")` / `BuildFailedLooks`**：上游 `lib.rs:180`/`:398`。会话文件登记我们没有；
  构建失败的红色提示 look 也没有（`rig_harness_in_3d` 的 reload 场景用不到）。
- **`BatchPlugin` 不在视图插件里**：`FeathersViewPlugin::build` 的插件元组没有它（`lib.rs:154`），
  应用必须自己加（`rig_harness_in_3d/src/lib.rs:438`、`main.rs:68`）。上游的
  `renderers::add(app)` 是在 `TuiViewPlugin::build` 里做的（上游 `lib.rs:170`）。
- **turn 级 `StatusItems` 没注册**：`finish` 只做 `Agent`（`lib.rs:179`），上游两行都做
  （`status.rs:226`–`:227`）。

### 3.2 `clipboard.rs`（粘贴通道）

- **文本粘贴没有调用者**：`paste_from_clipboard`（`clipboard.rs:280`）写好且 16 个测试覆盖，
  但 `grep -rn paste_from_clipboard rig_niri/src` 只命中定义与测试。Ctrl+V 绑的是
  `InputAction::PasteImage`（`keys.rs:616`），`editor::act` 里 `Do::PasteImage => {}`（`editor.rs:481`）。
- **图片粘贴取不到图**：上游 `Do::PasteImage` 要 `Scratch::images` + `Processes::launcher`，再
  `clipboard::paste(...)`（上游 `input.rs:863`–`:877`）；我们的 `bevy_clipboard` 没开 `image`
  feature，模块文档写明只保留 `Paste::Image` 的应用语义（`clipboard.rs:11`–`14`）。
- **拖入/粘贴出来的文件路径不解析**：上游 `dropped_images`/`image_file`/`local_url_path`/
  `percent_decoded`（`clipboard.rs:227`/`:296`/`:307`/`:316`）把终端粘贴的一段文本拆成图片路径；
  我们只有 `type_path`（`:268`），没有"从一段文本里认出路径"的入口。

### 3.3 `editor.rs`（显示与作答）

- **多行显示**：缺在 `editor_rows`（`editor.rs:685`）——只产 1 个 `KeyedRow`，整段文本塞进一个
  `Line`（换行符直接留在 span 里），光标是 `▏` 字符。上游 `draw_editors`（上游 `editor.rs:686`）
  用 `Layout`（上游 `:37`：`rows`/`cursor_row`/`cursor_column`）+ `Paragraph::scroll((top,0))`，
  光标按行列落在屏上，输入框高度随内容长（`MOST_LINES=10`）。
- **在输入框里作答**：缺 `answer`/`answering`/`take_answer`（上游 `editor.rs:188`–`:220`）。
  上游把问题答案放进输入框、Enter 回答、Esc 拒绝、关掉后把草稿放回；我们的确认 tile 只能点。
- **组件身份**：`Editor` 是 `Resource`（`editor.rs:43`），上游是 `Component` + `#[require(Caret)]`
  （上游 `:61`），并有 `MainEditor`/`MainInput` 让插件用 `Single<&mut Editor, With<MainEditor>>`。
- **输入框内的按键提示**：上游在 `draw_editors` 里按状态换（recalled / 面板拿走键 / 正在作答 /
  空输入），我们只有独立的 `PanelId::Hints` 一行，且恒显示（见 3.4）。

### 3.4 `hints.rs`（裁剪）

- **三处行为不同**，都在 `fit_keys`（`hints.rs:50`–`:64`）：上游 `" {} "` 带首尾空格 → 我们裸 join；
  上游 `.width()`（unicode 宽度）→ 我们 `chars().count()`；上游同权重丢**最后**（
  `min_by_key(|(at,(_,keep))| (*keep, Reverse(*at)))`，上游 `:90`）→ 我们丢**第一个**（`min_by_key`
  只看 weight）。后果：中文/emoji 提示行会多留或少留一个；窄面板下先丢哪个不一样。
- **恒显示**：`hint_rows`（`hints.rs:118`）不看输入是否为空；上游只在空输入时画（上游
  `editor.rs` 的 `draw_editors`：`else if !editor.is_empty() { keys = &[] }`）。

### 3.5 `input.rs`（键 → 会话）

- **发送/排队/转向**：缺在 `editor::act` 的 `Do::Send`（`editor.rs:484`）与 `Do::Tab`（`:483`）——
  现在只写 `PromptSubmitted`（`:621`）和本地 `SubmittedPrompts`，**没有任何 `Deliver`**。
  上游 `send`（`input.rs:985`）按 `queue` 参数选 `DeliveryMode::Queue`/`Steer`（`:1005`–`:1008`），
  `Tab` 在 turn 运行中走排队（`:831`–`:833`）。
- **停止**：`Do::Stop => {}`（`editor.rs:493`）。上游 `StopTurn::new(agent)`（`input.rs:854`–`:859`）。
- **退出**：`Do::Exit => {}`（`editor.rs:453`）。上游空输入 Ctrl+D 触发 `Exit(Quit)`（`:795`）。
- **内联回答**：`AnswerInline`/`DismissInline` 空实现（`editor.rs:476`–`:479`），Alt+1..9 绑定挂
  `Disabled`（`keys.rs:641`–`:655`，等"内联显示方"）；上游 `answer_inline`（`input.rs:950`）。
- **条目选择**：`SelectUp/SelectDown/Deselect` 空实现（`editor.rs:476`–`:478`）；上游 `select`
  （`input.rs:922`）改 `TranscriptView::selected`。
- **过滤/上下文**：没有 `InputSystems::Filter`（插件在绑定之前读/改/丢键）、`TuiKeys` 每键一次的
  调度（上游 `input.rs:111`–`:130`）、`InputContext`（`typed`/`completion`/`answering`）。我们的
  `editor::keys`（`editor.rs:549`）一次读完这一帧的 `KeyboardInput` 直接分派。

### 3.6 `keys.rs`（插件侧）

- **`run_command`**：上游 `keys.rs:455`，把斜杠命令绑到键上。我们没有——应用层自己把输入框里
  `/` 开头的提交转成 `RunCommand`（`rig_harness_in_3d/src/lib.rs:355`–`:357`），插件键位到这个
  链路没有公开入口。
- **`KeyPressed.agent` 恒 `None`**（`editor.rs:591`）：上游填显示中的 agent，插件的观察者据此
  判断"哪个 agent 的键"。
- **`Reflect` / `tell_refused`**：上游这些组件都 `derive(Reflect)` 供 `/plugins`、场景保存；
  键位冲突我们只 `warn!` + `RefusedKeys`（`keys.rs:468`），上游还会发 `Notice`。
- **`KeyScope::Transcript` 是死作用域**：变体在（`keys.rs:330`），`add_built_in` 也绑了转写区
  Esc（`:636`–`:640`），但**没有系统会切进去**（`:338` 注释自认）——条目选择整条链没接。

### 3.7 `panel.rs` / `panels.rs`（开放画布）

- **插件画不了自己的面板**：上游 `TuiPanel`（`panel.rs:106`）+ `PanelCanvas`（`:160`，`render`/
  `buffer_mut`/`copy_to`）；我们是封闭 `PanelId`（`panels.rs:33`）+ `PanelRows`（`:82`），加面板
  要改核心枚举。
- **按需重绘没有**：`RequestRedraw`/`RedrawArea`（上游 `:82`/`:91`）。我们每帧跑整个 `Update` 链。
- **尺寸不跟窗口**：`FrameSize`（`frame.rs:86`–`:107`）从 env/`parse` 读固定格子数，没有监听
  `WindowResized`；上游 `TerminalSize`（`panel.rs:95`）每帧由终端尺寸填。

### 3.8 `pane.rs`（窗格）

- **多来源窗格**：`TranscriptPane`/`TranscriptSource::{Shown,Agent,Value}`（上游 `pane.rs:57`/`:84`）
  → 只有全局 `PanelId::Transcript`；`Pane2`（拆窗）每帧复制同一份可见行。
- **滚动/选择的键**：`pane_keys`（`pane.rs:219`）直接读 `KeyCode::PageUp/PageDown`，没有
  `PaneKeys`（上游 `:125` 的 6 条绑定）→ Up/Down 逐行、Alt+Up/Down 选条目都不存在（`act` 里空实现）。
- **`Below::New`**：`window_transcript` 只放 `BELOW_MARKER`（`pane.rs:165`/`:184`）；`BelowSeen`
  （`transcript.rs:815`）已实现、有测试，但没人持有它（自注释 `transcript.rs:811`–`:813`）。

### 3.9 `pick.rs` / `picker.rs`（队列与选项键）

- **队列**：`sync_picker` 用 `pickers.iter().next()`（`picker.rs:376`）；上游 `queue_choices`
  （`pick.rs:500`）把多个 picker/问题排队、一次显示一个、关掉后交接 `carried` 状态。
- **选项键**：`ChoiceKeys`/`ChoiceAction`（上游 `:51`/`:55`）没有——插件不能给单选器/问题加
  `Alt+1..9` 之类的键；我们的 `PickerChoice` 只能点。
- **body 细节**：`PickerInput`（`picker.rs:42`）缺上游的 `scroll`（列表体滚动）、`draft`（在输入框
  作答时的草稿）、`more`（展开选项说明）、`picked`（重新填充后选回同一项）；`in_input()`
  （上游 `:132`）等价能力没有。

### 3.10 `render.rs`（非绘制部分）

- **搜索高亮**：`view::Highlight`/`LineSpan` + `render::Lights`/`light`/`lit`（上游 `render.rs:848`/
  `:899`）全链路 0 命中，`grep -rn "Highlight" rig_niri/src` 只命中 `pick::Highlighted`（无关）。
- **帧导出**：`TuiFrame`（上游 `:1196`，`screen_row`/`area`/`covered`）没有；`TranscriptLayout`
  （上游 `:437`，无终端导出转写行）没有——插件拿不到"第 n 行在屏幕哪"。
- **收件箱/压缩提示**：`inbox_lines`（上游 `:1520`）没有生产者（没有 `Deliver`）；
  `compacted_line`（上游 `:797`）不画（`batch.rs` 的测试断言压缩摘要不显示）。

### 3.11 `renderers.rs`（事实与版本）

- **`facts: None`**：`batch.rs:547`/`:806` 两处 TODO——内核的 `Outcome` 只给输出文本，没有
  `ToolResultContext`，于是 `header()` 的调用状态色、`exit()` 的退出码、`OutputCut` 的"输出被截断"
  行、`FilesChanged` 都没有；`took` 只在结算时从 `Started` 算（`record_calls`），运行中没有。
- **`RenderRevision`**：0 命中 → 每帧全量 `rebuild_batch` + `apply`（diff 靠内容比较兜住，
  但大对话的 CPU 成本比上游高）。
- **流式预画**：模型流参数、调用效应还没出现时，上游先画 `CallState::Streaming` 的卡片；
  我们等效应出现（`batch.rs:567` TODO）。

### 3.12 `status.rs`（开放状态行）

- **`StatusLine` 组件 / `StatusLinePlugin`**（上游 `:382`/`:243`）：任何面板（尤其是输入框）都能
  带一条状态行；我们只有固定 `PanelId::Status` 一块 tile，输入框底行没有状态。
- **`StatusParts`/`AppStatusPartsExt`**（上游 `:296`/`:329`）：`/plugins` 能按名字显示/隐藏某个
  `StatusItem`；我们没登记 part（上游在 `lib.rs:169` 用 `add_status_part` 登记 `COPIED`）。
- **`StatusSettings`/`[status] hide`**（上游 `:274`）：应用层用 `[view] hide_status` 清空面板
  （`rig_harness_in_3d/src/status.rs::apply_view_settings`）替代。
- **`no agent` 占位 / turn 级 items**（上游 `:406`/`:404`）：没有 agent 时上游显示 "no agent"；
  本 turn 的 `StatusItems`（token 速率、进度）我们没读。

### 3.13 `theme.rs`（band 与主题注入）

- **17 个 band + 方法**：`on`/`shade`/`without_color`/`selection`/`changed_word`/`panel`/`raised`/
  `of_terminal`（上游 `:138`–`:234`）没有；`panel_surface`/`raised_surface`/`input`/`call_waiting`/
  `code` 在 `upstream_theme` 里是 `Theme::on` 的默认值（`rich.rs:52`–`:62` 注释自认"猜的"）。
- **batch 不跟我们的主题**：`RTheme::default()` 写死在 `rebuild_batch`/`collect_rows`/`confirm_*`
  （`batch.rs:446`/`:640`/`:1120`/`:1354`），`crate::theme::Theme` 只被 `rich.rs` 调到。
- **NO_COLOR**：没有通路（上游 `:191` `without_color`）。

### 3.14 `transcript.rs`（几处可见缺口）

- **`PastEffects`**（上游 `:434`）：恢复会话时从效应日志重建旧调用的事实（`took`、调用树、
  子 agent 的调用）。`batch.rs:77` TODO；后果：重启后旧卡片没有时长/调用树。
- **`Replaced`**（上游 `:723`）：压缩移出对话的条目仍显示（`view::TranscriptItem::Replaced` 能选）；
  我们 0 命中，压缩摘要也不画。
- **`stop_lines`**（上游 `:1669`）：被停掉的 turn 下的 `[stopped]`/`[stopped before it was answered]`；
  `batch.rs:862` TODO(stops)。**`TextId::Inbox`** 有槽位无生产者。

### 3.15 `view.rs`（选择/导出）

- **条目级选择**：`TranscriptView::selected`/`TranscriptItem`/`CallItem`（上游 `:143`/`:84`/`:112`）
  没有；`Selection` 是**文本**选区，不能"选中第 n 个调用"。
- **搜索/标记**：`Highlight`/`LineSpan`/`TextMark`/`MarkIn`/`RowWrap`（上游 `:224`/`:241`/`:207`/
  `:174`/`:361`）没有，配套的 `render::light` 也没有（见 3.10）。
- **`ScrollTo`/`ItemRow`**（上游 `:259`/`:271`）、**`ShowAgent`**（上游 `:49`）、
  **`DrawnTranscript`**（上游 `:316`）、**`Notices` 资源**（上游 `:165`）、
  **`load_draft`/`keep_draft`**（上游 `:723`/`:701`）：都没有。notice 的显示是应用层
  `rig_harness_in_3d/src/notice.rs` 桥到 `TextId::Notice`，前端没有 `Notices` 资源。

---

## 4. 有意替换 / 不适用（不是缺口）

### 4.1 `terminal.rs`：整条终端管道换成窗口

| 上游能力 | 终端实现 | 我们的取代 |
|---|---|---|
| 裸模式/备用屏/光标隐藏/鼠标捕获/括号粘贴 | crossterm + `Terminal<CrosstermBackend>` | bevy 窗口 + winit 输入（不需要这些模式） |
| 每帧合成到 `Buffer` | `render.rs` 写格子 | bevy_ui 节点 + `feathers_in_3d`（`panels.rs::sync_panels`） |
| 复制 | OSC 52 + base64 | `bevy_clipboard` 系统剪贴板（`selection.rs:696`） |
| 尺寸/焦点事件 | crossterm resize/focus | bevy `Window`（尺寸**尚未**接，见 3.7） |
| `TuiTestScreen` | 内存 `Buffer` 断言 | 行是值：`PanelRows` + headless `App`（317 个测试） |
| `TerminalWrite`（任意转义序列，如桌面通知） | 帧后写 stdout | 无对应物；插件要通知得走 bevy 消息（可接受） |
| `RunInTerminal`（跑 `$EDITOR`） | 交还 tty 给子进程 | **没有替代**；窗口版要"挂起输入 + 恢复重排 + 退出码事件"，价值低，见 `REMAINING.md` §终端专有 |

### 4.2 `title.rs`：OSC 标题 → 窗口标题

OSC 133 提示符标记、OSC 2 标题、标题栈 push/pop（`PUSH_TITLE`/`POP_TITLE`）只对终端有意义；
窗口前端用 `WindowTitle`（`title.rs:25`）写 `Window::title`，状态机照抄（§1 #23）。
退出恢复原标题（上游 `:209`）不做：窗口关闭即结束。

### 4.3 `tests.rs`：不是移植目标

上游 433 行是 TUI 集成测试（`TuiTestScreen` + 模拟按键）。我们不需要等价物：行/选区/面板区域
都是值，测试直接断言值（`batch` 的 headless 端到端、`selection` 的指针事件、`workspaces` 的布局
不变量）。

---

## 5. 旧审计被推翻的结论（只列最严重的）

| 旧文（2026-11-23） | 现在的证据 |
|---|---|
| "完全没有键位机制：`KeyPress`/`Keys`/`KeyBinding`/`KeyScope`/`KeyFocus`/`KeyPressed`/`AppKeyBindingsExt` 全部不存在；43 条绑定与 32 个动作一个没落" | `keys.rs` 1402 行：全部类型在（`:140`/`:274`/`:367`/`:330`/`:424`/`:412`/`:427`），`built_in()` **43 条**（`:548`）、`InputAction` **32 个**（`:476`）、`find` 多键前缀（`:686`）。只差 `run_command`/`KeyPressed.agent`（§3.6）。 |
| "没有输入历史：Up 只在本次进程里翻，`tui/history.jsonl` 不存在" | `history.rs` 733 行：`PromptHistory`（JSONL、去重、坏行跳过）+ `HistorySystems::{Seed,Record,Save}`（`:193`）+ `Sent`（`:207`）；应用层已把文件落在 `RIG_HOME/data/tui/history.jsonl`（`rig_harness_in_3d/src/lib.rs:169`–`:171`）。 |
| "输入补全完全没有：没有 `CompletionSource`/`Candidate`/`Completion`/`FileIndex`" | `complete.rs` 1881 行，数据模型与上游逐个同名（`:92`/`:101`/`:181`/`:206`/`:215`），`/` 命令、`@` 路径（后台遍历 `receive_paths`）、插件 marker、Tab/Enter 接受、Esc 关闭（`editor.rs:500`）。只差 `.gitignore` 感知与两条抑制。 |
| "没有选字/剪贴板；`bevy_clipboard` 一次没用" | `selection.rs` 2141 行（拖选/双击/三击/grapheme 命中/反色）+ `clipboard.rs` 566 行（系统剪贴板、`Copied N lines`、错误状态），鼠标事件已接（`selection.rs:788`）。缺的只是**粘贴**接线（§3.2）。 |
| "回复是写死的一句中文；转写里没有对话；`transcript.rs` 整块没移植" | 假回复已删（`batch.rs:18`–`19` 注释明说）。`batch` 五类 `RenderKey`/四类 `RenderInput` 全填（`conversation_frame`/`rebuild_batch`）；`transcript.rs::Transcript`（`:501`）是块缓存/排序/折行引擎；notice/aside/流式/推理/内容块都进转写。 |
| "`pane`/`view`/`render` 都 ❌ 未开始；覆盖率约 25%" | `pane` 窗口化/滚动/滚轮齐（`pane.rs:184`/`:219`/`:271`），`view` 的文本选择/绘制行有替代（§1 #25），`render` 的布局/窗口化/装饰/补全/选择都落地（§1 #18）。行数：4943 → **22,557**，文件 17 → **25**，状态 1✅/11🟡/11❌ → **8✅/15🟡/0❌/2🔄**。 |

---

## 6. 复现

```sh
cd /var/home/lichi/桌面/bevy_3d_ai_research
UP=.cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/rig-tui-0.47.0/src

# 文件数 / 行数
find rig_niri/src -name '*.rs' | wc -l                      # 25
find rig_niri/src -name '*.rs' -print0 | xargs -0 wc -l | tail -1   # 22557（快照）
find "$UP" -name '*.rs' | wc -l                              # 46（25 实现 + 21 tests.rs）
find "$UP" -name '*.rs' ! -name 'tests.rs' -print0 | xargs -0 wc -l | tail -1  # 14316
find "$UP" -name 'tests.rs' -print0 | xargs -0 wc -l | tail -1        # 9395

# 测试数
grep -rh '#\[test\]' rig_niri/src | wc -l                    # 317（基线；并行改动中会变）
grep -rh '#\[test\]' "$UP" | wc -l                           # 246

# 实现/测试行拆分（各文件从第一个 #[cfg(test)] 到文件尾算测试）
for f in rig_niri/src/*.rs; do
  s=$(grep -n '^#\[cfg(test)\]' "$f" | head -1 | cut -d: -f1)
  [ -n "$s" ] && echo "$f impl=$((s-1)) test=$(($(wc -l < "$f")-s+1))" || echo "$f impl=$(wc -l < "$f") test=0"
done

# 测试
export CARGO_HOME=$PWD/.cargo-home
cargo test -p rig_niri --offline
cargo test -p rig_harness_in_3d --offline
```

---

## 7. 怎么读这份文档

1. 想知道"某个能力有没有"：先看 §1 的对应上游模块，再看 §3 的同名小节。
2. 想知道"还差什么、先做哪个"：看 [`REMAINING.md`](REMAINING.md)（按用户能感知的价值排序）。
3. 想改代码前确认边界：§2 的"我们的 25 个文件"给每个文件的职责与上游对应；6 个 ➕ 文件
   是自有设计（`layout`/`niri`/`workspaces`/`spatial`/`camera_rig`/`style`），改它们不算"偏离上游"。
4. 文档里的数字都能用 §6 的命令重新数；`文件:行` 是快照位置，`batch.rs`/`transcript.rs`/`rich.rs`/`panels.rs`/`lib.rs`
   正在并行改动，以函数名为准（见开头的快照警告）。
