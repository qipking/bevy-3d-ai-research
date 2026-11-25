# rig_niri ↔ rig-tui 移植缺口清单

> **性质**：只读审计。本文档是唯一新增文件，没有改动任何 `.rs`，没有动 git/jj 状态。
> **基准**：`rig-tui = 0.47.0`（`/var/home/lichi/桌面/bevy_3d_ai_research/.cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/rig-tui-0.47.0/src/`）
> 对 `rig_niri 0.1.0`（`/var/home/lichi/桌面/bevy_3d_ai_research/rig_niri/src/`）。
> **方法**：逐文件读两边源码；每条"部分/缺什么"都指到上游函数名或行号。
> **口径**：`已移植` = 该模块对外的职责已完整落地（可以换实现）；`部分` = 主路径在，但公开能力有明确缺口；`未开始` = 上游模块没有任何对应物；`有意替换` = 纯终端管道，用 bevy 窗口/渲染取代，不该移植。

---

## 0. 先看数字

| 指标 | rig-tui | rig_niri |
|---|---|---|
| `.rs` 文件 | **26**（含 `tests.rs` 集成测试 433 行） | **17** |
| 总行数 | **14749** | **4943** |
| 扣掉测试后的实现行 | 14316 | 4943 |
| 其中"上游没有的自有能力" | — | **1385 行**：`niri.rs` 771、`spatial.rs` 433、`camera_rig.rs` 128、`style.rs` 53 |
| 真正对位移植的代码 | — | ≈ **3558 行** |

**按行数覆盖率 ≈ 3558 / 14316 ≈ 25%**（这个数字还偏乐观：对位代码里有相当一部分是"重新设计的等价物"，不是同构移植，例如 `theme.rs` 完全换了一套字段）。

**模块状态分布（下文 §1 的 25 个模块 + 1 个测试模块）：**

| 状态 | 个数 |
|---|---|
| 未开始 | **11** |
| 部分 | **11** |
| 已移植 | **1**（`screen.rs`，46 行） |
| 有意替换 | **2**（`terminal.rs`、`title.rs`） |
| 不适用 | **1**（`tests.rs`，上游集成测试，不是移植目标） |

**公开 API 覆盖率（下文 §3 逐名对照 104 个名字）：**

| | 个数 | 比例 |
|---|---|---|
| ✅ 有同义对应 | 10 | 9.6% |
| ⚠️ 有对应但不同名/不同形 | 14 | 13.5% |
| ❌ 完全没有 | **80** | **76.9%** |

---

## 1. 总表

图例：✅ 已移植　🟡 部分　❌ 未开始　🔄 有意替换　➖ 不适用

| # | rig-tui 模块 | 行数 | 我们对应 | 行数 | 状态 | 缺什么（一句话 + 证据） |
|---|---|---|---|---|---|---|
| 1 | `lib.rs` | 422 | `lib.rs` + `front.rs` | 134+39 | 🟡 部分 | 只做了 `FeathersViewPlugin` 的 build/finish 骨架；缺 `TuiPlugin`/`TuiPlugins`（可选 part 的 `is_plugin_added` 去重）、`add_view`（无终端的测试装配）、`BuildFailedLooks`、`app.keeps_files("tui")`（`lib.rs:180`）、`register_required_components::<TurnOf, StatusItems>`（`status.rs:227`，我们只注册了 `Agent`，见 `lib.rs:129`）。 |
| 2 | `terminal.rs` | 620 | — | — | 🔄 有意替换 | 裸模式/备用屏/鼠标捕获/括号粘贴/`Terminal<CrosstermBackend>`/OSC 52 复制全部由 bevy 窗口 + `bevy_clipboard` 取代；但两个公开能力在窗口前端**仍然需要**且不存在：`RunInTerminal`（跑 `$EDITOR`）与 `CopyToClipboard`/`TerminalWrite`（`terminal.rs:57`、`:59`）。 |
| 3 | `screen.rs` | 46 | `frame::spawn_default_screen` | ~50 | ✅ 已移植 | 三个默认面板（输入/状态/转写）都 spawn 了；唯一的出入：`InlineStrip`（`screen.rs:38`）没有 spawn，因为内联问题功能整体未移植（见 #17 `pick.rs`）。 |
| 4 | `clipboard.rs` | 340 | — | — | ❌ 未开始 | `Cargo.toml` 里有 `bevy_clipboard`，`src/` 一次没用；缺 `paste`（`clipboard.rs:44`）、`Pasted` 异步回合（`:32`）、`words`/`unquoted`/`unescape`/`image_file`/`local_url_path`/`percent_decoded`（`:240`–`:316`）整套"粘贴/拖入路径 → `@path` 附件"逻辑。 |
| 5 | `complete.rs` | 681 | — | — | ❌ 未开始 | 没有 `CompletionSource`/`CompletionTrigger`/`Candidate`/`Completion`/`CompletionKind`；没有 `FileIndex` 与 `walk`（`:271`、`:322`，`.gitignore` 感知的目录树）、`token`/`match_at` 的 token 定位（`:364`、`:676`）、`Sources` 的候选聚合（`:409`）、`markers`（`:429`）。 |
| 6 | `diff.rs` | 318 | — | — | ❌ 未开始 | 没有 `unified_lines`（`:38`）/`change_lines`（`:51`）/`classify`（`:120`）/`highlighted`（`:212`）/`changed_words`（`:247`）/`overlay`（`:300`）；工具返回的 diff 现在只是普通文本（我们的 look 用 `result_text()` 自己拼行，见 `batch.rs:330`）。 |
| 7 | `editor.rs` | 770 | `editor.rs` | 190 | 🟡 部分 | `Editor` 是 `Resource` 不是 `Component`（上游 `#[require(Caret)]` 组件，`editor.rs:61`），只有 6 个键位；缺 `set`/`clear`/`take`/`answer`/`take_dropped`/`recalled`/`insert_char` 的 grapheme 语义、`word_left`/`word_right`（`:370`/`:375`）、`home`/`end`、`delete`/`delete_word_before`/`after`、`delete_to_line_start`/`end`、`up`/`down` 的"历史 + 草稿 + 跨行"模型（`:480`/`:518`）、`draw_editors` 的多行滚动（`MOST_LINES = 10`，`:28`、`:686`）。 |
| 8 | `highlight.rs` | 444 | — | — | ❌ 未开始 | 没有 `Code`（`:86`）/`Language`（`:44`）/`Code::line`（`:183`）/`code_rows`（`:387`）/`syntax`（`:399`，syntect 语法集）；代码块和命令参数都没有语法色。 |
| 9 | `hints.rs` | 102 | `hints.rs` | 81 | 🟡 部分 | 两张键位表逐字照抄（`hints.rs:13`），但 `fit_keys` 有 **3 处行为不一致**：上游返回 `format!(" {} ", …)` 带首尾空格（`:82`）我们用裸 join（我们 `:47`）；上游按 `unicode_width`（`:83`）我们按 `chars().count()`（我们 `:48`）；同权重时上游丢**最后**一个（`:90` 的 `Reverse(at)`）我们丢**第一个**（我们 `:55`）。且上游只在输入为空时显示（`editor.rs:47`），我们恒显（`editor.rs:156`）。 |
| 10 | `history.rs` | 223 | — | — | ❌ 未开始 | `Editor.history` 只是本次进程内的 `Vec<String>`（`editor.rs:24`）；缺 `PromptHistory` 资源与 `tui/history.jsonl` 持久化（`:16`、`load` `:65`）、`record` 的"编辑器丢弃的草稿 + `Sent` 消息"合并（`:176`）、`save`（`:209`）、`seed_shown`（`:129`）、`HistorySystems::{Record,Save,Seed}` 过滤钩子、`rewrite`（`:95`）。 |
| 11 | `input.rs` | 1026 | `editor::keys` + `pane::pane_keys` + `picker::picker_keys` | ~150 | 🟡 部分 | 按键处理散在三个模块里，没有 `PendingInput`/`KeyInput`/`InputContext`/`InputSystems`/`TuiKeys`；缺 `read_input`（`:463`）的"作用域→动作"分派、`pressed`（`:684`）/`released`（`:740`）的多键序列（`Keys` 多按）、`act`（`:765`）的 32 个动作语义、`send`（`:985`）的"斜杠命令/队列/turn 运行中转向"、`moused`（`:628`，滚轮 3 行 + 拖选）、`select`（`:922`）、`answer_inline`（`:950`）、`accept_completion`（`:1016`）、`note_input_context`（`:323`）。 |
| 12 | `jsonl.rs` | 75 | — | — | ❌ 未开始 | 没有"一行一个 JSON 值"的 `append`/`write`/`tail`（`:36`/`:48`/`:57`）与 `Kept` 文件句柄；历史、notice、草稿的持久化都缺这一层（`Files` trait 与 `PluginData` 是 rig-harness 的抽象，非终端相关，可直接用）。 |
| 13 | `keys.rs` | 751 | — | — | ❌ 未开始 | 完全没有键位机制：`KeyPress`/`Keys`/`KeyBinding`/`KeyScope`/`KeyFocus`/`KeyPressed`/`Keys`/`AppKeyBindingsExt`/`run_command` 全部不存在；`built_in()` 的 **43 条**绑定（`:570` 起）与 `InputAction` 的 **32 个**动作（`:484` 起）一个没落；`find` 的多键前缀匹配（`:722`）也没有 → **插件无法绑定自己的键，也收不到 `KeyPressed`**。 |
| 14 | `markdown.rs` | 193 | — | — | ❌ 未开始 | 没有 `render`（`:17`）/`render_marked`（`:74`）；`Grammars`（`:43`）把 fenced code 交给 `Code` 的通道也没有 → 回复里的标题/列表/表格/引用/代码围栏全按纯文本画。 |
| 15 | `panel.rs` | 344 | `frame.rs`（布局部分）+ `panels.rs` | ~500 | 🟡 部分 | `Placement` 七种、`lay_out` 的五步顺序、`MIN_TRANSCRIPT`（`:217`）、`take`（`:338`）、`below_rows`（`:229`）**逐条照抄**（我们 `frame.rs:297`–`:419`），这是移植质量最高的一块；缺的是**开放画布**：上游任何实体挂 `TuiPanel` 就有 `PanelCanvas`（`:160`，`render(widget)`/`buffer_mut()`/`copy_to`），我们只有封闭枚举 `PanelId::{Transcript,Status,Input,Overlay,List,Preview}`（`panels.rs:27`）→ 插件要加面板必须改核心枚举；另外 `RequestRedraw`（`:82`）/`RedrawArea`（`:91`）/`TerminalSize`（`:95`）无对应物。 |
| 16 | `pane.rs` | 392 | `pane.rs` | 249 | 🟡 部分 | 只有 `PaneScroll{top,follow}` + 窗口化 + **PgUp/PgDn**（`pane.rs:186`）；缺 `TranscriptPane` 组件（`:57`）与 `title` 行（`:76`）、`TranscriptSource::{Agent, Value}`（`:84`，即"看子 agent / 看存档会话"）、`MainPane`/`PaneKeys`（`:101`/`:125`）的 `Up`/`Down` 行滚与 `Alt+Up/Down` 选择（`:164`–`:195`）、`Below::{More,New}` 两种标记（我们只有 `BELOW_MARKER`，`pane.rs:117`）、`DrawnTranscript` 的产出、`pane::draw_below` 的右下角叠加（`:372`）。 |
| 17 | `pick.rs` | 1098 | `picker.rs` | 929 | 🟡 部分 | picker 侧覆盖不错（标题/说明/过滤/树折叠/多选打勾/`free_text`/`secret`/`PickPreview` 列/点击行）；缺：**`Ask` 完全不进浮层**（`picker.rs:292` 只取 `pickers.iter().next()`，上游 `queue_choices`（`:500`）把 `Picker` 和 `Ask` 放进同一个队列一次显示一个）、`InlineStrip` 内联问题条（`:1064`、`draw_inline_strips` `:1067`）、`ChoiceKeys`/`ChoiceAction`（`:51`/`:55`，插件给单选器加键）、`more` 展开（`:809` `unfold` 的另一半）、body 的 `scroll`（`PickerInput.scroll`）、被取代问题的状态交接 `carried`（`:464`）、`in_input`（输入框内作答，`:132`）、`PickerInput.picked` 的"重新填充后选中同一项"（`:124`）。 |
| 18 | `render.rs` | 1543 | `frame.rs`（帧布局）+ `batch.rs`（look 调度） | ~700 | 🟡 部分 | 只移植了"面板 → 区域"（`layout` `:237` 对应 `frame::lay_out_frame`）。缺整个**绘制/合成层**：`needs_redraw` 与 `Paced` 的 33ms 流式节流（`:84`/`:70`）、`render`（`:1038`）、`draw_body`（`:1234`）、`draw_completion`（`:1255`，8 行候选浮层）、`draw_choice`（`:1309`，picker/问题浮层）、`strip_colors`（`:1488`）、`layout` 的 pane 级布局（`:628` `lay_out_transcript`）、`CallStates`（`:288`）、`ShownConversation`（`:352`）、`asides`（`:403`）、`decorate`（`:807`，选中行反色）、`light`/`lit`（`:848`/`:899`，高亮命中）、`selectable`（`:1181`）、`TuiFrame`（`:1196`，含 `screen_row`/`area`/`covered`）、`TranscriptLayout::lay_out`（`:442`，无终端的导出）与光标/补全/问题三个 overlay。 |
| 19 | `renderers.rs` | 1068 | `batch.rs` | 431 | 🟡 部分 | **只填 `RenderKey::Tool`**（`batch.rs:104`–`:170`）；`RenderInput::{Entry, Content, Aside}`、`RenderKey::{Streaming, Aside, Content, Entry}` 一律没填 → note/插件条目/图片/推理/流式调用都不进转写。`ToolRender::new` 之后 `theme`/`facts`/`took`/`output`/`children`/`item` 六个字段**永远是默认值**（`renderers.rs:118`–`:130`）→ `header()` 的调用状态色带、`exit()` 的退出码、`progress_lines()` 的进度、`children` 的子 agent 调用树全部拿不到；`RenderBatch.pane`/`.agent` 从不设置；`EntryRender`/`ContentRender`/`AsideRender`/`ChildCall`/`RenderRevision`/`text_rows`/`OUTPUT_INDENT` 无对应物。 |
| 20 | `selection.rs` | 54 | — | — | ❌ 未开始 | 没有 `Selection` 与 `mark`（`:29`，"按 cell 宽度标 REVERSED 并拼出文本"）；鼠标拖选完全没有。 |
| 21 | `status.rs` | 605 | `status.rs` | 88 | 🟡 部分 | 聪明地**直接调** `rig_tui::status::line(items, width)`（`status.rs:565`）复用裁剪规则，所以 keep/`min_width`/"先缩后丢"是原样行为；缺 `StatusLine` 组件（`:382`，可给任意面板画一条）、输入框底行绘制（`editor.rs:684`/`:690`：编辑器带 `StatusLine` 时自己画在底行）、`no agent` 占位（`:406`）、turn 级 items（`:404`）、`StatusSettings.hide` + `[status]` 设置（`:272`）、`StatusParts`/`AppStatusPartsExt` 与 `/plugins` 的联动（`:329`/`:347`）、`StatusSystems::{Show,Edit}` 两个阶段（`:216`，我们全在 `Draw`）。 |
| 22 | `theme.rs` | 337 | `theme.rs` + `style.rs` | 52+53 | 🟡 部分 | 换成了一套 10 个 `Style` 的"前景色 token"，上游是 **17 个语义 band + plain/background/truecolor**（`:80`–`:125`）；缺 `Theme::on`（`:138`）/`shade`（`:175`）/`without_color`（`:191`）/`selection`（`:200`）/`changed_word`（`:210`）/`panel`（`:221`）/`raised`（`:227`）/`of_terminal`（`:234`）→ 用户/工具/调用状态/选中/代码/note/重要提示/diff 增删的底色全没有，`NO_COLOR` 无支持。`style.rs`（我们的，53 行）是 ratatui `Color` → bevy `Color` 的桥，上游没有对应物，可复用。 |
| 23 | `title.rs` | 216 | `title.rs` | 132 | 🔄 有意替换 | OSC 133 语义提示符标记 + OSC 2 标题 + 标题栈 push/pop（`:65`/`:68`/`:203`）只对终端有意义；状态机（`needs you`/`working`/`error`/`idle`，`:187`–`:195`）**照抄正确**（我们 `title.rs:84`–`:92`）。缺 `[title] mark_nonce_env` 设置（`:73`）与退出时恢复原标题（`:209`）——窗口前端建议保留"退出恢复窗口标题"这一条。 |
| 24 | `transcript.rs` | 1783 | `batch.rs::collect_rows` + `transcript.rs`（适配层） | ~430+199 | ❌ 未开始 | **这是最大的洞**。"对话 → 行"的引擎整块没移植：`Part`（`:942`）、`Transcript`（`:972`）、`Wrapped`/`Cached` 缓存（`:896`/`:848`）、`Scroll`/`Below`/`Inside`（`:1335`/`:1364`/`:569`）、`visible`（`:1111`）/`laid_out`（`:1168`）、`message_lines`（`:1487`）、`content_lines`（`:1687`）、`delivered_lines`（`:1708`）、`notice_lines`（`:1729`）、`stop_lines`（`:1669`）、`wrap_marked`（`:1296`）。我们只把 `LookLine` 画成节点（`transcript.rs:159`），行内容由 `batch.rs` 手拼：**提示词（本地已敲的）+ 工具调用 + 一句写死的假流式回复（`batch.rs:89`）+ 未回答的 `Ask`**。公开项 `PastEffects`（`:434`）/`Replaced`（`:723`）也没有。 |
| 25 | `view.rs` | 865 | — | — | ❌ 未开始 | `TranscriptView`（`:143`，选中/高亮/标记/top/width）、`TranscriptItem`/`CallItem`（`:84`/`:112`）、`ShowAgent`（`:49`，"看另一个 agent"）、`ScrollTo`/`ItemRow`（`:259`/`:271`）、`Highlight`/`LineSpan`（`:224`/`:241`）、`TextMark`/`MarkIn`（`:207`/`:174`，回复正文里插标签）、`DrawnTranscript`/`DrawnRow`/`DrawnLine`/`RowWrap`（`:316`/`:326`/`:341`/`:361`，给插件读"现在画出了什么"）、`Notices`（`:165`，会话通知）、`load_draft`/`keep_draft`（`:723`/`:701`，未发送草稿跨重启保留）全部没有。 |
| 26 | `tests.rs` | 433 | — | — | ➖ 不适用 | 上游的 TUI 集成测试（`TuiTestScreen` + 模拟按键），不是移植目标；我们的等价可测性反而更好（行是值，见 §4.6）。 |

---

## 2. 用户能感知的缺口 —— 前 5 名

### #1 转写里没有"对话"（只有工具卡片 + 一句假回复）

**用户会看到什么不同**
- assistant 的**正文回复不出现**。现在的回复是 `batch.rs:89` 里写死的一句中文，由 `drive_reply_stream`（`batch.rs:85`）每两帧吐一个字模拟流式；接真模型后这句仍然是假的。
- 没有 reasoning 段落（上游 `renderers.rs:633` `reasoning_lines`，dim + italic）。
- 自己发出去的消息只来自本进程的 `SubmittedPrompts`（`batch.rs:189`），**重放会话/切 agent 后历史消息全空**。
- 插件写进对话的 note/条目（`EntryRender`）、图片/音频/文档内容块（`ContentRender`）、aside 全都不显示。
- 会话通知（notice）、"本 turn 被停止/失败"的标记、子 agent 的调用树（`ToolRender::children`）都没有。
- 挂在调用下的具名装饰（`RenderOutput::decorate`）不显示。
- 滚上去时的标记只有一种：`↓ more below (PgDn)`（`pane.rs:117`），少了上游的 `↓ new output (PgDn)`（`pane.rs:376`）。

**涉及的上游公开 API**
`pane::{TranscriptPane, TranscriptSource}`、`view::{TranscriptItem, CallItem, TranscriptView, ShowAgent, DrawnTranscript, ScrollTo, ItemRow, Highlight, TextMark, MarkIn, LineSpan, RowWrap}`、`transcript::{PastEffects, Replaced}`、`render::TranscriptLayout`、`renderers::{EntryRender, ContentRender, AsideRender, ContentKind, ChildCall, RenderKey::*, RenderInput::*, RenderBatch::pane/.agent}`。
内部但必须照抄的：`transcript::{Part, Transcript::visible, Transcript::laid_out, message_lines, content_lines, delivered_lines, notice_lines, stop_lines, wrap_marked, Scroll, Below}`、`render::{lay_out_transcript, CallStates, ShownConversation, asides, decorate, light}`。

**工作量：L**（≈ 2500–3000 行；其中 `message_lines`/`content_lines`/`delivered_lines` 约 500 行，缓存与窗口化约 400 行，渲染系统桥接约 300 行，其余是四种 `RenderKey` 的填充与身份/选择。）

**依赖**
- 上游 `renderers.rs` 必须先能"按对话填满四种 key"——现在 `batch.rs` 只认 `Tool`。这是本项的第一块砖，也是 §6 的建议起点。
- `markdown`（#5）不是硬依赖：可以先按纯文本落行，再换 `markdown::render`（它输出的就是 `LookLine`，接口天然对齐）。
- `TranscriptView`/item 身份是选择、高亮、`ScrollTo`、`DrawnTranscript` 的公共前置，要和本项一起立。

---

### #2 输入补全完全没有

**用户会看到什么不同**
- 打 `/` 不弹命令列表；打 `@` 不弹项目文件；插件提供的 `#` 标记候选、`/model` 之类命令参数的候选（`CompletionSource::arg`）都不出现。
- Tab 不补全；候选列表的 ↑/↓ 选择、Esc 关闭、`InputContext::typed()` 的"我打的是哪类 token"都没有。
- 上游候选列表是 8 行浮层（`render.rs:65` `COMPLETION_ROWS = 8`），会按"越靠前越优先"排序、忽略大小写、包含即命中（`match_at` `complete.rs:676`）。

**涉及的上游公开 API**
`complete::{CompletionSource, CompletionTrigger, Candidate, Completion, CompletionKind}`、`InputContext::{typed, completion}`、`keys::InputAction::{Tab, Up, Down}`。
内部必须的：`complete::{FileIndex, walk, token, Sources, markers, path_items, match_at}`、`input::accept_completion`（`input.rs:1016`）、`render::draw_completion`（`render.rs:1255`）。

**工作量：L**

**依赖**
- **键位表（#3）先做**：Tab / Up / Down 的"先补全、再行、再历史"三级语义在 `InputAction` 里（`keys.rs:520`–`:536`）。
- **`Editor` 的游标模型要换**：上游 `cursor` 是**字节偏移、落在 grapheme 边界**（`editor.rs:64`、`set_cursor` `:133`），`token()` 用 `char_indices` 找 token 起止；我们是"字符下标"（`editor.rs:22`、`byte_at` `:44`）。不改这一层，补全的起止位置会算错。这一项被低估，实际是 #2 的前置。
- 浮层要新增一种面板：`PanelId` 是封闭枚举（`panels.rs:27`），得加 `Completion` 一类（或顺手做 §1 #15 说的"开放画布"）。

---

### #3 键位表（编辑器/转写/选择/内联回答的全部快捷键）

**用户会看到什么不同**
- 编辑类：Ctrl+A/E（行首行尾）、Ctrl+B/F、Alt+B/F 与 Ctrl+←/→（按词移动）、Ctrl+W / Ctrl+Backspace（删词）、Alt+D（前删词）、Ctrl+U/K（删到行首/行尾）、Home/End/Delete、**Shift+Enter / Ctrl+J / Alt+Enter 换行**（多行输入）全部无效。
- 转写类：Shift+↑/↓ 逐行滚、PageUp/PageDown 翻页（只有这两个有）、Alt+↑/↓ 选条目、Esc 从转写回到输入框（`KeyScope::Transcript` → `Deselect`）都没有。
- 会话类：Ctrl+C（清空输入且 ↑ 能找回草稿）、Ctrl+D（空输入退出）、**Esc 停止正在跑的 turn**、Tab（turn 运行时"排队发送"）、Enter（turn 运行时"转向"）都没有——现在 `Esc`/`Tab` 对运行中的 turn 无效。
- **插件生态**：插件无法声明 `KeyBinding`、无法读 `KeyPressed{entity, agent, released}`、无法用 `KeyScope::{App, Input, Panel(e), Transcript}` + `KeyFocus` 抢占焦点、无法用 `run_command` 绑定斜杠命令。这是"作为插件的前端"能不能被别人用的分水岭。

**涉及的上游公开 API**
`keys::{KeyPress, Keys, KeyBinding, KeyScope, KeyFocus, KeyPressed, InputAction, AppKeyBindingsExt, run_command}`、`input::{PendingInput, KeyInput, InputContext, InputSystems, TuiKeys}`、`pane::PaneKeys`、`pick::ChoiceKeys`。
内部必须的：`keys::{built_in, bind, find, arm_inline_keys}`、`input::{read_input, pressed, released, act, send, close, select, note_input_context, keyed_editor, keyed_pane}`。

**工作量：M**（`keys.rs` 是纯 ECS + 键码表，机械但量大；`input::act` 的语义要看 `Editor` 补齐到什么程度。若只做表 + 分派 + 15 个编辑动作，M；连内联问题/选择/排队一起做，L。）

**依赖：无。** 这是唯一一块不依赖任何其他缺口的模块（`crossterm::KeyCode`/`KeyModifiers` 是纯 enum，不带终端耦合，可以直接当 IR 用，见 §4.1）。

---

### #4 鼠标选字 + 剪贴板

**用户会看到什么不同**
- 拖选转写文字没有任何视觉反馈（没有反色），松手也不会进剪贴板；状态行不会出现 `Copied 2 lines`（`input.rs:662` 的 `COPIED` 条目）。
- Ctrl+V 什么也不发生：不贴文本、不贴图片、不会把剪贴板里的图片存进 session 的 `Scratch::images` 并插一个 `@path` 附件（`keys.rs:542`–`:547`）；把图片文件拖进窗口也不会变成 `@path`。
- 鼠标滚轮不滚转写（上游一格 3 行，`input.rs:635`）。
- 上游还有 Shift+拖拽"让终端做自己的选择"的兜底，窗口前端不需要，但"选中的文本要能出去"必须有替代品。

**涉及的上游公开 API**
`terminal::CopyToClipboard`、`selection`（仅 `pub(crate)`，但行为公开）、`status::StatusItem`（`COPIED` 用它）。
内部必须的：`input::{moused, show_copied, COPIED}`、`clipboard::{paste, read, Pasted, unquoted, unescape, words, image_file, local_url_path, percent_decoded}`、`render::cells_in`（`render.rs:1171`）、`view::TuiView.selection`。

**工作量：M**

**依赖**
- **字符级 hit-test**：我们每行只有整行 `RowText` + `Pickable`（`transcript.rs:139`），没有"第几列是哪个字符"。要么用 `bevy_text::TextLayoutInfo` 做字形级命中（`feathers_in_3d` 已经在用这个类型），要么先做**行级**选择（点行首/行尾 + Shift 扩选）作为中间态。
- `bevy_clipboard` 已在依赖里（`Cargo.toml`），但一次没用，接线成本为零。
- 图片粘贴需要 `rig_tools::clipboard`/`rig_tools::attach::Reference` 与 session 的 `Scratch`（rig-harness 服务，非终端）。

---

### #5 富文本：markdown + 代码高亮 + diff

**用户会看到什么不同**
- 回复里的 `#` 标题、列表、表格（按宽度自适应列宽）、引用块、`> [!NOTE]`/`TIP`/`WARNING`/`CAUTION`/`IMPORTANT` 五种带底色的提示块，现在全按原字符画。
- 代码围栏没有语法色；行内 `code` 没有底色（`Theme::code`）。
- 工具返回的 unified diff 不会红绿标行、不会在行首标新旧行号、不会把"改动的词"用更深的底色标出来（`diff.rs:247` `changed_words` + `:300` `overlay`）。
- `ToolRender::result_lines()`（`renderers.rs:234`，**公开**）本来会自动走 diff 分支——我们的 look 用 `result_text()` 自己拼行，所以这条免费路径被绕过了。

**涉及的上游公开 API**
`markdown::{render, render_marked}`、`highlight::{Code, Language, code_rows}`、`diff::{unified_lines, change_lines}`、`renderers::{LookLine, text_rows, OUTPUT_INDENT, ToolRender::result_lines, ToolRender::header_code}`、`Theme::{code, note, call_done, call_running, call_failed, important, added, removed, added_word, removed_word}`。

**工作量：L —— 但可以压到 S/M。** 关键判断：**这三个模块的输入输出都是纯值**（`&str` + `Theme` → `Vec<LookLine>`，而 `LookLine` 我们已经会画）。所以**不需要移植，直接调上游 `pub` 函数即可**——这正是我们在 `status.rs:44` 对 `rig_tui::status::line`、在 `picker.rs:596` 对 `rig_tui::wrap::wrap_all` 已经用过的做法。
  - markdown：`rig_tui::markdown::render(text, &theme, Some(width))`，S。
  - 代码高亮：包含在 `markdown::render` 里（`Grammars`），S。
  - diff：`rig_tui::diff::unified_lines(&text, &theme)` 直接替换现在的 `result_text()` 分支，S。
  - **真正的成本在 `Theme`**：这三个函数都要 `rig_tui::Theme`，而我们的 `crate::Theme` 是另一套字段（§1 #22）。要么先补一个"平行 Theme"（上游 17 个 band），要么只为了调它们构造 `rig_tui::Theme::default()`（颜色会是终端深色系，和我们的 3D 配色不一致）。→ 所以 #5 的真实依赖是 **#22 主题**。

**依赖**：#1（没有正文行就没有地方用）；#22（颜色）。

---

**紧随其后（第 6–9 名，都是一句话级别）**

6. **输入历史不跨会话**（`history.rs`）：Up 只在本次进程里翻；`tui/history.jsonl` 不存在；切 agent 不播种该 agent 的提示词；没有 `Sent`/`HistorySystems` 给插件做脱敏过滤。
7. **主题语义色缺失**（`theme.rs`）：没有 band，只有写死的 RGB；`NO_COLOR` 无效；`/plugins` 能隐藏的状态项也不存在。
8. **picker 的队列 / Ask / 内联条**（`pick.rs`）：多个 picker 只显示第一个；`Ask` 不进浮层（现在是塞进转写当可点行，`batch.rs:245`）；没有 `Alt+1..9` 的内联回答；没有 `ChoiceKeys`。
9. **多窗格 + 换 agent + 被移出的条目**（`pane.rs`/`view.rs`）：只能有一个转写；不能 `ShowAgent` 看子 agent；压缩移出对话的条目（`Replaced`）不保留；没有 notice。

---

## 3. 公开 API 对照（逐个名字）

计数口径：✅ 有同义对应物（同名同义）｜⚠️ 有对应但改名/缩水/只用了子集｜❌ 无对应物。
合计 **104 个名字：✅ 10 / ⚠️ 14 / ❌ 80**。

### 3.1 `complete`（5 个，❌ 5）

| 上游 | 我们的对应 |
|---|---|
| `Candidate` | ❌ |
| `Completion` | ❌ |
| `CompletionKind` | ❌ |
| `CompletionSource` | ❌ |
| `CompletionTrigger` | ❌ |

### 3.2 `editor`（3 个，⚠️ 1 / ❌ 2）

| 上游 | 我们的对应 |
|---|---|
| `Editor` | ⚠️ 同名，但我们是 `Resource`（`editor.rs:18`，字段 `text/cursor/history/history_pos` 全公开）；上游是 `Component`（`editor.rs:61`，字段全私有，20+ 个方法） |
| `MainEditor` | ❌ 没有"主编辑器"标记 → 没有 `Single<&mut Editor, With<MainEditor>>` 语义 |
| `MainInput` | ❌ 没有这个 `SystemParam` 别名（插件 `fn template(mut input: MainInput)` 的写法用不了） |

### 3.3 `history`（3 个，❌ 3）

`HistorySystems` ❌　`PromptHistory` ❌　`Sent` ❌

### 3.4 `input`（5 个，❌ 5）

`InputContext` ❌　`InputSystems` ❌　`KeyInput` ❌　`PendingInput` ❌　`TuiKeys` ❌

### 3.5 `keys`（9 个，❌ 9）

`AppKeyBindingsExt` ❌　`InputAction` ❌　`KeyBinding` ❌　`KeyFocus` ❌　`KeyPress` ❌　`KeyPressed` ❌　`KeyScope` ❌　`Keys` ❌　`run_command` ❌

### 3.6 `pane`（5 个，❌ 5）

`MainPane` ❌　`MainTranscript` ❌　`PaneKeys` ❌　`TranscriptPane` ❌　`TranscriptSource` ❌
（我们的近似物：`pane::PaneScroll`、`pane::TranscriptRows`、`pane::window_transcript`，都是全局单例，无实体身份。）

### 3.7 `panel`（7 个，⚠️ 4 / ❌ 3）

| 上游 | 我们的对应 |
|---|---|
| `PanelCanvas` | ❌ 没有"任意 widget 画进面板"的画布；只有 `PanelRows`（行值） |
| `Placement` | ⚠️ `frame::Placement`，7 个变体齐全，但 `Constraint` → 自造 `Extent::{Cells, Percent, Rest}`（`frame.rs:189`，缺 `Fill(n)`/`Min`/`Max`/`Ratio`) |
| `RedrawArea` | ❌ |
| `RequestRedraw` | ❌（我们每帧都画，没有"按需重绘"） |
| `TerminalSize` | ⚠️ `frame::FrameSize`（`frame.rs:125`），但来自 env/`parse`，不跟窗口 resize |
| `TuiPanel` | ⚠️ `frame::FramePanel`（`frame.rs:427`），多了 `order`/`inset`，少了默认屏幕"spawn 顺序"语义 |
| `TuiSystems` | ⚠️ `FeathersSystems`（`lib.rs:52`），`Fit` 与 `Draw` 顺序有意交换（`lib.rs:27`–`:29`） |

### 3.8 `pick`（5 个，⚠️ 1 / ❌ 4）

| 上游 | 我们的对应 |
|---|---|
| `ChoiceAction` | ❌ |
| `ChoiceKeys` | ❌ |
| `ChoiceSystems` | ❌ |
| `InlineStrip` | ❌ 没有内联问题条 |
| `PickerInput` | ⚠️ 同名 `Resource`（`picker.rs:45`）；字段：上游 `filter/selected/ticked/scroll/draft/more/touched/picked/open`（`pick.rs:103`），我们 `picker/panel/list_panel/preview_panel/filter/open/ticked/touched/rows/selected` → **缺 `scroll`/`draft`/`more`/`picked`**，多了三个面板实体 |

### 3.9 `render`（2 个，❌ 2）

`TranscriptLayout` ❌（"不画也能导出整段转写"的能力没有）　`TuiFrame` ❌（`screen_row`/`area`/`covered` 的帧查询没有对应物；我们只有 `frame::PanelAreas`）

### 3.10 `renderers`（19 个，✅ 6 / ⚠️ 4 / ❌ 9）

| 上游 | 我们的对应 |
|---|---|
| `AsideRender` | ❌ |
| `CallState` | ✅ 用全了 6 个变体（`batch.rs:138`–`:146`） |
| `ChildCall` | ❌ |
| `ContentKind` | ❌ |
| `ContentRender` | ❌ |
| `EntryRender` | ❌ |
| `LookLine` | ✅ 经 `Row::from_look` + `LookLine::colored()`（`transcript.rs:81`） |
| `OUTPUT_INDENT` | ❌ |
| `RenderBatch` | ⚠️ 只是当资源用：`width`/`requests` 会填，`pane`/`agent` 从不设置，`entries_mut`/`contents_mut`/`asides_mut`/`calls_mut` 未用 |
| `RenderContent` | ❌ |
| `RenderInput` | ⚠️ 只构造 `RenderInput::Tool` |
| `RenderKey` | ⚠️ 只构造 `RenderKey::Tool(0, n)`（`batch.rs:126`） |
| `RenderOutput` | ✅ `RenderOutput::default()` + looks 写 `lines` |
| `RenderRequest` | ✅ |
| `RenderRevision` | ❌ 从不推进 → 上游那种"变了才重排"的失效机制没有 |
| `RenderSystems` | ✅ 在 `TranscriptRender` 里链起 `Render→Decorate→Edit`（`batch.rs:406`） |
| `ToolRender` | ⚠️ 只 `new`/`argument`/`result_text`/`name`/`state`；`theme`/`facts`/`took`/`output`/`children`/`item` 从不填 |
| `TranscriptRender` | ✅ 我们自己 `world.run_schedule(TranscriptRender)`（`batch.rs:174`） |
| `text_rows` | ❌ |

### 3.11 `screen`（1 个，⚠️ 1）

`DefaultScreenPlugin` ⚠️ → `frame::spawn_default_screen`（`frame.rs:578`），缺 `InlineStrip` 实体。

### 3.12 `status`（8 个，✅ 2 / ❌ 6）

| 上游 | 我们的对应 |
|---|---|
| `AppStatus` | ✅ |
| `AppStatusPartsExt` | ❌（`/plugins` 的 show/hide 联动没有） |
| `Side` | ❌ 未直接使用（只在 `status::line` 内部消费） |
| `StatusItem` | ❌ 从不构造（业务拿不到 `says`/`styled`/`shrinks_to` 的便捷构造） |
| `StatusItems` | ✅（`status.rs:24` 查询 + `lib.rs:129` 注册） |
| `StatusLine` | ❌（没有"给任意面板画一条状态行"的组件） |
| `StatusSystems` | ❌（我们全塞进 `FeathersSystems::Draw`） |
| `Tone` | ❌ 未直接使用 |

### 3.13 `terminal`（6 个，❌ 6 —— 全部有意不移植，但两个要替代）

| 上游 | 我们的对应 |
|---|---|
| `CopyToClipboard` | ❌ → **需要 `bevy_clipboard` 版本** |
| `RunInTerminal` | ❌ → **需要"跑 `$EDITOR`"的窗口版**（挂起输入 + 恢复重绘 + `TerminalExit` 等价事件） |
| `TerminalExit` | ❌ |
| `TerminalFocus` | ❌ → `bevy_window::WindowFocused` 可替代 |
| `TerminalWrite` | ❌（OSC 序列无意义；但"给插件一个写出通道"的扩展点没了） |
| `TuiTestScreen` | ❌（无 headless 测试后端；我们用 `PanelRows` 值断言，见 §4.6） |

### 3.14 `theme`（1 个，⚠️ 1）

`Theme` ⚠️ 同名 `Resource`，但**字段是两个不相交的集合**：上游 17 个 `Color` band + `plain`/`background`/`truecolor`（`theme.rs:80`–`:125`）；我们 10 个 `Style` 前景 token（`theme.rs:10`–`:31`）。同名不同义，**不能直接传给 `markdown::render`/`diff::unified_lines`**。

### 3.15 `transcript`（2 个，❌ 2）

`PastEffects` ❌（从 effect log 重建历史调用的事实，续会话时用）　`Replaced` ❌（被移出对话的条目仍显示）

### 3.16 `view`（14 个，❌ 14）

`CallItem` ❌　`DrawnLine` ❌　`DrawnRow` ❌　`DrawnTranscript` ❌　`Highlight` ❌　`ItemRow` ❌　`LineSpan` ❌　`MarkIn` ❌　`RowWrap` ❌　`ScrollTo` ❌　`ShowAgent` ❌　`TextMark` ❌　`TranscriptItem` ❌　`TranscriptView` ❌

### 3.17 根部（6 个，⚠️ 2 / ❌ 4）

| 上游 | 我们的对应 |
|---|---|
| `FRONT` | ⚠️ `front::FRONT = "feathers"`（有意不同名） |
| `TuiPlugin` | ❌（`FeathersViewPlugin` + `FramePlugin`/`PanelsPlugin`/… 是等价装配，但没有"一键装全套"） |
| `TuiPlugins` | ❌（没有"可选 part 各自 `is_plugin_added` 去重"的装配器；上游 `TuiPlugins` 的 `is_unique() = false`，`lib.rs:135`–`:155`） |
| `TuiViewPlugin` | ⚠️ `FeathersViewPlugin`（build/finish 同形，`lib.rs:94`–`:131`） |
| `add_view` | ❌（没有"无窗口的测试装配入口"） |
| `BuildFailedLooks` | ❌（构建失败的红字提示 look，`lib.rs:398`/`:412`） |

### 3.18 重导出（3 个，✅ 2 / ❌ 1）

`crossterm` ❌（不用）　`ratatui` ✅（`ratatui::text::{Line, Span}`、`style::{Style, Color, Modifier}` 到处在用）　`wrap` ✅（`picker.rs:596`/`:764` 用 `rig_tui::wrap::wrap_all`）

### 3.19 模块级公开项（不在根部 `pub use` 清单里，但插件会用）

| 上游项 | 我们的对应 |
|---|---|
| `status::line(items, width) -> (Line, Line)`（`status.rs:565`） | ✅ 直接调用（`status.rs:44`）——**这是本仓库最省力的复用范式** |
| `status::StatusLinePlugin`（`:243`） | ❌ |
| `status::StatusSettings`（`:272`）/ `StatusPart`（`:282`）/ `StatusParts`（`:296`） | ❌ |
| `hints::KeyHintsPlugin`（`hints.rs:22`） | ⚠️ `hints::HintsPlugin`（`hints.rs:64`），但 `KeyHints` 我们误设成 `pub`（上游是 `pub(crate)`） |
| `title::TerminalTitlePlugin`（`title.rs:34`） | ⚠️ `title::TitlePlugin` |
| `markdown::render`（`markdown.rs:17`）/ `render_marked`（`:74`） | ❌ **可直接复用，无需移植** |
| `highlight::Language`（`:44`）/ `Code`（`:86`，`new`/`line`/`has_grammar`/`len`）/ `code_rows`（`:387`） | ❌ **可直接复用** |
| `diff::unified_lines`（`:38`）/ `change_lines`（`:51`） | ❌ **可直接复用** |
| `keys::find`（`:722`）/ `keys::Found`（`:708`） | ❌ |
| `keys::{KeyPress::new/key/ctrl/alt/of/typed}`、`Keys::presses`、`KeyBinding::{new,in_scope,with_releases,conflicts}` | ❌ |

---

## 4. 风险：终端特有的东西，移植时要换掉的接口

### 4.1 crossterm 的事件类型**是公开 API 的一部分** —— 但是纯 enum，建议保留当 IR

`keys.rs` 的公开形状是 `KeyPress { code: KeyCode, modifiers: KeyModifiers }`（`keys.rs:46`）、`KeyBinding.keys: Keys`、`KeyPressed`（`EntityEvent`）、`pick::ChoiceKeys(pub Vec<(KeyCode, ChoiceAction)>)`（`pick.rs:51`），`pane::bind_pane_keys` 也用 `KeyCode::{Up,Down,PageUp,PageDown}`（`pane.rs:164`–`:195`）。

- **好消息**：`crossterm::event::{KeyCode, KeyModifiers}` 是纯值类型，不依赖 tty、不需要裸模式。保留它当"键位 IR"，就只需在窗口侧写一个 `KeyboardInput → KeyEvent` 的贴片（还需处理 `KeyEventKind::Repeat`，上游 `keys.rs:725` 用它实现"长按只算一次"，见 `binding.releases`）。
- **坏消息**：如果要换成 bevy 的 `KeyCode`/`Key`，则 `KeyPress::of`（`keys.rs:79`）、`Display`（`:106`）、`FromStr`（`:158`）、`Keys` 的多键解析（`:236`）、`built_in()` 的 43 条表（`:570` 起）**全部要重写**，而且插件生态里大家写的是 `KeyCode::Char('k')`。→ **建议：保留 crossterm 的键码类型，只换事件的来源。**
- 已存在的对照：我们现在直接在 bevy 的 `KeyboardInput` 上做 `match &event.logical_key { Key::Character(..) | Key::ArrowUp | ... }`（`editor.rs:71`、`picker.rs:481`），两套键码已经并存——移植时应该统一到一套。

### 4.2 ratatui `Buffer` / `Frame` / `Widget` —— 只有"往格子写"的部分要换，`Line`/`Span`/`Style` 不用换

**必须换（写了格子或用了布局算法）**：
- `render.rs`：`render()`（`:1038`）整帧合成、`draw_body`（`:1234`）、`draw_completion`（`:1255`，`List`+`ListState`）、`draw_choice`（`:1309`）、`strip_colors`（`:1488`，直接改 `Buffer` 的 cell）、`cells_in`（`:1171`）、`draw_below`（`pane.rs:372`，`Clear.render`）。
- `panel.rs`：`PanelCanvas::{render, buffer_mut, reset, copy_to}`（`:164`–`:199`）、`lay_out` 里的 `Layout::vertical/horizontal`（`:256`–`:334`）。
- `pane.rs`：`draw_panes`（`:341`）、title 行的 `Block::inner`（`:331`）。
- `editor.rs`：`draw_editors`（`:686`，`Paragraph` + `Block` + `Padding` + 光标 `Position`）。
- `status.rs`：`draw_status_lines`（`:424`，`Line::render(left_area, buffer)`）。
- `pick.rs`：`draw_inline_strips`（`:1067`）。
- `selection.rs`：`Selection::mark`（`:29`，`buffer.set_style(.., Modifier::REVERSED)`）。
- `theme.rs`：`panel()`（`:221`）/`raised()`（`:227`）返回 `Block`。

**不用换（纯值，可继续当"行 IR"）**：`ratatui::text::{Line, Span}`、`ratatui::style::{Style, Color, Modifier}`、`ratatui::layout::Rect`。我们已经在用这条边界（`transcript.rs:159` `spawn_text_line` 把 `Span` 变成 `TextSpan`），这正是把 1783 行的 `transcript.rs` 搬过来的前提——**它产出 `Line`，我们负责画**。

**换的时候会掉的三个能力（真实风险）**：
1. **span 级背景色**。`Theme::selection()` 给整行加 `.bg(selected)`，`StatusItem::styled` 的彩色 gauge、diff 的"改动词更深一档"（`diff.rs:300` `overlay`）都是**行内某个 span 的底色**。我们现在只能整行一个底色（`Row.band` → `BackgroundColor`，`transcript.rs:132`）。要在 bevy_ui 里做 span 底色，得给每个 `TextSpan` 外面包一个带 `BackgroundColor` 的 `Node`，行高与基线会变——这是渲染层要提前拍板的决定。
2. **`Line::width()` 的 unicode-width vs 字体度量**。上游所有换行（`wrap_all`、`fit_keys`、`status::fit`、`pick::detail_column`）都按终端列宽算；bevy 里真实字形宽度不整除，`CELL = (7.8, 18.0)`（`frame.rs:35`）是硬编码的近似值。行数已经按"格子"算的地方（`pane_rows`、`area.cols`）会与实际显示有 1–2 列误差。
3. **`Block::inner()` 的几何**。上游靠 `Block` 的 padding 算 inner area（`pane.rs:331`、`theme.rs:258` `Padding::new(1,1,0,1)`）；我们靠 `PanelInset{pad,border}` + `PanelArea::new`（`frame.rs:160`/`:272`）手工扣。两套模型一旦不一致，面板会错位或截断末行。

### 4.3 调度与重绘策略不同 —— 没有 `frame_due`，成本会放大

上游：`TuiSystems` 在 **`PostUpdate`**，且整段 `Draw`/`Render` 挂在 `.run_if(render::frame_due)`（`lib.rs:220`–`:225`）；`needs_redraw`（`render.rs:84`）监听 ~20 种 `Changed`/`Added`，流式回复另有 `STREAM_FRAME = 33ms` 节流（`render.rs:70`）。

我们：`FeathersSystems` 在 **`Update`**，每帧全跑（`lib.rs:98`–`:123`），没有 `needs_redraw`、没有 `Paced`。

后果与风险：
- 长转写每帧全量重建 `Vec<Row>`（`batch::collect_rows` 每次都 new 一个 `Vec` 再整体替换 `TranscriptRows`，`batch.rs:185`/`:289`）。
- `panels::sync_panels` 只做**前缀 diff**（`panels.rs:110`–`:131`）：`common` 从头比 key，一滚动所有 key 都变 → 整段 despawn + respawn，一屏 100 行 × 每行 N 个 `TextSpan` 个节点。
- 每帧 `fill_row_text` 会 `despawn_children` 再重建（`transcript.rs:93`），文字行一旦变化就整行重建。
→ **建议在移植 #1 时同时引入上游的缓存模型**（`transcript.rs:848 Cached` / `:896 Wrapped` / `:931 Transcript` 的 `revision`+`width` 失效），否则 1783 行的引擎搬过来会卡。

### 4.4 主题依赖终端来问背景色 —— 窗口前端必须改成显式配置

`Theme::of_terminal()`（`theme.rs:234`）调 `terminal_colorsaurus::background_color()` 拿终端底色，再 `Theme::on(background, truecolor())` 混出所有 band；`truecolor()` 读 `COLORTERM`（`:267`），`no_color()` 读 `NO_COLOR`（`:262`），非 24 位时退到 `DARK_256`/`LIGHT_256` 两套索引色（`:60`/`:66`）。

窗口化后**没有"终端背景"这个概念**。必须选一条：
- 显式配置（env 或 bevy 资源）给一个 `Rgb` 背景，`Theme::on(bg, true)`；
- 或直接用 bevy 窗口 clear color 反推。
否则 `Theme::default()` 永远按 `DARK = 0x1c1c1c`（`theme.rs:27`）混色，3D 场景里偏暗且没法换主题。
`NO_COLOR` 的无障碍语义（`without_color`，`:191`）建议保留成"高对比/无色"模式。

### 4.5 尺寸来自终端 —— 窗口 resize 不会重排

`TerminalSize(pub Rect)`（`panel.rs:95`）由 `terminal.rs` 的 `terminal::size()` 与 crossterm 的 resize 事件填；我们换成 `FrameSize`（`frame.rs:125`）从 env/`parse` 读**固定格子数**，`FramePlugin` 在 `Startup` 插一次（`frame.rs:699`）。
→ 窗口拖大拖小、缩放、换显示器，布局都不变。接 `bevy_window::WindowResized` 重算 `FrameSize` 是必须补的一条（现在连这个 message 都没监听）。

### 4.6 "复制出去/交给外部程序/测试后端"这三件事没有终端替代品

| 上游 | 终端实现 | 窗口前端要什么 |
|---|---|---|
| `CopyToClipboard`（`terminal.rs`） | OSC 52 + base64 写进终端 | `bevy_clipboard`（已在 `Cargo.toml`，未用） |
| `RunInTerminal`（`:57`） | 退出备用屏 + 关裸模式，把 tty 交给子进程 | `std::process` + 挂起输入系统 + 恢复后强制重排；`TerminalExit` 的等价事件 |
| `TerminalWrite`（任意转义序列） | 帧后写 stdout | 无对应物；插件想要"桌面通知/改标题"需要各自的 bevy 通道 |
| `TerminalFocus` | crossterm 的 focus 事件 | `bevy_window::WindowFocused` |
| `TuiTestScreen`（`TestBackend`） | 内存里的 Buffer，可断言 | 我们的行是**值**（`PanelRows`），断言比 `Buffer` 更容易——**这是我们的优势，别丢**；建议加一个 headless `App` 测试工具 |
| `terminal.rs` 的 `take_screen`/`report_releases` | 键盘增强协议（`RELEASES`）让长按有 release 事件 | bevy 的 `KeyboardInput` 有 `state` 与重复，`binding.releases`（`keys.rs:325`）的语义要重定义 |

### 4.7 `Reflect` / settings / 组件注册的差距（容易被忽略）

- `TuiPanel`、`KeyBinding`、`KeyPressed`、`KeyFocus`、`StatusItem`、`StatusItems`、`ShowAgent` 都 `derive(Reflect)`（`panel.rs:103`、`keys.rs:311`/`:363`/`:378`、`status.rs:61`/`:146`、`view.rs:47`），因为 `/plugins`、`/reload`、场景保存都要 reflection；我们的 `FramePanel`/`PickerInput`/`Theme` 都没有 `Reflect`。
- `StatusSettings`/`TitleSettings` 走 `AppSettingsExt::add_setting("status"/"title")`（`status.rs:247`、`title.rs:38`），带 `deny_unknown_fields` 的 `settings.toml` 段；我们没有任何 settings 层。
- **turn 级 `StatusItems` 缺注册**：上游 `register_required_components::<Agent, StatusItems>()` **和** `::<TurnOf, StatusItems>()` 都做（`status.rs:226`–`:227`），我们只有 `Agent`（`lib.rs:129`）→ 拿不到"本 turn"的状态项（token 速率、进度等）。
- **`app.keeps_files("tui")`**（`lib.rs:180`）：会话级文件（history/notices/draft）注册缺失；`PluginData`/`Files` 一次都没用。

---

## 5. 附：与 `docs/status/PLAN.md` §1 的出入（供修订）

PLAN.md §1 的 8 行标注偏乐观（6 处 ✅ + 2 处 ⚠️），按本文档的证据应下调：

| PLAN.md 标注 | 实际 | 证据 |
|---|---|---|
| `transcript.rs` ✅ | ❌ 未开始（只有行→节点的适配层） | `batch.rs:104` 只填 `RenderKey::Tool`；回复是写死的 `batch.rs:89`；`transcript.rs`（199 行）没有 `Part`/`visible`/`message_lines` |
| `pane.rs` ✅ | 🟡 部分 | 只有 PgUp/PgDn（`pane.rs:186`），无 Up/Down、无 `Below::New`、无多窗格、无 `TranscriptSource::{Agent,Value}` |
| `hints.rs` ✅ | 🟡 部分 | `fit_keys` 三处与上游不一致（见 §1 #9） |
| `status.rs` ✅ | 🟡 部分 | 复用 `status::line` 很好，但缺 turn items、`no agent`、`[status] hide`、`StatusLine`、输入框底行、`StatusParts` |
| `pick.rs` ✅ | 🟡 部分 | `Ask` 不进浮层、无队列、无 `InlineStrip`、无 `ChoiceKeys`、无 `more`、无 body 滚动、无 `carried` |
| `editor.rs`/`input.rs`/`keys.rs` ⚠️ 基础 | 比"基础"更薄 | `Editor` 是 `Resource` 且 6 个键位；`keys.rs` 751 行 + `input.rs` 1026 行的机制为 0 |
| `clipboard.rs`/`selection.rs` ⚠️ 行级 | ❌ 未开始 | `grep -rn "clipboard"` 在 `src/` 无命中 |
| `markdown/highlight/diff` ❌ | ❌（但**可直接复用**，不必移植） | 三个都是 `pub fn → Vec<LookLine>`，见 §2 #5 |

---

## 6. 建议的第一个动手点

**先整块移植 `keys.rs`（`KeyPress`/`Keys`/`KeyBinding`/`KeyScope`/`KeyFocus`/`KeyPressed`/`InputAction`/`AppKeyBindingsExt`/`find`，含 43 条内置绑定表），并把 `input.rs::act`/`send` 的语义对着 `Editor` 补齐——它零终端耦合（crossterm 的键码是纯值）、体积可控、且是补全（Tab/Up/Down）、滚动、选择、内联回答、插件自定义键的共同前置；保留 `crossterm::KeyCode` 当键位 IR，只把事件来源换成 bevy 的 `KeyboardInput`。**

> 如果目标改成"先让 demo 一眼更像 rig"，则换一个起点：把 `batch.rs` 从"只填 `RenderKey::Tool`"扩成"按对话填满 `Tool/Entry/Content/Aside`"，assistant 正文直接调 `rig_tui::markdown::render` 拿 `LookLine`——这一步同时吃掉 #1 的骨架和 #5 的大半，且不必新写 markdown。（但它会立刻撞上 §4.3 的缓存问题和 §4.2 的 span 底色问题，所以我认为 `keys.rs` 是更稳的第一步。）

---

## 5. 本轮进展（2026-11-25 夜，四路并行后）

按上面的审计逐条推进，**四个缺口已填**（全部有测试，没有视觉工作）：

| 原缺口 | 现在 | 证据 |
|---|---|---|
| 缺口 1：转写里没有对话 | ✅ 已填 | `batch.rs` 1800 行 + `transcript.rs` 1052 行；`RenderKey` 五类（Tool/Entry/Content/Aside/Streaming）全走真实 `Conversation`/`Partial`/效应组件；假回复与"每两帧吐一个字"已删；23 个测试，含 2 个 headless 端到端 |
| 缺口 2 的前置：Editor 游标 | ✅ 已填 | `editor.rs` 1146 行：`cursor` 改字节偏移 + grapheme 边界（CJK/emoji/ZWJ/组合字符共 17 个测试） |
| 缺口 3：键位表 | ✅ 已填 | `keys.rs` 1295 行：`InputAction` **32 个**变体全有、`built_in()` **43 条**内置绑定 + 11 条额外内置、`KeyBinding`/`KeyScope`/`KeyFocus`/`KeyPressed` 插件机制；11 个测试 |
| 缺口 5：markdown / 高亮 / diff | ✅ 已填 | `rich.rs` 313 行：直接调上游 `markdown::render` / `highlight::code_rows` / `diff::unified_lines`，`upstream_theme()` 把我们的 Theme 映射过去；12 个测试 |

**顺带修正**：审计里写的"45 条绑定 / 19 个动作"是错的，上游源码实测 **43 条 / 32 个**（已在本文改正）。

### 仍然没做（下一步按这个顺序）

1. **补全 `complete.rs`**（L）：`/` 命令、`@` 文件、插件 `CompletionSource` 全无。前置（Editor 字节游标）已就绪，现在可以做了。
2. **选字 + 剪贴板 `selection.rs`/`clipboard.rs`**（M）：需要字符级 hit-test；`bevy_clipboard` 已在依赖里但零使用。
3. **`input.rs` 剩余语义**（M）：鼠标滚轮 3 行、拖选、`accept_completion`、`answer_inline`、`note_input_context`。
4. **`history.rs` / `jsonl.rs` 持久化**（S/M）：输入历史还只在内存。
5. **`view.rs` 顶层编排**（M）：`needs_redraw` / 33ms 节流、Front 协议。
6. ~~**渲染层读 span 样式**~~ ✅ 已填（2026-11-25 深夜）：`transcript::span_style_color` 把 `Modifier`/`bg` 折成可渲染颜色——BOLD 提亮 25%、DIM 往面板底色混 45%、UNDERLINED 提亮 12%、行内底色按「与底反着来」提/降 22%、ITALIC/CROSSED_OUT 无字形仍忽略；`spawn_row_themed`/`fill_row_text_themed` 让 `panels.rs` 读 `Res<Theme>`（没有主题的测试 App 退化成默认值）。5 个测试。**仍是退化实现**：行内底色与下划线没有真画出来，要真画得给每个 span 加一层 `BackgroundColor` Node（行高与基线会变）。

### 集成时的已知冲突（要 lead 收口）

- **notice 两条路**：demo 的 `NoticeLog`（应用层，`rig_harness_in_3d/src/lib.rs`）与引擎的 `TextId::Notice` 只能二选一，否则真 agent 的通知会画两遍。当前只走 `NoticeLog`（demo 没有真对话，不冲突）。
- **`↓ new output` 标记重复**：`pane::BELOW_MARKER` 与 `transcript::MORE_BELOW` 是两份；要画 new output 得让 `window_transcript` 改用 `Below`/`BelowSeen`。
- **主题两套**：引擎正文用 rig 默认主题，`rich::upstream_theme()` 是另一套映射，需要统一。

## 6. 第二轮并行（2026-11-25 深夜，四个通道）

| 缺口 | 结果 | 证据 |
|---|---|---|
| 缺口 2：补全 `complete.rs` | ✅ 已填 | `complete.rs` 1500 行：`CompletionSource`/`CompletionTrigger`/`Candidate`/`Completion`/`CompletionKind`、`SHOWN=50`、`match_at`/`token`/`path_items`、`/` 命令（过滤 `Disabled`）、`@` 路径（`IoTaskPool` 后台遍历 + `FileWatch` 防抖）、`CompletionState` + `follow_input` + `CompletionPlugin`；21 个测试。**简化**：`walk` 不读 `.gitignore`，只跳过 `.git`/`target`/`node_modules`/`.` 开头 |
| 缺口 4：选字 + 剪贴板 | ✅ 已填 | `selection.rs`（grapheme 级 hit-test、拖选/双击选词/三击选行/清除、`selected_text` 行尾裁剪、`ranges_in`/`byte_ranges_in`，25 个测试）+ `clipboard.rs`（`ClipboardText` trait 便于 mock、`copy_text`/`copy_selection`/`paste_text`/`paste_from_clipboard`、`Copied N lines` + `AppStatus` 条目，16 个测试）。新增依赖 `unicode-width 0.2`；`bevy_clipboard` 打开 `system_clipboard`+`wayland` |
| 补全的键位接线 | ✅ 已接 | `editor.rs::completion_act`：列表开着时 ↑↓ 改选中、Tab 接受、Enter（非精确）接受、Esc 关闭并记住 token；5 个测试。`CompletionState::show` / `Completion::from_items` 作为公开入口 |
| 渲染层 span 样式 | ✅ 已填 | 见 §5 第 6 条 |

测试总数：**175 全过**（本轮起点 103）。

### 仍然没做

1. **鼠标事件接线**（M）：`selection.rs` 的 hit-test 与 `clipboard` 都已就绪，但**没有系统**把 `feathers_in_3d` 的拾取事件接到 `Selection::press/drag/release`，也没把 `byte_ranges_in` 套到行样式上 → 拖选/双击/点击复制**还看不见效果**。这是下一轮第一件事。
2. **`input.rs` 剩余语义**（M）：滚轮 3 行、窗口 resize 清选区、`answer_inline`、`note_input_context`。
3. **`history.rs` / `jsonl.rs` 持久化**（S/M）：输入历史仍只在内存。
4. **`view.rs` 顶层编排**（M）：`needs_redraw` / 33ms 节流、Front 协议。
5. **补全面板的落点**（S）：`completion_rows` 有，但 `PanelId` 是封闭枚举，还没决定补全画在哪（输入面板尾部 / 新浮层）。

## 7. 第三轮并行（2026-11-25 凌晨，五个通道）

| 缺口 | 结果 | 证据 |
|---|---|---|
| 鼠标拾取 → 选区 → 复制 | ✅ 已接 | `selection.rs::SelectionPlugin`：`PointerPress/Move/Release` → `RowLayout` + `WorldPanel` 逆变换 → 面板局部像素 → 格子列；按下/拖动/双击选词/三击选行/点空白清除/松手复制；`panels.rs` 用 `byte_ranges_in` 切开 span 套 `REVERSED`（`span_style_color` 新增反色），hash 保证拖动重画。20 个测试 |
| 输入历史持久化 `history.rs`/`jsonl.rs` | ✅ 已填 | `history.rs` 733 行：`PromptHistory`（JSONL、去重、坏行跳过）、`HistorySystems::{Seed, Record, Save}` + `mirror`、`Sent`；缺任何可选资源都是 no-op。12 个测试。**没加新依赖**（`typed_prompts` 走 `rig_harness::rig_tools` re-export） |
| 应用层：通知单一路径 | ✅ 已收口 | 删掉应用层 `NoticeLog`，只留引擎 `TextId::Notice`；新增 `rig_harness_in_3d/src/notice.rs`。遗留：`batch::collect_rows` 的 `retain_text` 会清 Notice，应用层暂时回填（若把它改成保留 Notice，这段可删） |
| 应用层：设置 `Settings` | ✅ 已接 | `settings.rs`：`RIG_HOME/settings.toml`，`[view] hide_status` 真的影响视图；`/setting` 查看与修改并写回；坏 TOML/类型错降级 + `warn!` |
| 应用层：状态行真实化 | ✅ 已接 | `status.rs`：每帧从 `SessionPaths`/Agent/`ActiveTurn`/`Conversation`/`Running`/`LastUsage` 写 `AppStatus`，演示数字已删 |

**规模**：`rig_niri` 4943 → **15351 行 / 23 个模块**（上游 26 模块 14749 行）；
`rig_harness_in_3d` 116 → **2350 行**。
**测试**：`rig_niri` **207 个**（从 40 起）、`rig_harness_in_3d` **13 + 1 doctest**。

### 估算覆盖率

| 维度 | 之前 | 现在 |
|---|---|---|
| 上游模块有对应实现 | 14/26 | **25/26**（只剩 `tests.rs` 不适用） |
| 对位代码量 | ≈ 3558 行（25%） | **≈ 11000 行（约 75%）**（差额主要是 `view.rs` 顶层编排、`input.rs` 的鼠标/内联语义、`terminal.rs` 有意不做） |

### 仍然没做（下一轮）

1. **`view.rs` 顶层编排**（M）：`needs_redraw` / 33ms 节流、Front 协议、`--print` 等价物。
2. **`input.rs` 剩余语义**（M）：滚轮 3 行、resize 清选区、`answer_inline`、`note_input_context`。
3. **补全面板的落点**（S）：`completion_rows` 有，`PanelId` 封闭枚举需要加一块或并进输入面板。
4. **`terminal.rs` 有意不移植**（bevy 窗口取代），`title.rs` 的 OSC 133 同理。
5. **行内底色/下划线仍是退化实现**（要真画需 span 级 `BackgroundColor` Node）。
