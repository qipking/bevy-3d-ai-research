# 还没做的事（按用户能感知的价值排序）

> **状态**：当前（2026-11-25 第二轮之后；第 1 条已落地，其余是 20:10 快照）。配套
> [`PARITY.md`](PARITY.md)：那边是"每个模块到什么程度"，这里是"现在真正还剩什么、先做哪个"。
> **范围**：`rig_niri` 前端 + 为跑通它必须接的应用层线（`rig_harness_in_3d`）。
> 真实模型接入（provider/API key）不在这份表里——那是 app 侧的事，见
> [`HANDOFF.md`](HANDOFF.md) §7。
> **工作量口径**：**S** ≤ 半天｜**M** ≈ 1–3 天｜**L** ≥ 1 周（按一个熟悉这套代码的人算）。
> **快照警告**：`batch.rs`/`transcript.rs`/`rich.rs`/`panels.rs` 正在并行改动；行号以快照为准，
> 函数名不会漂。所有条目都能用 `grep`/`read` 复现（PARITY §6 有命令）。

---

## 1. 按 Enter 发不出消息 —— ✅ 已完成（2026-11-25 第二轮）

- **现在的链路**：`editor::keys` 把非空输入按 Enter 变成
  `Deliver::new(agent, text, DeliveryMode::Steer)`；本轮正在跑时 Tab 变成 `Queue`；
  空输入 Ctrl+D 写 `AppExit`；有 turn 在跑时 Esc 触发 `StopTurn`。目标 agent 取
  `ShownAgent`（应用层切工作区时更新它），所以发到的是**聚焦工作区**那个 agent。
  斜杠命令仍只走 `RunCommand`，不再额外发一条普通消息。
- **顺带修的根因**：`completion_act` 以前在补全列表**关着**时也吞掉 Tab/Enter/Esc/↑↓，
  导致发送与停止永远到不了 `act`；现在只在列表开着时接管。
- **测试**：`rig_niri` 14 个（`enter_delivers_the_trimmed_text_to_the_shown_agent_as_steer`、
  `tab_queues_while_a_turn_runs`、`escape_stops_the_running_turn`、
  `ctrl_d_exits_only_when_the_input_is_empty` 等）；app 侧 4 个，其中
  `a_typed_prompt_reaches_the_kernel_and_comes_back_as_recalled` 证明文本进了内核并被回写成
  `Recalled`，`enter_sends_to_the_focused_workspaces_agent` 证明两个工作区时发给了 1 号。
- **还没做（都不阻塞）**：本 crate 没有 `refused` 概念，所以没有上游
  `Deliver::literal()` 的"命令被拒后当普通消息重发"；排队没有 UI 反馈、不能撤回；
  斜杠命令经 Tab 仍是立即 Steer；`PaneScroll` 还是全局一份，不是每个工作区各一份。

---

## 2. 输入框只画一行：换行、光标所在行、长文本都看不见 —— ✅ 已完成（2026-11-25）

- **现在的行为**：`editor::layout`（`rig_niri/src/editor.rs:524`）按输入面板列宽折行，
  用的是转写同一个 `rig_tui::wrap::wrap`（`transcript::wrap_rows` 也用它）；
  `editor_rows`（`editor.rs:1140`）每行产一个 `KeyedRow`，第一行 `› `、续行两个空格。
  光标不再是字面量，而是插在**真实行列**上的反色 + 下划线 `▏`
  （`CURSOR`/`with_cursor`，`editor.rs:173`/`:199`），中文/emoji 按 unicode 显示列算。
  行数超过 `MOST_LINES=10` 时 `EditorLayout.top`（`editor.rs:123`）跟着光标滚动，
  光标行一定在 `top..top+visible` 里。
- **输入 tile 高度**：`sync_editor_tile`（`editor.rs:1123`）把 `FrameTree` 的 Input
  叶子改成 `input_tile_height`（`editor.rs:113`）=
  `max(3 行, clamp(行数, 1, 10) 行 + chrome)`，chrome 由 `PanelInset::DEFAULT` 算出
  （pad 8 + border 2，上下共 20px）：空输入 54px、4 行 92px、10 行 200px。Input 仍是
  **固定子**，多/少出来的高度由顶部 Spacer（比例子）吸收，右列总高精确不变；
  `layout::default_tree` 的初始 72px 保留，layout 既有测试不破。
- **测试**：`long_text_wraps_to_the_input_width`、`cursor_is_drawn_at_its_real_row_and_column`、
  `cursor_columns_count_double_width_characters`、`scrolling_keeps_the_cursor_row_visible`、
  `editor_rows_writes_every_visible_line_with_the_cursor_in_place`、
  `the_input_tile_grows_and_shrinks_with_the_text`、
  `growing_the_input_tile_keeps_the_right_column_exact` 等。
- **还差（不阻塞）**：输入框内的跨行选择/鼠标选字还没接（选区仍只服务转写/pane 行）；
  输入非空时 hints tile 仍然常显，没有上游的按状态换文案/让位（见 PARITY #9）。

---

## 3. 窗口缩放不重排

- **用户会看到什么**：拖大/拖小窗口，界面格子数不变——面板还是按启动时那一套
  `FrameSize`（默认 96×28 格）铺，缩放后只是整块 3D 面板变大变小/被裁，**不是**"多显示几行、
  转写变宽"。换显示器/改分辨率也一样。
- **上游 API / 我们的落点**：上游 `TerminalSize(pub Rect)`（`panel.rs:95`）每帧由终端尺寸填，
  crossterm 的 resize 事件驱动重排。我们的 `FrameSize` 是 `frame.rs:86`–`:107`，`FramePlugin`
  在 Startup `insert_resource(FrameSize::from_env())`（`frame.rs:837`）之后**再也不更新**；
  `WindowResized` 只有选区清理在读（`selection.rs:733`–`:734`），布局系统（`frame.rs:368`
  `lay_out_frame`）不监听。
- **工作量**：**M**（把窗口像素换算成格子 → 更新 `FrameSize` → 已有布局/窗口化自动跟着走；
  难点是 DPI/缩放的取整与"变化太大时是否重建行"）。
- **依赖**：无。工作区条带的间距也读 `FrameSize`（`workspaces.rs:562`），改一处即可。

---

## 4. Ctrl+V 粘不进文本（图片粘贴、拖入也没有） —— 🟡 部分完成（文本粘贴已完成）

- **文本粘贴（已完成）**：Ctrl+V 绑的还是 `InputAction::PasteImage`（`keys.rs:616`），
  但 `editor::act` 现在把它变成 `Act::Paste`（`editor.rs:778`、`:1004`），走
  `clipboard::paste_from`（`rig_niri/src/clipboard.rs:317`）：优先内存替身、其次
  `bevy_clipboard::Clipboard`，有文本就 `Editor::insert` 到光标处（多行文本里的
  `\n` 原样保留，`\r\n`/`\r` 归一）；空剪贴板、没有后端、读取失败写状态行的
  `PASTE_FAILED`（`clipboard.rs:113`，和 `Copied` 同一套状态反馈），成功粘贴会清掉
  上一次的红色错误。没有图片粘贴入口，原因写在代码里：`bevy_clipboard` 没开
  `image` feature，也没有剪贴板图片落盘到会话目录的约定。
- **测试**：`ctrl_v_pastes_text_at_the_cursor`、
  `ctrl_v_with_an_empty_clipboard_shows_a_visible_error`、
  `ctrl_v_without_any_clipboard_resource_still_reports`、
  `a_successful_paste_clears_the_previous_error`。
- **还差**：图片粘贴（`bevy_clipboard` 的 `image` feature + `Scratch` 目录 + 落盘 +
  `@path`）；剪贴板里只有图片时仍只会报 "The clipboard holds no text."。把图片文件
  拖进窗口变 `@path` 也没接。提示行仍写 "Ctrl+V image"（`hints.rs:33`，本轮按文件
  边界不改 hints），与实际（只粘文本）不符。
- **依赖**：图片部分依赖会话目录（`SessionPaths`，应用层已有）。

---

## 5. 确认/问题只能点，键盘答不了

- **用户会看到什么**：闸门弹出 `[1] 确认 [2] 取消`，鼠标能点，但 **Alt+1/Alt+2 没反应**，
  Esc 也不会拒绝；也没法在输入框里直接打字回答（上游"在输入框里作答"）。
- **上游 API / 我们的落点**：上游 `input::answer_inline`（`input.rs:950`）+ `pick::choice_key`
  （`pick.rs:723`），Alt+1..9/Alt+0 的绑定只在问题显示时启用（`keys::arm_inline_keys`）。
  我们的确认行由 `batch::confirm_rows`（`batch.rs:1174`）产出、带 `Row.choice` 和 `AskChoice`
  （`transcript.rs:71`/`:190`），点击由应用层 `answer_ask` 观察者处理
  （`rig_harness_in_3d/src/kernel.rs:267`）；`AnswerInline`/`DismissInline` 在 `act` 里空实现
  （`editor.rs:476`–`:479`），Alt+1..9 绑定被 `Disabled` 挂起（`keys.rs:641`–`:655`）。
- **工作量**：**S**（键盘回答：把 `AnswerInline` 接到 `Chosen::item` 并摘 `Disabled`；
  输入框内作答是 M）。
- **依赖**：无（点击路径已在，照着接）。

---

## 6. 看不了别的会话：子 agent、已保存会话、被压缩掉的条目、重启前的调用事实

- **用户会看到什么**：转写永远只显示"当前 agent 的当前对话"：不能切到子 agent 看它干了什么，
  不能把已保存的会话读进来对照，压缩（compaction）移出对话的条目直接消失，重启后旧调用卡片
  没有时长/调用树，"N messages above were compacted" 也没有。拆窗（Alt+S）只是把同一份转写
  复制成两列。
- **上游 API / 我们的落点**：上游 `pane::{TranscriptPane, TranscriptSource::{Shown,Agent,Value}}`
  （`pane.rs:57`/`:84`）、`view::ShowAgent`（`view.rs:49`）、`transcript::Replaced`
  （`transcript.rs:723`）、`transcript::PastEffects`（`:434`）、`render::compacted_line`
  （`render.rs:797`）。我们：`pane.rs` 只有全局 `PanelId::Transcript`（`Pane2` 复制同一份行）；
  `Replaced` 0 命中；`PastEffects` 只有 `batch.rs:77` 的 TODO；压缩摘要被测试明确断言不画
  （`batch.rs` 的 `tests` 模块里，压缩摘要那条断言）。
- **工作量**：**L**（要先把转写从"全局一份"改成"窗格带来源"，再接 `Replaced`/`PastEffects`
  两条数据源；`TranscriptSource::Value` 还要读会话存档）。
- **依赖**：§7 的条目选择共用 `TranscriptView` 身份；`PastEffects` 要看 rig-harness 的效应日志
  是否可读（只读服务）。

---

## 7. Alt+↑↓ 选不了条目，Esc 也回不到输入框

- **用户会看到什么**：想用键盘在转写上"选上一条/下一条"（配合复制、跳转）没有任何反应；
  焦点也进不了转写区，自然没有"Esc 把键交回输入框"。
- **上游 API / 我们的落点**：上游 `InputAction::{SelectUp,SelectDown,Deselect}` → `input::select`
  （`input.rs:922`）改 `TranscriptView::selected`（`view.rs:143`），`PaneKeys` 绑
  Up/Down/Alt+Up/Alt+Down（`pane.rs:164`–`:195`），`KeyScope::Transcript` 在选中时生效。
  我们：`act` 里这三个动作空实现（`editor.rs:476`–`:478`）；`pane_keys` 只读物理
  `PageUp/PageDown`（`pane.rs:219`）；`KeyScope::Transcript` 有变体但没有系统切进去
  （`keys.rs:338` 注释自认）。
- **工作量**：**M**（条目身份/选中态 + 反色绘制已有 `selection::selected_row` 可复用）。
- **依赖**：§6 的 `TranscriptItem`/`CallItem` 身份（同一块地基）。

---

## 8. 三个"状态标记"不画：↓ new output、[stopped]、压缩提示

- **用户会看到什么**：滚上去之后有新输出，底下仍然是 "↓ more below"（分不出"下面有旧行"还是
  "刚来了新行"）；turn 被停止后转写里没有 `[stopped]` 标记；对话被压缩过也没有任何提示，
  用户会以为消息丢了。
- **上游 API / 我们的落点**：上游 `transcript::stop_lines`（`transcript.rs:1669`）、
  `pane::draw_below` 的 `Below::New`（`pane.rs:376`）、`render::compacted_line`（`render.rs:797`）。
  我们：`window_transcript` 只放 `BELOW_MARKER`（`pane.rs:165`/`:184`），`BelowSeen`
  （`transcript.rs:815`）实现了但**没人持有**（`transcript.rs:811`–`:813` 自注释）；
  `[stopped]` 是 `batch.rs:862` 的 TODO(stops)；压缩摘要不画（见 §6）。
- **工作量**：**S**（`↓ new output` 已写好一半；`[stopped]` 读 turn 记录即可；压缩提示随 §6）。
- **依赖**：`↓ new output` 自身没有依赖；`[stopped]` 要读 turn 的 `TurnEnd`（rig-ecs 已有）。

---

## 9. 多个 picker/问题只显示第一个；插件不能给选项加键

- **用户会看到什么**：两个插件同时打开选择器时，只看到其中一个（顺序还不稳定），另一个既不
  显示也不会被排队；单选器/问题不能像上游那样用 Alt+1..9/插件自定义键选，只能用鼠标点；
  选项的 `more`（展开选中项的说明）、列表体滚动、在输入框里作答这些细节也没有。
- **上游 API / 我们的落点**：上游 `pick::queue_choices`（`pick.rs:500`，一次一个、关掉交接）、
  `ChoiceKeys`/`ChoiceAction`（`:51`/`:55`）、`PickerInput` 的 `scroll`/`more`/`draft`/`picked`/
  `in_input`（`:103`–`:132`）。我们：`sync_picker` 取 `pickers.iter().next()`
  （`rig_niri/src/picker.rs:376`）；`PickerInput` 字段见 `picker.rs:42`（缺 scroll/more/draft/
  picked）；`ChoiceKeys` 0 命中。
- **工作量**：**M**。
- **依赖**：`in_input`（输入框内作答）与 §5 的输入框作答是同一件事。

---

## 10. 插件扩展点：画自己的面板、自己的状态行、绑命令、读"画了什么"

- **用户会看到什么**：这不是"用户直接看到"，但决定这个前端能不能被别人当 rig 的前端用：
  插件**不能**新增一块自己的面板（除非改 `PanelId` 枚举）、不能在任意面板上画状态行、
  不能把键位绑到斜杠命令、不能读"第 n 行现在在屏幕哪里"（截图/测试/远程同步类插件都要）。
- **上游 API / 我们的落点**：上游 `panel::{TuiPanel, PanelCanvas, RequestRedraw, RedrawArea}`
  （`panel.rs:106`/`:160`/`:82`/`:91`）、`status::{StatusLine, StatusParts, AppStatusPartsExt}`
  （`status.rs:382`/`:296`/`:329`）、`render::TuiFrame`（`render.rs:1196`）、
  `render::TranscriptLayout`（`:437`）、`view::DrawnTranscript`（`view.rs:316`）、
  `keys::run_command`（`keys.rs:455`）、`add_view`（`lib.rs:260`）。我们：封闭 `PanelId`
  （`panels.rs:33`）与 `PanelRows`；状态行只有固定 `PanelId::Status`（`status.rs:21`–`:44`）；
  `TuiFrame`/`TranscriptLayout`/`run_command`/`add_view` 全 0 命中；`BatchPlugin` 还要应用层
  手动加（`lib.rs:154` 的插件元组里没有）。
- **工作量**：**L**（开放画布是架构改动：`PanelId` → 实体标识 + 每工作区根表；`TuiFrame`
  依赖工作区的世界位姿反算）。
- **依赖**：`TuiFrame`/`DrawnTranscript` 依赖 §6/§7 的条目身份；其余独立。

---

## 已知小差异（不单列，不值得单独排期）

| 差异 | 上游 | 我们 | 影响 |
|---|---|---|---|
| 键位提示裁剪 | `fit_keys` 带首尾空格、按 unicode 宽度、同权重丢最后（`hints.rs:77`–`:100`） | `hints.rs:50`–`:64`：裸 join、`chars().count()`、丢第一个 | 中文/emoji 提示行可能多留/少留一个；窄面板先丢的不同 |
| 提示行何时显示 | 只在输入为空时（`editor::draw_editors`） | 恒显示（`hints.rs:118`） | 输入时底部仍占一行 |
| 工具卡片事实 | `ToolRender.facts` 给退出码/输出截断/`FilesChanged`（`renderers.rs:118`–`:130`、`result_lines` `:234`） | `facts: None`（`batch.rs` 的 TODO），只显示输出文本 | 命令失败时看不到 `exit status N`；长输出被截断时没有提示行 |
| 主题注入 | `Theme` 是资源，绘制处处读它 | `batch` 用 `RTheme::default()`（`batch.rs:446`/`:640`/`:1120`…），我们的 `Theme` 只进 `rich.rs` | 改 `theme.rs` 不会改工具卡片/entry 的默认底色；NO_COLOR 无通路 |
| turn 级状态项 | `register_required_components::<TurnOf, StatusItems>()`（`status.rs:226`–`:227`） | 只注册 `Agent`（`lib.rs:179`） | 插件放在 turn 上的状态项（token 速率等）不显示 |
| Ctrl+D 退出 | 空输入触发 `Exit(Quit)`（`input.rs:795`） | `act` 空实现（`editor.rs:453`） | 键盘关不掉窗口；窗口标题栏还能关 |
| 空闲 Esc | 停插件想停的东西（rebuild 等，`input.rs:854` 一带） | 空实现 | 与 §1 的停止一起做 |

---

## 终端专有 / 有意不做（不是缺口）

| 项 | 为什么不做 | 备注 |
|---|---|---|
| 裸模式、备用屏、光标/鼠标捕获、括号粘贴、焦点事件 | bevy 窗口没有这些概念 | 有意替换，见 PARITY §4.1 |
| OSC 52 复制、OSC 133 提示符标记、OSC 2 标题、标题栈 | 终端专用；复制已走系统剪贴板，标题已走窗口标题 | 见 PARITY §4.1/§4.2 |
| `TerminalWrite`（任意转义序列，如桌面通知） | 无窗口等价物；插件要通知得走 bevy 消息 | 生态里暂无需求 |
| `RunInTerminal`（跑 `$EDITOR`） | 终端专用：要交还 tty。窗口版要"挂起输入 + 恢复重排 + 退出码事件"，收益低 | 如果以后要做，落点：`editor::act` 新增动作 + `TerminalExit` 等价消息 |
| `TuiTestScreen`（内存 Buffer 断言） | 行是值，headless `App` 断言更强 | 不移植是优势 |
| `[title] mark_nonce_env`、退出恢复原标题 | 只有终端 shell 集成需要 | 窗口关闭即结束 |
| 上游 `tests.rs`（433 行 TUI 集成测试） | 不是移植目标 | 我们 317 个内联测试覆盖同等行为 |

---

## 建议的动手顺序（依赖最少、用户最先看到）

1. **§1 输入通道**（M）——不做这个，前端对真 agent 等于只读。
2. **§2 多行输入** + **§4 文本粘贴**（M+S）——写长 prompt 的日常手感。
3. **§3 窗口缩放**（M）——桌面窗口的默认预期。
4. **§5 键盘回答**（S）——闸门场景立刻可用。
5. **§8 状态标记**（S）——小改动、明显可见。
6. 其余按 §6 → §7 → §9 → §10 排（§6/§7/§10 共用"条目身份/多来源窗格"这块地基，建议一起设计）。
