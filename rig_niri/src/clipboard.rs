//! 剪贴板：复制选区、Ctrl+V 粘贴文本。
//!
//! 上游 rig-tui 的 `clipboard.rs` 有 340 行，其中一大半是终端专用通路
//! （OSC 52、`wl-paste`/`xclip`/`pngpaste` 外部程序、任务池、粘贴图片存盘）。
//! 这里只保留**语义**：
//!
//! - 复制：把 [`Selection`] 选中的文本写进系统剪贴板，并给出
//!   `Copied N lines`（上游原文，`1` 行时是 `Copied 1 line`）。
//! - 粘贴：读剪贴板的文本，插到编辑器光标处；空剪贴板/读取失败给一句
//!   可显示的错误，不 panic。
//! - 粘贴图片：上游是“把剪贴板图片存成文件，再在输入里打 `@path`”。
//!   `bevy_clipboard` 的 `image` feature 没有开，也没有临时文件存放约定，
//!   所以这里保留 [`Paste::Image`] 与 `@path` 的**应用语义**（
//!   [`apply_paste`] / [`type_path`]），但没有从剪贴板取图片的入口。
//!
//! 真正的系统剪贴板由 `bevy_clipboard::Clipboard` 提供；为了让单测不碰
//! 系统剪贴板，所有逻辑都走 [`ClipboardText`] trait，测试用内存假实现注入。
//!
//! ## 接线
//!
//! `Clipboard` 是 `bevy_clipboard` 的资源，需要先把它的插件加进 App：
//!
//! ```ignore
//! app.add_plugins(bevy_clipboard::ClipboardPlugin)
//!     .init_resource::<selection::Selection>()   // 选区的资源
//!     .add_message::<clipboard::Copied>()
//!     .add_systems(Update, clipboard::show_copied.in_set(FeathersSystems::Prepare));
//! ```
//!
//! 鼠标松开时：
//!
//! ```ignore
//! if let Some(text) = selection.release(&rows) {
//!     match clipboard::copy_selection(&mut selection, &rows, &mut clipboard) {
//!         Ok(copied) => messages.write(copied),
//!         Err(why) => messages.write(ClipboardFailed(why)), // 调用方自己的消息
//!     }
//! }
//! ```
//!
//! Ctrl+V 时：`editor::keys` 把 `InputAction::PasteImage` 交到
//! [`paste_from`]（优先内存替身、其次系统剪贴板），文本插到光标处；失败写
//! [`PASTE_FAILED`] 状态行。**只粘文本**：图片分支没有入口，原因见上。

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy_clipboard::{Clipboard, ClipboardError as BevyClipboardError};
use rig_tui::{AppStatus, Side, StatusItem, Tone};

use crate::editor::Editor;
use crate::selection::Selection;
use crate::transcript::Row;

/// 没有选区/选区全是空白时复制失败的说法。
pub const NOTHING_SELECTED: &str = "Nothing is selected.";

/// 剪贴板里没有文本时的说法（上游 `type_pasted` 的同一类提示）。
pub const NO_CLIPBOARD_TEXT: &str = "The clipboard holds no text.";

/// 这台机器上没有可用的剪贴板后端（比如没有 X11/Wayland 的 SSH）。
pub const NO_CLIPBOARD: &str = "No clipboard is reachable from this machine.";

/// 复制成功了几行；`MessageWriter<Copied>` 把它交给状态行。
///
/// 文案照上游：`Copied 1 line` / `Copied N lines`。
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Copied {
    /// 复制的行数（`text.lines().count()`，末尾没有多余换行）。
    pub lines: usize,
}

impl Copied {
    /// 要显示的提示文本。
    pub fn message(&self) -> String {
        copied_message(self.lines)
    }
}

/// `Copied N lines` 的文案，`1` 行时按英文单数说 `Copied 1 line`。
///
/// 与上游 `show_copied` 逐字一致。
pub fn copied_message(lines: usize) -> String {
    match lines {
        1 => "Copied 1 line".to_owned(),
        lines => format!("Copied {lines} lines"),
    }
}

/// 状态行里“复制了几行”的条目（名字、边、顺序、keep 都与上游一致）。
pub const COPIED: StatusItem = StatusItem::new("copied", Side::Left, 95, 20);

/// 复制失败时状态行的条目。
///
/// 上游没有对应项（终端里复制失败多半只能吞掉）；这里把错误当成一条红色
/// 状态显示出来，让“没有系统剪贴板”这类问题能看见。
pub const COPY_FAILED: StatusItem = StatusItem::new("copy", Side::Left, 94, 10);

/// 把复制失败的原因写进状态行（红色）。
pub fn show_copy_failed(status: &mut AppStatus, why: &str) {
    status.0.show(COPY_FAILED.says(why, Tone::Red));
}

/// 复制成功后清掉上一次的失败提示。
pub fn clear_copy_failed(status: &mut AppStatus) {
    status.0.hide(COPY_FAILED.name);
}

/// 粘贴失败时状态行的条目。
///
/// 和 [`COPY_FAILED`] 同形但名字分开：复制失败和粘贴失败各自清各自的，
/// 一次成功的粘贴不会顺手清掉复制错误的提示。
pub const PASTE_FAILED: StatusItem = StatusItem::new("paste", Side::Left, 94, 10);

/// 把粘贴失败的原因写进状态行（红色），与 [`Copied`] 的成功提示同一套机制。
pub fn show_paste_failed(status: &mut AppStatus, why: &str) {
    status.0.show(PASTE_FAILED.says(why, Tone::Red));
}

/// 粘贴成功后清掉上一次的失败提示。
pub fn clear_paste_failed(status: &mut AppStatus) {
    status.0.hide(PASTE_FAILED.name);
}

/// 把 `Selection::copied` 变成状态行的 `Copied N lines`。
///
/// 上游 `show_copied` 在 `StatusSystems::Show` 里跑；这里由接线方放进
/// `FeathersSystems::Draw` 之前。没有 `Selection`/`AppStatus` 资源时什么
/// 也不做（方便单独使用本模块）。
pub fn show_copied(selection: Option<Res<Selection>>, status: Option<ResMut<AppStatus>>) {
    let (Some(selection), Some(mut status)) = (selection, status) else {
        return;
    };
    let text = selection.copied().map(copied_message).unwrap_or_default();
    let changed = status
        .0
        .iter()
        .find(|item| item.name == COPIED.name)
        .map(|item| item.text.as_str())
        .unwrap_or_default()
        != text;
    if changed {
        status.0.show(COPIED.says(text, Tone::Green));
    }
}

/// 可注入的剪贴板文本后端。
///
/// 真实现是 [`Clipboard`]；单测用内存假实现，因此不需要窗口或系统剪贴板。
/// 错误已经是给人看的句子，直接显示即可。
pub trait ClipboardText {
    /// 读剪贴板文本；空剪贴板给空串（由 [`read_paste`] 判成错误）。
    fn read_text(&mut self) -> Result<String, String>;

    /// 写剪贴板文本。
    fn write_text(&mut self, text: &str) -> Result<(), String>;
}

/// `bevy_clipboard::Clipboard` 的文本实现。
impl ClipboardText for Clipboard {
    fn read_text(&mut self) -> Result<String, String> {
        let mut read = self.fetch_text();
        match read.poll_result() {
            Some(Ok(text)) => Ok(text),
            Some(Err(error)) => Err(read_error(&error)),
            // wasm 上读取是异步的：这一帧还没有结果。
            None => Err("The clipboard is still being read; try again.".to_owned()),
        }
    }

    fn write_text(&mut self, text: &str) -> Result<(), String> {
        Clipboard::set_text(self, text).map_err(|error| write_error(&error))
    }
}

/// 内存剪贴板：单测用的可观察替身（不碰系统剪贴板）。
///
/// 生产 App 不会插入它；[`crate::selection::SelectionPlugin`] 的松手复制
/// 只在它存在时优先写它，测试因此能断言“松手复制了什么”，而不依赖窗口
/// 或桌面剪贴板。它也是一个合法的后备：真机上没有系统剪贴板时，插它
/// 至少能在进程内复制。
#[derive(Resource, Default, Debug)]
pub struct MemoryClipboard {
    /// 写过的文本（按写入顺序，测试断言最后一项即可）。
    pub written: Vec<String>,
}

impl ClipboardText for MemoryClipboard {
    fn read_text(&mut self) -> Result<String, String> {
        Ok(self.written.last().cloned().unwrap_or_default())
    }

    fn write_text(&mut self, text: &str) -> Result<(), String> {
        self.written.push(text.to_owned());
        Ok(())
    }
}

/// 读剪贴板失败的说法。
fn read_error(error: &BevyClipboardError) -> String {
    match error {
        BevyClipboardError::ClipboardNotSupported => NO_CLIPBOARD.to_owned(),
        BevyClipboardError::ContentNotAvailable => NO_CLIPBOARD_TEXT.to_owned(),
        other => format!("Could not read the clipboard: {other}."),
    }
}

/// 写剪贴板失败的说法。
fn write_error(error: &BevyClipboardError) -> String {
    match error {
        BevyClipboardError::ClipboardNotSupported => NO_CLIPBOARD.to_owned(),
        other => format!("Could not write the clipboard: {other}."),
    }
}

/// 把 `text` 写进剪贴板，返回行数；空文本不算可复制的选区。
///
/// 行数用 `lines().count()`，与上游 `Selection::copied` 同一算法；文本本身
/// 已经由 [`Selection::selected_text`] 去掉末尾换行，所以不会多算一行。
pub fn copy_text(text: &str, clipboard: &mut impl ClipboardText) -> Result<usize, String> {
    if text.is_empty() {
        return Err(NOTHING_SELECTED.to_owned());
    }
    clipboard.write_text(text)?;
    Ok(text.lines().count())
}

/// 复制当前选区：取出文本、写剪贴板、记下行数（供 [`show_copied`] 显示）。
///
/// 失败时保留原选区、不记 `copied`，把可显示的错误交给调用方。
pub fn copy_selection(
    selection: &mut Selection,
    rows: &[Row],
    clipboard: &mut impl ClipboardText,
) -> Result<Copied, String> {
    let text = selection
        .selected_text(rows)
        .ok_or_else(|| NOTHING_SELECTED.to_owned())?;
    let lines = copy_text(&text, clipboard)?;
    selection.mark_copied(lines);
    Ok(Copied { lines })
}

/// Ctrl+V 读到的东西。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Paste {
    /// 剪贴板里的文本。
    Text(String),
    /// 剪贴板里的图片，已经存成这个文件；应用时在输入里打 `@path`。
    ///
    /// 现在没有产生它的入口（`bevy_clipboard` 未开 `image` feature），保留
    /// 变体是为了让将来接上图片读取时不用改 `apply_paste`/上层类型。
    Image(PathBuf),
}

/// 读剪贴板：只读文本，空剪贴板算错误。
///
/// 剪贴板里**只有图片**时 `bevy_clipboard` 的文本读取给
/// `ContentNotAvailable`，这里落成 [`NO_CLIPBOARD_TEXT`]（图片粘贴没接，
/// 原因见 [`Paste::Image`] 与模块文档）。
pub fn read_paste(clipboard: &mut impl ClipboardText) -> Result<Paste, String> {
    let text = clipboard.read_text()?;
    if text.is_empty() {
        return Err(NO_CLIPBOARD_TEXT.to_owned());
    }
    Ok(Paste::Text(text))
}

/// 把粘贴内容应用在光标处（上游 `type_pasted` 的核心）。
pub fn apply_paste(editor: &mut Editor, paste: Paste) {
    match paste {
        Paste::Text(text) => paste_text(editor, &text),
        Paste::Image(path) => type_path(editor, &path),
    }
}

/// 在光标处插入文本（上游 `paste_text` 的文本分支）。
///
/// 换行归一成 `\n` 由 [`Editor::insert`] 负责。上游还会把“整段都是图片
/// 路径”的粘贴拆成 `@path`；本 crate 还没有附件/拖放接线，这里只按文本插。
pub fn paste_text(editor: &mut Editor, text: &str) {
    if !text.is_empty() {
        editor.insert(text);
    }
}

/// 在光标处打一个 `@path` 引用（图片粘贴的落点，上游 `type_path`）。
///
/// 前面不是空白时先补一个空格，末尾也留一个空格，让它成为独立 token。
pub fn type_path(editor: &mut Editor, path: &Path) {
    let before = editor.text().get(..editor.cursor()).unwrap_or_default();
    let space = if before.is_empty() || before.ends_with(char::is_whitespace) {
        ""
    } else {
        " "
    };
    let path = path.to_string_lossy();
    editor.insert(&format!("{space}{} ", reference(&path)));
}

/// Ctrl+V 的文本入口：读剪贴板，成功才插入；失败给可显示的错误。
pub fn paste_from_clipboard(
    clipboard: &mut impl ClipboardText,
    editor: &mut Editor,
) -> Result<(), String> {
    let paste = read_paste(clipboard)?;
    apply_paste(editor, paste);
    Ok(())
}

/// Ctrl+V 的可注入入口：优先内存替身，其次系统剪贴板，都没有就报错。
///
/// 和 `selection::copy_released` 同一套优先级，所以单测/没有系统剪贴板的
/// 环境插入 [`MemoryClipboard`] 就能验证整条 Ctrl+V 链路。返回的错误已经
/// 是给人看的句子（[`NO_CLIPBOARD`] / [`NO_CLIPBOARD_TEXT`] / 读取失败），
/// 调用方直接写状态行即可。图片粘贴仍然不做：原因见 [`Paste::Image`]。
pub fn paste_from(
    editor: &mut Editor,
    system: Option<&mut Clipboard>,
    memory: Option<&mut MemoryClipboard>,
) -> Result<(), String> {
    match (memory, system) {
        (Some(memory), _) => paste_from_clipboard(memory, editor),
        (None, Some(system)) => paste_from_clipboard(system, editor),
        (None, None) => Err(NO_CLIPBOARD.to_owned()),
    }
}

/// `path` 写成 `@path`；含空白、引号、行号范围或以标点结尾时加引号。
///
/// 与上游 `rig_tools::attach::Reference::format` 的规则一致（这里只实现
/// 粘贴图片需要的那部分，免得为一行格式化引入整个工具 crate）。
fn reference(path: &str) -> String {
    /// 不加引号时会被当成句子标点的结尾字符。
    const TRAILING: [char; 10] = [',', '.', ';', ':', '!', '?', ')', '"', '\'', '`'];
    let ranged = path.rsplit_once(':').is_some_and(|(_, range)| {
        !range.is_empty() && range.chars().all(|c| c.is_ascii_digit() || c == '-')
    });
    let plain = !path.is_empty()
        && !path.starts_with('"')
        && !path.contains(char::is_whitespace)
        && !path.ends_with(TRAILING)
        && !ranged;
    match plain {
        true => format!("@{path}"),
        false => format!("@\"{}\"", path.replace('"', "\"\"")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::{ClickKind, Selection};
    use rig_tui::ratatui::text::Line;

    /// 内存剪贴板：读写都可注入错误，完全不碰系统剪贴板。
    #[derive(Default)]
    struct MockClipboard {
        content: Option<String>,
        written: Vec<String>,
        read_error: Option<String>,
        write_error: Option<String>,
    }

    impl ClipboardText for MockClipboard {
        fn read_text(&mut self) -> Result<String, String> {
            match &self.read_error {
                Some(why) => Err(why.clone()),
                None => Ok(self.content.clone().unwrap_or_default()),
            }
        }

        fn write_text(&mut self, text: &str) -> Result<(), String> {
            match &self.write_error {
                Some(why) => Err(why.clone()),
                None => {
                    self.written.push(text.to_owned());
                    Ok(())
                }
            }
        }
    }

    fn row(text: &str) -> Row {
        Row::new(Line::from(text.to_owned()))
    }

    fn transcript(lines: &[&str]) -> Vec<Row> {
        lines.iter().map(|line| row(line)).collect()
    }

    fn selected(rows: &[Row], from: (usize, usize), to: (usize, usize)) -> Selection {
        let mut selection = Selection::new();
        selection.press_kind(
            crate::panels::PanelId::Transcript,
            rows,
            from.0,
            from.1,
            ClickKind::Single,
        );
        selection.drag(
            crate::panels::PanelId::Transcript,
            rows,
            to.0,
            to.1,
        );
        selection
    }

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::default();
        editor.set(text);
        editor
    }

    // ---- 复制 ----

    #[test]
    fn copy_text_puts_the_text_on_the_clipboard() {
        let mut clipboard = MockClipboard::default();
        let lines = copy_text("hello", &mut clipboard).unwrap();
        assert_eq!(lines, 1);
        assert_eq!(clipboard.written, vec!["hello".to_owned()]);
    }

    #[test]
    fn copy_text_counts_lines() {
        let mut clipboard = MockClipboard::default();
        let lines = copy_text("a\nb\nc", &mut clipboard).unwrap();
        assert_eq!(lines, 3);
    }

    #[test]
    fn copy_text_refuses_empty_text() {
        let mut clipboard = MockClipboard::default();
        assert_eq!(
            copy_text("", &mut clipboard),
            Err(NOTHING_SELECTED.to_owned())
        );
        assert!(clipboard.written.is_empty(), "失败时不写剪贴板");
    }

    #[test]
    fn copy_selection_writes_the_extracted_text_and_remembers_the_lines() {
        let rows = transcript(&["hello", "world"]);
        let mut selection = selected(&rows, (0, 2), (1, 2));
        let mut clipboard = MockClipboard::default();
        let copied = copy_selection(&mut selection, &rows, &mut clipboard).unwrap();
        assert_eq!(copied, Copied { lines: 2 });
        assert_eq!(clipboard.written, vec!["llo\nwo".to_owned()]);
        assert_eq!(selection.copied(), Some(2));
    }

    #[test]
    fn copy_selection_without_a_selection_is_an_error() {
        let rows = transcript(&["hello"]);
        let mut selection = Selection::new();
        let mut clipboard = MockClipboard::default();
        assert_eq!(
            copy_selection(&mut selection, &rows, &mut clipboard),
            Err(NOTHING_SELECTED.to_owned())
        );
        assert!(clipboard.written.is_empty());
        assert_eq!(selection.copied(), None);
    }

    #[test]
    fn a_write_failure_is_a_displayable_error() {
        let rows = transcript(&["hello"]);
        let mut selection = selected(&rows, (0, 0), (0, 5));
        let mut clipboard = MockClipboard {
            write_error: Some("the clipboard is busy".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            copy_selection(&mut selection, &rows, &mut clipboard),
            Err("the clipboard is busy".to_owned())
        );
        assert!(clipboard.written.is_empty());
        assert_eq!(selection.copied(), None, "复制失败不显示 Copied");
    }

    #[test]
    fn copied_message_follows_the_upstream_wording() {
        assert_eq!(copied_message(1), "Copied 1 line");
        assert_eq!(copied_message(2), "Copied 2 lines");
        assert_eq!(Copied { lines: 3 }.message(), "Copied 3 lines");
    }

    // ---- 粘贴 ----

    #[test]
    fn paste_text_inserts_at_the_cursor() {
        let mut editor = editor("hi");
        editor.set_cursor(2);
        paste_text(&mut editor, " there");
        assert_eq!(editor.text(), "hi there");
        assert_eq!(editor.cursor(), 8);
    }

    #[test]
    fn paste_text_normalizes_windows_newlines() {
        let mut editor = editor("");
        paste_text(&mut editor, "a\r\nb\rc");
        assert_eq!(editor.text(), "a\nb\nc");
    }

    #[test]
    fn paste_from_clipboard_reads_then_inserts() {
        let mut editor = editor("say: ");
        let mut clipboard = MockClipboard {
            content: Some("hello".to_owned()),
            ..Default::default()
        };
        assert_eq!(paste_from_clipboard(&mut clipboard, &mut editor), Ok(()));
        assert_eq!(editor.text(), "say: hello");
    }

    #[test]
    fn pasting_an_empty_clipboard_is_an_error() {
        let mut editor = editor("keep");
        let mut clipboard = MockClipboard {
            content: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(
            paste_from_clipboard(&mut clipboard, &mut editor),
            Err(NO_CLIPBOARD_TEXT.to_owned())
        );
        assert_eq!(editor.text(), "keep", "空剪贴板不动输入");
    }

    #[test]
    fn a_read_failure_is_a_displayable_error() {
        let mut editor = editor("keep");
        let mut clipboard = MockClipboard {
            read_error: Some("no clipboard backend".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            paste_from_clipboard(&mut clipboard, &mut editor),
            Err("no clipboard backend".to_owned())
        );
        assert_eq!(editor.text(), "keep");
    }

    #[test]
    fn read_paste_returns_the_clipboard_text() {
        let mut clipboard = MockClipboard {
            content: Some("text".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            read_paste(&mut clipboard),
            Ok(Paste::Text("text".to_owned()))
        );
    }

    // ---- 图片粘贴的语义（只到 @path，没有取图入口） ----

    #[test]
    fn applying_an_image_paste_types_a_quoted_reference() {
        let mut editor = editor("see");
        apply_paste(&mut editor, Paste::Image(PathBuf::from("/tmp/shot 1.png")));
        assert_eq!(
            editor.text(),
            "see @\"/tmp/shot 1.png\" ",
            "含空白的路径加引号，前后留空格"
        );
    }

    #[test]
    fn a_plain_image_path_needs_no_quotes() {
        let mut editor = editor("");
        apply_paste(&mut editor, Paste::Image(PathBuf::from("/tmp/a.png")));
        assert_eq!(editor.text(), "@/tmp/a.png ");
    }

    // ---- 状态行接线 ----

    #[test]
    fn show_copied_writes_the_status_item() {
        let mut app = App::new();
        app.init_resource::<Selection>()
            .init_resource::<AppStatus>()
            .add_systems(Update, show_copied);
        let mut selection = app.world_mut().resource_mut::<Selection>();
        selection.mark_copied(1);
        app.update();
        let status = app.world().resource::<AppStatus>();
        let item = status
            .0
            .iter()
            .find(|item| item.name == COPIED.name)
            .expect("状态行显示 Copied");
        assert_eq!(item.text, "Copied 1 line");

        app.world_mut().resource_mut::<Selection>().clear();
        app.update();
        let status = app.world().resource::<AppStatus>();
        assert!(
            !status.0.iter().any(|item| item.name == COPIED.name),
            "清除选区后提示消失"
        );
    }
}
