# 还没做的事（按用户能感知的价值排序）

> **状态**：当前（2026-11-26；快照 00:5x，`rig_niri` **526** 个 lib 测试，`cargo test` 全绿）。
> 这个 526 是 F1–F5 的基线；写文档期间 `niri-l1` 并行改动的工作树已到 **528**（也全绿），
> 行数/行号会继续变，函数名是稳定锚。
> 配套 [`PARITY.md`](PARITY.md)：那边是"每个模块到什么程度"，这里是"现在真正还剩什么、先做哪个"。
> **口径（2026-11-26 起）**：内容/行为对照 rig-tui；**视觉与布局以 `bevy_feathers` + niri 为准**
> （[`FEATHERS_UI.md`](../design/FEATHERS_UI.md)、[`NIRI_SCROLL.md`](../design/NIRI_SCROLL.md)），
> **像素级一致不再是验收标准**。F1–F5 已落地；布局 L1（列宽模型）**正在另一个子代理手里做**，
> 本文不把 L1–L5 记成完成。
> **范围**：`rig_niri` 前端 + 为跑通它必须接的应用层线（`rig_harness_in_3d`）。
> 真实模型接入（provider/API key）不在这份表里——那是 app 侧的事，见
> [`HANDOFF.md`](HANDOFF.md) §7。
> **工作量口径**：**S** ≤ 半天｜**M** ≈ 1–3 天｜**L** ≥ 1 周（按一个熟悉这套代码的人算）。
> **编号不能重排**：别的文档按 `#1`–`#10` 引用这份表，已完成项保留位次；新增的 UI 重组单列 §11。
> **快照警告**：`picker.rs`/`pane.rs`/`editor.rs`/`hints.rs`/`status.rs` 这一轮都在改（F1–F5、
> 选项键、滚动条、提示行逐字对齐、状态行占位），**行号以快照为准，函数名不会漂**。
> 所有条目都能用 `grep`/`read` 复现（PARITY §6 有命令）。

**现在的分布**：已完成 5 项（#1 #2 #3 #5 #7）；部分完成 3 项（#4 #8 #9）；
未开始 2 项（#6 #10）；UI 重组（feathers）另见 §11（F10 已完成，F6 切片 2/F7–F9 未开始）。

---

## 1. 按 Enter 发不出消息 —— ✅ 已完成（2026-11-25 第二轮）

- **现在的链路**：`editor::keys` 把非空输入按 Enter 变成
  `Deliver::new(agent, text, DeliveryMode::Steer)`（`editor.rs:1274`）；一轮正在跑时 Tab 变成
  `Queue`（`editor.rs:1038`）；空输入 Ctrl+D 写 `AppExit`（`editor.rs:998`）；
  有 turn 在跑时 Esc 触发 `StopTurn`（`editor.rs:1279`）。目标 agent 取
  `ShownAgent`（`editor.rs:1169`，应用层切工作区时更新它），所以发到的是**聚焦工作区**那个 agent。
  斜杠命令仍只走 `RunCommand`，不再额外发一条普通消息（`editor.rs:1300` 一带）。
- **顺带修的根因**：`completion_act` 以前在补全列表**关着**时也吞掉 Tab/Enter/Esc/↑↓，
  导致发送与停止永远到不了 `act`；现在只在列表开着时接管（`editor.rs:1082` `completion_act`）。
- **测试**：`rig_niri`：`enter_delivers_the_trimmed_text_to_the_shown_agent_as_steer`
  （`editor.rs:2585`）、`tab_queues_while_a_turn_runs`（`:2629`）、`escape_stops_the_running_turn`
  （`:2678`）、`ctrl_d_exits_only_when_the_input_is_empty`（`:2721`）；
  app 侧：`a_typed_prompt_reaches_the_kernel_and_comes_back_as_recalled`
  （`rig_harness_in_3d/src/lib.rs:953`）、`enter_sends_to_the_focused_workspaces_agent`
  （`workspaces_demo.rs:917`）。
- **还没做（都不阻塞）**：本 crate 没有 `refused` 概念，所以没有上游
  `Deliver::literal()` 的"命令被拒后当普通消息重发"（`editor.rs:1068`）；
  排队没有 UI 反馈、不能撤回（`grep -n Queue` 在 `batch.rs`/`pane.rs` 为 0）；斜杠命令经 Tab 仍是
  立即 Steer（`editor.rs:1038` → 应用层 `RunCommand`，`rig_harness_in_3d/src/lib.rs:362`）；
  滚动状态挂在 `TranscriptPane.scroll` 上（S1，`pane.rs:608`），但 `WorkspacePanels` 仍只有
  rows/roots/spawned/areas（`workspaces.rs:119`），每个工作区各一份还没做。

---

## 2. 输入框只画一行：换行、光标所在行、长文本都看不见 —— ✅ 已完成（2026-11-25）

- **现在的行为**：`editor::layout_within`（`rig_niri/src/editor.rs:677`）按输入面板列宽折行，
  用的是转写同一个 `rig_tui::wrap::wrap_typed`；`editor_rows`（`editor.rs:1447`）每行产一个
  `KeyedRow`，第一行 `› `、续行两个空格（`visible_rows` `:216`）。光标不再是把字符塞进文本，而是插在
  **真实行列**上的反色 + 下划线 `▏`（`CURSOR` `editor.rs:261`、`with_cursor` `:294`），
  中文/emoji 按 unicode 显示列算。行数超过 `MOST_LINES=10`（`:110`）时 `EditorLayout.top` 跟着
  光标滚动，光标行一定在 `top..top+visible` 里（`layout` `:667`/`:677`）。
- **输入 tile 高度**：`sync_editor_tile`（`editor.rs:1429`）把 `FrameTree` 的 Input 叶子改成
  `input_tile_height`（`editor.rs:176`）= `max(3 行, clamp(行数, 1, 10) 行 + chrome)`，
  chrome 由 `PanelInset::DEFAULT` 算出（`INPUT_CHROME` `:126`，pad 8 + border 2，上下共 20px）：
  空输入 72px、4 行 116px、10 行 260px（F5 把行高换成 feathers 的 24px 后）。Input 仍是**固定子**，
  多/少出来的高度由顶部 Spacer（比例子）吸收，右列总高精确不变；`layout::default_tree` 的初始
  72px 保留（`layout.rs:783`）。
- **测试**：`long_text_wraps_to_the_input_width`（`editor.rs:2777`）、
  `cursor_is_drawn_at_its_real_row_and_column`（`:2840`）、
  `scrolling_keeps_the_cursor_row_visible`（`:3101`）、
  `editor_rows_writes_every_visible_line_with_the_cursor_in_place`（`:3177`）、
  `the_input_tile_grows_and_shrinks_with_the_text`（`:3252`）。
- **还差（不阻塞）**：输入框的**键盘**选区（Shift+方向键）没有——`Editor` 只有光标，没有锚点；
  鼠标拖选已能落在输入面板并反色/复制（`panels.rs:203` 对所有面板生效，测试
  `clicking_the_input_boxes_blank_area_keeps_its_own_text_selection`，`pane.rs:2460`）。
  输入非空时 hints tile 现在写一行空行、不再显示键位（`hints.rs:183`），tile 高度不变；
  "输入框里打字作答"是另一件事，见 §5；输入框的 Shift+方向键选区仍写在 §11 F10 的剩余里
  （F10 的转写选区键盘复制已完成）。

---

## 3. 窗口缩放重排 —— ✅ 已完成（2026-11-25 第五轮）

- **现在的行为**：`FrameSize`（`frame.rs:117`）每帧由 `sync_frame_size_to_window`（`frame.rs:530`）
  从主窗口的**逻辑像素**换算成格子数（`from_window_size` `:180`），值变了才写资源；拖大/拖小窗口，
  布局、窗口化、输入框折行都在下一帧按新尺寸走。`RF_FRAME=96x28` 仍可钉死（`FrameEnv` `:205`、
  `FrameSizePinned` `:259`，`FramePlugin` 在 `:1128` 一带装），截图/测试因此确定。
- **物理→逻辑**：`feathers_in_3d` 的三处镜像用节点的 `inverse_scale_factor` 把
  `ComputedNode` 的物理像素换回逻辑像素（`feathers_in_3d/src/world3d.rs:65`–`:88`，
  `physical_to_logical`/`physical_size_to_world`/`physical_len_to_world`；1× 缩放有逐字节回归测试
  `:385`），否则 2× DPI 下 3D 面板会大一倍、和 `Val::Px` 布局对不上。
- **测试**：`the_frame_size_follows_the_primary_window`（`frame.rs:1776`）、
  `frame_plugin_follows_the_primary_window`（`:1825`）、`resizing_the_window_relayouts_the_panels`
  （`:1849`）、`an_unchanged_window_does_not_rewrite_the_frame_size`（`:1911`）、
  `workspace_strip_spacing_uses_the_same_frame_size`（`:1994`）。
- **还差（都不阻塞）**：`WindowResized` 只被"换算成格子"用；极小窗口没有最小尺寸钳制
  （`a_tiny_window_lays_out_without_panicking` 只保证不 panic，`:1958`）；
  resize 会清掉文本选区（`selection.rs:787`，有意），但没有"resize 后恢复转写位置"的说法。

---

## 4. Ctrl+V 附件粘贴 / 拖入 `@path` —— ✅ 文本/路径粘贴与拖入已完成；剪贴板图片解码未做（缺 feature）（2026-11-26）

- **文本粘贴（已完成）**：Ctrl+V 绑的还是 `InputAction::PasteImage`（`keys.rs:563`/`:649`），
  `editor::act` 把它变成 `Act::Paste`（`editor.rs:1127`），`editor::keys` 调
  `clipboard::paste_from`（`editor.rs:1409`，实现 `clipboard.rs:496`）：优先内存替身、其次
  `bevy_clipboard::Clipboard`，有文本就 `Editor::insert` 到光标处（多行文本里的
  `\n` 原样保留，`\r\n`/`\r` 归一）；空剪贴板、没有后端、读取失败写状态行的
  `PASTE_FAILED`（`clipboard.rs:118`/`:121`，和 `Copied` 同一套状态反馈），成功粘贴会清掉
  上一次的红色错误（`clipboard.rs:126`）。提示行已经改成 `Ctrl+V paste`（`hints.rs:33`/`:41`），
  和实际行为一致。
- **文本里的图片路径（已完成）**：`clipboard::image_paths`（`clipboard.rs:285`）识别文本里的
  图片文件路径，`clipboard::attach_image_paths`（`clipboard.rs:297`）把它们原地换成 `@path`
  （其余文字、换行、路径之间的连接词都不动；没有图片路径返回 `None`）。粘贴成功路径
  `paste_text`（`clipboard.rs:456`）先做这个替换再 `Editor::insert`。规则：
  - 分词照 shell：空白分隔，单/双引号里的空白属于路径且引号去掉，反斜杠转义下一个字符
    （Windows 除外）；已经是 `@path` 的词不再处理。
  - `file://` 只接受空 host 或 `localhost`（别的 host 不猜成本机路径）；百分号解码
    （`%20` → 空格），解码结果必须是合法 UTF-8。
  - 只认扩展名 `png/jpg/jpeg/gif/webp/bmp/svg/avif`（大小写不敏感），**不要求文件此刻存在**。
  - 替换按自然位置；含空白的路径写成 `@"/tmp/a b.png"`（和 `type_path` 的引用规则一致）。
- **拖入（已完成）**：应用层 `drop_image_files`（`rig_harness_in_3d/src/lib.rs:382`，注册在
  `:344` 的 `FeathersSystems::Prepare`）读 `bevy_window::FileDragAndDrop::DroppedFile`，用
  `clipboard::dropped_file_text`（`clipboard.rs:321`）判断扩展名，图片就插 `@path`；非图片
  文件忽略，没有 `Editor` 资源时静默忽略、不 panic。`FileDragAndDrop` 由 `WindowPlugin`
  注册，`HarnessExtPlugin` 也补了 `add_message`（`rig_harness_in_3d/src/lib.rs:310`），
  headless 测试因此能直接写消息。
- **测试**：原有 Ctrl+V 四条（`editor.rs:3725` 起）不回归；路径纯逻辑在
  `clipboard.rs:743`/`:761`/`:782`/`:791`/`:801`/`:807`/`:820`/`:834`/`:845`，
  拖入在 `rig_harness_in_3d/src/lib.rs:920`/`:927`/`:937`。
- **还差（剪贴板图片解码，未做）**：`bevy_clipboard` 没开 `image` feature，离线环境也拉不到
  新依赖，所以不做"剪贴板里直接是图片"的解码/落盘。剪贴板里只有图片时仍只报
  "The clipboard holds no text."（`clipboard.rs:64`/`:266`）；`Paste::Image`/`type_path`
  （`clipboard.rs:469`）保留语义，等有了 `image` feature + `Scratch`/会话目录落盘约定再接
  （上游 `clipboard.rs:227` 的 `dropped_images` 一带）。
- **工作量**：**S**（剩余只有剪贴板图片解码/落盘）。
- **依赖**：剪贴板图片部分依赖 `bevy_clipboard` 的 `image` feature 与会话目录（`SessionPaths`，应用层已有）。

---

## 5. 确认/问题作答 —— ✅ 键盘作答 + 输入框作答已完成（2026-11-26）

- **现在的行为**：确认 tile 里有未回答的 `Ask` 时，`keys::arm_inline_keys`（`keys.rs:720`）
  摘掉 Alt+1..9/Alt+0 绑定的 `Disabled`（绑定生成 `keys.rs:669` 一带）：Alt+n 用当前确认 tile
  第一块（最老、按内核效应 id FIFO，和 `batch::confirm_rows` 同一顺序）的第 n 个选项回答
  （`answer_shown_ask` `editor.rs:959` → `answer_ask` `:911`）；越界什么都不做。Alt+0 与 Esc
  拒绝问题（`refuse_ask` `:926`，`Outcome::refused`，和 rig-harness `pick.rs` 的
  `Pick::person(ask, None)` 是同一个结论）。Esc 只在有未回答问题、且该问题属于当前工作区时抢占
  （`editor.rs:1239`、`keys::ask_is_visible` `keys.rs:708`）；没有问题仍是原来的
  关补全 / 停 turn / 面板交还焦点。
- **点击与键盘共用一条路**：`editor::answer_ask`（`editor.rs:911`）是唯一的作答落点——
  应用层点击观察者 `kernel::answer_ask`（`rig_harness_in_3d/src/kernel.rs:291`）和键盘 Alt+n
  都调它；拒绝是 `editor::refuse_ask`（`:926`）。点击仍可点任何一块的任意选项。
- **输入框作答（✅ 2026-11-26）**：最老的 `Ask` 是 free-text（`free_text && !multiple`，
  上游 `ask.rs:62-82`）时，Enter 不再发送消息，而是用输入框里的文本回答它
  （`editor::first_free_text_ask` → `answer_with_text`，`Decider::Person` + `Answer::text`，
  即上游 `pick.rs:128-134` 的 `in_input`/`draft` 语义）；输入为空时 Enter 不作答也不发送。
  多个 free-text 问题按内核效应 id FIFO 答最老的。`initial` 在这个问题第一次成为最老的
  可作答问题时填进空输入框，Ask 实体上的 `AnswerSeeded` 保证只填一次（用户清空后不回填）；
  `multiline` 问题照旧 Shift+Enter 换行、Enter 提交。确认 tile 的 free-text 块最后一行有
  dim 提示 `类型作答：在输入框写，Enter 提交`（`batch::push_ask_block`）。选项型
  `Ask`（`free_text == false`）不抢 Enter，仍是普通消息（上游 "next line is a message,
  not its answer"）。
- **测试（输入框作答）**：`enter_answers_a_free_text_ask_instead_of_delivering`、
  `enter_with_an_empty_input_answers_nothing_and_sends_nothing`、
  `enter_answers_the_oldest_free_text_ask_first`、
  `a_multiline_free_text_ask_takes_every_line_typed_before_enter`、
  `the_initial_text_is_filled_once_and_never_again`、
  `enter_still_delivers_when_the_ask_only_has_choices`、
  `the_confirm_tile_points_a_free_text_ask_at_the_input`（`editor.rs`）。
- **应用层**：`answer_to_decision`（`kernel.rs:257`）现在也把"人拒绝问题"（Esc/Alt+0，没有
  `Answer`）转成被按住调用的拒绝；否则 hold 释放后，没决定的 `create_order` 会照常开跑。
- **测试**（`rig_niri`，`editor.rs`）：`alt_one_answers_the_first_choice_of_the_confirm_ask`
  （`:2311`）、`alt_two_answers_the_second_choice_of_the_confirm_ask`（`:2331`）、
  `alt_past_the_last_choice_answers_nothing`（`:2343`）、
  `alt_digits_answer_the_first_ask_in_the_confirm_tile`（`:2360`）、
  `alt_zero_refuses_the_ask_the_way_the_harness_does`（`:2382`）、
  `inline_keys_arm_only_while_an_ask_is_open`（`:2402`）、
  `alt_digit_without_an_ask_does_not_swallow_a_plugin_binding`（`:2431`）、
  `answering_the_ask_clears_the_confirm_panel_next_frame`（`:2457`）、
  `escape_refuses_the_open_ask_before_stopping_the_turn`（`:2483`）；
  应用层（`kernel.rs`）：`a_click_and_alt_one_make_the_same_rig_harness_answer`（`:450`）、
  `a_click_can_answer_any_choice_and_alt_one_the_first`（`:473`）、
  `refusing_the_ask_cancels_the_held_order`（`:490`）。
- **还没做**：输入框里直接打字作答（上游 `pick.rs:132` 的 `in_input`/`draft`，落到
  `Answer::text`）；多问题时键盘只答第一块（FIFO），不能像上游那样用插件自定义的 `ChoiceKeys`；
  `Alt+(选项数+1)` 把问题挪进输入框作答的分支也没有。
- **工作量**：输入框内作答是 **M**。
- **依赖**：无（输入框内作答复用已有的 `Editor`）。

---

## 6. 看不了别的会话：子 agent、已保存会话、被压缩掉的条目、重启前的调用事实（S1–S4 已完成，S5–S6 未做）

- **用户会看到什么**：转写永远只显示"当前 agent 的当前对话"：不能切到子 agent 看它干了什么，
  不能把已保存的会话读进来对照，压缩（compaction）移出对话的条目直接消失，重启后旧调用卡片
  没有时长/调用树，"N messages above were compacted" 也没有。拆窗（Alt+S）只是把同一份转写
  复制成两列（`panels.rs:156`）。
- **进展（2026-11-26）**：`SHOW_AGENT.md` 的 **S1–S4 已完成**。
  - **S1/S2**：`TranscriptPane` 组件（`source` / `title` / 每窗格 `scroll` / `selected` /
    `below`）+ `ActiveTranscript`；`batch::source_rows` 支持 `Shown`/`Agent`/`Value`，
    `pane::window_transcript` 按每块窗格自己的来源取行、各自窗口化，`Pane2` 不再镜像。
  - **S3**：应用层 `show.rs` 注册 `/show`（`HarnessExtPlugin`，和 `/hello`/`/setting`
    同一张命令表）：无参数弹 `Picker`（`shown` + 每个活 agent + 已保存会话）；
    `/show <id>`、`/show agent <id>`、`/show session <id|名字>`、`/show shown`。
    它只改**聚焦工作区活动窗格**（`ActiveTranscript` → 主转写）的 `TranscriptPane.source`/
    `title`，**不移动 `ShownAgent`**，所以主列看 `Agent`/`Value` 时全局 `Shown` 管线不受影响。
    `title` 非空时 `window_transcript` 在窗格第一行画 dim 标题（key `u64::MAX - 2`，
    不参与滚动、不占条目；`item_rows` 跳过），`shown`/空标题不占行。
  - **S4**：`/show` 的 picker 用 `Home::saved_sessions` + `StoreOpener::open` +
    `SessionMeta` 列最近 8 个；`/show session` 用 `read_conversation` 读成
    `TranscriptSource::Value`（只读，不动 `ShownAgent`/`Conversation`）。
  **S5–S6 未做**：压缩条目（`Replaced`/`summarized_in_log`）与重启前的调用事实
  （`CallFact` 落盘/`PastEffects` 替代）仍缺；`grep -rnE 'Replaced|PastEffects' rig_niri/src`
  只剩注释/TODO。
- **上游 API / 我们的落点**：上游 `pane::{TranscriptPane, TranscriptSource::{Shown,Agent,Value}}`
  （`pane.rs:57`/`:84`）、`view::ShowAgent`（`view.rs:49`）、`transcript::Replaced`
  （`transcript.rs:723`）、`transcript::PastEffects`（`:434`）、`render::compacted_line`
  （`render.rs:797`）。我们：来源/标题/滚动已经按窗格拆开（S1/S2/S3），存档走公开 API
  （S4）；还缺的是 `Replaced` 的行生成与 `CallFact` 落盘，两者都不改 `TranscriptSource`
  的形状。

- **工作量**：S5–S6 剩余 **L**（`Replaced` 两条数据源 + 行生成约 M；`CallFact` 落盘 +
  `Effects::continue_after` 约 L）。
- **依赖**：§7 的条目身份（已有 `Selection::item`，但没有公开 `TranscriptView`）；`PastEffects`
  要看 rig-harness 的效应日志是否可读（只读服务）。

---

## 7. Alt+↑↓ 选不了条目，Esc 也回不到输入框 —— ✅ 已完成（2026-11-25 第五轮）

- **现在的行为**：`pane::pane_keys`（`pane.rs:925`）里，Alt+↑/↓ 选"上一条/下一条"（
  `select_pane_item` `:860`），块身份来自行 key 的高 32 位（`item_rows` `:117`）；
  条目第一行不在窗口里时把它顶到窗顶，两端夹住不越界；选中态写进 `Selection`
  （`Selection::select_item` `selection.rs:212`，每次窗口化后 `sync_item_selection`
  `pane.rs:1027` 把块序号换算回可见行）。Esc 清掉条目选区并把键交回输入框
  （`release_transcript_focus` `pane.rs:1089`）。
- **和上游的差异（有意）**：上游在两端会取消选中，我们**夹住**（`:330`–`:331` 注释）；
  上游走 `PaneKeys` 绑定表，我们是物理键直读——所以
  `InputAction::{SelectUp,SelectDown,Deselect}` 在 `act` 里仍是空实现（`editor.rs:1020`–`:1026`），
  插件不能重绑这几个键。
- **测试**（`pane.rs`）：`alt_down_selects_items_in_order_and_alt_up_walks_back_without_overshooting`
  （`:1971`）、`a_selected_item_puts_all_of_its_rows_in_the_selection_and_no_others`（`:2008`）、
  `selecting_an_item_scrolls_it_into_view`（`:2044`）、
  `alt_up_with_no_selection_starts_at_the_newest_item`（`:2071`）、
  `escape_clears_the_item_selection_and_gives_the_keys_back_to_the_input`（`:2080`）、
  `a_vanished_block_clears_the_item_selection`（`:2121`）、
  `a_keyboard_item_follows_the_rows_after_paging_and_scrolling_back`（`:2183`）、
  `an_item_that_scrolls_out_of_view_gives_the_keys_back`（`:2210`）、
  `live_calls_become_one_item_each_and_alt_down_steps_between_them`（`:2515`）。
- **还差（不阻塞）**：`TranscriptView`/`TranscriptItem`/`CallItem` 这套公开 API（上游
  `view.rs:143`/`:84`/`:112`）——插件读不到"当前选中哪条、有哪些条"；`ScrollTo`/`ItemRow`
  （上游 `:259`/`:271`）没有公开入口；`PaneKeys` 绑定（上游 `pane.rs:125`）没有。
- **工作量**：公开 API 是 **S**（数据已经在 `Selection::item` 里）。

---

## 8. 状态标记：`↓ new output` ✅、`[stopped]` ✅、压缩提示 ⛔（依赖 §6）

- **`↓ new output`（✅ 2026-11-25）**
  - **判定来源**：`PaneScroll::follow` + `transcript::BelowSeen`（现在是 bevy 资源，
    `PanePlugin::init_resource` 持有，`pane.rs` 每帧读）。`window_transcript`（`pane.rs:748`）
    调 `BelowSeen`（`transcript.rs:1036`）的 `of(follow, total, top, height)`：跟随末尾 → `Below::Nothing`（标记消失、
    `seen` 复位成当前整段）；刚滚上去 → `seen = 当时整段行数`，下面还有旧行 → `Below::More`；
    滚动之后整段又长了（`total > seen`）→ `Below::New`。
  - **落点**：`window_transcript` 的最后一行右端（`below_marker` `pane.rs:717`），文案取
    `transcript::{MORE_BELOW, NEW_OUTPUT}`（`:999`/`:1001`，与上游 `pane::draw_below` 逐字一致）。
    宽度裁剪同上游：窗格列数小于标记显示宽度时整句不画、最后一行留空；窗格只有一行时不占标记行。
    标记行 hash 按内容算，`more below` 换成 `new output` 时增量 diff 会重画。
  - **测试**（`pane.rs`）：`following_the_tail_draws_no_below_marker`（`:1546`）、
    `scrolling_up_shows_more_below_then_new_output_when_rows_arrive`（`:1562`）、
    `scrolling_back_to_the_bottom_clears_the_marker_and_resets_below_seen`（`:1598`）、
    `a_pane_narrower_than_the_below_marker_leaves_the_last_row_blank`（`:1632`）。
- **`[stopped]`（✅ 2026-11-25）**
  - **判定来源**：对话 turn 记录的 `TurnRecord.end == Some(TurnEnd::Stopped { at: Some(id), .. })`
    （`batch.rs:963` 一带）。这是持久来源，和上游 `transcript::stop_lines` 读同一种记录；live 的
    `TurnOutcome` 只在 turn 实体上（Commit 后实体和 `TurnOf` 关系就没了），不能事后判断。
    `at` 是停止那一刻该轮最后一条消息（用户 Esc 经 `Stopper::stop` 会写）；`at: None`
    或那条消息已被移走 → **不画**，不凭空造。
  - **落点**：`batch::conversation_frame` 产 `TextId::Message { at, block: STOPPED_BLOCK }`
    （`STOPPED_BLOCK = usize::MAX`，`batch.rs:214`；最大块序号让它在排序上排在该消息所有
    内容块之后、下一条消息之前）。行是 dim 的 `[stopped]`（`:1183`–`:1191`）。这里复用
    `TextId::Message` 而不新增变体：应用层非聚焦条带的 `retain_text` 只保留
    `TextId::Message { .. }`（`workspaces_demo.rs:433` 一带），新变体会在那条路径被丢掉。
    上游对停在 user 消息/工具结果上的轮次还会写 `[stopped before it was answered]`
    （`transcript.rs:1669`）；这里按任务要求统一成一条 `[stopped]`。
  - **测试**：`transcript::tests::a_stopped_row_follows_the_last_block_of_its_message`
    （`transcript.rs:1214`）；`batch::tests::a_stopped_turn_draws_a_dim_stopped_line_after_its_last_message`
    （`batch.rs:2449`）、`an_answered_turn_has_no_stopped_line`（`:2479`）、
    `a_stop_record_without_a_last_message_draws_nothing`（`:2499`）。
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
    （`batch.rs:2519`）构造真实摘要形状（`Author::plugin("compaction")` + 包裹文案），
    断言摘要仍照常画成 entry、且没有任何 `N ... compacted` 行。
  - **真正要做**：等 §6 的 `Replaced`（或应用层把 `summarized_in_log` 喂给前端）；文案照上游
    `render::compacted_line`：1 条 `── 1 message above was compacted ──`，其余
    `── N messages above were compacted ──`，dim，放在摘要 entry 上方。
- **工作量/依赖**：前两个已完成；压缩提示剩余 **S**，依赖 §6 的 `Replaced`/历史读取。

---

## 9. picker 队列 / 内置选项键 / `ChoiceKeys` 已完成（✅）；`ChoiceSystems` 仍是缺口（🟡）

- **picker 队列（✅ 2026-11-25）**：多个 picker 同时存在时，条带最右边仍然只有
  **一列**、一次显示一个，按创建顺序排队；关掉队首（Esc/回车提交/取消 → harness
  收到 `Pick` 后 despawn）或被外部 despawn 后，下一个在**同一帧**的 `sync_picker`
  里接进同一列（容器/列表/preview 面板复用，不闪空列）。队列是 `PickerQueue`
  资源（`picker.rs:256`）：`Add<Picker>` 观察者（`:705`）在组件加入 world 的瞬间按
  spawn/insert 顺序入队（是创建顺序，不依赖 ECS 查询迭代顺序），`sync_picker`（`:813`）
  每帧清掉已 despawn 的、兜底补漏；`push`（`:262`）对同一实体去重。
  最后一个关掉后 `remove_columns_everywhere`（`:644`）摘列、树逐字节复原；列里有 preview
  时先摘 `Preview` 叶子再摘整列（`sync_preview_panel` `:952`）。
- **`Ask` 有意不进这个队列**：3D 这边的 `Ask` 是条带里的确认 tile
  （`batch::confirm_rows`，`PanelId::Confirm`），不是 picker 列、也没有弹层。
  队列只装 `Picker`，和上游 `queue_choices`（`pick.rs:500`）的差异是故意的。
- **内置选项键已落地（✅ 2026-11-25 第五轮）**：picker 显示时 `picker_keys`
  （`picker.rs:1079`）接管 Alt+1..9——选第 n 个**可见**项并直接确认（和点击一样交
  `Chosen::item`），Alt+0 取消（和 Esc 一样）；没有 picker 时这些键仍归输入框/插件绑定。
  同时补上：`more`（→ 展开选中项的完整说明，再按收起）、列表体 `scroll`（↑↓/PgUp/PgDn
  移动时窗口跟着走）、`picked`（过滤/重新填充后按条目身份选回同一项）——
  `PickerInput` 字段见 `picker.rs:164`–`:191`。测试：`alt_one_picks_the_first_item_like_a_click`
  （`:2932`）、`alt_three_picks_the_third_item`（`:2953`）、`alt_zero_cancels_the_picker`
  （`:2968`）、`alt_past_the_last_item_answers_nothing`（`:2980`）、
  `alt_digits_stay_with_the_input_when_no_picker_shows`（`:2991`）、
  `more_expands_and_collapses_the_selected_items_description`（`:3241`）、
  `scrolling_follows_the_selection_and_stops_at_the_ends`（`:3307`）、
  `filtering_keeps_the_selected_label_when_it_still_matches`（`:3385`）。
- **`ChoiceKeys` 已移植（✅ 部分）**：`ChoiceKeys`（`picker.rs:86`）+ `ChoiceKey`（`:102`）+
  `ChoiceAction`（`:124`，收窄成 `Choose(n)`/`Cancel`/`Message(String)`）挂在 `Picker`
  实体上；`picker_keys` 在 picker 显示时先于内置键与过滤输入处理，命中发
  `ChoiceKeyPressed`（`:140`）。测试 `choice_keys_fire_a_message_and_keep_out_of_the_filter`
  （`:3067`）、`choice_keys_choose_and_cancel_like_alt_digits`（`:3100`）、
  `choice_keys_do_not_take_alt_digits_away`（`:3133`）、
  `choice_keys_on_a_queued_picker_do_not_take_the_key`（`:3163`）、
  `choice_keys_without_a_picker_do_not_swallow_the_key`（`:3187`）。
- **还没做**：上游的 `ChoiceSystems` 调度（上游 `pick.rs:589`）与
  `Tick`/`TickAll`/`TickNone`/`More`/`Answer` 变体（现在用 `Message` 让插件自己做）；
  树里的 `more` 没键（树里 →/← 归折叠）。`Ask` 的输入框作答**不在 picker 这条路上**：
  3D 前端的确认 tile 由 `editor.rs` 直接接 Enter（见 §5），所以 `in_input` 在
  `rig_niri` 里仍为 0 是有意分叉，不再是缺口。
- **测试（队列）**：`pickers_show_one_at_a_time_in_creation_order`（`:3523`）、
  `the_same_picker_is_never_queued_twice`（`:3605`）、`escape_hands_the_column_to_the_next_picker`
  （`:3640`）、`closing_a_picker_whose_preview_is_open_removes_the_whole_column`（`:3672`）、
  `an_ask_never_enters_the_picker_queue`（`:3720`）、
  `closing_after_a_focus_switch_clears_the_old_workspaces_column`（`:3745`）。
- **工作量**：剩余部分 **S**（只剩 `ChoiceSystems`；内置选项键/队列/滚动/说明/输入框作答已完成）。
- **依赖**：§5 的输入框作答已完成，不再卡在这里。


---

## 10. 插件扩展点：画自己的面板、自己的状态行、绑命令、读"画了什么"

- **状态（2026-11-26）**：**面板注册 + 条带列 + 行读回已完成（切片 1）**，设计见
  [`PANEL_CANVAS.md`](../design/PANEL_CANVAS.md)。插件现在能
  `app.register_panel("git")` 拿 `PluginPanel` 句柄、用
  `add_panel_column(world, workspace, handle, fraction)` 把它作为 `Fraction` 列放进指定
  工作区的条带、拿根实体 spawn `Node`/`Text`/`ImageNode`（3D 镜像直接可见），并用
  `drawn_row(world, workspace, panel, n)` 读回第 n 行的世界矩形。内置 `PanelId` 面板的
  行为/几何不变（`rig_niri` lib 测试 580 → 587）。
- **还没做**：`StatusLine` 组件（任意面板底部一条状态行）留在切片 2；活动列/自动滚动、
  “注册后自动装进所有工作区”的策略、插件命令绑定入口也未做。
- **上游 API / 我们的落点**：上游 `panel::{TuiPanel, PanelCanvas, RequestRedraw, RedrawArea}`
  （`reference/.../panel.rs:106`/`:160`/`:82`/`:91`）、
  `status::{StatusLine, StatusParts, AppStatusPartsExt}`（同目录 `status.rs:382`/`:296`/`:329`）、
  `render::TuiFrame`（`render.rs:1196`）、`view::DrawnTranscript`（`view.rs:316`）、
  `keys::run_command`（`keys.rs:455`）、`add_view`（`lib.rs:260`）。我们：`PanelId` 仍是
  内置封闭枚举，但多了一个 `PanelId::Plugin(PluginPanel)` 变体（`panels.rs:66`），
  插件身份/注册/放置/读回在 `rig_niri/src/panel.rs`；状态行仍只有固定
  `PanelId::Status`（每帧在 `status_rows` `status.rs:186` 里现算），插件能加**条目**
  （`StatusParts` `:113`）但不能给任意面板挂 `StatusLine`；`TuiFrame`/`DrawnTranscript`
  没有移植（3D 里读回是 `DrawnRow` 的世界矩形，不需要终端屏幕格），也没有
  `RequestRedraw` 门控（bevy_ui + `sync_mirror` 每帧重排）；`run_command` 已移植
  （`keys.rs:488`）；`BatchPlugin` 仍要应用层手动加（`lib.rs:239`–`:256` 的插件元组里没有，
  `rig_harness_in_3d/src/main.rs:68` 自己加）。
- **工作量**：切片 1 已完成；**切片 2 约 M**（`StatusLine` S/M + 注册策略 S + 活动列 S/M +
  命令绑定 M）。不需要“`PanelId` → 实体标识”的架构改写了：`PanelId::Plugin` 只扩了一个变体，
  `layout`/`frame`/`workspaces` 的键空间原样复用。
- **依赖**：`StatusLine` 依赖现在这套面板画布（根/区域/行），无 §6 依赖；命令绑定独立；
  `DrawnRow` 的几何只依赖 `RowLayout` + `PanelAreas`（已有）。


---

## 11. UI 重组（feathers）：F6 以后

> F1–F5 已完成（[`FEATHERS_UI.md`](../design/FEATHERS_UI.md) §2）；**F6 切片 1 已完成**
> （插件面板注册/条带列/行读回，见 [`PANEL_CANVAS.md`](../design/PANEL_CANVAS.md)）。
> 这一节只列**还没做**的候选，不阻塞功能移植。布局的 L1–L5 在
> [`NIRI_SCROLL.md`](../design/NIRI_SCROLL.md)，L1 进行中，不在本节重复。

| 步 | 内容 | 工作量 | 依赖 / 现状 |
|---|---|---|---|
| **F6** | 开放面板画布：**切片 1 已完成（2026-11-26）**——注册表 + `PanelId::Plugin(..)` + `add_panel_column` + `drawn_row`（见 [`PANEL_CANVAS.md`](../design/PANEL_CANVAS.md)）；**切片 2 未做**：`StatusLine` 组件（任意面板一行状态）、自动装进所有工作区的策略、活动列自动滚动 | **L → 切片 2 M** | 同 §10；切片 1 只扩了 `PanelId` 一个变体，内置面板行为不变（`rig_niri` 580 → 587 测试） |
| **F7** | 主题切换 UI：`/theme` 与持久化（浅色/高对比），`UiTheme` 已是资源、面板/行/滚动条都按 token 解析（`theme.rs:98`–`:141`、`lib.rs:151`–`:159`），缺入口与 `[view] theme` 设置 | **S** | 无；`Settings`/`add_setting` 的先例已在（`status.rs:64`/`:281`） |
| **F8** | 3D 镜像图标/图片：feathers 组件里的 `ImageNode` 现在不镜像，按钮只能文字（`FEATHERS_UI.md` §5）；要做就得给 `feathers_in_3d` 加图片/图标层 | **M** | 无；影响所有带图标的 feathers 控件 |
| **F9** | 过渡/补间：动作现在当帧生效，niri 的 `scroll` 插值与面板切换没有动画 | **S–M** | 依赖 L1/L5；先做 `scroll` 插值再谈按钮过渡 |
| **F10** | 转写选区键盘复制：**已完成（2026-11-26）**——`Ctrl+Shift+C`（`InputAction::CopySelection`，独立系统 `clipboard::copy_selection_keys`，不占 `editor::keys` 的 16 参数上限）复制活动转写窗格的鼠标拖选或 Alt+↑/↓ 选中的条目（条目按整条的所有视觉行、`\n` 连接），写 `Copied N lines`；没有选中时不写剪贴板、状态行给 `NOTHING_SELECTED`。原列的“输入框 Shift+方向键选区”仍**未做**：`Editor` 只有光标、没有锚点 | **S（复制）/ 原输入框选区 M** | 鼠标松手复制、Alt+↑↓ 选条目、`Ctrl+C` 清空都不变 |

**不做**：不为了"像终端"回退 F1–F5 的 token；不引入弹层；不给 `TextInput`/`ListView`/`Button`
换内核（理由见 `FEATHERS_UI.md` §4）。


---

## 已知小差异（不单列，不值得单独排期）

| 差异 | 上游 | 我们 | 影响 |
|---|---|---|---|
| 空闲 Esc | 停插件想停的东西（rebuild 等，`input.rs:854` 一带） | 没有 turn 在跑时什么也不做（`editor.rs:1045`–`:1046`，注释说明不空触发） | 插件没有"Esc 停我的后台工作"的入口（属于 §10） |
| `Ctrl+V` 提示文案 | `image` | `paste`（`hints.rs:33`/`:41`，如实写当前行为） | 图片粘贴（§4）做完后改回 `image` |
| 输入框内按键提示 | `draw_editors` 按状态换（recalled / 面板拿走键 / 正在作答 / 空输入，上游 `editor.rs:686`） | 两张表 + "输入是否为空 / turn 是否在跑"（`hints.rs:44`、`:183`） | 少几种状态的文案；不影响按键 |

> **已从这张表移除**（2026-11-26 复核，都已被代码推翻）：
> **键位提示裁剪**——`fit_keys` 已按上游逐字实现（首尾空格、`unicode_width`、同权重丢最后，
> `hints.rs:73`，测试 `:288`/`:302`/`:312`/`:321`）；
> **提示行何时显示**——现在只在输入为空时显示、非空画空行（`hints.rs:183`，测试 `:432`）；
> **工具卡片事实**——`ToolRender.facts` 已接（`batch.rs::published_facts` `:539`，
> 三处填充 `:505`/`:664`/`:789`）；
> **主题注入**——batch 已用 `upstream_theme(&Theme)`（`batch.rs:628`/`:915`/`:1530`/`:1782`），
> `NO_COLOR` 走 `Theme::from_env`（`theme.rs:420`）；
> **turn 级状态项**——`TurnOf` 的 `StatusItems` 已注册（`lib.rs:205`–`:206`），
> 状态行按 `ActiveTurn` 读 turn 的条目（`status.rs:186`，测试 `:426`）；
> 以及更早移除的 **Ctrl+D 退出**、**Alt+↑↓ 条目选择**。

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
| **终端格对齐 / 面板像素位置** | 视觉与布局以 feathers + niri 为准，不再以终端为准 | `FEATHERS_UI.md` §1/§6；布局见 `NIRI_SCROLL.md` |
| **`keep`/`min_width` 的逐字节复刻** | 状态行现在直接调上游 `status::line`（`status.rs:186`），但"逐字节一致"本身不再是验收 | 同上 |
| `status` 的阶段拆分（`StatusSystems::{Show,Edit}`） | 状态行每帧现算一行，插件用 `StatusParts` 加条目（`status.rs:113`） | `status.rs:30`–`:35` 自注 |
| hints `fit_keys` 的上游裁剪细节 | 现在恰好逐字一致（`hints.rs:73`），但不再为它返工/加验收 | 按 `FEATHERS_UI.md` §6 |
| 上游 `tests.rs`（433 行 TUI 集成测试） | 不是移植目标 | 我们 **526**（基线；L1 并行时 528）个内联测试覆盖同等行为 |

---

## 建议的动手顺序（依赖最少、用户最先看到）

> 2026-11-26：F1–F5 已落地；布局 L1 在另一个子代理手里（`NIRI_SCROLL.md`），不排进这份表。
> **剩下的候选**按"看得见的收益 ÷ 成本"排：

1. **§4 图片粘贴 + 拖入 `@path`**（M）——Ctrl+V 的文本粘贴已完整，只差图片这一半；
   提示行已如实写 `Ctrl+V paste`，做完再改回 `image`。
2. **§9 输入框内作答（`in_input`/`draft`）+ `ChoiceSystems` 收尾**（M）——`ChoiceKeys`
   已能用，插件还差调度；输入框作答和 §5 是同一块地基。
3. **§11 F7 主题切换**（S）——`UiTheme` 已经驱动所有 token，缺 `/theme` 入口与持久化，
   是最便宜的"看得见"。
4. **§6 多来源转写**（L）——S1 已完成，S2–S6（`dialogue_batch_of`/`source_rows`/`ShowAgent`/
   `Replaced`/`PastEffects`）是用户能看见的最大一块；§8 的压缩提示等它的 `Replaced`。
5. **§11 F6 面板画布 + §10 插件扩展点**（L+L）——同一件事的两种说法；
   F8/F9 视需求插入（F10 的转写选区键盘复制已完成）。

