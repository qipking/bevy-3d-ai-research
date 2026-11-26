# 还没做的事（按用户能感知的价值排序）

> **状态**：当前（2026-11-26 08:2x；`rig_niri` **606** 个 lib 测试、app lib **36**、bin **5**、
> `feathers_in_3d` **15**，`cargo test --workspace --offline` 全绿）。
> 配套 [`PARITY.md`](PARITY.md)：那边是"每个模块到什么程度"，这里是"现在真正还剩什么、先做哪个"。
> **口径（2026-11-26 起）**：内容/行为对照 rig-tui；**视觉与布局以 `bevy_feathers` + niri 为准**
> （[`FEATHERS_UI.md`](../design/FEATHERS_UI.md)、[`NIRI_SCROLL.md`](../design/NIRI_SCROLL.md)），
> **像素级一致不再是验收标准**。F1–F8、F10 已落地；布局 **L1–L4 已落地，L5 未做**。
> **范围**：`rig_niri` 前端 + 为跑通它必须接的应用层线（`rig_harness_in_3d`）。
> 真实模型接入（provider/API key）不在这份表里——那是 app 侧的事，见 [`HANDOFF.md`](HANDOFF.md) §7。
> **工作量口径**：**S** ≤ 半天｜**M** ≈ 1–3 天｜**L** ≥ 1 周（按一个熟悉这套代码的人算）。
> **编号不能重排**：别的文档按 `#1`–`#10` 引用这份表，已完成项保留位次；本轮新盘点的
> niri 交互 / 主题 / 存档读取 / 移动端单列 §12。
> **快照警告**：行号是 2026-11-26 08:2x 的快照，**函数名/类型名是稳定锚**；所有条目都能用
> `grep`/`read` 复现（PARITY §6 有命令）。

**现在的分布**：✅ 已完成 6 项（#1 #2 #3 #4 #5 #7；#4 的文本/路径/拖入已完成，
“对话图片接线”另计，见 §4）；🟡 部分完成 4 项
（#6 S1–S4 已完成、S5–S6 未做；#8 `↓ new output`/`[stopped]` 已完成、压缩提示未做；
#9 队列/选项键已完成、`ChoiceSystems` 未做；#10 切片 1/2 已完成、命令绑定未做）。
§11 里 F1–F8、F10 已完成，F9 未做，F6 切片 3 未做。

---

## 0. 现在真正还剩什么（先看这张表）

| 序 | 事项 | 用户能感知到什么 | 位置 | 工作量 |
|---|---|---|---|---|
| 1 | 多来源转写收尾：压缩条目与被压缩摘要、重启前的调用事实 | 压缩后旧条目不再"凭空消失"；重启后旧卡片仍有耗时/调用树 | §6（S5–S6） | **L**（S5 约 M、S6 约 L） |
| 2 | 对话图片接线（剪贴板图片 → `Scratch::images` → `@path` → 消息附件） | Ctrl+V 一张图就能发给模型，不只是粘贴文本路径 | §4 | **S** |
| 3 | 浅色主题对比度 | `/theme light` 下 3D 小字号不糊成中灰 | §12.1 | **S** |
| 4 | 主题持久化 | 重启后仍是上次选的 dark/light | §11 F7 | **S** |
| 5 | 存档会话异步读取 | `/show` 打开时不再卡住帧（尤其是会话多、存储慢时） | §6 S4 剩余 | **S–M** |
| 6 | niri 交互补齐：consume/expel、拖列、L5 滚动插值 | 能把窗口并进邻列/弹出成列、用鼠标拖列、滚动不瞬移 | §12.2 | **S–M** |
| 7 | F6 切片 3：插件命令绑定入口 | 插件能注册"我的命令 + 默认键"，不用改核心 | §10 / §11 F6 | **M** |
| 8 | 公开 view/选择 API | 插件能读"当前选中哪条/画了什么"，能加搜索高亮 | §7 / PARITY §3.15 | **S**（数据已在） |
| 9 | Android 适配 | 手机转头看面板、点按钮、下单全链路 | §12.4 | **L**（需 NDK/SDK/真机） |

> 排序口径：先"用户直接看见"（1–4），再"用户能感觉到的卡顿/操作"（5–6），
> 最后是插件/移植完整度（7–8）与平台扩展（9）。互相之间都不阻塞，1–5 可并行。

---

## 1. 按 Enter 发不出消息 —— ✅ 已完成（2026-11-25 第二轮）

- **现在的链路**：`editor::keys`（`editor.rs:1280`）把非空输入按 Enter 变成
  `Deliver::new(agent, text, DeliveryMode::Steer)`（`:1418`）；一轮正在跑时 Tab 变成
  `Queue`（`act` `:1154`）；空输入 Ctrl+D 写 `AppExit`（`:1427`）；
  有 turn 在跑时 Esc 触发 `StopTurn`（`:1429`）。目标 agent 取 `ShownAgent`
  （`editor.rs:1313`，应用层切工作区时更新它），所以发到的是**聚焦工作区**那个 agent。
  斜杠命令仍只走 `RunCommand`，不再额外发一条普通消息（`editor.rs:1412` 一带）。
- **顺带修的根因**：`completion_act`（`editor.rs:1198`）只在补全列表**开着**时接管
  Tab/Enter/Esc/↑↓，不再吞掉发送/停止。
- **测试**：`rig_niri`：`enter_delivers_the_trimmed_text_to_the_shown_agent_as_steer`
  （`editor.rs:2994`）、`tab_queues_while_a_turn_runs`（`:3038`）、`escape_stops_the_running_turn`
  （`:3087`）、`ctrl_d_exits_only_when_the_input_is_empty`（`:3130`）；
  app 侧：`a_typed_prompt_reaches_the_kernel_and_comes_back_as_recalled`
  （`rig_harness_in_3d/src/lib.rs:1056`）、`enter_sends_to_the_focused_workspaces_agent`
  （`workspaces_demo.rs:1130`）。
- **还没做（都不阻塞）**：本 crate 没有 `refused` 概念，所以没有上游
  `Deliver::literal()` 的"命令被拒后当普通消息重发"（`editor.rs:1184`）；
  排队没有 UI 反馈、不能撤回（`grep -n Queue` 在 `batch.rs`/`pane.rs` 为 0）；
  斜杠命令经 Tab 仍是立即 Steer（`editor.rs:1154` → 应用层 `RunCommand`，
  `rig_harness_in_3d/src/lib.rs:378`）。

---

## 2. 输入框只画一行：换行、光标所在行、长文本都看不见 —— ✅ 已完成（2026-11-25）

- **现在的行为**：`editor::layout_within`（`editor.rs:682`）按输入面板列宽折行，
  用的是转写同一个 `rig_tui::wrap::wrap_typed`；`editor_rows`（`:1591`）每行产一个
  `KeyedRow`，第一行 `› `、续行两个空格（`visible_rows` `:221`）。光标不再是把字符塞进文本，而是插在
  **真实行列**上的反色 + 下划线 `▏`（`CURSOR` `:266`、`with_cursor` `:299`），
  中文/emoji 按 unicode 显示列算。行数超过 `MOST_LINES=10`（`:115`）时 `EditorLayout.top` 跟着
  光标滚动，光标行一定在 `top..top+visible` 里（`layout` `:672`/`:682`）。
- **输入 tile 高度**：`sync_editor_tile`（`:1573`）把 `FrameTree` 的 Input 叶子改成
  `input_tile_height`（`:181`）= `max(3 行, clamp(行数, 1, 10) 行 + chrome)`，
  chrome 由 `PanelInset::DEFAULT` 算出（`INPUT_CHROME` `:131`，pad 8 + border 2，上下共 20px）：
  行高取 feathers 的 24px（`INPUT_ROW_HEIGHT` `:165`），空输入 72px、4 行 116px、10 行 260px。
  Input 仍是**固定子**，多/少出来的高度由顶部 Spacer（比例子）吸收，右列总高精确不变；
  `layout::default_tree` 的初始 72px 保留（`layout.rs:1123` 的 root，`main_row` `:1137`）。
- **测试**：`long_text_wraps_to_the_input_width`（`editor.rs:3186`）、
  `cursor_is_drawn_at_its_real_row_and_column`（`:3249`）、
  `scrolling_keeps_the_cursor_row_visible`（`:3510`）、
  `editor_rows_writes_every_visible_line_with_the_cursor_in_place`（`:3586`）、
  `the_input_tile_grows_and_shrinks_with_the_text`（`:3698`）。
- **还差（不阻塞）**：输入框的**键盘**选区（Shift+方向键）没有——`Editor` 只有光标，没有锚点；
  鼠标拖选已能落在输入面板并反色/复制（`panels.rs:298` 对所有面板生效，测试
  `clicking_the_input_boxes_blank_area_keeps_its_own_text_selection`，`pane.rs:2662`）。
  输入非空时 hints tile 现在写一行空行、不再显示键位（`hints.rs:183`），tile 高度不变；
  "输入框里打字作答"见 §5（已完成）；输入框的 Shift+方向键选区仍写在 §11 F10 的剩余里。

---

## 3. 窗口缩放重排 —— ✅ 已完成（2026-11-25 第五轮）

- **现在的行为**：`FrameSize`（`frame.rs:119`）每帧由 `sync_frame_size_to_window`（`:553`）
  从主窗口的**逻辑像素**换算成格子数（`from_window_size` `:182`），值变了才写资源；拖大/拖小窗口，
  布局、窗口化、输入框折行、条带滚动都在下一帧按新尺寸走。`RF_FRAME=96x28` 仍可钉死
  （`FrameEnv` `:207`、`FrameSizePinned` `:261`）。
- **物理→逻辑**：`feathers_in_3d` 的三处镜像用节点的 `inverse_scale_factor` 把
  `ComputedNode` 的物理像素换回逻辑像素（`world3d.rs:101`/`:107`/`:117`，
  1× 缩放有逐字节回归测试 `:886`），否则 2× DPI 下 3D 面板会大一倍、和 `Val::Px` 布局对不上。
- **测试**：`the_frame_size_follows_the_primary_window`（`frame.rs:1848`）、
  `frame_plugin_follows_the_primary_window`（`:1897`）、`resizing_the_window_relayouts_the_panels`
  （`:1921`）、`an_unchanged_window_does_not_rewrite_the_frame_size`（`:1983`）、
  `workspace_strip_spacing_uses_the_same_frame_size`（`:2066`）。
- **还差（都不阻塞）**：`WindowResized` 只被"换算成格子"用；极小窗口没有最小尺寸钳制
  （`a_tiny_window_lays_out_without_panicking` 只保证不 panic，`:2030`）；
  resize 会清掉文本选区（`selection.rs:788`，有意），但没有"resize 后恢复转写位置"的说法。
  **条带竖直重叠已修**：间距按当前帧高重算，见 §12.2 与 `workspaces_demo.rs:557`。

---

## 4. Ctrl+V 附件粘贴 / 拖入 `@path` —— ✅ 文本/路径粘贴与拖入已完成；**对话图片接线**未做（2026-11-26）

- **文本粘贴（已完成）**：Ctrl+V 绑的还是 `InputAction::PasteImage`（`keys.rs:659`），
  `editor::act` 把它变成 `Act::Paste`（`editor.rs:1150`），`editor::keys` 调
  `clipboard::paste_from`（`editor.rs:1435`，实现 `clipboard.rs:726`）：优先内存替身、其次
  `bevy_clipboard::Clipboard`，有文本就 `Editor::insert` 到光标处（多行文本里的
  `\n` 原样保留，`\r\n`/`\r` 归一）；空剪贴板、没有后端、读取失败写状态行的
  `PASTE_FAILED`（`clipboard.rs:126`/`:129`），成功粘贴会清掉上一次的红色错误（`editor.rs:1442`）。
  提示行已经改成 `Ctrl+V paste`（`hints.rs:51`/`:59`），和实际行为一致。
- **文本里的图片路径（已完成）**：`clipboard::image_paths`（`clipboard.rs:486`）识别文本里的
  图片文件路径，`clipboard::attach_image_paths`（`:499`）把它们换成 `@path`（**只有整段都是
  图片路径**才替换；混有普通文字就原样保留，和上游一致）。粘贴成功路径 `paste_text`（`:663`）
  先做这个替换再 `Editor::insert`。规则：
  - 分词照 shell：空白分隔，单/双引号里的空白属于路径且引号去掉，反斜杠转义下一个字符
    （Windows 除外）；已经是 `@path` 的词不再处理。
  - `file://` 只接受空 host 或 `localhost`（别的 host 不猜成本机路径）；百分号解码
    （`%20` → 空格），解码结果必须是合法 UTF-8。
  - 只认扩展名 `png/jpg/jpeg/gif/webp/bmp/svg/avif`（大小写不敏感），**不要求文件此刻存在**。
  - 含空白的路径写成 `@"/tmp/a b.png"`（和 `type_path` 的引用规则一致）。
- **拖入（已完成）**：应用层 `drop_image_files`（`rig_harness_in_3d/src/lib.rs:389`，注册在
  `:350` 的 `FeathersSystems::Prepare`）读 `bevy_window::FileDragAndDrop::DroppedFile`，用
  `clipboard::dropped_file_text`（`clipboard.rs:525`）判断扩展名，图片就插 `@path`；非图片
  文件忽略，没有 `Editor` 资源时静默忽略、不 panic。`FileDragAndDrop` 由 `WindowPlugin`
  注册，`HarnessExtPlugin` 也补了 `add_message`，headless 测试因此能直接写消息。
- **测试**：Ctrl+V 四条（`editor.rs:3817`/`:3839`/`:3863`/`:3886`）；路径纯逻辑在
  `clipboard.rs:1216`/`:1255`/`:1280`/`:1299`/`:1313`/`:1324`/`:1339`/`:1354`；
  拖入在 `rig_harness_in_3d/src/lib.rs:932`/`:951`。
- **还差（= 对话图片接线，未做）**：`bevy_clipboard` 没开 `image` feature，离线环境也拉不到
  新依赖，所以不做"剪贴板里直接是图片"的解码；`read_paste` 只读文本（`clipboard.rs:467`），
  `Paste::Image`（`:452`–`:459`）**没有生产者**。上游的落点是"图片存进会话的
  `Scratch::images`，消息按路径附件"（`rig-harness-0.47.0/src/host/session.rs:170`），
  本仓还没有任何地方用 `Scratch`（`grep -rn Scratch rig_harness_in_3d/src` 为 0）；
  送出时也没有调用上游的 `rig_tools::attach::attachments`/`read_image`
  （`rig-tools-0.47.0/src/attach.rs:262`/`:337`）把 `@path` 读成附件，所以输入框里出现的
  `@path` 只是文本 token，图片内容不会进对话。转写里模型/工具自带的图片按上游渲染成
  `[image]`/`[N images]` 文字占位（`batch.rs:1357`/`:1368`–`:1379`，这是内容对照，不是缺口）；
  剪贴板里只有图片时仍只报
  `"The clipboard holds no text."`（`clipboard.rs:72`/`:467`）。
- **工作量**：剩余 **S**（解码/落盘 + 送出时用 `rig_tools::attach` 读 `@path` 附件；`Scratch::images` 是上游存图位置）。
- **依赖**：`bevy_clipboard` 的 `image` feature 与会话目录（`SessionPaths`，应用层已有）。

---

## 5. 确认/问题作答 —— ✅ 键盘作答 + 输入框作答已完成（2026-11-26）

- **现在的行为**：确认 tile 里有未回答的 `Ask` 时，`keys::arm_inline_keys`（`keys.rs:829`）
  摘掉 Alt+1..9/Alt+0 绑定的 `Disabled`（绑定生成 `keys.rs:701` 一带）：Alt+n 用当前确认 tile
  第一块（最老、按内核效应 id FIFO）的第 n 个选项回答（`answer_shown_ask` `editor.rs:1065` →
  `answer_ask` `:916`）；越界什么都不做。Alt+0 与 Esc 拒绝问题（`refuse_shown_ask` `:1087` →
  `refuse_ask` `:931`，`Outcome::refused`）。Esc 只在有未回答问题、且该问题属于当前工作区时抢占
  （`editor.rs:1364`–`:1368`、`keys::ask_is_visible` `keys.rs:817`）。
- **点击与键盘共用一条路**：`editor::answer_ask`（`editor.rs:916`）是唯一的作答落点——
  应用层点击观察者 `kernel::answer_ask`（`rig_harness_in_3d/src/kernel.rs:291`）和键盘 Alt+n
  都调它；拒绝是 `editor::refuse_ask`（`:931`）。
- **输入框作答（✅ 2026-11-26）**：最老的 `Ask` 是 free-text（`free_text && !multiple && !inline`，
  上游 `ask.rs:62-82`；`editor.rs:983`–`:986`）时，Enter 不再发送消息，而是用输入框里的文本回答它
  （`editor::first_free_text_ask` `:978` → `answer_with_text` `:1049`，`Decider::Person` +
  `Answer::text`，即上游 `pick.rs:128-134` 的 `in_input`/`draft` 语义）；输入为空时 Enter
  不作答也不发送。多个 free-text 问题按内核效应 id FIFO 答最老的。`initial` 在问题第一次成为
  最老的可作答问题时填进空输入框，Ask 实体上的 `AnswerSeeded`（`editor.rs:1009`）保证只填一次；
  `multiline` 问题照旧 Shift+Enter 换行、Enter 提交。确认 tile 的 free-text 块最后一行有
  dim 提示 `类型作答：在输入框写，Enter 提交`（`batch.rs:1704`/`:1705`）。选项型
  `Ask`（`free_text == false`）与 `inline` 问题（`Ask::inline`，问题显示在工作旁边）不抢
  Enter，仍是普通消息。
- **测试（输入框作答）**：`enter_answers_a_free_text_ask_instead_of_delivering`（`editor.rs:2735`）、
  `enter_with_an_empty_input_answers_nothing_and_sends_nothing`（`:2759`）、
  `enter_answers_the_oldest_free_text_ask_first`（`:2833`）、
  `a_multiline_free_text_ask_takes_every_line_typed_before_enter`（`:2852`）、
  `the_initial_text_is_filled_once_and_never_again`（`:2870`）、
  `enter_still_delivers_when_the_ask_only_has_choices`（`:2779`）、
  `the_confirm_tile_points_a_free_text_ask_at_the_input`（`:2898`）。
- **应用层**：`answer_to_decision`（`kernel.rs:257`）现在也把"人拒绝问题"（Esc/Alt+0，没有
  `Answer`）转成被按住调用的拒绝；否则 hold 释放后，没决定的 `create_order` 会照常开跑。
- **测试**（`rig_niri`，`editor.rs`）：`alt_one_answers_the_first_choice_of_the_confirm_ask`
  （`:2525`）、`alt_two_answers_the_second_choice_of_the_confirm_ask`（`:2545`）、
  `alt_past_the_last_choice_answers_nothing`（`:2557`）、
  `alt_digits_answer_the_first_ask_in_the_confirm_tile`（`:2574`）、
  `alt_zero_refuses_the_ask_the_way_the_harness_does`（`:2596`）、
  `inline_keys_arm_only_while_an_ask_is_open`（`:2616`）、
  `escape_refuses_the_open_ask_before_stopping_the_turn`（`:2697`）；
  应用层（`kernel.rs`）：`a_click_and_alt_one_make_the_same_rig_harness_answer`（`:450`）、
  `a_click_can_answer_any_choice_and_alt_one_the_first`（`:473`）、
  `refusing_the_ask_cancels_the_held_order`（`:490`）。
- **还没做（不阻塞）**：多问题时键盘只答第一块（FIFO），不能像上游那样用插件自定义的
  `ChoiceKeys` 指定答哪一块；`Alt+(选项数+1)` 把问题挪进输入框作答的分支也没有。
- **工作量**：剩余 **S**。
- **依赖**：无（输入框内作答复用已有的 `Editor`）。

---

## 6. 看不了别的会话：子 agent、已保存会话、被压缩掉的条目、重启前的调用事实（S1–S4 已完成，S5–S6 未做）

- **用户会看到什么**：S1–S4 之后已经能 `/show` 切到子 agent 或已保存会话（标题行标明在看谁），
  但**压缩（compaction）移出对话的条目仍直接消失**、`N messages above were compacted` 不画，
  **重启后旧调用卡片没有时长/调用树**。
- **进展（2026-11-26）**：`SHOW_AGENT.md` 的 **S1–S4 已完成**。
  - **S1/S2**：`TranscriptPane` 组件（`source`/`title`/每窗格 `scroll`/`selected`/`below`，
    `pane.rs:613`）+ `ActiveTranscript`（`:662`）；`batch::source_rows`（`batch.rs:2044`）支持
    `Shown`/`Agent`/`Value`，`dialogue_batch_of`（`:929`）建存档批，`pane::window_transcript`
    （`pane.rs:816`）按每块窗格自己的来源取行、各自窗口化；`Pane2` 不再镜像
    （`grep -rn mirror_pane2_rows rig_niri/src` 为 0）。
  - **S3**：应用层 `show.rs` 注册 `/show`（`rig_harness_in_3d/src/show.rs:77`）：无参数弹 `Picker`
    （`shown` + 每个活 agent + 已保存会话，`:292`）；`/show <id>`、`/show agent <id>`、
    `/show session <id|名字>`、`/show shown`。它只改**聚焦工作区活动窗格**
    （`ActiveTranscript` → 主转写）的 `TranscriptPane.source`/`title`（`:236`），
    **不移动 `ShownAgent`**；`title` 非空时 `window_transcript` 在窗格第一行画 dim 标题
    （`pane.rs:734`，不参与滚动、不占条目）。
  - **S4**：`/show` 的 picker 用 `Home::saved_sessions` + `StoreOpener::open` + `SessionMeta`
    列最近 8 个（`show.rs:326`–`:360`）；`/show session` 用 `read_conversation`（`:272`）读成
    `TranscriptSource::Value`（只读，不动 `ShownAgent`/`Conversation`）。
    **S4 的剩余**：这条读取是**同步**的——`/show` 命令直接在系统里 `read_conversation`，
    picker 构造时对每个会话 `opener.open(...)`（`show.rs:326`–`:340`）。会话多/存储慢时会卡帧，
    `SHOW_AGENT.md` §4 原本要求异步。见 §12.3。
- **S5–S6 未做**：压缩条目（`Replaced`/`summarized_in_log`）与重启前的调用事实
  （`CallFact` 落盘/`PastEffects` 替代）仍缺。
  `grep -rnE 'Replaced|PastEffects' rig_niri/src` 只剩 `batch.rs:106` TODO(past) 和
  `batch.rs:2643` 的说明注释（`struct Replaced` 0 命中；没有 `calls.rs`）。
- **上游 API / 我们的落点**：上游 `pane::{TranscriptPane, TranscriptSource::{Shown,Agent,Value}}`
  （`pane.rs:57`/`:84`）、`view::ShowAgent`（`view.rs:49`）、`transcript::Replaced`
  （`transcript.rs:723`）、`transcript::PastEffects`（`:434`）、`render::compacted_line`
  （`render.rs:797`）。我们：来源/标题/滚动已经按窗格拆开（S1/S2/S3），存档走公开 API
  （S4）；还缺的是 `Replaced` 的行生成与 `CallFact` 落盘，两者都不改 `TranscriptSource`
  的形状。
- **工作量**：S5–S6 剩余 **L**（`Replaced` 两条数据源 + 行生成约 M；`CallFact` 落盘 +
  `Effects::continue_after` 约 L）。
- **依赖**：§7 的条目身份（已有 `Selection::item`，但没有公开 `TranscriptView`）；
  `PastEffects` 要看 rig-harness 的效应日志是否可读（只读服务）。

---

## 7. Alt+↑↓ 选不了条目，Esc 也回不到输入框 —— ✅ 已完成（2026-11-25 第五轮）

- **现在的行为**：`pane::pane_keys`（`pane.rs:1081`）里，Alt+↑/↓ 选"上一条/下一条"（
  `select_pane_item` `:1013`），块身份来自行 key 的高 32 位（`item_rows` `:118`）；
  条目第一行不在窗口里时把它顶到窗顶，两端夹住不越界；选中态写进 `Selection`
  （`Selection::select_item` `selection.rs:213`，每次窗口化后 `sync_item_selection`
  `pane.rs:1185` 把块序号换算回可见行）。Esc 清掉条目选区并把键交回输入框
  （`release_transcript_focus` `pane.rs:1247`）。
- **和上游的差异（有意）**：上游在两端会取消选中，我们**夹住**（`pane.rs:1010`–`:1011` 注释）；
  上游走 `PaneKeys` 绑定表，我们是物理键直读——所以
  `InputAction::{SelectUp,SelectDown,Deselect}` 在 `act` 里仍是空实现（`editor.rs:1136`–`:1138`），
  插件不能重绑这几个键。
- **测试**（`pane.rs`）：`alt_down_selects_items_in_order_and_alt_up_walks_back_without_overshooting`
  （`:2172`）、`a_selected_item_puts_all_of_its_rows_in_the_selection_and_no_others`（`:2209`）、
  `selecting_an_item_scrolls_it_into_view`（`:2245`）、
  `alt_up_with_no_selection_starts_at_the_newest_item`（`:2272`）、
  `escape_clears_the_item_selection_and_gives_the_keys_back_to_the_input`（`:2281`）、
  `a_vanished_block_clears_the_item_selection`（`:2322`）、
  `a_keyboard_item_follows_the_rows_after_paging_and_scrolling_back`（`:2384`）、
  `an_item_that_scrolls_out_of_view_gives_the_keys_back`（`:2411`）、
  `live_calls_become_one_item_each_and_alt_down_steps_between_them`（`:2717`）。
- **还差（不阻塞）**：`TranscriptView`/`TranscriptItem`/`CallItem` 这套公开 API（上游
  `view.rs:143`/`:84`/`:112`）——插件读不到"当前选中哪条、有哪些条"；`ScrollTo`/`ItemRow`
  （上游 `:259`/`:271`）没有公开入口；`PaneKeys` 绑定（上游 `pane.rs:125`）没有。
- **工作量**：公开 API 是 **S**（数据已经在 `Selection::item` 里）。

---

## 8. 状态标记：`↓ new output` ✅、`[stopped]` ✅、压缩提示 ⛔（依赖 §6）

- **`↓ new output`（✅ 2026-11-25）**
  - **判定来源**：`PaneScroll::follow` + `transcript::BelowSeen`（现在是 bevy 资源，
    `PanePlugin::init_resource` 持有，`pane.rs` 每帧读）。`window_transcript`（`pane.rs:816`）
    调 `BelowSeen`（`transcript.rs:1047`）的 `of(follow, total, top, height)`（`pane.rs:973`）：跟随末尾 →
    `Below::Nothing`；刚滚上去 → `Below::More`；滚动之后整段又长了 → `Below::New`。
  - **落点**：`window_transcript` 的最后一行右端（`below_marker` `pane.rs:772`），文案取
    `transcript::{MORE_BELOW, NEW_OUTPUT}`（`:1010`/`:1012`，与上游 `pane::draw_below`
    逐字一致）。宽度裁剪同上游：窗格列数小于标记显示宽度时整句不画、最后一行留空；
    窗格只有一行时不占标记行。标记行 hash 按内容算，`more below` 换成 `new output` 时
    增量 diff 会重画。
  - **测试**（`pane.rs`）：`following_the_tail_draws_no_below_marker`（`:1713`）、
    `scrolling_up_shows_more_below_then_new_output_when_rows_arrive`（`:1729`）、
    `scrolling_back_to_the_bottom_clears_the_marker_and_resets_below_seen`（`:1765`）、
    `a_pane_narrower_than_the_below_marker_leaves_the_last_row_blank`（`:1799`）。
- **`[stopped]`（✅ 2026-11-25）**
  - **判定来源**：对话 turn 记录的 `TurnRecord.end == Some(TurnEnd::Stopped { at: Some(id), .. })`
    （`batch.rs:983`–`:991`）。这是持久来源，和上游 `transcript::stop_lines` 读同一种记录；
    live 的 `TurnOutcome` 只在 turn 实体上，不能事后判断。`at` 是停止那一刻该轮最后一条消息；
    `at: None` 或那条消息已被移走 → **不画**，不凭空造。
  - **落点**：`batch::conversation_frame` 产 `TextId::Message { at, block: STOPPED_BLOCK }`
    （`STOPPED_BLOCK = usize::MAX`，`batch.rs:216`；最大块序号让它在排序上排在该消息所有
    内容块之后、下一条消息之前）。行是 dim 的 `[stopped]`（`:1209`–`:1211`）。这里复用
    `TextId::Message` 而不新增变体：应用层非聚焦条带的 `retain_text` 只保留
    `TextId::Message { .. }`（`workspaces_demo.rs:441`），新变体会在那条路径被丢掉。
    上游对停在 user 消息/工具结果上的轮次还会写 `[stopped before it was answered]`
    （`transcript.rs:1669`）；这里按任务要求统一成一条 `[stopped]`。
  - **测试**：`transcript::tests::a_stopped_row_follows_the_last_block_of_its_message`
    （`transcript.rs:1225`）；`batch::tests::a_stopped_turn_draws_a_dim_stopped_line_after_its_last_message`
    （`batch.rs:2576`）、`an_answered_turn_has_no_stopped_line`（`:2606`）、
    `a_stop_record_without_a_last_message_draws_nothing`（`:2626`）。
- **压缩提示（⛔ 没做，原因已核实）**
  - **摘要长什么样**：`rig_memory::apply_summary` 写一条 user 条目，作者
    `Author::plugin("compaction")`，正文以 `"The conversation history before this point was
    compacted into the following summary:\n\n<summary>\n"` 开头、`"\n</summary>"` 结尾。
    这条摘要本身已经在画（带作者的文字 → `RenderKey::Entry`），缺的是它上面那行分隔。
  - **为什么画不了**：上游 `render::compacted_line(count)`（`render.rs:797`）要的 count 是
    "这条摘要代表的归档条目数"（`Entry.stands_for`）。它只在做压缩的一方填的 `Replaced`
    （§6）和会话日志里（`rig_memory::summarized_in_log` 重放）；live `Conversation` 只有
    摘要条目，`EntryRef`/`TurnRecord`/`SummaryState` 都不带条数，被替换的消息已经从对话里
    移走。`rig_niri` 没有 `Replaced`，也没有从 `Conversation` 读会话日志的入口
    （`SessionLog` 不暴露 replaced），所以**不画**——不写猜出来的 N。
  - **防误画**：`batch::tests::a_compaction_summary_gets_no_invented_message_count`
    （`batch.rs:2646`）构造真实摘要形状，断言摘要仍照常画成 entry、且没有任何
    `N ... compacted` 行。
  - **真正要做**：等 §6 的 `Replaced`（或应用层把 `summarized_in_log` 喂给前端）；文案照上游
    `render::compacted_line`：1 条 `── 1 message above was compacted ──`，其余
    `── N messages above were compacted ──`，dim，放在摘要 entry 上方。
- **工作量/依赖**：前两个已完成；压缩提示剩余 **S**，依赖 §6 的 `Replaced`/历史读取。

---

## 9. picker 队列 / 内置选项键 / `ChoiceKeys` 已完成（✅）；`ChoiceSystems` 仍是缺口（🟡）

- **picker 队列（✅ 2026-11-25）**：多个 picker 同时存在时，条带最右边仍然只有
  **一列**、一次显示一个，按创建顺序排队；关掉队首后下一个在**同一帧**的 `sync_picker`
  里接进同一列。队列是 `PickerQueue` 资源（`picker.rs:256`）：`Add<Picker>` 观察者（`:723`）
  按 spawn/insert 顺序入队，`sync_picker`（`:831`）每帧清掉已 despawn 的、兜底补漏。
  最后一列关掉后 `remove_columns_everywhere`（`:644`）摘列、树逐字节复原；
  列里有 preview 时先摘 `Preview` 叶子再摘整列（`sync_preview_panel` `:975`）。
- **`Ask` 有意不进这个队列**：3D 这边的 `Ask` 是条带里的确认 tile
  （`batch::confirm_rows`，`PanelId::Confirm`），不是 picker 列、也没有弹层。
  队列只装 `Picker`，和上游 `queue_choices`（`pick.rs:500`）的差异是故意的。
- **内置选项键已落地（✅）**：picker 显示时 `picker_keys`（`picker.rs:1102`）接管 Alt+1..9——
  选第 n 个**可见**项并直接确认（和点击一样交 `Chosen::item`），Alt+0 取消（和 Esc 一样）；
  没有 picker 时这些键仍归输入框/插件绑定。同时补上：`more`（→ 展开选中项的完整说明，
  再按收起）、列表体 `scroll`（↑↓/PgUp/PgDn 移动时窗口跟着走）、`picked`（过滤/重新填充后
  按条目身份选回同一项）——`PickerInput` 字段见 `picker.rs:164`–`:191`。测试：
  `alt_one_picks_the_first_item_like_a_click`（`:2955`）、`alt_three_picks_the_third_item`
  （`:2976`）、`alt_zero_cancels_the_picker`（`:2991`）、`alt_past_the_last_item_answers_nothing`
  （`:3003`）、`alt_digits_stay_with_the_input_when_no_picker_shows`（`:3014`）、
  `more_expands_and_collapses_the_selected_items_description`（`:3264`）、
  `scrolling_follows_the_selection_and_stops_at_the_ends`（`:3330`）、
  `filtering_keeps_the_selected_label_when_it_still_matches`（`:3408`）。
- **`ChoiceKeys` 已移植（✅ 部分）**：`ChoiceKeys`（`picker.rs:86`）+ `ChoiceKey`（`:102`）+
  `ChoiceAction`（`:124`，收窄成 `Choose(n)`/`Cancel`/`Message(String)`）挂在 `Picker`
  实体上；`picker_keys` 在 picker 显示时先于内置键与过滤输入处理，命中发
  `ChoiceKeyPressed`（`:140`）。测试 `choice_keys_fire_a_message_and_keep_out_of_the_filter`
  （`:3090`）、`choice_keys_choose_and_cancel_like_alt_digits`（`:3123`）、
  `choice_keys_do_not_take_alt_digits_away`（`:3156`）、
  `choice_keys_on_a_queued_picker_do_not_take_the_key`（`:3186`）、
  `choice_keys_without_a_picker_do_not_swallow_the_key`（`:3210`）。
- **还没做**：上游的 `ChoiceSystems` 调度（上游 `pick.rs:589`）与
  `Tick`/`TickAll`/`TickNone`/`More`/`Answer` 变体（现在用 `Message` 让插件自己做）；
  树里的 `more` 没键（树里 →/← 归折叠）。`Ask` 的输入框作答**不在 picker 这条路上**：
  3D 前端的确认 tile 由 `editor.rs` 直接接 Enter（见 §5），所以 `in_input` 在
  `rig_niri` 里仍为 0 是有意分叉，不再是缺口。
- **测试（队列）**：`pickers_show_one_at_a_time_in_creation_order`（`:3546`）、
  `the_same_picker_is_never_queued_twice`（`:3628`）、`escape_hands_the_column_to_the_next_picker`
  （`:3663`）、`closing_a_picker_whose_preview_is_open_removes_the_whole_column`（`:3695`）、
  `an_ask_never_enters_the_picker_queue`（`:3743`）。
- **工作量**：剩余部分 **S**（只剩 `ChoiceSystems`）。
- **依赖**：无（§5 已完成）。


---

## 10. 插件扩展点：画自己的面板、自己的状态行、绑命令、读"画了什么"

- **状态（2026-11-26）**：**切片 1 + 切片 2 已完成**，设计见
  [`PANEL_CANVAS.md`](../design/PANEL_CANVAS.md)。插件现在能：
  - `app.register_panel("git")` / `register_panel_with("git", PanelSpec::everywhere(0.28))`
    拿 `PluginPanel` 句柄（`panel.rs:234`/`:241`，内部 `:145`/`:153`）；
  - 用 `add_panel_column(world, workspace, handle, fraction)` 把面板作为 `Fraction` 列
    放进指定工作区（`:271`），或用 `PanelAuto::AllWorkspaces` 自动装进**所有**工作区（`:374`）；
  - `remove_panel_column` / `remove_panel_everywhere` / `unregister_panel` 摘列/注销
    （`:440`/`:521`/`:541`），只动包含该面板的那一列（已有列宽不变）；
  - 拿根实体 spawn `Node`/`Text`/`ImageNode`（3D 镜像直接可见），并用
    `drawn_row(world, workspace, panel, n)` 读回第 n 行的世界矩形（`:722`）；
  - 给**任意面板根**（内置 `PanelId` 或插件面板）挂 `status::StatusLine`（`status.rs:114`），
    在底部画一条与 `PanelId::Status` 同源的状态行（输入框上落在编辑器底行、不额外占行）；
  - 用 `focus_panel_column`（`panel.rs:626`）让插件列成为活动列，
    `scroll_active_column` / 改宽 / 移动列键都认它；picker 开列/关列与它共存。
  当前 `rig_niri` lib 测试 **606**（内置 `PanelId` 面板的行为/几何不变）。
- **还没做**：
  - 插件**命令绑定**的公开入口（注册表 + 默认键位；`InputAction` / `KeyBinding` /
    `run_command` 已有，缺的是“插件注册自己的命令 + 默认键”的 API）——F6 切片 3；
  - `DrawnRow` 的可见性辅助（`effective_clip` 非空）与行文本/高亮的读回扩展；
  - `RequestRedraw` / `RedrawArea` / `TuiFrame` 门控（[`PANEL_CANVAS.md`](../design/PANEL_CANVAS.md) §4
    明确不做：3D 每帧重排 + 每帧镜像，没有省一帧的收益）。
- **上游 API / 我们的落点**：上游 `panel::{TuiPanel, PanelCanvas, RequestRedraw, RedrawArea}`
  （`rig-tui-0.47.0/src/panel.rs:106`/`:160`/`:82`/`:91`）、
  `status::{StatusLine, StatusParts, AppStatusPartsExt}`（同目录 `status.rs:382`/`:296`/`:329`）、
  `render::TuiFrame`（`render.rs:1196`）、`view::DrawnTranscript`（`view.rs:316`）、
  `keys::run_command`（`keys.rs:455`）、`add_view`（`lib.rs:260`）。我们：`PanelId` 仍是
  内置封闭枚举，但多了一个 `PanelId::Plugin(PluginPanel)` 变体（`panels.rs:66`），
  插件身份/注册/放置/读回/列表策略在 `rig_niri/src/panel.rs`；任意面板的状态行在
  `status.rs::StatusLine` + `draw_status_lines`（与 `status_rows` 共用
  `merged_status_items` / `status_line_row`）；`TuiFrame`/`DrawnTranscript` 没有移植
  （3D 里读回是 `DrawnRow` 的世界矩形），也没有 `RequestRedraw` 门控；
  `run_command` 已移植（`keys.rs:490`）；`BatchPlugin` 仍要应用层手动加
  （`rig_harness_in_3d/src/main.rs:68`）。
- **工作量**：切片 1/2 已完成；**只剩命令绑定 M**（读回可见性辅助 S 可选，不阻塞）。
- **依赖**：`StatusLine`/自动工作区/活动列都依赖现有面板画布（根/区域/行/树），
  无 §6 依赖；命令绑定独立；`DrawnRow` 的几何只依赖 `RowLayout` + `PanelAreas`（已有）。


---

## 11. UI 重组（feathers）：F6 以后

> F1–F5 已完成（[`FEATHERS_UI.md`](../design/FEATHERS_UI.md) §2）；**F6 切片 1、2 已完成**
> （插件面板注册/条带列/行读回 + `StatusLine`/自动工作区/活动列，见
> [`PANEL_CANVAS.md`](../design/PANEL_CANVAS.md)）；**F7 已完成**（暗/浅切换，持久化未做）；
> **F8 已完成**（`ImageNode` 镜像，SVG 未做）；**F10 已完成**（键盘复制）。
> 这一节只列**还没做**的候选，不阻塞功能移植。布局的 L1–L5 在
> [`NIRI_SCROLL.md`](../design/NIRI_SCROLL.md)，L1–L4 已完成、L5 未做。

| 步 | 内容 | 工作量 | 依赖 / 现状 |
|---|---|---|---|
| **F6** | 开放面板画布：**切片 1、2 已完成（2026-11-26）**——注册表 + `PanelId::Plugin(..)` + `add_panel_column` + `drawn_row`；切片 2 的 `StatusLine`、`PanelAuto::AllWorkspaces` 自动工作区、`remove_panel_*` / `unregister_panel` 整树移除、`ActivePanelColumn` 插件列焦点；**只剩**插件命令绑定入口（切片 3，§10） | **剩余 M** | 同 §10；`PanelId` 只扩了一个变体，内置面板行为不变（当前 lib 测试 606） |
| **F7** | 主题切换 UI：**暗/浅切换已完成（2026-11-26），持久化未做**——`/theme` 无参数报当前/可选值，`/theme dark\|light` 同时替换 `Theme` + `UiTheme`，非法参数只写可见 notice；`RF_THEME=light` 启动走 `theme::install_theme`；浅色 17 band 与 `create_light_theme()` 的 feathers token 都在 `theme.rs`；**未做** `[view] theme` 落盘、高对比/自定义主题（浅色对比度见 §12.1） | **S（仅持久化）** | 无；`add_setting` 的先例已在（`status.rs:490`、`settings.rs:76`）、`ViewSettings` 已有 `hide_status`（`settings.rs:31`） |
| **F8** | 3D 镜像图标/图片：**`ImageNode` 已完成（2026-11-26）**——tint/flip/rect/裁剪，测试 `world3d.rs:1114`/`:1188`；**SVG 图标未做**（没开 `bevy_ui/svg`，见 §12.5） | **S（仅 SVG，可选）** | 无 |
| **F9** | 过渡/补间：动作现在当帧生效，niri 的 `scroll` 插值与面板切换没有动画 | **S–M** | 依赖 L5（§12.2）；先做 `scroll` 插值再谈按钮过渡 |
| **F10** | 转写选区键盘复制：**已完成（2026-11-26）**——`Ctrl+Shift+C`（`InputAction::CopySelection`，独立系统 `clipboard::copy_selection_keys` `:273`，不塞进 `editor::keys`）复制活动转写窗格的鼠标拖选或 Alt+↑/↓ 选中的条目（条目按整条的所有视觉行、`\n` 连接），写 `Copied N lines`；没有选中时不写剪贴板、状态行给 `NOTHING_SELECTED`。原列的“输入框 Shift+方向键选区”仍**未做**：`Editor` 只有光标、没有锚点 | **S（复制已完成）/ 原输入框选区 M** | 鼠标松手复制、Alt+↑↓ 选条目、`Ctrl+C` 清空都不变 |

**不做**：不为了"像终端"回退 F1–F5 的 token；不引入弹层；不给 `TextInput`/`ListView`/`Button`
换内核（理由见 `FEATHERS_UI.md` §4）。


---

## 12. 其他还没做（niri 交互 / 主题 / 移动端）

### 12.1 浅色主题的 3D 小字号对比度（S）

- `/theme light` 功能可用（`UiTheme` + 17 band 一起换、面板/按钮/滚动条都跟着走），
  但 3D 里的文本是字形网格，13px 在 3D 缩放后笔画覆盖约 60%，浅色主题下会糊成中灰；
  深色主题"亮字暗底"更清楚。证据：[`FEATHERS_UI.md`](../design/FEATHERS_UI.md) §5.5；
  浅色定义在 `theme.rs:753`（`Theme::light`）/`:309`（`create_light_theme`）。
- **未做**：浅色下加大字号 / 换 bold 字面，或给文本材质单独做不透明处理。默认仍是 dark。
- **工作量 S**，不阻塞其他项。

### 12.2 niri 交互收尾（L5 + consume/expel + 拖列，S–M）

- **L1–L4 已完成**：列宽 `Size::Fraction`（`layout.rs:99`）、滚动 `WorkspaceScroll`
  （`workspaces.rs:110`）+ `placements_scrolled`（`layout.rs:302`）+ 视口裁剪
  （`frame.rs:674`）、聚焦/滚动键（`workspaces.rs:406`/`:490`/`:546`）、预设宽度/移动列
  （`layout.rs:912`/`:944`；`workspaces.rs:630`）。状态表见 `NIRI_SCROLL.md` 开头。
- **L5 未做**：`scroll` 当帧写定，没有视觉插值，也没有 `center-focused-column on-overflow`；
  全仓没有 `WorkspaceScroll` 的补间（`grep -rnE "interpolat|\\btween\\b" rig_niri/src` 为 0）。
- **consume/expel 未做**：只有"拆窗"（`frame.rs:1136` `split_transcript`）和整列增删
  （`layout.rs:739`/`:827`），没有 niri 的"把窗口吞进邻列 / 从列里弹出成独立列"
  （`grep -rn "fn consume\|fn expel" rig_niri/src` 为 0）。
- **拖列未做**：移动列只有键盘 `Alt+Ctrl+Shift+H/L`（`workspaces.rs:630` 的
  `ColumnAction::Move` → `move_column` `layout.rs:944`），没有用鼠标拖动列头/列的交互。
- **条带竖直重叠已修（不是缺口）**：工作区间距按**当前帧高**算
  （`workspaces.rs:1203` `workspace_strip_height`、`:1208` `workspace_step`），
  resize 时应用层 `restack_on_resize`（`workspaces_demo.rs:557`）只重排自动位姿、
  手放的不动；测试 `workspaces_demo.rs:654`/`:692`/`:730`。
- **工作量**：L5 是 **S**；consume/expel 是 **S–M**；鼠标拖列是 **M**。

### 12.3 存档会话异步读取（S–M）

- **现状**：`/show` 命令在系统里同步 `read_conversation`（`rig_harness_in_3d/src/show.rs:272`），
  `/show` 无参 picker 构造时对每个已保存会话同步 `opener.open(...)`（`:326`–`:340`）。
  会话多/网络盘慢时会阻塞这一帧（`SHOW_AGENT.md` §4 的 S4 设计要求异步）。
- **要做**：把 `saved_sessions`/`read_conversation` 移到 `IoTaskPool`，先出列表、读完再换
  `TranscriptSource::Value`（或先显示"读取中"标题）；`show.rs` 已有测试
  （`:696`）可改成等待任务完成。
- **工作量 S–M**；只影响 `/show` 的打开速度，不阻塞功能。

### 12.4 Android 适配（L）

- **现状**：相机只有键盘/手柄（`camera_rig.rs`；`grep -rn "gyro\|sensor\|ndk\|android" rig_niri/src` 为 0）。
  `HANDOFF.md` §7 的接续清单第 2 条：把 `camera_rig` 的 yaw/pitch 换成设备姿态
  （Android `ndk::sensor`、iOS `objc2-core-motion`、wasm `DeviceOrientationEvent`），
  键盘/手柄作兜底。
- **方案与验收**：`RIG_NIRI_MOBILE_PLAN.md` §5/§7 的 M3（Android 传感器 + 陀螺仪相机 + 触屏兜底，
  约 2 周）；本机未装 NDK/SDK、也没有真机（同文档风险表），所以只能先做无窗口的姿态变换测试。
- **工作量 L**；依赖工具链与真机，不阻塞桌面功能。

### 12.5 SVG 图标（S，可选）

- `feathers_in_3d` 现在镜像 `ImageNode`（`world3d.rs:186`），但 **SVG 图标不显示**：
  `bevy_ui` 的 `svg` feature 没开（`FEATHERS_UI.md` §2.6）。要做得先加 feature + 确认
  SVG loader 形态，位图图标/截图已可用。
- **工作量 S**；纯粹是视觉完整度。

---

## 已知小差异（不单列，不值得单独排期）

| 差异 | 上游 | 我们 | 影响 |
|---|---|---|---|
| 空闲 Esc | 停插件想停的东西（rebuild 等，`input.rs:854` 一带） | 没有 turn 在跑时什么也不做（`editor.rs:1161`–`:1162`，注释说明不空触发） | 插件没有"Esc 停我的后台工作"的入口（属于 §10） |
| `Ctrl+V` 提示文案 | `image` | `paste`（`hints.rs:51`/`:59`，如实写当前行为） | 对话图片接线（§4）做完后改回 `image` |
| 输入框内按键提示 | `draw_editors` 按状态换（recalled / 面板拿走键 / 正在作答 / 空输入，上游 `editor.rs:686`） | 两张表 + "输入是否为空 / turn 是否在跑"（`hints.rs:44`、`:183`） | 少几种状态的文案；不影响按键 |

> **已从这张表移除**（2026-11-26 复核，都已被代码推翻）：
> **键位提示裁剪**——`fit_keys` 已按上游逐字实现（`hints.rs:73`，测试 `:288`/`:302`/`:312`/`:321`）；
> **提示行何时显示**——现在只在输入为空时显示、非空画空行（`hints.rs:183`，测试 `:432`）；
> **工具卡片事实**——`ToolRender.facts` 已接（`batch.rs::published_facts` `:541`，
> 三处填充 `:507`/`:666`/`:1879`）；
> **主题注入**——batch 已用 `upstream_theme(&Theme)`（`batch.rs:630`/`:935`/`:1550`/`:1817`），
> `NO_COLOR` 与 `RF_THEME` 走 `Theme::from_env` / `ThemeKind::from_env`（`theme.rs:727`/`:213`）；
> **turn 级状态项**——`TurnOf` 的 `StatusItems` 已注册（`lib.rs:216`–`:217`），
> 状态行按 `ActiveTurn` 读 turn 的条目（`status.rs:135`，测试 `:644`）；
> **输入框作答**——free-text 问题 Enter 直接在输入框作答（§5）；
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
| `status` 的阶段拆分（`StatusSystems::{Show,Edit}`） | 状态行每帧现算一行，插件用 `StatusParts` 加条目（`status.rs:212`） | `status.rs:41` 自注 |
| 终端格坐标 / 逐字节对齐 | 视觉与布局以 feathers + niri 为准，不再以终端为准 | `FEATHERS_UI.md` §1/§6；布局见 `NIRI_SCROLL.md` |
| 上游 `tests.rs`（433 行 TUI 集成测试） | 不是移植目标 | 我们 **606** 个内联测试覆盖同等行为 |

---

## 建议的动手顺序（依赖最少、用户最先看到）

> 2026-11-26：F1–F8、F10 与 L1–L4 已落地；下面是**真正还没做**的候选，按"看得见的收益 ÷ 成本"排。
> 详细证据见 §0 表和各节。

1. **§6 S5–S6：压缩条目 + 重启前的调用事实**（L）——用户能看见"压缩后旧条目消失"和"重启后卡片
   没耗时/调用树"；§8 的压缩提示等它的 `Replaced`。这是最大的一块，先做 S5（M）再做 S6（L）。
2. **§4 对话图片接线**（S）——文本/路径粘贴与拖入已完成，只差把剪贴板图片存进
   `Scratch::images` 并让 `@path` 成为真正的图片附件。
3. **§12.1 浅色主题对比度 + §11 F7 主题持久化**（S + S）——两个都是一眼可见的小改动，
   `/theme` 已有唯一入口（`theme.rs:173`/`:282`），`ViewSettings` 已有先例（`settings.rs:31`）。
4. **§12.3 存档会话异步读取**（S–M）——`/show` 已经是用户入口，卡帧会直接感觉到。
5. **§12.2 niri 交互收尾**（S–M）：consume/expel、鼠标拖列、L5 滚动插值——比"插件 API"更贴近
   日常操作；F9 过渡等 L5。
6. **§10 / §11 F6 切片 3：插件命令绑定**（M）——切片 1/2 已经把面板/状态行/读回做完，
   剩最后一截；开发者向，排在用户可见项之后。
7. **§7 公开 view/选择 API**（S）——数据已在 `Selection::item`，补 `TranscriptView`/`Highlight`/
   `DrawnTranscript` 就能让插件做搜索/读回。
8. **§12.4 Android 适配**（L）——工具链/真机就绪后再排；§12.5 SVG 图标（S）可以随时插队。
