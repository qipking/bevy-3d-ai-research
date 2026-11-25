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
//! - 粘贴附件：上游是“把剪贴板图片存成文件，再在输入里打 `@path`”，
//!   再把“粘贴/拖入的文本里的图片文件路径”也变成 `@path`。这里做后一半：
//!   [`image_paths`] 识别文本里的图片路径；**整段都是图片路径**时
//!   [`attach_image_paths`] 把它们换成 `@path` 列表（混有普通文字就原样
//!   保留，和上游一致），[`paste_text`] 插入替换后的文本；窗口拖入由应用层
//!   调 [`type_path`]（先经 [`dropped_file_text`] 判断是不是图片）。
//! - 剪贴板图片：`bevy_clipboard` 的 `image` feature 没有开，也没有临时文件
//!   存放约定，所以这里保留 [`Paste::Image`] 与 `@path` 的**应用语义**（
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
//! [`PASTE_FAILED`] 状态行。文本里的图片路径变成 `@path`；剪贴板里**只有
//! 图片**时仍然没有入口，原因见上。

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

/// 文本里能被当成附件的图片扩展名（小写比较）。
const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif"];

/// `text` 里的图片文件路径，按出现顺序返回。
///
/// 分词照 shell 的习惯：空白分隔；单/双引号里的空白算路径的一部分，引号本身
/// 去掉；反斜杠转义下一个字符（Windows 上反斜杠是路径分隔符，不转义）。
/// 每个词再经过 [`image_path`]：普通本地路径，或 host 为空/`localhost` 的
/// `file://` URL（百分号解码）；**只看扩展名**，不要求文件此刻存在。
///
/// 已经带 `@` 的词跳过，免得把 `@/tmp/a.png` 再包一层。
pub fn image_paths(text: &str) -> Vec<PathBuf> {
    scan_words(text)
        .into_iter()
        .filter_map(|word| image_path(&word.text))
        .collect()
}

/// 把**整段都是图片路径**的 `text` 换成对应的 `@path` 列表。
///
/// 与上游一致：只有每个词（空白分隔）都是图片文件时才算"粘贴了附件"；
/// 只要混进一个普通词（`convert a.png b.png out.pdf`、`look at /tmp/a.png`）
/// 就返回 `None`，由调用方原样插入——不能把命令/句子里顺带提到的路径改掉。
/// 路径之间的空白（含换行）原样保留，含空白的路径按 [`reference`] 加引号。
pub fn attach_image_paths(text: &str) -> Option<String> {
    let words = scan_words(text);
    if words.is_empty() {
        return None;
    }
    // 先全部识别：有一个词不是图片路径就不算纯附件粘贴。
    let paths: Vec<PathBuf> = words
        .iter()
        .map(|word| image_path(&word.text))
        .collect::<Option<_>>()?;
    let mut attached = String::with_capacity(text.len());
    let mut copied = 0;
    for (word, path) in words.iter().zip(paths) {
        attached.push_str(&text[copied..word.start]);
        attached.push_str(&reference(&path.to_string_lossy()));
        copied = word.end;
    }
    attached.push_str(&text[copied..]);
    Some(attached)
}

/// 拖进窗口的文件若是一张图片，返回要插进输入框的 `@path` 引用。
///
/// 只看扩展名（[`IMAGE_EXTENSIONS`]），不查文件是否存在；非图片返回 `None`，
/// 调用方因此可以原样忽略它。引用用 [`reference`] 格式化（含空白的路径加
/// 引号），窗口拖入本身走 [`type_path`] 补前导/末尾空格。
pub fn dropped_file_text(path: &Path) -> Option<String> {
    is_image_path(path).then(|| reference(&path.to_string_lossy()))
}

/// 一个 shell 风格分词后的词，以及它在原文里的字节范围（含引号）。
struct Word {
    /// 词在原文里的起点。
    start: usize,
    /// 词在原文里的终点（不含）。
    end: usize,
    /// 去掉引号、处理转义后的词本身。
    text: String,
}

/// 按空白/引号切词，保留每个词在原文里的范围。
fn scan_words(text: &str) -> Vec<Word> {
    let mut words = Vec::new();
    let mut word: Option<(usize, String)> = None;
    let mut quote: Option<char> = None;
    let mut chars = text.char_indices();
    while let Some((index, character)) = chars.next() {
        match (quote, character) {
            // 结束引号：不进入词本身，但它的位置仍属于这个词的原文范围。
            (Some(open), character) if character == open => quote = None,
            (Some(_), character) => {
                word.get_or_insert_with(|| (index, String::new()))
                    .1
                    .push(character);
            }
            // 开始引号：引号本身不进词，词从引号处开始，范围会连引号一起替换。
            (None, '\'' | '"') => {
                quote = Some(character);
                word.get_or_insert_with(|| (index, String::new()));
            }
            (None, '\\') if !cfg!(windows) => {
                if let Some((_, escaped)) = chars.next() {
                    word.get_or_insert_with(|| (index, String::new()))
                        .1
                        .push(escaped);
                }
            }
            (None, character) if character.is_whitespace() => {
                if let Some((start, value)) = word.take() {
                    words.push(Word {
                        start,
                        end: index,
                        text: value,
                    });
                }
            }
            (None, character) => {
                word.get_or_insert_with(|| (index, String::new()))
                    .1
                    .push(character);
            }
        }
    }
    if let Some((start, value)) = word {
        words.push(Word {
            start,
            end: text.len(),
            text: value,
        });
    }
    words
}

/// 一个词指向的图片文件；不是本地图片路径就 `None`。
fn image_path(part: &str) -> Option<PathBuf> {
    if part.is_empty() || part.starts_with('@') {
        return None;
    }
    let path = match part.strip_prefix("file://") {
        Some(url) => PathBuf::from(local_url_path(url)?),
        // 别的 scheme（http/https/...）不是本机文件，不猜成本地路径。
        None if part.contains("://") => return None,
        None => PathBuf::from(part),
    };
    is_image_path(&path).then_some(path)
}

/// `file://` 后面的本地路径；别的 host 指向本机读不到的文件，返回 `None`。
fn local_url_path(url: &str) -> Option<String> {
    let path = url.strip_prefix("localhost").unwrap_or(url);
    path.starts_with('/')
        .then(|| percent_decoded(path))
        .flatten()
}

/// 把 `%XX` 还原成对应字节；结果不是合法 UTF-8 时 `None`。
fn percent_decoded(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        let hex = after
            .get(..2)
            .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (byte, hex) {
            (b'%', Some(decoded)) => {
                bytes.push(decoded);
                rest = after.get(2..).unwrap_or_default();
            }
            _ => {
                bytes.push(byte);
                rest = after;
            }
        }
    }
    String::from_utf8(bytes).ok()
}

/// 路径的扩展名是不是 [`IMAGE_EXTENSIONS`]（大小写不敏感）。
fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|image| extension.eq_ignore_ascii_case(image))
        })
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
/// 换行归一成 `\n` 由 [`Editor::insert`] 负责。**整段都是图片路径**时先被
/// [`attach_image_paths`] 换成 `@path` 列表；混有普通文字时原样插入（与上游
/// 一致，不把命令/句子里顺带提到的路径改掉）。附件若落在词中间（光标前不是
/// 空白），先补一个空格，保证 `@` 在词首。
pub fn paste_text(editor: &mut Editor, text: &str) {
    if text.is_empty() {
        return;
    }
    match attach_image_paths(text) {
        Some(attached) => {
            let before = editor
                .text()
                .get(..editor.cursor())
                .unwrap_or_default()
                .to_owned();
            // 附件文本开头的空白已经把它和前面的词隔开了；没有就补一个。
            let attached = if attached.starts_with(char::is_whitespace) {
                attached
            } else {
                format!("{}{attached}", word_break(&before))
            };
            editor.insert(&attached);
        }
        None => editor.insert(text),
    }
}

/// `before` 之后写一个 `@path` 需要的分隔空格：前面不是空白（也不是文本
/// 开头）就补一个，保证引用从词首开始。
fn word_break(before: &str) -> &'static str {
    if before.is_empty() || before.ends_with(char::is_whitespace) {
        ""
    } else {
        " "
    }
}

/// 在光标处打一个 `@path` 引用（图片粘贴/拖入的落点，上游 `type_path`）。
///
/// 前面不是空白时先补一个空格，末尾也留一个空格，让它成为独立 token；含空白
/// 的路径由 [`reference`] 加引号。
pub fn type_path(editor: &mut Editor, path: &Path) {
    let before = editor
        .text()
        .get(..editor.cursor())
        .unwrap_or_default()
        .to_owned();
    let path = path.to_string_lossy();
    editor.insert(&format!("{}{} ", word_break(&before), reference(&path)));
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

    // ---- 文本里的图片路径 -> @path ----

    #[test]
    fn image_paths_recognizes_plain_and_quoted_paths() {
        assert_eq!(
            image_paths("/tmp/x.png"),
            vec![PathBuf::from("/tmp/x.png")]
        );
        assert_eq!(
            image_paths("'/tmp/x.png'"),
            vec![PathBuf::from("/tmp/x.png")],
            "单引号只是分词用的，不进路径"
        );
        assert_eq!(
            image_paths("\"/tmp/a b.png\""),
            vec![PathBuf::from("/tmp/a b.png")],
            "双引号里的空白属于路径"
        );
    }

    #[test]
    fn file_urls_are_percent_decoded() {
        assert_eq!(
            image_paths("file:///tmp/a%20b.png"),
            vec![PathBuf::from("/tmp/a b.png")]
        );
        assert_eq!(
            image_paths("file://localhost/tmp/a.png"),
            vec![PathBuf::from("/tmp/a.png")],
            "空 host / localhost 才是本机路径"
        );
        assert!(
            image_paths("file://elsewhere/tmp/a.png").is_empty(),
            "别的 host 不猜成本机路径"
        );
        assert!(
            image_paths("https://example.com/a.png").is_empty(),
            "别的 scheme 不是本机路径"
        );
    }

    #[test]
    fn image_paths_returns_every_image_in_order() {
        assert_eq!(
            image_paths("a.png and b.JPG and c.txt"),
            vec![PathBuf::from("a.png"), PathBuf::from("b.JPG")],
            "扩展名大小写不敏感，非图片不进来"
        );
    }

    #[test]
    fn an_existing_attachment_is_not_attached_again() {
        assert_eq!(
            image_paths("@/tmp/x.png"),
            Vec::<PathBuf>::new(),
            "已经是 @path 的词不再包一层"
        );
        assert_eq!(attach_image_paths("@/tmp/x.png"), None);
    }

    #[test]
    fn non_image_paths_are_not_attached() {
        assert!(image_paths("/tmp/x.txt").is_empty());
        assert_eq!(attach_image_paths("look at /tmp/x.txt"), None);
    }

    #[test]
    fn attach_image_paths_converts_only_whole_image_lists() {
        assert_eq!(
            attach_image_paths("/tmp/a.png /tmp/b.jpg"),
            Some("@/tmp/a.png @/tmp/b.jpg".to_owned()),
            "整段都是图片路径：每个路径各自变成 @path"
        );
        assert_eq!(
            attach_image_paths("convert a.png b.png out.pdf"),
            None,
            "混有普通文件（命令）时保持原样，不能改中间的图片路径"
        );
        assert_eq!(
            attach_image_paths("look at /tmp/x.png ok"),
            None,
            "图片路径夹在句子里时也不转（与上游一致）"
        );
    }

    #[test]
    fn attach_image_paths_keeps_line_breaks_and_quotes_spaces() {
        assert_eq!(
            attach_image_paths("/tmp/a.png\n/tmp/b.jpg"),
            Some("@/tmp/a.png\n@/tmp/b.jpg".to_owned()),
            "多行纯附件粘贴保持换行，附件各在自己的行"
        );
        assert_eq!(
            attach_image_paths("'file:///tmp/a%20b.png'"),
            Some("@\"/tmp/a b.png\"".to_owned()),
            "百分号解码后有空白，@path 加引号"
        );
    }

    #[test]
    fn pasting_a_text_with_an_image_path_attaches_it() {
        let mut editor = editor("look: ");
        let mut clipboard = MockClipboard {
            content: Some("/tmp/x.png".to_owned()),
            ..Default::default()
        };
        assert_eq!(paste_from_clipboard(&mut clipboard, &mut editor), Ok(()));
        assert_eq!(editor.text(), "look: @/tmp/x.png");
    }

    #[test]
    fn pasting_mixed_text_leaves_image_paths_alone() {
        let mut editor = editor("");
        let mut clipboard = MockClipboard {
            content: Some("convert a.png b.png out.pdf".to_owned()),
            ..Default::default()
        };
        assert_eq!(paste_from_clipboard(&mut clipboard, &mut editor), Ok(()));
        assert_eq!(
            editor.text(),
            "convert a.png b.png out.pdf",
            "命令里的 a.png/b.png 不能被改成 @path"
        );
    }

    #[test]
    fn pasting_an_image_path_at_a_word_end_keeps_it_a_token() {
        let mut editor = editor("hi");
        let mut clipboard = MockClipboard {
            content: Some("/tmp/a.png".to_owned()),
            ..Default::default()
        };
        assert_eq!(paste_from_clipboard(&mut clipboard, &mut editor), Ok(()));
        assert_eq!(
            editor.text(),
            "hi @/tmp/a.png",
            "光标在词尾粘贴路径：先补空格，@ 必须在词首"
        );
    }

    #[test]
    fn dropped_file_text_only_formats_images() {
        assert_eq!(
            dropped_file_text(Path::new("/tmp/a.png")),
            Some("@/tmp/a.png".to_owned())
        );
        assert_eq!(
            dropped_file_text(Path::new("/tmp/a b.png")),
            Some("@\"/tmp/a b.png\"".to_owned()),
            "带空格的路径必须加引号，否则 rig 会在第一个空白处截断"
        );
        assert_eq!(
            dropped_file_text(Path::new("/tmp/a.WEBP")),
            Some("@/tmp/a.WEBP".to_owned()),
            "扩展名大小写不敏感"
        );
        assert_eq!(dropped_file_text(Path::new("/tmp/notes.txt")), None);
        assert_eq!(dropped_file_text(Path::new("/tmp/no-extension")), None);
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
