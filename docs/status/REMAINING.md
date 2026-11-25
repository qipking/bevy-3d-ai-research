# 还没做的事（按用户能感知的价值排序）

> **状态**：当前（2026-11-25 第五轮之后；快照 22:50，`rig_niri` **441** 个 lib 测试，`cargo test` 全绿）。
> 配套 [`PARITY.md`](PARITY.md)：那边是"每个模块到什么程度"，这里是"现在真正还剩什么、先做哪个"。
> **范围**：`rig_niri` 前端 + 为跑通它必须接的应用层线（`rig_harness_in_3d`）。
> 真实模型接入（provider/API key）不在这份表里——那是 app 侧的事，见
> [`HANDOFF.md`](HANDOFF.md) §7。
> **工作量口径**：**S** ≤ 半天｜**M** ≈ 1–3 天｜**L** ≥ 1 周（按一个熟悉这套代码的人算）。
> **编号不能重排**：别的文档按 `#1`–`#10` 引用这份表，已完成项保留位次。
> **快照警告**：`picker.rs`/`pane.rs`/`batch.rs`/`transcript.rs` 第五轮刚改完（选项键、`↓ new output`、`[stopped]`），
> 行号以快照为准，函数名不会漂。
> 所有条目都能用 `grep`/`read` 复现（PARITY §6 有命令）。

**现在的分布**：已完成 5 项（#1 #2 #3 #5 #7）；部分完成 3 项（#4 #8 #9）；
未开始 2 项（#6 #10）。

---

## 1. 按 Enter 发不出消息 —— ✅ 已完成（2026-11-25 第二轮）

- **现在的链路**：`editor::keys` 把非空输入按 Enter 变成
  `Deliver::new(agent, text, DeliveryMode::Steer)`（`editor.rs:1220`）；一轮正在跑时 Tab 变成
  `Queue`（`editor.rs:987`、`:1010`）；空输入 Ctrl+D 写 `AppExit`（`editor.rs:947`/`:1229`）；
  有 turn 在跑时 Esc 触发 `StopTurn`（`editor.rs:1225`）。目标 agent 取
  `ShownAgent`（`editor.rs:1178`，应用层切工作区时更新它），所以发到的是**聚焦工作区**那个 agent。
  斜杠命令仍只走 `RunCommand`，不再额外发一条普通消息（`editor.rs:1217`）。
- **顺带修的根因**：`completion_act` 以前在补全列表**关着**时也吞掉 Tab/Enter/Esc/↑↓，
  导致发送与停止永远到不了 `act`；现在只在列表开着时接管（`editor.rs:1037`）。
- **测试**：`rig_niri`：`enter_delivers_the_trimmed_text_to_the_shown_agent_as_steer`
  （`editor.rs:2410`）、`tab_queues_while_a_turn_runs`（`:2454`）、`escape_stops_the_running_turn`
  （`:2503`）、`ctrl_d_exits_only_when_the_input_is_empty`（`:2546`）；
  app 侧：`a_typed_prompt_reaches_the_kernel_and_comes_back_as_recalled`
  （`rig_harness_in_3d/src/lib.rs:953`）、`enter_sends_to_the_focused_workspaces_agent`
  （`workspaces_demo.rs:917`）。
- **还没做（都不阻塞）**：本 crate 没有 `refused` 概念，所以没有上游
  `Deliver::literal()` 的"命令被拒后当普通消息重发"（`editor.rs:1013`–`:1020`）；
  排队没有 UI 反馈、不能撤回（`grep -n Queue` 在 `batch.rs`/`pane.rs` 为 0）；斜杠命令经 Tab 仍是
  立即 Steer（`editor.rs:987` → 应用层 `RunCommand`，`rig_harness_in_3d/src/lib.rs:362`）；
  `PaneScroll` 还是全局一份，不是每个工作区各一份（`WorkspacePanels` 只有
  rows/roots/spawned/areas，`workspaces.rs:119`–`:135`）。

---

## 2. 输入框只画一行：换行、光标所在行、长文本都看不见 —— ✅ 已完成（2026-11-25）

- **现在的行为**：`editor::layout_within`（`rig_niri/src/editor.rs:626`）按输入面板列宽折行，
  用的是转写同一个 `rig_tui::wrap::wrap_typed`；`editor_rows`（`editor.rs:1386`）每行产一个
  `KeyedRow`，第一行 `› `、续行两个空格（`visible_rows`）。光标不再是把字符塞进文本，而是插在
  **真实行列**上的反色 + 下划线 `▏`（`CURSOR` `editor.rs:210`、`with_cursor` `:243`），
  中文/emoji 按 unicode 显示列算。行数超过 `MOST_LINES=10`（`:100`）时 `EditorLayout.top` 跟着
  光标滚动，光标行一定在 `top..top+visible` 里（`:616`/`:626`）。
- **输入 tile 高度**：`sync_editor_tile`（`editor.rs:1368`）把 `FrameTree` 的 Input 叶子改成
  `input_tile_height`（`editor.rs:125`）= `max(3 行, clamp(行数, 1, 10) 行 + chrome)`，
  chrome 由 `PanelInset::DEFAULT` 算出（`INPUT_CHROME` `:115`，pad 8 + border 2，上下共 20px）：
  空输入 54px、4 行 92px、10 行 200px。Input 仍是**固定子**，多/少出来的高度由顶部 Spacer
  （比例子）吸收，右列总高精确不变；`layout::default_tree` 的初始 72px 保留（`layout.rs:770`）。
- **测试**：`long_text_wraps_to_the_input_width`（`editor.rs:2602`）、
  `cursor_is_drawn_at_its_real_row_and_column`（`:2665`）、
  `scrolling_keeps_the_cursor_row_visible`（`:2926`）、
  `editor_rows_writes_every_visible_line_with_the_cursor_in_place`（`:3002`）、
  `the_input_tile_grows_and_shrinks_with_the_text`（`:3096`）。
- **还差（不阻塞）**：输入框的**键盘**选区（Shift+方向键）没有——`Editor` 只有光标，没有锚点；
  鼠标拖选已能落在输入面板并反色/复制（`panels.rs:203` 对所有面板生效，测试
  `clicking_the_input_boxes_blank_area_keeps_its_own_text_selection`，`pane.rs:1792`）。
  输入非空时 hints tile 仍然常显（§8 之外的独立小差异，见 PARITY #9）。
  "输入框里打字作答"是另一件事，见 §5。

---

## 3. 窗口缩放不重排 —— ✅ 已完成（2026-11-25 第五轮）

- **现在的行为**：`FrameSize`（`frame.rs:102`）每帧由 `sync_frame_size_to_window`（`frame.rs:504`）
  从主窗口的**逻辑像素**换算成格子数（`from_window_size` `:165`），值变了才写资源；拖大/拖小窗口，
  布局、窗口化、输入框折行都在下一帧按新尺寸走。`RF_FRAME=96x28` 仍可钉死（`FrameEnv` `:190`、
  `FrameSizePinned` `:244`，`FramePlugin` 在 `:1006`–`:1015` 装），截图/测试因此确定。
- **物理→逻辑**：`feathers_in_3d` 的三处镜像用节点的 `inverse_scale_factor` 把
  `ComputedNode` 的物理像素换回逻辑像素（`feathers_in_3d/src/world3d.rs:65`–`:88`，
  `physical_to_logical`/`physical_size_to_world`/`physical_len_to_world`；1× 缩放有逐字节回归测试
  `:385`），否则 2× DPI 下 3D 面板会大一倍、和 `Val::Px` 布局对不上。
- **测试**：`the_frame_size_follows_the_primary_window`（`frame.rs:1642`）、
  `frame_plugin_follows_the_primary_window`（`:1691`）、`resizing_the_window_relayouts_the_panels`
  （`:1715`）、`an_unchanged_window_does_not_rewrite_the_frame_size`（`:1769`）、
  `workspace_strip_spacing_uses_the_same_frame_size`（`:1852`）。
- **还差（都不阻塞）**：`WindowResized` 只被"换算成格子"用；极小窗口没有最小尺寸钳制
  （`a_tiny_window_lays_out_without_panicking` 只保证不 panic，`:1816`）；
  resize 会清掉文本选区（`selection.rs:787`，有意），但没有"resize 后恢复转写位置"的说法。

---

## 4. Ctrl+V 粘不进文本（图片粘贴、拖入也没有） —— 🟡 部分完成（文本粘贴已完成）

- **文本粘贴（已完成）**：Ctrl+V 绑的还是 `InputAction::PasteImage`（`keys.rs:621`），
  但 `editor::act` 现在把它变成 `Act::Paste`（`editor.rs:983`），`editor::keys` 调
  `clipboard::paste_from`（`editor.rs:1237`，实现 `clipboard.rs:317`）：优先内存替身、其次
  `bevy_clipboard::Clipboard`，有文本就 `Editor::insert` 到光标处（多行文本里的
  `\n` 原样保留，`\r\n`/`\r` 归一）；空剪贴板、没有后端、读取失败写状态行的
  `PASTE_FAILED`（`clipboard.rs:113`/`:116`，和 `Copied` 同一套状态反馈），成功粘贴会清掉
  上一次的红色错误（`clipboard.rs:121`）。提示行也已经改成 `Ctrl+V paste`（`hints.rs:33`/`:41`），
  和实际行为一致。
- **测试**：`ctrl_v_pastes_text_at_the_cursor`（`editor.rs:3215`）、
  `ctrl_v_with_an_empty_clipboard_shows_a_visible_error`（`:3237`）、
  `ctrl_v_without_any_clipboard_resource_still_reports`（`:3261`）、
  `a_successful_paste_clears_the_previous_error`；纯逻辑在 `clipboard.rs:493`/`:502`/`:509`。
- **还差**：图片粘贴（`bevy_clipboard` 的 `image` feature + `Scratch` 目录 + 落盘 +
  `@path`，上游 `clipboard.rs:227` 的 `dropped_images` 一带）；剪贴板里只有图片时仍只会报
  "The clipboard holds no text."（`clipboard.rs:59`/`:261`）。把图片文件拖进窗口变 `@path`
  也没接——`grep -rn dropped_images rig_niri/src` 为 0，我们只有 `type_path`（`clipboard.rs:290`）。
- **工作量**：**M**（图片通路）/ **S**（拖入找路径）。
- **依赖**：图片部分依赖会话目录（`SessionPaths`，应用层已有）。

---

## 5. 确认/问题只能点，键盘答不了 —— ✅ 已完成（键盘作答；输入框内作答仍未做）

- **现在的行为**：确认 tile 里有未回答的 `Ask` 时，`keys::arm_inline_keys`（`keys.rs:692`）
  摘掉 Alt+1..9/Alt+0 绑定的 `Disabled`（绑定生成 `keys.rs:657`–`:670`）：Alt+n 用当前确认 tile
  第一块（最老、按内核效应 id FIFO，和 `batch::confirm_rows` 同一顺序）的第 n 个选项回答
  （`editor.rs:908` → `answer_ask` `:860`）；越界什么都不做（`:917`–`:922`）。Alt+0 与 Esc
  拒绝问题（`refuse_ask` `:875`，`Outcome::refused`，和 rig-harness `pick.rs` 的
  `Pick::person(ask, None)` 是同一个结论）。Esc 只在有未回答问题、且该问题属于当前工作区时抢占
  （`editor.rs:1182`–`:1187`、`keys::ask_is_visible` `keys.rs:680`）；没有问题仍是原来的
  关补全 / 停 turn / 面板交还焦点。
- **点击与键盘共用一条路**：`editor::answer_ask`（`editor.rs:860`）是唯一的作答落点——
  应用层点击观察者 `kernel::answer_ask`（`rig_harness_in_3d/src/kernel.rs:291`）和键盘 Alt+n
  都调它；拒绝是 `editor::refuse_ask`（`:875`）。点击仍可点任何一块的任意选项。
- **应用层**：`answer_to_decision`（`kernel.rs:257`）现在也把"人拒绝问题"（Esc/Alt+0，没有
  `Answer`）转成被按住调用的拒绝；否则 hold 释放后，没决定的 `create_order` 会照常开跑。
- **测试**（`rig_niri`，`editor.rs`）：`alt_one_answers_the_first_choice_of_the_confirm_ask`
  （`:2136`）、`alt_two_answers_the_second_choice_of_the_confirm_ask`（`:2156`）、
  `alt_past_the_last_choice_answers_nothing`（`:2168`）、
  `alt_digits_answer_the_first_ask_in_the_confirm_tile`（`:2185`）、
  `alt_zero_refuses_the_ask_the_way_the_harness_does`（`:2207`）、
  `inline_keys_arm_only_while_an_ask_is_open`（`:2227`）、
  `alt_digit_without_an_ask_does_not_swallow_a_plugin_binding`（`:2256`）、
  `answering_the_ask_clears_the_confirm_panel_next_frame`（`:2282`）、
  `escape_refuses_the_open_ask_before_stopping_the_turn`（`:2308`）；
  应用层（`kernel.rs`）：`a_click_and_alt_one_make_the_same_rig_harness_answer`（`:450`）、
  `a_click_can_answer_any_choice_and_alt_one_the_first`（`:473`）、
  `refusing_the_ask_cancels_the_held_order`（`:490`）。
- **还没做**：输入框里直接打字作答（上游 `pick.rs:132` 的 `in_input`/`draft`，落到
  `Answer::text`）；多问题时键盘只答第一块（FIFO），不能像上游那样用插件自定义的 `ChoiceKeys`；
  `Alt+(选项数+1)` 把问题挪进输入框作答的分支也没有。
- **工作量**：输入框内作答是 **M**。
- **依赖**：无（输入框内作答复用已有的 `Editor`）。

---

## 6. 看不了别的会话：子 agent、已保存会话、被压缩掉的条目、重启前的调用事实

- **用户会看到什么**：转写永远只显示"当前 agent 的当前对话"：不能切到子 agent 看它干了什么，
  不能把已保存的会话读进来对照，压缩（compaction）移出对话的条目直接消失，重启后旧调用卡片
  没有时长/调用树，"N messages above were compacted" 也没有。拆窗（Alt+S）只是把同一份转写
  复制成两列（`panels.rs:143`）。
- **上游 API / 我们的落点**：上游 `pane::{TranscriptPane, TranscriptSource::{Shown,Agent,Value}}`
  （`pane.rs:57`/`:84`）、`view::ShowAgent`（`view.rs:49`）、`transcript::Replaced`
  （`transcript.rs:723`）、`transcript::PastEffects`（`:434`）、`render::compacted_line`
  （`render.rs:797`）。我们：`pane.rs` 只有全局 `PanelId::Transcript`（`Pane2` 复制同一份行，
  `panels.rs:143`）；`grep -rn TranscriptSource rig_niri/src` / `grep -rn ShowAgent` 均为 0，
  `grep -rn Replaced` 只命中 `batch.rs:2166` 的注释（没有实现）；`PastEffects` 只有 `batch.rs:89` 的 TODO；`compacted_line` 0 命中（`grep -rn 'above were compacted' rig_niri/src` 只命中
  `batch.rs:2166` 的注释和 `:2199`–`:2200` 断言它不出现的测试）。
- **工作量**：**L**（要先把转写从"全局一份"改成"窗格带来源"，再接 `Replaced`/`PastEffects`
  两条数据源；`TranscriptSource::Value` 还要读会话存档）。
- **依赖**：§7 的条目身份（已有 `Selection::item`，但没有公开 `TranscriptView`）；`PastEffects`
  要看 rig-harness 的效应日志是否可读（只读服务）。

---

## 7. Alt+↑↓ 选不了条目，Esc 也回不到输入框 —— ✅ 已完成（2026-11-25 第五轮）

- **现在的行为**：`pane::pane_keys`（`pane.rs:394`）里，Alt+↑/↓ 选"上一条/下一条"（
  `select_pane_item` `:333`，入口 `:429`–`:442`），块身份来自行 key 的高 32 位
  （`item_rows` `:109`）；条目第一行不在窗口里时把它顶到窗顶（`:359`–`:363`），两端夹住不越界；
  选中态写进 `Selection`（`Selection::select_item` `selection.rs:212`，每次窗口化后
  `sync_item_selection` `pane.rs:479` 把块序号换算回可见行）。Esc 清掉条目选区并把键交回输入框
  （`pane.rs:409`–`:413`、`release_transcript_focus` `:536`）。
- **和上游的差异（有意）**：上游在两端会取消选中，我们**夹住**（`:330`–`:331` 注释）；
  上游走 `PaneKeys` 绑定表，我们是物理键直读（`:385`–`:392` 自注）——
  所以 `InputAction::{SelectUp,SelectDown,Deselect}` 在 `act` 里仍是空实现（`editor.rs:973`–`:975`），
  插件不能重绑这几个键。
- **测试**（`pane.rs`）：`alt_down_selects_items_in_order_and_alt_up_walks_back_without_overshooting`
  （`:1306`）、`a_selected_item_puts_all_of_its_rows_in_the_selection_and_no_others`（`:1343`）、
  `selecting_an_item_scrolls_it_into_view`（`:1379`）、
  `alt_up_with_no_selection_starts_at_the_newest_item`（`:1406`）、
  `escape_clears_the_item_selection_and_gives_the_keys_back_to_the_input`（`:1415`）、
  `a_vanished_block_clears_the_item_selection`（`:1456`）、
  `a_keyboard_item_follows_the_rows_after_paging_and_scrolling_back`（`:1518`）、
  `an_item_that_scrolls_out_of_view_gives_the_keys_back`（`:1545`）、
  `live_calls_become_one_item_each_and_alt_down_steps_between_them`（`:1847`）。
- **还差（不阻塞）**：`TranscriptView`/`TranscriptItem`/`CallItem` 这套公开 API（上游
  `view.rs:143`/`:84`/`:112`）——插件读不到"当前选中哪条、有哪些条"；`ScrollTo`/`ItemRow`
  （上游 `:259`/`:271`）没有公开入口；`PaneKeys` 绑定（上游 `pane.rs:125`）没有。
- **工作量**：公开 API 是 **S**（数据已经在 `Selection::item` 里）。

---

## 8. 状态标记：`↓ new output` ✅、`[stopped]` ✅、压缩提示 ⛔（依赖 §6）

- **`↓ new output`（✅ 2026-11-25）**
  - **判定来源**：`PaneScroll::follow` + `transcript::BelowSeen`（现在是 bevy 资源，
    `PanePlugin::init_resource` 持有，`pane.rs` 每帧读）。`window_transcript`（`pane.rs:272`）
    调 `BelowSeen::of(follow, total, top, height)`：跟随末尾 → `Below::Nothing`（标记消失、
    `seen` 复位成当前整段）；刚滚上去 → `seen = 当时整段行数`，下面还有旧行 → `Below::More`；
    滚动之后整段又长了（`total > seen`）→ `Below::New`。
  - **落点**：`window_transcript` 的最后一行右端（`below_marker` `pane.rs:248`），文案取
    `transcript::{MORE_BELOW, NEW_OUTPUT}`（`:775`/`:777`，与上游 `pane::draw_below` 逐字一致）。
    宽度裁剪同上游：窗格列数小于标记显示宽度时整句不画、最后一行留空；窗格只有一行时不占标记行。
    标记行 hash 按内容算，`more below` 换成 `new output` 时增量 diff 会重画。
  - **测试**（`pane.rs`）：`following_the_tail_draws_no_below_marker`（`:899`）、
    `scrolling_up_shows_more_below_then_new_output_when_rows_arrive`（`:915`）、
    `scrolling_back_to_the_bottom_clears_the_marker_and_resets_below_seen`（`:951`）、
    `a_pane_narrower_than_the_below_marker_leaves_the_last_row_blank`（`:982`）。
- **`[stopped]`（✅ 2026-11-25）**
  - **判定来源**：对话 turn 记录的 `TurnRecord.end == Some(TurnEnd::Stopped { at: Some(id), .. })`
    （`batch.rs:783`）。这是持久来源，和上游 `transcript::stop_lines` 读同一种记录；live 的
    `TurnOutcome` 只在 turn 实体上（Commit 后实体和 `TurnOf` 关系就没了），不能事后判断。
    `at` 是停止那一刻该轮最后一条消息（用户 Esc 经 `Stopper::stop` 会写）；`at: None`
    或那条消息已被移走 → **不画**，不凭空造。
  - **落点**：`batch::conversation_frame` 产 `TextId::Message { at, block: STOPPED_BLOCK }`
    （`STOPPED_BLOCK = usize::MAX`，`batch.rs:196`；最大块序号让它在排序上排在该消息所有
    内容块之后、下一条消息之前）。行是 dim 的 `[stopped]`（`:976`–`:986`）。这里复用
    `TextId::Message` 而不新增变体：应用层非聚焦条带的 `retain_text` 只保留
    `TextId::Message { .. }`（`workspaces_demo.rs:422`–`:426`），新变体会在那条路径被丢掉。
    上游对停在 user 消息/工具结果上的轮次还会写 `[stopped before it was answered]`
    （`transcript.rs:1669`）；这里按任务要求统一成一条 `[stopped]`。
  - **测试**：`transcript::tests::a_stopped_row_follows_the_last_block_of_its_message`
    （`transcript.rs:990`）；`batch::tests::a_stopped_turn_draws_a_dim_stopped_line_after_its_last_message`
    （`batch.rs:2099`）、`an_answered_turn_has_no_stopped_line`（`:2129`）、
    `a_stop_record_without_a_last_message_draws_nothing`（`:2149`）。
- **压缩提示（⛔ 没做，原因已核实）**
  - **摘要长什么样**：`rig_memory::apply_summary` 写一条 user 条目，作者
    `Author::plugin("compaction")`，正文以 `"The conversation history before this point was
    compacted into the following summary:\n\n<summary>\n"` 开头、`"\n</summary>"` 结尾。
    这条摘要本身已经在画（带作者的文字 → `RenderKey::Entry`），缺的是它上面那行分隔。
  - **为什么画不了**：上游 `render::compacted_line(count)`（`render.rs:797`）要的 count 是
    “这条摘要代表的归档条目数”（`Entry.stands_for`）。它只在做压缩的一方填的 `Replaced`
    （§6）和会话日志里（`rig_memory::summarized_in_log` 重放）；live `Conversation` 只有
    摘要条目，`EntryRef`/`TurnRecord`/`SummaryState` 都不带条数，被替换的消息已经从对话里
    移走。`rig_niri` 没有 `Replaced`，也没有从 `Conversation` 读会话日志的入口
    （`SessionLog` 不暴露 replaced），所以**不画**——不写猜出来的 N。
  - **防误画**：`batch::tests::a_compaction_summary_gets_no_invented_message_count`
    （`batch.rs:2169`）构造真实摘要形状（`Author::plugin("compaction")` + 包裹文案），
    断言摘要仍照常画成 entry、且没有任何 `N ... compacted` 行。
  - **真正要做**：等 §6 的 `Replaced`（或应用层把 `summarized_in_log` 喂给前端）；文案照上游
    `render::compacted_line`：1 条 `── 1 message above was compacted ──`，其余
    `── N messages above were compacted ──`，dim，放在摘要 entry 上方。
- **工作量/依赖**：前两个已完成；压缩提示剩余 **S**，依赖 §6 的 `Replaced`/历史读取。

---

## 9. picker 队列与内置选项键已完成（✅）；插件仍不能给选项加键

- **picker 队列（✅ 2026-11-25）**：多个 picker 同时存在时，条带最右边仍然只有
  **一列**、一次显示一个，按创建顺序排队；关掉队首（Esc/回车提交/取消 → harness
  收到 `Pick` 后 despawn）或被外部 despawn 后，下一个在**同一帧**的 `sync_picker`
  里接进同一列（容器/列表/preview 面板复用，不闪空列）。队列是 `PickerQueue`
  资源（`rig_niri/src/picker.rs:104`）：`Add<Picker>` 观察者（`:553`）在组件加入
  world 的瞬间按 spawn/insert 顺序入队（是创建顺序，不依赖 ECS 查询迭代顺序），
  `sync_picker`（`:568`）每帧清掉已 despawn 的、兜底补漏；`push`（`:110`）对同一实体去重。
  最后一个关掉后 `remove_column` 摘列、树逐字节复原；列里有 preview 时先
  `remove_leaf(Preview)` 再摘整列（`sync_preview_panel` `:697`）。
- **`Ask` 有意不进这个队列**：3D 这边的 `Ask` 是条带里的确认 tile
  （`batch::confirm_rows` `batch.rs:1297`，`PanelId::Confirm`），不是 picker 列、也没有弹层。
  队列只装 `Picker`，和上游 `queue_choices`（`pick.rs:500`）的差异是故意的。
- **内置选项键已落地（✅ 2026-11-25 第五轮，`picker-keys`）**：picker 显示时 `picker_keys`
  （`picker.rs:826`）接管 Alt+1..9——选第 n 个**可见**项并直接确认（和点击一样交
  `Chosen::item`），Alt+0 取消（和 Esc 一样）；没有 picker 时这些键仍归输入框/插件绑定。
  同时补上：`more`（→ 展开选中项的完整说明，再按收起）、列表体 `scroll`（↑↓/PgUp/PgDn
  移动时窗口跟着走）、`picked`（过滤/重新填充后按条目身份选回同一项）——
  `PickerInput` 字段见 `picker.rs:56`–`:95`。测试 `alt_one_picks_the_first_item_like_a_click`、
  `alt_three_picks_the_third_item`、`alt_zero_cancels_the_picker`、
  `alt_past_the_last_item_answers_nothing`、`alt_digits_stay_with_the_input_when_no_picker_shows`、
  `more_expands_and_collapses_the_selected_items_description`、
  `scrolling_follows_the_selection_and_stops_at_the_ends`、
  `filtering_keeps_the_selected_label_when_it_still_matches`、
  `refilling_the_items_keeps_the_selected_label`（都在 `picker.rs` 的 `tests` 里，
  行号以文件为准——这块正在收尾）。
- **还没做**：插件自定义选项键的公开 API `ChoiceKeys`/`ChoiceAction`/`ChoiceSystems`
  （上游 `pick.rs:51`/`:55`/`:589`，本仓 0 命中）——现在只有内置 Alt+数字，插件加不了自己的键；
  树里的 `more` 也没键（树里 →/← 归折叠，要等 `ChoiceKeys` 或另配一个键）；
  在输入框里作答（`in_input`/`draft`，上游 `:132`，落到 `Answer::text`）。
- **测试（队列）**：`pickers_show_one_at_a_time_in_creation_order`（`:2356`）、
  `the_same_picker_is_never_queued_twice`（`:2436`）、`escape_hands_the_column_to_the_next_picker`
  （`:2473`）、`closing_a_picker_whose_preview_is_open_removes_the_whole_column`（`:2505`）、
  `an_ask_never_enters_the_picker_queue`（`:2553`）、
  `closing_after_a_focus_switch_clears_the_old_workspaces_column`（`:2578`）。
- **工作量**：剩余部分 **M**（`ChoiceKeys` 公开 API + 输入框作答；内置选项键/滚动/说明已完成）。
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
  `grep -rnE 'TuiFrame|TranscriptLayout|DrawnTranscript|run_command|add_view' rig_niri/src`
  只命中 `keys.rs:23` 的一行注释；`BatchPlugin` 还要应用层手动加（`lib.rs:156`–`:172` 的
  插件元组里没有，`rig_harness_in_3d/src/main.rs:68` 自己加）。
- **工作量**：**L**（开放画布是架构改动：`PanelId` → 实体标识 + 每工作区根表；`TuiFrame`
  依赖工作区的世界位姿反算）。
- **依赖**：`TuiFrame`/`DrawnTranscript` 依赖 §6/§7 的条目身份（§7 的身份已有，公开 API 还没）；
  其余独立。

---

## 已知小差异（不单列，不值得单独排期）

| 差异 | 上游 | 我们 | 影响 |
|---|---|---|---|
| 键位提示裁剪 | `fit_keys` 带首尾空格、按 unicode 宽度、同权重丢最后（`hints.rs:79`–`:100`） | `hints.rs:50`–`:74`：`" · "` 裸 join（无首尾空格）、`chars().count()`、丢第一个 | 中文/emoji 提示行可能多留/少留一个；窄面板先丢的不同 |
| 提示行何时显示 | 只在输入为空时（`editor::draw_editors`） | 恒显示（`hint_panel_rows` 每帧 `turn_running=false`，`hints.rs:147`） | 输入时底部仍占一行 |
| 工具卡片事实 | `ToolRender.facts` 给退出码/输出截断/`FilesChanged`（`renderers.rs:118`–`:130`、`result_lines` `:234`） | 已接：结果 entry 里的 `Entry::facts`（`batch.rs::conversation_frame`）和效应上的 `Published`（`record_calls`/`rebuild_batch`/`live_additions`）都进 `ToolRender.facts`，header/结果行据此画退出码与截断标记 | 已无此差异；剩余限制：`KeptFacts` 是 rig-ecs 的 `pub(crate)`，重启后旧调用的 `Published` 事实读不到（只有本进程 `CallHistory`），更早 turn 的 `took` 仍缺 `EffectRecorded` |
| 主题注入 | `Theme` 是资源，绘制处处读它 | `batch` 用 `RTheme::default()`（`batch.rs:530`/`:739`/`:1243`/`:1492`），我们的 `Theme` 只进 `rich.rs` | 改 `theme.rs` 不会改工具卡片/entry 的默认底色；NO_COLOR 无通路 |
| turn 级状态项 | `register_required_components::<TurnOf, StatusItems>()`（`status.rs:226`–`:227`） | 只注册 `Agent`（`lib.rs:181`） | 插件放在 turn 上的状态项（token 速率等）不显示 |
| 空闲 Esc | 停插件想停的东西（rebuild 等，`input.rs:854` 一带） | 没有 turn 在跑时什么也不做（`editor.rs:994`–`:995`，注释说明不空触发） | 插件没有"Esc 停我的后台工作"的入口 |

> 已从这张表移除：**Ctrl+D 退出**（已实现，`editor.rs:947`/`:1229`，测试 `:2546`）、
> **提示文案 Ctrl+V image**（现在是 `Ctrl+V paste`，`hints.rs:33`/`:41`）、
> **Alt+↑↓ 条目选择**（已实现，见 §7）。

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
| 上游 `tests.rs`（433 行 TUI 集成测试） | 不是移植目标 | 我们 **441** 个内联测试覆盖同等行为 |

---

## 建议的动手顺序（依赖最少、用户最先看到）

> 第五轮之后：§1/§2/§3/§5/§7 已完成，§8 的 `↓ new output`、§9 的队列与内置 Alt+1..9
> 也已落地。**剩下的真正候选只有下面五件**（按"看得见的收益 ÷ 成本"排）：

1. **§8 的压缩提示**（S，依赖 §6 的 `Replaced`）——现在只能等数据源；`↓ new output`
   和 `[stopped]` 已完成。
2. **§4 图片粘贴 + 拖入 `@path`**（M）——Ctrl+V 现在是完整的文本粘贴，差图片这一半；
   提示行已经如实写 `Ctrl+V paste`，做完图片再改回。
3. **§5/§9 的输入框内作答（`in_input`/`draft`）+ §9 的 `ChoiceKeys` 公开 API**（M）——
   内置 Alt+1..9 已完成，插件仍不能加自己的选项键；输入框作答是同一块地基。
4. **§6 多来源转写**（L）——`TranscriptSource`/`ShowAgent`/`Replaced`/`PastEffects`
   一起做；这是用户能看见的最大一块（子 agent、已保存会话、压缩历史）。
5. **§7 公开只读 API → §10 插件扩展点**（L+L）——选中态已经有了，先补
   `TranscriptView`/`DrawnTranscript`/`StatusLine` 这类只读导出，最后再动"开放画布"。
