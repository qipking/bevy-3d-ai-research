//! 输入编辑器：多行文本框 + 光标 + 历史，键位走 [`crate::keys`] 的动作表。
//!
//! 游标语义对齐 rig-tui 0.47（`editor.rs:64`、`:133`）：`cursor` 是 `text` 的
//! **字节偏移**，永远落在 grapheme cluster 边界上；左右移动、退格、删除、
//! 插入、Ctrl+W/Alt+D 都按 grapheme 走，中文、emoji（含 ZWJ 序列）与组合字符
//! 都当一个字符。
//!
//! 历史模型也对齐上游：Up 先在第一行内上移，到第一行行首后再进历史；
//! Down 往回走，翻过最新一条时把浏览前写的草稿带回来。`Ctrl+P`/`Ctrl+N`
//! 分别对应上游的 `line_up`/`line_down`（不从补全走）。被清掉、被取走的
//! 未发送文本也进 `history`（上游 `keep` 的行为）；Enter 发送的进 `history`
//! 与 [`SubmittedPrompts`]。
//!
//! 未接的行为：补全（Tab 只停止浏览历史）、停止 agent（Esc）、粘贴图片
//! （Ctrl+V）、转写区滚动/选择（由 `pane.rs` 现有系统负责）；这些动作在
//! 表里已经能解析，`act` 里是空实现，等对应模块移植时再连。

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::{ButtonInput, ButtonState};
use bevy::prelude::*;
use rig_tui::ratatui::style::{Color as RColor, Style};
use unicode_segmentation::UnicodeSegmentation;

use crate::frame::{PanelAreas, PanelRoots};
use crate::keys::{
    self, BindingRef, Found, InputAction, KeyBinding, KeyFocus, KeyPressed, KeyScope, Modifiers,
    PendingInput,
};
use crate::panels::{KeyedRow, PanelId, PanelRows};
use crate::theme::Theme;
use crate::transcript::Row;

/// 编辑器状态。
///
/// 四个公开字段都还在；`cursor` 的语义从“字符下标”改成“字节偏移
/// （grapheme 边界）”，`history` 从“只装已发送”变成上游那样的整体历史
/// （已发送 + 被清掉的草稿）。`draft` 是浏览历史前的草稿，不公开——本
/// crate 内没有外部代码用结构体字面量构造 `Editor`，但外部 crate 若这样做
/// 会少一个字段，见移植回报。
#[derive(Resource, Default)]
pub struct Editor {
    /// 已经打出来的文本。
    pub text: String,
    /// 光标：`text` 的字节偏移，落在 grapheme 边界上。
    pub cursor: usize,
    /// 历史：Enter 发送的，加上被清掉/取走的未发送文本（上游 `keep`）。
    pub history: Vec<String>,
    /// 正在浏览历史的第几条（`None` 表示在写新内容）。
    pub history_pos: Option<usize>,
    /// 开始浏览历史前正在写的草稿；Down 翻过最新一条时带回来。
    draft: String,
}

impl Editor {
    /// 文本。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 光标：`text` 的字节偏移，落在 grapheme 边界上。
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// 是否什么都没打。
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// 替换文本，光标放到末尾，`\r\n`/`\r` 当换行。被替换的文本不保留
    /// （插件编辑文本时用；要丢弃用户文本请先调 [`Self::clear`]）。换一个
    /// 文本会停止浏览历史，浏览前的草稿进历史。
    pub fn set(&mut self, text: impl Into<String>) {
        let text = newlines(text.into());
        if text != self.text {
            self.stop_browsing();
        }
        self.show(text);
    }

    /// 把光标放到字节 `offset`，或它前面最近的 grapheme 边界。
    pub fn set_cursor(&mut self, offset: usize) {
        let starts = self.text.grapheme_indices(true).map(|(at, _)| at);
        self.cursor = starts.take_while(|at| *at <= offset).last().unwrap_or(0);
        if offset >= self.text.len() {
            self.cursor = self.text.len();
        }
    }

    /// 光标停在某个 grapheme 中间时（比如在组合字符前插入），挪到它末尾，
    /// 与上游 `settle` 一致。
    fn settle(&mut self) {
        let ends = self
            .text
            .grapheme_indices(true)
            .map(|(at, grapheme)| at + grapheme.len());
        let mut ends = std::iter::once(0).chain(ends);
        self.cursor = ends
            .find(|end| *end >= self.cursor)
            .unwrap_or(self.text.len());
    }

    /// 显示 `text`，光标放到末尾。
    fn show(&mut self, text: String) {
        self.text = text;
        self.cursor = self.text.len();
    }

    /// 清空编辑器并停止浏览历史。用户自己的文本（以及正在浏览的历史
    /// 提示词后面藏着的草稿）进历史，Up 能找回来。
    pub fn clear(&mut self) {
        let recalled = self.recalled();
        self.stop_browsing();
        let text = std::mem::take(&mut self.text);
        if !recalled {
            self.keep(text);
        }
        self.cursor = 0;
    }

    /// 把文本取走，留下空编辑器。浏览历史时，后面的草稿进历史。
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        self.stop_browsing();
        std::mem::take(&mut self.text)
    }

    /// 是否正原样显示一条历史提示词。
    pub fn recalled(&self) -> bool {
        self.history_pos
            .and_then(|at| self.history.get(at))
            .is_some_and(|entry| *entry == self.text)
    }

    /// 记录一段调用方已发送的文本，排在此前丢弃的文本之后。
    pub fn sent(&mut self, text: String) {
        self.stop_browsing();
        self.text.clear();
        self.cursor = 0;
        self.draft.clear();
        self.keep(text);
    }

    /// 停止浏览历史：正在显示的历史提示词从此算用户自己的文本，浏览前的
    /// 草稿进历史（上游 `stop_browsing`）。
    pub fn stop_browsing(&mut self) {
        if self.history_pos.take().is_some() {
            let draft = std::mem::take(&mut self.draft);
            self.keep(draft);
        }
    }

    /// 把 `text` 收进历史，除非它是空白。
    fn keep(&mut self, text: String) {
        if !text.trim().is_empty() {
            self.history.push(text);
        }
    }

    /// 在光标处打 `text`，光标落在它后面；`\r\n`/`\r` 归一成 `\n`。
    pub fn insert(&mut self, text: &str) {
        let text = newlines(text.to_owned());
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.settle();
    }

    /// 在光标处打一个字符。
    pub fn insert_char(&mut self, character: char) {
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
        self.settle();
    }

    /// 光标前一个 grapheme 的起点。
    fn previous_boundary(&self, from: usize) -> usize {
        self.text
            .get(..from)
            .and_then(|before| before.grapheme_indices(true).next_back())
            .map_or(0, |(index, _)| index)
    }

    /// 光标处 grapheme 的终点。
    fn next_boundary(&self, from: usize) -> usize {
        self.text
            .get(from..)
            .and_then(|after| after.graphemes(true).next())
            .map_or(self.text.len(), |grapheme| from + grapheme.len())
    }

    /// 后退一个 grapheme。
    pub fn left(&mut self) {
        self.cursor = self.previous_boundary(self.cursor);
    }

    /// 前进一个 grapheme。
    pub fn right(&mut self) {
        self.cursor = self.next_boundary(self.cursor);
    }

    /// 光标前一个词的开头。
    fn word_start(&self) -> usize {
        let before = self.text.get(..self.cursor).unwrap_or_default();
        let mut start = self.cursor;
        let mut in_word = false;
        for (at, grapheme) in before.grapheme_indices(true).rev() {
            match (word_grapheme(grapheme), in_word) {
                (true, _) => in_word = true,
                (false, true) => break,
                (false, false) => {}
            }
            start = at;
        }
        start
    }

    /// 光标后一个词的结尾。
    fn word_end(&self) -> usize {
        let after = self.text.get(self.cursor..).unwrap_or_default();
        let mut end = self.cursor;
        let mut in_word = false;
        for (at, grapheme) in after.grapheme_indices(true) {
            match (word_grapheme(grapheme), in_word) {
                (true, _) => in_word = true,
                (false, true) => break,
                (false, false) => {}
            }
            end = self.cursor + at + grapheme.len();
        }
        end
    }

    /// 光标前一个词的开头。
    pub fn word_left(&mut self) {
        self.cursor = self.word_start();
    }

    /// 光标后一个词的结尾。
    pub fn word_right(&mut self) {
        self.cursor = self.word_end();
    }

    /// 光标所在行的行首。
    fn line_start(&self) -> usize {
        self.text
            .get(..self.cursor)
            .and_then(|before| before.rfind('\n'))
            .map_or(0, |index| index + 1)
    }

    /// 光标所在行的行尾（不含换行）。
    fn line_end(&self) -> usize {
        self.text
            .get(self.cursor..)
            .and_then(|after| after.find('\n'))
            .map_or(self.text.len(), |index| self.cursor + index)
    }

    /// 行首。
    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    /// 行尾。
    pub fn end(&mut self) {
        self.cursor = self.line_end();
    }

    /// 删光标前一个 grapheme。
    pub fn backspace(&mut self) {
        self.remove(self.previous_boundary(self.cursor)..self.cursor);
    }

    /// 删光标后一个 grapheme。
    pub fn delete(&mut self) {
        self.remove(self.cursor..self.next_boundary(self.cursor));
    }

    /// 删光标前一个词。
    pub fn delete_word_before(&mut self) {
        self.remove(self.word_start()..self.cursor);
    }

    /// 删光标后一个词。
    pub fn delete_word_after(&mut self) {
        self.remove(self.cursor..self.word_end());
    }

    /// 删到行首；已经在行首时删掉前面的换行。
    pub fn delete_to_line_start(&mut self) {
        let start = match self.line_start() {
            start if start == self.cursor => self.previous_boundary(start),
            start => start,
        };
        self.remove(start..self.cursor);
    }

    /// 删到行尾；已经在行尾时删掉后面的换行。
    pub fn delete_to_line_end(&mut self) {
        let end = match self.line_end() {
            end if end == self.cursor => self.next_boundary(end),
            end => end,
        };
        self.remove(self.cursor..end);
    }

    /// 删掉 `range` 这些字节，光标放在删处；两侧会拼成一个 grapheme 时，
    /// 光标落在拼出来的 grapheme 后面。
    fn remove(&mut self, range: std::ops::Range<usize>) {
        self.cursor = range.start;
        self.text.replace_range(range, "");
        self.settle();
    }

    /// 光标在它那一行的第几个 grapheme。
    fn column(&self) -> usize {
        self.text
            .get(self.line_start()..self.cursor)
            .map_or(0, |line| line.graphemes(true).count())
    }

    /// 移到以 `start` 开头那一行的第 `column` 个 grapheme，没有就移到行尾。
    fn move_to_column(&mut self, start: usize, column: usize) {
        let line = self.text.get(start..).unwrap_or_default();
        let line = line.split('\n').next().unwrap_or_default();
        self.cursor = start
            + line
                .grapheme_indices(true)
                .nth(column)
                .map_or(line.len(), |(index, _)| index);
    }

    /// Up/Down 是否正在历史里走：原样显示一条历史提示词，光标在它的
    /// 开头或末尾。
    fn walking(&self) -> bool {
        self.recalled() && (self.cursor == 0 || self.cursor == self.text.len())
    }

    /// Up：在历史里走时取更早的一条，否则上一行，否则第一行行首，否则
    /// 历史最新一条（把当前文本留作草稿）。
    pub fn up(&mut self) {
        let history = self.history.clone();
        self.up_from(&history, true);
    }

    /// Ctrl+P：与 [`Self::up`] 一样，但从第一行的任何位置直接进历史。
    pub fn line_up(&mut self) {
        let history = self.history.clone();
        self.up_from(&history, false);
    }

    fn up_from(&mut self, history: &[String], to_start: bool) {
        let start = self.line_start();
        if self.walking() || (start == 0 && (self.cursor == 0 || !to_start)) {
            self.history_back(history);
            return;
        }
        if start == 0 {
            self.cursor = 0;
            return;
        }
        let column = self.column();
        let above = self
            .text
            .get(..start - 1)
            .and_then(|before| before.rfind('\n'))
            .map_or(0, |index| index + 1);
        self.move_to_column(above, column);
    }

    /// Down：在历史里走时取更晚的一条（翻过最新一条回到草稿），否则下一
    /// 行，否则最后一行行尾。
    pub fn down(&mut self) {
        let history = self.history.clone();
        self.down_with(&history);
    }

    /// Ctrl+N：与 [`Self::down`] 一样（本 crate 没有补全，没有别的分支）。
    pub fn line_down(&mut self) {
        self.down();
    }

    fn down_with(&mut self, history: &[String]) {
        if self.walking() {
            self.history_forward(history);
            return;
        }
        let end = self.line_end();
        if end == self.text.len() {
            self.cursor = end;
            return;
        }
        let column = self.column();
        self.move_to_column(end + 1, column);
    }

    fn history_back(&mut self, history: &[String]) {
        if !self.recalled() {
            self.stop_browsing();
        }
        let index = match self.history_pos {
            Some(shown) => match place(shown, &self.text, history) {
                0 => return,
                index => index - 1,
            },
            None if history.is_empty() => return,
            None => {
                self.draft = self.text.clone();
                history.len() - 1
            }
        };
        self.browse(history, index);
    }

    fn history_forward(&mut self, history: &[String]) {
        let Some(shown) = self.history_pos else {
            return;
        };
        let index = place(shown, &self.text, history) + 1;
        if index < history.len() {
            self.browse(history, index);
        } else {
            self.history_pos = None;
            let draft = std::mem::take(&mut self.draft);
            self.show(draft);
        }
    }

    /// 显示历史第 `index` 条。
    fn browse(&mut self, history: &[String], index: usize) {
        let text = history.get(index).cloned().unwrap_or_default();
        self.history_pos = Some(index);
        self.show(text);
    }
}

/// 已提交的提示词（真接大模型时改成发给 agent；模板里先记下来）。
#[derive(Resource, Default)]
pub struct SubmittedPrompts(pub Vec<String>);

/// 提交事件（其他插件可以监听）。
#[derive(Message, Debug, Clone)]
pub struct PromptSubmitted(pub String);

/// 执行一个内置动作；`Send` 时返回要提交的文本，由调用方写事件。
fn act(action: InputAction, editor: &mut Editor) -> Option<String> {
    use InputAction as Do;
    match action {
        Do::Clear => editor.clear(),
        // 本 crate 还没有退出通道：输入为空时的 Ctrl+D 先什么也不做。
        Do::Exit => {}
        Do::NewLine => editor.insert_char('\n'),
        Do::LineStart => editor.home(),
        Do::LineEnd => editor.end(),
        Do::Left => editor.left(),
        Do::Right => editor.right(),
        Do::WordLeft => editor.word_left(),
        Do::WordRight => editor.word_right(),
        Do::Backspace => editor.backspace(),
        Do::Delete => editor.delete(),
        Do::DeleteWordBefore => editor.delete_word_before(),
        Do::DeleteWordAfter => editor.delete_word_after(),
        Do::DeleteToLineStart => editor.delete_to_line_start(),
        Do::DeleteToLineEnd => editor.delete_to_line_end(),
        Do::LineUp => editor.line_up(),
        Do::LineDown => editor.line_down(),
        Do::Up => editor.up(),
        Do::Down => editor.down(),
        // 滚动/选择/内联回答由别的模块负责；这里只有动作表。
        Do::ScrollUp
        | Do::ScrollDown
        | Do::PageUp
        | Do::PageDown
        | Do::SelectUp
        | Do::SelectDown
        | Do::Deselect
        | Do::AnswerInline(_)
        | Do::DismissInline
        | Do::PasteImage => {}
        // 没有补全：Tab 把正在浏览的历史提示词变成用户自己的文本。
        Do::Tab => editor.stop_browsing(),
        Do::Send => {
            let text = editor.text.trim().to_string();
            if text.is_empty() {
                return None;
            }
            editor.sent(text.clone());
            return Some(text);
        }
        // 没有补全、也还没有停 agent 的通道。
        Do::Stop => {}
    }
    None
}

/// 补全列表开着时先吃掉这个动作；返回 `true` 表示已处理。
///
/// 对应上游 `input.rs::accept_completion` 与列表导航：↑↓ 改选中项、Tab/Enter 接受、
/// Esc 关掉（并记住这个 token，直到它变了才允许重开）。
fn completion_act(
    action: InputAction,
    state: &mut crate::complete::CompletionState,
    editor: &mut Editor,
) -> bool {
    use InputAction as Do;
    match action {
        Do::Up => {
            if let Some(completion) = state.completion_mut() {
                completion.select(-1);
            }
            true
        }
        Do::Down => {
            if let Some(completion) = state.completion_mut() {
                completion.select(1);
            }
            true
        }
        Do::Tab => {
            if let Some(completion) = state.take() {
                completion.accept(editor);
            }
            true
        }
        // 精确命中时 Enter 直接发送、列表关掉；否则接受候选（上游同义）。
        Do::Send => {
            let exact = state.completion().is_some_and(|c| c.is_exact());
            if exact {
                state.close();
                return false;
            }
            if let Some(completion) = state.take() {
                completion.accept(editor);
            }
            true
        }
        Do::Stop => {
            state.dismiss();
            true
        }
        _ => false,
    }
}

/// 输入系统的键处理：走 [`crate::keys`] 的绑定表；编辑动作在这里执行，
/// 插件绑定（没有 [`InputAction`] 的）触发 [`KeyPressed`]。
pub fn keys(
    mut input: MessageReader<KeyboardInput>,
    mut editor: ResMut<Editor>,
    mut prompts: ResMut<SubmittedPrompts>,
    mut submitted: MessageWriter<PromptSubmitted>,
    held: Res<ButtonInput<KeyCode>>,
    bindings: Query<(Entity, &KeyBinding, Option<&InputAction>, Has<Disabled>), Allow<Disabled>>,
    mut focus: ResMut<KeyFocus>,
    mut pending: ResMut<PendingInput>,
    mut commands: Commands,
    mut completion: Option<ResMut<crate::complete::CompletionState>>,
) {
    let rows: Vec<BindingRef> = bindings
        .iter()
        .filter(|(.., disabled)| !*disabled)
        .map(|(entity, binding, action, _)| BindingRef {
            entity,
            binding,
            action: action.copied(),
        })
        .collect();
    let mut modifiers = Modifiers::from_held(&held);
    for event in input.read() {
        // 修饰键自己更新状态；同一帧里先按修饰键的事件也因此正确。
        if Modifiers::is_modifier(event.key_code) {
            match event.state {
                ButtonState::Pressed => {
                    modifiers.insert(modifier_of(event.key_code));
                }
                ButtonState::Released => {
                    modifiers.remove(modifier_of(event.key_code));
                }
            }
            continue;
        }
        let scope = match focus.0 {
            Some(entity) => KeyScope::Panel(entity),
            None => KeyScope::Input,
        };
        let press = keys::press_with(&event, modifiers);
        if event.state != ButtonState::Pressed {
            if let Some(entity) = keys::released(&rows, &press, scope) {
                commands.trigger(KeyPressed {
                    entity,
                    agent: None,
                    released: true,
                });
            }
            continue;
        }
        pending.0.push(press.clone());
        match keys::find(&rows, &pending.0, scope) {
            Found::Partial => continue,
            Found::Binding(entity) => {
                pending.0.clear();
                let Some(row) = rows.iter().find(|row| row.entity == entity) else {
                    continue;
                };
                // 请求松开的绑定按住不放时只算一次（终端会上报 repeat）。
                if row.binding.releases && event.repeat {
                    continue;
                }
                match row.action {
                    Some(action) => {
                        // 补全列表开着时，↑↓/Tab/Enter/Esc 先归列表（上游 accept_completion）。
                        if let Some(state) = completion.as_mut() {
                            if completion_act(action, state, &mut editor) {
                                continue;
                            }
                        }
                        if let Some(text) = act(action, &mut editor) {
                            prompts.0.push(text.clone());
                            submitted.write(PromptSubmitted(text));
                        }
                    }
                    None => commands.trigger(KeyPressed {
                        entity,
                        agent: None,
                        released: false,
                    }),
                }
            }
            Found::Nothing => {
                let sequence = pending.0.len() > 1;
                pending.0.clear();
                // 多键前缀走空：整串吃掉，不把字符打进输入。
                if sequence {
                    continue;
                }
                // 面板作用域下 Esc 没被绑：把键交回输入框。
                if matches!(scope, KeyScope::Panel(_)) && press.key == Key::Escape {
                    focus.0 = None;
                    continue;
                }
                // 本 crate 只有主编辑器；面板拿键时按键不该落进主输入框
                // （上游会敲进拿到焦点的那个面板的编辑器）。
                if matches!(scope, KeyScope::Panel(_)) {
                    continue;
                }
                if press
                    .modifiers
                    .intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER)
                {
                    continue;
                }
                // 用事件带的文本（死键组合后的完整文本），不要控制字符。
                let text: String = event
                    .text
                    .as_deref()
                    .unwrap_or_default()
                    .chars()
                    .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
                    .collect();
                if !text.is_empty() {
                    editor.insert(&text);
                }
            }
        }
    }
}

/// `key_code` 对应的修饰键。
fn modifier_of(code: KeyCode) -> Modifiers {
    match code {
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Modifiers::SHIFT,
        KeyCode::ControlLeft | KeyCode::ControlRight => Modifiers::CONTROL,
        KeyCode::AltLeft | KeyCode::AltRight => Modifiers::ALT,
        KeyCode::SuperLeft | KeyCode::SuperRight => Modifiers::SUPER,
        _ => Modifiers::NONE,
    }
}

/// 输入面板的两行：提示行（带光标）+ 键位提示。
///
/// 键位提示的宽度取输入面板这一帧的文字列数（rig-tui 用面板宽度做同一件事）。
pub fn editor_rows(
    mut panels: ResMut<PanelRows>,
    editor: Res<Editor>,
    theme: Res<Theme>,
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
) {
    let cols = roots
        .entity(PanelId::Input)
        .and_then(|root| areas.get(root))
        .map_or(97, |area| usize::from(area.cols.max(1)));
    // 光标是字节偏移；外部直接写 pub 字段时也保证切片落在字符边界上。
    let mut cursor = editor.cursor.min(editor.text.len());
    while cursor > 0 && !editor.text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let before = editor.text.get(..cursor).unwrap_or_default().to_owned();
    let after = editor.text.get(cursor..).unwrap_or_default().to_owned();
    let rows = vec![
        KeyedRow {
            key: 1,
            hash: 1,
            row: Row::new(rig_tui::ratatui::text::Line::from(vec![
                ratatui_span("› ", theme.prompt),
                ratatui_span(before, theme.fg),
                ratatui_span("▏", Style::default().fg(RColor::LightCyan)),
                ratatui_span(after, theme.fg),
            ])),
        },
        KeyedRow {
            key: 2,
            hash: 2,
            row: Row::new(rig_tui::ratatui::text::Line::from(vec![ratatui_span(
                crate::hints::current_hints(cols, false),
                theme.dim,
            )])),
        },
    ];
    panels.set(PanelId::Input, rows);
}

pub fn ratatui_span(text: impl Into<String>, style: Style) -> rig_tui::ratatui::text::Span<'static> {
    rig_tui::ratatui::text::Span::styled(text.into(), style)
}

pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Editor>()
            .init_resource::<SubmittedPrompts>()
            .init_resource::<KeyFocus>()
            .init_resource::<PendingInput>()
            .add_message::<PromptSubmitted>();
        // 43 条输入键 + 转写区 Esc + 先禁用的 Alt+1..9/Alt+0。
        crate::keys::add_built_in(app);
        app.add_systems(
            Update,
            keys.in_set(crate::FeathersSystems::Prepare)
                .run_if(crate::picker::no_picker),
        )
        .add_systems(
            Update,
            editor_rows.in_set(crate::FeathersSystems::Draw),
        );
    }
}

/// `text` 里的 `\r\n` 与单独的 `\r` 都归一成 `\n`。
fn newlines(text: String) -> String {
    match text.contains('\r') {
        true => text.replace("\r\n", "\n").replace('\r', "\n"),
        false => text,
    }
}

/// `grapheme` 是否属于一个词：首字符是字母、数字或 `_`。
fn word_grapheme(grapheme: &str) -> bool {
    grapheme
        .chars()
        .next()
        .is_some_and(|first| first.is_alphanumeric() || first == '_')
}

/// 历史条目 `shown` 现在在哪个位置：还在原下标就是它，否则按文本找，找不到
/// 就当它被挤到最新一条之后。
fn place(index: usize, shown: &str, history: &[String]) -> usize {
    if history.get(index).is_some_and(|entry| entry == shown) {
        return index;
    }
    history
        .iter()
        .rposition(|entry| entry == shown)
        .unwrap_or(history.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{AppKeyBindingsExt, KeyPress};

    fn ed(text: &str) -> Editor {
        let mut editor = Editor::default();
        editor.set(text);
        editor
    }

    fn history_editor(draft: &str, history: &[&str]) -> Editor {
        let mut editor = ed(draft);
        editor.history = history.iter().map(|entry| (*entry).to_owned()).collect();
        editor
    }

    // ---- grapheme：中文 / emoji / 组合字符 ----

    #[test]
    fn chinese_moves_and_deletes_by_grapheme() {
        let mut editor = ed("你好世界");
        assert_eq!(editor.cursor(), 12, "4 个汉字 = 12 字节");
        editor.left();
        assert_eq!(editor.cursor(), 9);
        editor.left();
        assert_eq!(editor.cursor(), 6);
        editor.right();
        assert_eq!(editor.cursor(), 9);
        editor.right();
        assert_eq!(editor.cursor(), 12);
        editor.backspace();
        assert_eq!(editor.text(), "你好世");
        assert_eq!(editor.cursor(), 9);
        editor.backspace();
        assert_eq!(editor.text(), "你好");
        assert_eq!(editor.cursor(), 6);
        editor.set_cursor(3);
        editor.insert_char('x');
        assert_eq!(editor.text(), "你x好");
        assert_eq!(editor.cursor(), 4);
        editor.set_cursor(1);
        assert_eq!(editor.cursor(), 0, "光标吸附到前一个 grapheme 边界");
    }

    #[test]
    fn emoji_zwj_sequences_are_one_grapheme() {
        // "a" + 👩‍💻（👩 4 字节 + ZWJ 3 字节 + 💻 4 字节）+ "b"
        let text = "a👩\u{200d}💻b";
        assert_eq!(text.len(), 13);
        let mut editor = ed(text);
        assert_eq!(editor.cursor(), 13);
        editor.left();
        assert_eq!(editor.cursor(), 12);
        editor.left();
        assert_eq!(editor.cursor(), 1);
        editor.left();
        assert_eq!(editor.cursor(), 0);
        editor.right();
        assert_eq!(editor.cursor(), 1);
        assert_eq!(editor.text().get(1..12), Some("👩\u{200d}💻"));
        editor.right();
        assert_eq!(editor.cursor(), 12);
        editor.backspace();
        assert_eq!(editor.text(), "ab");
        assert_eq!(editor.cursor(), 1);
        editor.delete();
        assert_eq!(editor.text(), "a");
        editor.set_cursor(2);
        assert_eq!(editor.cursor(), 1, "👩‍💻 中间吸附到它前面");
    }

    #[test]
    fn combining_marks_are_one_grapheme() {
        // e + U+0301（组合尖音）：一个 grapheme，3 字节
        let mut editor = ed("e\u{301}x");
        assert_eq!(editor.cursor(), 4);
        editor.left();
        assert_eq!(editor.cursor(), 3);
        editor.left();
        assert_eq!(editor.cursor(), 0);
        editor.set_cursor(3);
        editor.backspace();
        assert_eq!(editor.text(), "x");
        assert_eq!(editor.cursor(), 0);
        let mut editor = ed("e\u{301}");
        editor.home();
        editor.insert("a");
        assert_eq!(editor.text(), "ae\u{301}");
        assert_eq!(editor.cursor(), 1);
    }

    #[test]
    fn words_move_over_chinese_and_delete_by_grapheme() {
        let mut editor = ed("hello 世界");
        editor.word_left();
        assert_eq!(editor.cursor(), 6);
        editor.word_left();
        assert_eq!(editor.cursor(), 0);
        editor.word_right();
        assert_eq!(editor.cursor(), 5);
        editor.word_right();
        assert_eq!(editor.cursor(), 12);
        editor.delete_word_before();
        assert_eq!(editor.text(), "hello ");
        assert_eq!(editor.cursor(), 6);
        editor.delete_word_before();
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor(), 0);
        let mut editor = ed("hello 世界");
        editor.set_cursor(0);
        editor.delete_word_after();
        assert_eq!(editor.text(), " 世界");
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn line_deletes_use_grapheme_boundaries() {
        let mut editor = ed("abc\ndef");
        editor.set_cursor(5);
        editor.delete_to_line_start();
        assert_eq!(editor.text(), "abc\nef");
        assert_eq!(editor.cursor(), 4);
        editor.set_cursor(4);
        editor.delete_to_line_start();
        assert_eq!(editor.text(), "abcef");
        assert_eq!(editor.cursor(), 3);

        let mut editor = ed("abc\ndef");
        editor.set_cursor(1);
        editor.delete_to_line_end();
        assert_eq!(editor.text(), "a\ndef");
        assert_eq!(editor.cursor(), 1);
        editor.set_cursor(1);
        editor.delete_to_line_end();
        assert_eq!(editor.text(), "adef");
        assert_eq!(editor.cursor(), 1);
    }

    // ---- 历史：空 / 单条 / 多条 / 跨行 ----

    #[test]
    fn up_and_down_on_an_empty_history_only_move_the_cursor_to_the_line_start() {
        let mut editor = ed("draft");
        editor.up();
        assert_eq!(editor.text(), "draft");
        assert_eq!(editor.cursor(), 0, "Up 先到第一行行首");
        assert_eq!(editor.history_pos, None);
        editor.up();
        assert_eq!(editor.text(), "draft");
        assert_eq!(editor.cursor(), 0, "空历史时停在行首");
        assert_eq!(editor.history_pos, None);
        editor.down();
        assert_eq!(editor.text(), "draft");
        assert_eq!(editor.cursor(), 5, "Down 到最后一行行尾");
        assert_eq!(editor.history_pos, None);
    }

    #[test]
    fn one_history_entry_round_trips_to_the_draft() {
        let mut editor = history_editor("draft", &["one"]);
        editor.up();
        assert_eq!((editor.text(), editor.cursor()), ("draft", 0));
        editor.up();
        assert_eq!(editor.text(), "one");
        assert_eq!(editor.cursor(), 3);
        assert_eq!(editor.history_pos, Some(0));
        assert_eq!(editor.recalled(), true);
        editor.up();
        assert_eq!(editor.text(), "one", "最新到最早一条后不再动");
        assert_eq!(editor.history_pos, Some(0));
        editor.down();
        assert_eq!(editor.text(), "draft", "翻过最新一条回到草稿");
        assert_eq!(editor.cursor(), 5);
        assert_eq!(editor.history_pos, None);
        editor.down();
        assert_eq!(editor.text(), "draft", "草稿后面没有了");
        assert_eq!(editor.cursor(), 5);
    }

    #[test]
    fn many_history_entries_walk_both_ways() {
        let mut editor = history_editor("draft", &["one", "two", "three"]);
        editor.up();
        editor.up();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("three".to_owned(), Some(2)));
        editor.up();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("two".to_owned(), Some(1)));
        editor.up();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("one".to_owned(), Some(0)));
        editor.up();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("one".to_owned(), Some(0)));
        editor.down();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("two".to_owned(), Some(1)));
        editor.down();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("three".to_owned(), Some(2)));
        editor.down();
        assert_eq!((editor.text().to_owned(), editor.history_pos), ("draft".to_owned(), None));
        assert_eq!(editor.cursor(), 5);
    }

    #[test]
    fn ctrl_p_enters_the_history_from_the_middle_of_the_first_line() {
        let mut editor = history_editor("draft", &["one", "two"]);
        editor.set_cursor(2);
        editor.line_up();
        assert_eq!(editor.text(), "two", "Ctrl+P 不走行内上移");
        assert_eq!(editor.history_pos, Some(1));
        editor.line_down();
        assert_eq!(editor.text(), "draft");
        assert_eq!(editor.history_pos, None);
    }

    #[test]
    fn up_and_down_move_between_lines_before_the_history() {
        let mut editor = ed("ab\ncd");
        assert_eq!(editor.cursor(), 5);
        editor.up();
        assert_eq!(editor.cursor(), 2, "上一行同列");
        editor.up();
        assert_eq!(editor.cursor(), 0, "第一行行首");
        editor.down();
        assert_eq!(editor.cursor(), 3, "下一行同列");
        editor.down();
        assert_eq!(editor.cursor(), 5, "最后一行行尾");
        let mut editor = history_editor("ab\ncd", &["old"]);
        editor.up();
        editor.up();
        editor.up();
        assert_eq!(editor.text(), "old");
        assert_eq!(editor.history_pos, Some(0));
    }

    #[test]
    fn editing_a_recalled_prompt_keeps_it_out_of_the_walking_path() {
        let mut editor = history_editor("draft", &["one"]);
        editor.up();
        editor.up();
        assert_eq!(editor.text(), "one");
        editor.insert("!");
        assert_eq!(editor.text(), "one!");
        assert_eq!(editor.recalled(), false);
        editor.down();
        assert_eq!(editor.text(), "one!");
        assert_eq!(editor.cursor(), 4, "改过之后 Down 只走行尾");
    }

    #[test]
    fn clear_keeps_the_unsent_text_for_up() {
        let mut editor = ed("keep me");
        editor.clear();
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor(), 0);
        assert_eq!(editor.history, vec!["keep me".to_owned()]);
        editor.up();
        assert_eq!(editor.text(), "keep me");
        assert_eq!(editor.history_pos, Some(0));
    }

    #[test]
    fn enter_sends_the_trimmed_text_and_clears_the_editor() {
        let mut editor = ed("  hello  ");
        assert_eq!(act(InputAction::Send, &mut editor), Some("hello".to_owned()));
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor(), 0);
        assert_eq!(editor.history, vec!["hello".to_owned()]);
        let mut empty = ed("   ");
        assert_eq!(act(InputAction::Send, &mut empty), None);
        assert_eq!(empty.text(), "   ", "空白不发送，也不清空");
    }

    // ---- keys 系统：动作表真的接上了 ----

    fn test_app() -> App {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .add_message::<PromptSubmitted>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Editor>()
            .init_resource::<SubmittedPrompts>()
            .init_resource::<KeyFocus>()
            .init_resource::<PendingInput>();
        crate::keys::add_built_in(&mut app);
        app.add_systems(Update, keys);
        app
    }

    fn send(app: &mut App, key: Key, code: KeyCode, text: Option<&str>) {
        app.world_mut().write_message(KeyboardInput {
            key_code: code,
            logical_key: key,
            state: ButtonState::Pressed,
            text: text.map(|text| text.into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    fn character(app: &mut App, character: char) {
        let code = KeyCode::KeyA; // 测试里只关心字符与文本，不关心物理键
        send(
            app,
            Key::Character(character.to_string().into()),
            code,
            Some(&character.to_string()),
        );
    }

    fn with_modifier(app: &mut App, modifier: KeyCode, key: Key, code: KeyCode) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(modifier);
        send(app, key, code, None);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(modifier);
    }

    fn text(app: &App) -> String {
        app.world().resource::<Editor>().text.clone()
    }

    #[test]
    fn the_built_in_table_drives_typing_and_editing() {
        let mut app = test_app();
        for character in ['a', 'b', 'c'] {
            self::character(&mut app, character);
        }
        assert_eq!(text(&app), "abc");
        assert_eq!(app.world().resource::<Editor>().cursor(), 3);

        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("a".into()), KeyCode::KeyA);
        assert_eq!(app.world().resource::<Editor>().cursor(), 0, "Ctrl+A 行首");
        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("e".into()), KeyCode::KeyE);
        assert_eq!(app.world().resource::<Editor>().cursor(), 3, "Ctrl+E 行尾");

        app.world_mut().resource_mut::<Editor>().set("hello 世界");
        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("w".into()), KeyCode::KeyW);
        assert_eq!(text(&app), "hello ", "Ctrl+W 按 grapheme 删掉「世界」");

        // Ctrl+U / Ctrl+K 的真实删除结果。
        app.world_mut().resource_mut::<Editor>().set("abc def");
        app.world_mut().resource_mut::<Editor>().set_cursor(3);
        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("k".into()), KeyCode::KeyK);
        assert_eq!(text(&app), "abc", "Ctrl+K 删到行尾");
        with_modifier(&mut app, KeyCode::ControlLeft, Key::Character("u".into()), KeyCode::KeyU);
        assert_eq!(text(&app), "", "Ctrl+U 删到行首");

        app.world_mut().resource_mut::<Editor>().set("one two");
        with_modifier(&mut app, KeyCode::AltLeft, Key::Character("b".into()), KeyCode::KeyB);
        assert_eq!(app.world().resource::<Editor>().cursor(), 4, "Alt+B 词首");
        with_modifier(&mut app, KeyCode::AltLeft, Key::Character("f".into()), KeyCode::KeyF);
        assert_eq!(app.world().resource::<Editor>().cursor(), 7, "Alt+F 词尾");
        app.world_mut().resource_mut::<Editor>().set("one two");
        app.world_mut().resource_mut::<Editor>().set_cursor(0);
        with_modifier(&mut app, KeyCode::AltLeft, Key::Character("d".into()), KeyCode::KeyD);
        assert_eq!(text(&app), " two", "Alt+D 删后一个词");
    }

    #[test]
    fn shift_enter_inserts_a_newline_and_enter_sends() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ShiftLeft);
        send(&mut app, Key::Enter, KeyCode::Enter, None);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ShiftLeft);
        assert_eq!(text(&app), "\n");
        self::character(&mut app, 'x');
        assert_eq!(text(&app), "\nx");
        send(&mut app, Key::Enter, KeyCode::Enter, None);
        assert_eq!(text(&app), "");
        let submitted = app.world().resource::<SubmittedPrompts>();
        assert_eq!(submitted.0, vec!["x".to_owned()], "发送时去掉首尾空白（沿用旧行为）");
        let editor = app.world().resource::<Editor>();
        assert_eq!(editor.history, vec!["x".to_owned()]);
    }

    #[test]
    fn esc_and_up_down_are_wired_through_the_table() {
        let mut app = test_app();
        self::character(&mut app, 'a');
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Escape,
            logical_key: Key::Escape,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert_eq!(text(&app), "a", "Esc 是 Stop，不是字符");

        app.world_mut().resource_mut::<Editor>().history =
            vec!["one".to_owned(), "two".to_owned()];
        send(&mut app, Key::ArrowUp, KeyCode::ArrowUp, None);
        assert_eq!(text(&app), "a", "先到第一行行首");
        send(&mut app, Key::ArrowUp, KeyCode::ArrowUp, None);
        assert_eq!(text(&app), "two");
        send(&mut app, Key::ArrowDown, KeyCode::ArrowDown, None);
        assert_eq!(text(&app), "a", "Down 回到草稿");
    }

    #[test]
    fn a_plugins_binding_receives_key_pressed() {
        let mut app = test_app();
        app.add_key_binding(
            KeyBinding::new(KeyPress::alt('t'), "Template").in_scope(KeyScope::Global),
            |pressed: On<KeyPressed>, mut editor: ResMut<Editor>| {
                if !pressed.released {
                    editor.insert("T");
                }
            },
        );
        let event = KeyboardInput {
            key_code: KeyCode::KeyT,
            logical_key: Key::Character("t".into()),
            state: ButtonState::Pressed,
            text: Some("t".into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        };
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::AltLeft);
        app.world_mut().write_message(event);
        app.update();
        assert_eq!(text(&app), "T", "全局插件绑定遮住打字");
        assert!(app.world().resource::<SubmittedPrompts>().0.is_empty());
    }
}

#[cfg(test)]
mod completion_key_tests {
    use super::*;
    use crate::complete::{Candidate, Completion, CompletionKind, CompletionState};

    fn open_list(state: &mut CompletionState) {
        state.show(Completion::from_items(
            CompletionKind::Marker('#'),
            0,
            "",
            vec![
                Candidate {
                    text: "142".into(),
                    detail: "Crash on empty input".into(),
                },
                Candidate {
                    text: "143".into(),
                    detail: "Slow start".into(),
                },
            ],
        ));
    }

    #[test]
    fn down_moves_the_selection_and_does_not_touch_the_editor() {
        let mut state = CompletionState::default();
        open_list(&mut state);
        let mut editor = Editor::default();
        editor.insert("hello");
        assert!(completion_act(InputAction::Down, &mut state, &mut editor));
        assert_eq!(state.completion().unwrap().selected, 1);
        assert_eq!(editor.text, "hello", "↑↓ 只改选中项，不动输入");
    }

    #[test]
    fn tab_accepts_the_candidate() {
        let mut state = CompletionState::default();
        open_list(&mut state);
        let mut editor = Editor::default();
        assert!(completion_act(InputAction::Tab, &mut state, &mut editor));
        assert!(!state.is_open(), "接受后列表关掉");
        assert!(editor.text.contains("142"), "候选落进输入：{}", editor.text);
    }

    #[test]
    fn escape_dismisses_and_keeps_the_token_from_reopening() {
        let mut state = CompletionState::default();
        open_list(&mut state);
        let mut editor = Editor::default();
        assert!(completion_act(InputAction::Stop, &mut state, &mut editor));
        assert!(!state.is_open());
        assert_eq!(editor.text, "", "Esc 不改输入");
    }

    #[test]
    fn enter_on_an_inexact_list_accepts_instead_of_sending() {
        let mut state = CompletionState::default();
        open_list(&mut state);
        let mut editor = Editor::default();
        // 返回 true = 已吃掉这个动作，调用方不会再去 `act` 里发送。
        assert!(completion_act(InputAction::Send, &mut state, &mut editor));
        assert!(!state.is_open());
    }

    #[test]
    fn keys_outside_the_list_fall_through() {
        let mut state = CompletionState::default();
        let mut editor = Editor::default();
        assert!(!completion_act(InputAction::Left, &mut state, &mut editor));
    }
}
