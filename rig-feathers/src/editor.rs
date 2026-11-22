//! 输入编辑器：多行文本框 + 光标 + 历史，键位照 rig-tui 的习惯。
//!
//! rig-tui 的 `editor.rs` 是 770 行的完整编辑器（emacs 键位、补全、粘贴）；
//! 模板里保留同样的**状态形状**（文本 + 光标 + 历史 + 提交），键位收窄到
//! 打字/退格/左右/上下历史/Enter 发送，剩下的按需加。

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use rig_tui::ratatui::style::{Color as RColor, Style};

use crate::frame::{PanelAreas, PanelRoots};
use crate::panels::{KeyedRow, PanelId, PanelRows};
use crate::theme::Theme;
use crate::transcript::Row;

/// 编辑器状态。
#[derive(Resource, Default)]
pub struct Editor {
    /// 已经打出来的文本。
    pub text: String,
    /// 光标（字符下标，不是字节）。
    pub cursor: usize,
    /// 已发送的历史。
    pub history: Vec<String>,
    /// 正在浏览历史的第几条（`None` 表示在写新内容）。
    pub history_pos: Option<usize>,
}

impl Editor {
    fn insert(&mut self, ch: char) {
        let byte = self.byte_at(self.cursor);
        self.text.insert(byte, ch);
        self.cursor += 1;
    }
    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let byte = self.byte_at(self.cursor - 1);
        let next = self.byte_at(self.cursor);
        self.text.replace_range(byte..next, "");
        self.cursor -= 1;
    }
    fn byte_at(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map(|(byte, _)| byte)
            .unwrap_or(self.text.len())
    }
}

/// 已提交的提示词（真接大模型时改成发给 agent；模板里先记下来）。
#[derive(Resource, Default)]
pub struct SubmittedPrompts(pub Vec<String>);

/// 提交事件（其他插件可以监听）。
#[derive(Message, Debug, Clone)]
pub struct PromptSubmitted(pub String);

pub fn keys(
    mut input: MessageReader<KeyboardInput>,
    mut editor: ResMut<Editor>,
    mut prompts: ResMut<SubmittedPrompts>,
    mut submitted: MessageWriter<PromptSubmitted>,
) {
    for event in input.read() {
        if event.state != bevy::input::ButtonState::Pressed {
            continue;
        }
        match &event.logical_key {
            Key::Character(chars) => {
                for ch in chars.chars() {
                    editor.insert(ch);
                }
                editor.history_pos = None;
            }
            Key::Space => {
                editor.insert(' ');
                editor.history_pos = None;
            }
            Key::Backspace => editor.backspace(),
            Key::ArrowLeft => editor.cursor = editor.cursor.saturating_sub(1),
            Key::ArrowRight => editor.cursor = (editor.cursor + 1).min(editor.text.chars().count()),
            Key::ArrowUp => {
                if editor.history.is_empty() {
                    continue;
                }
                let pos = match editor.history_pos {
                    Some(index) => index.saturating_sub(1),
                    None => editor.history.len() - 1,
                };
                editor.history_pos = Some(pos);
                editor.text = editor.history[pos].clone();
                editor.cursor = editor.text.chars().count();
            }
            Key::ArrowDown => {
                let Some(pos) = editor.history_pos else { continue };
                if pos + 1 >= editor.history.len() {
                    editor.history_pos = None;
                    editor.text.clear();
                    editor.cursor = 0;
                } else {
                    editor.history_pos = Some(pos + 1);
                    editor.text = editor.history[pos + 1].clone();
                    editor.cursor = editor.text.chars().count();
                }
            }
            Key::Enter => {
                let text = editor.text.trim().to_string();
                if text.is_empty() {
                    continue;
                }
                editor.history.push(text.clone());
                editor.text.clear();
                editor.cursor = 0;
                editor.history_pos = None;
                prompts.0.push(text.clone());
                submitted.write(PromptSubmitted(text));
            }
            _ => {}
        }
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
    let before: String = editor.text.chars().take(editor.cursor).collect();
    let after: String = editor.text.chars().skip(editor.cursor).collect();
    let rows = vec![
        KeyedRow {
            key: 1,
            hash: 1,
            row: Row::new(rig_tui::ratatui::text::Line::from(vec![
                ratatui_span("› ", theme.prompt),
                ratatui_span(before, theme.fg),
                ratatui_span(
                    "▏",
                    Style::default().fg(RColor::LightCyan),
                ),
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
            .add_message::<PromptSubmitted>()
            // picker 打开时方向键归它（rig-tui 用键位作用域做同一件事）。
            .add_systems(
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
