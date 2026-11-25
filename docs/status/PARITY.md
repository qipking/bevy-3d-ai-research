# rig_niri ↔ rig-tui 移植核对（2026-11-25 第五轮之后）

> **状态**：当前。取代 2026-11-25 第二次审计（20:10 快照）及其结论；差异清单见 §5。
> 本文只改结论与证据指针，审计方法沿用：逐文件读两边源码。
>
> **基准**：上游 `rig-tui 0.47.0`（`.cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/rig-tui-0.47.0/src/`，下称"上游"）
> vs 我们 `rig_niri 0.1.0`（`rig_niri/src/`）。
> **方法**：结论只来自代码（`grep`/`read`），数字来自 `wc -l`、`grep -c '#\[test\]'`、`cargo test`
> （复现命令见 §6）。不沿用旧文档的任何结论。
> **口径**：
> ✅ **已移植** = 该模块对外的职责已完整落地，实现可以换；
> 🟡 **部分** = 主路径在，但公开能力有明确缺口（§3 逐条写"缺什么、缺在哪个函数"）；
> ❌ **未开始**；
> 🔄 **有意替换** = 终端管道，用 bevy 窗口/渲染取代；
> ➕ **自有** = 上游没有对应职责，不是移植目标（§2 单列，不计入状态分布）。
>
> **快照警告**：本文件写于 **2026-11-25 22:36–22:50**，期间 `picker.rs`（选项键 Alt+1..9、`more`）、
> `pane.rs`/`batch.rs`/`transcript.rs`（`↓ new output`、`[stopped]`）正被并行改动落地，
> `editor.rs`/`selection.rs`/`frame.rs` 是刚改完的。
> **行号是这一小时的快照，以函数名为准**；`grep` 命令都能复现结论本身。
> 快照里 `rig_niri` 有 **441** 个 `#[test]`（22:50 全绿，见 §0）。

---

## 0. 数字

| 指标 | rig-tui 0.47.0 | rig_niri 0.1.0 |
|---|---|---|
| `.rs` 文件 | **46** = 25 个实现文件 + 21 个 `tests.rs` | **25**（测试内联在 `#[cfg(test)] mod tests`，没有独立测试文件） |
| 总行数 | **23,711** | **29,219** |
| 实现行（扣掉测试） | **14,316** | **15,578** |
| 测试行 | **9,395**（21 个 `tests.rs`） | **13,641**（各文件从第一个 `#[cfg(test)]` 算到文件尾） |
| `#[test]` 个数 | 246 | **441**（`grep -rho '#\[test\]' rig_niri/src` 的行数，22:50） |
| `cargo test` | — | `rig_niri` **441 passed**（22:50，全绿）；`rig_harness_in_3d` **25**（lib 20 + main 5）；`feathers_in_3d` **3** |

- 两次实测：**423**（22:40，`picker-keys` 落地前）与 **441**（22:50，全绿）。都是
  `cargo test --workspace --offline`；doctest：`rig_niri` 1 passed / 2 ignored，`rig_harness_in_3d` 1 passed。
- 22:44–22:48 之间曾出现过 1 个红：`picker::tests::more_expands_and_collapses_the_selected_items_description`
  ——那是 `picker-keys` 正在写的用例（`more` 展开），22:50 复跑已随它的改动一起转绿，不是基线回归。
- 计数只会继续涨（`picker.rs` 收尾、`markers` 的状态标记测试），**以你复跑的结果为准**。
- 我们的"自有"行约 **5,224**（`niri.rs` 771、`workspaces.rs` 2373、`layout.rs` 1466、`spatial.rs` 433、
  `camera_rig.rs` 128、`style.rs` 53），其中实现 2,889、测试 2,335。
- 行数**不能**当覆盖率：我们的行引擎、工作区、niri 平铺是自己设计的，不比上游少。

**模块状态分布**（§1 的 26 个上游模块）：

| 状态 | 个数 | 是哪些 |
|---|---|---|
| ✅ 已移植 | **8** | `screen` `complete` `diff` `highlight` `history` `jsonl` `markdown` `selection` |
| 🟡 部分 | **15** | `lib` `clipboard` `editor` `hints` `input` `keys` `panel` `pane` `pick` `render` `renderers` `status` `theme` `transcript` `view` |
| ❌ 未开始 | **0** | — |
| 🔄 有意替换 | **2** | `terminal` `title` |
| ➖ 不适用 | **1** | `tests`（上游集成测试，不是移植目标） |

一句话：**25 个上游实现模块全部有落点，没有"未开始"**；这一轮补掉了五块曾经写着"没做"的——
输入到 agent 的通道（`Deliver`/`StopTurn`/Tab 排队/Esc 停）、输入框与作答（多行显示、Alt+1..9 键盘作答）、
picker 队列 + 内置 Alt+1..9、窗口缩放（逻辑像素 → 格子）、转写条目选择与 `↓ more below`/`↓ new output`/`[stopped]`。
剩下的缺口集中在两处：
① 视图层的多来源转写与搜索/标记（`TranscriptSource`/`ShowAgent`/`Highlight`/`DrawnTranscript`）
② 插件用的开放画布与导出（`TuiPanel`/`StatusLine`/`TuiFrame`/`ChoiceKeys`）。逐项排在 [`REMAINING.md`](REMAINING.md)。

---

## 1. 上游模块总表

**证据写法**：`文件:行` 是我们/上游在快照里的位置；"缺什么"只写**公开能力**的缺口，不列实现差异。

| # | 上游模块（行） | 我们的落点（行） | 状态 | 证据 / 缺什么 |
|---|---|---|---|---|
| 1 | `lib.rs`（422） | `lib.rs` 186、`front.rs` 39 | 🟡 | 有：`FeathersSystems` 五阶段（`lib.rs:60`）、`FeathersViewPlugin` build（`lib.rs:136`–`:176`）、finish 里给 `Agent` 注册 `StatusItems`（`lib.rs:178`–`:182`）、前端身份 `FRONT="feathers"` + `Answerer`/`ShowsPickers`（`front.rs:10`/`:28`）。缺：`TuiPlugin`/`TuiPlugins`（上游 `:118`/`:133`）那种"一键装全套 + 可选 part 去重"的装配器；无终端的 `add_view`（上游 `:260`）；`app.keeps_files("tui")`（上游 `:180`）；`BuildFailedLooks`（上游 `:398`）；`StatusItems` 只注册 `Agent` 没注册 `TurnOf`（上游 `status.rs:226`–`:227`）；`BatchPlugin` 不在视图插件里（应用层手动加，`rig_harness_in_3d/src/main.rs:68`）。 |
| 2 | `terminal.rs`（620） | — | 🔄 | 裸模式/备用屏/鼠标捕获/括号粘贴/OSC 52/`TuiTestScreen` 全部由 bevy 窗口 + `bevy_clipboard` 取代（见 §4）。还有用户价值但没替代的是 `RunInTerminal`（跑 `$EDITOR`，上游 `:57`）。复制已由 `selection.rs` + `clipboard.rs::copy_selection`（`:231`）直接写系统剪贴板。 |
| 3 | `screen.rs`（46） | `frame.rs:485`/`:849`、`layout.rs:723` | ✅ | 默认屏幕 spawn：`DefaultScreen`（`frame.rs:485`）+ `spawn_default_screen`（`:849`）+ `default_screen_panels`（`:730`），确认 tile 高度由 `sync_confirm_tile`（`:829`）管；树来自 `layout::default_tree`（`layout.rs:723`），7 个叶子：转写/留白/确认/补全/输入/提示/状态。上游的 `InlineStrip`（`pick.rs:1064`）换成 `PanelId::Confirm` tile——职责（"工作旁边的问题"）落地，位置按 niri 设计换掉。 |
| 4 | `clipboard.rs`（340） | `clipboard.rs` 606 | 🟡 | 有：`ClipboardText` 抽象（`:151`）、`copy_text`/`copy_selection`（`:220`/`:231`）、`Copied N lines` 与状态项（`:68`/`:91`）、`read_paste`/`apply_paste`/`paste_text`/`type_path`（`:261`/`:270`/`:281`/`:290`）、`Paste::Image` 的 `@path` 语义（`:246`）。**文本粘贴已接线**：`Do::PasteImage` → `Act::Paste`（`editor.rs:983`）→ `clipboard::paste_from`（`clipboard.rs:317`，调用点 `editor.rs:1231`–`:1253`），失败写 `PASTE_FAILED`（`clipboard.rs:113`/`:116`）；测试 `editor.rs:3215`/`:3237`/`:3261`。缺：**图片剪贴板**取不到（模块文档 `clipboard.rs:11`–`:14`：`bevy_clipboard` 没开 image feature）；拖入/粘贴出来的文件路径没解析——上游 `dropped_images`/`image_file`/`local_url_path`/`percent_decoded`（上游 `:227`/`:296`/`:307`/`:316`）在 `grep -rn dropped_images rig_niri/src` 是 0。 |
| 5 | `complete.rs`（681） | `complete.rs` 1881 | ✅ | 数据模型逐个对齐：`CompletionSource`（`:92`）/`CompletionTrigger`（`:101`）/`CompletionKind`（`:181`）/`Candidate`（`:206`）/`Completion`（`:215`，`chosen`/`select`/`is_exact` 都在）；`SHOWN=50`（`:69`）；`FileIndex`+`walk`（`:489`/`:588`）、`match_at`/`token`/`path_items`（`:671`/`:677`/`:711`）、`markers`（`:767`）、`Sources`（`:783`）、`typed`（`:805`）、`follow_input`/`receive_paths`（`:919`/`:948`）、`CompletionPlugin`（`:1000`）、候选 tile（`:1042`/`:1172`，高度 `:1080`/`:1122`，`sync_completion_tile` `:1093`）。键位接线：`editor.rs:1031` 的 `completion_act`（↑↓ 选中、Tab/Enter 接受、Esc 关闭并记住 token）。已知差异：`walk` 不读 `.gitignore`（`complete.rs:29`–`:31`）；上游"粘贴后不弹列表 / 被拒命令放回不弹列表"两条抑制没做（`complete.rs:24`–`:26`）；`InputContext` 没移植（`typed()` 等价，`:805`）。 |
| 6 | `diff.rs`（318） | `rich.rs:124` + 上游 `ToolRender` | ✅ | `diff_rows` 直接调 `rig_tui::diff::unified_lines`（`rich.rs:124`）；工具卡片走 `ToolRender::default_lines`→`result_lines` 的 diff 分支（上游 `renderers.rs:234`），我们的建批用 `call.default_lines()`（`batch.rs::default_lines`）。行级红绿 → `Row.band`，改动词更深一档 → span 底色（渲染层已支持，见 §3 `theme`）。 |
| 7 | `editor.rs`（770） | `editor.rs` 3437 | 🟡 | 有：游标=字节偏移+grapheme 边界（`Editor` `:85`、`set_cursor` `:351`），`set/take/clear/sent/recalled/stop_browsing/insert/insert_char/word_left/right/home/end/backspace/delete/delete_word_*/delete_to_line_*/up/down/line_up/down` 全部在（`:342`–`:760`），CJK/emoji/ZWJ 有测试。**多行显示已完成**：`layout`/`layout_within`（`:616`/`:626`）按面板列宽折行（和转写同一个 `rig_tui::wrap::wrap`），`MOST_LINES=10`（`:100`）、最少 3 行（`LEAST_ROWS`）、光标按真实行列反色下划线（`CURSOR` `:210`、`with_cursor` `:243`），`editor_rows`（`:1386`）每行一个 `KeyedRow`，tile 高度随内容 `input_tile_height`（`:125`）+ `sync_editor_tile`（`:1368`）；测试 `:2602`/`:2665`/`:2926`/`:3002`/`:3096`。缺：**输入框内作答**——上游 `answer`/`answering`/`take_answer`（上游 `:188`/`:202`/`:208`）在 `grep -rE 'answering\|take_answer' rig_niri/src` 是 0；`Editor` 是 `Resource`（`:85`）不是 `Component`（上游 `:61`），`MainEditor`（上游 `:83`）/`MainInput` 不存在；输入框内按状态换提示（上游 `draw_editors` `:686`）没有。 |
| 8 | `highlight.rs`（444） | `rich.rs:106` | ✅ | `highlight_rows` 调 `rig_tui::highlight::code_rows`（`rich.rs:106`）；围栏代码的语法色由 `markdown::render` 内部的 `Grammars` 带上（`rich.rs:92`，建批的 `batch::reply_lines` 直接调上游 `markdown::render`），所以回复里的代码块有语法色。 |
| 9 | `hints.rs`（102） | `hints.rs` 241 | 🟡 | 有：两张键位表逐字相同（`:26`–`:45`，与上游 `:48`–`:70` 一致）、`fit_keys`（`:50`）、独立提示 tile（`hint_rows` `:118`、`hint_panel_rows` `:131`、`PanelId::Hints`）。缺口：`fit_keys` **三处行为与上游不同**（`:50`–`:74` vs 上游 `:79`–`:100`）——上游返回 `format!(" {} ", …)` 带首尾空格、按 `unicode_width` 算宽、同权重时丢**最后**一个（`Reverse(at)`）；我们返回 `" · "` 裸 join（无首尾空格）、`chars().count()`、丢**第一个**。提示**恒显示**（`hint_panel_rows` 每帧 `turn_running=false`，`:147`；上游只在输入为空时显示）。文案已跟上实际：`Ctrl+V paste`（`:33`/`:41`），不再是 `Ctrl+V image`。 |
| 10 | `history.rs`（223） | `history.rs` 733 | ✅ | `PromptHistory`（`:50`）含 `load`（`:65`）/`record`（`:114`）/`seed`（`:126`）/`rewrite`（`:136`）、`Sent`（`:207`）、`HistorySystems::{Seed,Record,Save}`（`:193`）；系统 `load`（`:214`）/`record`（`:235`）/`save`（`:287`）/`seed_shown`（`:328`）；JSONL 一行一个字符串、坏行跳过、去重按最后位置（测试 `:489`–`:714` 共 12 个）。差异：文件路径由插件参数给（`HistoryPlugin` `:374`），不用上游的 `PluginData`/`Files`；`Editor` 自己是历史持有者，所以多一层 `mirror`（`:312`）。 |
| 11 | `input.rs`（1026） | 分散在 `editor.rs`/`keys.rs`/`pane.rs`/`selection.rs`/`complete.rs` | 🟡 | 有：`PendingInput`（`keys.rs:821`）、按键分派（`editor.rs:1107` `keys`）、作用域/焦点（`keys.rs`）、滚轮 3 行（`pane.rs:592`，`WHEEL_ROWS` `:138`）、拖选/双击/三击（`selection.rs:943` `pointer_selection`）、补全接受（`editor.rs:1031`）。**"键 → 会话"的动作已接**：Enter → `Deliver`（`DeliveryMode::Steer`，`editor.rs:1220`）、turn 跑着时 Tab → `Queue`（`editor.rs:987`）、Esc → `StopTurn`（`editor.rs:1225`）、空输入 Ctrl+D → `AppExit`（`editor.rs:947`/`:1229`）、Alt+1..9 → `answer_ask`（`editor.rs:1198`–`:1203`/`:860`）、Alt+0/Esc → `refuse_ask`（`:1201`/`:1182`–`:1187`/`:875`）。缺：**细粒度**——`InputSystems::{Filter,Dispatch}` / `TuiKeys`（上游 `:111`/`:116`）、`InputContext`（上游 `:130`）都没有；`act` 里的 `SelectUp/SelectDown/Deselect` 仍是空实现（`editor.rs:973`–`:975`），转写区的 Alt+↑/↓ 是 `pane_keys` 直接读物理键做的（`pane.rs:429`–`:442`），不经过绑定表；`Deliver::literal` 的"命令被拒后当普通消息重发"没有（本 crate 没有 `refused`，`editor.rs:1013`–`:1020`）。 |
| 12 | `jsonl.rs`（75） | `history.rs` | ✅ | "一行一个 JSON 值"的追加/读取落在 `PromptHistory::{load,record,save}`（`history.rs:65`/`:114`/`:287`）里，坏行跳过不 panic。上游的 `tail`（跟随文件）没有；我们没有"跟读历史文件"的需求。 |
| 13 | `keys.rs`（751） | `keys.rs` 1457 | 🟡 | 有：`KeyPress`（`:145`）、`Keys`（`:279`）、`KeyScope`（`:335`）、`KeyBinding`（`:372`）、`KeyPressed`（`:417`）、`KeyFocus`（`:429`）、`AppKeyBindingsExt`（`:432`）、`InputAction` **32 个**（`:481`）、`built_in()` **43 条**（`:553`）、`add_built_in`（`:641`，另生成转写 Esc + Alt+1..9/Alt+0 共 11 条 → 54 个绑定）、`InlineKey`（`:637`）、`arm_inline_keys`（`:692`）、`ask_is_visible`（`:680`）、`Found`/`find` 多键前缀（`:728`/`:741`）、`released`（`:806`）、冲突拒绝 `RefusedKeys`（`:473`）。缺：`run_command`（上游 `:455`，`keys.rs:23` 自注未移植）；`KeyPressed.agent` 恒 `None`（`editor.rs:1157`/`:1259`，上游填显示中的 agent）；`Reflect` 没有；`tell_refused` 走 `Notice` 的上游行为只降级成 `warn!`（`keys.rs:23`–`:25`）。 |
| 14 | `markdown.rs`（193） | `rich.rs:92` + `batch::reply_lines` | ✅ | `markdown_rows` 调 `rig_tui::markdown::render(text,&theme,Some(width))`（`rich.rs:92`）；回复正文直接走上游 render（`batch.rs::reply_lines`），标题/列表/表格/引用/提示块都按上游规则。`render_marked` 没用到——它只服务上游的 `TranscriptRender` 查找标记（上游 `renderers.rs:629`），我们没有搜索高亮，见 `view`。 |
| 15 | `panel.rs`（344） | `frame.rs` + `layout.rs` + `panels.rs` | 🟡 | 有：布局职责——`lay_out`（`frame.rs:367`）、`lay_out_frame`（`:537`）、`place_panels`（`:617`）、树叶子/容器矩形（`layout.rs:177`/`:195`）、`PanelRows`/`PanelRoots`/`PanelSpawned`（`panels.rs:82`/`:112`/`:130`）、`sync_panels`（`:271`）、`MIN_TRANSCRIPT`（`frame.rs:59`）。`Placement` 从上游 7 种收成 `Tree`/`Rest`（`frame.rs:281`），`Over`/`Within` 按工作区模型删掉（`grep -rE 'Placement::(Over\|Within)' rig_niri/src` 为空，有意）。**尺寸已跟窗口**：`FrameSize`（`:102`）每帧由 `sync_frame_size_to_window`（`:504`）从主窗口逻辑像素换算（`from_window_size` `:165`），`RF_FRAME` 可钉死（`FrameEnv` `:190`/`FrameSizePinned` `:244`）；测试 `frame.rs:1642`/`:1691`/`:1715`。缺：**开放画布**——上游任何实体挂 `TuiPanel`（`panel.rs:106`）就有 `PanelCanvas`（`:160`，`render`/`buffer_mut`/`copy_to`），我们只有封闭枚举 `PanelId`（`panels.rs:33`，10 个变体），插件要加面板必须改核心枚举；`RequestRedraw`/`RedrawArea`（上游 `:82`/`:91`）没有。 |
| 16 | `pane.rs`（392） | `pane.rs` 1912 | 🟡 | 有：`PaneScroll{top,follow}`（`:130`）、窗口化 `window_transcript`（`:272`）、PgUp/PgDn（`:455`–`:460`）、Shift+↑/↓ 逐行（`:444`–`:451`）、滚轮（`pane_wheel` `:592`）、"下面还有"标记（`BELOW_MARKER` `:238`）——**两种标记都会画**：`BelowSeen` 已是资源（`transcript.rs:812`），`window_transcript` 每帧 `seen.of(...)`（`:296`）判定 `Below::{More,New}`（`:299`–`:309`），`↓ more below` / `↓ new output` 都按宽度裁剪（比窗格宽时整句不画，最后一行仍留空）；**Alt+↑/↓ 选条目**（`select_pane_item` `:333`，入口 `:429`–`:442`；块身份 `item_rows` `:109`，每帧重算 `sync_item_selection` `:479`）、选中后 Esc 清选区并把键交回输入框（`:409`–`:413`/`release_transcript_focus` `:536`）。缺：`TranscriptPane`/`title`/`TranscriptSource::{Shown,Agent,Value}`（上游 `:57`/`:84`）——我们只有全局 `PanelId::Transcript`，不能"看子 agent"或"看已保存会话"，`Pane2` 每帧复制同一份可见行（`panels.rs:143`）；`MainPane`/`PaneKeys`（上游 `:101`/`:125`）——滚动/选择键是物理 `KeyCode` 直读（`:385`–`:392` 自注），插件不能重绑；`DrawnTranscript` 没有。 |
| 17 | `pick.rs`（1098） | `picker.rs` 2635 | 🟡 | 有：`PickerInput`（`:56`）、标题/说明/过滤（`picker_rows` `:957`）、树折叠 `unfold`（`:359`）、多选打勾 `tick`（`:396`）、`free_text`、`secret` 打码、PgUp/PgDn（`PAGE=10` `:45`）、`PickPreview` 列（`sync_preview_panel` `:697`）、点击行（`answer_picker` `:1223`）、高亮同步（`mark_highlighted` `:744`）。**队列已完成**：`PickerQueue`（`:104`）+ `Add<Picker>` 观察者 `enqueue_picker`（`:553`）+ `sync_picker`（`:568`）。**选项键已落地**：Alt+1..9 直接选第 n 个可见项并确认、Alt+0 取消（`picker_keys` `:826`，测试 `alt_one_picks_the_first_item_like_a_click` `:1950` 等 5 个）；`more`（→ 展开选中项说明）、列表体 `scroll`、`picked`（重新填充后按身份选回）都在 `PickerInput`（`:56`–`:95`）。缺：插件自定义选项键的公开 API `ChoiceKeys`/`ChoiceAction`/`ChoiceSystems`（上游 `:51`/`:55`/`:589`，本仓 0 命中；现在只有内置 Alt+数字）；`in_input`/`draft`（输入框内作答，上游 `:132`）没有，`grep -n 'in_input' rig_niri/src/picker.rs` 为 0。 |
| 18 | `render.rs`（1543） | `frame.rs` + `pane.rs` + `panels.rs` + `picker.rs` + `complete.rs` + `selection.rs` | 🟡 | **绘制管道整体换成 bevy**（有意）：帧布局 `frame.rs:367`、窗口化 `pane.rs:272`、行→节点 `panels.rs::sync_panels`（`:271`）、补全/选择 tile（`complete.rs:1042`/`picker_rows` `picker.rs:955`）、选区反色 `selection.rs:1087`。语义侧有：`CallStates`→`batch::live_state`（`batch.rs:477`）、`ShownConversation`→`batch::{conversation_frame,dialogue_batch}`（`batch.rs:768`/`:734`）、`asides`→`conversation_frame` 的 Slot、`decorate`→`transcript::request_lines`（`:683`）+ `batch::child_calls_look`（`batch.rs:1185`）、`cells_in`→`selection::row_text`。缺：搜索高亮 `light`/`lit`（上游 `:848`/`:899`）与 `view::Highlight` 全链路（0 命中，`grep -rn Highlight rig_niri/src` 只命中 `pick::Highlighted`）；`TuiFrame`（上游 `:1196`，`screen_row`/`area`/`covered`）没有；`TranscriptLayout::lay_out`（上游 `:437`，无终端导出）没有；`Paced` 33ms 流式节流与 `needs_redraw`（上游 `:75`）——我们每帧全跑（bevy 自己驱动，属有意）；`inbox_lines`（上游 `:1520`）没有（没有收件箱 UI）；`compacted_line`（上游 `:797`"N messages above were compacted"）不画（`batch.rs:2166` 注释说明它需要上游 `Replaced`/会话计数；测试 `:2199`–`:2200` 断言这行不出现）。 |
| 19 | `renderers.rs`（1068） | `batch.rs` 3528 | 🟡 | 有：五类 `RenderKey`（Tool/Streaming/Entry/Content/Aside）与四类 `RenderInput` 全填（`conversation_frame` `:768`/`rebuild_batch` `:504`）；`batch.pane`/`batch.agent` 真的设置；`ToolRender` 的 `theme`/`took`（`record_calls` `:405` 用 `Started` 算）/`output`/`children`（`CallTree` `:208`）/`item` 都填；`RenderOutput.{lines,decorations}`；`RenderSystems::{Render,Decorate,Edit}` 链（`BatchPlugin` `:1847`，`world.run_schedule(TranscriptRender)` `:1197`）；`text_rows` 直接复用上游。缺：`ToolRender.facts` 恒 `None`（TODO `:646`/`:674`/`:920`：内核 `Outcome` 没给 `ToolResultContext`）→ 退出码、`OutputCut`"输出被截断"提示、`FilesChanged` 等事实拿不到，上游 `header()` 的调用状态色带/`exit()` 也就少一半输入；`RenderRevision` 从不推进（`grep -rn RenderRevision rig_niri/src` 为 0）→ 每帧全量重建；模型流参数、调用效应还没出现时的 `RenderKey::Streaming` 预画没做（TODO `:666`）。 |
| 20 | `selection.rs`（54） | `selection.rs` 2452 | ✅ | 语义比上游多得多且对齐：grapheme 列（不是字节）、CJK/emoji ZWJ 只算一列、`Selection::press/drag/release`（`:260`/`:301`/`:314`）、复制文本的 `trim_end` + `join("\n")` + 整体 `trim_end` 与上游 `Selection::mark` 一致（`:324`）、`ranges_in`/`byte_ranges_in`/`all_ranges`（`:355`/`:386`/`:400`）、`SELECTED_MODIFIER=REVERSED`（`:62`）、鼠标系统 `pointer_selection`（`:943`）、反色绘制 `selected_row`（`:1087`）。**条目级选中**：`item`/`select_item`/`suspend_item`（`:199`/`:212`/`:227`），给 Alt+↑/↓ 用。上游没有的超集：双击选词/三击选行（`ClickKind` `:93`、`apply` `:458`）、窗口 resize 清选区（`clear_selection_on_resize` `:787`）、复制失败写状态行。 |
| 21 | `status.rs`（605） | `status.rs` 88 | 🟡 | 有：**直接调** `rig_tui::status::line(items,width)`（`status.rs:44`），所以 keep/`min_width`/"先缩后丢"与上游逐字相同；`AppStatus`+agent 的 `StatusItems` 合并（`:21`–`:38`）。缺：`StatusLine` 组件（上游 `:382`）与 `StatusLinePlugin`（`:243`）——任何面板都能画一条状态行，我们只有固定 `PanelId::Status`；`StatusParts`/`AppStatusPartsExt`（上游 `:296`/`:329`，`/plugins` 的显示/隐藏联动）；`StatusSettings` + `[status] hide`（上游 `:274`）——应用层用自己的 `[view] hide_status` 清面板替代；`StatusSystems::{Show,Edit}`（上游 `:216`）——我们全在 `Draw`；`no agent` 占位（上游 `:407`）没有；turn 级 items（上游 `:404` 读 `ActiveTurn`/`TurnOf` 的 `StatusItems`）没有，应用层 `sync_status_items` 只读 agent 的 `ActiveTurn` 计数。 |
| 22 | `theme.rs`（337） | `theme.rs` 52 + `rich.rs:63` | 🟡 | 有：10 个前景色 token（`theme.rs:10`–`:31`），`upstream_theme`（`rich.rs:63`）把底色当基底、用我们的语义前景混出 12 个 band（user/tool/added/added_word/removed/removed_word/selected/call_running/call_done/call_failed/note/important），markdown/diff 因此能拿上游颜色（`rich.rs:94`/`:126`）。缺：上游 `Theme` 的 17 个 band 与 `on`/`shade`/`without_color`/`selection`/`changed_word`/`panel`/`raised`/`of_terminal`（上游 `:138`–`:234`）没有对应公开 API，`plain`（NO_COLOR）无通路；**batch 用自己的 `RTheme::default()`**（`batch.rs:530`/`:739`/`:1243`/`:1492`）而不是 `upstream_theme(我们的 Theme)` → 工具卡片/entry 行的默认底色不跟我们的主题走；`panel_surface`/`raised_surface`/`input`/`call_waiting`/`code` 是 `Theme::on` 的默认值（`rich.rs:56`–`:62` 自注"猜的"）。 |
| 23 | `title.rs`（216） | `title.rs` 132 | 🔄 | OSC 133 语义提示符/OSC 2 标题/标题栈只对终端有意义，换成**窗口标题**（`WindowTitle` `title.rs:25`，`compose` `:36`）。状态机照抄：`needs you`（有 picker 或有未回答 `Ask`）/`working`（非子 agent 的 turn 在跑）/`error`（上个 turn 失败）/`idle`（`tell_state` `:71`，与上游 `:184`–`:195` 同条件）。缺：退出时恢复原标题（上游 `:209` `restore_title`）没有——窗口关闭即结束，价值低；`[title] mark_nonce_env`（上游 `:76`）不适用。 |
| 24 | `transcript.rs`（1783） | `transcript.rs` 1558 + `batch.rs` | 🟡 | 有：对话引擎 `Transcript`（`transcript.rs:501`）——按块缓存折行、宽度失效重折（`set_width`/`apply`）、`TextId`（`Message`/`Notice`/`StreamingReasoning`/`StreamingReply`/`Prompt`/`Inbox`，`:386` 一带；**没有 `Stopped` 变体**）、`BlockId`、`Slot`（`Top`/`After`/`Inside`，`:415`）、排序键与上游 `Part` 顺序一致（`rank`）、折行用上游 `rig_tui::wrap::wrap`（`wrap_rows` `:717`）、`Below`（`:764`）+ `BelowSeen`（`:812`）；建块在 `batch::conversation_frame`（`:768`）。**`[stopped]` 已画**：不加新变体，用 `TextId::Message { at, block: usize::MAX }`（`batch::STOPPED_BLOCK` `:196`；压入 `:967`–`:985`），因为应用层非聚焦条带清理时只保留 `TextId::Message`（`:191`–`:195` 注释）。缺：`PastEffects`（上游 `:434`，恢复会话时旧调用的事实）——`batch.rs:89` TODO；`Replaced`（上游 `:723`，被压缩移出对话的条目仍显示）——本仓只有 `batch.rs:2166` 注释提到它，没有实现（`grep -rn 'struct Replaced'` 为 0）；压缩标记 `── N messages above were compacted ──`（上游 `render.rs:797`）不画（`batch.rs:2166` 注释说明：需要 `Replaced`/会话计数）；`stop_lines` 的第二种文案 `[stopped before it was answered]`（上游 `:1669`）只统一画 `[stopped]`；`TextId::Inbox` 有槽位（`transcript.rs:462`）但没有生产者。 |
| 25 | `view.rs`（865） | `pane.rs` + `selection.rs` + `transcript.rs` | 🟡 | 有：转写位置与宽度（`PaneScroll` + `Transcript::set_width`）、文本选择（`selection.rs:260`）与反色绘制、画出来的行（`PanelRows[Transcript]`，`pane::window_transcript` `:272`）、notice 的落点（`TextId::Notice`，应用层 `rig_harness_in_3d/src/notice.rs` 桥接）。**条目级选择部分落地**：`Selection::item`/`select_item`（`selection.rs:199`/`:212`）+ `pane::select_pane_item`（`:334`）——Alt+↑/↓ 能选"一整块"（调用/消息）并滚进可见区，Esc 清掉并交还键。缺：`TranscriptView`/`TranscriptItem`/`CallItem` 这套**公开 API**（上游 `:143`/`:84`/`:112`）没有——选中态是 `Selection` 里的一个 `u32` 块序号，插件读不到"当前选中哪条/有哪些条"；`Highlight`/`LineSpan`/`TextMark`/`MarkIn`/`RowWrap`（上游 `:224`/`:241`/`:207`/`:174`/`:361`，搜索/标记）全没有；`ScrollTo`/`ItemRow`（上游 `:259`/`:271`，"滚到某条"是内部行为，无公开入口）；`ShowAgent`（上游 `:49`，切到另一个 agent）没有；`DrawnTranscript`/`DrawnRow`/`DrawnLine`（上游 `:316`/`:326`/`:341`，给插件读"画了什么"）没有；`Notices` 资源（上游 `:165`）没有；`load_draft`/`keep_draft`（上游 `:723`/`:701`，只在 reload 时保留未发送输入）没有。 |
| 26 | `tests.rs`（433） | — | ➖ | 上游的 TUI 集成测试（`TuiTestScreen` + 模拟按键），不是移植目标。我们的等价可测性在行是**值**：`rig_niri` 441 个内联测试 + headless `App`（`batch` 的端到端、`selection` 的指针事件、`workspaces` 的布局不变量）。 |

---

## 2. 我们的 25 个文件 → 上游

| # | 文件 | 行（实现/测试） | 对应上游 | 状态 | 一句话职责 |
|---|---|---|---|---|---|
| 1 | `batch.rs` | 3528（1890/1638） | `renderers.rs` + `render.rs` 的建批/调度 | 🟡 | 从对话 + 效应建 `RenderBatch`，跑 look，把行交给转写引擎；Ask 单独进确认 tile。 |
| 2 | `camera_rig.rs` | 128（128/0） | —— | ➕ | 3D 相机架（手柄/键盘），上游无对应。 |
| 3 | `clipboard.rs` | 606（349/257） | `clipboard.rs` + `terminal::CopyToClipboard` | 🟡 | 复制选区到系统剪贴板、`Copied N lines`、Ctrl+V 粘文本（失败可见）；图片粘贴未做。 |
| 4 | `complete.rs` | 1881（1206/675） | `complete.rs` | ✅ | `/` 命令、`@` 路径、插件 `CompletionSource`；候选 tile + 键位。 |
| 5 | `editor.rs` | 3437（1467/1970） | `editor.rs` + `input.rs` 的编辑动作 | 🟡 | 多行文本模型 + 历史 + grapheme 光标 + 折行显示/光标/高度同步；缺输入框内作答。 |
| 6 | `frame.rs` | 1883（1035/848） | `panel.rs` + `screen.rs` + `render.rs` 的帧布局 | 🟡 / `screen` ✅ | 树叶子 → 帧内格子区域 → UI 尺寸/3D 锚点；默认屏幕 7 个叶子；`FrameSize` 跟窗口。 |
| 7 | `front.rs` | 39（39/0） | `lib.rs` 的前端身份 | ✅ | `Front::new("feathers")` + `Answerer` + `ShowsPickers`。 |
| 8 | `hints.rs` | 241（160/81） | `hints.rs` | 🟡 | 两张键位表 + 裁剪成一行；3 处裁剪行为与上游不同，且恒显示。 |
| 9 | `history.rs` | 733（432/301） | `history.rs` + `jsonl.rs` | ✅ | 跨会话提示词历史（JSONL、去重、seed/record/save/rewrite）。 |
| 10 | `keys.rs` | 1457（1076/381） | `keys.rs` | 🟡 | 键位 IR、43 条内置绑定（+11 条）、32 个动作、作用域/焦点/插件绑定、内联回答键开关；缺 `run_command`。 |
| 11 | `layout.rs` | 1466（777/689） | ——（替代 `panel.rs::Placement`） | ➕ | niri 分裂树：`Split/Size/Child/Node`、`rects`/`placements`、增删列、`default_tree`。 |
| 12 | `lib.rs` | 186（186/0） | `lib.rs` | 🟡 | `FeathersSystems` + `FeathersViewPlugin` + prelude；没有一键全套/headless `add_view`。 |
| 13 | `niri.rs` | 771（575/196） | —— | ➕ | niri 平铺几何：图层/列/瓦片、`place_tiles`、`spawn_niri_screen`。 |
| 14 | `pane.rs` | 1912（662/1250） | `pane.rs` | 🟡 | 转写窗口化 + 滚动（PgUp/PgDn、Shift+↑↓、滚轮）+ `↓ more below`/`↓ new output` + Alt+↑↓ 选条目；没有多来源窗格。 |
| 15 | `panels.rs` | 612（345/267） | `panel.rs` 的 `PanelCanvas`/绘制侧 | 🟡 | 行是值：`PanelRows` → bevy_ui 节点（前缀增量 diff）；面板身份是封闭枚举。 |
| 16 | `picker.rs` | 2635（1270/1365） | `pick.rs` | 🟡 | 列表选择器（过滤/树/多选/填空/secret/preview）+ 队列 + 内置 Alt+1..9，是条带最右一列而不是浮层。 |
| 17 | `rich.rs` | 563（263/300） | `markdown.rs` + `highlight.rs` + `diff.rs` | ✅ | 上游三个纯值渲染器的适配层 + `upstream_theme`（给业务/插件用）。 |
| 18 | `selection.rs` | 2452（1204/1248） | `selection.rs` + `render::cells_in` + `input::moused` | ✅ | 鼠标拖选/双击/三击、grapheme 级命中、反色、复制、条目级选中身份。 |
| 19 | `spatial.rs` | 433（364/69） | —— | ➕ | 3D 卡片/漏斗/自由视角（空间 UI 的渲染侧），上游无对应。 |
| 20 | `status.rs` | 88（88/0） | `status.rs` 的裁剪 + 绘制 | 🟡 | 合并 `AppStatus`+agent `StatusItems`，调上游 `status::line` 裁成两条。 |
| 21 | `style.rs` | 53（53/0） | —— | ➕ | ratatui `Color` → bevy `Color` 的桥（含 256 色近似）。 |
| 22 | `theme.rs` | 52（52/0） | `theme.rs` | 🟡 | 10 个前景 token；band 由 `rich::upstream_theme` 混出。 |
| 23 | `title.rs` | 132（114/18） | `title.rs` | 🔄 | 状态机照抄，输出换成窗口标题。 |
| 24 | `transcript.rs` | 1558（851/707） | `transcript.rs` | 🟡 | 对话引擎（块/缓存/排序/折行/finalize/notice/`[stopped]` 槽位/`BelowSeen`）+ 行 → 节点。 |
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
  （`PanePlugin`/`BatchPlugin`/`StatusPlugin` 各自能跑，见 `rig_harness_in_3d/src/lib.rs:419` 的
  `headless_app` 与 `:432` 的 `engine_app`）。
- **没有可选 part 的装配器**：上游 `TuiPlugins`（`lib.rs:133`）用 `is_plugin_added` 去重，逐个加
  `StatusLinePlugin`/`KeyHintsPlugin`/`TerminalTitlePlugin`。我们的 `StatusPlugin`/`HintsPlugin`/
  `TitlePlugin` 是固定装进 `FeathersViewPlugin` 的（`lib.rs:156`–`:172`）。
- **`keeps_files("tui")` / `BuildFailedLooks`**：上游 `lib.rs:180`/`:398`。会话文件登记我们没有；
  构建失败的红色提示 look 也没有（`rig_harness_in_3d` 的 reload 场景用不到）。
- **`BatchPlugin` 不在视图插件里**：`FeathersViewPlugin::build` 的插件元组没有它（`lib.rs:156`–`:172`），
  应用必须自己加（`rig_harness_in_3d/src/main.rs:68`、`src/lib.rs:447`）。上游的
  `renderers::add(app)` 是在 `TuiViewPlugin::build` 里做的（上游 `lib.rs:172`）。
- **turn 级 `StatusItems` 没注册**：`finish` 只做 `Agent`（`lib.rs:181`），上游两行都做
  （`status.rs:226`–`:227`）。

### 3.2 `clipboard.rs`（图片粘贴通道）

- **文本粘贴已通**（不再是缺口）：Ctrl+V 绑定是 `InputAction::PasteImage`（`keys.rs:621`），
  `act` 把它变成 `Act::Paste`（`editor.rs:983`），由 `editor::keys` 调 `clipboard::paste_from`
  （`editor.rs:1237`–`:1252`，实现 `clipboard.rs:317`）：优先内存替身、其次系统剪贴板，有文本就
  `Editor::insert` 到光标处；失败写 `PASTE_FAILED`（`clipboard.rs:113`/`:116`），成功清掉上一次错误。
- **图片粘贴取不到图**：上游 `Do::PasteImage` 要 `Scratch::images` + `Processes::launcher`，再
  `clipboard::paste(...)`（上游 `input.rs:863`–`:877`）；我们的 `bevy_clipboard` 没开 `image`
  feature，模块文档写明只保留 `Paste::Image` 的应用语义（`clipboard.rs:11`–`:14`）。
- **拖入/粘贴出来的文件路径不解析**：上游 `dropped_images`/`image_file`/`local_url_path`/
  `percent_decoded`（`clipboard.rs:227`/`:296`/`:307`/`:316`）把终端粘贴的一段文本拆成图片路径；
  我们只有 `type_path`（`:290`），没有"从一段文本里认出路径"的入口（`grep -rn dropped_images rig_niri/src` 为 0）。

### 3.3 `editor.rs`（输入框内作答与组件身份）

- **多行显示已完成**（不再是缺口）：`layout`（`editor.rs:616`）/`editor_rows`（`:1386`）按列宽折行，
  光标落在真实行列（`CURSOR` `:210`/`with_cursor` `:243`），`MOST_LINES=10`（`:100`）之后随光标滚动，
  tile 高度由 `input_tile_height`（`:125`）+ `sync_editor_tile`（`:1368`）同步。
- **在输入框里作答**：缺 `answer`/`answering`/`take_answer`（上游 `editor.rs:188`–`:208`）。
  上游把问题答案放进输入框、Enter 回答、Esc 拒绝、关掉后把草稿放回；我们只有 `Editor` 的普通文本，
  `grep -rE 'answering|take_answer' rig_niri/src` 为 0。Alt+n 只能选固定选项（`editor.rs:908`）。
- **组件身份**：`Editor` 是 `Resource`（`editor.rs:85`），上游是 `Component` + `#[require(Caret)]`
  （上游 `:61`/`:92`），并有 `MainEditor`（上游 `:83`）/`MainInput` 让插件用
  `Single<&mut Editor, With<MainEditor>>`。
- **输入框内的按键提示**：上游在 `draw_editors`（上游 `:686`）里按状态换（recalled / 面板拿走键 /
  正在作答 / 空输入），我们只有独立的 `PanelId::Hints` 一行，且恒显示（见 3.4）。

### 3.4 `hints.rs`（裁剪与显示时机）

- **三处行为不同**，都在 `fit_keys`（`hints.rs:50`–`:74`）：上游 `" {} "` 带首尾空格（上游 `:87`）→
  我们 `items.join(" · ")` 无首尾空格（`:60`）；上游 `.width()`（unicode 宽度）→ 我们 `chars().count()`
  （`:61`）；上游同权重丢**最后**（`min_by_key(|(at,(_,keep))| (*keep, Reverse(*at)))`，上游 `:90`）→
  我们只按 weight 取最小、丢**第一个**（`:65`–`:72`）。后果：中文/emoji 提示行会多留或少留一个；
  窄面板下先丢哪个不一样。
- **恒显示**：`hint_panel_rows`（`hints.rs:131`）每帧用 `turn_running=false` 产行，不看输入是否为空
  （`:147`）；上游只在空输入时画（上游 `editor.rs` 的 `draw_editors`：`else if !editor.is_empty()`）。

### 3.5 `input.rs`（键 → 会话：细粒度部分）

- **已接**（不再是缺口）：`Send` → `Deliver::new(agent, text, DeliveryMode::Steer)`（`editor.rs:1220`）；
  turn 跑着时 `Tab` → `Queue`（`editor.rs:987`、`:1010`）；`Esc` → `StopTurn::new(agent)`
  （`editor.rs:1225`）；空输入 `Ctrl+D` → `AppExit`（`editor.rs:947`/`:1229`）；Alt+1..9 → `Act::AnswerInline`
  （`editor.rs:976`/`:1198`）；Alt+0 → `Act::DismissInline`（`:977`/`:1201`）；有未回答 `Ask` 时 Esc 先拒绝
  （`:1182`–`:1187`）。
- **过滤/上下文**：没有 `InputSystems::Filter`（插件在绑定之前读/改/丢键）、`TuiKeys` 每键一次的
  调度（上游 `input.rs:111`–`:116`）、`InputContext`（上游 `:130`，`typed`/`completion`/`answering`）。
  我们的 `editor::keys`（`editor.rs:1107`）一次读完这一帧的 `KeyboardInput` 直接分派。
- **条目选择的键位通道**：转写区的 Alt+↑/↓、Shift+↑/↓、PgUp/PgDn 在 `pane_keys` 里**直接读物理键**
  （`pane.rs:385`–`:392` 自注），`InputAction::{SelectUp,SelectDown,Deselect}` 在 `act` 里仍是空实现
  （`editor.rs:973`–`:975`）——能力有，插件不能重绑。
- **`Deliver::literal`**：本 crate 没有 `refused` 概念，所以没有"命令被拒后当普通消息重发"
  （`editor.rs:1013`–`:1020`、`:1216`）。

### 3.6 `keys.rs`（插件侧）

- **`run_command`**：上游 `keys.rs:455`，把斜杠命令绑到键上。我们没有（`keys.rs:23` 自注）——应用层
  自己把输入框里 `/` 开头的提交转成 `RunCommand`（`rig_harness_in_3d/src/lib.rs:362`），
  插件键位到这个链路没有公开入口。
- **`KeyPressed.agent` 恒 `None`**（`editor.rs:1157`/`:1259`）：上游填显示中的 agent，插件的观察者
  据此判断"哪个 agent 的键"。
- **`Reflect` / `tell_refused`**：上游这些组件都 `derive(Reflect)` 供 `/plugins`、场景保存；
  键位冲突我们只 `warn!` + `RefusedKeys`（`keys.rs:473`、`:23`–`:25`），上游还会发 `Notice`。

### 3.7 `panel.rs` / `panels.rs`（开放画布）

- **插件画不了自己的面板**：上游 `TuiPanel`（`panel.rs:106`）+ `PanelCanvas`（`:160`，`render`/
  `buffer_mut`/`copy_to`）；我们是封闭 `PanelId`（`panels.rs:33`，10 个变体）+ `PanelRows`（`:82`），
  加面板要改核心枚举。
- **按需重绘没有**：`RequestRedraw`/`RedrawArea`（上游 `:82`/`:91`）。我们每帧跑整个 `Update` 链。
- **尺寸已跟窗口**（原来的缺口，已修）：`FrameSize`（`frame.rs:102`）由 `sync_frame_size_to_window`
  （`:504`）每帧从 `Window::size()`（逻辑像素）换算，值变了才写资源；`RF_FRAME` 可钉死
  （`FrameEnv` `:190`/`FrameSizePinned` `:244`）；测试 `frame.rs:1642`/`:1691`/`:1715`。

### 3.8 `pane.rs`（多来源窗格）

- **多来源窗格**：`TranscriptPane`/`TranscriptSource::{Shown,Agent,Value}`（上游 `pane.rs:57`/`:84`）
  → 只有全局 `PanelId::Transcript`；`Pane2`（拆窗）每帧复制同一份可见行（`panels.rs:143`）。
- **公开的滚动/选择键**：Alt+↑/↓ 选条目、Shift+↑/↓ 逐行、PgUp/PgDn 已经实现
  （`pane.rs:429`–`:460`），但走的是物理 `KeyCode` 而不是 `PaneKeys` 绑定（上游 `:125` 的 6 条绑定），
  所以插件不能改键；`ScrollTo`/`ItemRow` 也没有公开入口。
- **两种"下面还有"标记都已画**（不再是缺口）：`BelowSeen` 是资源（`transcript.rs:812`，
  `pane.rs:634` 初始化），`window_transcript` 每帧 `seen.of(...)`（`pane.rs:296`）决定
  `Below::{More,New}`，标记文案来自 `Below::marker`（`transcript.rs:784`），按窗格宽度裁剪
  （`pane.rs:299`–`:309`）。

### 3.9 `pick.rs` / `picker.rs`（公开选项键与输入框作答）

- **队列已完成**（不再是缺口）：`PickerQueue`（`picker.rs:104`）+ `Add<Picker>` 观察者
  `enqueue_picker`（`:553`）+ `sync_picker`（`:568`）；测试 `picker.rs:2356`/`:2438`/`:2473`/`:2505`。
- **内置 Alt+1..9 已落地**（不再是缺口）：Alt+1..9 选第 n 个可见项并确认、Alt+0 取消，
  只在 picker 显示时由 `picker_keys`（`picker.rs:826`）接管；测试 `alt_one_picks_the_first_item_like_a_click`
  （`:1950`）、`alt_three_picks_the_third_item`（`:1971`）、`alt_zero_cancels_the_picker`（`:1986`）、
  `alt_past_the_last_item_answers_nothing`（`:1998`）、`alt_digits_stay_with_the_input_when_no_picker_shows`
  （`:2009`）。
- **还缺**：插件自定义选项键的公开 API `ChoiceKeys`/`ChoiceAction`/`ChoiceSystems`
  （上游 `:51`/`:55`/`:589`，本仓 0 命中）——现在只有内置 Alt+数字，插件加不了自己的键；
  `in_input`/`draft`（在输入框里作答，上游 `:132`）没有。`scroll`/`more`/`picked` 已进
  `PickerInput`（`:56`–`:95`）。

### 3.10 `render.rs`（非绘制部分）

- **搜索高亮**：`view::Highlight`/`LineSpan` + `render::Lights`/`light`/`lit`（上游 `render.rs:848`/
  `:899`）全链路 0 命中，`grep -rn "Highlight" rig_niri/src` 只命中 `pick::Highlighted`（无关）。
- **帧导出**：`TuiFrame`（上游 `:1196`，`screen_row`/`area`/`covered`）没有；`TranscriptLayout`
  （上游 `:437`，无终端导出转写行）没有——插件拿不到"第 n 行在屏幕哪"。
- **收件箱/压缩提示**：`inbox_lines`（上游 `:1520`）没有生产者（本前端没有收件箱 UI）；
  `compacted_line`（上游 `:797`）不画（`batch.rs:2166` 注释；测试 `:2199` 断言不出现）。

### 3.11 `renderers.rs`（事实与版本）

- **`facts: None`**：`batch.rs:646`/`:674`/`:920` 三处 TODO——内核的 `Outcome` 只给输出文本，没有
  `ToolResultContext`，于是 `header()` 的调用状态色、`exit()` 的退出码、`OutputCut` 的"输出被截断"
  行、`FilesChanged` 都没有；`took` 只在结算时从 `Started` 算（`record_calls` `:405`），运行中没有。
- **`RenderRevision`**：0 命中 → 每帧全量 `rebuild_batch`（`:504`）+ `apply`（`transcript.rs:532`），
  diff 靠内容比较兜住，但大对话的 CPU 成本比上游高。
- **流式预画**：模型流参数、调用效应还没出现时，上游先画 `CallState::Streaming` 的卡片；
  我们等效应出现（`batch.rs:666` TODO）。

### 3.12 `status.rs`（开放状态行）

- **`StatusLine` 组件 / `StatusLinePlugin`**（上游 `:382`/`:243`）：任何面板（尤其是输入框）都能
  带一条状态行；我们只有固定 `PanelId::Status` 一块 tile，输入框底行没有状态。
- **`StatusParts`/`AppStatusPartsExt`**（上游 `:296`/`:329`）：`/plugins` 能按名字显示/隐藏某个
  `StatusItem`；我们没登记 part（上游在 `lib.rs:169` 用 `add_status_part` 登记 `COPIED`）。
- **`StatusSettings`/`[status] hide`**（上游 `:274`）：应用层用 `[view] hide_status` 清空面板
  （`rig_harness_in_3d/src/status.rs::apply_view_settings`）替代。
- **`no agent` 占位 / turn 级 items**（上游 `:407`/`:404`）：没有 agent 时上游显示 "no agent"；
  本 turn 的 `StatusItems`（token 速率、进度）我们没读。

### 3.13 `theme.rs`（band 与主题注入）

- **17 个 band + 方法**：`on`/`shade`/`without_color`/`selection`/`changed_word`/`panel`/`raised`/
  `of_terminal`（上游 `:138`/`:175`/`:191`/`:200`/`:221`/`:227`/`:234`）没有；`panel_surface`/
  `raised_surface`/`input`/`call_waiting`/`code` 在 `upstream_theme` 里是 `Theme::on` 的默认值
  （`rich.rs:56`–`:62` 注释自认"猜的"）。
- **batch 不跟我们的主题**：`RTheme::default()` 写死在 `rebuild_batch`/`conversation_frame` 一带
  （`batch.rs:530`/`:739`/`:1243`/`:1492`），`crate::theme::Theme` 只被 `rich.rs` 调到。
- **NO_COLOR**：没有通路（上游 `:191` `without_color`）。

### 3.14 `transcript.rs`（几处可见缺口）

- **`PastEffects`**（上游 `:434`）：恢复会话时从效应日志重建旧调用的事实（`took`、调用树、
  子 agent 的调用）。`batch.rs:89` TODO；后果：重启后旧卡片没有时长/调用树。
- **`Replaced`**（上游 `:723`）：压缩移出对话的条目仍显示（`view::TranscriptItem::Replaced` 能选）；
  本仓只有 `batch.rs:2166` 一行注释提到它，没有实现；压缩摘要也不画。
- **停止标记只画一种**：`[stopped]` 已画（`batch.rs:967`–`:985`，用
  `TextId::Message { at, block: usize::MAX }` = `batch::STOPPED_BLOCK` `:196`，见 `:191`–`:195`
  的取舍说明），上游 `stop_lines`（`transcript.rs:1669`）还有
  `[stopped before it was answered]`。**`TextId::Inbox`** 有槽位（`transcript.rs:462`）无生产者。

### 3.15 `view.rs`（公开选择 API / 导出）

- **条目级选择**：能力有（`Selection::item`/`select_item` `selection.rs:199`/`:212` +
  `pane::select_pane_item` `:333`），但**公开 API 没有**：`TranscriptView::selected` 不存在，
  插件读不到"第 n 个调用/当前选中哪条"（上游 `:143`/`:84`/`:112`）。
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
| 复制 | OSC 52 + base64 | `bevy_clipboard` 系统剪贴板（`clipboard.rs:231`） |
| 尺寸/焦点事件 | crossterm resize/focus | bevy `Window`：`sync_frame_size_to_window`（`frame.rs:504`）+ 物理→逻辑像素换算（`feathers_in_3d/src/world3d.rs:65`–`:88`） |
| `TuiTestScreen` | 内存 `Buffer` 断言 | 行是值：`PanelRows` + headless `App`（441 个测试） |
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

## 5. 上一版（第二次审计）被推翻的结论

| 旧文（20:10 快照） | 现在的证据 |
|---|---|
| "文本粘贴没接线：`paste_from_clipboard` 没有调用者；Ctrl+V 绑的 `PasteImage` 在 `act` 里是空实现（`editor.rs:481`）" | 已接：`Do::PasteImage => Act::Paste`（`editor.rs:983`），`editor::keys` 调 `clipboard::paste_from`（`editor.rs:1237`，`clipboard.rs:317`），失败写 `PASTE_FAILED`（`clipboard.rs:113`/`:116`）；测试 `ctrl_v_pastes_text_at_the_cursor`（`editor.rs:3215`）等 3 个，成功粘贴会清掉上一次错误。 |
| "`answer_inline` 空实现（`editor.rs:479`）；Alt+1..9 的绑定被 `Disabled` 挂起（`keys.rs:641`–`:655`）" | 键盘作答已实现：`Do::AnswerInline/DismissInline` → `Act`（`editor.rs:976`–`:977`）→ `answer_shown_ask`/`refuse_shown_ask`（`:908`/`:927`）→ 与点击同一个落点 `answer_ask`/`refuse_ask`（`:860`/`:875`）；`keys::arm_inline_keys`（`keys.rs:692`）按确认面板内容摘/挂 `Disabled`；Esc 有未回答 `Ask` 时先拒绝（`editor.rs:1182`–`:1187`）。测试 `editor.rs:2136`/`:2156`/`:2207`/`:2227`。 |
| "picker 没有队列：`sync_picker` 只取 `pickers.iter().next()`" | `PickerQueue`（`picker.rs:104`）+ `Add<Picker>` 观察者（`:551`）+ `sync_picker`（`:568`）按创建顺序排队、一次一列、关掉后同帧交接；测试 `pickers_show_one_at_a_time_in_creation_order`（`:2356`）等 6 个。 |
| "`FrameSize` 从 env 读固定格子数，不跟窗口 resize（`frame.rs:86`–`:107`）" | `FrameSize`（`frame.rs:102`）现在每帧由 `sync_frame_size_to_window`（`:504`）从主窗口逻辑像素换算，值变了才写；`RF_FRAME` 生效时钉死（`FrameEnv` `:190`/`FrameSizePinned` `:244`）。测试 `the_frame_size_follows_the_primary_window`（`:1642`）、`resizing_the_window_relayouts_the_panels`（`:1715`）。 |
| "多行显示没做：`editor_rows` 只产 1 行，光标是 `▏` 字符；输入框高度固定" | `layout`/`layout_within`（`editor.rs:616`/`:626`）按列宽折行，`MOST_LINES=10`（`:100`）、可见行随光标滚动，`editor_rows`（`:1386`）每行一个 `KeyedRow`，光标按真实行列反色（`with_cursor` `:243`），tile 高度 `input_tile_height`（`:125`）+ `sync_editor_tile`（`:1368`）；测试 `editor.rs:2602`/`:2665`/`:2926`/`:3002`/`:3096`。 |
| "`grep Deliver` 为 0；Send/Tab/Stop/Exit 在 `act` 里是空实现" | `Deliver` 25 处命中：Enter → `Deliver::new(.., Steer)`（`editor.rs:1220`），转写里 Tab 在 turn 跑着时 `Queue`（`:987`），Esc → `StopTurn`（`:1225`），空输入 Ctrl+D → `AppExit`（`:947`/`:1229`）。 |
| "转写 Alt+↑↓ 没有目标：`SelectUp/SelectDown/Deselect` 空实现；`KeyScope::Transcript` 没有系统切进去" | 条目选择已落地：`pane::select_pane_item`（`pane.rs:333`，入口 `:429`–`:442`）+ `Selection::item/select_item`（`selection.rs:199`/`:212`），Esc 清选区并交还键（`pane.rs:409`–`:413`/`:536`）；测试 `alt_down_selects_items_in_order_and_alt_up_walks_back_without_overshooting`（`pane.rs:1306`）等 9 个。仍需注意：它是物理键直读，`act` 里的 `SelectUp/SelectDown` 仍是空实现（`editor.rs:973`–`:975`）。 |
| "输入补全 … 只差 `.gitignore` 感知与两条抑制"（这条仍然对） | 保持不变：`complete.rs:29`–`:31` 自注不读 `.gitignore`；`complete.rs:24`–`:26` 自注两条抑制没做。 |
| "`↓ new output` 不画：`window_transcript` 只放 `BELOW_MARKER`，`BelowSeen` 没人持有（`transcript.rs:811`–`:813`）" | 已画：`BelowSeen` 是资源（`transcript.rs:812`），`window_transcript` 每帧 `seen.of(...)`（`pane.rs:296`）在 `Below::{More,New}` 间切换，按窗格宽度裁剪（`pane.rs:299`–`:309`）；测试 `pane.rs:926`/`:975` 断言 `seen`/`scrolled`。 |
| "picker 不能用 Alt+1..9 选，只能点" | 已落地：`picker_keys`（`picker.rs:826`）在 picker 显示时接管 Alt+1..9（选第 n 个可见项并确认）与 Alt+0（取消），测试 `alt_one_picks_the_first_item_like_a_click`（`:1950`）等 5 个；`more`/`scroll`/`picked` 也进了 `PickerInput`（`:56`–`:95`）。插件自定义键（`ChoiceKeys`）仍没有。 |
| "`[stopped]` 不画（`batch.rs:862` TODO）" | 已画：`batch.rs:967`–`:985` 用 `TextId::Message { at, block: usize::MAX }`（`STOPPED_BLOCK` `:196`）画 dim 的 `[stopped]`，来源是 turn 记录 `TurnEnd::Stopped`（`:783`）。没有新增 `TextId` 变体（原因见 `:191`–`:195`）。 |
| "`rig_niri` **317** 个测试（基线）" | 现在 **`#[test]` 441 个**（§0 有实测口径与时刻），且 `picker.rs` 还在加。 |

---

## 6. 复现

```sh
cd /var/home/lichi/桌面/bevy_3d_ai_research
UP=.cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/rig-tui-0.47.0/src

# 文件数 / 行数
find rig_niri/src -name '*.rs' | wc -l                      # 25
find rig_niri/src -name '*.rs' -print0 | xargs -0 wc -l | tail -1   # 29219（22:56 快照）
find "$UP" -name '*.rs' | wc -l                              # 46（25 实现 + 21 tests.rs）
find "$UP" -name '*.rs' ! -name 'tests.rs' -print0 | xargs -0 wc -l | tail -1  # 14316
find "$UP" -name 'tests.rs' -print0 | xargs -0 wc -l | tail -1        # 9395

# 测试数
grep -rho '#\[test\]' rig_niri/src | wc -l                   # 441（22:50；并行改动中会变）
grep -rh '#\[test\]' "$UP" | wc -l                           # 246

# 实现/测试行拆分（各文件从第一个 #[cfg(test)] 到文件尾算测试）
for f in rig_niri/src/*.rs; do
  s=$(grep -n '^#\[cfg(test)\]' "$f" | head -1 | cut -d: -f1)
  [ -n "$s" ] && echo "$f impl=$((s-1)) test=$(($(wc -l < "$f")-s+1))" || echo "$f impl=$(wc -l < "$f") test=0"
done

# 测试
export CARGO_HOME=$PWD/.cargo-home
cargo test --workspace --offline

# 本轮改掉的关键结论，逐条可查
grep -rn "Deliver" rig_niri/src | wc -l                      # 25
grep -n "Do::PasteImage" rig_niri/src/editor.rs              # 983
grep -nE "fn (answer|refuse)_ask" rig_niri/src/editor.rs     # 860 / 875
grep -n "pub struct PickerQueue" rig_niri/src/picker.rs      # 104
grep -nE "fn (alt_|picker_keys)" rig_niri/src/picker.rs      # 内置 Alt+1..9 的测试与入口
grep -n "fn sync_frame_size_to_window" rig_niri/src/frame.rs # 504
grep -n "fn select_pane_item" rig_niri/src/pane.rs           # 333
grep -n "let below = seen.of" rig_niri/src/pane.rs           # 296（↓ new output）
grep -n "STOPPED_BLOCK" rig_niri/src/batch.rs                # 196 / 978 一带（[stopped]）
grep -rn "TextId::Stopped" rig_niri/src                      # 0（用 Message 的 block=usize::MAX）
```

---

## 7. 怎么读这份文档

1. 想知道"某个能力有没有"：先看 §1 的对应上游模块，再看 §3 的同名小节。
2. 想知道"还差什么、先做哪个"：看 [`REMAINING.md`](REMAINING.md)（按用户能感知的价值排序）。
3. 想改代码前确认边界：§2 的"我们的 25 个文件"给每个文件的职责与上游对应；6 个 ➕ 文件
   是自有设计（`layout`/`niri`/`workspaces`/`spatial`/`camera_rig`/`style`），改它们不算"偏离上游"。
4. 文档里的数字都能用 §6 的命令重新数；`文件:行` 是 22:5x 快照的位置，`picker.rs` 正在并行改动，
   以函数名为准（见开头的快照警告）。
